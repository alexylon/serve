use axum::Router;
use clap::Parser;
use http::HeaderValue;
use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode, DebounceEventResult};
use std::io::Error;
use std::net::SocketAddr;
use std::path::{Component, PathBuf};
use std::time::Duration;
use tower_http::compression::CompressionLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_livereload::LiveReloadLayer;

#[derive(Parser, Debug)]
#[command(author, version, about = "Static file server with live reload and SPA support")]
struct Args {
    #[arg(short, long, default_value_t = 3030)]
    port: u16,

    #[arg(short, long, default_value = ".")]
    dir: PathBuf,
}

const BLUE: &str = "\x1b[94m";
const RESET: &str = "\x1b[0m";
const LINK_START: &str = "\x1b]8;;";
const LINK_END: &str = "\x1b]8;;\x1b\\";
const LINK_MID: &str = "\x1b\\";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let addr = SocketAddr::from(([0, 0, 0, 0], args.port));
    let static_dir = resolve_dir(args.dir)?;

    if !static_dir.exists() || !static_dir.is_dir() {
        return Err(format!("{static_dir:?} does not exist or is not a directory").into());
    }

    // SPA fallback: serve index.html for unmatched routes
    let index = static_dir.join("index.html");
    let serve = ServeDir::new(&static_dir).not_found_service(ServeFile::new(&index));

    // Live reload + file watcher
    let livereload = LiveReloadLayer::new();
    let reloader = livereload.reloader();

    let watch_dir = static_dir.clone();
    let mut debouncer = new_debouncer(
        Duration::from_millis(200),
        move |res: DebounceEventResult| {
            if let Ok(events) = res {
                if !events.is_empty() {
                    println!("  File changed, reloading...");
                    reloader.reload();
                }
            }
        },
    )?;

    debouncer
        .watcher()
        .watch(&watch_dir, RecursiveMode::Recursive)?;

    let app = Router::new()
        .fallback_service(serve)
        .layer(livereload)
        .layer(CompressionLayer::new())
        .layer(SetResponseHeaderLayer::overriding(
            http::header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            http::header::X_FRAME_OPTIONS,
            HeaderValue::from_static("SAMEORIGIN"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            http::header::X_XSS_PROTECTION,
            HeaderValue::from_static("1; mode=block"),
        ));

    let listener = tokio::net::TcpListener::bind(addr).await?;

    println!("-----------------------------------------------");
    println!("  Static dir  : {BLUE}{static_dir:?}{RESET}");
    println!("  Live reload : {BLUE}on{RESET}");
    println!(
        "  Listening on: {BLUE}{LINK_START}http://localhost:{0}{LINK_MID}localhost:{0}{LINK_END}{RESET}",
        args.port
    );
    println!("-----------------------------------------------\n");

    axum::serve(listener, app).await?;

    drop(debouncer);

    Ok(())
}

fn resolve_dir(path: PathBuf) -> Result<PathBuf, Error> {
    if path.is_absolute() && !path.exists() {
        let stripped = path
            .components()
            .skip_while(|c| matches!(c, Component::Prefix(_) | Component::RootDir))
            .collect::<PathBuf>();
        std::env::current_dir()?.join(stripped)
    } else if path.is_relative() {
        std::env::current_dir()?.join(&path)
    } else {
        path.clone()
    }
    .canonicalize()
}
