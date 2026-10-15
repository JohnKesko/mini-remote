//! Session errors, each carrying a plain-English message for the user.

use std::io;

use crate::session::{DisconnectKind, DisconnectReason};

#[derive(Debug)]
pub struct SessionError {
    pub kind: DisconnectKind,
    pub message: String,
}

impl SessionError {
    pub fn new(kind: DisconnectKind, message: impl Into<String>) -> Self {
        SessionError { kind, message: message.into() }
    }

    pub fn protocol(detail: impl std::fmt::Display) -> Self {
        Self::new(DisconnectKind::Protocol, format!("The connection failed: {detail}."))
    }

    pub fn auth(message: impl Into<String>) -> Self {
        Self::new(DisconnectKind::AuthFailed, message)
    }

    pub fn closed() -> Self {
        Self::new(DisconnectKind::Closed, "Disconnected.")
    }

    pub fn into_reason(self) -> DisconnectReason {
        DisconnectReason { kind: self.kind, message: self.message }
    }
}

impl From<io::Error> for SessionError {
    fn from(e: io::Error) -> Self {
        let message = match e.kind() {
            io::ErrorKind::UnexpectedEof | io::ErrorKind::ConnectionReset | io::ErrorKind::BrokenPipe => {
                "The connection was closed by the other computer.".to_string()
            }
            io::ErrorKind::ConnectionRefused => {
                "The computer refused the connection. Check that screen sharing (VNC) is turned on.".to_string()
            }
            io::ErrorKind::TimedOut => "The computer did not answer in time.".to_string(),
            io::ErrorKind::InvalidData => format!("The connection was damaged ({e})."),
            _ => format!("Network problem: {e}."),
        };
        let kind = if e.kind() == io::ErrorKind::InvalidData { DisconnectKind::Protocol } else { DisconnectKind::Network };
        SessionError::new(kind, message)
    }
}

impl From<codec::DecodeError> for SessionError {
    fn from(e: codec::DecodeError) -> Self {
        match e {
            codec::DecodeError::Io(io) => io.into(),
            other => SessionError::new(DisconnectKind::Protocol, format!("{}.", capitalise(&other.to_string()))),
        }
    }
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}
