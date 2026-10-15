//! Reading the greeting every VNC server sends first: `RFB 003.008\n`.

use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::time::timeout;

/// Length of the greeting in bytes.
const BANNER_LEN: usize = 12;

/// Turns a greeting such as `RFB 003.008\n` into `3.8`.
pub(crate) fn parse_banner(bytes: &[u8]) -> Option<String> {
    let banner = bytes.get(..BANNER_LEN)?;
    if &banner[..4] != b"RFB " || banner[7] != b'.' || banner[11] != b'\n' {
        return None;
    }
    let major = parse_three_digits(&banner[4..7])?;
    let minor = parse_three_digits(&banner[8..11])?;
    Some(format!("{major}.{minor}"))
}

fn parse_three_digits(digits: &[u8]) -> Option<u16> {
    if digits.len() != 3 || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(
        digits
            .iter()
            .fold(0, |sum, d| sum * 10 + u16::from(d - b'0')),
    )
}

/// Reads the greeting from an open connection, without sending anything.
/// Returns the version, or `None` if it is not VNC or took too long.
pub(crate) async fn read_banner<R: AsyncRead + Unpin>(
    stream: &mut R,
    wait: Duration,
) -> Option<String> {
    let mut buffer = [0u8; BANNER_LEN];
    timeout(wait, stream.read_exact(&mut buffer))
        .await
        .ok()?
        .ok()?;
    parse_banner(&buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_versions() {
        assert_eq!(parse_banner(b"RFB 003.008\n").as_deref(), Some("3.8"));
        assert_eq!(parse_banner(b"RFB 003.007\n").as_deref(), Some("3.7"));
        assert_eq!(parse_banner(b"RFB 003.003\n").as_deref(), Some("3.3"));
        assert_eq!(parse_banner(b"RFB 003.889\n").as_deref(), Some("3.889"));
        assert_eq!(parse_banner(b"RFB 005.000\n").as_deref(), Some("5.0"));
    }

    #[test]
    fn ignores_bytes_after_the_greeting() {
        assert_eq!(parse_banner(b"RFB 003.008\nextra").as_deref(), Some("3.8"));
    }

    #[test]
    fn rejects_things_that_are_not_vnc() {
        assert_eq!(parse_banner(b""), None);
        assert_eq!(parse_banner(b"RFB 003.008"), None);
        assert_eq!(parse_banner(b"SSH-2.0-Open"), None);
        assert_eq!(parse_banner(b"RFB 003.008\r"), None);
        assert_eq!(parse_banner(b"RFB 3.8\n\0\0\0\0\0"), None);
        assert_eq!(parse_banner(b"RFB 00a.008\n"), None);
        assert_eq!(parse_banner(b"rfb 003.008\n"), None);
        assert_eq!(parse_banner(b"HTTP/1.1 400"), None);
    }

    #[tokio::test]
    async fn reads_from_a_stream() {
        let mut data: &[u8] = b"RFB 003.008\n";
        let version = read_banner(&mut data, Duration::from_millis(200)).await;
        assert_eq!(version.as_deref(), Some("3.8"));
    }

    #[tokio::test]
    async fn gives_up_on_short_or_silent_streams() {
        let mut short: &[u8] = b"RFB 003";
        assert_eq!(
            read_banner(&mut short, Duration::from_millis(100)).await,
            None
        );

        let (mut near, _far) = tokio::io::duplex(64);
        assert_eq!(
            read_banner(&mut near, Duration::from_millis(50)).await,
            None
        );
    }
}
