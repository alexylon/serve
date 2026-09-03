//! What does and does not make the browser refresh.

mod common;

use common::{Server, TempDir, get, watches_for_reload};
use std::time::Duration;

/// Long enough for a poll to have had a look, and for the debounce to have
/// grouped what it found.
const A_LOOK: Duration = Duration::from_millis(2500);

/// Closes a directory to this program, which is how a look comes to fail on a
/// path that is still there.
#[cfg(unix)]
fn close_to_us(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000))
        .expect("could not close the directory");
}

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

    let before = server.reloads();
    dir.write("app.css", "body { color: red }");
    server.wait_for_reloads(before + 1);
}

#[test]
fn saving_a_file_refreshes_the_browser_while_polling() {
    // Where the system reports nothing, the look has to find the change itself.
    let dir = site("save-polling");
    let server = Server::start(dir.path(), &["--poll"]);
    server.settle();

    let before = server.reloads();
    dir.write("app.css", "body { color: red }");
    server.wait_for_reloads(before + 1);
}

#[test]
fn while_polling_the_ignored_files_are_still_ignored() {
    // A directory's write time moves for a swap file as readily as for a page,
    // and counting that refreshed the browser for every scratch file an editor
    // wrote.
    let dir = site("scratch-polling");
    let server = Server::start(dir.path(), &["--poll"]);
    server.settle();

    let before = server.reloads();
    dir.write("index.html.swp", "vim");
    dir.write("node_modules/left-pad/index.js", "module.exports = 1");

    server.expect_no_reload_within(before, A_LOOK);
}

#[cfg(unix)]
#[test]
fn while_polling_a_path_it_cannot_read_is_not_a_broken_watch() {
    // A look meets the same closed directory every time. Treated as a watch
    // that had gone down, it set the watch up again once a second and refreshed
    // the browser every time round.
    let dir = site("unreadable-polling");
    let closed = dir.join("closed");
    std::fs::create_dir_all(&closed).expect("could not create the directory");
    close_to_us(&closed);

    // Running as root, which reads it anyway: nothing to check.
    if std::fs::read_dir(&closed).is_ok() {
        return;
    }

    let server = Server::start(dir.path(), &["--poll"]);
    let before = server.reloads();
    server.expect_no_reload_within(before, A_LOOK);

    // And it is worth saying once, by name, not once a second.
    assert_eq!(
        server.count("Cannot look at closed"),
        1,
        "the same problem was said over and over:\n{}",
        server.lines().join("\n")
    );
    assert!(
        !server.said("Cannot watch for changes"),
        "one unreadable path is not a watch that went down:\n{}",
        server.lines().join("\n")
    );

    // What must not be lost: a real change is still found.
    let before = server.reloads();
    dir.write("app.css", "body { color: red }");
    server.wait_for_reloads(before + 1);
}

#[cfg(unix)]
#[test]
fn while_polling_an_unreadable_path_the_browser_never_sees_is_not_worth_saying() {
    // Nothing under node_modules can change what the browser shows, so a
    // complaint about it would send people after a fault that is not there.
    let dir = site("ignored-unreadable-polling");
    let closed = dir.join("node_modules/.cache");
    std::fs::create_dir_all(&closed).expect("could not create it");
    close_to_us(&closed);

    if std::fs::read_dir(&closed).is_ok() {
        return;
    }

    let server = Server::start(dir.path(), &["--poll"]);
    let before = server.reloads();
    server.expect_no_reload_within(before, A_LOOK);

    assert!(
        !server.said("Cannot"),
        "an ignored path was complained about:\n{}",
        server.lines().join("\n")
    );
}

#[cfg(unix)]
#[test]
fn while_polling_a_link_that_leads_nowhere_is_not_a_fault() {
    // Build output is full of these. Links are not followed, so there is
    // nothing to report.
    let dir = site("dangling-polling");
    std::os::unix::fs::symlink("/nowhere", dir.join("broken")).expect("could not link");

    let server = Server::start(dir.path(), &["--poll"]);
    let before = server.reloads();
    server.expect_no_reload_within(before, A_LOOK);

    assert!(
        !server.said("Cannot"),
        "a link that leads nowhere was reported as a fault:\n{}",
        server.lines().join("\n")
    );
}

#[cfg(unix)]
#[test]
fn while_polling_a_link_to_a_directory_above_is_not_a_walk_with_no_end() {
    // What npm and pnpm leave in node_modules. Followed, the walk never ends,
    // and servio said the watch was broken while it was doing its job.
    let dir = site("loop-polling");
    std::fs::create_dir_all(dir.join("node_modules/pkg")).expect("could not create it");
    std::os::unix::fs::symlink("../..", dir.join("node_modules/pkg/up")).expect("could not link");

    let server = Server::start(dir.path(), &["--poll"]);
    server.settle();

    assert!(
        !server.said("Cannot"),
        "a link to a directory above was reported as a fault:\n{}",
        server.lines().join("\n")
    );

    // And the look still finds a real change.
    let before = server.reloads();
    dir.write("app.css", "body { color: red }");
    server.wait_for_reloads(before + 1);
}

#[test]
fn while_polling_a_rebuild_is_not_reported_as_a_fault() {
    // A look that lands between the old directory going and the new one
    // arriving finds nothing there. The check says "Directory replaced" when it
    // does arrive; saying the directory cannot be read as well reads like a
    // fault.
    let dir = site("rebuild-quiet-polling");
    let server = Server::start(dir.path(), &["--poll"]);
    server.settle();

    dir.remove_all();
    std::thread::sleep(Duration::from_millis(1500));
    dir.create();
    dir.write("index.html", "<html>rebuilt</html>");
    server.wait_for("Directory replaced");

    assert!(
        !server.said("Cannot"),
        "a rebuild was reported as a fault:\n{}",
        server.lines().join("\n")
    );
}

#[cfg(unix)]
#[test]
fn while_polling_a_second_save_in_the_same_second_still_refreshes() {
    // A poll keeps the write time only to the whole second, so two saves close
    // together carry the same one and only the contents tell them apart. The
    // write time is pinned rather than raced for: the same second is otherwise
    // a matter of luck.
    let dir = site("same-second");
    let save = |contents: &str| {
        dir.write("app.css", contents);
        let pinned = std::process::Command::new("touch")
            .args(["-t", "202601011200"])
            .arg(dir.join("app.css"))
            .status()
            .expect("could not run touch");
        assert!(pinned.success(), "could not pin the write time");
    };

    save("body { color: red }");
    let server = Server::start(dir.path(), &["--poll"]);
    server.settle();

    let before = server.reloads();
    save("body { color: blue }");
    server.wait_for_reloads(before + 1);
}

#[test]
fn a_rebuild_is_announced_once_while_polling() {
    // The check notices the new directory before the poll's own account of the
    // same rebuild arrives. One rebuild, one line.
    let dir = site("announced-once-polling");
    let server = Server::start(dir.path(), &["--poll"]);
    server.settle();

    dir.remove_all();
    dir.create();
    dir.write("index.html", "<html>rebuilt</html>");
    server.wait_for("Directory replaced");

    // Long enough for the poll's own account of the rebuild to have arrived:
    // one look, plus the debounce that groups what it found.
    std::thread::sleep(Duration::from_millis(2000));
    assert_eq!(
        server.count("File changed"),
        0,
        "the rebuild was announced twice:\n{}",
        server.lines().join("\n")
    );

    // What must not be lost: editing after a rebuild still says so.
    dir.write("index.html", "<html>edited</html>");
    server.wait_for("File changed");
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
fn files_that_are_never_served_do_not_refresh() {
    // An editor writing into .idea or .vscode inside the site should not
    // refresh the page, and neither should a change to .env.
    let dir = site("unservable");
    let server = Server::start(dir.path(), &[]);
    server.settle();

    let before = server.reloads();
    dir.write(".idea/workspace.xml", "<project/>");
    dir.write(".vscode/settings.json", "{}");
    dir.write(".env", "API_KEY=secret");

    server.expect_no_reload(before);
}

#[test]
fn changes_the_web_can_reach_still_refresh() {
    let dir = site("well-known");
    let server = Server::start(dir.path(), &[]);
    server.settle();

    let before = server.reloads();
    dir.write(".well-known/token", "public");
    server.wait_for_reloads(before + 1);
}

// Linux only. macOS reports the changes to a file as one running total, so a
// permission change arrives carrying the file's creation as well, and there is
// no way to tell the two apart. The page refreshes once; nothing worse.
#[cfg(target_os = "linux")]
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
    let elsewhere = TempDir::new("rename-published");
    let moved = elsewhere.join("earlier");
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
}

#[test]
fn a_rebuild_is_announced_once() {
    // The watcher sees the old files go away and the check sees the new
    // directory, but that is one rebuild and reads better as one line.
    let dir = site("announced-once");
    let server = Server::start(dir.path(), &[]);
    server.settle();

    dir.remove_all();
    dir.create();
    dir.write("index.html", "<html>rebuilt</html>");
    server.wait_for("Directory replaced");
    server.settle();

    assert_eq!(
        server.count("File changed"),
        0,
        "the rebuild was announced twice:\n{}",
        server.lines().join("\n")
    );

    // What must not be lost: editing after a rebuild still says so.
    dir.write("index.html", "<html>edited</html>");
    server.wait_for("File changed");
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

#[cfg(target_os = "linux")]
fn set_permissions(dir: &TempDir, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir.join("app.css"), std::fs::Permissions::from_mode(mode))
        .expect("could not change the permissions");
}
