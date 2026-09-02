//! Finding a port to listen on.

use anyhow::{Result, anyhow};
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
pub(crate) async fn listen(host: IpAddr, port: Option<u16>) -> Result<TcpListener> {
    if let Some(port) = port {
        let address = SocketAddr::new(host, port);
        return match TcpListener::bind(address).await {
            Ok(listener) => Ok(listener),
            Err(error) if error.kind() == ErrorKind::AddrInUse => {
                Err(cannot_use(format!("port {port} is already in use")))
            }
            // Below 1024 on Unix, and in the ranges Windows keeps for itself.
            // "Permission denied" and a number help nobody.
            Err(error) if error.kind() == ErrorKind::PermissionDenied => Err(cannot_use(format!(
                "the system will not give this program port {port}"
            ))),
            // A typo in --host, or an address this machine does not answer to.
            Err(error) if error.kind() == ErrorKind::AddrNotAvailable => Err(no_such_address(host)),
            Err(error) => Err(cannot_listen(address, &error)),
        };
    }

    for port in DEFAULT_PORT..DEFAULT_PORT + PORTS_TO_TRY {
        let address = SocketAddr::new(host, port);
        match TcpListener::bind(address).await {
            Ok(listener) => return Ok(listener),
            Err(error) if is_taken(&error) => continue,
            Err(error) if error.kind() == ErrorKind::AddrNotAvailable => {
                return Err(no_such_address(host));
            }
            Err(error) => return Err(cannot_listen(address, &error)),
        }
    }

    Err(cannot_use(format!(
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
fn cannot_use(what: String) -> anyhow::Error {
    anyhow!("{what} — choose another one with --port")
}

/// Says which part is wrong: the address, not the port, so a different port
/// would not help.
fn no_such_address(host: IpAddr) -> anyhow::Error {
    anyhow!("this machine has no address {host} — check --host")
}

/// Anything the named cases do not cover. There is no plainer way to put a
/// refusal nobody expected, so the system's own words stand, with the address
/// they are about.
fn cannot_listen(address: SocketAddr, error: &std::io::Error) -> anyhow::Error {
    anyhow!("cannot listen on {address}: {error}")
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
