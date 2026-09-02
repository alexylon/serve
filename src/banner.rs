//! What the server says on the way up.

use crate::Args;
use crate::listen::DEFAULT_PORT;
use crate::serve::INDEX_FILE;
use std::fmt::Display;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::Path;

const BLUE: &str = "\x1b[94m";
const RESET: &str = "\x1b[0m";
const LINK_START: &str = "\x1b]8;;";
const LINK_END: &str = "\x1b]8;;\x1b\\";
const LINK_MID: &str = "\x1b\\";
const RULE: &str = "-----------------------------------------------";
/// Fits the longest label, so the colons line up.
const LABEL_WIDTH: usize = 15;

pub(crate) fn print(bound: SocketAddr, args: &Args, static_dir: &Path) {
    let authority = authority(bound);
    let url = format!("http://{authority}");

    println!("{RULE}");
    row("Serving", static_dir.display());
    if args.port.is_none() && bound.port() != DEFAULT_PORT {
        row(
            "Note",
            format!("port {DEFAULT_PORT} was busy, using {}", bound.port()),
        );
    }
    row("Live reload", on_off(!args.no_reload));
    row("Single-page app", on_off(args.spa));
    row(
        "Caching",
        if args.cache_assets {
            "files under /assets/ for a year"
        } else {
            "off"
        },
    );
    if args.spa && !static_dir.join(INDEX_FILE).is_file() {
        row(
            "Warning",
            format!("there is no {INDEX_FILE} here, so no page will load"),
        );
    }
    row("Open", hyperlink(&url, &authority));
    println!("{RULE}\n");
}

/// The address to open in a browser. 0.0.0.0 and [::] mean every network
/// interface, which a browser cannot open, so those become localhost. So do
/// 127.0.0.1 and ::1, and nothing else: 127.0.0.2 is loopback too, but the
/// name does not lead there.
fn authority(bound: SocketAddr) -> String {
    let ip = bound.ip();
    let is_localhost = ip.is_unspecified()
        || ip == IpAddr::V4(Ipv4Addr::LOCALHOST)
        || ip == IpAddr::V6(Ipv6Addr::LOCALHOST);

    if is_localhost {
        format!("localhost:{}", bound.port())
    } else {
        bound.to_string()
    }
}

fn row(label: &str, value: impl Display) {
    println!("  {label:<LABEL_WIDTH$}: {BLUE}{value}{RESET}");
}

fn on_off(enabled: bool) -> &'static str {
    if enabled { "on" } else { "off" }
}

/// Makes `text` a clickable link to `url` in terminals that support it.
fn hyperlink(url: &str, text: impl Display) -> String {
    format!("{LINK_START}{url}{LINK_MID}{text}{LINK_END}")
}
