//! Windows and Linux. On Linux passwords go to the desktop's password store
//! (GNOME Keyring, KWallet and the like); on Windows they are not remembered
//! until a password store for it is plugged in here.

use std::io;
use std::path::Path;
use std::process::Command;

use egui_winit::winit::keyboard::{ModifiersState, PhysicalKey};
use egui_winit::winit::window::WindowAttributes;

use super::{Folder, SecretStore};
use crate::model::MetaMapping;

pub const SECRET_STORE_NAME: &str = "password store";
#[cfg(target_os = "windows")]
pub const REVEAL_LABEL: &str = "Show in Explorer";
#[cfg(not(target_os = "windows"))]
pub const REVEAL_LABEL: &str = "Show in folder";
pub const META_KEY_LABEL: &str = "Windows key";
pub const ALT_KEY_LABEL: &str = "Alt";
pub const DEFAULT_META: MetaMapping = MetaMapping::Super;
pub const CAPS_LOCK_REPORTS_STATE: bool = false;
pub(super) const SCREENSHOT_FOLDER: Folder = Folder::Pictures;
pub const SHORTCUT_PREFIX: &str = "Ctrl+Alt+Shift+";

pub fn is_app_shortcut(mods: ModifiersState) -> bool {
    mods.control_key() && mods.alt_key() && mods.shift_key()
}

#[cfg(target_os = "linux")]
pub fn secret_store() -> Box<dyn SecretStore> {
    Box::new(secret_service::SecretService::new())
}

#[cfg(not(target_os = "linux"))]
pub fn secret_store() -> Box<dyn SecretStore> {
    Box::new(NoStore)
}

/// Placeholder until the system password store is wired in.
#[cfg(not(target_os = "linux"))]
struct NoStore;

#[cfg(not(target_os = "linux"))]
impl SecretStore for NoStore {
    fn get(&self, _key: &str) -> Option<String> {
        None
    }

    fn set(&self, _key: &str, _secret: &str) -> Result<(), String> {
        Err("Remembering passwords is not available on this system yet.".into())
    }

    fn delete(&self, _key: &str) {}

    fn available(&self) -> bool {
        false
    }
}

/// Passwords in the desktop's password store, reached over D-Bus with the
/// Secret Service protocol that GNOME Keyring, KWallet and others speak.
#[cfg(target_os = "linux")]
mod secret_service {
    use std::sync::OnceLock;
    use std::sync::mpsc::{self, Receiver};
    use std::time::Duration;

    use super::SecretStore;

    /// Name the passwords are filed under in the password store.
    const SERVICE: &str = "Mini Remote";

    /// How long to wait for the start-up check before giving up on the store.
    const CHECK_WAIT: Duration = Duration::from_secs(3);

    pub(super) struct SecretService {
        /// Answer from the start-up check, until it has been read.
        check: parking_lot::Mutex<Option<Receiver<bool>>>,
        reachable: OnceLock<bool>,
    }

    impl SecretService {
        pub(super) fn new() -> Self {
            let (tx, rx) = mpsc::channel();
            // Check in the background, so a slow or missing store never holds
            // up the window.
            let spawned = std::thread::Builder::new().name("password store check".into()).spawn(move || {
                let _ = tx.send(reachable());
            });
            let check = spawned.is_ok().then_some(rx);
            Self { check: parking_lot::Mutex::new(check), reachable: OnceLock::new() }
        }
    }

    /// Is a password store running? Looking up a name that is never saved
    /// tells, without asking the user to unlock anything.
    fn reachable() -> bool {
        let result = keyring::Entry::new(SERVICE, "availability check").and_then(|entry| entry.get_password());
        match result {
            Ok(_)
            | Err(keyring::Error::NoEntry | keyring::Error::NoStorageAccess(_) | keyring::Error::Ambiguous(_)) => true,
            Err(err) => {
                log::info!("no password store found, so passwords will not be remembered: {err}");
                false
            }
        }
    }

    impl SecretStore for SecretService {
        fn get(&self, key: &str) -> Option<String> {
            if !self.available() {
                return None;
            }
            let entry = keyring::Entry::new(SERVICE, key).ok()?;
            match entry.get_password() {
                Ok(secret) => Some(secret),
                Err(keyring::Error::NoEntry) => None,
                Err(err) => {
                    log::warn!("could not read a password from the password store: {err}");
                    None
                }
            }
        }

        fn set(&self, key: &str, secret: &str) -> Result<(), String> {
            if !self.available() {
                return Err("No password store is running.".into());
            }
            let entry = keyring::Entry::new(SERVICE, key).map_err(|e| e.to_string())?;
            entry.set_password(secret).map_err(|e| e.to_string())
        }

        fn delete(&self, key: &str) {
            if !self.available() {
                return;
            }
            if let Ok(entry) = keyring::Entry::new(SERVICE, key) {
                match entry.delete_credential() {
                    Ok(()) | Err(keyring::Error::NoEntry) => {}
                    Err(err) => log::warn!("could not remove a password from the password store: {err}"),
                }
            }
        }

        fn available(&self) -> bool {
            *self.reachable.get_or_init(|| {
                let check = self.check.lock().take();
                check.and_then(|rx| rx.recv_timeout(CHECK_WAIT).ok()).unwrap_or(false)
            })
        }
    }
}

pub fn reveal(path: &Path) -> io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        Command::new("explorer").arg("/select,").arg(path).spawn().map(|_| ())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let folder = path.parent().unwrap_or(path);
        Command::new("xdg-open").arg(folder).spawn().map(|_| ())
    }
}

pub fn open_url(url: &str) -> io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        Command::new("cmd").args(["/C", "start", "", url]).spawn().map(|_| ())
    }
    #[cfg(not(target_os = "windows"))]
    {
        Command::new("xdg-open").arg(url).spawn().map(|_| ())
    }
}

pub fn window_attributes(attrs: WindowAttributes) -> WindowAttributes {
    // The name matches mini-remote.desktop, so the dock and task switcher
    // show the app's own icon (Wayland app id and X11 window class).
    #[cfg(target_os = "linux")]
    let attrs = {
        use egui_winit::winit::platform::wayland::WindowAttributesExtWayland;
        attrs.with_name("mini-remote", "mini-remote")
    };
    attrs
}

/// The native title bar is kept here, so the app header has no window buttons.
/// Other systems keep their normal title bar.
pub fn window_chrome(_window: &egui_winit::winit::window::Window, _fullscreen: bool) {}

pub fn title_bar_height(_window: &egui_winit::winit::window::Window) -> f32 {
    0.0
}

pub const WINDOW_BUTTONS_WIDTH: f32 = 0.0;

pub fn physical_key(key: PhysicalKey) -> PhysicalKey {
    key
}

/// Let the user choose a folder: the desktop's own picker (through its
/// portal), or zenity where there is no portal. With neither, nothing opens
/// and the reason is logged.
#[cfg(target_os = "linux")]
pub async fn pick_folder(title: &str) -> Option<std::path::PathBuf> {
    let folder = rfd::AsyncFileDialog::new().set_title(title).pick_folder().await;
    folder.map(|f| f.path().to_path_buf())
}

/// Let the user choose files to copy to the remote computer. Linux pickers
/// choose files or folders, not both at once, so this asks for files; folders
/// can still be dropped onto the window.
#[cfg(target_os = "linux")]
pub async fn pick_files_to_upload() -> Vec<std::path::PathBuf> {
    rfd::AsyncFileDialog::new()
        .set_title("Choose files to copy to the Pi")
        .pick_files()
        .await
        .map(|files| files.into_iter().map(|f| f.path().to_path_buf()).collect())
        .unwrap_or_default()
}

/// No folder picker on this system yet.
#[cfg(not(target_os = "linux"))]
pub async fn pick_folder(_title: &str) -> Option<std::path::PathBuf> {
    None
}

/// No file picker on this system yet; files can still be dropped onto the window.
#[cfg(not(target_os = "linux"))]
pub async fn pick_files_to_upload() -> Vec<std::path::PathBuf> {
    Vec::new()
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    /// Needs a running password store, so it only runs when asked for:
    /// `cargo test -p mini-remote -- --ignored password_store`
    #[test]
    #[ignore]
    fn password_store_round_trip() {
        let store = super::secret_store();
        assert!(store.available(), "no password store is running");
        store.set("test entry", "secret").unwrap();
        assert_eq!(store.get("test entry").as_deref(), Some("secret"));
        store.delete("test entry");
        assert_eq!(store.get("test entry"), None);
    }
}
