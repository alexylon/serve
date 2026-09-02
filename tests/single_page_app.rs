//! `--spa`: page requests fall back to index.html, everything else does not.

mod common;

use common::{PAGE, Server, TempDir, get, get_page, request};

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
fn the_app_page_carries_none_of_the_index_files_own_details() {
    // The index has a date and a length of its own, and neither describes the
    // address that was asked for.
    let dir = app("conditional");
    let server = Server::start(dir.path(), &["--spa"]);

    let conditional = request(
        server.port,
        "GET",
        "/users/123",
        &[
            ("Accept", PAGE),
            ("If-Modified-Since", "Thu, 01 Jan 2099 00:00:00 GMT"),
        ],
    );
    assert_eq!(conditional.status, 200, "a 304 would leave the page blank");
    assert_eq!(conditional.header("last-modified"), None);
    assert_eq!(conditional.header("etag"), None);
    assert!(conditional.text().contains("the app"));

    let ranged = request(
        server.port,
        "GET",
        "/users/123",
        &[("Accept", PAGE), ("Range", "bytes=200000-")],
    );
    assert_eq!(
        ranged.status, 200,
        "the index's length is not this address's"
    );
    assert_eq!(ranged.header("content-range"), None);
}

#[test]
fn a_missing_file_stays_missing_however_it_is_asked_for() {
    let dir = app("not-a-page");
    let server = Server::start(dir.path(), &["--spa"]);

    let conditional = request(
        server.port,
        "GET",
        "/gone.js",
        &[
            ("Accept", "*/*"),
            ("If-Modified-Since", "Thu, 01 Jan 2099 00:00:00 GMT"),
        ],
    );
    assert_eq!(conditional.status, 404);

    let ranged = request(
        server.port,
        "GET",
        "/clip.mp4",
        &[("Accept", "*/*"), ("Range", "bytes=200000-")],
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

    // The banner already said it; the first request should not say it again.
    assert_eq!(server.count("no page will load"), 1, "said twice");
}

#[test]
fn says_so_each_time_the_app_page_goes_missing() {
    let dir = app("index-comes-and-goes");
    let server = Server::start(dir.path(), &["--spa", "--no-reload"]);
    let page = dir.join("index.html");

    std::fs::remove_file(&page).expect("could not remove the page");
    assert_eq!(get_page(server.port, "/users/123").status, 404);
    server.wait_for_count("no page will load", 1);

    // A build clearing the directory takes the page away for a moment. That
    // must not use up the one warning the real outage needs — and the page
    // counts as back even though the only request in between is for a real
    // file, which never reads it.
    dir.write("index.html", "<html>the app</html>");
    assert_eq!(get(server.port, "/assets/app.css").status, 200);

    std::fs::remove_file(&page).expect("could not remove the page");
    assert_eq!(get_page(server.port, "/users/456").status, 404);
    server.wait_for_count("no page will load", 2);

    // Still once for each time it goes, not once for each address.
    assert_eq!(get_page(server.port, "/users/789").status, 404);
    assert_eq!(
        server.count("no page will load"),
        2,
        "said for each address"
    );
}
