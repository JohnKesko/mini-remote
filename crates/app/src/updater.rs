//! Checking for, downloading and installing new versions in the background,
//! using the `update` crate. The window shows the state through `ui::update`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};

use update::{Installed, Release};

use crate::session::Wake;

/// Where the update process is.
pub enum Status {
    Idle,
    Checking { manual: bool },
    /// A manual check found nothing newer.
    UpToDate,
    Available(Release),
    Downloading { release: Release, done: u64, total: u64 },
    /// Installed; the app restarts as soon as this is seen.
    Restarting,
    Failed { message: String, page: Option<String> },
}

enum Msg {
    Checked { manual: bool, result: Result<Option<Release>, String> },
    Progress { done: u64, total: u64 },
    Installed(Result<Installed, String>),
}

pub struct Updater {
    pub status: Status,
    /// The update window is showing.
    pub dialog_open: bool,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    cancel: Arc<AtomicBool>,
    installed: Option<Installed>,
}

impl Default for Updater {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Updater {
            status: Status::Idle,
            dialog_open: false,
            tx,
            rx,
            cancel: Arc::new(AtomicBool::new(false)),
            installed: None,
        }
    }
}

/// Something the app must act on.
pub enum Notice {
    /// A newer version was found.
    Found(Release),
    /// Installed: restart now.
    Restart(Installed),
}

impl Updater {
    pub fn busy(&self) -> bool {
        matches!(self.status, Status::Checking { .. } | Status::Downloading { .. } | Status::Restarting)
    }

    /// Ask whether there is a newer version. `manual` checks report "up to
    /// date" and errors; automatic ones stay quiet unless something is found.
    pub fn check(&mut self, runtime: &tokio::runtime::Handle, wake: Wake, manual: bool) {
        if self.busy() {
            return;
        }
        self.status = Status::Checking { manual };
        let tx = self.tx.clone();
        runtime.spawn_blocking(move || {
            let result = update::check().map_err(|e| e.to_string());
            let _ = tx.send(Msg::Checked { manual, result });
            wake();
        });
    }

    /// Download, verify and install the release being offered.
    pub fn start_update(&mut self, runtime: &tokio::runtime::Handle, wake: Wake) {
        let Status::Available(release) = &self.status else { return };
        let release = release.clone();
        self.cancel = Arc::new(AtomicBool::new(false));
        self.status = Status::Downloading { release: release.clone(), done: 0, total: release.asset.size };
        let tx = self.tx.clone();
        let cancel = self.cancel.clone();
        runtime.spawn_blocking(move || {
            let mut last_percent = u64::MAX;
            let mut progress = |done: u64, total: u64| {
                let percent = done * 100 / total.max(1);
                if percent != last_percent {
                    last_percent = percent;
                    let _ = tx.send(Msg::Progress { done, total });
                    wake();
                }
            };
            let result = update::download(&release, &mut progress, &cancel)
                .and_then(|downloaded| update::install(&downloaded))
                .map_err(|e| e.to_string());
            let _ = tx.send(Msg::Installed(result));
            wake();
        });
    }

    pub fn cancel_download(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Close the window (and forget a finished or failed check).
    pub fn dismiss(&mut self) {
        self.dialog_open = false;
        if !self.busy() {
            self.status = Status::Idle;
        }
    }

    /// Handle results from the background work.
    pub fn poll(&mut self, skipped: Option<&str>) -> Option<Notice> {
        let mut notice = None;
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Checked { manual, result } => match result {
                    Ok(Some(release)) => {
                        if !manual && skipped == Some(release.version.as_str()) {
                            log::info!("version {} is out but was skipped", release.version);
                            self.status = Status::Idle;
                        } else {
                            self.status = Status::Available(release.clone());
                            self.dialog_open = true;
                            notice = Some(Notice::Found(release));
                        }
                    }
                    Ok(None) => self.status = if manual { Status::UpToDate } else { Status::Idle },
                    Err(message) => {
                        log::info!("update check failed: {message}");
                        self.status = if manual { Status::Failed { message, page: None } } else { Status::Idle };
                    }
                },
                Msg::Progress { done, total } => {
                    if let Status::Downloading { done: d, total: t, .. } = &mut self.status {
                        *d = done;
                        *t = total;
                    }
                }
                Msg::Installed(Ok(installed)) => {
                    self.status = Status::Restarting;
                    self.installed = Some(installed);
                }
                Msg::Installed(Err(message)) => {
                    let page = match &self.status {
                        Status::Downloading { release, .. } => Some(release.page.clone()),
                        _ => None,
                    };
                    if self.cancel.load(Ordering::Relaxed) {
                        self.status = Status::Idle;
                        self.dialog_open = false;
                    } else {
                        self.status = Status::Failed { message, page };
                        self.dialog_open = true;
                    }
                }
            }
        }
        if let Some(installed) = self.installed.take() {
            notice = Some(Notice::Restart(installed));
        }
        notice
    }
}
