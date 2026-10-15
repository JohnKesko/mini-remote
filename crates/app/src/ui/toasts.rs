//! Short messages at the bottom of the window.

use egui::{Align2, Area, CornerRadius, Id, Margin, Order, RichText, pos2};

use super::icon;
use super::theme::Palette;
use super::widgets::{floating, icon_button};
use crate::app::{App, ToastAction};

enum Action {
    Dismiss(u64),
    Reveal(std::path::PathBuf, u64),
    Save(u64, u64),
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    if app.toasts.is_empty() {
        return;
    }
    let p = Palette::of(ctx);
    let screen = ctx.content_rect();
    let mut action = None;
    Area::new(Id::new("toasts"))
        .order(Order::Tooltip)
        .pivot(Align2::CENTER_BOTTOM)
        .fixed_pos(pos2(screen.center().x, screen.bottom() - 20.0))
        .show(ctx, |ui| {
            for toast in &app.toasts {
                floating(&p).inner_margin(Margin::symmetric(14, 8)).corner_radius(CornerRadius::same(12)).show(ui, |ui| {
                    ui.set_max_width(520.0);
                    ui.horizontal(|ui| {
                        ui.add(egui::Label::new(RichText::new(&toast.text)).wrap());
                        match &toast.action {
                            Some(ToastAction::Reveal(path)) => {
                                if ui.button(crate::platform::reveal_label()).clicked() {
                                    action = Some(Action::Reveal(path.clone(), toast.id));
                                }
                            }
                            Some(ToastAction::SaveConnection(tab)) => {
                                if ui.button(RichText::new("Save").strong().color(p.accent)).clicked() {
                                    action = Some(Action::Save(*tab, toast.id));
                                }
                            }
                            None => {}
                        }
                        if icon_button(ui, icon::X, "Dismiss").clicked() {
                            action = Some(Action::Dismiss(toast.id));
                        }
                    });
                });
                ui.add_space(6.0);
            }
        });
    match action {
        Some(Action::Dismiss(id)) => app.toasts.retain(|t| t.id != id),
        Some(Action::Reveal(path, id)) => {
            crate::platform::reveal_in_file_manager(&path);
            app.toasts.retain(|t| t.id != id);
        }
        Some(Action::Save(tab, id)) => {
            app.toasts.retain(|t| t.id != id);
            app.save_tab_connection(tab);
        }
        None => {}
    }
}
