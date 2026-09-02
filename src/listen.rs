//! Finding a port to listen on.

use std::io::ErrorKind;
use std::net::{IpAddr, SocketAddr};
use tokio::net::TcpListener;

pub(crate) const DEFAULT_PORT: u16 = 3030;

/// Enough for a few servers left running, few enough that the address stays
/// easy to remember.
const PORTS_TO_TRY: u16 = 10;

/// A port that was asked for and cannot be had is an error: something else
/// expects that number. With none asked for, step up from 3030 until a free
/// one turns up; the banner says which.
pub(crate) async fn listen(host: IpAddr, port: Option<u16>) -> Result<TcpListener, std::io::Error> {
    if let Some(port) = port {
        return match TcpListener::bind(SocketAddr::new(host, port)).await {
            Err(error) if error.kind() == ErrorKind::AddrInUse => {
                Err(cannot_use(&format!("port {port} is already in use")))
            }
            // Below 1024 on Unix, and in the ranges Windows keeps for itself.
            // "Permission denied" and a number help nobody.
            Err(error) if error.kind() == ErrorKind::PermissionDenied => Err(cannot_use(&format!(
                "the system will not give this program port {port}"
            ))),
            listener => listener,
        };
    }

    for port in DEFAULT_PORT..DEFAULT_PORT + PORTS_TO_TRY {
        match TcpListener::bind(SocketAddr::new(host, port)).await {
            Ok(listener) => return Ok(listener),
            Err(error) if is_taken(&error) => continue,
            Err(error) => return Err(error),
        }
    }

    Err(cannot_use(&format!(
        "no free port between {DEFAULT_PORT} and {}",
        DEFAULT_PORT + PORTS_TO_TRY - 1
    )))
}

/// True when the next port is worth a try. Windows keeps whole ranges of
/// ports for itself and refuses them as forbidden rather than taken.
fn is_taken(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::AddrInUse | ErrorKind::PermissionDenied
    )
}

/// Adds the one thing to do about it.
fn cannot_use(what: &str) -> std::io::Error {
    std::io::Error::other(format!("{what} — choose another one with --port"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_port_the_system_keeps_is_stepped_past_like_a_busy_one() {
        assert!(is_taken(&ErrorKind::AddrInUse.into()));
        assert!(is_taken(&ErrorKind::PermissionDenied.into()));

        // This address does not exist here; the next port would not help.
        assert!(!is_taken(&ErrorKind::AddrNotAvailable.into()));
    }
}
