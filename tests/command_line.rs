//! Arguments, and what the server says on the way up.

mod common;

use common::{Server, TempDir, get};
use std::process::Command;

fn run(args: &[&str]) -> (String, bool) {
    let output = Command::new(env!("CARGO_BIN_EXE_servio"))
        .args(args)
        .output()
        .expect("could not run servio");

    let said = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);

    (said, output.status.success())
}

#[test]
fn a_directory_that_does_not_exist_names_itself() {
    let (said, ok) = run(&["--dir", "/definitely/not/here"]);

    assert!(!ok);
    assert!(
        said.contains("/definitely/not/here"),
        "the message should say which path failed: {said}"
    );
    assert!(
        said.contains("no such directory"),
        "it should say what is wrong in words: {said}"
    );
    assert!(
        !said.contains("os error"),
        "no numbered errors, please: {said}"
    );
}

#[cfg(unix)]
#[test]
fn a_directory_it_may_not_reach_says_so_plainly() {
    use std::os::unix::fs::PermissionsExt;

    // Closed at the parent: the name below it cannot even be looked up. Closing
    // the directory itself would not do, since it can still be named.
    let dir = TempDir::new("unreachable");
    let inner = dir.join("locked/inner");
    std::fs::create_dir_all(&inner).expect("could not create the directory");
    let locked = dir.join("locked");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000))
        .expect("could not close the directory");

    // Asked before running it: root reaches the directory anyway, and the
    // server would then start and never come back.
    let reachable = std::fs::read_dir(&inner).is_ok();
    let said = reachable.then(String::new).unwrap_or_else(|| {
        let (said, ok) = run(&["--dir", inner.to_str().unwrap()]);
        assert!(!ok);
        said
    });
    let _ = std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755));

    if reachable {
        return;
    }

    assert!(said.contains("will not let this program read it"), "{said}");
    assert!(
        !said.contains("os error"),
        "no numbered errors, please: {said}"
    );
}

#[test]
fn a_file_is_not_a_directory() {
    let dir = TempDir::new("not-a-dir");
    dir.write("index.html", "<html>hi</html>");

    let (said, ok) = run(&["--dir", dir.join("index.html").to_str().unwrap()]);

    assert!(!ok);
    assert!(said.contains("is not a directory"), "{said}");
}

#[test]
fn reports_the_port_the_system_chose() {
    // Every other test relies on this: `--port 0` must print a usable address.
    let dir = TempDir::new("port-zero");
    dir.write("index.html", "<html>hi</html>");

    let server = Server::start(dir.path(), &[]);
    assert!(server.port > 0);
    assert_eq!(get(server.port, "/").status, 200);
}

#[test]
fn offers_localhost_when_listening_everywhere() {
    let dir = TempDir::new("everywhere");
    dir.write("index.html", "<html>hi</html>");

    // 0.0.0.0 is not an address a browser can open.
    let server = Server::start(dir.path(), &["--host", "0.0.0.0"]);
    assert!(server.said("localhost"));
    assert_eq!(get(server.port, "/").status, 200);
}

// Only Linux binds the whole of 127.0.0.0/8 without being asked.
#[cfg(target_os = "linux")]
#[test]
fn offers_the_real_address_when_it_is_not_localhost() {
    let dir = TempDir::new("other-loopback");
    dir.write("index.html", "<html>hi</html>");

    // 127.0.0.2 is loopback too, but nothing answers there under the name
    // localhost, so the banner has to say 127.0.0.2.
    let server = Server::start(dir.path(), &["--host", "127.0.0.2"]);
    assert!(
        server.said("127.0.0.2"),
        "the banner should point at the address it is listening on:\n{}",
        server.lines().join("\n")
    );
}

#[test]
fn says_what_it_is_doing_on_the_way_up() {
    let dir = TempDir::new("banner");
    dir.write("index.html", "<html>hi</html>");

    let server = Server::start(dir.path(), &[]);

    assert!(server.said("Serving"));
    assert!(server.said("Live reload"));
    assert!(server.said("Single-page app"));
}

#[test]
fn a_port_that_was_asked_for_is_never_swapped_for_another() {
    // Something else expects that number: a proxy rule, a service file, a
    // bookmark. Moving quietly would turn a loud failure into a puzzling one.
    let dir = TempDir::new("busy-port");
    dir.write("index.html", "<html>hi</html>");
    let taken = Server::start(dir.path(), &[]);

    let (said, ok) = run(&[
        "--dir",
        dir.path().to_str().unwrap(),
        "--port",
        &taken.port.to_string(),
    ]);

    assert!(!ok);
    assert!(said.contains(&taken.port.to_string()), "{said}");
    assert!(said.contains("already in use"), "{said}");
    assert!(
        !said.contains("os error"),
        "no numbered errors, please: {said}"
    );
}

#[test]
fn the_next_port_is_used_when_none_was_asked_for() {
    let dir = TempDir::new("next-port");
    dir.write("index.html", "<html>hi</html>");

    // Nothing here assumes 3030 itself is free: this machine may well be
    // running the very server that makes the second one step aside.
    let first = Server::start_choosing_a_port(dir.path());
    let second = Server::start_choosing_a_port(dir.path());

    // Higher, not exactly one higher: anything else on this machine may hold
    // the port in between, and stepping past that is the point.
    assert!(
        second.port > first.port,
        "the second server should have stepped up from {}, not taken {}",
        first.port,
        second.port
    );
    assert!(second.said("was busy"), "{}", second.lines().join("\n"));
    assert_eq!(get(second.port, "/").status, 200);
}

#[test]
fn a_port_the_system_will_not_give_us_says_so_plainly() {
    // Ports below 1024 need privileges on Unix, and Windows keeps whole ranges
    // for itself. Either way the answer has to be readable.
    let Err(refused) = std::net::TcpListener::bind(("127.0.0.1", 80)) else {
        return; // This machine allows it, so there is nothing to test.
    };
    if refused.kind() != std::io::ErrorKind::PermissionDenied {
        return; // Something is simply listening there.
    }

    let dir = TempDir::new("forbidden-port");
    dir.write("index.html", "<html>hi</html>");

    let (said, ok) = run(&["--dir", dir.path().to_str().unwrap(), "--port", "80"]);

    assert!(!ok);
    assert!(said.contains("80"), "it should say which port: {said}");
    assert!(said.contains("--port"), "it should say what to do: {said}");
    assert!(
        !said.contains("os error"),
        "no numbered errors, please: {said}"
    );
}

#[cfg(unix)]
#[test]
fn a_directory_it_may_not_read_says_so_plainly() {
    use std::os::unix::fs::PermissionsExt;

    // This one can be named, so it gets past the first check and fails in the
    // watcher instead — which used to answer with a number and a list of paths.
    let dir = TempDir::new("closed");
    let closed = dir.join("closed");
    std::fs::create_dir_all(&closed).expect("could not create the directory");
    std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o000))
        .expect("could not close the directory");

    // Asked before running it, for the reason above.
    let readable = std::fs::read_dir(&closed).is_ok();
    let said = readable.then(String::new).unwrap_or_else(|| {
        let (said, ok) = run(&["--dir", closed.to_str().unwrap()]);
        assert!(!ok);
        said
    });
    let _ = std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o755));

    if readable {
        return;
    }

    assert!(said.contains("will not let this program read it"), "{said}");
    assert!(
        !said.contains("os error"),
        "no numbered errors, please: {said}"
    );
    assert!(
        !said.contains("about ["),
        "the watcher's own wording leaked through: {said}"
    );
}
