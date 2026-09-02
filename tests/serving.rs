//! Serving files: what comes back, and what does not.

mod common;

use common::{PAGE, Server, TempDir, get, get_page, request};

fn site(name: &str) -> TempDir {
    let dir = TempDir::new(name);
    dir.write("index.html", "<html>hello</html>");
    dir.write("assets/app.css", &"body { color: red; }\n".repeat(200));
    dir
}

#[test]
fn serves_the_index_page() {
    let dir = site("index");
    let server = Server::start(dir.path(), &[]);

    let response = get(server.port, "/");
    assert_eq!(response.status, 200);
    assert!(response.text().contains("hello"));
}

#[test]
fn adds_the_reload_script_to_pages() {
    let dir = site("script");
    let server = Server::start(dir.path(), &[]);

    assert!(get(server.port, "/").text().contains("tower-livereload"));
}

#[test]
fn a_missing_file_is_not_found() {
    let dir = site("missing");
    let server = Server::start(dir.path(), &[]);

    assert_eq!(get(server.port, "/nowhere.png").status, 404);
    assert_eq!(get_page(server.port, "/users/123").status, 404);
}

#[test]
fn nothing_is_cached() {
    let dir = site("cache");
    let server = Server::start(dir.path(), &[]);

    for path in ["/", "/assets/app.css", "/nowhere.png"] {
        let response = get(server.port, path);
        assert_eq!(
            response.header("cache-control"),
            Some("no-store"),
            "for {path}"
        );
    }
}

#[test]
fn a_browser_is_never_told_that_nothing_changed() {
    let dir = site("conditional");
    let server = Server::start(dir.path(), &[]);

    let modified = get(server.port, "/assets/app.css")
        .header("last-modified")
        .expect("no last-modified to test with")
        .to_string();

    let response = request(
        server.port,
        "GET",
        "/assets/app.css",
        &[("If-Modified-Since", &modified)],
    );

    assert_eq!(response.status, 200, "a 304 would hide the newest edit");
}

#[test]
fn part_of_a_file_can_still_be_asked_for() {
    // Seeking in audio and video depends on this.
    let dir = site("range");
    let server = Server::start(dir.path(), &[]);

    let response = request(
        server.port,
        "GET",
        "/assets/app.css",
        &[("Range", "bytes=0-3")],
    );

    assert_eq!(response.status, 206);
}

#[test]
fn compresses_what_it_sends() {
    let dir = site("compression");
    let server = Server::start(dir.path(), &[]);

    for (offered, expected) in [("br", "br"), ("gzip", "gzip")] {
        let response = request(
            server.port,
            "GET",
            "/assets/app.css",
            &[("Accept-Encoding", offered)],
        );
        assert_eq!(response.header("content-encoding"), Some(expected));
    }
}

#[test]
fn sends_the_security_headers() {
    let dir = site("headers");
    let server = Server::start(dir.path(), &[]);
    let response = get(server.port, "/");

    assert_eq!(response.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(response.header("x-frame-options"), Some("SAMEORIGIN"));
    assert_eq!(
        response.header("referrer-policy"),
        Some("strict-origin-when-cross-origin")
    );
}

#[test]
fn refuses_to_climb_out_of_the_directory() {
    let dir = site("traversal");
    let server = Server::start(dir.path(), &[]);

    assert_eq!(get(server.port, "/../../etc/passwd").status, 404);
    assert_eq!(get(server.port, "/%2e%2e/%2e%2e/etc/passwd").status, 404);
}

#[test]
fn only_answers_the_methods_a_static_site_needs() {
    let dir = site("methods");
    let server = Server::start(dir.path(), &[]);

    let head = request(server.port, "HEAD", "/", &[("Accept", PAGE)]);
    assert_eq!(head.status, 200);
    assert!(head.body.is_empty());

    assert_eq!(request(server.port, "POST", "/", &[]).status, 405);
}

#[test]
fn hidden_files_are_not_served() {
    let dir = site("hidden");
    dir.write(".env", "API_KEY=secret");
    dir.write(".git/config", "[core]");
    dir.write(".well-known/token", "public");

    let server = Server::start(dir.path(), &[]);

    let refused = get(server.port, "/.env");
    assert_eq!(refused.status, 404);
    assert_eq!(
        refused.header("cache-control"),
        Some("no-store"),
        "a refusal is answered like anything else"
    );
    assert_eq!(get(server.port, "/.git/config").status, 404);

    // The same dot, written the long way round.
    assert_eq!(get(server.port, "/%2e%65nv").status, 404);

    // And an encoded separator, which the file service reads as a real one.
    dir.write("sub/.env", "SUB_SECRET=1");
    dir.write("sub/.git/config", "[core]");
    assert_eq!(get(server.port, "/sub/.env").status, 404);
    assert_eq!(get(server.port, "/sub%2f.env").status, 404);
    assert_eq!(get(server.port, "/sub%2F.env").status, 404);
    assert_eq!(get(server.port, "/sub%2f.git%2fconfig").status, 404);

    // The one hidden directory the web actually uses.
    assert_eq!(get(server.port, "/.well-known/token").status, 200);
}

/// Windows needs privileges to make one, so these are for the rest.
#[cfg(unix)]
mod links {
    use super::common::{Server, TempDir, get};
    use std::os::unix::fs::symlink;

    #[test]
    fn a_link_leading_out_of_the_served_directory_is_refused() {
        // Otherwise `servio --host 0.0.0.0` in a directory with one link in it
        // hands out whatever that link leads to.
        let dir = TempDir::new("link-out");
        let elsewhere = TempDir::new("link-target");
        dir.write("index.html", "<html>hi</html>");
        elsewhere.write("private.txt", "SECRET");

        symlink(elsewhere.path(), dir.join("out")).expect("could not make the link");
        symlink(elsewhere.join("private.txt"), dir.join("private.txt"))
            .expect("could not make the link");

        let server = Server::start(dir.path(), &[]);

        assert_eq!(get(server.port, "/out/private.txt").status, 404);
        assert_eq!(get(server.port, "/private.txt").status, 404);
        assert_eq!(get(server.port, "/out%2Fprivate.txt").status, 404);
    }

    #[test]
    fn a_link_inside_the_served_directory_still_works() {
        // Only leaving is refused. A site may well link one of its own files.
        let dir = TempDir::new("link-in");
        dir.write("index.html", "<html>hi</html>");
        dir.write("real/app.css", "body {}");

        symlink(dir.join("real"), dir.join("linked")).expect("could not make the link");

        let server = Server::start(dir.path(), &[]);

        assert_eq!(get(server.port, "/linked/app.css").status, 200);
        assert_eq!(get(server.port, "/linked/app.css").text(), "body {}");
    }
}
