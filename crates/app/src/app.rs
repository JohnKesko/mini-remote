//! The app's state: saved connections, settings, open tabs, dialogs, and the
//! glue between the window, the sessions and the stored files.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use egui_winit::winit::event::KeyEvent;
use egui_winit::winit::keyboard::{Key, ModifiersState};

use crate::clipboard::{ClipboardWatcher, POLL_INTERVAL};
use crate::input::keyboard::KeyInput;
use crate::model::{Connection, Settings, now_secs, parse_address};
use crate::platform::{self, SecretStore};
use crate::session::{Notice, SessionEnv, SessionTab, Status, Wake};
use crate::storage::{self, Storage};
use crate::texture::RemoteTexture;

/// Requests for the window, carried out by the runner after each frame.
pub enum WindowRequest {
    Title(String),
    Fullscreen(bool),
    /// Light or dark window frame; `None` follows the system.
    Theme(Option<crate::model::Appearance>),
    /// Move the window with the mouse (dragging the header).
    Drag,
    /// Zoom or unzoom (double-click on the header).
    ToggleZoom,
    /// Debug builds: resize the window (for checking layouts).
    #[allow(dead_code)]
    Size(f32, f32),
}

/// Results of work done on background threads.
pub enum BgResult {
    ScreenshotSaved(PathBuf),
    ScreenshotFailed(String),
    ThumbnailSaved(String),
    DownloadFolder(PathBuf),
}

pub enum ToastAction {
    Reveal(PathBuf),
    /// Save the connection of this tab.
    SaveConnection(u64),
}

pub struct Toast {
    pub id: u64,
    pub text: String,
    pub action: Option<ToastAction>,
    pub created: Instant,
}

/// The connection editor dialog.
pub struct EditorState {
    pub conn: Connection,
    pub is_new: bool,
    pub port_text: String,
    pub password: String,
    pub password_changed: bool,
    /// A password for this connection is already in the password store.
    pub has_saved_password: bool,
    pub show_password: bool,
    pub ssh_port_text: String,
    /// Separate SSH password or key passphrase.
    pub ssh_password: String,
    pub ssh_password_changed: bool,
    pub has_saved_ssh_password: bool,
    pub show_ssh_password: bool,
    pub error: Option<String>,
    /// Tab to reconnect after saving (when opened from an error message).
    pub for_tab: Option<u64>,
}

/// Computers found on the local network.
#[derive(Default)]
pub struct DiscoveryState {
    pub running: bool,
    pub hosts: Vec<discovery::DiscoveredHost>,
    started: Option<Instant>,
    rx: Option<mpsc::Receiver<discovery::DiscoveryEvent>>,
}

#[derive(Default)]
pub struct HomeState {
    /// Thumbnails by connection id (`None`: no picture saved).
    pub thumbnails: HashMap<String, Option<egui::TextureHandle>>,
    /// Connection waiting for "really delete?".
    pub confirm_delete: Option<String>,
}

/// What the content area shows (chosen in the side menu).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Saved,
    /// The connection editor (new or edit).
    Editor,
    Find,
    Settings,
    /// The remote screen of an open connection.
    Session(u64),
}

/// App shortcuts that work even while the remote screen has the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shortcut {
    FullScreen,
    CloseTab,
    NextTab,
    PreviousTab,
    ToggleSidebar,
}

pub struct App {
    pub(crate) ctx: egui::Context,
    pub(crate) gpu: egui_wgpu::RenderState,
    runtime: tokio::runtime::Runtime,
    wake: Wake,
    pub(crate) storage: Storage,
    pub(crate) secrets: Box<dyn SecretStore>,
    pub(crate) settings: Settings,
    pub(crate) connections: Vec<Connection>,
    trusted: BTreeMap<String, String>,
    pub(crate) tabs: Vec<SessionTab>,
    /// What the content area shows.
    pub(crate) page: Page,
    /// Where "Cancel" in the editor goes back to.
    previous_page: Page,
    next_tab_id: u64,
    pub(crate) home: HomeState,
    pub(crate) editor: Option<EditorState>,
    pub(crate) toasts: Vec<Toast>,
    next_toast_id: u64,
    clipboard: ClipboardWatcher,
    focused: bool,
    pub(crate) fullscreen: bool,
    /// Esc left full screen; keep its release from the remote computer too.
    swallow_escape_release: bool,
    requests: Vec<WindowRequest>,
    bg_tx: mpsc::Sender<BgResult>,
    bg_rx: mpsc::Receiver<BgResult>,
    /// Keys go to the remote computer instead of the app's own controls.
    remote_keyboard: bool,
    modifiers: ModifiersState,
    pub(crate) discovery: DiscoveryState,
    title: String,
    /// Height of the transparent system title bar over the app (0 if none).
    pub(crate) title_bar: f32,
    /// Debug builds only: things to open automatically, for checking screens.
    debug_open: Vec<String>,
    /// Debug builds only: press the toolbar's Disconnect button at this time.
    pub(crate) debug_disconnect_at: Option<Instant>,
}

/// Build the environment a session needs from the app's fields.
macro_rules! session_env {
    ($app:expr) => {
        SessionEnv {
            runtime: $app.runtime.handle(),
            wake: &$app.wake,
            settings: &$app.settings,
            trusted: &$app.trusted,
        }
    };
}

impl App {
    pub fn new(ctx: egui::Context, gpu: egui_wgpu::RenderState, wake: Wake, connect_to: Option<String>) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(3)
            .thread_name("mini-remote-net")
            .enable_all()
            .build()
            .expect("could not start the network threads");
        let storage = Storage::open();
        let settings = storage.load_settings();
        let connections = storage.load_connections();
        let trusted = storage.load_trusted();
        let (bg_tx, bg_rx) = mpsc::channel();

        crate::ui::theme::install(&ctx);
        apply_appearance(&ctx, &settings);
        let appearance = settings.appearance;

        let mut app = App {
            ctx,
            gpu,
            runtime,
            wake,
            storage,
            // With a separate config folder, never touch the real Keychain.
            secrets: if storage::custom_dir().is_some() { platform::memory_store() } else { platform::secret_store() },
            settings,
            connections,
            trusted,
            tabs: Vec::new(),
            page: Page::Saved,
            previous_page: Page::Saved,
            next_tab_id: 1,
            home: HomeState::default(),
            editor: None,
            toasts: Vec::new(),
            next_toast_id: 1,
            clipboard: ClipboardWatcher::new(),
            focused: true,
            fullscreen: false,
            swallow_escape_release: false,
            requests: Vec::new(),
            bg_tx,
            bg_rx,
            remote_keyboard: false,
            modifiers: ModifiersState::empty(),
            discovery: DiscoveryState::default(),
            title: String::new(),
            title_bar: 0.0,
            debug_open: Vec::new(),
            debug_disconnect_at: None,
        };
        app.requests.push(WindowRequest::Theme(Some(appearance)));
        app.debug_startup();
        match connect_to {
            Some(address) => app.quick_connect(&address),
            None => app.start_discovery(false),
        }
        app
    }

    // ----- Connections and tabs -------------------------------------------------

    /// Connect to an address given on the command line (`--connect`).
    pub fn quick_connect(&mut self, text: &str) {
        let Some((host, port)) = parse_address(text) else {
            self.toast(format!("“{text}” is not an address Mini Remote understands, for example raspberrypi.local."));
            return;
        };
        let existing = self
            .connections
            .iter()
            .find(|c| c.host.eq_ignore_ascii_case(&host) && c.port == port)
            .cloned();
        match existing {
            Some(conn) => self.open_connection(conn, None),
            None => {
                let conn = Connection { host, port, ..Connection::new_with_defaults(&self.settings) };
                self.open_tab(conn, false, None);
            }
        }
    }

    /// Connect to a saved connection, or show its tab if it is already open.
    pub fn open_connection(&mut self, conn: Connection, password: Option<String>) {
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.conn.id == conn.id) {
            let id = tab.id;
            if matches!(tab.status, Status::Failed(_) | Status::Reconnecting { .. }) {
                tab.update_connection(conn);
                tab.saved = true;
                if let Some(pw) = password {
                    tab.set_password(pw);
                }
                let env = session_env!(self);
                tab.retry(&env);
            }
            self.activate(Some(id));
            return;
        }
        self.open_tab(conn, true, password);
    }

    fn open_tab(&mut self, conn: Connection, saved: bool, password: Option<String>) {
        let password = password.or_else(|| {
            (saved && conn.remember_password && self.secrets.available())
                .then(|| self.secrets.get(&conn.secret_key()))
                .flatten()
        });
        let ssh_password = (saved && self.secrets.available()).then(|| self.secrets.get(&conn.ssh_secret_key())).flatten();
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let mut tab = SessionTab::new(id, conn, saved, password, ssh_password);
        let env = session_env!(self);
        tab.connect(&env);
        self.tabs.push(tab);
        self.activate(Some(id));
    }

    /// The open connection whose screen is shown, if any.
    pub fn active(&self) -> Option<u64> {
        match self.page {
            Page::Session(id) => Some(id),
            _ => None,
        }
    }

    /// Show a connection's screen (or the saved connections with `None`).
    pub fn activate(&mut self, id: Option<u64>) {
        self.show_page(id.map_or(Page::Saved, Page::Session));
    }

    /// Switch what the content area shows.
    pub fn show_page(&mut self, page: Page) {
        if self.page == page {
            return;
        }
        if let Some(tab) = self.active_tab_mut() {
            tab.release_input();
        }
        if self.page == Page::Editor {
            self.editor = None;
        } else {
            self.previous_page = self.page;
        }
        self.page = page;
        if page == Page::Find {
            self.start_discovery(false);
        }
        if let Page::Session(_) = page {
            // Give the keyboard to the remote screen.
            self.ctx.memory_mut(|m| {
                if let Some(focused) = m.focused() {
                    m.surrender_focus(focused);
                }
            });
        }
    }

    /// Disconnect a session. The tab itself is removed at the start of the
    /// next frame, because the code drawing it may still be running (removing
    /// it right away made the next drawing step use a tab that was gone).
    pub fn close_tab(&mut self, id: u64) {
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
            tab.close();
            self.ctx.request_repaint();
        }
    }

    fn remove_tab(&mut self, id: u64) {
        let Some(index) = self.tabs.iter().position(|t| t.id == id) else { return };
        self.save_thumbnail(index, false);
        let mut tab = self.tabs.remove(index);
        tab.close();
        if let Some(tex) = tab.texture.take() {
            tex.free(&self.gpu);
        }
        if self.page == Page::Session(id) {
            let next = self.tabs.get(index.min(self.tabs.len().saturating_sub(1))).map(|t| t.id);
            self.page = Page::Saved;
            self.activate(next);
            if next.is_none() && self.fullscreen {
                self.set_fullscreen(false);
            }
        }
    }

    pub fn active_index(&self) -> Option<usize> {
        let id = self.active()?;
        self.tabs.iter().position(|t| t.id == id)
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut SessionTab> {
        let id = self.active()?;
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    /// Status of the open tab for a saved connection, if any.
    pub fn connection_status(&self, conn_id: &str) -> Option<&Status> {
        self.tabs.iter().find(|t| t.conn.id == conn_id).map(|t| &t.status)
    }

    pub fn delete_connection(&mut self, id: &str) {
        if let Some(pos) = self.connections.iter().position(|c| c.id == id) {
            let conn = self.connections.remove(pos);
            self.secrets.delete(&conn.secret_key());
            self.storage.delete_thumbnail(&conn.id);
            self.home.thumbnails.remove(&conn.id);
            self.storage.save_connections(&self.connections);
            for tab in self.tabs.iter_mut().filter(|t| t.conn.id == id) {
                tab.saved = false;
            }
        }
    }

    /// Add or replace a saved connection and update open tabs that use it.
    pub fn store_connection(&mut self, conn: Connection) {
        match self.connections.iter_mut().find(|c| c.id == conn.id) {
            Some(existing) => *existing = conn.clone(),
            None => self.connections.push(conn.clone()),
        }
        self.storage.save_connections(&self.connections);
        for tab in self.tabs.iter_mut().filter(|t| t.conn.id == conn.id) {
            tab.saved = true;
            let mut updated = conn.clone();
            updated.last_used = tab.conn.last_used.or(conn.last_used);
            tab.update_connection(updated);
        }
    }

    /// Settings of a tab changed (quality, scaling, ...); save them.
    pub fn tab_connection_changed(&mut self, index: usize) {
        let Some(tab) = self.tabs.get(index) else { return };
        if !tab.saved {
            return;
        }
        let conn = tab.conn.clone();
        if let Some(existing) = self.connections.iter_mut().find(|c| c.id == conn.id) {
            *existing = conn;
            self.storage.save_connections(&self.connections);
        }
    }

    pub fn open_editor_new(&mut self) {
        let conn = Connection::new_with_defaults(&self.settings);
        self.open_editor(conn, true, None);
    }

    pub fn open_editor(&mut self, conn: Connection, is_new: bool, for_tab: Option<u64>) {
        let has_saved_password = !is_new && conn.remember_password && self.secrets.get(&conn.secret_key()).is_some();
        let has_saved_ssh_password = !is_new && self.secrets.get(&conn.ssh_secret_key()).is_some();
        self.editor = Some(EditorState {
            port_text: conn.port.to_string(),
            ssh_port_text: conn.ssh.port.to_string(),
            conn,
            is_new,
            password: String::new(),
            password_changed: false,
            has_saved_password,
            show_password: false,
            ssh_password: String::new(),
            ssh_password_changed: false,
            has_saved_ssh_password,
            show_ssh_password: false,
            error: None,
            for_tab,
        });
        self.show_page(Page::Editor);
    }

    /// Leave the editor without saving.
    pub fn cancel_editor(&mut self) {
        let back = match self.previous_page {
            Page::Session(id) if !self.tabs.iter().any(|t| t.id == id) => Page::Saved,
            Page::Editor => Page::Saved,
            other => other,
        };
        self.editor = None;
        self.show_page(back);
    }

    /// Side menu while a remote screen is shown: full, icons only, hidden.
    pub fn cycle_sidebar(&mut self) {
        use crate::model::SidebarMode;
        self.settings.sidebar = match self.settings.sidebar {
            SidebarMode::Full => SidebarMode::Rail,
            SidebarMode::Rail => SidebarMode::Hidden,
            SidebarMode::Hidden => SidebarMode::Full,
        };
        self.storage.save_settings(&self.settings);
    }

    /// Save the editor's connection; optionally connect right away.
    pub fn save_editor(&mut self, connect: bool) {
        let Some(editor) = &mut self.editor else { return };
        let mut conn = editor.conn.clone();
        conn.host = conn.host.trim().to_string();
        conn.name = conn.name.trim().to_string();
        if conn.host.is_empty() {
            editor.error = Some("Please type the Pi's address, for example raspberrypi.local".into());
            return;
        }
        let port: Option<u16> = editor.port_text.trim().parse().ok().filter(|p| *p > 0);
        let Some(mut port) = port else {
            editor.error = Some("The port must be a number, usually 5900.".into());
            return;
        };
        // Allow "pi.local:5901" in the address field.
        if (conn.host.contains(':') || conn.host.contains("://"))
            && let Some((host, parsed_port)) = parse_address(&conn.host)
        {
            conn.host = host;
            if port == crate::model::DEFAULT_VNC_PORT {
                port = parsed_port;
            }
        }
        conn.port = port;
        let Some(ssh_port) = editor.ssh_port_text.trim().parse::<u16>().ok().filter(|p| *p > 0) else {
            editor.error = Some("The SSH port must be a number, usually 22.".into());
            return;
        };
        conn.ssh.port = ssh_port;
        conn.ssh.host = conn.ssh.host.trim().to_string();
        conn.ssh.username = conn.ssh.username.trim().to_string();
        if let crate::model::SshLogin::KeyFile(path) = &conn.ssh.login
            && path.trim().is_empty()
        {
            editor.error = Some("Please choose the key file for secure remote login.".into());
            return;
        }

        // The separate SSH password (or key passphrase) lives in the password store too.
        let ssh_password = editor.ssh_password.clone();
        let ssh_changed = editor.ssh_password_changed;
        let ssh_key = conn.ssh_secret_key();
        let keeps_ssh_secret = matches!(conn.ssh.login, crate::model::SshLogin::Password | crate::model::SshLogin::KeyFile(_));
        if !keeps_ssh_secret || (ssh_changed && ssh_password.is_empty()) {
            self.secrets.delete(&ssh_key);
        } else if ssh_changed && let Err(err) = self.secrets.set(&ssh_key, &ssh_password) {
            log::warn!("could not save SSH password: {err}");
        }
        let typed_ssh = (ssh_changed && !ssh_password.is_empty()).then_some(ssh_password);

        let Some(editor) = &mut self.editor else { return };
        let password = editor.password.clone();
        let password_changed = editor.password_changed;
        let for_tab = editor.for_tab;
        let key = conn.secret_key();
        if conn.remember_password {
            if password_changed && !password.is_empty() {
                if let Err(err) = self.secrets.set(&key, &password) {
                    log::warn!("could not save password: {err}");
                    self.toast(format!("The password could not be saved in the {}.", platform::secret_store_name()));
                }
            } else if password_changed {
                self.secrets.delete(&key);
            }
        } else {
            self.secrets.delete(&key);
        }
        self.editor = None;
        self.store_connection(conn.clone());

        let typed = (password_changed && !password.is_empty()).then_some(password);
        if let Some(tab) = for_tab.and_then(|id| self.tabs.iter_mut().find(|t| t.id == id)) {
            tab.conn.id = conn.id.clone();
            tab.update_connection(conn.clone());
            tab.saved = true;
            if let Some(pw) = typed.clone() {
                tab.set_password(pw);
            }
            if let Some(pw) = typed_ssh.clone() {
                tab.set_ssh_password(pw);
            }
            if connect {
                let env = session_env!(self);
                tab.retry(&env);
            }
            let id = tab.id;
            self.activate(Some(id));
        } else if connect {
            self.open_connection(conn, typed);
        } else {
            self.show_page(Page::Saved);
        }
    }

    /// Ask where copied files should go (system folder picker).
    pub fn choose_download_folder(&mut self) {
        let tx = self.bg_tx.clone();
        let wake = self.wake.clone();
        self.runtime.spawn(async move {
            if let Some(dir) = platform::pick_folder("Where should copied files go?").await {
                let _ = tx.send(BgResult::DownloadFolder(dir));
                wake();
            }
        });
    }

    pub fn save_settings(&mut self) {
        self.storage.save_settings(&self.settings);
        apply_appearance(&self.ctx, &self.settings);
        self.requests.push(WindowRequest::Theme(Some(self.settings.appearance)));
    }

    // ----- Network discovery -----------------------------------------------------

    /// Look for computers on the local network. Without `force`, a scan that
    /// ran in the last few seconds is not repeated.
    pub fn start_discovery(&mut self, force: bool) {
        if self.discovery.running {
            return;
        }
        if !force && self.discovery.started.is_some_and(|t| t.elapsed() < Duration::from_secs(20)) {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let wake = self.wake.clone();
        if std::env::var_os("MINI_REMOTE_SAMPLE_DEVICES").is_some_and(|v| !v.is_empty() && v != "0") {
            self.runtime.spawn(sample_devices(tx, wake));
            self.discovery.running = true;
            self.discovery.started = Some(Instant::now());
            self.discovery.rx = Some(rx);
            return;
        }
        self.runtime.spawn(async move {
            let mut events = discovery::discover(discovery::DiscoveryOptions::default());
            while let Some(event) = events.recv().await {
                let finished = event == discovery::DiscoveryEvent::Finished;
                if tx.send(event).is_err() {
                    break;
                }
                wake();
                if finished {
                    break;
                }
            }
        });
        self.discovery.running = true;
        self.discovery.started = Some(Instant::now());
        self.discovery.rx = Some(rx);
    }

    fn poll_discovery(&mut self) {
        let Some(rx) = &self.discovery.rx else { return };
        loop {
            match rx.try_recv() {
                Ok(discovery::DiscoveryEvent::Found(host) | discovery::DiscoveryEvent::Updated(host)) => {
                    match self.discovery.hosts.iter_mut().find(|h| h.address == host.address) {
                        Some(existing) => *existing = host,
                        None => self.discovery.hosts.push(host),
                    }
                }
                Ok(discovery::DiscoveryEvent::Removed(address)) => {
                    self.discovery.hosts.retain(|h| h.address != address);
                }
                Ok(discovery::DiscoveryEvent::Finished) | Err(mpsc::TryRecvError::Disconnected) => {
                    self.discovery.running = false;
                    self.discovery.rx = None;
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
        // Raspberry Pis first, then by name.
        self.discovery.hosts.sort_by(|a, b| {
            b.is_raspberry_pi
                .cmp(&a.is_raspberry_pi)
                .then_with(|| (!b.vnc_ports.is_empty()).cmp(&!a.vnc_ports.is_empty()))
                .then_with(|| discovered_name(a).to_lowercase().cmp(&discovered_name(b).to_lowercase()))
        });
    }

    /// The saved connection for a found computer, if there is one.
    pub fn saved_for(&self, host: &discovery::DiscoveredHost) -> Option<&Connection> {
        let address = host.address.to_string();
        self.connections.iter().find(|c| {
            let h = c.host.trim().to_lowercase();
            (h == address || host.hostname.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(&h)))
                && (host.vnc_ports.is_empty() || host.vnc_ports.contains(&c.port))
        })
    }

    /// Connect to a computer found on the network; offer to save it once it works.
    pub fn connect_discovered(&mut self, host: &discovery::DiscoveredHost) {
        if let Some(conn) = self.saved_for(host).cloned() {
            self.open_connection(conn, None);
            return;
        }
        let Some(&port) = host.vnc_ports.first() else { return };
        let conn = Connection {
            name: discovered_name(host),
            host: host.hostname.clone().unwrap_or_else(|| host.address.to_string()),
            port,
            ..Connection::new_with_defaults(&self.settings)
        };
        self.open_tab(conn, false, None);
        if let Some(tab) = self.tabs.last_mut() {
            tab.offer_save = true;
        }
    }

    /// Open the editor pre-filled for a computer found on the network.
    pub fn save_discovered(&mut self, host: &discovery::DiscoveredHost) {
        let conn = Connection {
            name: discovered_name(host),
            host: host.hostname.clone().unwrap_or_else(|| host.address.to_string()),
            port: host.vnc_ports.first().copied().unwrap_or(crate::model::DEFAULT_VNC_PORT),
            ..Connection::new_with_defaults(&self.settings)
        };
        self.open_editor(conn, true, None);
    }

    /// Save the connection of a tab (from the "Save it?" message).
    pub fn save_tab_connection(&mut self, tab_id: u64) {
        let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) else { return };
        if tab.saved {
            return;
        }
        tab.saved = true;
        tab.offer_save = false;
        let conn = tab.conn.clone();
        let (password, _) = tab.passwords();
        let password = password.map(str::to_string);
        if conn.remember_password
            && let Some(pw) = password
        {
            let _ = self.secrets.set(&conn.secret_key(), &pw);
        }
        self.connections.push(conn);
        self.storage.save_connections(&self.connections);
        self.toast("Saved to your connections.");
    }

    // ----- File transfer ---------------------------------------------------------

    /// Show or hide the file panel of a tab, connecting it when shown.
    pub fn toggle_files(&mut self, index: usize) {
        let open = !self.tabs[index].files.open;
        self.tabs[index].files.open = open;
        if open {
            self.connect_files(index);
        }
    }

    pub fn connect_files(&mut self, index: usize) {
        let ctx = crate::files::FilesCtx { runtime: self.runtime.handle(), wake: &self.wake };
        let tab = &mut self.tabs[index];
        let trusted = self.trusted.get(&tab.conn.ssh_host_key()).cloned();
        let tunnel = tab.ssh_session().cloned();
        let (screen, ssh) = tab.passwords();
        let (screen, ssh) = (screen.map(str::to_string), ssh.map(str::to_string));
        let host_keys = tab.host_keys.clone();
        let conn = tab.conn.clone();
        tab.files.connect(&ctx, &conn, tunnel, screen.as_deref(), ssh.as_deref(), host_keys, trusted);
    }

    /// Run a file panel action that needs the network threads.
    pub fn with_files<R>(&mut self, index: usize, f: impl FnOnce(&mut crate::files::FilesPanel, &crate::files::FilesCtx) -> R) -> R {
        let ctx = crate::files::FilesCtx { runtime: self.runtime.handle(), wake: &self.wake };
        f(&mut self.tabs[index].files, &ctx)
    }

    /// Files were dropped onto the window: copy them to the remote computer.
    pub fn drop_files(&mut self, index: usize, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        if !self.tabs[index].files.open {
            self.tabs[index].files.open = true;
        }
        self.connect_files(index);
        self.with_files(index, |files, ctx| files.upload(ctx, paths));
    }

    // ----- Per-frame work ---------------------------------------------------------

    /// Everything that happens before the UI is drawn: session events, timers,
    /// picture uploads, clipboard, background results.
    pub(crate) fn update(&mut self) {
        while let Ok(result) = self.bg_rx.try_recv() {
            match result {
                BgResult::ScreenshotSaved(path) => {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    self.toast_with(format!("Screenshot saved: {name}"), Some(ToastAction::Reveal(path)));
                }
                BgResult::ScreenshotFailed(err) => self.toast(format!("The screenshot could not be saved: {err}")),
                BgResult::ThumbnailSaved(id) => {
                    self.home.thumbnails.remove(&id);
                }
                BgResult::DownloadFolder(dir) => {
                    self.settings.download_dir = Some(dir.display().to_string());
                    self.save_settings();
                }
            }
        }

        self.poll_discovery();

        let mut next_timer: Option<Instant> = None;
        let mut all_notices = Vec::new();
        {
            let env = session_env!(self);
            for (index, tab) in self.tabs.iter_mut().enumerate() {
                let mut notices = tab.poll(&env);
                if let Some(t) = tab.tick(&env, &mut notices) {
                    next_timer = Some(next_timer.map_or(t, |n| n.min(t)));
                }
                if !notices.is_empty() {
                    all_notices.push((index, notices));
                }
            }
        }
        for (index, notices) in all_notices {
            for notice in notices {
                self.handle_notice(index, notice);
            }
        }

        // Upload changed parts of each picture.
        for tab in &mut self.tabs {
            let frame = tab.current_frame();
            RemoteTexture::sync(&mut tab.texture, &self.gpu, &frame);
            if tab.cursor.changed {
                tab.cursor.changed = false;
                tab.cursor.texture = tab.cursor.image.as_ref().map(|c| {
                    let image =
                        egui::ColorImage::from_rgba_unmultiplied([usize::from(c.width), usize::from(c.height)], &c.rgba);
                    self.ctx.load_texture(format!("remote-cursor-{}", tab.id), image, egui::TextureOptions::LINEAR)
                });
            }
        }

        // Tabs whose session ended on purpose go away.
        let closed: Vec<u64> = self.tabs.iter().filter(|t| t.is_closed()).map(|t| t.id).collect();
        for id in closed {
            self.remove_tab(id);
        }

        // File panels.
        let mut file_notices = Vec::new();
        {
            let ctx = crate::files::FilesCtx { runtime: self.runtime.handle(), wake: &self.wake };
            for tab in &mut self.tabs {
                file_notices.extend(tab.files.poll(&ctx));
                if tab.files.busy() {
                    self.ctx.request_repaint_after(Duration::from_millis(200));
                }
            }
        }
        for notice in file_notices {
            match notice {
                crate::files::FilesNotice::Downloaded(path) => {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let folder = path.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    self.toast_with(format!("Copied “{name}” to {folder}"), Some(ToastAction::Reveal(path)));
                }
                crate::files::FilesNotice::Toast(text) => self.toast(text),
            }
        }

        self.poll_clipboard(false);
        if self.focused && self.sharing_clipboard() {
            self.ctx.request_repaint_after(POLL_INTERVAL);
        }
        if let Some(t) = next_timer {
            self.ctx.request_repaint_after(t.saturating_duration_since(Instant::now()));
        }
        // Toasts disappear by themselves.
        self.toasts.retain(|t| {
            let keep = if matches!(t.action, Some(ToastAction::SaveConnection(_))) { 15 } else { 6 };
            t.created.elapsed() < Duration::from_secs(keep)
        });
        if !self.toasts.is_empty() {
            self.ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    /// Called at the end of each frame, after the UI was drawn.
    pub(crate) fn after_ui(&mut self) {
        let remote = self.focused
            && self.home.confirm_delete.is_none()
            && !self.ctx.text_edit_focused()
            && !self.ctx.any_popup_open()
            && self.active_index().is_some_and(|i| self.tabs[i].accepts_input());
        if self.remote_keyboard && !remote
            && let Some(tab) = self.active_tab_mut()
        {
            tab.release_input();
        }
        self.remote_keyboard = remote;

        let title = match self.active_index() {
            Some(i) => format!("{} – Mini Remote", self.tabs[i].title()),
            None => "Mini Remote".to_string(),
        };
        if title != self.title {
            self.title = title.clone();
            self.requests.push(WindowRequest::Title(title));
        }
    }

    /// Debug builds: `MINI_REMOTE_DEBUG_OPEN=connect:<id>,files,info,editor:<id>,settings`
    /// opens screens without clicking, so they can be checked automatically.
    fn debug_startup(&mut self) {
        if !cfg!(debug_assertions) {
            return;
        }
        let Ok(list) = std::env::var("MINI_REMOTE_DEBUG_OPEN") else { return };
        for item in list.split(',').map(str::trim) {
            match item.split_once(':') {
                Some(("connect", id)) => {
                    if let Some(conn) = self.connections.iter().find(|c| c.id == id).cloned() {
                        self.open_connection(conn, None);
                    }
                }
                Some(("password", entry)) => {
                    // Test password for a saved connection, kept in memory only.
                    if storage::custom_dir().is_some()
                        && let Some((id, password)) = entry.split_once('=')
                        && let Some(conn) = self.connections.iter().find(|c| c.id == id)
                    {
                        let _ = self.secrets.set(&conn.secret_key(), password);
                    }
                }
                Some(("editor", id)) => {
                    if let Some(conn) = self.connections.iter().find(|c| c.id == id).cloned() {
                        self.open_editor(conn, false, None);
                    }
                }
                None if item == "settings" => self.show_page(Page::Settings),
                None if item == "find" => self.show_page(Page::Find),
                None if item == "new" => self.open_editor_new(),
                Some(("delete", id)) => self.home.confirm_delete = Some(id.to_string()),
                Some(("size", size)) => {
                    if let Some((w, h)) = size.split_once('x')
                        && let (Ok(w), Ok(h)) = (w.parse(), h.parse())
                    {
                        self.requests.push(WindowRequest::Size(w, h));
                    }
                }
                _ => self.debug_open.push(item.to_string()),
            }
        }
    }

    fn handle_notice(&mut self, index: usize, notice: Notice) {
        match notice {
            Notice::Connected => {
                for item in std::mem::take(&mut self.debug_open) {
                    match item.split_once(':') {
                        Some(("drop", path)) => self.drop_files(index, vec![PathBuf::from(path)]),
                        _ if item == "files" => self.toggle_files(index),
                        _ if item == "info" => self.tabs[index].info_open = true,
                        _ if item == "disconnect" => {
                            self.debug_disconnect_at = Some(Instant::now() + Duration::from_secs(2));
                        }
                        _ => {}
                    }
                }
                let tab = &mut self.tabs[index];
                tab.conn.last_used = Some(now_secs());
                if tab.offer_save && !tab.saved {
                    let (id, name) = (tab.id, tab.title().to_string());
                    tab.offer_save = false;
                    self.toast_with(
                        format!("Connected to {name}. Save it for next time?"),
                        Some(ToastAction::SaveConnection(id)),
                    );
                } else if !tab.saved {
                    // Remember addresses that worked, so they show up on the home screen.
                    tab.saved = true;
                    self.connections.push(tab.conn.clone());
                } else if let Some(saved) = self.connections.iter_mut().find(|c| c.id == tab.conn.id) {
                    saved.last_used = tab.conn.last_used;
                    saved.username = tab.conn.username.clone();
                    saved.remember_password = tab.conn.remember_password;
                }
                self.storage.save_connections(&self.connections);
                if self.sharing_clipboard_for(index)
                    && let Some(text) = self.clipboard.text()
                {
                    self.tabs[index].send_clipboard(&text);
                }
            }
            Notice::RemoteClipboard(text) => self.clipboard.set(&text),
            Notice::Trust { host_key, fingerprint } => {
                self.trusted.insert(host_key, fingerprint);
                self.storage.save_trusted(&self.trusted);
            }
            Notice::StorePassword { key, password } => {
                if self.secrets.available()
                    && let Err(err) = self.secrets.set(&key, &password)
                {
                    log::warn!("could not save password: {err}");
                    self.toast(format!("The password could not be saved in the {}.", platform::secret_store_name()));
                }
            }
            Notice::SaveThumbnail => self.save_thumbnail(index, true),
            Notice::Toast(text) => self.toast(text),
        }
    }

    fn sharing_clipboard(&self) -> bool {
        (0..self.tabs.len()).any(|i| self.sharing_clipboard_for(i))
    }

    fn sharing_clipboard_for(&self, index: usize) -> bool {
        let tab = &self.tabs[index];
        self.settings.share_clipboard && tab.conn.share_clipboard && tab.status.is_connected() && !tab.conn.view_only
    }

    /// Pass newly copied local text to the remote computers.
    fn poll_clipboard(&mut self, force: bool) {
        if !self.focused || !self.sharing_clipboard() {
            return;
        }
        if let Some(text) = self.clipboard.poll(force) {
            for i in 0..self.tabs.len() {
                if self.sharing_clipboard_for(i) {
                    self.tabs[i].send_clipboard(&text);
                }
            }
        }
    }

    /// Send the local clipboard to one tab now (toolbar menu).
    pub fn push_clipboard(&mut self, index: usize) {
        match self.clipboard.text() {
            Some(text) => {
                self.tabs[index].send_clipboard(&text);
                self.toast("Clipboard sent to the Pi.");
            }
            None => self.toast("There is no text on the clipboard."),
        }
    }

    /// Type the local clipboard text into the remote computer.
    pub fn type_clipboard(&mut self, index: usize) {
        match self.clipboard.text() {
            Some(text) => self.tabs[index].type_text(&text),
            None => self.toast("There is no text on the clipboard."),
        }
    }

    fn save_thumbnail(&mut self, index: usize, in_background: bool) {
        let tab = &mut self.tabs[index];
        if !tab.saved || !tab.has_picture() {
            return;
        }
        let Some(path) = self.storage.thumbnail_path(&tab.conn.id) else { return };
        let Some((w, h, pixels)) = tab.snapshot() else { return };
        let id = tab.conn.id.clone();
        if in_background {
            let tx = self.bg_tx.clone();
            let wake = self.wake.clone();
            self.runtime.spawn_blocking(move || {
                match storage::save_thumbnail(&path, w, h, pixels) {
                    Ok(()) => {
                        let _ = tx.send(BgResult::ThumbnailSaved(id));
                        wake();
                    }
                    Err(err) => log::warn!("could not save thumbnail: {err}"),
                }
            });
        } else {
            if let Err(err) = storage::save_thumbnail(&path, w, h, pixels) {
                log::warn!("could not save thumbnail: {err}");
            }
            self.home.thumbnails.remove(&id);
        }
    }

    pub fn take_screenshot(&mut self, index: usize) {
        let tab = &mut self.tabs[index];
        let Some((w, h, pixels)) = tab.snapshot() else {
            self.toast("There is no picture to save yet.");
            return;
        };
        let name: String = tab
            .title()
            .chars()
            .map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '.' { c } else { '-' })
            .collect();
        let stamp = chrono::Local::now().format("%Y-%m-%d at %H.%M.%S");
        let path = platform::screenshot_dir().join(format!("Mini Remote {name} {stamp}.png"));
        let tx = self.bg_tx.clone();
        let wake = self.wake.clone();
        self.runtime.spawn_blocking(move || {
            let result = match storage::save_png(&path, w, h, pixels) {
                Ok(()) => BgResult::ScreenshotSaved(path),
                Err(err) => BgResult::ScreenshotFailed(err),
            };
            let _ = tx.send(result);
            wake();
        });
    }

    pub fn toast(&mut self, text: impl Into<String>) {
        self.toast_with(text.into(), None);
    }

    pub fn toast_with(&mut self, text: String, action: Option<ToastAction>) {
        let id = self.next_toast_id;
        self.next_toast_id += 1;
        self.toasts.push(Toast { id, text, action, created: Instant::now() });
        if self.toasts.len() > 3 {
            self.toasts.remove(0);
        }
    }

    pub fn set_fullscreen(&mut self, on: bool) {
        self.fullscreen = on;
        self.requests.push(WindowRequest::Fullscreen(on));
    }

    pub fn retry_tab(&mut self, index: usize) {
        let env = session_env!(self);
        self.tabs[index].retry(&env);
    }

    pub fn submit_credentials(&mut self, index: usize) {
        let env = session_env!(self);
        self.tabs[index].submit_credentials(&env);
    }

    pub fn answer_identity(&mut self, index: usize, trust: bool) {
        if let Some(notice) = self.tabs[index].answer_identity(trust) {
            self.handle_notice(index, notice);
        }
    }

    // ----- Window events (called by the runner) ----------------------------------

    pub fn take_window_requests(&mut self) -> Vec<WindowRequest> {
        std::mem::take(&mut self.requests)
    }

    /// The runner reports whether the window is in full screen (the user may
    /// leave full screen with the window's own controls).
    pub fn sync_fullscreen(&mut self, on: bool) {
        self.fullscreen = on;
    }

    pub fn set_title_bar_height(&mut self, height: f32) {
        self.title_bar = height;
    }

    /// Ask the runner to do something with the window.
    pub fn window_request(&mut self, request: WindowRequest) {
        self.requests.push(request);
    }

    pub fn on_modifiers(&mut self, modifiers: ModifiersState) {
        self.modifiers = modifiers;
    }

    pub fn on_focus(&mut self, focused: bool) {
        self.focused = focused;
        if focused {
            self.poll_clipboard(true);
        } else {
            for tab in &mut self.tabs {
                tab.release_input();
            }
            self.remote_keyboard = false;
        }
    }

    /// A key was pressed or released. Returns true when it was used here
    /// (sent to the remote computer or an app shortcut) and egui must not see it.
    pub fn on_key(&mut self, event: &KeyEvent, synthetic: bool) -> bool {
        let input = KeyInput::from_winit(event);
        // Esc leaves full screen (unless turned off in Settings). Its release is
        // kept from the remote computer too, so it never sees half a key press.
        let escape = matches!(input.base, Key::Named(egui_winit::winit::keyboard::NamedKey::Escape));
        if escape && !input.pressed && self.swallow_escape_release {
            self.swallow_escape_release = false;
            return true;
        }
        if escape
            && input.pressed
            && self.fullscreen
            && self.settings.escape_leaves_fullscreen
            && self.modifiers.is_empty()
        {
            if !input.repeat {
                self.set_fullscreen(false);
            }
            self.swallow_escape_release = true;
            return true;
        }
        if let Some(shortcut) = shortcut_for(&input, self.modifiers) {
            if input.pressed && !input.repeat {
                self.run_shortcut(shortcut);
            }
            return true;
        }
        if !self.remote_keyboard {
            return false;
        }
        if synthetic {
            // Keys "pressed" while the window was in the background: ignore them.
            return true;
        }
        let settings = self.settings.clone();
        if let Some(tab) = self.active_tab_mut() {
            let cfg = tab.key_config(&settings);
            let msgs = tab.keyboard.key(&input, &cfg);
            tab.send_keys(&msgs);
        }
        true
    }

    pub fn run_shortcut(&mut self, shortcut: Shortcut) {
        match shortcut {
            Shortcut::FullScreen => {
                if self.active().is_some() || self.fullscreen {
                    self.set_fullscreen(!self.fullscreen);
                }
            }
            Shortcut::CloseTab => {
                if let Some(id) = self.active() {
                    self.close_tab(id);
                }
            }
            Shortcut::ToggleSidebar => self.cycle_sidebar(),
            Shortcut::NextTab | Shortcut::PreviousTab => {
                let mut ids: Vec<Option<u64>> = vec![None];
                ids.extend(self.tabs.iter().map(|t| Some(t.id)));
                let current = self.active();
                let pos = ids.iter().position(|id| *id == current).unwrap_or(0);
                let n = ids.len();
                let next = if shortcut == Shortcut::NextTab { (pos + 1) % n } else { (pos + n - 1) % n };
                self.activate(ids[next]);
            }
        }
    }

    /// Save what should survive and disconnect everything.
    pub fn on_exit(&mut self) {
        for i in 0..self.tabs.len() {
            self.save_thumbnail(i, false);
        }
        for tab in &mut self.tabs {
            tab.close();
        }
        self.tabs.clear();
        self.storage.save_settings(&self.settings);
    }

    /// Give the network threads a moment to say goodbye to the servers.
    pub fn shutdown(self) {
        let App { runtime, .. } = self;
        runtime.shutdown_timeout(Duration::from_millis(500));
    }
}

/// Development aid (`MINI_REMOTE_SAMPLE_DEVICES=1`): a pretend network scan with
/// made-up devices, for screenshots. The real network is not scanned.
async fn sample_devices(tx: mpsc::Sender<discovery::DiscoveryEvent>, wake: Wake) {
    use discovery::{DiscoveredHost, DiscoveryEvent};
    let host = |name: &str, ip: [u8; 4], pi: bool, vnc: bool, rfb: Option<&str>| DiscoveredHost {
        address: std::net::IpAddr::from(ip),
        hostname: Some(name.to_string()),
        vnc_ports: if vnc { vec![crate::model::DEFAULT_VNC_PORT] } else { Vec::new() },
        rfb_version: rfb.map(str::to_string),
        ssh: true,
        is_raspberry_pi: pi,
        service_name: None,
    };
    let hosts = [
        host("raspberrypi.local", [192, 168, 1, 50], true, true, Some("3.8")),
        host("pi-kitchen.local", [192, 168, 1, 51], true, false, None),
        host("nas.local", [192, 168, 1, 20], false, false, None),
        host("office-pc.local", [192, 168, 1, 30], false, true, Some("5.0")),
    ];
    for h in hosts {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if tx.send(DiscoveryEvent::Found(h)).is_err() {
            return;
        }
        wake();
    }
    tokio::time::sleep(Duration::from_millis(600)).await;
    let _ = tx.send(DiscoveryEvent::Finished);
    wake();
}

/// Name to show for a computer found on the network.
pub fn discovered_name(host: &discovery::DiscoveredHost) -> String {
    if let Some(name) = &host.hostname {
        let short = name.trim_end_matches('.').trim_end_matches(".local");
        if !short.is_empty() {
            return short.to_string();
        }
    }
    if let Some(name) = host.service_name.as_ref().filter(|n| !n.is_empty()) {
        return name.clone();
    }
    host.address.to_string()
}

/// Which app shortcut (if any) this key is. The shortcuts use Control+Command
/// on a Mac so they never clash with keys meant for the remote computer.
fn shortcut_for(input: &KeyInput, mods: ModifiersState) -> Option<Shortcut> {
    if !platform::is_app_shortcut(mods) {
        return None;
    }
    use egui_winit::winit::keyboard::NamedKey;
    match &input.base {
        Key::Character(c) if c.eq_ignore_ascii_case("f") => Some(Shortcut::FullScreen),
        Key::Character(c) if c.eq_ignore_ascii_case("w") => Some(Shortcut::CloseTab),
        Key::Named(NamedKey::ArrowRight) => Some(Shortcut::NextTab),
        Key::Named(NamedKey::ArrowLeft) => Some(Shortcut::PreviousTab),
        Key::Character(c) if c.eq_ignore_ascii_case("s") => Some(Shortcut::ToggleSidebar),
        _ => None,
    }
}

fn apply_appearance(ctx: &egui::Context, settings: &Settings) {
    use crate::model::Appearance;
    let pref = match settings.appearance {
        Appearance::System => egui::ThemePreference::System,
        Appearance::Light => egui::ThemePreference::Light,
        Appearance::Dark => egui::ThemePreference::Dark,
    };
    ctx.set_theme(pref);
}
