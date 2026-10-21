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

/// The window's AppKit object.
fn ns_window(window: &egui_winit::winit::window::Window) -> Option<objc2::rc::Retained<objc2_app_kit::NSWindow>> {
    use egui_winit::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = window.window_handle().ok()?;
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else { return None };
    // SAFETY: winit hands out a valid NSView for as long as the window lives,
    // and this is only called on the main thread while the window exists.
    let view: &objc2_app_kit::NSView = unsafe { appkit.ns_view.cast().as_ref() };
    view.window()
}

/// A normal window gets an empty toolbar in the "unified" style, like Finder
/// and Notes: the title bar becomes taller, with the window buttons centred
/// in it, so the app's header has room for its title and buttons. In full
/// screen the toolbar is removed, so nothing slides over the remote screen.
pub fn window_chrome(window: &egui_winit::winit::window::Window, fullscreen: bool) {
    use objc2_app_kit::{NSTitlebarSeparatorStyle, NSToolbar, NSWindowToolbarStyle};
    let Some(ns_window) = ns_window(window) else { return };
    let has_toolbar = ns_window.toolbar().is_some();
    if fullscreen {
        if has_toolbar {
            ns_window.setToolbar(None);
        }
        return;
    }
    if has_toolbar {
        return;
    }
    let Some(mtm) = objc2::MainThreadMarker::new() else { return };
    let toolbar = NSToolbar::new(mtm);
    ns_window.setToolbar(Some(&toolbar));
    ns_window.setToolbarStyle(NSWindowToolbarStyle::Unified);
    // The app draws its own line under the header.
    ns_window.setTitlebarSeparatorStyle(NSTitlebarSeparatorStyle::None);
}

/// Debug aid: which view macOS would hand a click at (x, y from the top-left
/// of the window). Shows whether header buttons under the title bar get clicks.
#[cfg(debug_assertions)]
pub fn debug_hit_test(window: &egui_winit::winit::window::Window, x: f64, y: f64) -> String {
    let Some(ns_window) = ns_window(window) else { return "no window".into() };
    let Some(content) = ns_window.contentView() else { return "no content view".into() };
    let Some(frame_view) = (unsafe { content.superview() }) else { return "no frame view".into() };
    let height = ns_window.frame().size.height;
    let point = objc2_foundation::NSPoint::new(x, height - y);
    match frame_view.hitTest(point) {
        Some(view) => view.class().name().to_string_lossy().into_owned(),
        None => "nothing".into(),
    }
}

/// Height of the (transparent) title bar the app draws under, in points.
/// 0 in full screen, where the window buttons are gone. The header is made
/// twice as tall as the middle of the window buttons, so the page title and
/// the header's buttons sit on the same line as them (macOS reports a taller
/// bar for the unified style than where it actually centres the buttons).
pub fn title_bar_height(window: &egui_winit::winit::window::Window) -> f32 {
    use objc2_app_kit::NSWindowButton;
    let Some(ns_window) = ns_window(window) else { return 0.0 };
    let frame = ns_window.frame();
    let content = ns_window.contentLayoutRect();
    let reported = (frame.size.height - content.size.height).max(0.0) as f32;
    if reported == 0.0 {
        return 0.0;
    }
    let Some(close) = ns_window.standardWindowButton(NSWindowButton::CloseButton) else { return reported };
    let rect = close.convertRect_toView(close.bounds(), None);
    let middle_from_top = frame.size.height - (rect.origin.y + rect.size.height / 2.0);
    if middle_from_top <= 0.0 {
        return reported;
    }
    (2.0 * middle_from_top).round() as f32
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
