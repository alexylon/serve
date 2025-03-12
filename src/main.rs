use axum::Router;
use clap::Parser;
use std::net::SocketAddr;
use std::path::PathBuf;
use tower_http::services::ServeDir;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long, default_value_t = 3030)]
    port: u16,

    #[arg(short = 'P', long, default_value = ".")]
    path: PathBuf,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let addr = SocketAddr::from(([127, 0, 0, 1], args.port));

    // Get the absolute path of the static content
    let static_dir = if args.path.is_relative() {
        std::env::current_dir()?.join(&args.path)
    } else {
        args.path.clone()
    };

    if !static_dir.exists() || !static_dir.is_dir() {
        return Err(format!(
            "The path {:?} does not exist or is not a directory",
            static_dir
        )
        .into());
    }

    let app = Router::new().fallback_service(ServeDir::new(&static_dir));

    let listener = tokio::net::TcpListener::bind(addr).await?;

    println!("Serving static files from: {:?}", static_dir);
    println!("Server running on localhost:{}", args.port);

    axum::serve(listener, app).await?;

    Ok(())
}
