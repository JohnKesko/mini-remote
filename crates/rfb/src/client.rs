//! The running session: one task reads and decodes what the server sends,
//! another writes our messages, and a control loop handles app commands,
//! clipboard exchange, statistics and automatic quality.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use codec::{Decoders, Framebuffer, PixelFormat, Rect, RfbReader, encoding as enc};
use parking_lot::Mutex;
use tokio::io::{AsyncWriteExt, WriteHalf};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::error::SessionError;
use crate::handshake::{self, EventSink, Prompter, decode_text};
use crate::messages::{self as msg, Screen, clip, fence};
use crate::session::{
    BoxedStream, Command, DisconnectKind, Event, Phase, Quality, ServerInfo, SessionOptions, SharedFrame, Stats,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_CLIPBOARD: usize = 10 * 1024 * 1024;

/// Messages from the reader task to the control loop.
enum Internal {
    Size(u16, u16),
    Screens(Vec<Screen>),
    CanResize,
    PhysicalKeys,
    ExtendedMouse,
    FenceSupported,
    FenceResponse(Vec<u8>),
    ClipboardCaps,
    ClipboardNotify,
    ClipboardRequest,
    ClipboardPeek,
    RemoteClipboard(String),
    /// One complete update: bytes received and time it took.
    UpdateTiming { bytes: u64, elapsed: Duration },
    FirstUpdate,
    Ended(Result<(), SessionError>),
}

#[derive(Default)]
struct Counters {
    bytes: AtomicU64,
    updates: AtomicU64,
    last_encoding: AtomicI32,
}

pub async fn run<F>(
    connect: F,
    mut options: SessionOptions,
    mut commands: UnboundedReceiver<Command>,
    events: EventSink,
    shared: Arc<Mutex<SharedFrame>>,
) where
    F: std::future::Future<Output = std::io::Result<BoxedStream>> + Send + 'static,
{
    let result = run_inner(connect, &mut options, &mut commands, &events, shared).await;
    let reason = match result {
        Ok(()) => SessionError::closed().into_reason(),
        Err(e) => e.into_reason(),
    };
    log::info!("session ended: {:?}", reason);
    events.send(Event::Disconnected(reason));
}

fn host_only(label: &str) -> String {
    let l = label.trim();
    if let Some(rest) = l.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest).to_string();
    }
    match l.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') && p.chars().all(|c| c.is_ascii_digit()) => h.to_string(),
        _ => l.to_string(),
    }
}

async fn run_inner<F>(
    connect: F,
    options: &mut SessionOptions,
    commands: &mut UnboundedReceiver<Command>,
    events: &EventSink,
    shared: Arc<Mutex<SharedFrame>>,
) -> Result<(), SessionError>
where
    F: std::future::Future<Output = std::io::Result<BoxedStream>> + Send + 'static,
{
    events.send(Event::Phase(Phase::Connecting));
    let label = options.host_label.clone();
    let stream = match tokio::time::timeout(CONNECT_TIMEOUT, connect).await {
        Err(_) => {
            return Err(SessionError::new(
                DisconnectKind::Network,
                format!("Could not reach {label}. Check that it is switched on and on the same network."),
            ));
        }
        Ok(Err(e)) => return Err(connect_error(&label, e)),
        Ok(Ok(s)) => s,
    };
    events.send(Event::Phase(Phase::Negotiating));

    let mut prompter = Prompter { options, commands, events, host: host_only(&label) };
    let neg = handshake::negotiate(stream, &mut prompter).await?;
    let options = prompter.options;
    let commands = prompter.commands;
    log::info!(
        "connected to {:?}: {}x{}, security {}, server pixel format {:?}",
        neg.init.name,
        neg.init.width,
        neg.init.height,
        neg.security,
        neg.init.pixel_format
    );

    let (width, height) = (neg.init.width, neg.init.height);
    {
        let mut f = shared.lock();
        f.width = width;
        f.height = height;
        f.pixels = Framebuffer::new(width, height).data().to_vec();
        f.dirty = vec![Rect::new(0, 0, width, height)];
        f.generation += 1;
    }

    let (rd, wr) = tokio::io::split(neg.stream);
    let reader = RfbReader::new(Box::new(rd));
    let (out_tx, out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let writer = tokio::spawn(writer_loop(wr, out_rx));

    let mut control = Control {
        out: out_tx.clone(),
        events: events.clone(),
        options: options.clone(),
        info: ServerInfo {
            name: neg.init.name.clone(),
            width,
            height,
            security: neg.security.clone(),
            encrypted: neg.encrypted,
            protocol_version: neg.version.clone(),
            can_resize: false,
            physical_keys: false,
        },
        width,
        height,
        screens: Vec::new(),
        extended_mouse: false,
        use_physical_keys: false,
        fence_supported: false,
        fence_sent: None,
        latency_ms: None,
        ext_clipboard: false,
        local_clipboard: None,
        last_remote_clipboard: None,
        remote_clipboard_at: None,
        effective: initial_quality(options.quality),
        link_kbps: None,
        auto_votes: 0,
        connected: false,
        start: Instant::now(),
    };

    let _ = out_tx.send(msg::set_pixel_format(&PixelFormat::RGBX));
    let _ = out_tx.send(msg::set_encodings(&control.encodings()));
    let _ = out_tx.send(msg::update_request(false, 0, 0, width, height));

    let counters = Arc::new(Counters::default());
    let (int_tx, mut int_rx) = mpsc::unbounded_channel();
    let reader_task = tokio::spawn(reader_loop(
        reader,
        Framebuffer::new(width, height),
        shared,
        out_tx.clone(),
        int_tx.clone(),
        events.clone(),
        counters.clone(),
    ));
    drop(out_tx);

    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_bytes = 0u64;
    let mut last_updates = 0u64;
    let mut last_tick = Instant::now();

    let result = loop {
        tokio::select! {
            cmd = commands.recv() => match cmd {
                None | Some(Command::Disconnect) => break Ok(()),
                Some(cmd) => control.command(cmd),
            },
            m = int_rx.recv() => match m {
                None => break Err(SessionError::closed()),
                Some(Internal::Ended(r)) => break r.and(Err(SessionError::new(
                    DisconnectKind::Network, "The connection was closed by the other computer."))),
                Some(m) => control.internal(m),
            },
            _ = tick.tick() => {
                let now = Instant::now();
                let secs = now.duration_since(last_tick).as_secs_f32().max(0.001);
                last_tick = now;
                let bytes = counters.bytes.load(Ordering::Relaxed);
                let updates = counters.updates.load(Ordering::Relaxed);
                let stats = Stats {
                    kbps: (bytes - last_bytes) as f32 * 8.0 / 1000.0 / secs,
                    updates_per_sec: (updates - last_updates) as f32 / secs,
                    latency_ms: control.latency_ms,
                    encoding: enc::name(counters.last_encoding.load(Ordering::Relaxed)),
                    effective_quality: control.effective,
                    bytes_total: bytes,
                };
                last_bytes = bytes;
                last_updates = updates;
                if control.connected {
                    events.send(Event::Stats(stats));
                }
                control.tick();
            }
        }
        if writer.is_finished() {
            break Err(SessionError::new(DisconnectKind::Network, "The connection was lost."));
        }
    };
    drop(control);
    reader_task.abort();
    let _ = tokio::time::timeout(Duration::from_millis(500), writer).await;
    result
}

fn connect_error(label: &str, e: std::io::Error) -> SessionError {
    use std::io::ErrorKind::*;
    let message = match e.kind() {
        ConnectionRefused => format!(
            "{label} refused the connection. Check that screen sharing (VNC) is turned on, for example in Raspberry Pi Configuration > Interfaces > VNC."
        ),
        TimedOut => format!("{label} did not answer. Check that it is switched on and on the same network."),
        _ if e.to_string().contains("lookup") || e.to_string().contains("resolve") || e.to_string().contains("nodename") => {
            format!("Could not find {label} on the network. Check the address (for example raspberrypi.local or an IP address).")
        }
        _ => format!("Could not connect to {label}: {e}."),
    };
    SessionError::new(DisconnectKind::Network, message)
}

fn initial_quality(q: Quality) -> Quality {
    if q == Quality::Automatic { Quality::Balanced } else { q }
}

async fn writer_loop(mut wr: WriteHalf<BoxedStream>, mut rx: UnboundedReceiver<Vec<u8>>) {
    while let Some(first) = rx.recv().await {
        let mut batch = first;
        while let Ok(more) = rx.try_recv() {
            batch.extend_from_slice(&more);
            if batch.len() > 256 * 1024 {
                break;
            }
        }
        if wr.write_all(&batch).await.is_err() || wr.flush().await.is_err() {
            return;
        }
    }
    let _ = wr.shutdown().await;
}

struct Control {
    out: UnboundedSender<Vec<u8>>,
    events: EventSink,
    options: SessionOptions,
    info: ServerInfo,
    width: u16,
    height: u16,
    screens: Vec<Screen>,
    extended_mouse: bool,
    use_physical_keys: bool,
    fence_supported: bool,
    fence_sent: Option<Instant>,
    latency_ms: Option<f32>,
    ext_clipboard: bool,
    local_clipboard: Option<String>,
    last_remote_clipboard: Option<String>,
    remote_clipboard_at: Option<Instant>,
    effective: Quality,
    /// Smoothed link speed measured on large updates.
    link_kbps: Option<f32>,
    /// Consecutive automatic-quality votes for a change (positive = better, negative = faster).
    auto_votes: i32,
    connected: bool,
    start: Instant,
}

impl Control {
    fn send(&self, m: Vec<u8>) {
        let _ = self.out.send(m);
    }

    fn encodings(&self) -> Vec<i32> {
        let mut v = Vec::new();
        if self.options.video {
            v.push(enc::OPEN_H264);
        }
        match self.effective {
            Quality::Best => v.extend([enc::ZRLE, enc::TIGHT]),
            _ => v.extend([enc::TIGHT, enc::ZRLE]),
        }
        v.extend([enc::COPY_RECT, enc::HEXTILE, enc::RRE, enc::RAW]);
        match self.effective {
            Quality::Best => v.push(enc::COMPRESS_LEVEL_0 + 1),
            Quality::Balanced | Quality::Automatic => {
                v.push(enc::JPEG_QUALITY_0 + 8);
                v.push(enc::COMPRESS_LEVEL_0 + 2);
            }
            Quality::Fast => {
                v.push(enc::JPEG_QUALITY_0 + 4);
                v.push(enc::COMPRESS_LEVEL_0 + 6);
            }
        }
        v.extend([
            enc::CURSOR,
            enc::DESKTOP_SIZE,
            enc::EXTENDED_DESKTOP_SIZE,
            enc::LAST_RECT,
            enc::QEMU_EXTENDED_KEY_EVENT,
            enc::DESKTOP_NAME,
            enc::FENCE,
            enc::CONTINUOUS_UPDATES,
            enc::EXTENDED_MOUSE_BUTTONS,
            enc::EXTENDED_CLIPBOARD,
        ]);
        v
    }

    fn resend_encodings(&self) {
        self.send(msg::set_encodings(&self.encodings()));
    }

    fn command(&mut self, cmd: Command) {
        match cmd {
            Command::Pointer { x, y, buttons } => {
                let x = x.min(self.width.saturating_sub(1));
                let y = y.min(self.height.saturating_sub(1));
                self.send(msg::pointer_event(x, y, buttons, self.extended_mouse));
            }
            Command::Key { down, keysym, scancode } => {
                if self.use_physical_keys && self.info.physical_keys && scancode != 0 {
                    self.send(msg::qemu_key_event(down, keysym, scancode));
                } else if keysym != 0 {
                    self.send(msg::key_event(down, keysym));
                }
            }
            Command::UsePhysicalKeys(on) => self.use_physical_keys = on,
            Command::Clipboard(text) => {
                if self.last_remote_clipboard.as_deref() == Some(text.as_str())
                    || self.local_clipboard.as_deref() == Some(text.as_str())
                {
                    return;
                }
                if self.ext_clipboard {
                    self.local_clipboard = Some(text);
                    self.send(msg::clipboard_notify_text());
                } else {
                    self.send(msg::client_cut_text_latin1(&text));
                    self.local_clipboard = Some(text);
                }
            }
            Command::SetDesktopSize { width, height } => {
                if !self.info.can_resize {
                    self.events.send(Event::ResizeRefused("This computer cannot change its screen size.".into()));
                    return;
                }
                let (width, height) = (width.max(320) & !1, height.max(240) & !1);
                if (width, height) == (self.width, self.height) {
                    return;
                }
                let screens = match self.screens.first() {
                    Some(s) => vec![Screen { x: 0, y: 0, w: width, h: height, ..*s }],
                    None => vec![Screen { id: 0, x: 0, y: 0, w: width, h: height, flags: 0 }],
                };
                self.send(msg::set_desktop_size(width, height, &screens));
            }
            Command::SetQuality(q) => {
                self.options.quality = q;
                self.effective = initial_quality(q);
                self.auto_votes = 0;
                self.resend_encodings();
            }
            Command::SetVideo(v) => {
                self.options.video = v;
                self.resend_encodings();
                self.send(msg::update_request(false, 0, 0, self.width, self.height));
            }
            Command::Refresh => self.send(msg::update_request(false, 0, 0, self.width, self.height)),
            Command::Credentials { .. } | Command::TrustIdentity(_) | Command::Disconnect => {}
        }
    }

    fn internal(&mut self, m: Internal) {
        match m {
            Internal::Size(w, h) => {
                self.width = w;
                self.height = h;
                self.info.width = w;
                self.info.height = h;
            }
            Internal::Screens(s) => self.screens = s,
            Internal::CanResize => self.info.can_resize = true,
            Internal::PhysicalKeys => self.info.physical_keys = true,
            Internal::ExtendedMouse => self.extended_mouse = true,
            Internal::FenceSupported => self.fence_supported = true,
            Internal::FenceResponse(payload) => {
                if let (Some(sent), Ok(b)) = (self.fence_sent, <[u8; 8]>::try_from(payload.as_slice()))
                    && u64::from_be_bytes(b) == sent.duration_since(self.start).as_micros() as u64
                {
                    let ms = sent.elapsed().as_secs_f32() * 1000.0;
                    self.latency_ms = Some(match self.latency_ms {
                        Some(old) => old * 0.7 + ms * 0.3,
                        None => ms,
                    });
                    self.fence_sent = None;
                }
            }
            Internal::ClipboardCaps => {
                self.ext_clipboard = true;
                self.send(msg::clipboard_caps());
            }
            Internal::ClipboardNotify => self.send(msg::clipboard_request_text()),
            Internal::ClipboardRequest => {
                if let Some(t) = &self.local_clipboard {
                    self.send(msg::clipboard_provide_text(t));
                }
            }
            Internal::ClipboardPeek => {
                if self.local_clipboard.is_some() {
                    self.send(msg::clipboard_notify_text());
                }
            }
            Internal::RemoteClipboard(text) => {
                // Some servers announce one copy twice in a row.
                if self.last_remote_clipboard.as_deref() == Some(text.as_str())
                    && self.remote_clipboard_at.is_some_and(|t| t.elapsed() < Duration::from_secs(1))
                {
                    return;
                }
                self.remote_clipboard_at = Some(Instant::now());
                self.last_remote_clipboard = Some(text.clone());
                self.local_clipboard = None;
                self.events.send(Event::Clipboard(text));
            }
            Internal::UpdateTiming { bytes, elapsed } => {
                // Only big updates say something about the link speed.
                if bytes > 200_000 && elapsed > Duration::from_millis(2) {
                    let kbps = bytes as f32 * 8.0 / 1000.0 / elapsed.as_secs_f32();
                    self.link_kbps = Some(match self.link_kbps {
                        Some(old) => old * 0.7 + kbps * 0.3,
                        None => kbps,
                    });
                }
            }
            Internal::FirstUpdate => {
                self.connected = true;
                self.events.send(Event::Phase(Phase::Connected));
                self.events.send(Event::Connected(self.info.clone()));
            }
            Internal::Ended(_) => {}
        }
    }

    /// Once a second: measure latency and adjust automatic quality.
    fn tick(&mut self) {
        if self.fence_supported && self.fence_sent.is_none_or(|t| t.elapsed() > Duration::from_secs(5)) {
            let now = Instant::now();
            let stamp = now.duration_since(self.start).as_micros() as u64;
            self.fence_sent = Some(now);
            self.send(msg::client_fence(fence::REQUEST, &stamp.to_be_bytes()));
        }
        if self.options.quality != Quality::Automatic {
            return;
        }
        let rtt = self.latency_ms.unwrap_or(10.0);
        let Some(kbps) = self.link_kbps else { return };
        let wanted = if kbps > 150_000.0 && rtt < 8.0 {
            Quality::Best
        } else if kbps < 6_000.0 || rtt > 120.0 {
            Quality::Fast
        } else {
            Quality::Balanced
        };
        if wanted == self.effective {
            self.auto_votes = 0;
            return;
        }
        self.auto_votes += 1;
        if self.auto_votes >= 3 {
            log::info!("automatic quality: {:?} -> {:?} ({kbps:.0} kbit/s, {rtt:.1} ms)", self.effective, wanted);
            self.effective = wanted;
            self.auto_votes = 0;
            self.resend_encodings();
        }
    }
}

/// Copy the changed parts of the local framebuffer into the shared frame.
fn publish(fb: &mut Framebuffer, shared: &Mutex<SharedFrame>) -> bool {
    if !fb.has_dirty() {
        return false;
    }
    let dirty = fb.take_dirty();
    let mut f = shared.lock();
    if f.width != fb.width() || f.height != fb.height() {
        f.width = fb.width();
        f.height = fb.height();
        f.pixels = fb.data().to_vec();
        f.dirty = vec![fb.bounds()];
        f.generation += 1;
        return true;
    }
    let stride = fb.stride();
    for r in &dirty {
        let row_len = r.w as usize * 4;
        for y in r.y as usize..r.bottom() as usize {
            let start = y * stride + r.x as usize * 4;
            f.pixels[start..start + row_len].copy_from_slice(&fb.data()[start..start + row_len]);
        }
    }
    if f.dirty.len() + dirty.len() > 64 {
        let merged = f.dirty.iter().chain(dirty.iter()).fold(Rect::default(), |acc, r| acc.union(r));
        f.dirty = vec![merged];
    } else {
        f.dirty.extend(dirty);
    }
    true
}

async fn reader_loop(
    mut r: RfbReader,
    mut fb: Framebuffer,
    shared: Arc<Mutex<SharedFrame>>,
    out: UnboundedSender<Vec<u8>>,
    internal: UnboundedSender<Internal>,
    events: EventSink,
    counters: Arc<Counters>,
) {
    let result = read_messages(&mut r, &mut fb, &shared, &out, &internal, &events, &counters).await;
    let _ = internal.send(Internal::Ended(result));
}

async fn read_messages(
    r: &mut RfbReader,
    fb: &mut Framebuffer,
    shared: &Mutex<SharedFrame>,
    out: &UnboundedSender<Vec<u8>>,
    internal: &UnboundedSender<Internal>,
    events: &EventSink,
    counters: &Counters,
) -> Result<(), SessionError> {
    let mut decoders = Decoders::new();
    let pf = PixelFormat::RGBX;
    // Report "connected" once real picture data arrived (or after a few
    // control-only updates), so the app never shows an empty screen.
    let mut first = true;
    let mut updates_seen = 0;
    // With continuous updates the server pushes changes without being asked.
    let mut continuous = false;
    loop {
        let kind = r.read_u8().await?;
        match kind {
            0 => {
                let started = Instant::now();
                let start_bytes = r.bytes_read();
                r.read_u8().await?;
                let count = r.read_u16().await?;
                let mut had_picture = false;
                let mut i = 0u32;
                while count == 0xffff || i < count as u32 {
                    i += 1;
                    let x = r.read_u16().await?;
                    let y = r.read_u16().await?;
                    let w = r.read_u16().await?;
                    let h = r.read_u16().await?;
                    let e = r.read_i32().await?;
                    let rect = Rect::new(x, y, w, h);
                    match e {
                        enc::LAST_RECT => break,
                        enc::DESKTOP_SIZE => {
                            resize(fb, shared, internal, events, w, h, continuous.then_some(out));
                        }
                        enc::EXTENDED_DESKTOP_SIZE => {
                            let n = r.read_u8().await? as usize;
                            r.skip(3).await?;
                            let mut screens = Vec::with_capacity(n);
                            for _ in 0..n {
                                let id = r.read_u32().await?;
                                let sx = r.read_u16().await?;
                                let sy = r.read_u16().await?;
                                let sw = r.read_u16().await?;
                                let sh = r.read_u16().await?;
                                let flags = r.read_u32().await?;
                                screens.push(Screen { id, x: sx, y: sy, w: sw, h: sh, flags });
                            }
                            log::debug!("extended desktop size: reason {x}, status {y}, {w}x{h}, screens {screens:?}");
                            let _ = internal.send(Internal::CanResize);
                            let _ = internal.send(Internal::Screens(screens));
                            // x is the reason, y the status (for our own requests).
                            // Status 4 means "forwarded, will finish shortly", not an error.
                            if x == 1 && matches!(y, 1..=3) {
                                let text = match y {
                                    1 => "The computer does not allow changing its screen size. This usually means a monitor is plugged in.",
                                    2 => "The computer could not change its screen size (not enough resources).",
                                    _ => "The computer could not use that screen size.",
                                };
                                events.send(Event::ResizeRefused(text.into()));
                            } else if x == 1 {
                                // Our own request: the real new size follows in a later update.
                            } else if w > 0 && h > 0 {
                                resize(fb, shared, internal, events, w, h, continuous.then_some(out));
                            }
                        }
                        enc::CURSOR => {
                            let cursor = decoders.decode_cursor(rect, &pf, r).await?;
                            let cursor = (w > 0 && h > 0).then_some(cursor);
                            events.send(Event::Cursor(cursor));
                        }
                        enc::DESKTOP_NAME => {
                            let len = r.read_u32().await? as usize;
                            if len > 4096 {
                                r.skip(len).await?;
                            } else {
                                let name = decode_text(&r.read_vec(len).await?);
                                events.send(Event::DesktopName(name));
                            }
                        }
                        enc::QEMU_EXTENDED_KEY_EVENT => {
                            let _ = internal.send(Internal::PhysicalKeys);
                        }
                        enc::EXTENDED_MOUSE_BUTTONS => {
                            let _ = internal.send(Internal::ExtendedMouse);
                        }
                        e if Decoders::supports(e) => {
                            decoders.decode(e, rect, &pf, r, fb).await?;
                            had_picture = true;
                            counters.last_encoding.store(e, Ordering::Relaxed);
                        }
                        e => {
                            return Err(SessionError::protocol(format!("the server used an unknown picture encoding ({e})")));
                        }
                    }
                }
                if publish(fb, shared) {
                    events.wake();
                }
                counters.updates.fetch_add(1, Ordering::Relaxed);
                counters.bytes.store(r.bytes_read(), Ordering::Relaxed);
                let _ = internal.send(Internal::UpdateTiming {
                    bytes: r.bytes_read() - start_bytes,
                    elapsed: started.elapsed(),
                });
                updates_seen += 1;
                if first && (had_picture || updates_seen >= 3) {
                    first = false;
                    let _ = internal.send(Internal::FirstUpdate);
                }
                if !continuous {
                    let _ = out.send(msg::update_request(true, 0, 0, fb.width(), fb.height()));
                }
            }
            1 => {
                // Colour map entries: we always ask for true colour, so skip.
                r.read_u8().await?;
                r.read_u16().await?;
                let n = r.read_u16().await? as usize;
                r.skip(n * 6).await?;
            }
            2 => events.send(Event::Bell),
            3 => {
                r.skip(3).await?;
                let len = r.read_i32().await?;
                if len >= 0 {
                    let len = len as usize;
                    let keep = len.min(MAX_CLIPBOARD);
                    let data = r.read_vec(keep).await?;
                    r.skip(len - keep).await?;
                    let text: String = data.iter().map(|&c| c as char).collect();
                    let _ = internal.send(Internal::RemoteClipboard(text.replace("\r\n", "\n")));
                } else {
                    let len = len.unsigned_abs() as usize;
                    if len < 4 {
                        return Err(SessionError::protocol("a clipboard message was too short"));
                    }
                    let flags = r.read_u32().await?;
                    let keep = (len - 4).min(MAX_CLIPBOARD);
                    let payload = r.read_vec(keep).await?;
                    r.skip(len - 4 - keep).await?;
                    extended_clipboard(flags, &payload, internal);
                }
            }
            150 => {
                // The server supports continuous updates: switch them on once.
                if !continuous {
                    continuous = true;
                    log::info!("using continuous updates");
                    let _ = out.send(msg::enable_continuous_updates(true, 0, 0, fb.width(), fb.height()));
                }
            }
            248 => {
                r.skip(3).await?;
                let flags = r.read_u32().await?;
                let len = r.read_u8().await? as usize;
                let payload = r.read_vec(len.min(64)).await?;
                let _ = internal.send(Internal::FenceSupported);
                if flags & fence::REQUEST != 0 {
                    let _ = out.send(msg::client_fence(flags & fence::SUPPORTED & !fence::REQUEST, &payload));
                } else {
                    let _ = internal.send(Internal::FenceResponse(payload));
                }
            }
            other => {
                return Err(SessionError::protocol(format!("the server sent an unknown message ({other})")));
            }
        }
        counters.bytes.store(r.bytes_read(), Ordering::Relaxed);
    }
}

fn resize(
    fb: &mut Framebuffer,
    shared: &Mutex<SharedFrame>,
    internal: &UnboundedSender<Internal>,
    events: &EventSink,
    w: u16,
    h: u16,
    continuous: Option<&UnboundedSender<Vec<u8>>>,
) {
    if (w, h) == (fb.width(), fb.height()) || w == 0 || h == 0 {
        return;
    }
    fb.resize(w, h);
    publish(fb, shared);
    if let Some(out) = continuous {
        let _ = out.send(msg::enable_continuous_updates(true, 0, 0, w, h));
    }
    let _ = internal.send(Internal::Size(w, h));
    events.send(Event::Resized { width: w, height: h });
}

fn extended_clipboard(flags: u32, payload: &[u8], internal: &UnboundedSender<Internal>) {
    if flags & clip::CAPS != 0 {
        let _ = internal.send(Internal::ClipboardCaps);
    } else if flags & clip::REQUEST != 0 {
        if flags & clip::TEXT != 0 {
            let _ = internal.send(Internal::ClipboardRequest);
        }
    } else if flags & clip::PEEK != 0 {
        let _ = internal.send(Internal::ClipboardPeek);
    } else if flags & clip::NOTIFY != 0 {
        if flags & clip::TEXT != 0 {
            let _ = internal.send(Internal::ClipboardNotify);
        }
    } else if flags & clip::PROVIDE != 0
        && let Some(text) = provided_text(flags, payload)
    {
        let _ = internal.send(Internal::RemoteClipboard(text));
    }
}

/// Unpack the text part of a "provide" message.
fn provided_text(flags: u32, payload: &[u8]) -> Option<String> {
    use std::io::Read;
    if flags & clip::TEXT == 0 {
        return None;
    }
    let mut data = Vec::new();
    flate2::read::ZlibDecoder::new(payload).take(MAX_CLIPBOARD as u64 + 16).read_to_end(&mut data).ok()?;
    // Text is the first format (bit 0), so its size comes first.
    let size = u32::from_be_bytes(data.get(..4)?.try_into().ok()?) as usize;
    let text = data.get(4..4 + size.min(data.len() - 4))?;
    let text = text.strip_suffix(&[0]).unwrap_or(text);
    Some(String::from_utf8_lossy(text).replace("\r\n", "\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provided_text_parses_our_own_message() {
        let m = msg::clipboard_provide_text("hej\nvärlden");
        let flags = u32::from_be_bytes(m[8..12].try_into().unwrap());
        assert_eq!(provided_text(flags, &m[12..]).as_deref(), Some("hej\nvärlden"));
    }

    #[test]
    fn host_only_strips_ports() {
        assert_eq!(host_only("raspberrypi.local:5900"), "raspberrypi.local");
        assert_eq!(host_only("[fe80::1]:5900"), "fe80::1");
        assert_eq!(host_only("fe80::1"), "fe80::1");
        assert_eq!(host_only("10.0.0.2"), "10.0.0.2");
    }
}
