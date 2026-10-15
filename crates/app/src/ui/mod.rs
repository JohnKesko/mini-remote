//! Everything the user sees. `show` draws one frame.

pub mod editor;
pub mod files_view;
pub mod home;
pub mod overlays;
pub mod session_view;
pub mod settings;
pub mod tabs;
pub mod theme;
pub mod toasts;
pub mod toolbar;
pub mod widgets;

use egui::{Frame, Margin, Ui};

use crate::app::App;
use crate::session::Status;
use theme::Palette;

pub use egui_phosphor::regular as icon;

/// Draw one frame of the whole window.
pub fn show(app: &mut App, ui: &mut Ui) {
    app.update();
    let p = Palette::of(ui.ctx());

    if !app.fullscreen {
        egui::Panel::top("tab-bar")
            .frame(Frame::new().fill(p.surface_alt).inner_margin(Margin { left: 8, right: 8, top: 6, bottom: 0 }))
            .show_separator_line(false)
            .exact_size(44.0)
            .show(ui, |ui| tabs::tab_bar(app, ui));
    }

    egui::CentralPanel::no_frame().frame(Frame::new().fill(p.bg)).show(ui, |ui| match app.active_index() {
        Some(index) => session_view::show(app, ui, index),
        None => home::show(app, ui),
    });

    let ctx = ui.ctx().clone();
    editor::show(app, &ctx);
    settings::show(app, &ctx);
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
