//! Secure remote login (SSH): tunnels for the screen connection and file transfer.
//!
//! * [`SshSession::connect`] logs in to the remote computer (checking its identity first).
//! * [`SshSession::open_tunnel`] gives a byte stream to a port on the remote computer, for
//!   example the screen-sharing server on the Raspberry Pi itself.
//! * [`SshSession::sftp`] gives a [`Sftp`] handle to browse, copy, rename and delete files.

mod error;
mod session;
mod sftp;

pub use error::SshError;
pub use session::{
    HostKeyCheck, HostKeyInfo, SshAuth, SshConfig, SshSession, TunnelStream, default_key_files,
};
pub use sftp::{CancelToken, Progress, RemoteEntry, Sftp};
