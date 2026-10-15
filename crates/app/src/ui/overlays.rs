//! Cards shown over the remote screen: connecting, log in, "is this your
//! Pi?", errors and reconnecting.

use std::time::Instant;

use egui::{Align, Align2, Area, Id, Key, Layout, Order, Rect, RichText, Ui};
use rfb::{DisconnectKind, Phase};

use super::icon;
use super::theme::Palette;
use super::widgets::{danger_button, floating, hint_text, primary_button, secondary_button, text_field};
use crate::app::App;
use crate::session::{IdentitySource, LoginPurpose, Prompt, Status};

const CARD_WIDTH: f32 = 380.0;

enum Action {
    Cancel,
    SubmitCredentials,
    Trust(bool),
    Retry,
    Edit,
}

pub fn show(app: &mut App, ui: &mut Ui, index: usize, area: Rect) {
    let p = Palette::of(ui.ctx());
    let store_name = crate::platform::secret_store_name();
    let store_available = app.secrets.available();
    let tab = &mut app.tabs[index];
    let name = tab.title().to_string();
    let mut action = None;

    let has_prompt = tab.prompt.is_some();
    let showing = has_prompt || !tab.status.is_connected();
    if !showing {
        return;
    }

    let ctx = ui.ctx().clone();
    Area::new(Id::new(("overlay", tab.id)))
        .order(Order::Middle)
        .pivot(Align2::CENTER_CENTER)
        .fixed_pos(area.center())
        .constrain_to(area)
        .show(&ctx, |ui| {
            floating(&p).show(ui, |ui| {
                ui.set_width(CARD_WIDTH);
                match &mut tab.prompt {
                    Some(Prompt::Credentials(prompt)) => {
                        let (title, explain) = match prompt.purpose {
                            LoginPurpose::Screen if tab.conn.ssh.enabled => (
                                format!("Log in to {name}"),
                                "Use the username and password you use on the Pi. They are also used for the secure remote login (SSH).",
                            ),
                            LoginPurpose::Screen if prompt.need_username => {
                                (format!("Log in to {name}"), "Use the username and password you use on the Pi.")
                            }
                            LoginPurpose::Screen => (format!("Log in to {name}"), "Type the screen sharing password of the Pi."),
                            LoginPurpose::Ssh => {
                                (format!("Secure login to {name}"), "Type the password for the Pi's secure remote login (SSH).")
                            }
                            LoginPurpose::SshPassphrase => {
                                ("Unlock your key".to_string(), "Type the passphrase of your SSH key file.")
                            }
                        };
                        heading(ui, &p, icon::LOCK_SIMPLE, p.accent, &title);
                        ui.label(RichText::new(explain).color(p.text_weak));
                        if let Some(err) = &prompt.error {
                            ui.add_space(4.0);
                            ui.label(RichText::new(format!("{}  {err}", icon::WARNING_CIRCLE)).color(p.danger));
                        }
                        ui.add_space(10.0);
                        let focus_id = Id::new(("credentials-focus", tab.id));
                        let first_show = ui.data_mut(|d| {
                            let shown = d.get_temp::<bool>(focus_id).unwrap_or(false);
                            d.insert_temp(focus_id, true);
                            !shown
                        });
                        let mut enter = false;
                        if prompt.need_username {
                            super::widgets::field_label(ui, "Username");
                            let r = ui.add(text_field(&mut prompt.username, "pi"));
                            if first_show && prompt.username.is_empty() {
                                r.request_focus();
                            }
                            enter |= r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                            ui.add_space(4.0);
                        }
                        let label = if prompt.purpose == LoginPurpose::SshPassphrase { "Passphrase" } else { "Password" };
                        super::widgets::field_label(ui, label);
                        ui.horizontal(|ui| {
                            let width = ui.available_width() - 40.0;
                            let r = ui.add(text_field(&mut prompt.password, "").password(!prompt.show_password).desired_width(width));
                            if first_show && (!prompt.need_username || !prompt.username.is_empty()) {
                                r.request_focus();
                            }
                            enter |= r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                            let eye = if prompt.show_password { icon::EYE_SLASH } else { icon::EYE };
                            let tip = if prompt.show_password { "Hide password" } else { "Show password" };
                            if super::widgets::icon_button(ui, eye, tip).clicked() {
                                prompt.show_password = !prompt.show_password;
                            }
                        });
                        if store_available {
                            ui.add_space(4.0);
                            ui.checkbox(&mut prompt.remember, format!("Remember password in {store_name}"));
                        }
                        ui.add_space(12.0);
                        buttons(ui, |ui| {
                            if primary_button(ui, "Log in").clicked() || enter {
                                action = Some(Action::SubmitCredentials);
                            }
                            if secondary_button(ui, "Cancel").clicked() {
                                action = Some(Action::Cancel);
                            }
                        });
                        if action.is_some() {
                            ui.data_mut(|d| d.remove::<bool>(focus_id));
                        }
                    }
                    Some(Prompt::Identity(prompt)) => {
                        let (host, how, raw) = match &prompt.source {
                            IdentitySource::Screen => (tab.conn.host.clone(), "connects to", false),
                            IdentitySource::Ssh { host, .. } => (host.clone(), "logs in securely (SSH) to", true),
                        };
                        let show_code = |ui: &mut Ui, code: &str| {
                            if raw { raw_fingerprint(ui, &p, code) } else { fingerprint(ui, &p, code) }
                        };
                        if prompt.previous.is_none() {
                            heading(ui, &p, icon::SHIELD_CHECK, p.accent, "Is this your Pi?");
                            ui.label(format!(
                                "This is the first time Mini Remote {how} {host}. Every Pi has its own identity code, and Mini Remote will remember this one."
                            ));
                            ui.add_space(8.0);
                            show_code(ui, &prompt.fingerprint);
                            ui.add_space(8.0);
                            hint_text(ui, "If this is your own Pi on your own network, it's safe to continue. Mini Remote will warn you if the code ever changes.");
                            ui.add_space(12.0);
                            buttons(ui, |ui| {
                                if primary_button(ui, "Yes, connect").clicked() {
                                    action = Some(Action::Trust(true));
                                }
                                if secondary_button(ui, "Cancel").clicked() {
                                    action = Some(Action::Trust(false));
                                }
                            });
                        } else {
                            heading(ui, &p, icon::SHIELD_WARNING, p.danger, "The Pi's identity has changed");
                            ui.label(format!(
                                "The identity code of {host} is different from last time. This happens when the Pi was set up again, but it can also mean someone is trying to listen in on your connection."
                            ));
                            ui.add_space(8.0);
                            ui.label(RichText::new("Only continue if you know why it changed.").strong());
                            ui.add_space(8.0);
                            ui.label(RichText::new("New code").small().color(p.text_weak));
                            show_code(ui, &prompt.fingerprint);
                            if let Some(prev) = &prompt.previous {
                                ui.add_space(4.0);
                                ui.label(RichText::new("Code you trusted before").small().color(p.text_weak));
                                show_code(ui, prev);
                            }
                            ui.add_space(12.0);
                            buttons(ui, |ui| {
                                if primary_button(ui, "Don't connect").clicked() {
                                    action = Some(Action::Trust(false));
                                }
                                if danger_button(ui, "Trust the new code").clicked() {
                                    action = Some(Action::Trust(true));
                                }
                            });
                        }
                    }
                    None => match &tab.status {
                        Status::Connecting(phase) => {
                            ui.horizontal(|ui| {
                                ui.add(egui::Spinner::new().size(22.0).color(p.accent));
                                ui.add_space(6.0);
                                ui.label(RichText::new(format!("Connecting to {name}…")).heading().strong());
                            });
                            ui.add_space(4.0);
                            let text = if tab.in_ssh_stage() {
                                "Logging in securely (SSH)…"
                            } else if tab.conn.ssh.enabled && *phase == Phase::Connecting {
                                "Opening the secure tunnel…"
                            } else {
                                phase_text(*phase)
                            };
                            ui.label(RichText::new(text).color(p.text_weak));
                            ui.add_space(12.0);
                            buttons(ui, |ui| {
                                if secondary_button(ui, "Cancel").clicked() {
                                    action = Some(Action::Cancel);
                                }
                            });
                        }
                        Status::Reconnecting { attempt, at, message } => {
                            heading(ui, &p, icon::WIFI_SLASH, p.warning, "Connection lost");
                            let secs = at.saturating_duration_since(Instant::now()).as_secs_f32().ceil() as u64;
                            ui.horizontal(|ui| {
                                ui.add(egui::Spinner::new().size(16.0).color(p.accent));
                                let text = if secs == 0 {
                                    "Reconnecting…".to_string()
                                } else {
                                    format!("Trying again in {secs} s")
                                };
                                ui.label(text);
                                if *attempt > 1 {
                                    ui.label(RichText::new(format!("(attempt {attempt})")).color(p.text_weak));
                                }
                            });
                            if !message.is_empty() {
                                hint_text(ui, message);
                            }
                            ui.add_space(12.0);
                            buttons(ui, |ui| {
                                if primary_button(ui, "Reconnect now").clicked() {
                                    action = Some(Action::Retry);
                                }
                                if secondary_button(ui, "Close").clicked() {
                                    action = Some(Action::Cancel);
                                }
                            });
                        }
                        Status::Failed(reason) => {
                            let (title, tip) = if tab.failed_in_ssh {
                                (
                                    format!("Couldn't log in securely to {name}"),
                                    Some("Check that SSH is turned on in Raspberry Pi Configuration > Interfaces > SSH, or turn off “Connect through SSH” under Edit connection."),
                                )
                            } else {
                                failure_text(reason.kind, &name)
                            };
                            heading(ui, &p, icon::WARNING_CIRCLE, p.danger, &title);
                            if !reason.message.is_empty() {
                                ui.label(&reason.message);
                            }
                            if let Some(tip) = tip {
                                ui.add_space(4.0);
                                hint_text(ui, tip);
                            }
                            ui.add_space(12.0);
                            buttons(ui, |ui| {
                                if primary_button(ui, "Try again").clicked() {
                                    action = Some(Action::Retry);
                                }
                                if secondary_button(ui, "Edit connection").clicked() {
                                    action = Some(Action::Edit);
                                }
                                if secondary_button(ui, "Close").clicked() {
                                    action = Some(Action::Cancel);
                                }
                            });
                        }
                        Status::Connected | Status::Closed => {}
                    },
                }
            });
        });

    match action {
        Some(Action::Cancel) => {
            let id = app.tabs[index].id;
            app.close_tab(id);
        }
        Some(Action::SubmitCredentials) => app.submit_credentials(index),
        Some(Action::Trust(trust)) => app.answer_identity(index, trust),
        Some(Action::Retry) => app.retry_tab(index),
        Some(Action::Edit) => {
            let tab = &app.tabs[index];
            let (conn, saved, id) = (tab.conn.clone(), tab.saved, tab.id);
            app.open_editor(conn, !saved, Some(id));
        }
        None => {}
    }
}

fn heading(ui: &mut Ui, p: &Palette, symbol: &str, color: egui::Color32, text: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(symbol).size(24.0).color(color));
        ui.add_space(2.0);
        ui.add(egui::Label::new(RichText::new(text).heading().strong().color(p.text)).wrap());
    });
    ui.add_space(4.0);
}

/// Buttons in a row on the right, the first one rightmost.
fn buttons(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.with_layout(Layout::right_to_left(Align::Center), add);
}

fn phase_text(phase: Phase) -> &'static str {
    match phase {
        Phase::Connecting => "Looking for the Pi on the network…",
        Phase::Negotiating => "Setting up a secure connection…",
        Phase::Authenticating => "Logging in…",
        Phase::Connected => "Getting the picture…",
    }
}

fn failure_text(kind: DisconnectKind, name: &str) -> (String, Option<&'static str>) {
    match kind {
        DisconnectKind::Network => (
            format!("Couldn't reach {name}"),
            Some("Check that the Pi is switched on, is on the same network as this computer, and that screen sharing (VNC) is turned on in the Pi's settings."),
        ),
        DisconnectKind::AuthFailed => ("Login failed".into(), None),
        DisconnectKind::Refused => ("The Pi refused the connection".into(), None),
        DisconnectKind::Untrusted => ("Connection stopped".into(), None),
        DisconnectKind::NeedsUnencrypted => (
            "This Pi only offers an unencrypted login".into(),
            Some("You can allow it under Edit connection, but others on your network could then read your password and see your screen."),
        ),
        DisconnectKind::Unsupported => ("Mini Remote can't log in to this Pi".into(), None),
        DisconnectKind::Protocol => ("Something went wrong".into(), Some("Trying again usually helps.")),
        DisconnectKind::Closed => ("The connection was closed".into(), None),
    }
}

/// Show an identity code in short groups so it is easy to compare.
fn fingerprint(ui: &mut Ui, p: &Palette, fp: &str) {
    let groups = group_fingerprint(fp);
    egui::Frame::new()
        .fill(p.surface_alt)
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            for line in groups.chunks(6) {
                ui.label(RichText::new(line.join("  ")).monospace().size(14.0));
            }
        });
}

/// An OpenSSH-style code ("SHA256:...") shown as it is.
fn raw_fingerprint(ui: &mut Ui, p: &Palette, fp: &str) {
    egui::Frame::new()
        .fill(p.surface_alt)
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(egui::Label::new(RichText::new(fp).monospace().size(13.0)).wrap_mode(egui::TextWrapMode::Wrap));
        });
}

/// "ab:cd:ef:01:..." or "abcdef01..." becomes ["ABCD", "EF01", ...].
pub fn group_fingerprint(fp: &str) -> Vec<String> {
    let clean: String = fp.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '+' || *c == '/').collect();
    let is_hex = clean.chars().all(|c| c.is_ascii_hexdigit());
    let text = if is_hex { clean.to_ascii_uppercase() } else { clean };
    text.chars().collect::<Vec<_>>().chunks(4).map(|c| c.iter().collect()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_groups() {
        assert_eq!(group_fingerprint("ab:cd:ef:01:23"), vec!["ABCD", "EF01", "23"]);
        assert_eq!(group_fingerprint("SHA256:xyZ+"), vec!["SHA2", "56xy", "Z+"]);
    }
}
