mod banner;
mod browser;
mod errors;
mod guard;
mod listen;
mod serve;
mod watch;

use crate::errors::cannot_reach;
use anyhow::{Context, Result, bail};
use clap::Parser;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
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

    /// Find changes by looking at the files, for a network or shared folder
    /// the system reports no changes in
    #[arg(long, conflicts_with = "no_reload")]
    poll: bool,

    /// Let the browser keep files under /assets/ for a year, for a published site
    #[arg(long)]
    cache_assets: bool,

    /// Open the address in the browser once the server is up
    #[arg(long)]
    open: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            errors::report(&error);
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<()> {
    let args = Args::parse();
    let static_dir = resolve_dir(&args.dir)?;

    // A missing path already failed in resolve_dir; this catches a file.
    if !static_dir.is_dir() {
        bail!(
            "cannot serve {}: it is not a directory",
            static_dir.display()
        );
    }

    let livereload = LiveReloadLayer::new();
    if !args.no_reload {
        watch::start(&static_dir, args.poll, livereload.reloader())?;
    }

    // One look, shared with the banner: two looks could disagree, and a page
    // that went between them would be announced twice.
    let no_app_page = args.spa && !static_dir.join(serve::INDEX_FILE).is_file();

    let app = serve::app(
        &static_dir,
        args.spa,
        args.cache_assets,
        (!args.no_reload).then_some(livereload),
        no_app_page,
    );

    let listener = listen::listen(args.host, args.port).await?;

    // With `--port 0` the system picks the port, so ask the listener.
    let bound = listener
        .local_addr()
        .context("cannot tell which address the server is listening on")?;
    banner::print(bound, &args, &static_dir, no_app_page);
    if args.open {
        browser::open(&banner::url(bound));
    }

    axum::serve(listener, app)
        .await
        .context("the server stopped")?;

    Ok(())
}

fn resolve_dir(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_relative() {
        std::env::current_dir()
            .map_err(|error| cannot_reach("cannot read the current directory".to_string(), &error))?
            .join(path)
    } else {
        path.to_path_buf()
    };

    // Name the path, so a typo shows.
    absolute
        .canonicalize()
        .map_err(|error| cannot_reach(format!("cannot serve {}", absolute.display()), &error))
}
