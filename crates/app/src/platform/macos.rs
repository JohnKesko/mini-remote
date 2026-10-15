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
    // Keep Option producing characters like @ and { on European keyboards; the
    // keyboard code decides when Option should act as Alt instead.
    attrs.with_option_as_alt(OptionAsAlt::None)
}

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

/// Let the user choose files or folders to copy to the remote computer.
pub async fn pick_files_to_upload() -> Vec<std::path::PathBuf> {
    rfd::AsyncFileDialog::new()
        .set_title("Choose files to copy to the Pi")
        .pick_files_or_folders()
        .await
        .map(|files| files.into_iter().map(|f| f.path().to_path_buf()).collect())
        .unwrap_or_default()
}
