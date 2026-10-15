//! The connection editor dialog.

use egui::{Align, ComboBox, Id, Layout, RichText, Ui};

use super::theme::Palette;
use super::toolbar::quality_options;
use super::widgets::{
    field_label, floating, hint_text, icon_button, primary_button, secondary_button, section_title, segmented,
    text_field, toggle_row, warning_box,
};
use super::icon;
use crate::app::{App, EditorState};
use crate::model::{KeyboardMode, Scaling, SshLogin};

enum Action {
    Cancel,
    Save,
    SaveAndConnect,
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    let store_name = crate::platform::secret_store_name();
    let store_available = app.secrets.available();
    let Some(editor) = &mut app.editor else { return };
    let p = Palette::of(ctx);
    let mut action = None;
    let max_height = (ctx.content_rect().height() - 230.0).max(240.0);

    let modal = egui::Modal::new(Id::new("connection-editor")).frame(floating(&p)).show(ctx, |ui| {
        ui.set_width(500.0);
        let title = if editor.is_new { "Add connection" } else { "Edit connection" };
        ui.label(RichText::new(title).heading().strong());
        ui.add_space(8.0);

        egui::ScrollArea::vertical().max_height(max_height).auto_shrink([false, true]).show(ui, |ui| {
            ui.set_width(ui.available_width() - 12.0);
            general(ui, editor, store_name, store_available);
            picture(ui, editor);
            control(ui, editor);
            security(ui, editor);
            ssh_section(ui, editor, &p, store_name);
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon::FOLDER_SIMPLE).color(p.text_weak));
                hint_text(ui, "Copy files to and from the Pi with the Files button in the toolbar. It uses the secure remote login above.");
            });
        });

        if let Some(err) = &editor.error {
            ui.add_space(6.0);
            ui.label(RichText::new(format!("{}  {err}", icon::WARNING_CIRCLE)).color(p.danger));
        }
        ui.add_space(12.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if primary_button(ui, "Save and connect").clicked() {
                action = Some(Action::SaveAndConnect);
            }
            if secondary_button(ui, "Save").clicked() {
                action = Some(Action::Save);
            }
            if secondary_button(ui, "Cancel").clicked() {
                action = Some(Action::Cancel);
            }
        });
    });
    if modal.should_close() && action.is_none() {
        action = Some(Action::Cancel);
    }
    match action {
        Some(Action::Cancel) => app.editor = None,
        Some(Action::Save) => app.save_editor(false),
        Some(Action::SaveAndConnect) => app.save_editor(true),
        None => {}
    }
}

fn general(ui: &mut Ui, e: &mut EditorState, store_name: &str, store_available: bool) {
    field_label(ui, "Name");
    ui.add(text_field(&mut e.conn.name, "For example: Living room Pi"));
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_width(ui.available_width() - 100.0);
            field_label(ui, "Address");
            ui.add(text_field(&mut e.conn.host, "raspberrypi.local or 192.168.1.20"));
        });
        ui.vertical(|ui| {
            field_label(ui, "Port");
            ui.add(text_field(&mut e.port_text, "5900").desired_width(88.0));
        });
    });

    section_title(ui, "Login");
    field_label(ui, "Username");
    ui.add(text_field(&mut e.conn.username, "pi"));
    hint_text(ui, "The Pi's username. Leave it empty if the Pi only asks for a password.");
    ui.add_space(4.0);
    field_label(ui, "Screen sharing password");
    let hint = if e.has_saved_password && !e.password_changed {
        format!("Saved in {store_name} – type to change")
    } else {
        String::new()
    };
    ui.horizontal(|ui| {
        let width = ui.available_width() - 40.0;
        let r = ui.add(text_field(&mut e.password, &hint).password(!e.show_password).desired_width(width));
        if r.changed() {
            e.password_changed = true;
        }
        let eye = if e.show_password { icon::EYE_SLASH } else { icon::EYE };
        if icon_button(ui, eye, if e.show_password { "Hide password" } else { "Show password" }).clicked() {
            e.show_password = !e.show_password;
        }
    });
    if store_available {
        ui.checkbox(&mut e.conn.remember_password, format!("Remember password in {store_name}"));
    }
}

fn picture(ui: &mut Ui, e: &mut EditorState) {
    section_title(ui, "Picture");
    ui.horizontal(|ui| {
        ui.label("Picture quality");
        let current = quality_options().iter().find(|(q, _)| *q == e.conn.quality).map(|(_, l)| *l).unwrap_or("");
        ComboBox::from_id_salt("editor-quality").selected_text(current).width(260.0).show_ui(ui, |ui| {
            for (q, label) in quality_options() {
                ui.selectable_value(&mut e.conn.quality, q, label);
            }
        });
    });
    ui.add_space(2.0);
    toggle_row(ui, &mut e.conn.video, "Smoother video (H.264)", Some("Works best on Pi 4. Good for videos and moving pictures."));
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.label("Size");
        segmented(ui, &mut e.conn.scaling, &[(Scaling::Fit, "Fit to window"), (Scaling::Actual, "Actual size")]);
    });
    ui.add_space(2.0);
    toggle_row(
        ui,
        &mut e.conn.resize_remote,
        "Resize the Pi's screen to fit the window",
        Some("Only works when no monitor is plugged into the Pi."),
    );
}

fn control(ui: &mut Ui, e: &mut EditorState) {
    section_title(ui, "Control");
    toggle_row(ui, &mut e.conn.view_only, "View only", Some("Watch the Pi's screen without controlling its mouse and keyboard."));
    toggle_row(ui, &mut e.conn.share_clipboard, "Share clipboard", Some("Text you copy on one computer can be pasted on the other."));
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.label("Keyboard");
        segmented(
            ui,
            &mut e.conn.keyboard_mode,
            &[(KeyboardMode::Characters, "Characters"), (KeyboardMode::Physical, "Physical keys")],
        );
    });
    hint_text(
        ui,
        match e.conn.keyboard_mode {
            KeyboardMode::Characters => "Sends the letters you type. Works with any keyboard layout (recommended).",
            KeyboardMode::Physical => "Sends which keys you press; the Pi's own keyboard layout decides the letters. Good for games.",
        },
    );
}

fn security(ui: &mut Ui, e: &mut EditorState) {
    section_title(ui, "Security");
    toggle_row(
        ui,
        &mut e.conn.allow_unencrypted,
        "Allow unencrypted login",
        Some("Only needed for older screen sharing servers."),
    );
    if e.conn.allow_unencrypted && !e.conn.ssh.enabled {
        warning_box(
            ui,
            "Without encryption, others on your network could read your password and see your screen. Only use this at home, or together with secure remote login (SSH) below.",
        );
    }
}

fn ssh_section(ui: &mut Ui, e: &mut EditorState, p: &Palette, store_name: &str) {
    ui.add_space(6.0);
    egui::CollapsingHeader::new(RichText::new("Secure remote login (SSH)").strong().size(13.0).color(p.text_weak))
        .id_salt("editor-ssh")
        .default_open(e.conn.ssh.enabled)
        .show(ui, |ui| {
            toggle_row(
                ui,
                &mut e.conn.ssh.enabled,
                "Connect through SSH (for use away from home)",
                Some("Everything goes through the Pi's encrypted remote login. Turn on SSH in Raspberry Pi Configuration > Interfaces."),
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width() - 100.0);
                    field_label(ui, "SSH address");
                    let same = if e.conn.host.trim().is_empty() { "Same as above".to_string() } else { e.conn.host.trim().to_string() };
                    ui.add(text_field(&mut e.conn.ssh.host, &same));
                });
                ui.vertical(|ui| {
                    field_label(ui, "Port");
                    ui.add(text_field(&mut e.ssh_port_text, "22").desired_width(88.0));
                });
            });
            field_label(ui, "SSH username");
            let same_user = if e.conn.username.trim().is_empty() { "pi".to_string() } else { e.conn.username.trim().to_string() };
            ui.add(text_field(&mut e.conn.ssh.username, &same_user));
            ui.add_space(4.0);

            let kind = login_kind(&e.conn.ssh.login);
            let labels = ["Same password as the screen login", "A separate password", "A key file", "The SSH agent"];
            let mut new_kind = kind;
            ui.horizontal(|ui| {
                ui.label("Log in with");
                ComboBox::from_id_salt("editor-ssh-login").selected_text(labels[kind]).width(260.0).show_ui(ui, |ui| {
                    for (i, label) in labels.iter().enumerate() {
                        ui.selectable_value(&mut new_kind, i, *label);
                    }
                });
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

            match &mut e.conn.ssh.login {
                SshLogin::SamePassword => {
                    hint_text(ui, "Uses the username and password above.");
                }
                SshLogin::Agent => {
                    hint_text(ui, "Uses the keys loaded in your SSH agent.");
                }
                SshLogin::Password => {
                    field_label(ui, "SSH password");
                    ssh_secret_field(ui, e_ssh(&mut e.ssh_password, &mut e.ssh_password_changed, e.has_saved_ssh_password, &mut e.show_ssh_password), store_name);
                }
                SshLogin::KeyFile(path) => {
                    field_label(ui, "Key file");
                    ui.horizontal(|ui| {
                        let width = ui.available_width() - 40.0;
                        ui.add(text_field(path, "~/.ssh/id_ed25519").desired_width(width));
                        let keys = ssh::default_key_files();
                        ui.add_enabled_ui(!keys.is_empty(), |ui| {
                            ui.menu_button(icon::KEY, |ui| {
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
                    });
                    field_label(ui, "Key passphrase");
                    ssh_secret_field(ui, e_ssh(&mut e.ssh_password, &mut e.ssh_password_changed, e.has_saved_ssh_password, &mut e.show_ssh_password), store_name);
                    hint_text(ui, "Leave empty if the key has no passphrase.");
                }
            }
        });
}

/// The SSH password fields of the editor, borrowed together.
struct SshSecret<'a> {
    text: &'a mut String,
    changed: &'a mut bool,
    saved: bool,
    show: &'a mut bool,
}

fn e_ssh<'a>(text: &'a mut String, changed: &'a mut bool, saved: bool, show: &'a mut bool) -> SshSecret<'a> {
    SshSecret { text, changed, saved, show }
}

fn ssh_secret_field(ui: &mut Ui, s: SshSecret, store_name: &str) {
    let hint = if s.saved && !*s.changed { format!("Saved in {store_name} – type to change") } else { String::new() };
    ui.horizontal(|ui| {
        let width = ui.available_width() - 40.0;
        if ui.add(text_field(s.text, &hint).password(!*s.show).desired_width(width)).changed() {
            *s.changed = true;
        }
        let eye = if *s.show { icon::EYE_SLASH } else { icon::EYE };
        if icon_button(ui, eye, if *s.show { "Hide" } else { "Show" }).clicked() {
            *s.show = !*s.show;
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
