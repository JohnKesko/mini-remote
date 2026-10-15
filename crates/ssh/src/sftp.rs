//! Browsing and copying files on the remote computer (SFTP).

use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use russh::ChannelStream;
use russh::client::Msg;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::fs::Metadata;
use russh_sftp::client::{Config as SftpConfig, SftpSession};
use russh_sftp::protocol::{FileAttributes, OpenFlags};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::task::JoinSet;

use crate::error::SshError;
use crate::session::SshSession;

/// Size of the pieces a file is moved in.
const CHUNK: usize = 256 * 1024;
/// How many files of a folder are copied at the same time.
const FILES_AT_ONCE: usize = 4;
/// How many small delete requests are in flight at the same time.
const DELETES_AT_ONCE: usize = 16;
/// Progress is reported at most this often.
const REPORT_EVERY: Duration = Duration::from_millis(50);
/// How often a waiting transfer looks at the cancel flag.
const CANCEL_POLL: Duration = Duration::from_millis(100);

/// One file or folder on the remote computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEntry {
    /// Name without the folder it is in.
    pub name: String,
    /// Full path on the remote computer.
    pub path: String,
    /// True for folders, and for shortcuts (symbolic links) that lead to a folder.
    pub is_dir: bool,
    /// True if this is a shortcut (symbolic link).
    pub is_symlink: bool,
    /// Size in bytes (for shortcuts: of the thing they lead to).
    pub size: u64,
    /// When it was last changed, if the remote computer says.
    pub modified: Option<SystemTime>,
    /// Unix permission bits such as `0o644`, if the remote computer says.
    pub permissions: Option<u32>,
}

/// Progress callback: bytes done so far, and total bytes.
///
/// Called while a transfer runs (at most about twenty times a second) and once more at the end.
pub type Progress = Box<dyn FnMut(u64 /*done*/, u64 /*total*/) + Send>;

/// A cheap flag to stop a transfer. Clones share the same flag.
#[derive(Clone, Default, Debug)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// Asks the transfer to stop as soon as possible.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// True once [`cancel`](Self::cancel) has been called.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// The user's cancel flag plus our own, used to stop sibling copies after one of them failed.
#[derive(Clone)]
struct Stop {
    user: CancelToken,
    own: CancelToken,
}

impl Stop {
    fn new(user: CancelToken) -> Self {
        Stop {
            user,
            own: CancelToken::default(),
        }
    }

    fn is_set(&self) -> bool {
        self.user.is_cancelled() || self.own.is_cancelled()
    }
}

/// Runs `fut`, but gives up with [`SshError::Cancelled`] soon after the cancel flag is set.
async fn guard<T>(
    stop: &Stop,
    fut: impl Future<Output = Result<T, SshError>>,
) -> Result<T, SshError> {
    if stop.is_set() {
        return Err(SshError::Cancelled);
    }
    tokio::pin!(fut);
    let mut tick = tokio::time::interval(CANCEL_POLL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            result = &mut fut => return result,
            _ = tick.tick() => {
                if stop.is_set() {
                    return Err(SshError::Cancelled);
                }
            }
        }
    }
}

/// Counts bytes and tells the app about them, not too often.
struct Tracker {
    done: AtomicU64,
    total: AtomicU64,
    callback: Mutex<Progress>,
    last: Mutex<Instant>,
}

impl Tracker {
    fn new(total: u64, callback: Progress) -> Arc<Tracker> {
        Arc::new(Tracker {
            done: AtomicU64::new(0),
            total: AtomicU64::new(total),
            callback: Mutex::new(callback),
            // Far enough in the past that the first report goes out at once.
            last: Mutex::new(
                Instant::now()
                    .checked_sub(REPORT_EVERY * 2)
                    .unwrap_or_else(Instant::now),
            ),
        })
    }

    fn set_total(&self, total: u64) {
        self.total.store(total, Ordering::SeqCst);
        self.report(true);
    }

    fn add(&self, bytes: u64) {
        self.done.fetch_add(bytes, Ordering::SeqCst);
        self.report(false);
    }

    fn report(&self, force: bool) {
        {
            let mut last = self.last.lock();
            if !force && last.elapsed() < REPORT_EVERY {
                return;
            }
            *last = Instant::now();
        }
        let done = self.done.load(Ordering::SeqCst);
        let total = self.total.load(Ordering::SeqCst).max(done);
        (self.callback.lock())(done, total);
    }

    /// Final report: everything that was expected is done.
    fn finish(&self) {
        let done = self.done.load(Ordering::SeqCst);
        let total = self.total.load(Ordering::SeqCst).max(done);
        self.done.store(total, Ordering::SeqCst);
        self.report(true);
    }
}

/// Removes a half-written local file unless told otherwise.
struct LocalTemp {
    path: PathBuf,
    keep: bool,
}

impl Drop for LocalTemp {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// What kind of thing a directory entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Dir,
    File,
    Symlink,
    Other,
}

/// A listing entry plus whether it is a shortcut that leads nowhere.
struct Listed {
    entry: RemoteEntry,
    broken: bool,
}

fn kind_of(meta: &Metadata) -> Option<Kind> {
    meta.permissions.map(|mode| match mode & 0o170000 {
        0o040000 => Kind::Dir,
        0o120000 => Kind::Symlink,
        0o100000 => Kind::File,
        _ => Kind::Other,
    })
}

fn modified_of(meta: &Metadata) -> Option<SystemTime> {
    meta.mtime
        .map(|secs| UNIX_EPOCH + Duration::from_secs(u64::from(secs)))
}

/// Joins a remote folder and a name with exactly one slash.
fn join_remote(parent: &str, name: &str) -> String {
    if parent.ends_with('/') {
        format!("{parent}{name}")
    } else if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

/// True for a name that is a single, ordinary file or folder name.
fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\0'])
        && !(cfg!(windows) && name.contains(['\\', ':']))
}

/// The last part of a remote path.
fn base_name(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or("")
}

/// A name for a half-finished file that will not clash with anything.
fn temp_name(name: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let short: String = name.chars().take(40).collect();
    format!(
        ".{short}.{:x}-{:x}.part",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// Same folder as `target`, temporary name.
fn temp_remote(target: &str) -> String {
    let trimmed = target.trim_end_matches('/');
    let name = base_name(trimmed);
    let dir = &trimmed[..trimmed.len() - name.len()];
    format!("{dir}{}", temp_name(name))
}

/// Turns an error from reading or writing a remote file into one of ours.
fn remote_io(e: io::Error) -> SshError {
    if let Some(inner) = e.get_ref().and_then(|i| i.downcast_ref::<SftpError>()) {
        return inner.clone().into();
    }
    match e.kind() {
        io::ErrorKind::TimedOut => SftpError::Timeout.into(),
        _ => SshError::Sftp(format!("The file transfer was interrupted: {e}")),
    }
}

/// File transfer session. Cheap to clone and usable from several tasks at once; each copy shares
/// the same connection. Keeps the login open for as long as it exists.
#[derive(Clone)]
pub struct Sftp {
    inner: Arc<SftpInner>,
}

struct SftpInner {
    sftp: SftpSession,
    _session: SshSession,
}

impl std::fmt::Debug for Sftp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Sftp")
    }
}

impl Sftp {
    pub(crate) async fn start(
        session: SshSession,
        stream: ChannelStream<Msg>,
    ) -> Result<Sftp, SshError> {
        let config = SftpConfig {
            max_packet_len: 256 * 1024,
            // Several requests in flight at once keep the line full.
            max_concurrent_reads: 32,
            max_concurrent_writes: 16,
            max_write_packet_len: 256 * 1024,
            // Slow lines may need a long time to answer a big queue of writes.
            request_timeout_secs: 120,
        };
        let sftp = SftpSession::new_with_config(stream, config).await?;
        Ok(Sftp {
            inner: Arc::new(SftpInner {
                sftp,
                _session: session,
            }),
        })
    }

    fn s(&self) -> &SftpSession {
        &self.inner.sftp
    }

    /// The user's home folder on the remote computer.
    pub async fn home_dir(&self) -> Result<String, SshError> {
        Ok(self.s().canonicalize(".").await?)
    }

    /// Lists a folder: folders first, then files, each sorted by name ignoring capitals.
    pub async fn list_dir(&self, path: &str) -> Result<Vec<RemoteEntry>, SshError> {
        Ok(self
            .list_inner(path)
            .await?
            .into_iter()
            .map(|l| l.entry)
            .collect())
    }

    /// Like [`list_dir`](Self::list_dir), but also notes which shortcuts lead nowhere.
    async fn list_inner(&self, path: &str) -> Result<Vec<Listed>, SshError> {
        let listing = self.s().read_dir(path).await?;
        let mut items: Vec<Listed> = Vec::new();
        let mut links = JoinSet::new();
        for item in listing {
            // "." and ".." are already left out by the library.
            let name = item.file_name();
            if !is_plain_name(&name) {
                // A dishonest server could try to make a download land outside the chosen folder.
                log::warn!("ignoring an entry with an unsafe name: {name:?}");
                continue;
            }
            let full = join_remote(path, &name);
            let meta = item.metadata();
            let kind = kind_of(&meta);
            let entry = RemoteEntry {
                is_dir: kind == Some(Kind::Dir),
                is_symlink: kind == Some(Kind::Symlink),
                size: meta.size.unwrap_or(0),
                modified: modified_of(&meta),
                permissions: meta.permissions.map(|p| p & 0o7777),
                path: full.clone(),
                name,
            };
            if entry.is_symlink {
                // Find out what the shortcut leads to.
                let index = items.len();
                let inner = self.inner.clone();
                links.spawn(async move { (index, inner.sftp.metadata(full).await.ok()) });
            }
            items.push(Listed {
                entry,
                broken: false,
            });
        }
        while let Some(done) = links.join_next().await {
            let Ok((index, target)) = done else { continue };
            let item = &mut items[index];
            match target {
                Some(target) => {
                    item.entry.is_dir = kind_of(&target) == Some(Kind::Dir);
                    item.entry.size = target.size.unwrap_or(item.entry.size);
                    item.entry.modified = modified_of(&target).or(item.entry.modified);
                }
                None => item.broken = true,
            }
        }
        items.sort_by(|a, b| {
            let (a, b) = (&a.entry, &b.entry);
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(items)
    }

    /// Copies one remote file to `local` (the full path of the new file, not a folder).
    ///
    /// The data goes to a temporary file next to `local` and is renamed when complete, so a
    /// failed or cancelled copy never leaves a half-written file behind. An existing file is replaced.
    pub async fn download(
        &self,
        remote: &str,
        local: &Path,
        progress: Progress,
        cancel: CancelToken,
    ) -> Result<(), SshError> {
        let stop = Stop::new(cancel);
        let tracker = Tracker::new(0, progress);
        self.download_file(remote, local, &tracker, &stop, true)
            .await?;
        tracker.finish();
        Ok(())
    }

    /// Copies one local file to the remote computer as `remote` (the full path of the new file,
    /// not a folder). Replaces an existing file, and is only visible there once complete.
    pub async fn upload(
        &self,
        local: &Path,
        remote: &str,
        progress: Progress,
        cancel: CancelToken,
    ) -> Result<(), SshError> {
        let stop = Stop::new(cancel);
        let tracker = Tracker::new(0, progress);
        self.upload_file(local, remote, &tracker, &stop, true)
            .await?;
        tracker.finish();
        Ok(())
    }

    /// Copies a whole local folder (and everything in it) into the remote folder `remote_parent`.
    ///
    /// Shortcuts (symbolic links) to files are copied as files; shortcuts to folders are skipped.
    /// If cancelled, files that were already finished stay on the remote computer.
    pub async fn upload_dir(
        &self,
        local: &Path,
        remote_parent: &str,
        progress: Progress,
        cancel: CancelToken,
    ) -> Result<(), SshError> {
        let stop = Stop::new(cancel);
        let name = local
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| {
                SshError::LocalFile(format!(
                    "\"{}\" is not a folder that can be copied.",
                    local.display()
                ))
            })?;
        let scan_root = local.to_path_buf();
        let plan = tokio::task::spawn_blocking(move || scan_local(&scan_root))
            .await
            .map_err(|e| SshError::LocalFile(e.to_string()))?
            .map_err(|e| SshError::local(local, &e))?;
        let total: u64 = plan.files.iter().map(|f| f.size).sum();
        let tracker = Tracker::new(total, progress);
        tracker.report(true);

        let remote_root = join_remote(remote_parent, &name);
        guard(&stop, self.ensure_dir(&remote_root)).await?;
        for dir in &plan.dirs {
            guard(
                &stop,
                self.ensure_dir(&join_remote(&remote_root, &dir.remote_rel)),
            )
            .await?;
        }
        let jobs: Vec<(PathBuf, String, u64)> = plan
            .files
            .into_iter()
            .map(|f| (f.local, join_remote(&remote_root, &f.remote_rel), f.size))
            .collect();
        let this = self.clone();
        let (t, s) = (tracker.clone(), stop.clone());
        run_pool(FILES_AT_ONCE, &stop, jobs, move |(from, to, _size)| {
            let (this, t, s) = (this.clone(), t.clone(), s.clone());
            async move { this.upload_file(&from, &to, &t, &s, false).await }
        })
        .await?;
        tracker.finish();
        Ok(())
    }

    /// Copies a whole remote folder (and everything in it) into the local folder `local_parent`.
    ///
    /// Shortcuts (symbolic links) to files are copied as files; shortcuts to folders are skipped.
    /// If cancelled, files that were already finished stay on this computer.
    pub async fn download_dir(
        &self,
        remote: &str,
        local_parent: &Path,
        progress: Progress,
        cancel: CancelToken,
    ) -> Result<(), SshError> {
        let stop = Stop::new(cancel);
        let name = base_name(remote);
        if name.is_empty() {
            return Err(SshError::Sftp(
                "Choose a folder inside the remote computer, not its top level.".into(),
            ));
        }
        let local_root = local_parent.join(name);

        // First look through everything, so that the total size is known.
        let mut dirs: Vec<String> = Vec::new(); // relative to the root, with '/'
        let mut files: Vec<(String, String, u64)> = Vec::new(); // remote path, relative path, size
        let mut pending: Vec<String> = vec![String::new()];
        while let Some(rel) = pending.pop() {
            let here = if rel.is_empty() {
                remote.to_string()
            } else {
                join_remote(remote, &rel)
            };
            let list = guard(&stop, self.list_inner(&here)).await?;
            for Listed { entry: e, broken } in list {
                let child_rel = join_remote(&rel, &e.name);
                if e.is_symlink {
                    // Shortcuts to files are copied as files; others are skipped (they could loop).
                    if !broken && !e.is_dir {
                        files.push((e.path, child_rel, e.size));
                    }
                } else if e.is_dir {
                    dirs.push(child_rel.clone());
                    pending.push(child_rel);
                } else {
                    files.push((e.path, child_rel, e.size));
                }
            }
        }
        let total: u64 = files.iter().map(|f| f.2).sum();
        let tracker = Tracker::new(total, progress);
        tracker.report(true);

        tokio::fs::create_dir_all(&local_root)
            .await
            .map_err(|e| SshError::local(&local_root, &e))?;
        for rel in &dirs {
            let dir = local_root.join(rel);
            tokio::fs::create_dir_all(&dir)
                .await
                .map_err(|e| SshError::local(&dir, &e))?;
        }
        let jobs: Vec<(String, PathBuf)> = files
            .into_iter()
            .map(|(from, rel, _)| (from, local_root.join(rel)))
            .collect();
        let this = self.clone();
        let (t, s) = (tracker.clone(), stop.clone());
        run_pool(FILES_AT_ONCE, &stop, jobs, move |(from, to)| {
            let (this, t, s) = (this.clone(), t.clone(), s.clone());
            async move { this.download_file(&from, &to, &t, &s, false).await }
        })
        .await?;
        tracker.finish();
        Ok(())
    }

    /// Makes a new folder.
    pub async fn mkdir(&self, path: &str) -> Result<(), SshError> {
        Ok(self.s().create_dir(path).await?)
    }

    /// Renames or moves a file or folder.
    pub async fn rename(&self, from: &str, to: &str) -> Result<(), SshError> {
        Ok(self.s().rename(from, to).await?)
    }

    /// Deletes a file, or a folder with everything in it. A shortcut is deleted, not what it leads to.
    pub async fn remove(&self, path: &str) -> Result<(), SshError> {
        let top = self.s().symlink_metadata(path).await?;
        if kind_of(&top) != Some(Kind::Dir) {
            return Ok(self.s().remove_file(path).await?);
        }
        let mut dirs = vec![path.to_string()]; // parents come before their children
        let mut files: Vec<String> = Vec::new();
        let mut pending = vec![path.to_string()];
        while let Some(dir) = pending.pop() {
            for item in self.s().read_dir(dir.clone()).await? {
                let child = join_remote(&dir, &item.file_name());
                let kind = match kind_of(&item.metadata()) {
                    Some(k) => k,
                    None => kind_of(&self.s().symlink_metadata(child.clone()).await?)
                        .unwrap_or(Kind::File),
                };
                if kind == Kind::Dir {
                    dirs.push(child.clone());
                    pending.push(child);
                } else {
                    files.push(child);
                }
            }
        }
        let this = self.clone();
        run_pool(
            DELETES_AT_ONCE,
            &Stop::new(CancelToken::default()),
            files,
            move |file| {
                let this = this.clone();
                async move { Ok(this.s().remove_file(file).await?) }
            },
        )
        .await?;
        for dir in dirs.into_iter().rev() {
            self.s().remove_dir(dir).await?;
        }
        Ok(())
    }

    /// Makes a folder unless it is already there.
    async fn ensure_dir(&self, path: &str) -> Result<(), SshError> {
        match self.s().create_dir(path).await {
            Ok(()) => Ok(()),
            Err(e) => match self.s().metadata(path).await {
                Ok(meta) if kind_of(&meta) == Some(Kind::Dir) => Ok(()),
                _ => Err(e.into()),
            },
        }
    }

    /// Copies one remote file. `single` is true when this is the whole job (it sets the total).
    async fn download_file(
        &self,
        remote: &str,
        local: &Path,
        tracker: &Tracker,
        stop: &Stop,
        single: bool,
    ) -> Result<(), SshError> {
        let file_name = local
            .file_name()
            .ok_or_else(|| {
                SshError::LocalFile(format!("\"{}\" is not a file name.", local.display()))
            })?
            .to_string_lossy()
            .into_owned();
        if local.is_dir() {
            return Err(SshError::LocalFile(format!(
                "\"{}\" is a folder. Choose a file name for the download.",
                local.display()
            )));
        }
        let mut remote_file = guard(stop, async { Ok(self.s().open(remote).await?) }).await?;
        let meta = guard(stop, async { Ok(remote_file.metadata().await?) }).await?;
        if kind_of(&meta) == Some(Kind::Dir) {
            return Err(SshError::Sftp("That is a folder, not a file.".into()));
        }
        if single {
            tracker.set_total(meta.size.unwrap_or(0));
        }

        let temp_path = local.with_file_name(temp_name(&file_name));
        let mut temp = LocalTemp {
            path: temp_path.clone(),
            keep: false,
        };
        let mut out = tokio::fs::File::create(&temp_path)
            .await
            .map_err(|e| SshError::local(local, &e))?;
        let mut buf = vec![0u8; CHUNK];
        loop {
            let n = guard(stop, async {
                remote_file.read(&mut buf).await.map_err(remote_io)
            })
            .await?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])
                .await
                .map_err(|e| SshError::local(local, &e))?;
            tracker.add(n as u64);
        }
        out.flush().await.map_err(|e| SshError::local(local, &e))?;
        drop(out);
        #[cfg(unix)]
        if let Some(mode) = meta
            .permissions
            .map(|p| p & 0o777)
            .filter(|m| m & 0o600 == 0o600)
        {
            use std::os::unix::fs::PermissionsExt;
            let _ =
                tokio::fs::set_permissions(&temp_path, std::fs::Permissions::from_mode(mode)).await;
        }
        tokio::fs::rename(&temp_path, local)
            .await
            .map_err(|e| SshError::local(local, &e))?;
        temp.keep = true;
        let _ = tokio::time::timeout(Duration::from_secs(5), remote_file.close()).await;
        Ok(())
    }

    /// Copies one local file. `single` is true when this is the whole job (it sets the total).
    async fn upload_file(
        &self,
        local: &Path,
        remote: &str,
        tracker: &Tracker,
        stop: &Stop,
        single: bool,
    ) -> Result<(), SshError> {
        let mut source = tokio::fs::File::open(local)
            .await
            .map_err(|e| SshError::local(local, &e))?;
        let meta = source
            .metadata()
            .await
            .map_err(|e| SshError::local(local, &e))?;
        if meta.is_dir() {
            return Err(SshError::LocalFile(format!(
                "\"{}\" is a folder. Use the folder upload instead.",
                local.display()
            )));
        }
        if single {
            tracker.set_total(meta.len());
        }
        let mut attrs = FileAttributes::empty();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = meta.permissions().mode() & 0o777;
            if mode & 0o600 == 0o600 {
                attrs.permissions = Some(mode);
            }
        }

        let temp = temp_remote(remote);
        let mut target = guard(stop, async {
            Ok(self
                .s()
                .open_with_flags_and_attributes(
                    temp.clone(),
                    OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
                    attrs,
                )
                .await?)
        })
        .await?;

        let copied: Result<(), SshError> = async {
            let mut buf = vec![0u8; CHUNK];
            loop {
                if stop.is_set() {
                    return Err(SshError::Cancelled);
                }
                let n = source
                    .read(&mut buf)
                    .await
                    .map_err(|e| SshError::local(local, &e))?;
                if n == 0 {
                    break;
                }
                guard(stop, async {
                    target.write_all(&buf[..n]).await.map_err(remote_io)
                })
                .await?;
                tracker.add(n as u64);
            }
            // Waits until the remote computer has confirmed every piece, then closes the file.
            guard(stop, async { target.shutdown().await.map_err(remote_io) }).await
        }
        .await;

        let finished = match copied {
            Ok(()) => self.move_into_place(&temp, remote).await,
            Err(e) => Err(e),
        };
        if let Err(e) = finished {
            drop(target);
            // Clean up the half-finished file; if the connection is gone this simply fails.
            let _ = tokio::time::timeout(Duration::from_secs(10), self.s().remove_file(temp)).await;
            return Err(e);
        }
        Ok(())
    }

    /// Renames the finished temporary file to its real name, replacing an older file.
    async fn move_into_place(&self, temp: &str, target: &str) -> Result<(), SshError> {
        match self.s().rename(temp, target).await {
            Ok(()) => Ok(()),
            Err(first) => {
                // The plain rename refuses to replace a file, so remove the old one and try again.
                match self.s().symlink_metadata(target).await {
                    Ok(old) if kind_of(&old) != Some(Kind::Dir) => {
                        self.s().remove_file(target).await?;
                        Ok(self.s().rename(temp, target).await?)
                    }
                    Ok(_) => Err(SshError::Sftp(
                        "A folder with that name already exists on the remote computer.".into(),
                    )),
                    Err(_) => Err(first.into()),
                }
            }
        }
    }
}

/// Runs `work` for every item, a few at a time. Stops starting new work after the first problem,
/// lets the running work finish, and reports the first problem (or `Cancelled` if the user cancelled).
async fn run_pool<T, F, Fut>(
    limit: usize,
    stop: &Stop,
    items: Vec<T>,
    work: F,
) -> Result<(), SshError>
where
    F: Fn(T) -> Fut,
    Fut: Future<Output = Result<(), SshError>> + Send + 'static,
{
    let mut running = JoinSet::new();
    let mut items = items.into_iter();
    let mut first_error: Option<SshError> = None;
    loop {
        while running.len() < limit && first_error.is_none() && !stop.is_set() {
            match items.next() {
                Some(item) => {
                    running.spawn(work(item));
                }
                None => break,
            }
        }
        let Some(done) = running.join_next().await else {
            break;
        };
        let problem = match done {
            Ok(Ok(())) => None,
            Ok(Err(e)) => Some(e),
            Err(join) => Some(SshError::Sftp(format!(
                "A copy stopped unexpectedly: {join}"
            ))),
        };
        if let Some(e) = problem {
            stop.own.cancel();
            // A real problem is more useful than the "cancelled" it causes in the other copies.
            if first_error.is_none() || matches!(first_error, Some(SshError::Cancelled)) {
                first_error = Some(e);
            }
        }
    }
    if stop.user.is_cancelled() {
        return Err(SshError::Cancelled);
    }
    match first_error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Everything found inside a local folder.
struct LocalPlan {
    dirs: Vec<PlanDir>,
    files: Vec<PlanFile>,
}

struct PlanDir {
    /// Path inside the copied folder, with `/` between the parts.
    remote_rel: String,
}

struct PlanFile {
    local: PathBuf,
    remote_rel: String,
    size: u64,
}

/// Walks a local folder. Parents are listed before their children.
fn scan_local(root: &Path) -> io::Result<LocalPlan> {
    let mut plan = LocalPlan {
        dirs: Vec::new(),
        files: Vec::new(),
    };
    let mut pending: Vec<(PathBuf, String)> = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, rel)) = pending.pop() {
        for item in std::fs::read_dir(&dir)? {
            let item = item?;
            let name = item.file_name().to_string_lossy().into_owned();
            let child_rel = join_remote(&rel, &name);
            let kind = item.file_type()?;
            if kind.is_dir() {
                plan.dirs.push(PlanDir {
                    remote_rel: child_rel.clone(),
                });
                pending.push((item.path(), child_rel));
            } else if kind.is_file() {
                plan.files.push(PlanFile {
                    size: item.metadata()?.len(),
                    local: item.path(),
                    remote_rel: child_rel,
                });
            } else if kind.is_symlink() {
                // Follow a shortcut to a file; skip shortcuts to folders (they could loop).
                if let Ok(target) = std::fs::metadata(item.path())
                    && target.is_file()
                {
                    plan.files.push(PlanFile {
                        size: target.len(),
                        local: item.path(),
                        remote_rel: child_rel,
                    });
                }
            }
        }
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_remote_paths() {
        assert_eq!(join_remote("/home/pi", "a.txt"), "/home/pi/a.txt");
        assert_eq!(join_remote("/", "a"), "/a");
        assert_eq!(join_remote("", "a"), "a");
        assert_eq!(join_remote("/x/", "a"), "/x/a");
    }

    #[test]
    fn finds_base_names_and_temp_files() {
        assert_eq!(base_name("/home/pi/docs/"), "docs");
        assert_eq!(base_name("/"), "");
        let t = temp_remote("/home/pi/report.txt");
        assert!(t.starts_with("/home/pi/.report.txt."), "{t}");
        assert!(t.ends_with(".part"));
        assert_ne!(temp_remote("/a/b"), temp_remote("/a/b"));
        assert!(temp_remote("b").starts_with(".b."));
    }

    #[test]
    fn unsafe_names_are_recognised() {
        for good in ["a.txt", ".hidden", "with space", "ü", "..."] {
            assert!(is_plain_name(good), "{good}");
        }
        for bad in ["", ".", "..", "a/b", "../x", "/etc", "nul\0byte"] {
            assert!(!is_plain_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn long_names_get_short_temp_names() {
        let long = "é".repeat(200);
        assert!(temp_name(&long).len() < 255);
    }

    #[test]
    fn cancel_tokens_share_the_flag() {
        let a = CancelToken::default();
        let b = a.clone();
        assert!(!b.is_cancelled());
        a.cancel();
        assert!(b.is_cancelled());
    }

    #[test]
    fn progress_is_throttled_but_final_report_is_exact() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let tracker = Tracker::new(100, Box::new(move |d, t| sink.lock().push((d, t))));
        for _ in 0..10 {
            tracker.add(10);
        }
        tracker.finish();
        let seen = seen.lock();
        assert!(seen.len() <= 3, "{seen:?}");
        assert_eq!(*seen.last().unwrap(), (100, 100));
    }

    #[test]
    fn scans_local_folders() {
        let root = std::env::temp_dir().join(format!("mini-remote-ssh-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::write(root.join("x.txt"), b"12345").unwrap();
        std::fs::write(root.join("a/b/y.txt"), b"1").unwrap();
        let plan = scan_local(&root).unwrap();
        let mut dirs: Vec<_> = plan.dirs.iter().map(|d| d.remote_rel.clone()).collect();
        dirs.sort();
        assert_eq!(dirs, ["a", "a/b"]);
        let mut files: Vec<_> = plan
            .files
            .iter()
            .map(|f| (f.remote_rel.clone(), f.size))
            .collect();
        files.sort();
        assert_eq!(
            files,
            [("a/b/y.txt".to_string(), 1), ("x.txt".to_string(), 5)]
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
