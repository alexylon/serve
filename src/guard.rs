//! What the server refuses to hand out: anything outside the served
//! directory, and anything hidden.

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http::StatusCode;
use std::path::{Path, PathBuf};

/// The one hidden directory the web uses.
const WELL_KNOWN: &str = ".well-known";

/// Windows reads a backslash as a separator, so a hidden file could hide
/// behind one there. Elsewhere it is an ordinary character.
#[cfg(windows)]
const SEPARATORS: &[char] = &['/', '\\'];
#[cfg(not(windows))]
const SEPARATORS: &[char] = &['/'];

pub(crate) async fn refuse_paths_outside(root: PathBuf, request: Request, next: Next) -> Response {
    // A link inside the directory can point anywhere on the disk, and the
    // file service follows it: `/etc/passwd` included.
    if !leads_inside(&root, request.uri().path()) {
        return StatusCode::NOT_FOUND.into_response();
    }

    next.run(request).await
}

/// True when the address leads inside the served directory, or nowhere at
/// all — the file service answers for a missing file, and answering here
/// would only tell a stranger which names exist.
///
/// This is one look at the disk, taken here rather than on a thread of its
/// own: the handoff costs more than the look, and the file service is about
/// to take several.
fn leads_inside(root: &Path, path: &str) -> bool {
    let decoded = decode(path);
    let relative = decoded.trim_start_matches(SEPARATORS);

    match root.join(relative).canonicalize() {
        Ok(resolved) => resolved.starts_with(root),
        Err(_) => true,
    }
}

pub(crate) async fn refuse_hidden_files(request: Request, next: Next) -> Response {
    // Otherwise `servio --host 0.0.0.0` in a project directory hands `.env`
    // and `.git/config` to anyone on the network.
    if names_a_hidden_file(request.uri().path()) {
        return StatusCode::NOT_FOUND.into_response();
    }

    next.run(request).await
}

/// True for a name the server will not serve: anything hidden, apart from
/// the one directory the web uses.
pub(crate) fn is_hidden(name: &str) -> bool {
    name.starts_with('.') && name != WELL_KNOWN
}

/// True for an address naming a hidden file. The address is decoded first,
/// as the file service decodes it: `%2e` is a dot and `%2f` a separator, so
/// a check on the raw text would miss `/sub%2f.env`.
fn names_a_hidden_file(path: &str) -> bool {
    decode(path).split(SEPARATORS).any(is_hidden)
}

/// Turns each `%XX` into the byte it stands for, once, as the file service
/// does.
fn decode(path: &str) -> String {
    let raw = path.as_bytes();
    let mut decoded = Vec::with_capacity(raw.len());
    let mut at = 0;

    while at < raw.len() {
        let pair = (raw.get(at + 1).and_then(hex), raw.get(at + 2).and_then(hex));
        match (raw[at], pair) {
            (b'%', (Some(high), Some(low))) => {
                decoded.push(high << 4 | low);
                at += 3;
            }
            _ => {
                decoded.push(raw[at]);
                at += 1;
            }
        }
    }

    String::from_utf8_lossy(&decoded).into_owned()
}

fn hex(digit: &u8) -> Option<u8> {
    (*digit as char).to_digit(16).map(|value| value as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_addresses_are_refused() {
        for path in [
            "/.env",
            "/.git/config",
            "/%2e%65nv",
            "/%2Eenv",
            "/js/.hidden.js",
            "/.well-known/.secret",
        ] {
            assert!(names_a_hidden_file(path), "{path}");
        }
    }

    #[test]
    fn an_encoded_separator_does_not_hide_a_hidden_file() {
        // The file service reads %2f as a separator, so this check must too.
        for path in ["/sub%2f.env", "/sub%2F.env", "/sub%2f.git%2fconfig"] {
            assert!(names_a_hidden_file(path), "{path}");
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn a_backslash_is_an_ordinary_character_in_a_name_here() {
        // The file service would serve a file called `sub\.env`, so the
        // backslash must not be read as a separator here.
        assert!(!names_a_hidden_file("/sub%5c.env"));
    }

    #[test]
    fn ordinary_addresses_are_allowed() {
        for path in ["/", "/index.html", "/a.b/c.js", "/.well-known/acme/token"] {
            assert!(!names_a_hidden_file(path), "{path}");
        }
    }
}
