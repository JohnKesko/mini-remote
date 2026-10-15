//! VNC client sessions: connecting, logging in, and keeping a live copy of
//! the remote screen while forwarding mouse, keyboard and clipboard.
//!
//! The app talks to a running session only through [`SessionHandle`]:
//! it sends [`Command`]s, polls [`Event`]s, and reads the shared picture.

mod client;
mod error;
mod handshake;
pub mod messages;
mod rsa_aes;
pub mod session;
mod tls;

pub use codec::{CursorImage, Rect};
pub use parking_lot;
pub use session::*;
