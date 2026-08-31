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
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tower_http::compression::CompressionLayer;
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_livereload::LiveReloadLayer;

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Static file server with live reload, for local development"
)]
struct Args {
    /// Port to listen on
    #[arg(short, long, default_value_t = 3030)]
    port: u16,

    /// Address to listen on (use 0.0.0.0 to reach this server from other devices)
    #[arg(long, default_value_t = IpAddr::V4(Ipv4Addr::LOCALHOST))]
    host: IpAddr,

    /// Directory to serve
    #[arg(short, long, default_value = ".")]
    dir: PathBuf,

    /// Serve index.html when the address matches no file, for single-page apps
    #[arg(long)]
    spa: bool,
}

const BLUE: &str = "\x1b[94m";
const RESET: &str = "\x1b[0m";
const LINK_START: &str = "\x1b]8;;";
const LINK_END: &str = "\x1b]8;;\x1b\\";
const LINK_MID: &str = "\x1b\\";
const RULE: &str = "-----------------------------------------------";
/// Fits the longest label, so the colons line up.
const LABEL_WIDTH: usize = 15;

/// Long enough to group the writes one save makes, short enough that the
/// refresh still feels immediate.
const DEBOUNCE_DELAY: Duration = Duration::from_millis(200);

const REWATCH_INTERVAL: Duration = Duration::from_millis(100);

const INDEX_FILE: &str = "index.html";

/// The one hidden directory the web actually uses.
const WELL_KNOWN: &str = ".well-known";

/// Directories whose contents should never trigger a browser reload.
const IGNORED_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "target",
    "node_modules",
    ".cache",
    ".next",
    ".svelte-kit",
];

/// Scratch files editors leave behind: vim swap files, backup copies.
const IGNORED_SUFFIXES: &[&str] = &["~", ".tmp", ".swp", ".swx", ".swo"];

/// vim writes `4913` to test whether a directory accepts writes; macOS
/// leaves `.DS_Store` behind.
const IGNORED_NAMES: &[&str] = &["4913", ".DS_Store"];

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("serve: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let addr = SocketAddr::new(args.host, args.port);
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
    let reloader = livereload.reloader();

    // The watch follows the directory, not its name: a build that deletes and
    // recreates it leaves the watch on something nobody can reach.
    let (rewatch, rewatch_requests) = mpsc::channel();

    let root = static_dir.clone();
    let mut debouncer = new_debouncer(DEBOUNCE_DELAY, None, move |result: DebounceEventResult| {
        match result {
            Ok(events) => {
                if events.iter().any(|event| is_watched_dir_gone(&root, event)) {
                    let _ = rewatch.send(());
                }
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
                let _ = rewatch.send(());
            }
        }
    })?;

    debouncer.watch(&static_dir, RecursiveMode::Recursive)?;

    let debouncer = Arc::new(Mutex::new(debouncer));
    let watching = Arc::clone(&debouncer);
    let watch_root = static_dir.clone();
    let rebuilt = livereload.reloader();
    std::thread::spawn(move || {
        while rewatch_requests.recv().is_ok() {
            // A build can take a while between removing the directory and
            // writing the new one, so wait rather than give up.
            loop {
                std::thread::sleep(REWATCH_INTERVAL);
                if !watch_root.is_dir() {
                    continue;
                }

                let Ok(mut debouncer) = watching.lock() else {
                    return;
                };

                // Let go of the old directory first: after a rename the watch
                // is still on it, reporting changes under its new name.
                let _ = debouncer.unwatch(&watch_root);
                if debouncer
                    .watch(&watch_root, RecursiveMode::Recursive)
                    .is_ok()
                {
                    // The new files were written before this watch existed,
                    // so nothing else will announce them.
                    println!("  Directory replaced, reloading...");
                    rebuilt.reload();
                    break;
                }
            }
        }
    });

    let app = app
        .layer(livereload)
        .layer(CompressionLayer::new())
        .layer(middleware::from_fn(guard_request))
        // Never let the browser hold on to a stale file.
        .layer(set_header(header::CACHE_CONTROL, "no-store"))
        .layer(set_header(header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
        .layer(set_header(header::X_FRAME_OPTIONS, "SAMEORIGIN"))
        .layer(set_header(
            header::REFERRER_POLICY,
            "strict-origin-when-cross-origin",
        ));

    let listener = tokio::net::TcpListener::bind(addr).await?;

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
    banner_row("Live reload", "on");
    banner_row("Single-page app", on_off(args.spa));
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

async fn guard_request(mut request: Request, next: Next) -> Response {
    // Otherwise `serve --host 0.0.0.0` in a project directory hands `.env`
    // and `.git/config` to anyone on the network.
    if names_a_hidden_file(request.uri().path()) {
        return StatusCode::NOT_FOUND.into_response();
    }

    // The browser is asking because something was just saved, so "nothing has
    // changed" is never the right answer. Range requests are left alone, so
    // seeking in audio and video still works.
    let headers = request.headers_mut();
    headers.remove(header::IF_MODIFIED_SINCE);
    headers.remove(header::IF_NONE_MATCH);

    next.run(request).await
}

/// True for an address with a hidden segment. `%2e` is the same dot once the
/// address is decoded, so settle that first.
fn names_a_hidden_file(path: &str) -> bool {
    path.replace("%2e", ".")
        .replace("%2E", ".")
        .split('/')
        .any(|segment| segment.starts_with('.') && segment != WELL_KNOWN)
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
        // and treating that as a change would start the loop again.
        EventKind::Modify(ModifyKind::Metadata(_)) => false,
        EventKind::Modify(_) => true,
        // A file open for writing has just been closed: a finished save.
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        EventKind::Access(_) | EventKind::Other => false,
    };

    written && event.paths.iter().any(|path| !is_ignored(root, path))
}

/// True when the served directory itself is gone. Builds that publish
/// atomically rename it away: `mv dist dist.old`.
fn is_watched_dir_gone(root: &Path, event: &DebouncedEvent) -> bool {
    let gone = matches!(
        event.kind,
        EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_))
    );

    gone && event.paths.iter().any(|path| path == root)
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

    let in_ignored_dir = relative.components().any(|component| match component {
        Component::Normal(part) => IGNORED_DIRS.iter().any(|dir| part == *dir),
        _ => false,
    });

    if in_ignored_dir {
        return true;
    }

    let Some(name) = relative.file_name().map(|name| name.to_string_lossy()) else {
        return false;
    };

    IGNORED_SUFFIXES.iter().any(|suffix| name.ends_with(*suffix))
        || IGNORED_NAMES.iter().any(|ignored| name == *ignored)
        || name.starts_with(".#")                         // emacs lock file
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
