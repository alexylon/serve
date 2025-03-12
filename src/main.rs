use axum::Router;
use clap::Parser;
use std::io::Error;
use std::net::SocketAddr;
use std::path::{Component, PathBuf};
use tower_http::services::ServeDir;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long, default_value_t = 3030)]
    port: u16,

    #[arg(short = 'P', long, default_value = ".")]
    path: PathBuf,
}

const BLUE: &str = "\x1b[94m";
const RESET: &str = "\x1b[0m";
const LINK_START: &str = "\x1b]8;;";
const LINK_END: &str = "\x1b]8;;\x1b\\";
const LINK_MID: &str = "\x1b\\";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let addr = SocketAddr::from(([127, 0, 0, 1], args.port));
    let static_dir = get_static_dir(args.path)?;

    if !static_dir.exists() || !static_dir.is_dir() {
        return Err(format!(
            "The path {:?} does not exist or is not a directory",
            static_dir
        )
        .into());
    }

    let app = Router::new().fallback_service(ServeDir::new(&static_dir));

    let listener = tokio::net::TcpListener::bind(addr).await?;

    println!("-----------------------------------------------");
    println!("📂 Static content dir: {BLUE}{:?}{RESET}", static_dir);
    println!(
        "🌐 Server running on : {BLUE}{LINK_START}http://localhost:{0}{LINK_MID}localhost:{0}{LINK_END}{RESET}",
        args.port
    );
    println!("-----------------------------------------------\n");

    axum::serve(listener, app).await?;

    Ok(())
}

// Get the absolute path of the static content
fn get_static_dir(path: PathBuf) -> Result<PathBuf, Error> {
    if path.is_absolute() && !path.exists() {
        // If it's absolute but doesn't exist, try treating it as relative
        // by getting just the components after the root
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
