use axum::Router;
use axum::extract::Request;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use clap::Parser;
use http::{HeaderName, HeaderValue, StatusCode, header};
use notify_debouncer_full::notify::event::{AccessKind, AccessMode};
use notify_debouncer_full::notify::{EventKind, RecursiveMode, event::ModifyKind};
use notify_debouncer_full::{DebounceEventResult, DebouncedEvent, new_debouncer};
use std::fmt::Display;
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;
use tower_http::compression::CompressionLayer;
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_livereload::LiveReloadLayer;

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "HTTP server for static files, with live reload, for local development"
)]
struct Args {
    /// Port to listen on [default: 3030, or the next free one]
    #[arg(short, long)]
    port: Option<u16>,

    /// Address to listen on (use 0.0.0.0 to reach this server from other devices)
    #[arg(long, default_value_t = IpAddr::V4(Ipv4Addr::LOCALHOST))]
    host: IpAddr,

    /// Directory to serve
    #[arg(short, long, default_value = ".")]
    dir: PathBuf,

    /// Serve index.html when the address matches no file, for single-page apps
    #[arg(long)]
    spa: bool,

    /// Do not watch for changes, and do not refresh the browser
    #[arg(long)]
    no_reload: bool,

    /// Let the browser keep files under /assets/ for a year, for a published site
    #[arg(long)]
    cache_assets: bool,
}

const BLUE: &str = "\x1b[94m";
const RESET: &str = "\x1b[0m";
const LINK_START: &str = "\x1b]8;;";
const LINK_END: &str = "\x1b]8;;\x1b\\";
const LINK_MID: &str = "\x1b\\";
const RULE: &str = "-----------------------------------------------";
/// Fits the longest label, so the colons line up.
const LABEL_WIDTH: usize = 15;

/// Where to listen when nothing else is asked for.
const DEFAULT_PORT: u16 = 3030;

/// How many ports to step through before giving up. Enough for a few servers
/// left running, few enough that the address stays easy to remember.
const PORTS_TO_TRY: u16 = 10;

/// Long enough to group the writes one save makes, short enough that the
/// refresh still feels immediate.
const DEBOUNCE_DELAY: Duration = Duration::from_millis(200);

/// How often to look at the served directory to see whether it is still the
/// one being watched, and how long to wait between attempts to watch it again.
const CHECK_INTERVAL: Duration = Duration::from_millis(100);

const INDEX_FILE: &str = "index.html";

/// The one hidden directory the web actually uses.
const WELL_KNOWN: &str = ".well-known";

/// Where a build puts files whose name changes with their contents, so the
/// browser can keep them for as long as it likes.
const ASSETS: &str = "/assets/";

/// A year, the longest any browser is asked to keep a file.
const KEEP_FOR_A_YEAR: &str = "public, max-age=31536000, immutable";

/// Directories whose contents should never trigger a browser reload. Hidden
/// ones, `.git` and `.svelte-kit` among them, are covered by [`is_hidden`].
const IGNORED_DIRS: &[&str] = &["target", "node_modules"];

/// Scratch files editors leave behind: vim swap files, backup copies.
const IGNORED_SUFFIXES: &[&str] = &["~", ".tmp", ".swp", ".swx", ".swo"];

/// vim writes `4913` to test whether a directory accepts writes.
const IGNORED_NAMES: &[&str] = &["4913"];

/// Why the watcher has to be set up again. Both mean changes were missed;
/// they differ only in what to call it.
enum Rewatch {
    /// The name now leads to a different directory: a build deleted the old
    /// one, or renamed it away.
    Replaced,
    /// The watcher itself failed, dropping whatever it had not reported.
    WatchFailed,
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("servio: {error}");
        std::process::exit(1);
    }
}

/// Takes the port that was asked for. Asking for one and not getting it is an
/// error: something else expects that number. Asking for none steps up from
/// 3030 until a free port turns up, since the banner says which one it is.
async fn listen(host: IpAddr, port: Option<u16>) -> Result<TcpListener, std::io::Error> {
    if let Some(port) = port {
        return match TcpListener::bind(SocketAddr::new(host, port)).await {
            Err(error) if error.kind() == ErrorKind::AddrInUse => {
                Err(in_use(&format!("port {port} is already in use")))
            }
            listener => listener,
        };
    }

    for port in DEFAULT_PORT..DEFAULT_PORT + PORTS_TO_TRY {
        match TcpListener::bind(SocketAddr::new(host, port)).await {
            Ok(listener) => return Ok(listener),
            Err(error) if error.kind() == ErrorKind::AddrInUse => continue,
            Err(error) => return Err(error),
        }
    }

    Err(in_use(&format!(
        "ports {DEFAULT_PORT} to {} are all in use",
        DEFAULT_PORT + PORTS_TO_TRY - 1
    )))
}

/// The one thing to do about a busy port, said once.
fn in_use(what: &str) -> std::io::Error {
    std::io::Error::other(format!("{what} — choose another one with --port"))
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let static_dir = resolve_dir(args.dir)?;

    // A missing path already failed in resolve_dir; this catches a file.
    if !static_dir.is_dir() {
        return Err(format!("{} is not a directory", static_dir.display()).into());
    }

    let index = static_dir.join(INDEX_FILE);
    let mut app = Router::new().fallback_service(ServeDir::new(&static_dir));

    if args.spa {
        // Goes inside the live reload layer, so its page gets the script.
        let index = index.clone();
        app = app.layer(middleware::from_fn(move |request, next| {
            serve_app_shell(index.clone(), request, next)
        }));
    }

    let livereload = LiveReloadLayer::new();

    if !args.no_reload {
        let reloader = livereload.reloader();

        let (failed, failures) = mpsc::channel();

        let root = static_dir.clone();
        let mut debouncer =
            new_debouncer(DEBOUNCE_DELAY, None, move |result: DebounceEventResult| {
                match result {
                    Ok(events) => {
                        if events.iter().any(|event| is_change(&root, event)) {
                            println!("  File changed, reloading...");
                            reloader.reload();
                        }
                    }
                    Err(errors) => {
                        // Otherwise the watcher dies quietly while the banner still
                        // says reloads are on.
                        for error in errors {
                            eprintln!("  Cannot watch for changes: {error}");
                        }
                        let _ = failed.send(());
                    }
                }
            })?;

        debouncer.watch(&static_dir, RecursiveMode::Recursive)?;

        let debouncer = Arc::new(Mutex::new(debouncer));
        let watcher = Arc::clone(&debouncer);
        let watch_root = static_dir.clone();
        let rebuilt = livereload.reloader();
        std::thread::spawn(move || {
            // The watch follows the directory, not its name: a build that deletes
            // and recreates it leaves the watch on something nobody can reach.
            // Only Linux reports that in the file events, so look at the
            // directory itself instead.
            let mut watched = Watched::at(&watch_root);

            loop {
                let reason = match failures.recv_timeout(CHECK_INTERVAL) {
                    Ok(()) => Rewatch::WatchFailed,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    Err(mpsc::RecvTimeoutError::Timeout) => match directory_id(&watch_root) {
                        // Missing for the moment means a build is between
                        // removing the directory and writing the new one.
                        Some(now) if Some(now) != watched.as_ref().map(|it| it.id) => {
                            Rewatch::Replaced
                        }
                        _ => continue,
                    },
                };

                // A build can take a while between removing the directory and
                // writing the new one, so wait rather than give up.
                loop {
                    if !watch_root.is_dir() {
                        std::thread::sleep(CHECK_INTERVAL);
                        continue;
                    }

                    let Ok(mut debouncer) = watcher.lock() else {
                        return;
                    };

                    // Let go of the old directory first: after a rename the watch
                    // is still on it, reporting changes under its new name.
                    let _ = debouncer.unwatch(&watch_root);
                    let watching_again = debouncer
                        .watch(&watch_root, RecursiveMode::Recursive)
                        .is_ok();
                    drop(debouncer);

                    if watching_again {
                        watched = Watched::at(&watch_root);

                        // Either way the page on screen may be out of date:
                        // whatever was written while there was no watch went
                        // unnoticed, and nothing else will announce it.
                        match reason {
                            Rewatch::Replaced => println!("  Directory replaced, reloading..."),
                            Rewatch::WatchFailed => println!("  Watching again, reloading..."),
                        }
                        rebuilt.reload();
                        break;
                    }

                    std::thread::sleep(CHECK_INTERVAL);
                }
            }
        });

        app = app.layer(livereload);
    }

    let mut app = app.layer(CompressionLayer::new());

    if args.cache_assets {
        app = app.layer(middleware::from_fn(keep_hashed_assets));
    } else {
        // Never let the browser hold on to a stale file.
        app = app
            .layer(middleware::from_fn(always_answer_in_full))
            .layer(set_header(header::CACHE_CONTROL, "no-store"));
    }

    let app = app
        .layer(middleware::from_fn(refuse_hidden_files))
        .layer(set_header(header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
        .layer(set_header(header::X_FRAME_OPTIONS, "SAMEORIGIN"))
        .layer(set_header(
            header::REFERRER_POLICY,
            "strict-origin-when-cross-origin",
        ));

    let listener = listen(args.host, args.port).await?;

    // With `--port 0` the system chooses the port, so ask the listener.
    let bound = listener.local_addr()?;

    // 0.0.0.0 and [::] mean "every network interface", which a browser cannot
    // open. Only 127.0.0.1 and ::1 are localhost: 127.0.0.2 is loopback too,
    // but nothing answers there under that name.
    let ip = bound.ip();
    let is_localhost = ip.is_unspecified()
        || ip == IpAddr::V4(Ipv4Addr::LOCALHOST)
        || ip == IpAddr::V6(Ipv6Addr::LOCALHOST);
    let authority = if is_localhost {
        format!("localhost:{}", bound.port())
    } else {
        bound.to_string()
    };
    let url = format!("http://{authority}");

    println!("{RULE}");
    banner_row("Serving", static_dir.display());
    if args.port.is_none() && bound.port() != DEFAULT_PORT {
        banner_row(
            "Note",
            format!("port {DEFAULT_PORT} was busy, using {}", bound.port()),
        );
    }
    banner_row("Live reload", on_off(!args.no_reload));
    banner_row("Single-page app", on_off(args.spa));
    banner_row(
        "Caching",
        if args.cache_assets {
            "files under /assets/ for a year"
        } else {
            "off"
        },
    );
    if args.spa && !index.is_file() {
        banner_row(
            "Warning",
            format!("there is no {INDEX_FILE} here, so no page will load"),
        );
    }
    banner_row("Open", hyperlink(&url, &authority));
    println!("{RULE}\n");

    axum::serve(listener, app).await?;

    Ok(())
}

/// With `--spa`, an address matching no file gets index.html so the app can
/// route it. Only for page requests: a missing script or image still returns
/// 404, rather than arriving as HTML the browser then refuses to run.
async fn serve_app_shell(index: PathBuf, request: Request, next: Next) -> Response {
    let wants_page = request
        .headers()
        .get(header::ACCEPT)
        .and_then(|accept| accept.to_str().ok())
        .is_some_and(|accept| accept.contains("text/html"));

    let response = next.run(request).await;
    if response.status() != StatusCode::NOT_FOUND || !wants_page {
        return response;
    }

    match tokio::fs::read(&index).await {
        Ok(page) => ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], page).into_response(),
        Err(_) => response,
    }
}

async fn refuse_hidden_files(request: Request, next: Next) -> Response {
    // Otherwise `servio --host 0.0.0.0` in a project directory hands `.env`
    // and `.git/config` to anyone on the network.
    if names_a_hidden_file(request.uri().path()) {
        return StatusCode::NOT_FOUND.into_response();
    }

    next.run(request).await
}

/// While you are working, the browser only asks whether a file changed because
/// you just saved it, so "nothing has changed" is never the right answer.
/// Range requests are left alone, so seeking in audio and video still works.
async fn always_answer_in_full(mut request: Request, next: Next) -> Response {
    let headers = request.headers_mut();
    headers.remove(header::IF_MODIFIED_SINCE);
    headers.remove(header::IF_NONE_MATCH);

    next.run(request).await
}

/// What a published site tells the browser to keep. Names under `/assets/`
/// carry a hash of their contents, so the file at one of those addresses never
/// changes and the browser can keep it. Everything else is checked each time.
async fn keep_hashed_assets(request: Request, next: Next) -> Response {
    let hashed = request.uri().path().starts_with(ASSETS);
    let mut response = next.run(request).await;

    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(if hashed { KEEP_FOR_A_YEAR } else { "no-cache" }),
    );

    response
}

/// True for a name the server will not serve: anything hidden, apart from the
/// one directory the web uses.
fn is_hidden(name: &str) -> bool {
    name.starts_with('.') && name != WELL_KNOWN
}

/// Windows reads a backslash as a separator, so a hidden file could hide
/// behind one there. Elsewhere it is an ordinary character in a name.
#[cfg(windows)]
const SEPARATORS: &[char] = &['/', '\\'];
#[cfg(not(windows))]
const SEPARATORS: &[char] = &['/'];

/// True for an address naming a hidden file. The address is decoded first:
/// `%2e` is a dot and `%2f` a separator, and the file service reads them that
/// way too, so a check on the raw text would miss `/sub%2f.env`.
fn names_a_hidden_file(path: &str) -> bool {
    decode(path).split(SEPARATORS).any(is_hidden)
}

/// Turns each `%XX` into the byte it stands for, once, as the file service
/// does.
fn decode(path: &str) -> String {
    let raw = path.as_bytes();
    let mut decoded = Vec::with_capacity(raw.len());
    let mut at = 0;

    while at < raw.len() {
        let pair = (raw.get(at + 1).and_then(hex), raw.get(at + 2).and_then(hex));
        match (raw[at], pair) {
            (b'%', (Some(high), Some(low))) => {
                decoded.push(high << 4 | low);
                at += 3;
            }
            _ => {
                decoded.push(raw[at]);
                at += 1;
            }
        }
    }

    String::from_utf8_lossy(&decoded).into_owned()
}

fn hex(digit: &u8) -> Option<u8> {
    (*digit as char).to_digit(16).map(|value| value as u8)
}

fn set_header(name: HeaderName, value: &'static str) -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::overriding(name, HeaderValue::from_static(value))
}

fn banner_row(label: &str, value: impl Display) {
    println!("  {label:<LABEL_WIDTH$}: {BLUE}{value}{RESET}");
}

fn on_off(enabled: bool) -> &'static str {
    if enabled { "on" } else { "off" }
}

/// Makes `text` a clickable link to `url` in terminals that support it.
fn hyperlink(url: &str, text: impl Display) -> String {
    format!("{LINK_START}{url}{LINK_MID}{text}{LINK_END}")
}

/// True when the browser should refresh.
///
/// Reading a file is an event of its own on Linux, so serving a page would
/// announce a change, the browser would reload, and that reload would read the
/// file again — refreshing forever.
fn is_change(root: &Path, event: &DebouncedEvent) -> bool {
    let written = match event.kind {
        EventKind::Create(_) | EventKind::Remove(_) | EventKind::Any => true,
        // The bytes did not change. Reading a file updates its access time,
        // and treating that as a change would start the loop again. Only
        // Linux says this plainly: macOS reports the changes to a file as one
        // running total, so a permission change there can arrive looking like
        // the file was written.
        EventKind::Modify(ModifyKind::Metadata(_)) => false,
        EventKind::Modify(_) => true,
        // A file open for writing has just been closed: a finished save.
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        EventKind::Access(_) | EventKind::Other => false,
    };

    written && event.paths.iter().any(|path| !is_ignored(root, path))
}

/// The directory the watcher is attached to.
struct Watched {
    /// Kept open on Unix for one reason: while a directory is open the system
    /// cannot give its number to the next one, so a directory built in its
    /// place always reads as different. Without this, a rebuild that lands in
    /// the same spot on disk looks like no change at all.
    #[cfg(unix)]
    _open: std::fs::File,
    id: (u64, u64),
}

impl Watched {
    #[cfg(unix)]
    fn at(path: &Path) -> Option<Watched> {
        let open = std::fs::File::open(path).ok()?;
        Some(Watched {
            _open: open,
            id: directory_id(path)?,
        })
    }

    #[cfg(not(unix))]
    fn at(path: &Path) -> Option<Watched> {
        Some(Watched {
            id: directory_id(path)?,
        })
    }
}

/// How the system knows the directory this name leads to right now.
#[cfg(unix)]
fn directory_id(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;

    let directory = std::fs::metadata(path).ok()?;
    Some((directory.dev(), directory.ino()))
}

#[cfg(windows)]
fn directory_id(path: &Path) -> Option<(u64, u64)> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, GetFileInformationByHandle,
    };

    // Windows only opens a directory when asked for one, and the standard
    // library has no settled way to read its number, so ask Windows directly.
    let directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .ok()?;

    let mut about = unsafe { std::mem::zeroed::<BY_HANDLE_FILE_INFORMATION>() };
    // Safe: the handle is open for the whole call, and `about` is ours alone.
    if unsafe { GetFileInformationByHandle(directory.as_raw_handle() as _, &mut about) } == 0 {
        return None;
    }

    Some((
        u64::from(about.dwVolumeSerialNumber),
        u64::from(about.nFileIndexHigh) << 32 | u64::from(about.nFileIndexLow),
    ))
}

#[cfg(not(any(unix, windows)))]
fn directory_id(_path: &Path) -> Option<(u64, u64)> {
    None
}

/// True for files the browser never sees: build and version-control
/// directories, and the scratch files editors write while you type.
///
/// Only the part below `root` is checked, since the served directory may
/// itself sit inside `target/` or `node_modules/`.
fn is_ignored(root: &Path, path: &Path) -> bool {
    // A path from outside the served directory is not ours to judge, and
    // treating it as ignored would drop real changes.
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };

    let unservable = relative.components().any(|component| match component {
        Component::Normal(part) => {
            let part = part.to_string_lossy();
            is_hidden(&part) || IGNORED_DIRS.contains(&part.as_ref())
        }
        _ => false,
    });

    if unservable {
        return true;
    }

    let Some(name) = relative.file_name().map(|name| name.to_string_lossy()) else {
        return false;
    };

    IGNORED_SUFFIXES.iter().any(|suffix| name.ends_with(*suffix))
        || IGNORED_NAMES.iter().any(|ignored| name == *ignored)
        || (name.starts_with('#') && name.ends_with('#')) // emacs autosave copy
        || name.contains("___jb_") // JetBrains, saving through a temporary copy
}

fn resolve_dir(path: PathBuf) -> Result<PathBuf, String> {
    let absolute = if path.is_relative() {
        std::env::current_dir()
            .map_err(|e| format!("cannot read the current directory: {e}"))?
            .join(&path)
    } else {
        path
    };

    // Name the path: this is what a typo shows you.
    absolute
        .canonicalize()
        .map_err(|e| format!("{}: {e}", absolute.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify_debouncer_full::notify::Event;
    use notify_debouncer_full::notify::event::{CreateKind, DataChange, MetadataKind, RemoveKind};
    use std::time::Instant;

    fn event(kind: EventKind, paths: &[&str]) -> DebouncedEvent {
        let event = paths
            .iter()
            .fold(Event::new(kind), |event, path| event.add_path(path.into()));

        DebouncedEvent::new(event, Instant::now())
    }

    fn written(paths: &[&str]) -> DebouncedEvent {
        event(EventKind::Modify(ModifyKind::Data(DataChange::Any)), paths)
    }

    const ROOT: &str = "/site";

    #[test]
    fn build_directories_are_ignored() {
        for path in [
            "/site/.git/HEAD",
            "/site/node_modules/left-pad/index.js",
            "/site/target/debug/app",
        ] {
            assert!(is_ignored(Path::new(ROOT), Path::new(path)), "{path}");
        }
    }

    #[test]
    fn editor_scratch_files_are_ignored() {
        for path in [
            "/site/index.html.swp",
            "/site/index.html~",
            "/site/draft.tmp",
            "/site/.#index.html",
            "/site/#index.html#",
            "/site/index.html___jb_tmp___",
            "/site/4913",
            "/site/.DS_Store",
        ] {
            assert!(is_ignored(Path::new(ROOT), Path::new(path)), "{path}");
        }
    }

    #[test]
    fn ordinary_files_are_not_ignored() {
        for path in ["/site/index.html", "/site/assets/app.css", "/site/a~b/c.js"] {
            assert!(!is_ignored(Path::new(ROOT), Path::new(path)), "{path}");
        }
    }

    #[test]
    fn only_the_part_below_the_root_is_judged() {
        // The site itself may live inside a directory named target.
        let root = Path::new("/project/target/site");
        assert!(!is_ignored(root, Path::new("/project/target/site/app.css")));
        assert!(is_ignored(root, Path::new("/project/target/site/target/x")));
    }

    #[test]
    fn a_path_from_outside_the_root_is_left_alone() {
        assert!(!is_ignored(
            Path::new(ROOT),
            Path::new("/elsewhere/app.css")
        ));
    }

    #[test]
    fn hidden_addresses_are_refused() {
        for path in [
            "/.env",
            "/.git/config",
            "/%2e%65nv",
            "/%2Eenv",
            "/js/.hidden.js",
            "/.well-known/.secret",
        ] {
            assert!(names_a_hidden_file(path), "{path}");
        }
    }

    #[test]
    fn an_encoded_separator_does_not_hide_a_hidden_file() {
        // The file service reads %2f as a separator, so this check must too.
        for path in ["/sub%2f.env", "/sub%2F.env", "/sub%2f.git%2fconfig"] {
            assert!(names_a_hidden_file(path), "{path}");
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn a_backslash_is_an_ordinary_character_in_a_name_here() {
        // The file service would serve a file called `sub\.env`, so the guard
        // must not read the backslash as a separator and refuse it.
        assert!(!names_a_hidden_file("/sub%5c.env"));
    }

    #[test]
    fn files_the_server_will_not_send_are_ignored() {
        for path in [
            "/site/.env",
            "/site/.idea/workspace.xml",
            "/site/.vscode/settings.json",
        ] {
            assert!(is_ignored(Path::new(ROOT), Path::new(path)), "{path}");
        }
    }

    #[test]
    fn changes_the_web_can_reach_are_not_ignored() {
        assert!(!is_ignored(
            Path::new(ROOT),
            Path::new("/site/.well-known/token")
        ));
    }

    #[test]
    fn ordinary_addresses_are_allowed() {
        for path in ["/", "/index.html", "/a.b/c.js", "/.well-known/acme/token"] {
            assert!(!names_a_hidden_file(path), "{path}");
        }
    }

    #[test]
    fn reading_a_file_is_not_a_change() {
        let read = event(
            EventKind::Access(AccessKind::Open(AccessMode::Any)),
            &["/site/index.html"],
        );

        assert!(!is_change(Path::new(ROOT), &read));
    }

    #[test]
    fn a_finished_save_is_a_change() {
        let saved = event(
            EventKind::Access(AccessKind::Close(AccessMode::Write)),
            &["/site/index.html"],
        );

        assert!(is_change(Path::new(ROOT), &saved));
    }

    #[test]
    fn permissions_and_timestamps_are_not_changes() {
        let touched = event(
            EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any)),
            &["/site/index.html"],
        );

        assert!(!is_change(Path::new(ROOT), &touched));
    }

    #[test]
    fn writing_creating_and_deleting_are_changes() {
        let root = Path::new(ROOT);
        assert!(is_change(root, &written(&["/site/app.css"])));
        assert!(is_change(
            root,
            &event(EventKind::Create(CreateKind::File), &["/site/new.css"])
        ));
        assert!(is_change(
            root,
            &event(EventKind::Remove(RemoveKind::File), &["/site/old.css"])
        ));
    }

    #[test]
    fn writing_an_ignored_file_is_not_a_change() {
        assert!(!is_change(
            Path::new(ROOT),
            &written(&["/site/app.css.swp"])
        ));
    }

    #[test]
    fn one_real_file_among_ignored_ones_is_a_change() {
        let mixed = written(&["/site/app.css.swp", "/site/app.css"]);
        assert!(is_change(Path::new(ROOT), &mixed));
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn a_new_directory_of_the_same_name_is_a_different_directory() {
        let path = std::env::temp_dir().join(format!("servio-id-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();

        let watched = Watched::at(&path).expect("could not look at the directory");
        assert_eq!(
            directory_id(&path),
            Some(watched.id),
            "the same directory read twice"
        );

        std::fs::remove_dir_all(&path).unwrap();
        assert_eq!(directory_id(&path), None, "there is no directory to read");

        std::fs::create_dir_all(&path).unwrap();
        assert_ne!(
            directory_id(&path),
            Some(watched.id),
            "a rebuilt directory read as the old one"
        );

        std::fs::remove_dir_all(&path).unwrap();
    }
}
