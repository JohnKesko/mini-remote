//! The "Saved connections" page (with quick connect) and the "Find devices"
//! page (computers found on the local network).

use egui::{
    Align, Align2, Color32, CornerRadius, Frame, Id, Key, Layout, Margin, Rect, RichText, Sense, Stroke, Ui,
    pos2, vec2,
};

use super::theme::{Palette, ic, medium, semibold};
use super::widgets::{card, danger_button, hint_text, primary_button, secondary_button};
use super::{icon, status_color, status_text};
use crate::app::App;
use crate::model::Connection;

const MIN_CARD_WIDTH: f32 = 230.0;
const MAX_CARD_WIDTH: f32 = 300.0;
const CARD_GAP: f32 = 16.0;
/// Widest the page content gets.
pub const PAGE_WIDTH: f32 = 980.0;
/// Space between the content area's left edge and every page's content.
pub const PAGE_MARGIN: f32 = 40.0;

enum Action {
    QuickConnect,
    Connect(Connection),
    Edit(Connection),
    AskDelete(String),
    ConnectFound(discovery::DiscoveredHost),
    SaveFound(discovery::DiscoveredHost),
}

/// A scrollable page: a column of at most `max_width`, the same margin from
/// the left on every page (lined up with the title in the header).
pub fn page(ui: &mut Ui, id: &str, max_width: f32, add: impl FnOnce(&mut Ui, f32)) {
    egui::ScrollArea::vertical().id_salt(id).auto_shrink([false, false]).show(ui, |ui| {
        let width = (ui.available_width() - 2.0 * PAGE_MARGIN).clamp(240.0, max_width);
        ui.add_space(24.0);
        ui.horizontal(|ui| {
            ui.add_space(PAGE_MARGIN);
            ui.vertical(|ui| {
                ui.set_width(width);
                add(ui, width);
                ui.add_space(32.0);
            });
        });
    });
}

/// A line explaining what the page is for (the title is in the header).
pub fn page_intro(ui: &mut Ui, text: &str) {
    let p = Palette::of(ui.ctx());
    ui.label(RichText::new(text).color(p.text_weak).size(14.0));
    ui.add_space(16.0);
}

/// Number of columns and card width for a grid in `width`.
fn grid(width: f32) -> (usize, f32) {
    let columns = (((width + CARD_GAP) / (MIN_CARD_WIDTH + CARD_GAP)).floor() as usize).max(1);
    let card = ((width - CARD_GAP * (columns as f32 - 1.0)) / columns as f32).min(MAX_CARD_WIDTH);
    (columns, card)
}

pub fn saved_page(app: &mut App, ui: &mut Ui) {
    let p = Palette::of(ui.ctx());
    let mut action = None;
    page(ui, "saved-page", PAGE_WIDTH, |ui, width| {
        page_intro(ui, "Connect to your Raspberry Pi by typing its name or address, or pick a saved one.");
        quick_connect(app, ui, &p, &mut action);
        ui.add_space(28.0);
        saved_connections(app, ui, &p, width, &mut action);
    });
    apply(app, action);
}

pub fn find_page(app: &mut App, ui: &mut Ui) {
    let p = Palette::of(ui.ctx());
    let mut action = None;
    page(ui, "find-page", PAGE_WIDTH, |ui, width| {
        page_intro(ui, "Raspberry Pis and other computers with screen sharing on your network.");
        found_devices(app, ui, &p, width, &mut action);
    });
    apply(app, action);
}

fn apply(app: &mut App, action: Option<Action>) {
    match action {
        Some(Action::QuickConnect) => {
            let text = app.home.quick_address.clone();
            app.quick_connect(&text);
            if app.home.quick_error.is_none() {
                app.home.quick_address.clear();
            }
        }
        Some(Action::Connect(conn)) => app.open_connection(conn, None),
        Some(Action::Edit(conn)) => app.open_editor(conn, false, None),
        Some(Action::AskDelete(id)) => app.home.confirm_delete = Some(id),
        Some(Action::ConnectFound(host)) => app.connect_discovered(&host),
        Some(Action::SaveFound(host)) => app.save_discovered(&host),
        None => {}
    }
}

fn quick_connect(app: &mut App, ui: &mut Ui, p: &Palette, action: &mut Option<Action>) {
    card(p).inner_margin(Margin::same(10)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            ui.label(ic(icon::MONITOR).size(22.0).color(p.accent));
            let button_width = 120.0;
            let field_width = ui.available_width() - button_width - ui.spacing().item_spacing.x;
            let edit = egui::TextEdit::singleline(&mut app.home.quick_address)
                .hint_text("Pi address, e.g. raspberrypi.local")
                .font(egui::FontId::proportional(17.0))
                .frame(Frame::NONE)
                .margin(Margin::symmetric(8, 0))
                .vertical_align(Align::Center)
                .min_size(vec2(field_width, 40.0))
                .desired_width(field_width);
            let response = ui.add(edit);
            if app.home.focus_quick_connect {
                response.request_focus();
                app.home.focus_quick_connect = false;
            }
            if response.changed() {
                app.home.quick_error = None;
            }
            let enter = response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            let button = egui::Button::new(RichText::new("Connect").font(medium(15.0)).color(p.on_accent))
                .fill(p.accent)
                .stroke(Stroke::NONE)
                .corner_radius(CornerRadius::same(8))
                .min_size(vec2(button_width, 40.0));
            if ui.add(button).clicked() || enter {
                *action = Some(Action::QuickConnect);
            }
        });
    });
    if let Some(err) = &app.home.quick_error {
        ui.add_space(4.0);
        super::widgets::error_line(ui, err);
    }
}

fn saved_connections(app: &mut App, ui: &mut Ui, p: &Palette, width: f32, action: &mut Option<Action>) {
    if app.connections.is_empty() {
        card(p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(ic(icon::DESKTOP).size(28.0).color(p.text_weak));
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new("Nothing saved yet").font(medium(15.0)));
                    hint_text(ui, "Type your Pi's address above and press Connect. It is saved here for next time.");
                });
            });
        });
        return;
    }

    let (columns, card_width) = grid(width);
    let mut list: Vec<Connection> = app.connections.clone();
    // Most recently used first.
    list.sort_by_key(|c| std::cmp::Reverse(c.last_used.unwrap_or(0)));
    for row in list.chunks(columns) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = CARD_GAP;
            for conn in row {
                connection_card(app, ui, p, conn, card_width, action);
            }
        });
        ui.add_space(CARD_GAP - ui.spacing().item_spacing.y);
    }
}

/// The fixed grid inside a connection card (all in points, from the top of the body).
mod card_grid {
    /// Inner padding of the body, on all sides.
    pub const PAD: f32 = 12.0;
    pub const NAME_LINE: f32 = 20.0;
    pub const ADDRESS_LINE: f32 = 18.0;
    pub const STATUS_LINE: f32 = 18.0;
    pub const LINE_GAP: f32 = 2.0;
    pub const ACTIONS_GAP: f32 = 12.0;
    pub const ACTIONS: f32 = 32.0;
    pub const BODY: f32 = PAD + NAME_LINE + LINE_GAP + ADDRESS_LINE + LINE_GAP + STATUS_LINE + ACTIONS_GAP + ACTIONS + PAD;
}

/// One saved connection: a 16:10 picture of its last screen, then name,
/// address, status, and a footer with Connect and "…", all on a fixed grid so
/// every card looks the same. The whole card connects when clicked.
fn connection_card(app: &mut App, ui: &mut Ui, p: &Palette, conn: &Connection, width: f32, action: &mut Option<Action>) {
    use card_grid::*;
    let width = width.floor();
    let thumb_height = (width * 10.0 / 16.0).round();
    let (rect, response) = ui.allocate_exact_size(vec2(width, thumb_height + BODY), Sense::click());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    let hovered = response.hovered();
    let radius = CornerRadius::same(12);

    // Card and hover shadow (drawn behind; nothing moves).
    let shadow = egui::Shadow {
        offset: [0, if hovered { 6 } else { 2 }],
        blur: if hovered { 18 } else { 8 },
        spread: 0,
        color: Color32::from_black_alpha(match (p.dark, hovered) {
            (true, true) => 110,
            (true, false) => 50,
            (false, true) => 34,
            (false, false) => 12,
        }),
    };
    ui.painter().add(shadow.as_shape(rect, radius));
    let stroke = Stroke::new(1.0, if hovered { p.accent.gamma_multiply(0.6) } else { p.border });
    ui.painter().rect(rect, radius, p.surface, stroke, egui::StrokeKind::Inside);

    // Picture of the last screen (filled and cropped), or a placeholder.
    let thumb_rect = Rect::from_min_size(rect.min, vec2(width, thumb_height)).shrink(1.0);
    let thumb_radius = CornerRadius { nw: 11, ne: 11, sw: 0, se: 0 };
    match thumbnail(app, ui.ctx(), &conn.id) {
        Some(tex) => {
            let uv = cover_uv(tex.size_vec2(), thumb_rect.size());
            let image = egui::Image::from_texture(&tex)
                .uv(uv)
                .maintain_aspect_ratio(false)
                .fit_to_exact_size(thumb_rect.size())
                .corner_radius(thumb_radius);
            image.paint_at(ui, thumb_rect);
        }
        None => {
            ui.painter().rect_filled(thumb_rect, thumb_radius, p.surface_alt);
            let center = super::widgets::snap(ui, thumb_rect.center());
            ui.painter().text(center, Align2::CENTER_CENTER, icon::MONITOR, super::theme::icon_font(36.0), p.idle);
        }
    }
    ui.painter().hline(thumb_rect.x_range(), thumb_rect.bottom() + 0.5, Stroke::new(1.0, p.border));

    // Text lines, each with its space reserved even when empty.
    let left = rect.left() + PAD;
    let text_width = width - 2.0 * PAD;
    let mut y = thumb_rect.bottom() + 1.0 + PAD;
    let line_center = |y: f32, h: f32| y + h / 2.0;

    let name = super::widgets::truncated(ui, conn.display_name(), semibold(15.0), p.text, text_width);
    let name_rect = super::widgets::paint_left_center(ui, name, left, line_center(y, NAME_LINE), p.text);
    y += NAME_LINE + LINE_GAP;

    let mut address = conn.address_label();
    if conn.ssh.enabled {
        address.push_str(" · via SSH");
    }
    let address_galley =
        super::widgets::truncated(ui, &address, egui::FontId::proportional(13.0), p.text_weak, text_width);
    super::widgets::paint_left_center(ui, address_galley, left, line_center(y, ADDRESS_LINE), p.text_weak);
    y += ADDRESS_LINE + LINE_GAP;

    let status = app.connection_status(&conn.id);
    let status_line = match status {
        Some(crate::session::Status::Connected) => "Connected".to_string(),
        Some(s @ (crate::session::Status::Connecting(_) | crate::session::Status::Reconnecting { .. })) => {
            status_text(Some(s)).to_string()
        }
        _ => last_used_text(conn.last_used),
    };
    let status_center = line_center(y, STATUS_LINE);
    let dot = super::widgets::snap(ui, pos2(left + 4.0, status_center));
    ui.painter().circle_filled(dot, 4.0, status_color(p, status));
    let status_galley = super::widgets::truncated(ui, &status_line, egui::FontId::proportional(12.0), p.text_weak, text_width - 14.0);
    let status_rect = super::widgets::paint_left_center(ui, status_galley, left + 14.0, status_center, p.text_weak);
    y += STATUS_LINE + ACTIONS_GAP;

    // Footer pinned to the bottom: Connect on the left, "…" on the right.
    let footer = Rect::from_min_max(pos2(left, y), pos2(rect.right() - PAD, y + ACTIONS));
    let mut menu_action = None;
    let mut connect_rect = Rect::NOTHING;
    let mut menu_rect = Rect::NOTHING;
    super::widgets::in_rect(ui, footer, Layout::left_to_right(Align::Center), |ui| {
        let connect = super::widgets::primary_button(ui, "Connect");
        connect_rect = connect.rect;
        if connect.clicked() {
            menu_action = Some(Action::Connect(conn.clone()));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let button = egui::Button::new(ic(icon::DOTS_THREE).size(18.0))
                .frame_when_inactive(false)
                .corner_radius(CornerRadius::same(8))
                .min_size(vec2(32.0, 32.0));
            let (menu_button, _) = egui::containers::menu::MenuButton::from_button(button).ui(ui, |ui| {
                ui.set_min_width(150.0);
                if super::widgets::menu_item(ui, icon::PLUGS_CONNECTED, "Connect").clicked() {
                    menu_action = Some(Action::Connect(conn.clone()));
                }
                if super::widgets::menu_item(ui, icon::PENCIL_SIMPLE, "Edit").clicked() {
                    menu_action = Some(Action::Edit(conn.clone()));
                }
                ui.separator();
                if super::widgets::danger_menu_item(ui, icon::TRASH, "Delete").clicked() {
                    menu_action = Some(Action::AskDelete(conn.id.clone()));
                }
            });
            menu_rect = menu_button.on_hover_text("More").rect;
        });
    });
    super::widgets::check_centres("card footer", footer.center().y, &[connect_rect, menu_rect]);
    super::widgets::check_centres("card name", line_center(thumb_rect.bottom() + 1.0 + PAD, NAME_LINE), &[name_rect]);
    super::widgets::check_centres("card status", status_center, &[status_rect]);

    if menu_action.is_some() {
        *action = menu_action;
    } else if response.clicked() {
        *action = Some(Action::Connect(conn.clone()));
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
    let Some(secs) = last_used else { return "Never used".into() };
    let Some(when) = Local.timestamp_opt(secs, 0).single() else { return "Never used".into() };
    let now = Local::now();
    let days = (now.date_naive() - when.date_naive()).num_days();
    match days {
        ..=0 => format!("Last used today at {}", when.format("%H:%M")),
        1 => "Last used yesterday".into(),
        2..=6 => format!("Last used {days} days ago"),
        _ if when.format("%Y").to_string() == now.format("%Y").to_string() => {
            format!("Last used on {}", when.format("%-d %B"))
        }
        _ => format!("Last used on {}", when.format("%-d %B %Y")),
    }
}

fn found_devices(app: &mut App, ui: &mut Ui, p: &Palette, width: f32, action: &mut Option<Action>) {
    if app.discovery.hosts.is_empty() {
        card(p).inner_margin(Margin::same(20)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(ic(icon::BROADCAST).size(30.0).color(p.text_weak));
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    if app.discovery.running {
                        ui.label(RichText::new("Looking for Raspberry Pis…").font(medium(15.0)));
                        hint_text(ui, "This takes a few seconds. Pis with screen sharing turned on show up here.");
                    } else {
                        ui.label(RichText::new("No Raspberry Pi found yet").font(medium(15.0)));
                        hint_text(
                            ui,
                            "Make sure the Pi is switched on and connected to the same network as this computer, and that VNC is turned on in Raspberry Pi Configuration > Interfaces. Then press Look again.",
                        );
                    }
                });
            });
        });
        return;
    }

    let (columns, card_width) = grid(width);
    let hosts = app.discovery.hosts.clone();
    for (index, row) in hosts.chunks(columns).enumerate() {
        // Cards in a row share the height of the tallest one (measured last frame).
        let key = Id::new(("device-row-height", index));
        let row_height: f32 = ui.data(|d| d.get_temp(key)).unwrap_or(0.0);
        let mut tallest: f32 = 0.0;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = CARD_GAP;
            for host in row {
                let saved = app.saved_for(host).is_some();
                let height = device_card(ui, p, host, saved, card_width, row_height, action);
                tallest = tallest.max(height);
            }
        });
        if (tallest - row_height).abs() > 0.5 {
            ui.data_mut(|d| d.insert_temp(key, tallest));
            ui.ctx().request_repaint();
        }
        ui.add_space(CARD_GAP - 8.0);
    }
}

/// One found computer. Returns the content height, so a row can line up.
fn device_card(
    ui: &mut Ui,
    p: &Palette,
    host: &discovery::DiscoveredHost,
    saved: bool,
    width: f32,
    row_height: f32,
    action: &mut Option<Action>,
) -> f32 {
    card(p)
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(width - 34.0);
            ui.set_min_height(row_height);
            ui.vertical(|ui| device_card_contents(ui, p, host, saved, row_height, action)).inner
        })
        .inner
}

/// Draws the card's contents and returns their natural height (without the
/// space that pushes the buttons to the bottom).
fn device_card_contents(
    ui: &mut Ui,
    p: &Palette,
    host: &discovery::DiscoveredHost,
    saved: bool,
    row_height: f32,
    action: &mut Option<Action>,
) -> f32 {
    let top = ui.cursor().top();
    ui.horizontal(|ui| {
        let symbol = if host.is_raspberry_pi { icon::CPU } else { icon::DESKTOP };
        let color = if host.vnc_ports.is_empty() { p.idle } else { p.accent };
        ui.label(ic(symbol).size(24.0).color(color));
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.add(egui::Label::new(RichText::new(crate::app::discovered_name(host)).font(medium(15.0))).truncate());
            let mut address = host.address.to_string();
            if let Some(port) = host.vnc_ports.first().filter(|p| **p != crate::model::DEFAULT_VNC_PORT) {
                address = format!("{address}:{port}");
            }
            ui.label(RichText::new(address).size(12.0).color(p.text_weak));
        });
    });
    // Always the same height, so text below lines up across cards.
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
    let mut x = row.left();
    let labels = [
        (host.is_raspberry_pi, "Raspberry Pi"),
        (host.rfb_version.as_deref() == Some("5.0"), "RealVNC Server"),
        (saved, "Saved"),
    ];
    for (_, text) in labels.iter().filter(|(on, _)| *on) {
        x += badge(ui, p, pos2(x, row.center().y), text) + 6.0;
    }
    if host.vnc_ports.is_empty() {
        ui.label(RichText::new("Screen sharing is off").font(medium(13.0)).color(p.text_weak));
        let hint = if host.is_raspberry_pi {
            "Turn on VNC in Raspberry Pi Configuration > Interfaces, then press Look again."
        } else {
            "This computer does not offer screen sharing (VNC)."
        };
        hint_text(ui, hint);
        return ui.cursor().top() - top;
    }
    if host.rfb_version.as_deref() == Some("5.0") {
        hint_text(ui, "Uses its own login: set Authentication to “VNC password” in RealVNC Server's options.");
    }
    // Buttons sit at the bottom, in line with the other cards of the row.
    let natural = ui.cursor().top() - top + 4.0 + crate::ui::theme::CONTROL_HEIGHT;
    ui.add_space((row_height - natural).max(0.0) + 4.0);
    ui.horizontal(|ui| {
        if primary_button(ui, "Connect").clicked() {
            *action = Some(Action::ConnectFound(host.clone()));
        }
        if !saved && secondary_button(ui, "Save…").on_hover_text("Save it to your connections").clicked() {
            *action = Some(Action::SaveFound(host.clone()));
        }
    });
    natural
}

/// A small tag at `left_center`; returns its width.
fn badge(ui: &Ui, p: &Palette, left_center: egui::Pos2, text: &str) -> f32 {
    let galley = ui.painter().layout_no_wrap(text.to_string(), medium(11.0), p.accent);
    let rect = Rect::from_min_size(
        pos2(left_center.x, left_center.y - 10.0),
        vec2(galley.size().x + 12.0, 20.0),
    );
    ui.painter().rect_filled(rect, CornerRadius::same(6), p.accent_soft);
    ui.painter().galley(pos2(rect.left() + 6.0, rect.center().y - galley.size().y / 2.0), galley, p.accent);
    rect.width()
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
        ui.label(RichText::new(format!("Delete “{}”?", conn.display_name())).heading());
        ui.add_space(4.0);
        ui.label(
            RichText::new("The saved settings, password and picture for this connection will be removed.").color(p.text_weak),
        );
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

    #[test]
    fn grid_fits() {
        let (cols, w) = grid(740.0);
        assert_eq!(cols, 3);
        assert!(w * 3.0 + 2.0 * CARD_GAP <= 740.5);
        assert_eq!(grid(200.0).0, 1);
    }
}
