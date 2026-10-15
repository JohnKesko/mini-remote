//! File transfer with the remote computer over secure remote login (SSH /
//! SFTP): browsing folders, copying files both ways, and simple file actions.
//! The work runs on the network threads; results come back as messages.

use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use ssh::{CancelToken, RemoteEntry, Sftp, SshAuth, SshError, SshSession};

use crate::model::Connection;
use crate::remote_login::{self, HostKeyLink};
use crate::session::Wake;

/// Finished transfers stay in the list this long.
const KEEP_FINISHED: Duration = Duration::from_secs(8);

/// Hint shown when the Pi's secure remote login can't be reached.
pub const SSH_OFF_HINT: &str =
    "File transfer uses the Pi's secure remote login (SSH). Turn it on in Raspberry Pi Configuration > Interfaces > SSH.";

pub struct FilesCtx<'a> {
    pub runtime: &'a tokio::runtime::Handle,
    pub wake: &'a Wake,
}

/// What the file panel is connected to.
pub enum FilesConn {
    Idle,
    /// A username or password is needed first.
    NeedLogin,
    Connecting,
    Ready(Sftp),
    Failed { message: String, ssh_off: bool },
}

#[derive(Default)]
pub struct LoginForm {
    pub username: String,
    pub password: String,
    pub error: Option<String>,
    /// Set when the form was filled in, so the next attempt uses it.
    used: bool,
}

pub struct Transfer {
    pub id: u64,
    pub label: String,
    pub upload: bool,
    /// Bytes done and total.
    pub progress: Arc<Mutex<(u64, u64)>>,
    cancel: CancelToken,
    /// `Some` once finished: `Ok` or a plain-English error.
    pub result: Option<Result<(), String>>,
    pub finished_at: Option<Instant>,
    /// Where a download was saved.
    pub saved_to: Option<PathBuf>,
}

impl Transfer {
    pub fn fraction(&self) -> f32 {
        let (done, total) = *self.progress.lock();
        if total == 0 { 0.0 } else { (done as f64 / total as f64).clamp(0.0, 1.0) as f32 }
    }

    pub fn bytes(&self) -> (u64, u64) {
        *self.progress.lock()
    }
}

enum Msg {
    Connected(Result<(Sftp, String), SshError>),
    Listed { path: String, result: Result<Vec<RemoteEntry>, String> },
    Done(Result<(), String>),
    TransferDone { id: u64, result: Result<(), SshError> },
    Picked(Vec<PathBuf>),
}

/// Things the app should show after file panel work.
pub enum FilesNotice {
    Downloaded(PathBuf),
    Toast(String),
}

pub struct FilesPanel {
    pub open: bool,
    /// Show files whose names start with a dot.
    pub show_hidden: bool,
    pub conn: FilesConn,
    pub cwd: String,
    pub home: String,
    pub entries: Vec<RemoteEntry>,
    pub loading: bool,
    pub selected: Option<String>,
    /// Problem with the last action, shown above the list.
    pub error: Option<String>,
    pub transfers: Vec<Transfer>,
    /// Path being renamed and the new name typed so far.
    pub rename: Option<(String, String)>,
    /// Name typed for a new folder.
    pub new_folder: Option<String>,
    pub confirm_delete: Option<RemoteEntry>,
    pub login: LoginForm,
    /// Files dropped before the panel was ready.
    queued_uploads: Vec<PathBuf>,
    tx: mpsc::Sender<Msg>,
    rx: mpsc::Receiver<Msg>,
    next_id: u64,
}

impl FilesPanel {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        FilesPanel {
            open: false,
            show_hidden: false,
            conn: FilesConn::Idle,
            cwd: String::new(),
            home: String::new(),
            entries: Vec::new(),
            loading: false,
            selected: None,
            error: None,
            transfers: Vec::new(),
            rename: None,
            new_folder: None,
            confirm_delete: None,
            login: LoginForm::default(),
            queued_uploads: Vec::new(),
            tx,
            rx,
            next_id: 1,
        }
    }

    pub fn sftp(&self) -> Option<&Sftp> {
        match &self.conn {
            FilesConn::Ready(sftp) => Some(sftp),
            _ => None,
        }
    }

    /// Connect for file transfer, reusing the screen's SSH tunnel when there is one.
    #[allow(clippy::too_many_arguments)]
    pub fn connect(
        &mut self,
        ctx: &FilesCtx,
        conn: &Connection,
        tunnel: Option<SshSession>,
        screen_password: Option<&str>,
        ssh_password: Option<&str>,
        host_keys: Arc<HostKeyLink>,
        trusted: Option<String>,
    ) {
        if matches!(self.conn, FilesConn::Ready(_) | FilesConn::Connecting) {
            return;
        }
        let tx = self.tx.clone();
        let wake = ctx.wake.clone();
        if let Some(session) = tunnel {
            self.conn = FilesConn::Connecting;
            ctx.runtime.spawn(async move {
                let result = async {
                    let sftp = session.sftp().await?;
                    let home = sftp.home_dir().await?;
                    Ok((sftp, home))
                }
                .await;
                let _ = tx.send(Msg::Connected(result));
                wake();
            });
            return;
        }

        let cfg = if self.login.used {
            Ok(ssh::SshConfig {
                host: conn.ssh_host().to_string(),
                port: conn.ssh.port,
                username: self.login.username.trim().to_string(),
                auth: SshAuth::Password(self.login.password.clone()),
            })
        } else {
            remote_login::ssh_config(conn, screen_password, ssh_password)
        };
        let cfg = match cfg {
            Ok(cfg) => cfg,
            Err(_) => {
                if self.login.username.is_empty() {
                    self.login.username = conn.ssh_username().to_string();
                }
                self.conn = FilesConn::NeedLogin;
                return;
            }
        };
        self.conn = FilesConn::Connecting;
        let check = remote_login::host_key_check(host_keys, trusted, ctx.wake.clone());
        ctx.runtime.spawn(async move {
            let result = async {
                let session = SshSession::connect(cfg, check).await?;
                let sftp = session.sftp().await?;
                let home = sftp.home_dir().await?;
                Ok((sftp, home))
            }
            .await;
            let _ = tx.send(Msg::Connected(result));
            wake();
        });
    }

    /// Use what was typed in the login form for the next attempt.
    pub fn use_login_form(&mut self) {
        self.login.used = true;
        self.login.error = None;
        self.conn = FilesConn::Idle;
    }

    /// Try again after a failure.
    pub fn reset(&mut self) {
        self.conn = FilesConn::Idle;
        self.error = None;
    }

    /// Handle results from the network threads.
    pub fn poll(&mut self, ctx: &FilesCtx) -> Vec<FilesNotice> {
        let mut notices = Vec::new();
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Connected(Ok((sftp, home))) => {
                    log::info!("file transfer ready, home folder {home}");
                    self.conn = FilesConn::Ready(sftp);
                    self.home = home.clone();
                    self.login.error = None;
                    self.list(ctx, home);
                    let queued = std::mem::take(&mut self.queued_uploads);
                    if !queued.is_empty() {
                        self.upload(ctx, queued);
                    }
                }
                Msg::Connected(Err(err)) => {
                    log::info!("file transfer could not start: {err}");
                    self.conn = match err {
                        SshError::AuthFailed | SshError::PassphraseNeeded | SshError::WrongPassphrase => {
                            self.login.error = Some(err.to_string());
                            self.login.used = false;
                            if self.login.username.is_empty() {
                                self.login.username = "pi".into();
                            }
                            FilesConn::NeedLogin
                        }
                        SshError::Network(_) => FilesConn::Failed { message: err.to_string(), ssh_off: true },
                        other => FilesConn::Failed { message: other.to_string(), ssh_off: false },
                    };
                }
                Msg::Listed { path, result } => {
                    self.loading = false;
                    match result {
                        Ok(entries) => {
                            if path != self.cwd {
                                self.selected = None;
                            }
                            self.cwd = path;
                            self.entries = entries;
                            self.error = None;
                        }
                        Err(err) => self.error = Some(err),
                    }
                }
                Msg::Done(result) => {
                    if let Err(err) = result {
                        self.error = Some(err);
                    }
                    self.refresh(ctx);
                }
                Msg::TransferDone { id, result } => {
                    let mut refresh = false;
                    if let Some(t) = self.transfers.iter_mut().find(|t| t.id == id) {
                        t.finished_at = Some(Instant::now());
                        t.result = Some(match result {
                            Ok(()) => Ok(()),
                            Err(SshError::Cancelled) => Err("Cancelled".into()),
                            Err(err) => Err(err.to_string()),
                        });
                        if t.upload {
                            refresh = true;
                        } else if let (Some(Ok(())), Some(path)) = (&t.result, &t.saved_to) {
                            notices.push(FilesNotice::Downloaded(path.clone()));
                        }
                        if let Some(Err(err)) = &t.result
                            && err != "Cancelled"
                        {
                            notices.push(FilesNotice::Toast(format!("Copying “{}” failed: {err}", t.label)));
                        }
                    }
                    if refresh {
                        self.refresh(ctx);
                    }
                }
                Msg::Picked(paths) => {
                    if !paths.is_empty() {
                        self.upload(ctx, paths);
                    }
                }
            }
        }
        self.transfers.retain(|t| t.finished_at.is_none_or(|at| at.elapsed() < KEEP_FINISHED));
        notices
    }

    /// Something is running that needs the screen to update regularly.
    pub fn busy(&self) -> bool {
        self.loading
            || matches!(self.conn, FilesConn::Connecting)
            || !self.transfers.is_empty()
    }

    pub fn list(&mut self, ctx: &FilesCtx, path: String) {
        let Some(sftp) = self.sftp().cloned() else { return };
        self.loading = true;
        self.rename = None;
        self.new_folder = None;
        let tx = self.tx.clone();
        let wake = ctx.wake.clone();
        ctx.runtime.spawn(async move {
            let result = sftp.list_dir(&path).await.map_err(|e| e.to_string());
            let _ = tx.send(Msg::Listed { path, result });
            wake();
        });
    }

    pub fn refresh(&mut self, ctx: &FilesCtx) {
        let path = self.cwd.clone();
        self.list(ctx, path);
    }

    pub fn go_up(&mut self, ctx: &FilesCtx) {
        if let Some(parent) = parent_dir(&self.cwd) {
            self.list(ctx, parent);
        }
    }

    /// Run a quick file action and refresh the list afterwards.
    fn action<F, Fut>(&mut self, ctx: &FilesCtx, f: F)
    where
        F: FnOnce(Sftp) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<(), SshError>> + Send,
    {
        let Some(sftp) = self.sftp().cloned() else { return };
        let tx = self.tx.clone();
        let wake = ctx.wake.clone();
        ctx.runtime.spawn(async move {
            let result = f(sftp).await.map_err(|e| e.to_string());
            let _ = tx.send(Msg::Done(result));
            wake();
        });
    }

    pub fn make_folder(&mut self, ctx: &FilesCtx, name: &str) {
        let Some(name) = clean_name(name) else {
            self.error = Some("Please type a name without a slash (/).".into());
            return;
        };
        let path = join_remote(&self.cwd, &name);
        self.new_folder = None;
        self.action(ctx, move |sftp| async move { sftp.mkdir(&path).await });
    }

    pub fn rename_to(&mut self, ctx: &FilesCtx, from: String, new_name: &str) {
        let Some(name) = clean_name(new_name) else {
            self.error = Some("Please type a name without a slash (/).".into());
            return;
        };
        let to = join_remote(parent_dir(&from).as_deref().unwrap_or("/"), &name);
        self.rename = None;
        if to == from {
            return;
        }
        self.selected = Some(to.clone());
        self.action(ctx, move |sftp| async move { sftp.rename(&from, &to).await });
    }

    pub fn delete(&mut self, ctx: &FilesCtx, entry: RemoteEntry) {
        if self.selected.as_deref() == Some(entry.path.as_str()) {
            self.selected = None;
        }
        self.action(ctx, move |sftp| async move { sftp.remove(&entry.path).await });
    }

    /// Ask for files to copy to the remote computer (system file picker).
    pub fn pick_upload(&mut self, ctx: &FilesCtx) {
        let tx = self.tx.clone();
        let wake = ctx.wake.clone();
        ctx.runtime.spawn(async move {
            let paths = crate::platform::pick_files_to_upload().await;
            let _ = tx.send(Msg::Picked(paths));
            wake();
        });
    }

    /// Copy local files and folders into the current remote folder.
    pub fn upload(&mut self, ctx: &FilesCtx, paths: Vec<PathBuf>) {
        let Some(sftp) = self.sftp().cloned() else {
            // Not connected yet: copy them once the panel is ready.
            self.queued_uploads.extend(paths);
            return;
        };
        let dir = if self.cwd.is_empty() { self.home.clone() } else { self.cwd.clone() };
        for local in paths {
            let name = local.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            let is_dir = local.is_dir();
            let remote_dir = dir.clone();
            let sftp = sftp.clone();
            self.start_transfer(ctx, name.clone(), true, None, move |progress, cancel| {
                let sftp = sftp.clone();
                async move {
                    if is_dir {
                        sftp.upload_dir(&local, &remote_dir, progress, cancel).await
                    } else {
                        sftp.upload(&local, &join_remote(&remote_dir, &name), progress, cancel).await
                    }
                }
            });
        }
    }

    /// Copy a remote file or folder into the Downloads folder.
    pub fn download(&mut self, ctx: &FilesCtx, entry: &RemoteEntry) {
        let Some(sftp) = self.sftp().cloned() else { return };
        let downloads = crate::platform::download_dir();
        let remote = entry.path.clone();
        let is_dir = entry.is_dir;
        let target = if is_dir { downloads.join(&entry.name) } else { unique_path(&downloads, &entry.name) };
        let saved_to = target.clone();
        self.start_transfer(ctx, entry.name.clone(), false, Some(saved_to), move |progress, cancel| async move {
            if is_dir {
                sftp.download_dir(&remote, &downloads, progress, cancel).await
            } else {
                sftp.download(&remote, &target, progress, cancel).await
            }
        });
    }

    fn start_transfer<F, Fut>(&mut self, ctx: &FilesCtx, label: String, upload: bool, saved_to: Option<PathBuf>, f: F)
    where
        F: FnOnce(ssh::Progress, CancelToken) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<(), SshError>> + Send,
    {
        let id = self.next_id;
        self.next_id += 1;
        let progress = Arc::new(Mutex::new((0u64, 0u64)));
        let cancel = CancelToken::default();
        self.transfers.push(Transfer {
            id,
            label,
            upload,
            progress: progress.clone(),
            cancel: cancel.clone(),
            result: None,
            finished_at: None,
            saved_to,
        });
        let tx = self.tx.clone();
        let wake = ctx.wake.clone();
        let progress_wake = ctx.wake.clone();
        let report: ssh::Progress = Box::new(move |done, total| {
            *progress.lock() = (done, total);
            progress_wake();
        });
        ctx.runtime.spawn(async move {
            let result = f(report, cancel).await;
            let _ = tx.send(Msg::TransferDone { id, result });
            wake();
        });
    }

    pub fn cancel(&mut self, id: u64) {
        if let Some(t) = self.transfers.iter().find(|t| t.id == id) {
            t.cancel.cancel();
        }
    }

    pub fn cancel_all(&mut self) {
        for t in &self.transfers {
            t.cancel.cancel();
        }
    }
}

impl Default for FilesPanel {
    fn default() -> Self {
        Self::new()
    }
}

/// `dir` + `/` + `name`, without doubling slashes.
pub fn join_remote(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// The folder containing `path`, or `None` at the top.
pub fn parent_dir(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.rfind('/') {
        Some(0) => Some("/".to_string()),
        Some(i) => Some(trimmed[..i].to_string()),
        None => None,
    }
}

/// Clickable pieces of a path, starting at the home folder when inside it:
/// ("Home", "/home/pi"), ("Documents", "/home/pi/Documents").
pub fn breadcrumbs(path: &str, home: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let (start, rest) = if !home.is_empty() && (path == home || path.starts_with(&format!("{}/", home.trim_end_matches('/')))) {
        out.push(("Home".to_string(), home.to_string()));
        (home.trim_end_matches('/').to_string(), &path[home.len().min(path.len())..])
    } else {
        out.push(("/".to_string(), "/".to_string()));
        (String::new(), path)
    };
    let mut current = start;
    for part in rest.split('/').filter(|p| !p.is_empty()) {
        current = format!("{current}/{part}");
        out.push((part.to_string(), current.clone()));
    }
    out
}

fn clean_name(name: &str) -> Option<String> {
    let name = name.trim();
    (!name.is_empty() && !name.contains('/') && name != "." && name != "..").then(|| name.to_string())
}

/// A file name in `dir` that does not exist yet: "photo.jpg", "photo (2).jpg", ...
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (2..1000)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

/// "12 kB", "3.4 MB".
pub fn format_size(bytes: u64) -> String {
    let b = bytes as f64;
    if bytes < 1000 {
        format!("{bytes} bytes")
    } else if b < 1e6 {
        format!("{:.0} kB", b / 1e3)
    } else if b < 1e9 {
        format!("{:.1} MB", b / 1e6)
    } else {
        format!("{:.1} GB", b / 1e9)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_paths() {
        assert_eq!(join_remote("/home/pi", "a.txt"), "/home/pi/a.txt");
        assert_eq!(join_remote("/", "etc"), "/etc");
        assert_eq!(parent_dir("/home/pi"), Some("/home".into()));
        assert_eq!(parent_dir("/home"), Some("/".into()));
        assert_eq!(parent_dir("/"), None);
    }

    #[test]
    fn crumbs_start_at_home() {
        let c = breadcrumbs("/home/pi/Documents/old", "/home/pi");
        let names: Vec<_> = c.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["Home", "Documents", "old"]);
        assert_eq!(c[2].1, "/home/pi/Documents/old");
        let c = breadcrumbs("/etc/wayvnc", "/home/pi");
        assert_eq!(c[0], ("/".to_string(), "/".to_string()));
        assert_eq!(c[2].1, "/etc/wayvnc");
        assert_eq!(breadcrumbs("/home/pi", "/home/pi").len(), 1);
        // A folder that only starts with the same letters is not inside home.
        assert_eq!(breadcrumbs("/home/pine", "/home/pi")[0].0, "/");
    }

    #[test]
    fn names_and_sizes() {
        assert_eq!(clean_name(" new "), Some("new".into()));
        assert_eq!(clean_name("a/b"), None);
        assert_eq!(format_size(512), "512 bytes");
        assert_eq!(format_size(2_500_000), "2.5 MB");
        let dir = std::env::temp_dir();
        let p = unique_path(&dir, "mini-remote-surely-missing-file.txt");
        assert_eq!(p, dir.join("mini-remote-surely-missing-file.txt"));
    }
}
