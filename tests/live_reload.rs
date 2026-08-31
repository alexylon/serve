//! What does and does not make the browser refresh.

mod common;

use common::{Server, TempDir, get, watches_for_reload};
use std::time::Duration;

fn site(name: &str) -> TempDir {
    let dir = TempDir::new(name);
    dir.write("index.html", "<html>first</html>");
    dir.write("app.css", "body {}");
    dir
}

#[test]
fn saving_a_file_refreshes_the_browser() {
    let dir = site("save");
    let server = Server::start(dir.path(), &[]);
    server.settle();

    dir.write("app.css", "body { color: red }");
    server.wait_for_reloads(1);
}

#[test]
fn a_connected_browser_is_told_to_refresh() {
    let dir = site("connected");
    let server = Server::start(dir.path(), &[]);
    server.settle();

    let listening = watches_for_reload(server.port);
    std::thread::sleep(Duration::from_millis(300));
    dir.write("index.html", "<html>second</html>");

    assert!(
        listening.join().unwrap(),
        "the browser was never told to refresh"
    );
}

#[test]
fn serving_a_page_does_not_refresh_it() {
    // Reading a file is an event of its own on Linux. If that counted as a
    // change, every page would reload itself for ever.
    let dir = site("loop");
    let server = Server::start(dir.path(), &[]);
    server.settle();

    let before = server.reloads();
    for _ in 0..20 {
        assert_eq!(get(server.port, "/").status, 200);
        assert_eq!(get(server.port, "/app.css").status, 200);
    }

    server.expect_no_reload(before);
}

#[test]
fn editor_scratch_files_are_ignored() {
    let dir = site("scratch");
    let server = Server::start(dir.path(), &[]);
    server.settle();

    let before = server.reloads();
    dir.write("index.html.swp", "vim");
    dir.write("index.html~", "backup");
    dir.write("#index.html#", "emacs");
    dir.write(".#index.html", "emacs lock");
    dir.write(".git/HEAD", "ref: refs/heads/main");
    dir.write("node_modules/left-pad/index.js", "module.exports = 1");

    server.expect_no_reload(before);
}

#[test]
fn changing_only_permissions_does_not_refresh() {
    let dir = site("permissions");
    let server = Server::start(dir.path(), &[]);
    server.settle();

    let before = server.reloads();
    set_permissions(&dir, 0o600);
    set_permissions(&dir, 0o644);

    server.expect_no_reload(before);
}

#[test]
fn survives_a_build_that_replaces_the_directory() {
    // `rm -rf dist && mkdir dist` is what most build tools do, and the watch
    // follows the old directory unless it is re-established.
    let dir = site("rebuild");
    let server = Server::start(dir.path(), &[]);
    server.settle();

    dir.remove_all();
    dir.create();
    dir.write("index.html", "<html>rebuilt</html>");
    server.wait_for("Directory replaced");
    server.settle();

    let after_rebuild = server.reloads();
    dir.write("index.html", "<html>edited after the rebuild</html>");
    server.wait_for_reloads(after_rebuild + 1);
}

#[test]
fn survives_a_build_that_renames_the_directory_away() {
    let dir = site("rename");
    let moved = dir.path().with_extension("old");
    let server = Server::start(dir.path(), &[]);
    server.settle();

    std::fs::rename(dir.path(), &moved).expect("could not rename the directory");
    dir.create();
    dir.write("index.html", "<html>published</html>");
    server.wait_for("Directory replaced");
    server.settle();

    // The old directory is no longer the site, so changes there mean nothing.
    let after_rename = server.reloads();
    std::fs::write(moved.join("index.html"), "<html>stale</html>").unwrap();
    server.expect_no_reload(after_rename);

    let _ = std::fs::remove_dir_all(&moved);
}

#[test]
fn keeps_serving_after_the_directory_is_replaced() {
    let dir = site("still-serving");
    let server = Server::start(dir.path(), &[]);
    server.settle();

    dir.remove_all();
    dir.create();
    dir.write("index.html", "<html>rebuilt</html>");
    server.wait_for("Directory replaced");

    assert!(get(server.port, "/").text().contains("rebuilt"));
}

#[cfg(unix)]
fn set_permissions(dir: &TempDir, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir.join("app.css"), std::fs::Permissions::from_mode(mode))
        .expect("could not change the permissions");
}

#[cfg(not(unix))]
fn set_permissions(_dir: &TempDir, _mode: u32) {}
