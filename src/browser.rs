//! Opening the served address in a browser.

use std::io::ErrorKind;
use std::path::is_separator;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// How soon a browser that could not open stops. One that stops later was
/// open, and however it stops then is nobody's business here.
const GIVES_UP_WITHIN: Duration = Duration::from_secs(2);

/// How long to wait for a browser to fall over when there is another to try.
/// Long enough for a locked profile or an unknown flag to be refused, short
/// enough not to keep anyone waiting.
const STUMBLES_WITHIN: Duration = Duration::from_millis(300);

/// How often to look while waiting for that.
const A_MOMENT: Duration = Duration::from_millis(20);

/// Opens `url` in a browser `BROWSER` names, or in the usual one. A browser
/// that will not open is worth a line, not a stop: the banner has the address.
///
/// On a thread of its own, so that trying a list never holds up the server.
pub(crate) fn open(url: &str) {
    let url = url.to_string();
    std::thread::spawn(move || open_it(&url));
}

/// Whatever `BROWSER` names, tried in turn, or the usual browser.
fn open_it(url: &str) {
    let named = std::env::var("BROWSER").unwrap_or_default();
    let chosen: Vec<&str> = browsers(&named).collect();
    if chosen.is_empty() {
        if let Err(error) = open::that_detached(url) {
            eprintln!("  Cannot open a browser: {}", why(None, &error));
        }
        return;
    }

    // Each in turn, and the first that opens the address wins.
    let mut refused = Vec::new();
    for (at, browser) in chosen.iter().enumerate() {
        let (program, arguments) = command_line(browser, url);
        match run(&program, &arguments, at + 1 < chosen.len()) {
            Ok(()) => return,
            Err(refusal) => refused.push(refusal),
        }
    }

    eprintln!("  Cannot open a browser: {}", why_none_of_them(&refused));
}

/// The browsers `BROWSER` names, in the order to try them. It holds a list,
/// each entry a whole command line, kept apart by the character this system
/// puts between paths.
fn browsers(named: &str) -> impl Iterator<Item = &str> {
    let between_paths = if cfg!(windows) { ';' } else { ':' };

    named
        .split(between_paths)
        .map(str::trim)
        .filter(|browser| !browser.is_empty())
}

/// Starts the browser and leaves it running, or says why it opened nothing.
///
/// With `another` to try, this waits a moment first: a browser that stops at
/// once opened nothing, and the next one still can. The last one is left to
/// the thread below, there being nothing to fall back on.
fn run(program: &str, arguments: &[String], another: bool) -> Result<(), String> {
    let mut command = Command::new(program);
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    // Kept apart from the server, so Ctrl-C stops the server and not the
    // browser.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    let mut browser = command
        .spawn()
        .map_err(|error| why(Some(program), &error))?;
    let started = Instant::now();
    let program = program.to_string();

    if another && stumbled(&mut browser) {
        return Err(format!("{program} stopped with an error"));
    }

    // Waited on from a thread: unwaited, a closed browser lingers in the
    // system's list of processes until the server stops, and waiting here
    // would stall until the browser is closed.
    std::thread::spawn(move || {
        let Ok(status) = browser.wait() else {
            return;
        };
        // A wrong flag, a profile that is locked: the browser says so by
        // stopping at once, and the terminal is the only place to hear it.
        if !status.success() && started.elapsed() < GIVES_UP_WITHIN {
            eprintln!("  Cannot open a browser: {program} stopped with an error");
        }
    });

    Ok(())
}

/// True when the browser stops with an error within the moment: a wrong flag,
/// a locked profile. One still running, or one that stopped happily after
/// handing the address to a window already open, did its job.
fn stumbled(browser: &mut Child) -> bool {
    let waited = Instant::now();

    while waited.elapsed() < STUMBLES_WITHIN {
        match browser.try_wait() {
            Ok(Some(status)) => return !status.success(),
            Ok(None) => std::thread::sleep(A_MOMENT),
            Err(_) => return false,
        }
    }

    false
}

/// The program and its arguments, with `%s` standing for the address as it
/// does elsewhere. Without a `%s`, the address goes last.
fn command_line(browser: &str, url: &str) -> (String, Vec<String>) {
    let mut words = words(browser)
        .into_iter()
        .map(|word| word.replace("%s", url));

    let program = words.next().unwrap_or_default();
    let mut arguments: Vec<String> = words.collect();
    if !browser.contains("%s") {
        arguments.push(url.to_string());
    }

    (program, arguments)
}

/// The words of a command line. Whitespace parts them; a quote, anywhere in a
/// word, holds everything up to its match together; outside quotes `\` holds
/// the character after it. Two ways to write a space, in a name or in
/// `--profile-directory="Profile 1"`. As in a shell, a name with an apostrophe
/// in it has to be quoted whole or written with `\'`.
///
/// On Windows `\` parts directories, so it stands for itself there and quotes
/// are the one way to hold a name together.
fn words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut quote = None;
    let mut characters = line.chars();

    while let Some(character) = characters.next() {
        match (quote, character) {
            (Some(open), _) if character == open => quote = None,
            (Some(_), _) => word.push(character),
            (None, '\\') if !is_separator('\\') => {
                if let Some(held) = characters.next() {
                    word.push(held);
                    started = true;
                }
            }
            (None, '"' | '\'') => {
                quote = Some(character);
                started = true;
            }
            (None, _) if character.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            (None, _) => {
                word.push(character);
                started = true;
            }
        }
    }

    if started {
        words.push(word);
    }

    words
}

/// Why no browser opened, in plain words.
fn why(program: Option<&str>, error: &std::io::Error) -> String {
    match (program, error.kind()) {
        (Some(program), ErrorKind::NotFound) => format!("there is no program called {program}"),
        (None, ErrorKind::NotFound) => "nothing on this machine opens web addresses".to_string(),
        (_, ErrorKind::PermissionDenied) => {
            "the system will not let this program run it".to_string()
        }
        // The launcher's account of a browser that refused is a line of
        // debugging output, no use to anyone reading a terminal.
        (_, ErrorKind::Other) => "the browser would not open it".to_string(),
        _ => error.to_string(),
    }
}

/// Why none of the browsers named opened the address. One is explained;
/// several would be a paragraph, so they are counted.
fn why_none_of_them(refused: &[String]) -> String {
    match refused {
        [only] => only.clone(),
        many => format!(
            "none of the {} browsers BROWSER names would run",
            many.len()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "http://localhost:3030";

    #[test]
    fn a_browser_on_its_own_is_given_the_address() {
        let (program, arguments) = command_line("firefox", URL);

        assert_eq!(program, "firefox");
        assert_eq!(arguments, [URL]);
    }

    #[test]
    fn a_browser_with_arguments_has_the_address_put_where_it_says() {
        let (program, arguments) = command_line("firefox --new-window %s", URL);

        assert_eq!(program, "firefox");
        assert_eq!(arguments, ["--new-window", URL]);
    }

    #[test]
    fn a_browser_with_arguments_and_no_place_for_the_address_takes_it_last() {
        let (program, arguments) = command_line("chromium --incognito", URL);

        assert_eq!(program, "chromium");
        assert_eq!(arguments, ["--incognito", URL]);
    }

    #[test]
    fn quotes_hold_a_name_with_a_space_together() {
        let (program, arguments) = command_line(
            "'/Applications/Firefox.app/Contents/MacOS/firefox' -P work %s",
            URL,
        );

        assert_eq!(program, "/Applications/Firefox.app/Contents/MacOS/firefox");
        assert_eq!(arguments, ["-P", "work", URL]);
    }

    #[test]
    fn quotes_hold_a_space_inside_a_word_together() {
        // Quotes used to count only at the start of a word, and this came
        // apart into a wrong flag and a stray argument.
        let (program, arguments) =
            command_line(r#"chromium --profile-directory="Profile 1" %s"#, URL);

        assert_eq!(program, "chromium");
        assert_eq!(arguments, ["--profile-directory=Profile 1", URL]);
    }

    #[test]
    fn an_apostrophe_in_a_name_is_held_back_by_quoting_the_name() {
        let (program, arguments) = command_line(r#""/opt/o'brien/browser" %s"#, URL);

        assert_eq!(program, "/opt/o'brien/browser");
        assert_eq!(arguments, [URL]);
    }

    #[cfg(not(windows))]
    #[test]
    fn a_backslash_holds_a_space_in_a_name_too() {
        // The other way of writing it, and the one a shell teaches.
        let (program, arguments) = command_line(r"/opt/my\ browser/firefox %s", URL);

        assert_eq!(program, "/opt/my browser/firefox");
        assert_eq!(arguments, [URL]);
    }

    #[cfg(windows)]
    #[test]
    fn a_backslash_parts_directories_here_rather_than_holding_a_name() {
        // Reading it as an escape turned `C:\Program Files\...` into
        // `C:Program`, and no browser opened.
        let (program, arguments) = command_line(r"C:\Program Files\Firefox\firefox.exe %s", URL);

        assert_eq!(program, r"C:\Program");
        assert_eq!(arguments, [r"Files\Firefox\firefox.exe", URL]);

        // Quoted, it is one name, which is the way to write it there.
        let (program, arguments) =
            command_line(r#""C:\Program Files\Firefox\firefox.exe" %s"#, URL);

        assert_eq!(program, r"C:\Program Files\Firefox\firefox.exe");
        assert_eq!(arguments, [URL]);
    }

    #[test]
    fn a_list_of_browsers_is_read_as_a_list() {
        // The convention: several, to be tried in turn, parted by the
        // character this system puts between paths.
        let named = if cfg!(windows) {
            "firefox;chromium --incognito"
        } else {
            "firefox:chromium --incognito"
        };

        assert_eq!(
            browsers(named).collect::<Vec<_>>(),
            ["firefox", "chromium --incognito"]
        );
        assert!(browsers("  ").next().is_none(), "nothing named is no list");
    }

    #[test]
    fn one_browser_that_will_not_run_is_named_and_several_are_counted() {
        let missing = |program| why(Some(program), &std::io::Error::from(ErrorKind::NotFound));

        assert_eq!(
            why_none_of_them(&[missing("firefox")]),
            "there is no program called firefox"
        );
        assert_eq!(
            why_none_of_them(&[missing("firefox"), missing("chromium")]),
            "none of the 2 browsers BROWSER names would run"
        );
    }
}
