//! The settings window.

use egui::{Align, ComboBox, Id, Layout, RichText};

use super::theme::Palette;
use super::toolbar::quality_options;
use super::widgets::{floating, hint_text, primary_button, section_title, segmented, toggle_row};
use crate::app::App;
use crate::model::{AltMapping, Appearance, KeyboardMode, MetaMapping};

/// Plain-English help for letters that don't arrive on the Pi.
pub const LAYOUT_HINT: &str = "If letters like å, ä or ö don't appear on the Pi, its screen sharing needs to know your keyboard layout: add a line such as xkb_layout=se (se for Swedish, de for German, fr for French) to the file /etc/wayvnc/config on the Pi and restart it.";

pub fn show(app: &mut App, ctx: &egui::Context) {
    if !app.settings_open {
        return;
    }
    let p = Palette::of(ctx);
    let before = app.settings.clone();
    let mut close = false;
    let s = &mut app.settings;

    let modal = egui::Modal::new(Id::new("settings")).frame(floating(&p)).show(ctx, |ui| {
        ui.set_width(480.0);
        ui.label(RichText::new("Settings").heading().strong());
        ui.add_space(4.0);
        egui::ScrollArea::vertical().max_height((ctx.content_rect().height() - 200.0).max(240.0)).show(ui, |ui| {
            ui.set_width(ui.available_width() - 12.0);
            section_title(ui, "Appearance");
            segmented(ui, &mut s.appearance, &[(Appearance::System, "Same as system"), (Appearance::Light, "Light"), (Appearance::Dark, "Dark")]);

            section_title(ui, "Keyboard");
            ui.horizontal(|ui| {
                ui.label(format!("{} key sends", crate::platform::meta_key_label()));
                segmented(ui, &mut s.meta_key, &[(MetaMapping::Control, "Control"), (MetaMapping::Super, "Windows key")]);
            });
            if cfg!(target_os = "macos") {
                hint_text(ui, "With Control, ⌘C, ⌘V, ⌘X, ⌘Z and ⌘S work on the Pi just like on your Mac.");
            }
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.label(format!("{} key sends", crate::platform::alt_key_label()));
                segmented(ui, &mut s.alt_key, &[(AltMapping::Alt, "Alt"), (AltMapping::AltGr, "AltGr")]);
            });
            hint_text(ui, "Characters such as @ and { that you type with this key still arrive as typed.");
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.label("Keyboard for new connections");
                segmented(ui, &mut s.keyboard_mode, &[(KeyboardMode::Characters, "Characters"), (KeyboardMode::Physical, "Physical keys")]);
            });
            ui.add_space(2.0);
            hint_text(ui, LAYOUT_HINT);
            hint_text(ui, &format!(
                "Shortcuts for Mini Remote itself: {} full screen, {} close tab, {} / {} switch tabs. Everything else goes to the Pi.",
                crate::platform::shortcut_label("F"),
                crate::platform::shortcut_label("W"),
                crate::platform::shortcut_label("Left Arrow"),
                crate::platform::shortcut_label("Right Arrow"),
            ));

            section_title(ui, "Clipboard");
            toggle_row(ui, &mut s.share_clipboard, "Share the clipboard with the Pi", Some("Each connection can also turn this off."));

            section_title(ui, "Picture");
            ui.horizontal(|ui| {
                ui.label("Quality for new connections");
                let current = quality_options().iter().find(|(q, _)| *q == s.default_quality).map(|(_, l)| *l).unwrap_or("");
                ComboBox::from_id_salt("settings-quality").selected_text(current).width(240.0).show_ui(ui, |ui| {
                    for (q, label) in quality_options() {
                        ui.selectable_value(&mut s.default_quality, q, label);
                    }
                });
            });

            section_title(ui, "Connection");
            toggle_row(ui, &mut s.auto_reconnect, "Reconnect automatically", Some("If the network drops, Mini Remote keeps trying and shows the last picture meanwhile."));
        });
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("Mini Remote {} · free and open source (MIT licence)", env!("CARGO_PKG_VERSION")))
                    .size(12.0)
                    .color(p.text_weak),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if primary_button(ui, "Done").clicked() {
                    close = true;
                }
            });
        });
    });
    if app.settings != before {
        app.save_settings();
    }
    if close || modal.should_close() {
        app.settings_open = false;
    }
}
