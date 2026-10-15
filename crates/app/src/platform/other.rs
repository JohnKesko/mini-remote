//! Windows and Linux. Only the basics for now; passwords are not remembered
//! until a password store for these systems is plugged in here.

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

/// Placeholder until the system password store is wired in.
struct NoStore;

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

pub fn secret_store() -> Box<dyn SecretStore> {
    Box::new(NoStore)
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
    attrs
}

/// The native title bar is kept here, so the app header has no window buttons.
pub fn title_bar_height(_window: &egui_winit::winit::window::Window) -> f32 {
    0.0
}

pub const WINDOW_BUTTONS_WIDTH: f32 = 0.0;

pub fn physical_key(key: PhysicalKey) -> PhysicalKey {
    key
}

/// No folder picker on this system yet.
pub async fn pick_folder(_title: &str) -> Option<std::path::PathBuf> {
    None
}

/// No file picker on this system yet; files can still be dropped onto the window.
pub async fn pick_files_to_upload() -> Vec<std::path::PathBuf> {
    Vec::new()
}
