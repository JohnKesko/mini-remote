//! The Settings page: grouped cards with one setting per row. Changes are
//! saved right away.

use egui::{ComboBox, Response, RichText, Ui};

use super::home::centered_page;
use super::icon;
use super::theme::Palette;
use super::toolbar::quality_options;
use super::widgets::{row_divider, row_hint, section_card, secondary_button, segmented, setting_row, toggle};
use crate::app::App;
use crate::model::{AltMapping, Appearance, KeyboardMode, MetaMapping, RemoteSystem};
use rfb::Quality;

/// Plain-English help for letters that don't arrive on the Pi.
pub const LAYOUT_HINT: &str =
    "If å, ä, ö don't appear on the Pi, set its keyboard layout for screen sharing: add xkb_layout=se to /etc/wayvnc/config.";

const SETTINGS_WIDTH: f32 = 640.0;

/// A drop-down for picture quality, as tall as the other controls.
pub fn quality_combo(ui: &mut Ui, id: &str, value: &mut Quality) -> Response {
    let current = quality_options().iter().find(|(q, _)| *q == *value).map(|(_, l)| *l).unwrap_or("");
    let short = current.split(" (").next().unwrap_or(current);
    ComboBox::from_id_salt(id)
        .selected_text(short)
        .width(160.0)
        .show_ui(ui, |ui| {
            for (q, label) in quality_options() {
                ui.selectable_value(value, q, label);
            }
        })
        .response
}

pub fn page_ui(app: &mut App, ui: &mut Ui) {
    let p = Palette::of(ui.ctx());
    let before = app.settings.clone();
    let mut choose_folder = false;
    let download_folder = app.settings.download_folder();
    let mut check_now = false;
    let checking = app.updater.busy();
    let update_status = update_status_text(&app.updater.status);

    centered_page(ui, "settings-page", SETTINGS_WIDTH, |ui, _| {
        let s = &mut app.settings;
        ui.add_space(-24.0);

        section_card(ui, icon::PALETTE, "Appearance", |ui| {
            setting_row(ui, "Theme", Some("Light or dark, or follow the Mac."), |ui| {
                let options = [(Appearance::System, "System"), (Appearance::Light, "Light"), (Appearance::Dark, "Dark")];
                segmented(ui, &mut s.appearance, &options)
            });
        });

        section_card(ui, icon::KEYBOARD, "Keyboard", |ui| {
            let meta = format!("{} key acts as", crate::platform::meta_key_label());
            setting_row(ui, "Remote computer", Some("For new connections. Automatic recognises a Mac; others get Linux keys."), |ui| {
                segmented(ui, &mut s.default_remote_system, &RemoteSystem::OPTIONS)
            });
            row_divider(ui);
            setting_row(ui, &meta, Some("On Linux and Windows. Control makes copy and paste work as on the Mac."), |ui| {
                segmented(ui, &mut s.meta_key, &[(MetaMapping::Control, "Control"), (MetaMapping::Super, "Windows key")])
            });
            row_divider(ui);
            let alt = format!("{} key acts as", crate::platform::alt_key_label());
            setting_row(ui, &alt, Some("Characters like @ and { still arrive as typed."), |ui| {
                segmented(ui, &mut s.alt_key, &[(AltMapping::Alt, "Alt"), (AltMapping::AltGr, "AltGr")])
            });
            row_divider(ui);
            setting_row(ui, "Keyboard mode", Some("For new connections. Characters works with any layout."), |ui| {
                let options = [(KeyboardMode::Characters, "Characters"), (KeyboardMode::Physical, "Physical keys")];
                segmented(ui, &mut s.keyboard_mode, &options)
            });
            row_divider(ui);
            setting_row(ui, "Esc leaves full screen", Some("Turn off to send Esc to the Pi while in full screen."), |ui| {
                toggle(ui, &mut s.escape_leaves_fullscreen)
            });
            row_divider(ui);
            row_hint(ui, LAYOUT_HINT);
            row_divider(ui);
            row_hint(
                ui,
                &format!(
                    "Mini Remote's own shortcuts: {} full screen (Esc also leaves it), {} menu, {} disconnect. In the Pi's Terminal, paste with Command-Shift-V.",
                    crate::platform::shortcut_label("F"),
                    crate::platform::shortcut_label("S"),
                    crate::platform::shortcut_label("W"),
                ),
            );
        });

        section_card(ui, icon::CLIPBOARD_TEXT, "Clipboard", |ui| {
            setting_row(ui, "Share clipboard with the Pi", Some("Copy on one computer, paste on the other."), |ui| {
                toggle(ui, &mut s.share_clipboard)
            });
        });

        section_card(ui, icon::IMAGE, "Picture", |ui| {
            setting_row(ui, "Picture quality", Some("For new connections."), |ui| {
                quality_combo(ui, "settings-quality", &mut s.default_quality)
            });
            row_divider(ui);
            setting_row(ui, "Smoother video", Some("Use H.264 for new connections. Works best on Pi 4."), |ui| {
                toggle(ui, &mut s.default_video)
            });
        });

        section_card(ui, icon::PLUGS_CONNECTED, "Connection", |ui| {
            setting_row(ui, "Reconnect automatically", Some("Keeps trying if the network drops."), |ui| {
                toggle(ui, &mut s.auto_reconnect)
            });
            row_divider(ui);
            setting_row(ui, "Allow unencrypted login", Some("For new connections. Only for older servers."), |ui| {
                toggle(ui, &mut s.default_allow_unencrypted)
            });
            if s.default_allow_unencrypted {
                ui.label(
                    RichText::new("Others on your network could read the password and see the screen.")
                        .size(12.0)
                        .color(p.warning),
                );
                ui.add_space(12.0);
            }
        });

        section_card(ui, icon::FOLDER_SIMPLE, "Files", |ui| {
            let shown = short_path(&download_folder);
            setting_row(ui, "Save copied files in", Some(&shown), |ui| {
                let r = secondary_button(ui, "Change…");
                if r.clicked() {
                    choose_folder = true;
                }
                r
            });
        });

        section_card(ui, icon::INFO, "About", |ui| {
            let version = format!("Mini Remote {}", update::current_version());
            setting_row(ui, &version, Some(&update_status), |ui| {
                ui.add_enabled_ui(!checking, |ui| {
                    if secondary_button(ui, "Check for updates").clicked() {
                        check_now = true;
                    }
                })
                .response
            });
            row_divider(ui);
            setting_row(ui, "Check for updates at start", Some("Mini Remote always asks before installing."), |ui| {
                toggle(ui, &mut s.check_updates)
            });
            row_divider(ui);
            row_hint(ui, "Free software under the MIT licence. Inter font, SIL Open Font License.");
        });
    });

    if check_now {
        app.check_for_updates();
    }
    if choose_folder {
        app.choose_download_folder();
    }
    if app.settings != before {
        app.save_settings();
    }
}

/// One line about updates for the About section.
fn update_status_text(status: &crate::updater::Status) -> String {
    use crate::updater::Status;
    match status {
        Status::Checking { manual: true } => "Checking for updates…".into(),
        Status::UpToDate => "You have the latest version.".into(),
        Status::Available(release) => format!("Version {} is available.", release.version),
        Status::Downloading { release, .. } => format!("Updating to version {}…", release.version),
        Status::Restarting => "Restarting…".into(),
        Status::Failed { message, .. } => message.clone(),
        Status::Idle | Status::Checking { manual: false } => "Free and open source.".into(),
    }
}

/// A folder path with the home folder written as "~" (shorter, and it keeps
/// the account name out of sight).
fn short_path(path: &std::path::Path) -> String {
    let home = directories::UserDirs::new().map(|d| d.home_dir().to_path_buf());
    match home.and_then(|h| path.strip_prefix(&h).ok().map(|rest| rest.to_path_buf())) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}
