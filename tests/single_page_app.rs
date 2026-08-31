//! `--spa`: page requests fall back to index.html, everything else does not.

mod common;

use common::{Server, TempDir, get, get_page, request};

fn app(name: &str) -> TempDir {
    let dir = TempDir::new(name);
    dir.write("index.html", "<html>the app</html>");
    dir.write("assets/app.css", "body {}");
    dir
}

#[test]
fn an_address_with_no_file_behind_it_gets_the_app() {
    let dir = app("route");
    let server = Server::start(dir.path(), &["--spa"]);

    let response = get_page(server.port, "/users/123");
    assert_eq!(response.status, 200, "a 404 stops the app from starting");
    assert!(response.text().contains("the app"));
}

#[test]
fn the_app_page_still_gets_the_reload_script() {
    let dir = app("script");
    let server = Server::start(dir.path(), &["--spa"]);

    assert!(
        get_page(server.port, "/users/123")
            .text()
            .contains("tower-livereload")
    );
}

#[test]
fn a_missing_script_or_image_is_still_not_found() {
    // Otherwise a typo in a src attribute arrives as a page of HTML that the
    // browser then refuses to run.
    let dir = app("assets");
    let server = Server::start(dir.path(), &["--spa"]);

    assert_eq!(
        request(server.port, "GET", "/assets/typo.js", &[("Accept", "*/*")]).status,
        404
    );
    assert_eq!(
        request(server.port, "GET", "/photo.png", &[("Accept", "image/*")]).status,
        404
    );
}

#[test]
fn real_files_are_still_served() {
    let dir = app("real");
    let server = Server::start(dir.path(), &["--spa"]);

    assert_eq!(get(server.port, "/assets/app.css").status, 200);
    assert_eq!(get_page(server.port, "/").status, 200);
}

#[test]
fn an_address_with_no_file_is_never_answered_from_the_index() {
    // The index has a date and a length of its own; neither describes an
    // address that does not exist.
    let dir = app("conditional");
    let server = Server::start(dir.path(), &["--spa"]);

    let far_future = "Thu, 01 Jan 2099 00:00:00 GMT";
    let conditional = request(
        server.port,
        "GET",
        "/gone.js",
        &[("If-Modified-Since", far_future)],
    );
    assert_eq!(conditional.status, 404);

    let ranged = request(
        server.port,
        "GET",
        "/clip.mp4",
        &[("Range", "bytes=200000-")],
    );
    assert_eq!(ranged.status, 404);
}

#[test]
fn hidden_files_stay_hidden() {
    let dir = app("hidden");
    dir.write(".env", "API_KEY=secret");
    let server = Server::start(dir.path(), &["--spa"]);

    assert_eq!(get_page(server.port, "/.env").status, 404);
}

#[test]
fn without_the_flag_a_missing_address_is_not_found() {
    let dir = app("off");
    let server = Server::start(dir.path(), &[]);

    assert_eq!(get_page(server.port, "/users/123").status, 404);
}

#[test]
fn says_so_when_there_is_no_index_to_fall_back_on() {
    let dir = TempDir::new("no-index");
    let server = Server::start(dir.path(), &["--spa"]);

    assert!(server.said("Warning"));
    assert_eq!(get_page(server.port, "/users/123").status, 404);
}
