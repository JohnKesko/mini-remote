//! The side menu: pages, open connections, and settings at the bottom. While
//! a remote screen is shown it can shrink to icons or hide completely.

use egui::text::{LayoutJob, TextWrapping};
use egui::{Align2, Color32, CornerRadius, Rect, Sense, Ui, pos2, vec2};

use super::theme::{Palette, icon_font, medium, semibold};
use super::{icon, status_color, status_text};
use crate::app::{App, Page};
use crate::model::SidebarMode;

pub const FULL_WIDTH: f32 = 220.0;
/// Narrowest the icon-only side menu gets.
const RAIL_MIN_WIDTH: f32 = 60.0;

/// Width of the icon-only side menu. On a Mac it is wide enough to hold the
/// window buttons with an equal gap on both sides, so they never stick out
/// over the header.
pub fn rail_width(app: &App) -> f32 {
    if app.title_bar > 0.0 {
        let buttons = crate::platform::window_buttons_width();
        // The buttons start about 19 px from the left edge; mirror that on the right.
        (buttons + 18.0).max(RAIL_MIN_WIDTH).round()
    } else {
        RAIL_MIN_WIDTH
    }
}
const ITEM_HEIGHT: f32 = 36.0;
/// Centre of the icon column and start of the label column, from the item's left edge.
const ICON_COLUMN: f32 = 20.0;
const LABEL_COLUMN: f32 = 40.0;

/// How the side menu is shown right now, or `None` when hidden.
pub fn mode(app: &App) -> Option<SidebarMode> {
    if app.fullscreen {
        return None;
    }
    match app.page {
        Page::Session(_) => match app.settings.sidebar {
            SidebarMode::Hidden => None,
            other => Some(other),
        },
        _ => Some(SidebarMode::Full),
    }
}

enum Action {
    Show(Page),
    New,
    Close(u64),
}

pub fn show(app: &mut App, ui: &mut Ui, mode: SidebarMode) {
    let p = Palette::of(ui.ctx());
    let rail = mode == SidebarMode::Rail;
    let mut action = None;
    let full = ui.max_rect();

    // The window buttons sit in the strip at the top; the app mark and name
    // go below it, lined up with the menu's icon and label columns.
    super::header::sidebar_strip(app, ui);
    ui.add_space(8.0);
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
    let icon_x = if rail { row.center().x } else { row.left() + ICON_COLUMN };
    let mark = Rect::from_center_size(pos2(icon_x, row.center().y), vec2(28.0, 28.0));
    ui.painter().rect_filled(mark, CornerRadius::same(8), p.accent);
    ui.painter().text(mark.center(), Align2::CENTER_CENTER, icon::MONITOR_PLAY, icon_font(17.0), Color32::WHITE);
    if !rail {
        let pos = pos2(row.left() + LABEL_COLUMN, row.center().y);
        ui.painter().text(pos, Align2::LEFT_CENTER, "Mini Remote", semibold(16.0), p.text);
    }
    ui.add_space(16.0);

    let page = app.page;
    let scanning = app.discovery.running;
    let found = app.discovery.hosts.iter().filter(|h| !h.vnc_ports.is_empty()).count();
    if item(ui, &p, rail, icon::SQUARES_FOUR, "Saved connections", page == Page::Saved, None).clicked() {
        action = Some(Action::Show(Page::Saved));
    }
    let editing_new = page == Page::Editor && app.editor.as_ref().is_some_and(|e| e.is_new);
    if item(ui, &p, rail, icon::PLUS_CIRCLE, "New connection", editing_new, None).clicked() {
        action = Some(Action::New);
    }
    let badge = if scanning { Some(Badge::Busy) } else if found > 0 { Some(Badge::Count(found)) } else { None };
    if item(ui, &p, rail, icon::BROADCAST, "Find devices", page == Page::Find, badge).clicked() {
        action = Some(Action::Show(Page::Find));
    }

    // Open connections.
    if !app.tabs.is_empty() {
        ui.add_space(14.0);
        if rail {
            let y = ui.cursor().top();
            ui.painter().hline(full.x_range().shrink(14.0), y, egui::Stroke::new(1.0, p.border));
            ui.add_space(8.0);
        } else {
            let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
            let pos = pos2(row.left() + LABEL_COLUMN, row.center().y);
            ui.painter().text(pos, Align2::LEFT_CENTER, "CONNECTED", medium(11.0), p.text_weak);
        }
        let bottom_reserve = 60.0;
        egui::ScrollArea::vertical()
            .id_salt("sidebar-sessions")
            .max_height((ui.available_height() - bottom_reserve).max(40.0))
            .show(ui, |ui| {
                for tab in &app.tabs {
                    let selected = page == Page::Session(tab.id);
                    let dot = status_color(&p, Some(&tab.status));
                    let tip = format!("{} · {}", tab.conn.address_label(), status_text(Some(&tab.status)));
                    let r = session_item(ui, &p, rail, tab.title(), dot, selected, &tip);
                    if r.close {
                        action = Some(Action::Close(tab.id));
                    } else if r.clicked {
                        action = Some(Action::Show(Page::Session(tab.id)));
                    }
                }
            });
    }

    // Settings at the bottom. The sidebar is resized from the session toolbar.
    let bottom = Rect::from_min_max(pos2(full.left(), full.bottom() - ITEM_HEIGHT - 6.0), full.max);
    super::widgets::in_rect(ui, bottom, egui::Layout::top_down(egui::Align::Min), |ui| {
        if item(ui, &p, rail, icon::GEAR_SIX, "Settings", page == Page::Settings, None).clicked() {
            action = Some(Action::Show(Page::Settings));
        }
    });

    match action {
        Some(Action::Show(page)) => app.show_page(page),
        Some(Action::New) => app.open_editor_new(),
        Some(Action::Close(id)) => app.close_tab(id),
        None => {}
    }
}

enum Badge {
    Busy,
    Count(usize),
}

/// One menu entry: icon and label, highlighted when selected.
fn item(ui: &mut Ui, p: &Palette, rail: bool, symbol: &str, label: &str, selected: bool, badge: Option<Badge>) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ITEM_HEIGHT), Sense::click());
    let response = if rail { response.on_hover_text(label) } else { response };
    if ui.is_rect_visible(rect) {
        let fill = if selected {
            p.accent_soft
        } else if response.hovered() {
            p.border.gamma_multiply(0.55)
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, CornerRadius::same(8), fill);
        let color = if selected { p.accent } else { p.text };
        let icon_x = if rail { rect.center().x } else { rect.left() + ICON_COLUMN };
        ui.painter().text(pos2(icon_x, rect.center().y), Align2::CENTER_CENTER, symbol, icon_font(18.0), color);
        if !rail {
            let text_color = if selected { p.accent } else { p.text };
            let pos = pos2(rect.left() + LABEL_COLUMN, rect.center().y);
            ui.painter().text(pos, Align2::LEFT_CENTER, label, medium(14.0), text_color);
        }
        match badge {
            Some(Badge::Busy) => {
                let spot = if rail { pos2(rect.right() - 12.0, rect.top() + 10.0) } else { pos2(rect.right() - 16.0, rect.center().y) };
                let spinner = Rect::from_center_size(spot, vec2(12.0, 12.0));
                egui::Spinner::new().size(12.0).color(p.text_weak).paint_at(ui, spinner);
            }
            Some(Badge::Count(n)) if !rail => {
                let text = n.to_string();
                let center = pos2(rect.right() - 18.0, rect.center().y);
                let pill = Rect::from_center_size(center, vec2(20.0 + 6.0 * (text.len() as f32 - 1.0), 18.0));
                ui.painter().rect_filled(pill, CornerRadius::same(9), p.accent);
                ui.painter().text(center, Align2::CENTER_CENTER, text, medium(11.0), Color32::WHITE);
            }
            Some(Badge::Count(_)) => {
                ui.painter().circle_filled(pos2(rect.right() - 12.0, rect.top() + 9.0), 4.0, p.accent);
            }
            None => {}
        }
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

struct SessionResponse {
    clicked: bool,
    close: bool,
}

/// An open connection: status dot, name, and a close button on hover.
fn session_item(ui: &mut Ui, p: &Palette, rail: bool, name: &str, dot: Color32, selected: bool, tip: &str) -> SessionResponse {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ITEM_HEIGHT), Sense::click());
    let close_rect = Rect::from_center_size(pos2(rect.right() - 16.0, rect.center().y), vec2(22.0, 22.0));
    let close = (!rail).then(|| ui.interact(close_rect, response.id.with("close"), Sense::click()));
    let hovered = response.hovered() || close.as_ref().is_some_and(|c| c.hovered());
    if ui.is_rect_visible(rect) {
        let fill = if selected {
            p.accent_soft
        } else if hovered {
            p.border.gamma_multiply(0.55)
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, CornerRadius::same(8), fill);
        if rail {
            // A round initial with the status dot on its corner.
            let center = rect.center();
            ui.painter().circle_filled(center, 13.0, if selected { p.accent } else { p.border });
            let initial: String = name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
            let color = if selected { Color32::WHITE } else { p.text };
            ui.painter().text(center, Align2::CENTER_CENTER, initial, medium(13.0), color);
            ui.painter().circle_filled(center + vec2(10.0, -10.0), 4.5, p.surface_alt);
            ui.painter().circle_filled(center + vec2(10.0, -10.0), 3.5, dot);
        } else {
            ui.painter().circle_filled(pos2(rect.left() + ICON_COLUMN, rect.center().y), 4.0, dot);
            let max_text = rect.width() - LABEL_COLUMN - 30.0;
            let color = if selected { p.accent } else { p.text };
            let mut job = LayoutJob::simple_singleline(name.to_string(), medium(14.0), color);
            job.wrap = TextWrapping::truncate_at_width(max_text);
            let galley = ui.painter().layout_job(job);
            ui.painter().galley(pos2(rect.left() + LABEL_COLUMN, rect.center().y - galley.size().y / 2.0), galley, color);
            if let Some(close) = &close
                && (hovered || selected)
            {
                if close.hovered() {
                    ui.painter().rect_filled(close_rect, CornerRadius::same(6), p.border);
                }
                ui.painter().text(close_rect.center(), Align2::CENTER_CENTER, icon::X, icon_font(12.0), p.text_weak);
            }
        }
    }
    let close_clicked = close.as_ref().is_some_and(|c| c.clicked());
    let close_tip = close.map(|c| c.on_hover_text("Disconnect"));
    let _ = close_tip;
    let response = response.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand);
    let middle = response.middle_clicked();
    SessionResponse { clicked: response.clicked() && !close_clicked, close: close_clicked || middle }
}
