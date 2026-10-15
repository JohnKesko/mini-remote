//! One open connection (one tab): runs the VNC session, reacts to its events,
//! reconnects after network drops, and sends keyboard and mouse input. When
//! the connection goes through secure remote login (SSH), the tunnel is
//! opened first and the screen connection runs inside it.

use std::collections::BTreeMap;
use std::future::Future;
use std::io;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use rfb::{
    BoxedStream, Command, CursorImage, DisconnectKind, DisconnectReason, Event, Phase, Quality, ServerInfo,
    SessionHandle, SessionOptions, SharedFrame, Stats,
};
use ssh::{SshError, SshSession, TunnelStream};

use crate::files::FilesPanel;
use crate::input::keyboard::{KeyConfig, KeyMsg, Keyboard};
use crate::input::mouse::WheelAccumulator;
use crate::model::{Connection, KeyboardMode, Settings, SshLogin};
use crate::remote_login::{self, HostKeyLink, Missing};
use crate::texture::RemoteTexture;

/// Wait this long after the window stops changing size before asking the
/// remote computer to change its screen size.
const RESIZE_DELAY: Duration = Duration::from_millis(300);
/// Save a thumbnail this long after the first picture arrives.
const THUMBNAIL_DELAY: Duration = Duration::from_secs(4);

pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// What the app needs to hand a session.
pub struct SessionEnv<'a> {
    pub runtime: &'a tokio::runtime::Handle,
    pub wake: &'a Wake,
    pub settings: &'a Settings,
    pub trusted: &'a BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Connecting(Phase),
    Connected,
    /// The network dropped; trying again at `at`.
    Reconnecting { attempt: u32, at: Instant, message: String },
    Failed(DisconnectReason),
    /// Closed on purpose; the tab goes away.
    Closed,
}

impl Status {
    pub fn is_connected(&self) -> bool {
        matches!(self, Status::Connected)
    }
}

/// Which login a password prompt is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginPurpose {
    /// The screen sharing login (also used for SSH with "same password").
    Screen,
    /// A separate SSH password.
    Ssh,
    /// The passphrase of an SSH key file.
    SshPassphrase,
}

pub struct CredentialsPrompt {
    pub purpose: LoginPurpose,
    pub need_username: bool,
    pub username: String,
    pub password: String,
    pub remember: bool,
    pub show_password: bool,
    /// Shown above the fields, e.g. after a wrong password.
    pub error: Option<String>,
    /// Answer by starting a new connection (the old one already ended).
    reconnect: bool,
}

/// Whose identity is being confirmed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdentitySource {
    /// The screen sharing server (TLS certificate or RSA key).
    Screen,
    /// The secure remote login (SSH) server.
    Ssh { host: String, host_key: String },
}

pub struct IdentityPrompt {
    pub source: IdentitySource,
    pub fingerprint: String,
    pub previous: Option<String>,
}

pub enum Prompt {
    Credentials(CredentialsPrompt),
    Identity(IdentityPrompt),
}

/// Things the app should take care of after a session event.
pub enum Notice {
    /// Logged in and showing the screen.
    Connected,
    /// Text copied on the remote computer.
    RemoteClipboard(String),
    /// The user trusted this server identity.
    Trust { host_key: String, fingerprint: String },
    /// Keep this password in the password store.
    StorePassword { key: String, password: String },
    /// Save a thumbnail of the current screen.
    SaveThumbnail,
    /// A short message for the user.
    Toast(String),
}

#[derive(Default)]
pub struct MouseState {
    pub buttons: u16,
    pub last: Option<(u16, u16)>,
    pub wheel: WheelAccumulator,
}

/// The remote mouse pointer image.
#[derive(Default)]
pub struct RemoteCursor {
    /// The server sends its own pointer images.
    pub known: bool,
    pub image: Option<CursorImage>,
    pub texture: Option<egui::TextureHandle>,
    /// `image` changed and `texture` must be rebuilt.
    pub changed: bool,
}

#[derive(Default)]
struct ResizeState {
    /// Size (in screen pixels) the view area has had since `since`.
    wanted: Option<(u16, u16)>,
    since: Option<Instant>,
    sent: Option<(u16, u16)>,
    /// The server said no; stop asking for this session.
    refused: bool,
}

type TunnelResult = Result<(SshSession, TunnelStream), SshError>;

pub struct SessionTab {
    pub id: u64,
    pub conn: Connection,
    /// The connection is in the saved list.
    pub saved: bool,
    /// Ask whether to save the connection once it works (found on the network).
    pub offer_save: bool,
    handle: Option<SessionHandle>,
    /// Picture of the current (or last) session.
    frame: Arc<Mutex<SharedFrame>>,
    pub status: Status,
    pub prompt: Option<Prompt>,
    pub server: Option<ServerInfo>,
    pub desktop_name: Option<String>,
    pub stats: Option<Stats>,
    pub texture: Option<RemoteTexture>,
    pub cursor: RemoteCursor,
    pub keyboard: Keyboard,
    pub mouse: MouseState,
    /// Password for this tab, kept in memory for reconnecting.
    password: Option<String>,
    /// Separate SSH password or key passphrase, kept in memory.
    ssh_password: Option<String>,
    /// Passwords typed in a prompt that should be remembered once they work.
    remember_after_login: Vec<(String, String)>,
    need_username: bool,
    ever_connected: bool,
    attempt: u32,
    resize: ResizeState,
    /// Size of the view area in screen pixels (set by the view every frame).
    pub view_pixels: Option<(u16, u16)>,
    thumbnail_due: Option<Instant>,
    physical_keys: bool,
    closing: bool,
    /// The SSH tunnel is being opened.
    tunnel: Option<mpsc::Receiver<TunnelResult>>,
    /// The logged-in SSH connection carrying the screen (when tunnelled).
    ssh_session: Option<SshSession>,
    /// The last failure came from secure remote login, not screen sharing.
    pub failed_in_ssh: bool,
    /// SSH identity questions from the network side.
    pub host_keys: Arc<HostKeyLink>,
    ssh_answer: Option<tokio::sync::oneshot::Sender<bool>>,
    /// When the remote computer last rang its bell.
    pub bell_at: Option<Instant>,
    /// Show the connection info panel.
    pub info_open: bool,
    /// When the mouse was last near the toolbar.
    pub toolbar_seen: Option<Instant>,
    pub files: FilesPanel,
}

impl SessionTab {
    pub fn new(id: u64, conn: Connection, saved: bool, password: Option<String>, ssh_password: Option<String>) -> Self {
        SessionTab {
            id,
            conn,
            saved,
            offer_save: false,
            handle: None,
            frame: Arc::new(Mutex::new(SharedFrame::empty())),
            status: Status::Connecting(Phase::Connecting),
            prompt: None,
            server: None,
            desktop_name: None,
            stats: None,
            texture: None,
            cursor: RemoteCursor::default(),
            keyboard: Keyboard::new(),
            mouse: MouseState::default(),
            password,
            ssh_password,
            remember_after_login: Vec::new(),
            need_username: false,
            ever_connected: false,
            attempt: 0,
            resize: ResizeState::default(),
            view_pixels: None,
            thumbnail_due: None,
            physical_keys: false,
            closing: false,
            tunnel: None,
            ssh_session: None,
            failed_in_ssh: false,
            host_keys: Arc::new(HostKeyLink::default()),
            ssh_answer: None,
            bell_at: None,
            info_open: false,
            toolbar_seen: None,
            files: FilesPanel::new(),
        }
    }

    /// Start (or restart) the connection.
    pub fn connect(&mut self, env: &SessionEnv) {
        // Drop the old session first so it says goodbye to the server.
        self.handle = None;
        self.tunnel = None;
        self.ssh_session = None;
        if let Some(tex) = &mut self.texture {
            tex.invalidate();
        }
        self.status = Status::Connecting(Phase::Connecting);
        self.prompt = None;
        self.stats = None;
        self.keyboard = Keyboard::new();
        self.mouse = MouseState::default();
        self.resize = ResizeState { refused: self.resize.refused, ..ResizeState::default() };
        self.physical_keys = false;
        self.closing = false;
        self.failed_in_ssh = false;

        if self.conn.ssh.enabled {
            self.open_tunnel(env);
        } else {
            self.start_screen(env, connect_future(&self.conn), false);
        }
    }

    /// First log in securely (SSH), then run the screen connection inside it.
    fn open_tunnel(&mut self, env: &SessionEnv) {
        let cfg = match remote_login::ssh_config(&self.conn, self.password.as_deref(), self.ssh_password.as_deref()) {
            Ok(cfg) => cfg,
            Err(missing) => {
                let purpose = match self.conn.ssh.login {
                    SshLogin::Password => LoginPurpose::Ssh,
                    SshLogin::KeyFile(_) => LoginPurpose::SshPassphrase,
                    _ => LoginPurpose::Screen,
                };
                self.ask_login(purpose, missing == Missing::Username || self.conn.ssh_username().is_empty(), None);
                self.status = Status::Connecting(Phase::Authenticating);
                return;
            }
        };
        let check = remote_login::host_key_check(
            self.host_keys.clone(),
            env.trusted.get(&self.conn.ssh_host_key()).cloned(),
            env.wake.clone(),
        );
        let vnc_port = self.conn.port;
        let (tx, rx) = mpsc::channel();
        let wake = env.wake.clone();
        env.runtime.spawn(async move {
            let result = async {
                let session = SshSession::connect(cfg, check).await?;
                let tunnel = session.open_tunnel("localhost", vnc_port).await?;
                Ok((session, tunnel))
            }
            .await;
            let _ = tx.send(result);
            wake();
        });
        self.tunnel = Some(rx);
    }

    fn start_screen<F>(&mut self, env: &SessionEnv, connect: F, tunnelled: bool)
    where
        F: Future<Output = io::Result<BoxedStream>> + Send + 'static,
    {
        let username = self.conn.username.trim();
        let options = SessionOptions {
            host_label: self.conn.host.clone(),
            username: (!username.is_empty()).then(|| username.to_string()),
            password: self.password.clone(),
            // The tunnel encrypts everything, so the plain VNC login is safe inside it.
            allow_unencrypted: self.conn.allow_unencrypted || tunnelled,
            known_identity: env.trusted.get(&self.conn.host_key()).cloned(),
            quality: self.conn.quality,
            video: self.conn.video,
            shared: true,
        };
        self.handle = Some(rfb::start(env.runtime, connect, options, env.wake.clone()));
    }

    /// The screen connection goes through an SSH tunnel right now.
    pub fn tunnelled(&self) -> bool {
        self.ssh_session.is_some()
    }

    /// The logged-in SSH connection, if the screen goes through one.
    pub fn ssh_session(&self) -> Option<&SshSession> {
        self.ssh_session.as_ref().filter(|s| !s.is_closed())
    }

    /// Opening the SSH tunnel (before the screen connection starts).
    pub fn in_ssh_stage(&self) -> bool {
        self.tunnel.is_some()
    }

    /// Passwords known for this tab (screen, SSH).
    pub fn passwords(&self) -> (Option<&str>, Option<&str>) {
        (self.password.as_deref(), self.ssh_password.as_deref())
    }

    /// The picture to upload: the running session's, or the last one.
    pub fn current_frame(&mut self) -> Arc<Mutex<SharedFrame>> {
        if let Some(handle) = &self.handle {
            let new = handle.frame();
            if !Arc::ptr_eq(new, &self.frame) && new.lock().width > 0 {
                self.frame = new.clone();
            }
        }
        self.frame.clone()
    }

    /// Copy of the current picture (width, height, RGBA).
    pub fn snapshot(&mut self) -> Option<(u32, u32, Vec<u8>)> {
        let frame = self.current_frame();
        let f = frame.lock();
        let (w, h) = (u32::from(f.width), u32::from(f.height));
        let len = (w * h * 4) as usize;
        (w > 0 && h > 0 && f.pixels.len() >= len).then(|| (w, h, f.pixels[..len].to_vec()))
    }

    pub fn has_picture(&self) -> bool {
        self.texture.is_some()
    }

    /// Handle everything the session reported since last time.
    pub fn poll(&mut self, env: &SessionEnv) -> Vec<Notice> {
        let mut notices = Vec::new();

        if self.prompt.is_none()
            && let Some(q) = self.host_keys.take()
        {
            log::info!("{} asks to confirm its SSH identity {}", q.info.host, q.info.fingerprint);
            self.prompt = Some(Prompt::Identity(IdentityPrompt {
                source: IdentitySource::Ssh {
                    host: q.info.host.clone(),
                    host_key: remote_login::trusted_key(&q.info.host, q.info.port),
                },
                fingerprint: q.info.fingerprint.clone(),
                previous: q.previous.clone(),
            }));
            self.ssh_answer = Some(q.answer);
        }

        if let Some(rx) = &self.tunnel {
            match rx.try_recv() {
                Ok(Ok((session, tunnel))) => {
                    self.tunnel = None;
                    log::info!("secure tunnel to {} is open", self.conn.ssh_host());
                    self.ssh_session = Some(session);
                    let stream: BoxedStream = Box::new(tunnel);
                    self.start_screen(env, async move { Ok(stream) }, true);
                }
                Ok(Err(err)) => {
                    self.tunnel = None;
                    self.on_ssh_error(err, env);
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.tunnel = None;
                    self.on_ssh_error(SshError::Closed, env);
                }
            }
        }

        while let Some(event) = self.handle.as_ref().and_then(SessionHandle::try_event) {
            self.on_event(event, env, &mut notices);
        }
        notices
    }

    fn on_event(&mut self, event: Event, env: &SessionEnv, notices: &mut Vec<Notice>) {
        match event {
            Event::Phase(phase) => {
                if !self.status.is_connected() {
                    self.status = Status::Connecting(phase);
                }
            }
            Event::NeedCredentials { need_username } => {
                self.need_username = need_username;
                self.ask_login(LoginPurpose::Screen, need_username, None);
                self.prompt_mut_credentials(|p| p.reconnect = false);
            }
            Event::ConfirmIdentity { kind: _, fingerprint, previous } => {
                log::info!("{} asks to confirm its identity {fingerprint}", self.conn.host);
                self.prompt = Some(Prompt::Identity(IdentityPrompt { source: IdentitySource::Screen, fingerprint, previous }));
            }
            Event::Connected(info) => {
                log::info!("connected to {} ({}, {}x{})", self.conn.host, info.security, info.width, info.height);
                self.status = Status::Connected;
                self.prompt = None;
                self.ever_connected = true;
                self.attempt = 0;
                self.thumbnail_due = Some(Instant::now() + THUMBNAIL_DELAY);
                self.server = Some(info);
                self.apply_keyboard_mode();
                for (key, password) in self.remember_after_login.drain(..) {
                    notices.push(Notice::StorePassword { key, password });
                }
                notices.push(Notice::Connected);
            }
            Event::DesktopName(name) => self.desktop_name = Some(name),
            Event::Resized { width, height } => {
                if let Some(info) = &mut self.server {
                    info.width = width;
                    info.height = height;
                }
            }
            Event::Cursor(image) => {
                self.cursor.known = true;
                self.cursor.image = image.filter(|c| c.width > 0 && c.height > 0);
                self.cursor.changed = true;
            }
            Event::Clipboard(text) => {
                if self.conn.share_clipboard && env.settings.share_clipboard {
                    notices.push(Notice::RemoteClipboard(text));
                }
            }
            Event::Bell => self.bell_at = Some(Instant::now()),
            Event::ResizeRefused(message) => {
                log::info!("screen size change refused: {message}");
                if !self.resize.refused {
                    notices.push(Notice::Toast(
                        "The Pi's screen size can't be changed right now. This only works when no monitor is plugged into the Pi."
                            .into(),
                    ));
                }
                self.resize.refused = true;
            }
            Event::Stats(stats) => self.stats = Some(stats),
            Event::Disconnected(reason) => self.on_disconnected(reason, env),
        }
    }

    fn ask_login(&mut self, purpose: LoginPurpose, need_username: bool, error: Option<String>) {
        let remember = match purpose {
            LoginPurpose::Screen => self.conn.remember_password,
            LoginPurpose::Ssh | LoginPurpose::SshPassphrase => true,
        };
        let username = match purpose {
            LoginPurpose::Screen => self.conn.username.clone(),
            _ => self.conn.ssh_username().to_string(),
        };
        self.prompt = Some(Prompt::Credentials(CredentialsPrompt {
            purpose,
            need_username: need_username && purpose != LoginPurpose::SshPassphrase,
            username,
            password: String::new(),
            remember,
            show_password: false,
            error,
            reconnect: true,
        }));
    }

    fn prompt_mut_credentials(&mut self, f: impl FnOnce(&mut CredentialsPrompt)) {
        if let Some(Prompt::Credentials(p)) = &mut self.prompt {
            f(p);
        }
    }

    fn on_ssh_error(&mut self, err: SshError, env: &SessionEnv) {
        log::info!("secure login to {} failed: {err}", self.conn.ssh_host());
        self.ssh_session = None;
        if self.closing {
            self.status = Status::Closed;
            return;
        }
        let message = err.to_string();
        let failed = |kind| DisconnectReason { kind, message: message.clone() };
        match err {
            SshError::AuthFailed if remote_login::uses_password(&self.conn) => {
                let purpose =
                    if self.conn.ssh.login == SshLogin::Password { LoginPurpose::Ssh } else { LoginPurpose::Screen };
                if purpose == LoginPurpose::Ssh {
                    self.ssh_password = None;
                } else {
                    self.password = None;
                }
                self.remember_after_login.clear();
                self.ask_login(
                    purpose,
                    true,
                    Some("The Pi did not accept this username and password for secure remote login.".into()),
                );
                self.status = Status::Failed(failed(DisconnectKind::AuthFailed));
            }
            SshError::PassphraseNeeded | SshError::WrongPassphrase => {
                self.ssh_password = None;
                self.ask_login(LoginPurpose::SshPassphrase, false, Some(message.clone()));
                self.status = Status::Failed(failed(DisconnectKind::AuthFailed));
            }
            SshError::HostKeyRejected => self.status = Status::Closed,
            SshError::Network(_) | SshError::Closed if self.ever_connected && env.settings.auto_reconnect => {
                self.attempt += 1;
                let at = Instant::now() + backoff(self.attempt);
                self.status = Status::Reconnecting { attempt: self.attempt, at, message };
            }
            _ => {
                self.failed_in_ssh = true;
                self.status = Status::Failed(failed(DisconnectKind::Refused));
            }
        }
    }

    fn on_disconnected(&mut self, reason: DisconnectReason, env: &SessionEnv) {
        log::info!("disconnected from {}: {:?} {}", self.conn.host, reason.kind, reason.message);
        self.handle = None;
        self.ssh_session = None;
        self.keyboard = Keyboard::new();
        self.mouse = MouseState::default();
        if self.closing {
            self.status = Status::Closed;
            return;
        }
        match reason.kind {
            DisconnectKind::AuthFailed => {
                self.password = None;
                self.remember_after_login.clear();
                let message = if reason.message.is_empty() {
                    "That password didn't work. Please try again.".to_string()
                } else {
                    reason.message.clone()
                };
                let need_username = self.need_username || !self.conn.username.is_empty();
                self.ask_login(LoginPurpose::Screen, need_username, Some(message));
                self.status = Status::Failed(reason);
            }
            DisconnectKind::Network if self.ever_connected && env.settings.auto_reconnect => {
                self.attempt += 1;
                let at = Instant::now() + backoff(self.attempt);
                self.status = Status::Reconnecting { attempt: self.attempt, at, message: reason.message };
                self.prompt = None;
            }
            _ => {
                self.status = Status::Failed(reason);
                self.prompt = None;
            }
        }
    }

    /// Timers: reconnecting, screen size requests, thumbnails. Returns when
    /// it wants to be called again.
    pub fn tick(&mut self, env: &SessionEnv, notices: &mut Vec<Notice>) -> Option<Instant> {
        let now = Instant::now();
        let mut next: Option<Instant> = None;
        let mut soon = |t: Instant| next = Some(next.map_or(t, |n: Instant| n.min(t)));

        if let Status::Reconnecting { at, .. } = self.status {
            if now >= at {
                self.connect(env);
            } else {
                // Repaint every second so the countdown moves.
                soon(at.min(now + Duration::from_secs(1)));
            }
        }

        if let Some(due) = self.thumbnail_due {
            if now >= due {
                self.thumbnail_due = None;
                notices.push(Notice::SaveThumbnail);
            } else {
                soon(due);
            }
        }

        if let Some(at) = self.resize_tick(now) {
            soon(at);
        }
        next
    }

    fn resize_tick(&mut self, now: Instant) -> Option<Instant> {
        let can = self.conn.resize_remote
            && !self.resize.refused
            && self.status.is_connected()
            && self.server.as_ref().is_some_and(|s| s.can_resize);
        if !can {
            self.resize.wanted = None;
            return None;
        }
        let wanted = self.view_pixels.filter(|(w, h)| *w >= 200 && *h >= 150)?;
        if self.resize.wanted != Some(wanted) {
            self.resize.wanted = Some(wanted);
            self.resize.since = Some(now);
        }
        let since = self.resize.since?;
        let current = self.server.as_ref().map(|s| (s.width, s.height));
        if self.resize.sent == Some(wanted) || current == Some(wanted) {
            return None;
        }
        if now.duration_since(since) >= RESIZE_DELAY {
            self.resize.sent = Some(wanted);
            self.send(Command::SetDesktopSize { width: wanted.0, height: wanted.1 });
            None
        } else {
            Some(since + RESIZE_DELAY)
        }
    }

    fn send(&self, cmd: Command) {
        if let Some(handle) = &self.handle {
            handle.send(cmd);
        }
    }

    /// Keyboard and mouse input are allowed right now.
    pub fn accepts_input(&self) -> bool {
        self.status.is_connected() && self.prompt.is_none() && !self.conn.view_only
    }

    pub fn key_config(&self, settings: &Settings) -> KeyConfig {
        KeyConfig {
            meta: settings.meta_key,
            alt: settings.alt_key,
            physical: self.physical_keys,
            caps_lock_reports_state: crate::platform::caps_lock_reports_state(),
        }
    }

    pub fn send_keys(&self, msgs: &[KeyMsg]) {
        if !self.accepts_input() {
            return;
        }
        for m in msgs {
            self.send(Command::Key { down: m.down, keysym: m.keysym, scancode: m.scancode });
        }
    }

    /// Let go of all keys and mouse buttons (focus moved away).
    pub fn release_input(&mut self) {
        let msgs = self.keyboard.release_all();
        if self.status.is_connected() {
            for m in msgs {
                self.send(Command::Key { down: m.down, keysym: m.keysym, scancode: m.scancode });
            }
            if self.mouse.buttons != 0
                && let Some((x, y)) = self.mouse.last
            {
                self.send(Command::Pointer { x, y, buttons: 0 });
            }
        }
        self.mouse.buttons = 0;
        self.mouse.wheel.reset();
    }

    /// Press a key combination and let go again (from the "Send keys" menu).
    pub fn send_combo(&self, keys: &[(u32, u32)]) {
        if !self.accepts_input() {
            return;
        }
        for &(keysym, scancode) in keys {
            self.send(Command::Key { down: true, keysym, scancode });
        }
        for &(keysym, scancode) in keys.iter().rev() {
            self.send(Command::Key { down: false, keysym, scancode });
        }
    }

    /// Type text as key presses (for places where the clipboard can't be used).
    pub fn type_text(&self, text: &str) {
        if !self.accepts_input() {
            return;
        }
        for c in text.chars().filter(|c| *c != '\r').take(4000) {
            if let Some(keysym) = crate::input::keymap::char_keysym(c) {
                self.send(Command::Key { down: true, keysym, scancode: 0 });
                self.send(Command::Key { down: false, keysym, scancode: 0 });
            }
        }
    }

    pub fn send_pointer(&mut self, x: u16, y: u16, buttons: u16) {
        if !self.accepts_input() {
            return;
        }
        self.mouse.last = Some((x, y));
        self.send(Command::Pointer { x, y, buttons });
    }

    pub fn send_clipboard(&self, text: &str) {
        if self.status.is_connected() && !self.conn.view_only {
            self.send(Command::Clipboard(text.to_owned()));
        }
    }

    pub fn refresh(&self) {
        self.send(Command::Refresh);
    }

    pub fn set_quality(&mut self, quality: Quality) {
        self.conn.quality = quality;
        self.send(Command::SetQuality(quality));
    }

    pub fn set_video(&mut self, video: bool) {
        self.conn.video = video;
        self.send(Command::SetVideo(video));
    }

    /// Apply changed connection settings to the running session.
    pub fn update_connection(&mut self, conn: Connection) {
        let old = std::mem::replace(&mut self.conn, conn);
        if old.quality != self.conn.quality {
            self.send(Command::SetQuality(self.conn.quality));
        }
        if old.video != self.conn.video {
            self.send(Command::SetVideo(self.conn.video));
        }
        if old.keyboard_mode != self.conn.keyboard_mode {
            self.release_input();
            self.apply_keyboard_mode();
        }
        if old.resize_remote != self.conn.resize_remote {
            self.resize = ResizeState::default();
        }
    }

    fn apply_keyboard_mode(&mut self) {
        let supported = self.server.as_ref().is_some_and(|s| s.physical_keys);
        let physical = self.conn.keyboard_mode == KeyboardMode::Physical && supported;
        if physical != self.physical_keys || physical {
            self.send(Command::UsePhysicalKeys(physical));
        }
        self.physical_keys = physical;
    }

    /// The physical keyboard mode is active (the server supports it).
    pub fn physical_keys_active(&self) -> bool {
        self.physical_keys
    }

    /// Answer the password prompt.
    pub fn submit_credentials(&mut self, env: &SessionEnv) {
        let Some(Prompt::Credentials(p)) = self.prompt.take() else { return };
        let username = p.username.trim().to_string();
        match p.purpose {
            LoginPurpose::Screen => {
                if p.need_username || !username.is_empty() {
                    self.conn.username = username.clone();
                }
                self.conn.remember_password = p.remember;
                self.password = Some(p.password.clone());
                if p.remember {
                    self.remember_after_login.push((self.conn.secret_key(), p.password.clone()));
                }
            }
            LoginPurpose::Ssh | LoginPurpose::SshPassphrase => {
                if p.need_username && !username.is_empty() {
                    self.conn.ssh.username = username.clone();
                }
                self.ssh_password = Some(p.password.clone());
                if p.remember {
                    self.remember_after_login.push((self.conn.ssh_secret_key(), p.password.clone()));
                }
            }
        }
        if p.reconnect || self.handle.is_none() {
            self.attempt = 0;
            self.connect(env);
        } else {
            self.send(Command::Credentials {
                username: (p.need_username || !username.is_empty()).then_some(username),
                password: p.password,
            });
            self.status = Status::Connecting(Phase::Authenticating);
        }
    }

    /// Answer an "is this your Pi?" question.
    pub fn answer_identity(&mut self, trust: bool) -> Option<Notice> {
        let Some(Prompt::Identity(p)) = self.prompt.take() else { return None };
        match &p.source {
            IdentitySource::Screen => {
                self.send(Command::TrustIdentity(trust));
                if trust {
                    Some(Notice::Trust { host_key: self.conn.host_key(), fingerprint: p.fingerprint })
                } else {
                    self.close();
                    None
                }
            }
            IdentitySource::Ssh { host_key, .. } => {
                if let Some(answer) = self.ssh_answer.take() {
                    let _ = answer.send(trust);
                }
                trust.then(|| Notice::Trust { host_key: host_key.clone(), fingerprint: p.fingerprint.clone() })
            }
        }
    }

    /// Use this password for the next connection attempt.
    pub fn set_password(&mut self, password: String) {
        self.password = Some(password);
    }

    pub fn set_ssh_password(&mut self, password: String) {
        self.ssh_password = Some(password);
    }

    /// Try again now (after an error, or skipping the reconnect countdown).
    pub fn retry(&mut self, env: &SessionEnv) {
        self.attempt = 0;
        self.connect(env);
    }

    /// Disconnect on purpose. The tab is removed.
    pub fn close(&mut self) {
        self.closing = true;
        self.prompt = None;
        if self.status.is_connected() {
            self.release_input();
        }
        if let Some(answer) = self.ssh_answer.take() {
            let _ = answer.send(false);
        }
        self.files.cancel_all();
        // Dropping the handle tells the session to disconnect.
        self.handle = None;
        self.tunnel = None;
        self.ssh_session = None;
        self.status = Status::Closed;
    }

    pub fn is_closed(&self) -> bool {
        self.status == Status::Closed
    }

    /// Name for the tab and window title.
    pub fn title(&self) -> &str {
        self.conn.display_name()
    }
}

/// Wait before reconnect attempt `n`: 1, 2, 4, 8, 15, 30, 30, ... seconds.
pub fn backoff(attempt: u32) -> Duration {
    let secs = match attempt {
        0 | 1 => 1,
        2 => 2,
        3 => 4,
        4 => 8,
        5 => 15,
        _ => 30,
    };
    Duration::from_secs(secs)
}

/// Open a direct network connection for the screen.
fn connect_future(conn: &Connection) -> impl Future<Output = io::Result<BoxedStream>> + Send + 'static {
    rfb::connect_tcp(conn.host.trim().to_string(), conn.port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff(1), Duration::from_secs(1));
        assert_eq!(backoff(3), Duration::from_secs(4));
        assert_eq!(backoff(6), Duration::from_secs(30));
        assert_eq!(backoff(60), Duration::from_secs(30));
    }
}
