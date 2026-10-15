//! The connection editor page ("New connection" and editing a saved one).
//! Built only from the shared row helpers, so labels, inputs and controls
//! line up in fixed columns.

use egui::{Align, ComboBox, Frame, Layout, Margin, Response, Ui};

use super::home::{PAGE_MARGIN, page};
use super::icon;
use super::settings::quality_combo;
use super::theme::Palette;
use super::widgets::{
    form_extra, form_hint, form_row, icon_button, primary_button, row_divider, row_hint, section_card,
    secondary_button, segmented, setting_row, text_field, toggle, warning_box,
};
use crate::app::{App, EditorState};
use crate::model::{KeyboardMode, Scaling, SshLogin};

const EDITOR_WIDTH: f32 = 640.0;

enum Action {
    Cancel,
    Save,
    SaveAndConnect,
}

pub fn page_ui(app: &mut App, ui: &mut Ui) {
    let store_name = crate::platform::secret_store_name();
    let store_available = app.secrets.available();
    let p = Palette::of(ui.ctx());
    let Some(editor) = &mut app.editor else {
        app.show_page(crate::app::Page::Saved);
        return;
    };
    let mut action = None;

    // Buttons stay visible at the bottom while the form scrolls, lined up
    // with the right edge of the form.
    egui::Panel::bottom("editor-buttons")
        .frame(Frame::new().fill(p.bg).inner_margin(Margin::symmetric(0, 12)))
        .show(ui, |ui| {
            let width = (ui.available_width() - 2.0 * PAGE_MARGIN).clamp(240.0, EDITOR_WIDTH);
            ui.horizontal(|ui| {
                ui.add_space(PAGE_MARGIN);
                ui.allocate_ui_with_layout(egui::vec2(width, 32.0), Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if primary_button(ui, "Save and connect").clicked() {
                        action = Some(Action::SaveAndConnect);
                    }
                    if secondary_button(ui, "Save").clicked() {
                        action = Some(Action::Save);
                    }
                    if secondary_button(ui, "Cancel").clicked() {
                        action = Some(Action::Cancel);
                    }
                    if let Some(err) = &editor.error {
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| super::widgets::error_line(ui, err));
                    }
                });
            });
        });

    egui::CentralPanel::no_frame().show(ui, |ui| {
        page(ui, "editor-page", EDITOR_WIDTH, |ui, _| {
            ui.add_space(-24.0);
            general(ui, editor, store_name, store_available);
            picture(ui, editor);
            control(ui, editor);
            security(ui, editor);
            ssh_section(ui, editor, store_name);
        });
    });

    if ui.input(|i| i.key_pressed(egui::Key::Escape)) && action.is_none() {
        action = Some(Action::Cancel);
    }
    match action {
        Some(Action::Cancel) => app.cancel_editor(),
        Some(Action::Save) => app.save_editor(false),
        Some(Action::SaveAndConnect) => app.save_editor(true),
        None => {}
    }
}

fn general(ui: &mut Ui, e: &mut EditorState, store_name: &str, store_available: bool) {
    section_card(ui, icon::MONITOR, "Computer", |ui| {
        form_row(ui, "Name", |ui, w| ui.add(text_field(&mut e.conn.name, "For example: Living room Pi").desired_width(w)));
        row_divider(ui);
        form_row(ui, "Address", |ui, w| {
            ui.add(text_field(&mut e.conn.host, "raspberrypi.local or 192.168.1.20").desired_width(w))
        });
        row_divider(ui);
        form_row(ui, "Port", |ui, w| ui.add(text_field(&mut e.port_text, "5900").desired_width(w)));
    });

    section_card(ui, icon::LOCK_SIMPLE, "Login", |ui| {
        form_row(ui, "Username", |ui, w| ui.add(text_field(&mut e.conn.username, "pi").desired_width(w)));
        form_hint(ui, "Leave it empty if the Pi only asks for a password.");
        row_divider(ui);
        form_row(ui, "Password", |ui, w| {
            let hint = if e.has_saved_password && !e.password_changed {
                format!("Saved in {store_name} – type to change")
            } else {
                String::new()
            };
            password_field(ui, w, &mut e.password, &mut e.password_changed, &mut e.show_password, &hint)
        });
        if store_available {
            ui.add_space(-6.0);
            form_extra(ui, |ui| ui.checkbox(&mut e.conn.remember_password, format!("Remember password in {store_name}")));
        }
    });
}

/// A password input of width `w`, with a show/hide button after it.
fn password_field(ui: &mut Ui, w: f32, text: &mut String, changed: &mut bool, show: &mut bool, hint: &str) -> Response {
    let response = ui.add(text_field(text, hint).password(!*show).desired_width(w));
    if response.changed() {
        *changed = true;
    }
    let eye = if *show { icon::EYE_SLASH } else { icon::EYE };
    if icon_button(ui, eye, if *show { "Hide password" } else { "Show password" }).clicked() {
        *show = !*show;
    }
    response
}

fn picture(ui: &mut Ui, e: &mut EditorState) {
    section_card(ui, icon::IMAGE, "Picture", |ui| {
        setting_row(ui, "Picture quality", Some("Automatic adapts to your network."), |ui| {
            quality_combo(ui, "editor-quality", &mut e.conn.quality)
        });
        row_divider(ui);
        setting_row(ui, "Smoother video (H.264)", Some("Works best on Pi 4."), |ui| toggle(ui, &mut e.conn.video));
        row_divider(ui);
        setting_row(ui, "Size", None, |ui| {
            segmented(ui, &mut e.conn.scaling, &[(Scaling::Fit, "Fit to window"), (Scaling::Actual, "Actual size")])
        });
        row_divider(ui);
        setting_row(
            ui,
            "Resize the Pi's screen to fit the window",
            Some("Only works when no monitor is plugged into the Pi."),
            |ui| toggle(ui, &mut e.conn.resize_remote),
        );
    });
}

fn control(ui: &mut Ui, e: &mut EditorState) {
    section_card(ui, icon::MOUSE, "Control", |ui| {
        setting_row(ui, "View only", Some("Watch without controlling mouse and keyboard."), |ui| {
            toggle(ui, &mut e.conn.view_only)
        });
        row_divider(ui);
        setting_row(ui, "Share clipboard", Some("Copy on one computer, paste on the other."), |ui| {
            toggle(ui, &mut e.conn.share_clipboard)
        });
        row_divider(ui);
        let hint = match e.conn.keyboard_mode {
            KeyboardMode::Characters => "Sends the letters you type (recommended).",
            KeyboardMode::Physical => "Sends key positions; the Pi's layout decides.",
        };
        setting_row(ui, "Keyboard", Some(hint), |ui| {
            let options = [(KeyboardMode::Characters, "Characters"), (KeyboardMode::Physical, "Physical keys")];
            segmented(ui, &mut e.conn.keyboard_mode, &options)
        });
    });
}

fn security(ui: &mut Ui, e: &mut EditorState) {
    section_card(ui, icon::SHIELD_CHECK, "Security", |ui| {
        setting_row(ui, "Allow unencrypted login", Some("Only needed for older screen sharing servers."), |ui| {
            toggle(ui, &mut e.conn.allow_unencrypted)
        });
        if e.conn.allow_unencrypted && !e.conn.ssh.enabled {
            warning_box(
                ui,
                "Without encryption, others on your network could read your password and see your screen. Use it only at home, or with SSH below.",
            );
            ui.add_space(12.0);
        }
    });
}

fn ssh_section(ui: &mut Ui, e: &mut EditorState, store_name: &str) {
    section_card(ui, icon::KEY, "Secure remote login (SSH)", |ui| {
        setting_row(
            ui,
            "Connect through SSH",
            Some("For use away from home. Turn on SSH in Raspberry Pi Configuration > Interfaces."),
            |ui| toggle(ui, &mut e.conn.ssh.enabled),
        );
        row_divider(ui);
        row_hint(ui, "These details are also used for copying files.");
        row_divider(ui);
        form_row(ui, "SSH address", |ui, w| {
            let host = e.conn.host.trim();
            let same = if host.is_empty() { "Same as above".to_string() } else { host.to_string() };
            ui.add(text_field(&mut e.conn.ssh.host, &same).desired_width(w))
        });
        row_divider(ui);
        form_row(ui, "SSH port", |ui, w| ui.add(text_field(&mut e.ssh_port_text, "22").desired_width(w)));
        row_divider(ui);
        form_row(ui, "SSH username", |ui, w| {
            let user = e.conn.username.trim();
            let same = if user.is_empty() { "pi".to_string() } else { user.to_string() };
            ui.add(text_field(&mut e.conn.ssh.username, &same).desired_width(w))
        });
        row_divider(ui);

        let kind = login_kind(&e.conn.ssh.login);
        let labels = ["Same password as above", "A separate password", "A key file", "The SSH agent"];
        let mut new_kind = kind;
        form_row(ui, "Log in with", |ui, w| {
            ComboBox::from_id_salt("editor-ssh-login")
                .selected_text(labels[kind])
                .width(w)
                .show_ui(ui, |ui| {
                    for (i, label) in labels.iter().enumerate() {
                        ui.selectable_value(&mut new_kind, i, *label);
                    }
                })
                .response
        });
        if new_kind != kind {
            e.conn.ssh.login = match new_kind {
                0 => SshLogin::SamePassword,
                1 => SshLogin::Password,
                2 => SshLogin::KeyFile(
                    ssh::default_key_files().first().map(|p| p.display().to_string()).unwrap_or_default(),
                ),
                _ => SshLogin::Agent,
            };
        }

        let saved_hint = if e.has_saved_ssh_password && !e.ssh_password_changed {
            format!("Saved in {store_name} – type to change")
        } else {
            String::new()
        };
        match &mut e.conn.ssh.login {
            SshLogin::SamePassword => form_hint(ui, "Uses the username and password from Login above."),
            SshLogin::Agent => form_hint(ui, "Uses the keys loaded in your SSH agent."),
            SshLogin::Password => {
                row_divider(ui);
                form_row(ui, "SSH password", |ui, w| {
                    password_field(
                        ui,
                        w,
                        &mut e.ssh_password,
                        &mut e.ssh_password_changed,
                        &mut e.show_ssh_password,
                        &saved_hint,
                    )
                });
            }
            SshLogin::KeyFile(path) => {
                row_divider(ui);
                form_row(ui, "Key file", |ui, w| {
                    let response = ui.add(text_field(path, "~/.ssh/id_ed25519").desired_width(w));
                    let keys = ssh::default_key_files();
                    ui.add_enabled_ui(!keys.is_empty(), |ui| {
                        ui.menu_button(super::theme::ic(icon::CARET_DOWN), |ui| {
                            for key in keys {
                                let shown = key.display().to_string();
                                if ui.button(&shown).clicked() {
                                    *path = shown;
                                }
                            }
                        })
                        .response
                        .on_hover_text("Pick one of your keys");
                    });
                    response
                });
                row_divider(ui);
                form_row(ui, "Key passphrase", |ui, w| {
                    password_field(
                        ui,
                        w,
                        &mut e.ssh_password,
                        &mut e.ssh_password_changed,
                        &mut e.show_ssh_password,
                        &saved_hint,
                    )
                });
                form_hint(ui, "Leave it empty if the key has no passphrase.");
            }
        }
    });
}

fn login_kind(login: &SshLogin) -> usize {
    match login {
        SshLogin::SamePassword => 0,
        SshLogin::Password => 1,
        SshLogin::KeyFile(_) => 2,
        SshLogin::Agent => 3,
    }
}
