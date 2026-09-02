//! `--no-reload` and `--cache-assets`: what changes when the site is published
//! rather than being worked on.

mod common;

use common::{PAGE, Server, TempDir, get, get_page, request};

fn site(name: &str) -> TempDir {
    let dir = TempDir::new(name);
    dir.write("index.html", "<html>the app</html>");
    dir.write("assets/app-abc123.css", "body {}");
    dir
}

#[test]
fn hashed_assets_are_kept_and_everything_else_is_checked() {
    // A build puts the hash of the contents in the name, so the file at that
    // address never changes. The page itself does.
    let dir = site("cache");
    let server = Server::start(dir.path(), &["--cache-assets"]);

    assert_eq!(
        get(server.port, "/assets/app-abc123.css").header("cache-control"),
        Some("public, max-age=31536000, immutable")
    );
    assert_eq!(
        get(server.port, "/").header("cache-control"),
        Some("no-cache")
    );
    assert_eq!(
        get(server.port, "/nowhere.png").header("cache-control"),
        Some("no-cache")
    );
}

#[test]
fn a_browser_that_already_has_the_file_is_told_so() {
    // Without this, "check each time" would still send the whole file back.
    let dir = site("revalidate");
    let server = Server::start(dir.path(), &["--cache-assets"]);

    let modified = get(server.port, "/assets/app-abc123.css")
        .header("last-modified")
        .expect("no last-modified to test with")
        .to_string();

    let response = request(
        server.port,
        "GET",
        "/assets/app-abc123.css",
        &[("If-Modified-Since", &modified)],
    );

    assert_eq!(response.status, 304);
    assert!(response.body.is_empty());
}

#[test]
fn without_the_flag_nothing_is_kept() {
    let dir = site("off");
    let server = Server::start(dir.path(), &[]);

    assert_eq!(
        get(server.port, "/assets/app-abc123.css").header("cache-control"),
        Some("no-store")
    );
}

#[test]
fn no_reload_leaves_the_page_alone() {
    let dir = site("quiet");
    let server = Server::start(dir.path(), &["--no-reload"]);

    assert!(
        !get_page(server.port, "/")
            .text()
            .contains("tower-livereload")
    );
    assert!(server.said("Live reload    : off"));
}

#[test]
fn no_reload_stops_watching_altogether() {
    let dir = site("unwatched");
    let server = Server::start(dir.path(), &["--no-reload"]);
    server.settle();

    dir.write("index.html", "<html>edited</html>");
    server.expect_no_reload(0);
}

#[test]
fn a_published_site_still_refuses_hidden_files_and_serves_its_routes() {
    let dir = site("published");
    dir.write(".env", "API_KEY=secret");
    let server = Server::start(
        dir.path(),
        &[
            "--spa",
            "--no-reload",
            "--cache-assets",
            "--host",
            "0.0.0.0",
        ],
    );

    assert_eq!(get_page(server.port, "/.env").status, 404);

    let route = request(server.port, "GET", "/users/123", &[("Accept", PAGE)]);
    assert_eq!(route.status, 200);
    assert!(route.text().contains("the app"));
    assert!(!route.text().contains("tower-livereload"));
}
