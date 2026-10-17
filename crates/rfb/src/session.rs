//! Public interface of a VNC session. Everything the app needs is here.

use std::future::Future;
use std::io;
use std::sync::Arc;
use std::sync::mpsc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc as tokio_mpsc;

pub use codec::{CursorImage, Rect};

/// Any two-way byte stream: a TCP socket, an SSH tunnel channel, ...
pub trait AsyncStream: AsyncRead + AsyncWrite + Send + Unpin + 'static {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin + 'static> AsyncStream for T {}

pub type BoxedStream = Box<dyn AsyncStream>;

/// Open a plain TCP connection, ready to pass to [`start`].
pub async fn connect_tcp(host: String, port: u16) -> io::Result<BoxedStream> {
    let stream = tokio::net::TcpStream::connect((host.as_str(), port)).await?;
    stream.set_nodelay(true)?;
    Ok(Box::new(stream))
}

/// Picture quality. `Automatic` adapts to the measured network speed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Quality {
    #[default]
    Automatic,
    /// Lossless picture, uses the most network.
    Best,
    /// Slightly compressed photos and gradients, sharp text.
    Balanced,
    /// Strong compression for slow connections.
    Fast,
}

#[derive(Clone, Debug)]
pub struct SessionOptions {
    /// What to call the remote computer in messages, e.g. "raspberrypi.local".
    pub host_label: String,
    pub username: Option<String>,
    pub password: Option<String>,
    /// Allow login methods that do not encrypt the connection (plain VNC
    /// password or no password). Safe to turn on when going through an SSH tunnel.
    pub allow_unencrypted: bool,
    /// Fingerprint of the server identity the user trusted before, if any.
    pub known_identity: Option<String>,
    pub quality: Quality,
    /// Prefer the H.264 video encoding when the server offers it.
    pub video: bool,
    /// Let other viewers stay connected at the same time.
    pub shared: bool,
}

impl Default for SessionOptions {
    fn default() -> Self {
        SessionOptions {
            host_label: String::new(),
            username: None,
            password: None,
            allow_unencrypted: false,
            known_identity: None,
            quality: Quality::Automatic,
            video: false,
            shared: true,
        }
    }
}

/// Mouse button bits used in [`Command::Pointer`].
pub mod buttons {
    pub const LEFT: u16 = 1 << 0;
    pub const MIDDLE: u16 = 1 << 1;
    pub const RIGHT: u16 = 1 << 2;
    pub const WHEEL_UP: u16 = 1 << 3;
    pub const WHEEL_DOWN: u16 = 1 << 4;
    pub const WHEEL_LEFT: u16 = 1 << 5;
    pub const WHEEL_RIGHT: u16 = 1 << 6;
    pub const BACK: u16 = 1 << 7;
    pub const FORWARD: u16 = 1 << 8;
}

/// Requests from the app to the session.
#[derive(Clone, Debug)]
pub enum Command {
    /// Mouse position in remote screen pixels plus the held buttons
    /// ([`buttons`]). Send wheel "clicks" as a press followed by a release.
    Pointer { x: u16, y: u16, buttons: u16 },
    /// A key press or release. `keysym` is the X11 keysym. `scancode` is the
    /// physical key as a QEMU/XT scancode (0 when unknown); it is used when the
    /// server supports physical keys and the app asks for that mode.
    Key { down: bool, keysym: u32, scancode: u32 },
    /// Choose whether to send physical key positions (when supported) or only
    /// characters. Defaults to characters only.
    UsePhysicalKeys(bool),
    /// Local clipboard text changed; pass it to the remote computer.
    Clipboard(String),
    /// Ask the remote computer to change its screen size.
    SetDesktopSize { width: u16, height: u16 },
    SetQuality(Quality),
    SetVideo(bool),
    /// Ask for a complete fresh picture.
    Refresh,
    /// Answer to [`Event::NeedCredentials`].
    Credentials { username: Option<String>, password: String },
    /// Answer to [`Event::ConfirmIdentity`].
    TrustIdentity(bool),
    Disconnect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Connecting,
    Negotiating,
    Authenticating,
    Connected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdentityKind {
    /// Certificate used for the encrypted (TLS) login.
    TlsCertificate,
    /// RSA key used for the RSA-AES encrypted login.
    RsaKey,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerInfo {
    pub name: String,
    pub width: u16,
    pub height: u16,
    /// Human description of the login method, e.g. "Encrypted (RSA-AES-256)".
    pub security: String,
    pub encrypted: bool,
    /// e.g. "3.8"
    pub protocol_version: String,
    /// The version the server announced: "3.8" for most servers, "3.889" for
    /// a Mac's built-in Screen Sharing, "5.0" for RealVNC Server.
    pub server_version: String,
    /// Whether the server can change its screen size on request.
    pub can_resize: bool,
    /// Whether the server accepts physical key positions.
    pub physical_keys: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    /// Received data rate in kilobits per second.
    pub kbps: f32,
    /// Picture updates per second.
    pub updates_per_sec: f32,
    /// Round trip time, when it can be measured.
    pub latency_ms: Option<f32>,
    /// Name of the encoding used most recently.
    pub encoding: &'static str,
    /// Quality level currently in effect (resolved when `Automatic`).
    pub effective_quality: Quality,
    pub bytes_total: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisconnectKind {
    /// The user (or the app) closed the session.
    Closed,
    /// Wrong username or password. The app should ask again.
    AuthFailed,
    /// Could not reach the computer, or the network dropped. Worth retrying.
    Network,
    /// The server refused for another reason (too many tries, not allowed, ...).
    Refused,
    /// The user did not trust the server identity.
    Untrusted,
    /// Only unencrypted login offered and `allow_unencrypted` is off.
    NeedsUnencrypted,
    /// No login method we support.
    Unsupported,
    /// The server sent something we did not understand.
    Protocol,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisconnectReason {
    pub kind: DisconnectKind,
    /// Plain-English explanation suitable for showing to the user.
    pub message: String,
}

/// Notifications from the session to the app.
#[derive(Clone, Debug)]
pub enum Event {
    Phase(Phase),
    /// The server wants a password (and maybe a username) that was not given.
    /// Reply with [`Command::Credentials`] or [`Command::Disconnect`].
    NeedCredentials { need_username: bool },
    /// First connection, or the server identity changed. Reply with
    /// [`Command::TrustIdentity`]. `previous` is set when it changed.
    ConfirmIdentity { kind: IdentityKind, fingerprint: String, previous: Option<String> },
    Connected(ServerInfo),
    DesktopName(String),
    /// The remote screen size changed (the shared frame is already resized).
    Resized { width: u16, height: u16 },
    /// New mouse pointer image, or `None` to hide it.
    Cursor(Option<CursorImage>),
    Clipboard(String),
    Bell,
    /// A [`Command::SetDesktopSize`] request was refused.
    ResizeRefused(String),
    Stats(Stats),
    Disconnected(DisconnectReason),
}

/// The picture shared between the session and the app.
/// The app uploads the `dirty` areas and then clears the list.
pub struct SharedFrame {
    pub width: u16,
    pub height: u16,
    /// RGBA, row stride `width * 4`.
    pub pixels: Vec<u8>,
    pub dirty: Vec<Rect>,
    /// Increases every time the size changes.
    pub generation: u64,
}

impl SharedFrame {
    pub fn empty() -> Self {
        SharedFrame { width: 0, height: 0, pixels: Vec::new(), dirty: Vec::new(), generation: 0 }
    }
}

/// The app's end of a running session.
pub struct SessionHandle {
    commands: tokio_mpsc::UnboundedSender<Command>,
    events: mpsc::Receiver<Event>,
    frame: Arc<Mutex<SharedFrame>>,
    task: tokio::task::AbortHandle,
}

impl SessionHandle {
    pub fn send(&self, cmd: Command) {
        let _ = self.commands.send(cmd);
    }

    /// Next pending event, if any. Call every frame until it returns `None`.
    pub fn try_event(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }

    pub fn frame(&self) -> &Arc<Mutex<SharedFrame>> {
        &self.frame
    }
}

impl Drop for SessionHandle {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Disconnect);
        // Give the session a moment to close politely, then stop it.
        let task = self.task.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(500));
            task.abort();
        });
    }
}

/// Start a session on `runtime`. `connect` opens the byte stream (see
/// [`connect_tcp`]). `wake` is called whenever there is something new for
/// the app (an event or picture change), e.g. to request a repaint.
pub fn start<F>(
    runtime: &tokio::runtime::Handle,
    connect: F,
    options: SessionOptions,
    wake: Arc<dyn Fn() + Send + Sync>,
) -> SessionHandle
where
    F: Future<Output = io::Result<BoxedStream>> + Send + 'static,
{
    let (cmd_tx, cmd_rx) = tokio_mpsc::unbounded_channel();
    let (ev_tx, ev_rx) = mpsc::channel();
    let frame = Arc::new(Mutex::new(SharedFrame::empty()));
    let sink = crate::handshake::EventSink::new(ev_tx, wake);
    let task = runtime.spawn(crate::client::run(connect, options, cmd_rx, sink, frame.clone()));
    SessionHandle { commands: cmd_tx, events: ev_rx, frame, task: task.abort_handle() }
}
