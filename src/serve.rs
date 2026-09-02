//! How a request is answered: the file service, and the layers around it.

use crate::guard::{refuse_hidden_files, refuse_paths_outside};
use axum::Router;
use axum::extract::Request;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use http::{HeaderName, HeaderValue, StatusCode, header};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;
use tower_http::compression::CompressionLayer;
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_livereload::LiveReloadLayer;

pub(crate) const INDEX_FILE: &str = "index.html";

/// Where a build puts files whose name changes with their contents, so the
/// browser can keep them for as long as it likes.
const ASSETS: &str = "/assets/";

/// A year, the longest any browser is asked to keep a file.
const KEEP_FOR_A_YEAR: &str = "public, max-age=31536000, immutable";

/// The layers around the file service; each one added wraps those before it.
/// The order matters: the refusals sit inside the headers so a refusal is
/// answered like anything else, and the app shell sits inside live reload so
/// its page gets the script.
pub(crate) fn app(
    static_dir: &Path,
    spa: bool,
    cache_assets: bool,
    livereload: Option<LiveReloadLayer>,
    no_app_page: bool,
) -> Router {
    let mut app = Router::new().fallback_service(ServeDir::new(static_dir));

    if spa {
        let index = static_dir.join(INDEX_FILE);
        // Whether the app page is missing, as far as anyone has said out
        // loud. The banner is about to warn about the same look, so the first
        // request should not say it again.
        let missing = Arc::new(Mutex::new(no_app_page));
        app = app.layer(middleware::from_fn(move |request, next| {
            serve_app_shell(index.clone(), Arc::clone(&missing), request, next)
        }));
    }

    if let Some(livereload) = livereload {
        app = app.layer(livereload);
    }

    let root = static_dir.to_path_buf();
    let mut app = app
        .layer(CompressionLayer::new())
        .layer(middleware::from_fn(refuse_hidden_files))
        .layer(middleware::from_fn(move |request, next| {
            refuse_paths_outside(root.clone(), request, next)
        }));

    if cache_assets {
        app = app.layer(middleware::from_fn(keep_hashed_assets));
    } else {
        // Never let the browser hold on to a stale file.
        app = app
            .layer(middleware::from_fn(always_answer_in_full))
            .layer(set_header(header::CACHE_CONTROL, "no-store"));
    }

    app.layer(set_header(header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
        .layer(set_header(header::X_FRAME_OPTIONS, "SAMEORIGIN"))
        .layer(set_header(
            header::REFERRER_POLICY,
            "strict-origin-when-cross-origin",
        ))
}

/// With `--spa`, an address matching no file gets index.html so the app can
/// route it. Only for page requests: a missing script or image still gets a
/// 404, rather than HTML the browser then refuses to run.
async fn serve_app_shell(
    index: PathBuf,
    missing: Arc<Mutex<bool>>,
    request: Request,
    next: Next,
) -> Response {
    let wants_page = request
        .headers()
        .get(header::ACCEPT)
        .and_then(|accept| accept.to_str().ok())
        .is_some_and(|accept| accept.contains("text/html"));

    // A name under /assets/ carries a hash of the file's contents, so it is
    // a built file, never a route. Handing the app back there would leave the
    // browser keeping a page at that address for a year.
    let could_be_a_route = !request.uri().path().starts_with(ASSETS);

    let response = next.run(request).await;
    if response.status() != StatusCode::NOT_FOUND || !wants_page || !could_be_a_route {
        // Another file answering says nothing about the app page, and `/` is
        // served from disk without ever reading it. So while the page is
        // thought missing, look: it may be back.
        let mut missing = missing.lock().await;
        if *missing && is_there(&index).await {
            *missing = false;
        }

        return response;
    }

    // Held across the read, so two requests arriving together cannot record
    // what they found in the order they finished rather than the order they
    // looked, leaving the page down as gone while it is there.
    let mut missing = missing.lock().await;
    match tokio::fs::read(&index).await {
        Ok(page) => {
            // Back again, so the next time it goes is worth saying.
            *missing = false;
            ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], page).into_response()
        }
        // The 404 stands, but say why: the banner looked only at startup, so
        // a build that clears the directory takes the page away with nobody
        // watching.
        Err(error) => {
            // Once each time it goes, not once for each address. A build
            // that makes the directory before writing the page can set this
            // off for a gap that closes itself; it was true when it was said.
            if !std::mem::replace(&mut *missing, true) {
                let gone = error.kind() == ErrorKind::NotFound;
                let reason = if gone { "is gone" } else { "cannot be read" };
                eprintln!("  {INDEX_FILE} {reason}, so no page will load");
            }

            response
        }
    }
}

/// While you are working, the browser only asks whether a file changed
/// because you just saved it, so "nothing has changed" is never the right
/// answer. Range requests are left alone, so seeking in audio and video
/// still works.
async fn always_answer_in_full(mut request: Request, next: Next) -> Response {
    let headers = request.headers_mut();
    headers.remove(header::IF_MODIFIED_SINCE);
    headers.remove(header::IF_NONE_MATCH);

    next.run(request).await
}

/// What a published site tells the browser to keep. Names under `/assets/`
/// carry a hash of their contents, so the file at one of those addresses
/// never changes. Everything else is checked each time.
async fn keep_hashed_assets(request: Request, next: Next) -> Response {
    let hashed = request.uri().path().starts_with(ASSETS);
    let mut response = next.run(request).await;

    // Only a file that is really there: one missing during a deploy would
    // otherwise be remembered as missing for a year. "You already have it"
    // counts, since the browser takes the headers on that answer as the
    // file's own.
    let there = response.status().is_success() || response.status() == StatusCode::NOT_MODIFIED;
    let keep = hashed && there;

    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(if keep { KEEP_FOR_A_YEAR } else { "no-cache" }),
    );

    response
}

/// Whether the app page can be read, which is what serving it needs: one
/// that is there but closed to this program is no more use than a missing
/// one. Only asked while the page is thought missing.
async fn is_there(index: &Path) -> bool {
    let Ok(page) = tokio::fs::File::open(index).await else {
        return false;
    };

    page.metadata().await.is_ok_and(|found| found.is_file())
}

fn set_header(name: HeaderName, value: &'static str) -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::overriding(name, HeaderValue::from_static(value))
}
