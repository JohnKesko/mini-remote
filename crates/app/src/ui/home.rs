//! The home screen: quick connect, saved connections, and computers found
//! on the network.

use egui::{
    Align, Align2, Color32, CornerRadius, FontId, Frame, Id, Key, Layout, Margin, Rect, RichText, Sense, Stroke,
    Ui, UiBuilder, pos2, vec2,
};

use super::theme::{Palette, title_style};
use super::widgets::{card, centre_offset, danger_button, hint_text, primary_button, secondary_button};
use super::{icon, status_color, status_text};
use crate::app::App;
use crate::model::Connection;

const CARD_WIDTH: f32 = 264.0;
const THUMB_HEIGHT: f32 = 148.0;
const CARD_HEIGHT: f32 = THUMB_HEIGHT + 74.0;
const CARD_GAP: f32 = 16.0;
const CONTENT_WIDTH: f32 = 3.0 * CARD_WIDTH + 2.0 * CARD_GAP;

enum HomeAction {
    QuickConnect,
    Connect(Connection),
    Edit(Connection),
    AskDelete(String),
    Add,
    ConnectDiscovered(discovery::DiscoveredHost),
    Rescan,
}

pub fn show(app: &mut App, ui: &mut Ui) {
    let p = Palette::of(ui.ctx());
    let mut action = None;

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let width = CONTENT_WIDTH.min(ui.available_width() - 48.0).max(300.0);
        ui.add_space(36.0);
        ui.horizontal(|ui| {
            ui.add_space(centre_offset(ui, width));
            ui.vertical(|ui| {
                ui.set_width(width);
                quick_connect(app, ui, &p, &mut action);
                ui.add_space(36.0);
                saved_connections(app, ui, &p, width, &mut action);
                ui.add_space(36.0);
                on_your_network(app, ui, &p, &mut action);
                ui.add_space(36.0);
            });
        });
    });

    match action {
        Some(HomeAction::QuickConnect) => {
            let text = app.home.quick_address.clone();
            app.quick_connect(&text);
            if app.home.quick_error.is_none() {
                app.home.quick_address.clear();
            }
        }
        Some(HomeAction::Connect(conn)) => app.open_connection(conn, None),
        Some(HomeAction::Edit(conn)) => app.open_editor(conn, false, None),
        Some(HomeAction::AskDelete(id)) => app.home.confirm_delete = Some(id),
        Some(HomeAction::Add) => app.open_editor_new(),
        Some(HomeAction::ConnectDiscovered(host)) => app.connect_discovered(&host),
        Some(HomeAction::Rescan) => app.start_discovery(true),
        None => {}
    }
}

fn quick_connect(app: &mut App, ui: &mut Ui, p: &Palette, action: &mut Option<HomeAction>) {
    ui.label(RichText::new("Connect to your Raspberry Pi").text_style(title_style()).color(p.text));
    ui.add_space(2.0);
    ui.label(RichText::new("Type the Pi's name or address and press Connect.").color(p.text_weak).size(15.0));
    ui.add_space(16.0);

    card(p).inner_margin(Margin::same(10)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            ui.label(RichText::new(icon::MONITOR).size(22.0).color(p.accent));
            let button_width = 120.0;
            let field_width = ui.available_width() - button_width - 12.0;
            let edit = egui::TextEdit::singleline(&mut app.home.quick_address)
                .hint_text("Pi address, e.g. raspberrypi.local")
                .font(FontId::proportional(18.0))
                .frame(egui::Frame::NONE)
                .margin(Margin::symmetric(8, 10))
                .desired_width(field_width);
            let response = ui.add(edit);
            if app.home.focus_quick_connect && app.editor.is_none() && !app.settings_open {
                response.request_focus();
                app.home.focus_quick_connect = false;
            }
            if response.changed() {
                app.home.quick_error = None;
            }
            let enter = response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            let button = egui::Button::new(RichText::new("Connect").size(15.0).strong().color(p.on_accent))
                .fill(p.accent)
                .stroke(Stroke::NONE)
                .corner_radius(CornerRadius::same(8))
                .min_size(vec2(button_width, 40.0));
            if ui.add(button).clicked() || enter {
                *action = Some(HomeAction::QuickConnect);
            }
        });
    });
    if let Some(err) = &app.home.quick_error {
        ui.add_space(4.0);
        ui.label(RichText::new(err).color(p.danger));
    }
}

fn saved_connections(app: &mut App, ui: &mut Ui, p: &Palette, width: f32, action: &mut Option<HomeAction>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Saved connections").heading().strong());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if secondary_button(ui, format!("{}  Add connection", icon::PLUS)).clicked() {
                *action = Some(HomeAction::Add);
            }
        });
    });
    ui.add_space(8.0);

    if app.connections.is_empty() {
        card(p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon::DESKTOP).size(28.0).color(p.text_weak));
                ui.add_space(6.0);
                ui.label(
                    RichText::new("Nothing saved yet. Type your Pi's address above and press Connect — it will be saved here for next time.")
                        .color(p.text_weak),
                );
            });
        });
        return;
    }

    let columns = (((width + CARD_GAP) / (CARD_WIDTH + CARD_GAP)).floor() as usize).max(1);
    let mut list: Vec<Connection> = app.connections.clone();
    // Most recently used first.
    list.sort_by_key(|c| std::cmp::Reverse(c.last_used.unwrap_or(0)));
    for row in list.chunks(columns) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = CARD_GAP;
            for conn in row {
                connection_card(app, ui, p, conn, action);
            }
        });
        ui.add_space(CARD_GAP - 8.0);
    }
}

fn connection_card(app: &mut App, ui: &mut Ui, p: &Palette, conn: &Connection, action: &mut Option<HomeAction>) {
    let (rect, response) = ui.allocate_exact_size(vec2(CARD_WIDTH, CARD_HEIGHT), Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    if !ui.is_rect_visible(rect) {
        return;
    }
    let hovered = response.hovered();
    let radius = CornerRadius::same(12);
    let painter = ui.painter();
    if hovered {
        painter.rect_filled(rect.translate(vec2(0.0, 3.0)).expand(1.0), radius, Color32::from_black_alpha(if p.dark { 60 } else { 18 }));
    }
    painter.rect(rect, radius, p.surface, Stroke::new(1.0, if hovered { p.accent.gamma_multiply(0.7) } else { p.border }), egui::StrokeKind::Inside);

    // Picture of the last screen, or a placeholder.
    let thumb_rect = Rect::from_min_size(rect.min, vec2(CARD_WIDTH, THUMB_HEIGHT)).shrink(1.0);
    let thumb_radius = CornerRadius { nw: 11, ne: 11, sw: 0, se: 0 };
    match thumbnail(app, ui.ctx(), &conn.id) {
        Some(tex) => {
            let uv = cover_uv(tex.size_vec2(), thumb_rect.size());
            ui.put(
                thumb_rect,
                egui::Image::from_texture(&tex).uv(uv).fit_to_exact_size(thumb_rect.size()).corner_radius(thumb_radius),
            );
        }
        None => {
            ui.painter().rect_filled(thumb_rect, thumb_radius, p.surface_alt);
            ui.painter().text(thumb_rect.center(), Align2::CENTER_CENTER, icon::MONITOR, FontId::proportional(40.0), p.idle);
        }
    }

    // Name, address and when it was last used.
    let status = app.connection_status(&conn.id);
    let text_rect = Rect::from_min_max(pos2(rect.left() + 14.0, thumb_rect.bottom() + 10.0), pos2(rect.right() - 10.0, rect.bottom() - 8.0));
    let mut menu_action = None;
    ui.scope_builder(UiBuilder::new().max_rect(text_rect).layout(Layout::top_down(Align::Min)), |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.horizontal(|ui| {
            super::widgets::status_dot(ui, status_color(p, status)).on_hover_text(status_text(status));
            ui.add(egui::Label::new(RichText::new(conn.display_name()).strong().size(15.0)).truncate());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let button = egui::Button::new(RichText::new(icon::DOTS_THREE).size(18.0))
                    .frame_when_inactive(false)
                    .min_size(vec2(28.0, 24.0));
                let (_, inner) = egui::containers::menu::MenuButton::from_button(button).ui(ui, |ui| {
                    ui.set_min_width(150.0);
                    if ui.button(format!("{}  Connect", icon::PLUGS_CONNECTED)).clicked() {
                        menu_action = Some(HomeAction::Connect(conn.clone()));
                    }
                    if ui.button(format!("{}  Edit", icon::PENCIL_SIMPLE)).clicked() {
                        menu_action = Some(HomeAction::Edit(conn.clone()));
                    }
                    ui.separator();
                    if ui.button(RichText::new(format!("{}  Delete", icon::TRASH)).color(p.danger)).clicked() {
                        menu_action = Some(HomeAction::AskDelete(conn.id.clone()));
                    }
                });
                let _ = inner;
            });
        });
        let address = if conn.name.trim().is_empty() || conn.name.trim() == conn.host { String::new() } else { conn.address_label() };
        if !address.is_empty() {
            ui.label(RichText::new(address).size(12.5).color(p.text_weak));
        }
        ui.label(RichText::new(last_used_text(conn.last_used)).size(12.0).color(p.text_weak));
    });

    if menu_action.is_some() {
        *action = menu_action;
    } else if response.clicked() {
        *action = Some(HomeAction::Connect(conn.clone()));
    }
}

/// Texture coordinates that fill `target` with the picture without stretching.
fn cover_uv(image: egui::Vec2, target: egui::Vec2) -> Rect {
    if image.x <= 0.0 || image.y <= 0.0 || target.x <= 0.0 || target.y <= 0.0 {
        return Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    }
    let image_aspect = image.x / image.y;
    let target_aspect = target.x / target.y;
    if image_aspect > target_aspect {
        let w = target_aspect / image_aspect;
        Rect::from_min_max(pos2((1.0 - w) / 2.0, 0.0), pos2((1.0 + w) / 2.0, 1.0))
    } else {
        let h = image_aspect / target_aspect;
        // Keep the top of the screen (menu bar) in view.
        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, h))
    }
}

fn thumbnail(app: &mut App, ctx: &egui::Context, id: &str) -> Option<egui::TextureHandle> {
    if let Some(cached) = app.home.thumbnails.get(id) {
        return cached.clone();
    }
    let loaded = app
        .storage
        .thumbnail_path(id)
        .filter(|p| p.exists())
        .and_then(|p| crate::storage::load_thumbnail(&p))
        .map(|img| ctx.load_texture(format!("thumb-{id}"), img, egui::TextureOptions::LINEAR));
    app.home.thumbnails.insert(id.to_string(), loaded.clone());
    loaded
}

fn last_used_text(last_used: Option<i64>) -> String {
    use chrono::{Local, TimeZone};
    let Some(secs) = last_used else { return "Never connected".into() };
    let Some(when) = Local.timestamp_opt(secs, 0).single() else { return String::new() };
    let now = Local::now();
    let days = (now.date_naive() - when.date_naive()).num_days();
    match days {
        0 => format!("Used today at {}", when.format("%H:%M")),
        1 => "Used yesterday".into(),
        2..=6 => format!("Used {}", when.format("%A")),
        _ if when.format("%Y").to_string() == now.format("%Y").to_string() => format!("Used {}", when.format("%-d %B")),
        _ => format!("Used {}", when.format("%-d %B %Y")),
    }
}

fn on_your_network(app: &mut App, ui: &mut Ui, p: &Palette, action: &mut Option<HomeAction>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("On your network").heading().strong());
        if app.discovery.running {
            ui.add(egui::Spinner::new().size(14.0).color(p.text_weak));
            ui.label(RichText::new("Looking…").size(12.5).color(p.text_weak));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_enabled_ui(!app.discovery.running, |ui| {
                if super::widgets::icon_button(ui, icon::ARROW_CLOCKWISE, "Look again").clicked() {
                    *action = Some(HomeAction::Rescan);
                }
            });
        });
    });
    ui.add_space(8.0);

    if app.discovery.hosts.is_empty() {
        Frame::new()
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(16))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::BROADCAST).size(26.0).color(p.text_weak));
                    ui.add_space(6.0);
                    ui.vertical(|ui| {
                        if app.discovery.running {
                            ui.label(RichText::new("Looking for Raspberry Pis…").strong());
                            hint_text(ui, "Pis with screen sharing turned on will show up here.");
                        } else {
                            ui.label(RichText::new("No Raspberry Pi found").strong());
                            hint_text(ui, "Make sure the Pi is switched on and on the same network, and that VNC is turned on in Raspberry Pi Configuration > Interfaces.");
                        }
                    });
                });
            });
        return;
    }

    let hosts = app.discovery.hosts.clone();
    let width = ui.available_width();
    let columns = (((width + CARD_GAP) / (CARD_WIDTH + CARD_GAP)).floor() as usize).max(1);
    for row in hosts.chunks(columns) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = CARD_GAP;
            for host in row {
                let saved = app.saved_for(host).is_some();
                discovered_card(ui, p, host, saved, action);
            }
        });
        ui.add_space(CARD_GAP - 8.0);
    }
}

fn discovered_card(ui: &mut Ui, p: &Palette, host: &discovery::DiscoveredHost, saved: bool, action: &mut Option<HomeAction>) {
    card(p).inner_margin(Margin::same(14)).show(ui, |ui| {
        ui.set_width(CARD_WIDTH - 30.0);
        ui.set_min_height(96.0);
        ui.vertical(|ui| discovered_card_contents(ui, p, host, saved, action));
    });
}

fn discovered_card_contents(
    ui: &mut Ui,
    p: &Palette,
    host: &discovery::DiscoveredHost,
    saved: bool,
    action: &mut Option<HomeAction>,
) {
    {
        ui.horizontal(|ui| {
            let symbol = if host.is_raspberry_pi { icon::CPU } else { icon::DESKTOP };
            ui.label(RichText::new(symbol).size(22.0).color(if host.vnc_ports.is_empty() { p.idle } else { p.accent }));
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.add(egui::Label::new(RichText::new(crate::app::discovered_name(host)).strong().size(14.5)).truncate());
                let mut address = host.address.to_string();
                if let Some(port) = host.vnc_ports.first().filter(|p| **p != crate::model::DEFAULT_VNC_PORT) {
                    address = format!("{address}:{port}");
                }
                ui.label(RichText::new(address).size(12.0).color(p.text_weak));
            });
        });
        ui.horizontal(|ui| {
            if host.is_raspberry_pi {
                badge(ui, p, "Raspberry Pi");
            }
            if host.rfb_version.as_deref() == Some("5.0") {
                badge(ui, p, "RealVNC Server");
            }
            if saved {
                badge(ui, p, "Saved");
            }
        });
        if host.vnc_ports.is_empty() {
            ui.label(RichText::new("Screen sharing is off").size(12.5).strong().color(p.text_weak));
            hint_text(ui, "Turn on VNC in Raspberry Pi Configuration > Interfaces, then look again.");
            return;
        }
        if host.rfb_version.as_deref() == Some("5.0") {
            hint_text(ui, "Set Authentication to “VNC password” in RealVNC Server's options to connect.");
        }
        ui.add_space(2.0);
        if primary_button(ui, "Connect").clicked() {
            *action = Some(HomeAction::ConnectDiscovered(host.clone()));
        }
    }
}

fn badge(ui: &mut Ui, p: &Palette, text: &str) {
    Frame::new()
        .fill(p.accent_soft)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(6, 1))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(11.0).color(p.accent));
        });
}

/// "Delete this connection?" dialog.
pub fn delete_confirmation(app: &mut App, ctx: &egui::Context) {
    let Some(id) = app.home.confirm_delete.clone() else { return };
    let Some(conn) = app.connections.iter().find(|c| c.id == id).cloned() else {
        app.home.confirm_delete = None;
        return;
    };
    let p = Palette::of(ctx);
    let mut close = false;
    let mut delete = false;
    let modal = egui::Modal::new(Id::new("delete-connection")).frame(super::widgets::floating(&p)).show(ctx, |ui| {
        ui.set_width(360.0);
        ui.label(RichText::new(format!("Delete “{}”?", conn.display_name())).heading().strong());
        ui.add_space(4.0);
        ui.label(RichText::new("The saved settings, password and picture for this connection will be removed.").color(p.text_weak));
        ui.add_space(14.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if danger_button(ui, "Delete").clicked() {
                delete = true;
            }
            if secondary_button(ui, "Cancel").clicked() {
                close = true;
            }
        });
    });
    if delete {
        app.delete_connection(&id);
        close = true;
    }
    if close || modal.should_close() {
        app.home.confirm_delete = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_keeps_aspect() {
        let uv = cover_uv(vec2(1920.0, 1080.0), vec2(264.0, 148.0));
        assert!(uv.width() <= 1.0 && uv.height() <= 1.0);
        let wide = cover_uv(vec2(400.0, 100.0), vec2(200.0, 100.0));
        assert!((wide.width() - 0.5).abs() < 1e-5);
    }
}
