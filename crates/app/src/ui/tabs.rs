//! The tab bar: Home plus one tab per open connection.

use egui::text::{LayoutJob, TextWrapping};
use egui::{Align, Color32, CornerRadius, FontId, Layout, Rect, Sense, Stroke, Ui, pos2, vec2};

use super::theme::Palette;
use super::widgets::icon_button;
use super::{icon, status_color, status_text};
use crate::app::App;

const TAB_HEIGHT: f32 = 38.0;
const TAB_MAX_WIDTH: f32 = 220.0;

enum TabAction {
    Activate(Option<u64>),
    Close(u64),
    Settings,
}

pub fn tab_bar(app: &mut App, ui: &mut Ui) {
    let p = Palette::of(ui.ctx());
    let mut action = None;

    ui.with_layout(Layout::left_to_right(Align::Max), |ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let home = tab(ui, &p, "home", TabLook { icon: Some(icon::HOUSE), dot: None, label: "Home", tooltip: None, active: app.active.is_none(), closable: false });
        if home.clicked {
            action = Some(TabAction::Activate(None));
        }
        for t in &app.tabs {
            let look = TabLook {
                icon: None,
                dot: Some(status_color(&p, Some(&t.status))),
                label: t.title(),
                tooltip: Some(format!("{} · {}", t.conn.address_label(), status_text(Some(&t.status)))),
                active: app.active == Some(t.id),
                closable: true,
            };
            let r = tab(ui, &p, t.id, look);
            if r.close {
                action = Some(TabAction::Close(t.id));
            } else if r.clicked {
                action = Some(TabAction::Activate(Some(t.id)));
            }
        }
        ui.add_space(2.0);
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            if icon_button(ui, icon::PLUS, "New connection").clicked() {
                action = Some(TabAction::Activate(None));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if icon_button(ui, icon::GEAR_SIX, "Settings").clicked() {
                    action = Some(TabAction::Settings);
                }
            });
        });
    });

    match action {
        Some(TabAction::Activate(id)) => {
            app.activate(id);
            if id.is_none() {
                app.home.focus_quick_connect = true;
            }
        }
        Some(TabAction::Close(id)) => app.close_tab(id),
        Some(TabAction::Settings) => app.settings_open = true,
        None => {}
    }
}

struct TabLook<'a> {
    icon: Option<&'a str>,
    dot: Option<Color32>,
    label: &'a str,
    tooltip: Option<String>,
    active: bool,
    closable: bool,
}

struct TabResponse {
    clicked: bool,
    close: bool,
}

fn tab(ui: &mut Ui, p: &Palette, id: impl std::hash::Hash + std::fmt::Debug, look: TabLook) -> TabResponse {
    let font = FontId::proportional(13.5);
    let text_color = if look.active { p.text } else { p.text_weak };
    let lead = if look.icon.is_some() || look.dot.is_some() { 22.0 } else { 0.0 };
    let close_w = if look.closable { 22.0 } else { 0.0 };
    let padding = 12.0;
    let max_text = TAB_MAX_WIDTH - lead - close_w - padding * 2.0;

    let mut job = LayoutJob::simple_singleline(look.label.to_string(), font.clone(), text_color);
    job.wrap = TextWrapping::truncate_at_width(max_text);
    let galley = ui.painter().layout_job(job);
    let width = (padding * 2.0 + lead + galley.size().x + close_w).max(64.0);

    let (rect, response) = ui.allocate_exact_size(vec2(width, TAB_HEIGHT), Sense::click());
    let id = ui.id().with(("tab", id));
    let close_rect = Rect::from_center_size(pos2(rect.right() - padding - 7.0, rect.center().y), vec2(20.0, 20.0));
    let close = look.closable.then(|| ui.interact(close_rect, id.with("close"), Sense::click()));

    if ui.is_rect_visible(rect) {
        let radius = CornerRadius { nw: 10, ne: 10, sw: 0, se: 0 };
        if look.active {
            ui.painter().rect(rect, radius, p.bg, Stroke::NONE, egui::StrokeKind::Inside);
        } else if response.hovered() {
            ui.painter().rect_filled(rect.shrink2(vec2(0.0, 3.0)).translate(vec2(0.0, -2.0)), CornerRadius::same(8), p.border.gamma_multiply(0.5));
        }
        let mut x = rect.left() + padding;
        if let Some(icon) = look.icon {
            ui.painter().text(pos2(x + 8.0, rect.center().y), egui::Align2::CENTER_CENTER, icon, FontId::proportional(16.0), text_color);
            x += lead;
        } else if let Some(color) = look.dot {
            ui.painter().circle_filled(pos2(x + 5.0, rect.center().y), 4.0, color);
            x += lead - 6.0;
        }
        ui.painter().galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, text_color);

        if let Some(close) = &close {
            let show = look.active || response.hovered() || close.hovered();
            if show {
                if close.hovered() {
                    ui.painter().rect_filled(close_rect, CornerRadius::same(6), p.border);
                }
                ui.painter().text(close_rect.center(), egui::Align2::CENTER_CENTER, icon::X, FontId::proportional(12.0), p.text_weak);
            }
        }
    }

    let close_clicked = close.as_ref().is_some_and(|c| c.clicked());
    let response = match look.tooltip {
        Some(tip) => response.on_hover_text(tip),
        None => response,
    };
    // A middle click closes the tab, as in web browsers.
    let middle = look.closable && response.middle_clicked();
    TabResponse { clicked: response.clicked() && !close_clicked, close: close_clicked || middle }
}
