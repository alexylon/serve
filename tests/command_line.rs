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

    let first = Server::start_choosing_a_port(dir.path());
    let second = Server::start_choosing_a_port(dir.path());

    assert_ne!(first.port, second.port);
    assert_eq!(second.port, first.port + 1);
    assert!(second.said("was busy"), "{}", second.lines().join("\n"));
    assert!(!first.said("was busy"), "{}", first.lines().join("\n"));
    assert_eq!(get(second.port, "/").status, 200);
}
