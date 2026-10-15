//! Everything that depends on the operating system lives here, so other
//! systems can be added by writing one more file next to `macos.rs`.

use std::path::{Path, PathBuf};

use crate::model::{AltMapping, MetaMapping};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as imp;

#[cfg(not(target_os = "macos"))]
mod other;
#[cfg(not(target_os = "macos"))]
use other as imp;

/// Where passwords are kept. Implementations must never write to plain files.
pub trait SecretStore {
    fn get(&self, key: &str) -> Option<String>;
    fn set(&self, key: &str, secret: &str) -> Result<(), String>;
    fn delete(&self, key: &str);
    /// Can this store keep passwords at all on this computer?
    fn available(&self) -> bool;
}

/// The system password store (the Keychain on a Mac).
pub fn secret_store() -> Box<dyn SecretStore> {
    imp::secret_store()
}

/// Passwords kept only while the app runs (used with `MINI_REMOTE_CONFIG_DIR`).
pub fn memory_store() -> Box<dyn SecretStore> {
    Box::new(MemoryStore::default())
}

#[derive(Default)]
struct MemoryStore {
    secrets: parking_lot::Mutex<std::collections::HashMap<String, String>>,
}

impl SecretStore for MemoryStore {
    fn get(&self, key: &str) -> Option<String> {
        self.secrets.lock().get(key).cloned()
    }

    fn set(&self, key: &str, secret: &str) -> Result<(), String> {
        self.secrets.lock().insert(key.to_string(), secret.to_string());
        Ok(())
    }

    fn delete(&self, key: &str) {
        self.secrets.lock().remove(key);
    }

    fn available(&self) -> bool {
        true
    }
}

/// Name of the password store as users know it, e.g. "Keychain".
pub fn secret_store_name() -> &'static str {
    imp::SECRET_STORE_NAME
}

/// What the "meta" key (Command on a Mac) does on the remote computer by default.
pub fn default_meta_mapping() -> MetaMapping {
    imp::DEFAULT_META
}

/// What the Alt key (Option on a Mac) does on the remote computer by default.
pub fn default_alt_mapping() -> AltMapping {
    AltMapping::Alt
}

/// Labels for the local modifier keys, e.g. "⌘ Command".
pub fn meta_key_label() -> &'static str {
    imp::META_KEY_LABEL
}

pub fn alt_key_label() -> &'static str {
    imp::ALT_KEY_LABEL
}

/// Modifier keys held for the app's own shortcuts (Control+Command on a Mac),
/// chosen so they never clash with shortcuts meant for the remote computer.
pub fn is_app_shortcut(mods: egui_winit::winit::keyboard::ModifiersState) -> bool {
    imp::is_app_shortcut(mods)
}

/// Correct the reported key position where the system gets it wrong.
pub fn physical_key(key: egui_winit::winit::keyboard::PhysicalKey) -> egui_winit::winit::keyboard::PhysicalKey {
    imp::physical_key(key)
}

/// How an app shortcut is written, e.g. "Control-Command-F".
pub fn shortcut_label(key: &str) -> String {
    format!("{}{key}", imp::SHORTCUT_PREFIX)
}

/// Caps Lock arrives as "on"/"off" instead of press/release on some systems.
/// When true, every Caps Lock event is sent as a full press and release.
pub fn caps_lock_reports_state() -> bool {
    imp::CAPS_LOCK_REPORTS_STATE
}

/// Folder for screenshots (Desktop on a Mac, like the system's own screenshots).
pub fn screenshot_dir() -> PathBuf {
    let dirs = directories::UserDirs::new();
    let preferred = dirs.as_ref().and_then(|d| match imp::SCREENSHOT_FOLDER {
        Folder::Desktop => d.desktop_dir().map(Path::to_path_buf),
        Folder::Pictures => d.picture_dir().map(Path::to_path_buf),
    });
    preferred
        .or_else(|| dirs.as_ref().and_then(|d| d.picture_dir().map(Path::to_path_buf)))
        .or_else(|| dirs.as_ref().map(|d| d.home_dir().to_path_buf()))
        .unwrap_or_else(std::env::temp_dir)
}

/// Folder for files copied from the remote computer.
pub fn download_dir() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|d| d.download_dir().map(Path::to_path_buf))
        .unwrap_or_else(std::env::temp_dir)
}

/// Show a file in the file manager (Finder on a Mac).
pub fn reveal_in_file_manager(path: &Path) {
    if let Err(err) = imp::reveal(path) {
        log::warn!("could not show {}: {err}", path.display());
    }
}

/// Button label for [`reveal_in_file_manager`], e.g. "Show in Finder".
pub fn reveal_label() -> &'static str {
    imp::REVEAL_LABEL
}

/// Let the user choose files or folders to copy to the remote computer.
/// Can be called from any thread.
pub async fn pick_files_to_upload() -> Vec<PathBuf> {
    imp::pick_files_to_upload().await
}

/// Let the user choose a folder. Can be called from any thread.
pub async fn pick_folder(title: &str) -> Option<PathBuf> {
    imp::pick_folder(title).await
}

/// Open a web address in the default browser.
#[allow(dead_code)]
pub fn open_url(url: &str) {
    if let Err(err) = imp::open_url(url) {
        log::warn!("could not open {url}: {err}");
    }
}

/// Height of the title bar the app draws its header under (0 when the system
/// draws its own title bar, or in full screen).
pub fn title_bar_height(window: &egui_winit::winit::window::Window) -> f32 {
    imp::title_bar_height(window)
}

/// Room to leave on the left of the header for the window buttons.
pub fn window_buttons_width() -> f32 {
    imp::WINDOW_BUTTONS_WIDTH
}

/// Adjust the main window before it is created (title bar style and so on).
pub fn window_attributes(
    attrs: egui_winit::winit::window::WindowAttributes,
) -> egui_winit::winit::window::WindowAttributes {
    imp::window_attributes(attrs)
}

#[allow(dead_code)]
pub(crate) enum Folder {
    Desktop,
    Pictures,
}
