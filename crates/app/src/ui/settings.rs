//! The Settings page: grouped cards with one setting per row. Changes are
//! saved right away.

use egui::{ComboBox, Response, RichText, Ui};

use super::home::page;
use super::icon;
use super::theme::{Palette, medium};
use super::toolbar::quality_options;
use super::widgets::{row_divider, row_hint, section_card, secondary_button, segmented, setting_row, toggle};
use crate::app::App;
use crate::model::{AltMapping, Appearance, KeyboardMode, MetaMapping};
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

    page(ui, "settings-page", SETTINGS_WIDTH, |ui, _| {
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
            setting_row(ui, &meta, Some("Control makes copy, paste and undo work as on the Mac."), |ui| {
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
            row_hint(ui, LAYOUT_HINT);
            row_divider(ui);
            row_hint(
                ui,
                &format!(
                    "Mini Remote's own shortcuts: {} full screen, {} menu, {} disconnect.",
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
            let shown = download_folder.display().to_string();
            setting_row(ui, "Save copied files in", Some(&shown), |ui| {
                let r = secondary_button(ui, "Change…");
                if r.clicked() {
                    choose_folder = true;
                }
                r
            });
        });

        section_card(ui, icon::INFO, "About", |ui| {
            ui.add_space(12.0);
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(RichText::new(format!("Mini Remote {}", env!("CARGO_PKG_VERSION"))).font(medium(14.0)));
            ui.label(RichText::new("Free software under the MIT licence.").size(12.5).color(p.text_weak));
            ui.label(RichText::new("Inter font, SIL Open Font License.").size(12.5).color(p.text_weak));
            ui.add_space(12.0);
        });
    });

    if choose_folder {
        app.choose_download_folder();
    }
    if app.settings != before {
        app.save_settings();
    }
}
