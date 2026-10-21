//! The header strip at the top of the window, drawn by the app in its own
//! colours. On a Mac it sits under the transparent title bar, on the same
//! line as the window buttons; dragging it moves the window.

use egui::{Align, Layout, Rect, RichText, Sense, Ui, pos2, vec2};

use super::home::PAGE_MARGIN;
use super::theme::{Palette, medium, semibold};
use super::{icon, status_color, status_text};
use crate::app::{App, Page, WindowRequest};
use crate::model::SidebarMode;

/// Header height when there is no system title bar to line up with.
const OWN_HEIGHT: f32 = 44.0;

/// Height of the header strip.
pub fn height(app: &App) -> f32 {
    if app.title_bar > 0.0 { app.title_bar.max(28.0) } else { OWN_HEIGHT }
}

/// Height for buttons inside the header.
pub fn control_height(app: &App) -> f32 {
    (height(app) - 6.0).clamp(22.0, super::theme::SMALL_HEIGHT)
}

/// The window should hide its header (a remote screen in full screen).
pub fn hidden(app: &App) -> bool {
    app.fullscreen && matches!(app.page, Page::Session(_))
}

/// Let an empty part of the header move and zoom the window. Call this
/// before adding the header's own buttons, so they stay clickable.
pub fn drag_area(app: &mut App, ui: &Ui, rect: Rect, id: &str) {
    let response = ui.interact(rect, ui.id().with(id), Sense::click_and_drag());
    if response.drag_started_by(egui::PointerButton::Primary) {
        app.window_request(WindowRequest::Drag);
    }
    if response.double_clicked() {
        app.window_request(WindowRequest::ToggleZoom);
    }
}

enum Action {
    Rescan,
    NewConnection,
}

/// The content header: the page title (or the connection's name and status)
/// on the left, page actions on the right.
pub fn show(app: &mut App, ui: &mut Ui, sidebar: Option<SidebarMode>) {
    let p = Palette::of(ui.ctx());
    let rect = ui.max_rect();
    drag_area(app, ui, rect, "content-header");

    // Keep clear of the window buttons when the side menu does not cover them.
    let sidebar_width = match sidebar {
        Some(SidebarMode::Full) => super::sidebar::FULL_WIDTH,
        Some(SidebarMode::Rail) => super::sidebar::rail_width(app),
        _ => 0.0,
    };
    let buttons = if app.title_bar > 0.0 { crate::platform::window_buttons_width() } else { 0.0 };
    let in_session = matches!(app.page, Page::Session(_));
    let left = (buttons - sidebar_width).max(0.0) + if in_session { 16.0 } else { PAGE_MARGIN };
    let right = if in_session { 16.0 } else { PAGE_MARGIN };
    let inner = Rect::from_min_max(pos2(rect.left() + left, rect.top()), pos2(rect.right() - right, rect.bottom()));
    let control = control_height(app);

    let mut action = None;
    super::widgets::in_rect(ui, inner, Layout::left_to_right(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.spacing_mut().interact_size.y = control;
        match app.page {
            Page::Session(_) => {
                if let Some(index) = app.active_index() {
                    let tab = &app.tabs[index];
                    let color = status_color(&p, Some(&tab.status));
                    let (dot, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                    ui.painter().circle_filled(dot.center(), 4.0, color);
                    header_text(ui, tab.title().to_string(), semibold(14.0), p.text);
                    let detail = format!("{} · {}", tab.conn.address_label(), status_text(Some(&tab.status)));
                    header_text(ui, detail, medium(12.5), p.text_weak);
                }
            }
            page => {
                header_text(ui, title(app, page).to_string(), semibold(15.0), p.text);
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| match app.page {
            Page::Saved => {
                if super::widgets::primary_header_button(ui, icon::PLUS, "New connection", control).clicked() {
                    action = Some(Action::NewConnection);
                }
            }
            Page::Find => {
                ui.add_enabled_ui(!app.discovery.running, |ui| {
                    if small_button(ui, icon::ARROW_CLOCKWISE, "Look again", control).clicked() {
                        action = Some(Action::Rescan);
                    }
                });
                if app.discovery.running {
                    ui.add(egui::Spinner::new().size(14.0).color(p.text_weak));
                    ui.label(RichText::new("Looking…").font(medium(12.5)).color(p.text_weak));
                }
            }
            _ => {}
        });
    });

    match action {
        Some(Action::Rescan) => app.start_discovery(true),
        Some(Action::NewConnection) => app.open_editor_new(),
        None => {}
    }
}

fn title(app: &App, page: Page) -> &'static str {
    match page {
        Page::Saved => "Saved connections",
        Page::Find => "Find devices",
        Page::Settings => "Settings",
        Page::Editor => {
            if app.editor.as_ref().is_some_and(|e| !e.is_new) {
                "Edit connection"
            } else {
                "New connection"
            }
        }
        Page::Session(_) => "",
    }
}

/// Text in the header, centred on the same line as the window buttons (drawn
/// like the side menu's labels, so every text in the header lines up).
fn header_text(ui: &mut Ui, text: String, font: egui::FontId, color: egui::Color32) {
    let galley = ui.painter().layout_no_wrap(text, font, color);
    let (rect, _) = ui.allocate_exact_size(vec2(galley.size().x, ui.available_height()), Sense::hover());
    super::widgets::paint_left_center(ui, galley, rect.left(), rect.center().y, color);
}

/// A compact button with an icon, sized for the header.
fn small_button(ui: &mut Ui, symbol: &str, text: &str, height: f32) -> egui::Response {
    super::widgets::secondary_icon_button(ui, symbol, text, height)
}

/// The top of the side menu: empty (the window buttons sit here) and draggable.
pub fn sidebar_strip(app: &mut App, ui: &mut Ui) {
    let h = height(app);
    let full = ui.max_rect();
    let rect = Rect::from_min_size(pos2(full.left() - 16.0, full.top()), vec2(full.width() + 32.0, h));
    ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    drag_area(app, ui, rect, "sidebar-header");
}
