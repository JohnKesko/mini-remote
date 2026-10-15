//! Everything the user sees. `show` draws one frame.

pub mod editor;
pub mod files_view;
pub mod header;
pub mod home;
pub mod overlays;
pub mod session_view;
pub mod settings;
pub mod sidebar;
pub mod theme;
pub mod toasts;
pub mod toolbar;
pub mod widgets;

use egui::{Frame, Margin, Ui};

use crate::app::{App, Page};
use crate::model::SidebarMode;
use crate::session::Status;
use theme::Palette;

pub use egui_phosphor::regular as icon;

/// Draw one frame of the whole window: the side menu and the current page.
pub fn show(app: &mut App, ui: &mut Ui) {
    app.update();
    let p = Palette::of(ui.ctx());

    let sidebar = sidebar::mode(app);
    if let Some(mode) = sidebar {
        let width = if mode == SidebarMode::Rail { sidebar::RAIL_WIDTH } else { sidebar::FULL_WIDTH };
        let margin = if mode == SidebarMode::Rail { 8 } else { 10 };
        egui::Panel::left("sidebar")
            .resizable(false)
            .exact_size(width)
            .frame(Frame::new().fill(p.surface_alt).inner_margin(Margin { left: margin, right: margin, top: 0, bottom: 10 }))
            .show(ui, |ui| sidebar::show(app, ui, mode));
    }

    if !header::hidden(app) {
        egui::Panel::top("header")
            .resizable(false)
            .exact_size(header::height(app))
            .frame(Frame::new().fill(p.bg))
            .show(ui, |ui| header::show(app, ui, sidebar));
    }

    egui::CentralPanel::no_frame().frame(Frame::new().fill(p.bg)).show(ui, |ui| match app.page {
        Page::Session(_) => match app.active_index() {
            Some(index) => session_view::show(app, ui, index),
            None => home::saved_page(app, ui),
        },
        Page::Saved => home::saved_page(app, ui),
        Page::Find => home::find_page(app, ui),
        Page::Editor => editor::page_ui(app, ui),
        Page::Settings => settings::page_ui(app, ui),
    });

    let ctx = ui.ctx().clone();
    home::delete_confirmation(app, &ctx);
    toasts::show(app, &ctx);
    app.after_ui();
}

/// Colour of the status dot for a connection.
pub fn status_color(p: &Palette, status: Option<&Status>) -> egui::Color32 {
    match status {
        Some(Status::Connected) => p.success,
        Some(Status::Connecting(_) | Status::Reconnecting { .. }) => p.warning,
        Some(Status::Failed(_)) => p.danger,
        Some(Status::Closed) | None => p.idle,
    }
}

/// Plain-English status for tooltips.
pub fn status_text(status: Option<&Status>) -> &'static str {
    match status {
        Some(Status::Connected) => "Connected",
        Some(Status::Connecting(_)) => "Connecting…",
        Some(Status::Reconnecting { .. }) => "Reconnecting…",
        Some(Status::Failed(_)) => "Not connected",
        Some(Status::Closed) | None => "Not connected",
    }
}
