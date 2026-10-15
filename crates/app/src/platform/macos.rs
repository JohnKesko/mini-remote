//! macOS: Keychain passwords, Finder, and Mac keyboard defaults.

use std::io;
use std::path::Path;
use std::process::Command;

use egui_winit::winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use egui_winit::winit::window::WindowAttributes;

use super::{Folder, SecretStore};
use crate::model::MetaMapping;

pub const SECRET_STORE_NAME: &str = "Keychain";
pub const REVEAL_LABEL: &str = "Show in Finder";
pub const META_KEY_LABEL: &str = "⌘ Command";
pub const ALT_KEY_LABEL: &str = "Option";
/// Command sends Control, so Command+C, V, X, Z, A and S work as on the Mac.
pub const DEFAULT_META: MetaMapping = MetaMapping::Control;
pub const CAPS_LOCK_REPORTS_STATE: bool = true;
pub(super) const SCREENSHOT_FOLDER: Folder = Folder::Desktop;
pub const SHORTCUT_PREFIX: &str = "Control-Command-";

pub fn is_app_shortcut(mods: ModifiersState) -> bool {
    mods.super_key() && mods.control_key() && !mods.alt_key()
}

/// Service name the passwords are filed under in the Keychain.
const KEYCHAIN_SERVICE: &str = "Mini Remote";

struct Keychain;

impl SecretStore for Keychain {
    fn get(&self, key: &str) -> Option<String> {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, key).ok()?;
        match entry.get_password() {
            Ok(secret) => Some(secret),
            Err(keyring::Error::NoEntry) => None,
            Err(err) => {
                log::warn!("could not read a password from the Keychain: {err}");
                None
            }
        }
    }

    fn set(&self, key: &str, secret: &str) -> Result<(), String> {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, key).map_err(|e| e.to_string())?;
        entry.set_password(secret).map_err(|e| e.to_string())
    }

    fn delete(&self, key: &str) {
        if let Ok(entry) = keyring::Entry::new(KEYCHAIN_SERVICE, key) {
            match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => {}
                Err(err) => log::warn!("could not remove a password from the Keychain: {err}"),
            }
        }
    }

    fn available(&self) -> bool {
        true
    }
}

pub fn secret_store() -> Box<dyn SecretStore> {
    Box::new(Keychain)
}

pub fn reveal(path: &Path) -> io::Result<()> {
    Command::new("open").arg("-R").arg(path).spawn().map(|_| ())
}

pub fn open_url(url: &str) -> io::Result<()> {
    Command::new("open").arg(url).spawn().map(|_| ())
}

pub fn window_attributes(attrs: WindowAttributes) -> WindowAttributes {
    use egui_winit::winit::platform::macos::{OptionAsAlt, WindowAttributesExtMacOS};
    attrs
        // Keep Option producing characters like @ and { on European keyboards;
        // the keyboard code decides when Option should act as Alt instead.
        .with_option_as_alt(OptionAsAlt::None)
        // The app draws its own header in its own colours; the window buttons
        // stay where they are, on top of it.
        .with_titlebar_transparent(true)
        .with_fullsize_content_view(true)
        .with_title_hidden(true)
}

/// Height of the (transparent) title bar the app draws under, in points.
/// 0 in full screen, where the window buttons are gone.
pub fn title_bar_height(window: &egui_winit::winit::window::Window) -> f32 {
    use egui_winit::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = window.window_handle() else { return 0.0 };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else { return 0.0 };
    // SAFETY: winit hands out a valid NSView for as long as the window lives,
    // and this is only called on the main thread while the window exists.
    let view: &objc2_app_kit::NSView = unsafe { appkit.ns_view.cast().as_ref() };
    let Some(ns_window) = view.window() else { return 0.0 };
    let frame = ns_window.frame();
    let content = ns_window.contentLayoutRect();
    (frame.size.height - content.size.height).max(0.0) as f32
}

/// Room to leave on the left of the header for the window buttons.
pub const WINDOW_BUTTONS_WIDTH: f32 = 78.0;

/// Mac ISO (European) keyboards report the key left of 1 and the key next to
/// left Shift the other way round compared to PC keyboards. Swap them back so
/// the physical keyboard mode puts them in the right place.
pub fn physical_key(key: PhysicalKey) -> PhysicalKey {
    match key {
        PhysicalKey::Code(KeyCode::Backquote) if is_iso_keyboard() => PhysicalKey::Code(KeyCode::IntlBackslash),
        PhysicalKey::Code(KeyCode::IntlBackslash) if is_iso_keyboard() => PhysicalKey::Code(KeyCode::Backquote),
        other => other,
    }
}

fn is_iso_keyboard() -> bool {
    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        fn LMGetKbdType() -> u8;
        fn KBGetLayoutType(keyboard_type: i16) -> u32;
    }
    const KEYBOARD_ISO: u32 = u32::from_be_bytes(*b"ISO ");
    // SAFETY: plain system queries without pointers or lifetimes involved.
    unsafe { KBGetLayoutType(i16::from(LMGetKbdType())) == KEYBOARD_ISO }
}

/// Let the user choose a folder.
pub async fn pick_folder(title: &str) -> Option<std::path::PathBuf> {
    rfd::AsyncFileDialog::new().set_title(title).pick_folder().await.map(|f| f.path().to_path_buf())
}

/// Let the user choose files or folders to copy to the remote computer.
pub async fn pick_files_to_upload() -> Vec<std::path::PathBuf> {
    rfd::AsyncFileDialog::new()
        .set_title("Choose files to copy to the Pi")
        .pick_files_or_folders()
        .await
        .map(|files| files.into_iter().map(|f| f.path().to_path_buf()).collect())
        .unwrap_or_default()
}
