use axum::Router;
use clap::Parser;
use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode, DebounceEventResult};
use std::io::Error;
use std::net::SocketAddr;
use std::path::{Component, PathBuf};
use std::time::Duration;
use tower_http::services::ServeDir;
use tower_livereload::LiveReloadLayer;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
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
    let static_dir = get_static_dir(args.dir)?;

    if !static_dir.exists() || !static_dir.is_dir() {
        return Err(format!(
            "The path {:?} does not exist or is not a directory",
            static_dir
        )
        .into());
    }

    // Create livereload layer and get a reloader handle
    let livereload = LiveReloadLayer::new();
    let reloader = livereload.reloader();

    // Set up file watcher
    let watch_dir = static_dir.clone();
    let mut debouncer = new_debouncer(Duration::from_millis(200), move |res: DebounceEventResult| {
        if let Ok(events) = res {
            if !events.is_empty() {
                println!("🔄 File changed, reloading...");
                reloader.reload();
            }
        }
    })
    .expect("Failed to create file watcher");

    debouncer.watcher().watch(&watch_dir, RecursiveMode::Recursive)?;

    let app = Router::new()
        .fallback_service(ServeDir::new(&static_dir))
        .layer(livereload);

    let listener = tokio::net::TcpListener::bind(addr).await?;

    println!("-----------------------------------------------");
    println!("📂 Static content dir: {BLUE}{:?}{RESET}", static_dir);
    println!("🔄 Live reload: {BLUE}enabled{RESET}");
    println!(
        "🌐 Server running on : {BLUE}{LINK_START}http://localhost:{0}{LINK_MID}localhost:{0}{LINK_END}{RESET}",
        args.port
    );
    println!("-----------------------------------------------\n");

    axum::serve(listener, app).await?;

    // Keep debouncer alive (it's dropped when main exits)
    drop(debouncer);

    Ok(())
}

// Get the absolute path of the static content
fn get_static_dir(path: PathBuf) -> Result<PathBuf, Error> {
    if path.is_absolute() && !path.exists() {
        let path_without_root = path
            .components()
            .skip_while(|c| matches!(c, Component::Prefix(_) | Component::RootDir))
            .collect::<PathBuf>();
        std::env::current_dir()?.join(path_without_root)
    } else if path.is_relative() {
        std::env::current_dir()?.join(&path)
    } else {
        path.clone()
    }
    .canonicalize()
}
