//! Opening the served address in a browser.

use std::io::ErrorKind;
use std::process::{Command, Stdio};

/// Opens `url` in the browser `BROWSER` names, or the usual one. A browser
/// that will not open is worth a line, not a stop: the banner has the address.
pub(crate) fn open(url: &str) {
    let chosen = std::env::var("BROWSER")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let asked_for = chosen.as_deref().map(|browser| command_line(browser, url));

    let opened = match &asked_for {
        Some((program, arguments)) => run(program, arguments),
        None => open::that_detached(url),
    };

    if let Err(error) = opened {
        let program = asked_for.as_ref().map(|(program, _)| program.as_str());
        eprintln!("  Cannot open a browser: {}", why(program, &error));
    }
}

/// Starts the browser and leaves it running.
fn run(program: &str, arguments: &[String]) -> std::io::Result<()> {
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

    let mut browser = command.spawn()?;

    // Waited on from a thread: unwaited, a closed browser lingers in the
    // system's list of processes until the server stops, and waiting here
    // would stall on a browser that stays until it is closed.
    std::thread::spawn(move || {
        let _ = browser.wait();
    });

    Ok(())
}

/// The program and its arguments, with `%s` standing for the address as it
/// does elsewhere. Without a `%s`, the address goes last.
fn command_line(browser: &str, url: &str) -> (String, Vec<String>) {
    let mut words = browser
        .split_whitespace()
        .map(|word| word.replace("%s", url));

    let program = words.next().unwrap_or_default();
    let mut arguments: Vec<String> = words.collect();
    if !browser.contains("%s") {
        arguments.push(url.to_string());
    }

    (program, arguments)
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

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "http://localhost:3030";

    #[test]
    fn a_program_on_its_own_is_handed_the_address() {
        assert_eq!(
            command_line("firefox", URL),
            ("firefox".to_string(), vec![URL.to_string()])
        );
    }

    #[test]
    fn a_command_line_keeps_its_arguments() {
        // Handed to the system whole, this named no program and opened
        // nothing.
        assert_eq!(
            command_line("firefox --new-window %s", URL),
            (
                "firefox".to_string(),
                vec!["--new-window".to_string(), URL.to_string()]
            )
        );
    }

    #[test]
    fn arguments_without_a_place_for_the_address_still_get_it_last() {
        assert_eq!(
            command_line("chromium --incognito", URL),
            (
                "chromium".to_string(),
                vec!["--incognito".to_string(), URL.to_string()]
            )
        );
    }
}
