//! Checking a single port on a single computer.

use std::io::ErrorKind;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::banner::read_banner;

/// What we expect to find behind a port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PortKind {
    /// Secure shell: only check that the port is open.
    Ssh,
    /// Screen sharing: also read the VNC greeting.
    Vnc,
}

/// What happened when we tried a port.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PortState {
    /// A VNC server answered with this version.
    Vnc(String),
    /// Something is listening, but it did not greet us like VNC.
    Open,
    /// The computer is there but the port is closed.
    Refused,
    /// The network told us at once that the address cannot be reached.
    Unreachable,
    /// Nobody answered in time.
    NoAnswer,
}

/// Tries to connect and, for VNC, reads the greeting. Sends nothing and
/// closes the connection right away.
pub(crate) async fn probe(ip: IpAddr, port: u16, kind: PortKind, wait: Duration) -> PortState {
    let mut stream = match timeout(wait, TcpStream::connect(SocketAddr::new(ip, port))).await {
        Err(_) => return PortState::NoAnswer,
        Ok(Err(err)) => return classify_error(err.kind()),
        Ok(Ok(stream)) => stream,
    };
    match kind {
        PortKind::Ssh => PortState::Open,
        PortKind::Vnc => match read_banner(&mut stream, wait).await {
            Some(version) => PortState::Vnc(version),
            None => PortState::Open,
        },
    }
}

fn classify_error(kind: ErrorKind) -> PortState {
    match kind {
        ErrorKind::ConnectionRefused | ErrorKind::ConnectionReset => PortState::Refused,
        ErrorKind::HostUnreachable | ErrorKind::NetworkUnreachable => PortState::Unreachable,
        _ => PortState::NoAnswer,
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    use super::*;

    const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
    const WAIT: Duration = Duration::from_millis(500);

    /// Starts a tiny server that sends `greeting` to whoever connects.
    async fn serve(greeting: &'static [u8]) -> u16 {
        let listener = TcpListener::bind((LOOPBACK, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let _ = socket.write_all(greeting).await;
                // Keep the connection open for a moment.
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        });
        port
    }

    #[tokio::test]
    async fn recognises_a_vnc_server() {
        let port = serve(b"RFB 003.008\n").await;
        let state = probe(LOOPBACK, port, PortKind::Vnc, WAIT).await;
        assert_eq!(state, PortState::Vnc("3.8".into()));
    }

    #[tokio::test]
    async fn open_port_that_is_not_vnc() {
        let port = serve(b"SSH-2.0-test\r\n").await;
        assert_eq!(
            probe(LOOPBACK, port, PortKind::Vnc, WAIT).await,
            PortState::Open
        );
        assert_eq!(
            probe(LOOPBACK, port, PortKind::Ssh, WAIT).await,
            PortState::Open
        );
    }

    #[tokio::test]
    async fn closed_port_is_refused() {
        // Grab a free port number, then close it again.
        let listener = TcpListener::bind((LOOPBACK, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        assert_eq!(
            probe(LOOPBACK, port, PortKind::Vnc, WAIT).await,
            PortState::Refused
        );
    }

    #[test]
    fn sorts_connect_errors() {
        assert_eq!(
            classify_error(ErrorKind::ConnectionRefused),
            PortState::Refused
        );
        assert_eq!(
            classify_error(ErrorKind::HostUnreachable),
            PortState::Unreachable
        );
        assert_eq!(classify_error(ErrorKind::TimedOut), PortState::NoAnswer);
    }
}
