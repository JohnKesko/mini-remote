//! Errors, written so that a person without technical knowledge can understand them.

use std::io;

use russh_sftp::client::error::Error as SftpError;
use russh_sftp::protocol::StatusCode;

/// Everything that can go wrong while logging in, tunnelling or transferring files.
///
/// The `Display` text of every variant is a plain-English sentence that can be shown to the user.
#[derive(Debug, thiserror::Error)]
pub enum SshError {
    /// The username, password or key was not accepted.
    #[error(
        "The remote computer did not accept the login. Check the username and the password (or key) and try again."
    )]
    AuthFailed,

    /// The user (or the app) did not trust the remote computer's identity.
    #[error("The remote computer's identity was not accepted, so the connection was stopped.")]
    HostKeyRejected,

    /// The network connection failed or was lost.
    #[error("{}", describe_io(.0))]
    Network(#[from] io::Error),

    /// The secure connection broke the rules or could not be set up.
    #[error("The secure connection failed: {0}")]
    Protocol(String),

    /// The file transfer service on the remote computer reported a problem.
    #[error("{0}")]
    Sftp(String),

    /// The user cancelled the transfer.
    #[error("The transfer was cancelled.")]
    Cancelled,

    /// A key file could not be read or used.
    #[error("{0}")]
    KeyFile(String),

    /// The key file is locked with a passphrase and none was given.
    #[error("This key file is locked with a passphrase. Enter the passphrase and try again.")]
    PassphraseNeeded,

    /// The key file could not be unlocked with the passphrase given.
    #[error("The passphrase is wrong, or the key file is damaged.")]
    WrongPassphrase,

    /// The key agent (the helper that holds your keys) is missing or has no usable key.
    #[error("{0}")]
    Agent(String),

    /// The remote computer would not open the requested tunnel.
    #[error("The remote computer could not connect to {host} on port {port}: {reason}")]
    TunnelRefused {
        /// Address the tunnel should lead to (as seen from the remote computer).
        host: String,
        /// Port the tunnel should lead to.
        port: u16,
        /// Plain-English reason.
        reason: String,
    },

    /// A file or folder on this computer could not be read or written.
    #[error("{0}")]
    LocalFile(String),

    /// The connection has already been closed.
    #[error("The connection to the remote computer has been closed.")]
    Closed,
}

impl SshError {
    /// Describes a problem with a file on this computer.
    pub(crate) fn local(path: &std::path::Path, err: &io::Error) -> SshError {
        let what = match err.kind() {
            io::ErrorKind::NotFound => "was not found".to_string(),
            io::ErrorKind::PermissionDenied => {
                "cannot be used because this computer does not allow it".to_string()
            }
            io::ErrorKind::StorageFull => "cannot be written because the disk is full".to_string(),
            io::ErrorKind::AlreadyExists => "already exists".to_string(),
            _ => format!("cannot be used ({err})"),
        };
        SshError::LocalFile(format!("The file or folder \"{}\" {what}.", path.display()))
    }
}

/// Turns a low-level network error into a sentence a person can act on.
fn describe_io(e: &io::Error) -> String {
    use io::ErrorKind as K;
    let text = e.to_string();
    let lower = text.to_lowercase();
    if lower.contains("lookup address")
        || lower.contains("nodename nor servname")
        || lower.contains("name or service not known")
        || lower.contains("no such host")
        || lower.contains("temporary failure in name resolution")
    {
        return "The remote computer could not be found by that name or address. Check how it is spelled and that you are on the right network.".into();
    }
    match e.kind() {
        K::ConnectionRefused => "The remote computer refused the connection. Check that remote login is switched on there and that the port number is right.".into(),
        K::TimedOut => "The remote computer did not answer in time. Check that it is switched on and that the network connection works.".into(),
        K::HostUnreachable | K::NetworkUnreachable => "The remote computer cannot be reached from this network.".into(),
        K::ConnectionReset | K::ConnectionAborted | K::BrokenPipe | K::UnexpectedEof | K::NotConnected => {
            "The connection to the remote computer was lost.".into()
        }
        K::NetworkDown => "The network is down. Check your internet connection.".into(),
        _ => format!("A network problem stopped the connection: {text}"),
    }
}

impl From<russh::Error> for SshError {
    fn from(e: russh::Error) -> Self {
        use russh::Error as R;
        match e {
            R::UnknownKey => SshError::HostKeyRejected,
            R::IO(io) => SshError::Network(io),
            R::HUP => io::Error::new(io::ErrorKind::ConnectionReset, "closed by remote").into(),
            R::Disconnect | R::SendError | R::RecvError => {
                io::Error::new(io::ErrorKind::ConnectionAborted, "disconnected").into()
            }
            R::KeepaliveTimeout | R::ConnectionTimeout | R::InactivityTimeout | R::Elapsed(_) => {
                io::Error::new(io::ErrorKind::TimedOut, "timed out").into()
            }
            R::NoCommonAlgo { .. } | R::Kex | R::KexInit => SshError::Protocol(
                "this app and the remote computer could not agree on a way to encrypt the connection".into(),
            ),
            R::KeyChanged { .. } => SshError::HostKeyRejected,
            R::Keys(k) => k.into(),
            other => SshError::Protocol(other.to_string()),
        }
    }
}

impl From<SftpError> for SshError {
    fn from(e: SftpError) -> Self {
        let text = match &e {
            SftpError::Status(s) => match s.status_code {
                StatusCode::NoSuchFile => "The file or folder was not found on the remote computer.".to_string(),
                StatusCode::PermissionDenied => {
                    "The remote computer does not allow this (permission denied).".to_string()
                }
                StatusCode::Failure => {
                    "The remote computer could not do this. The name may already be in use, the folder may not be empty, or there may be no space left.".to_string()
                }
                StatusCode::OpUnsupported => "The remote computer does not support this action.".to_string(),
                StatusCode::NoConnection | StatusCode::ConnectionLost => {
                    "The connection to the remote computer was lost.".to_string()
                }
                StatusCode::Eof => "The file ended earlier than expected.".to_string(),
                StatusCode::BadMessage => "The remote computer could not understand the request.".to_string(),
                StatusCode::Ok => "The remote computer reported an unknown problem.".to_string(),
            },
            SftpError::Timeout => {
                "The remote computer took too long to answer. Check the network connection.".to_string()
            }
            SftpError::IO(msg) => format!("The file transfer was interrupted: {msg}"),
            // The library reports a lost connection with these words.
            SftpError::UnexpectedBehavior(msg)
                if msg.contains("session closed") || msg.contains("sender dropped") || msg.contains("SendError") || msg.contains("RecvError") =>
            {
                "The connection to the remote computer was lost.".to_string()
            }
            other => format!("The file transfer service reported a problem: {other}"),
        };
        SshError::Sftp(text)
    }
}

impl From<russh::keys::Error> for SshError {
    fn from(e: russh::keys::Error) -> Self {
        SshError::KeyFile(format!("The key file could not be used ({e})."))
    }
}
