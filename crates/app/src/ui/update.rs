//! The "Update available" window.

use egui::{Align, Id, Layout, RichText, ScrollArea};

use super::theme::{Palette, medium};
use super::widgets::{floating, primary_button, secondary_button};
use crate::app::App;
use crate::updater::Status;

pub fn dialog(app: &mut App, ctx: &egui::Context) {
    if !app.updater.dialog_open {
        return;
    }
    let p = Palette::of(ctx);
    let current = update::current_version();
    let open_sessions = !app.tabs.is_empty();
    let mut action = None;

    let modal = egui::Modal::new(Id::new("update-available")).frame(floating(&p)).show(ctx, |ui| {
        ui.set_width(440.0);
        match &app.updater.status {
            Status::Available(release) => {
                ui.label(RichText::new("A new version of Mini Remote is ready").heading());
                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!("Version {} is available. You have {current}.", release.version))
                        .color(p.text_weak),
                );
                if !release.notes.is_empty() {
                    ui.add_space(10.0);
                    ui.label(RichText::new("What's new").font(medium(13.0)));
                    ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                        ui.label(RichText::new(&release.notes).size(12.5).color(p.text_weak));
                    });
                }
                if open_sessions {
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("Mini Remote restarts to finish, so open connections will close.")
                            .size(12.5)
                            .color(p.text_weak),
                    );
                }
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    if ui.link(RichText::new("Skip this version").size(12.5)).clicked() {
                        action = Some(Action::Skip(release.version.clone()));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if primary_button(ui, "Update now").clicked() {
                            action = Some(Action::Update);
                        }
                        if secondary_button(ui, "Later").clicked() {
                            action = Some(Action::Close);
                        }
                    });
                });
            }
            Status::Downloading { release, done, total } => {
                ui.label(RichText::new(format!("Updating to version {}", release.version)).heading());
                ui.add_space(10.0);
                let fraction = if *total > 0 { *done as f32 / *total as f32 } else { 0.0 };
                ui.add(egui::ProgressBar::new(fraction).desired_height(8.0));
                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!("Downloading… {} of {}", megabytes(*done), megabytes(*total)))
                        .size(12.5)
                        .color(p.text_weak),
                );
                ui.add_space(14.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if secondary_button(ui, "Cancel").clicked() {
                        action = Some(Action::Cancel);
                    }
                });
            }
            Status::Restarting => {
                ui.label(RichText::new("Restarting Mini Remote…").heading());
                ui.add_space(14.0);
            }
            Status::Failed { message, page } => {
                ui.label(RichText::new("The update didn't work").heading());
                ui.add_space(4.0);
                ui.label(RichText::new(message).color(p.text_weak));
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    if let Some(page) = page
                        && ui.link(RichText::new("Open the release page").size(12.5)).clicked()
                    {
                        action = Some(Action::OpenPage(page.clone()));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if primary_button(ui, "Close").clicked() {
                            action = Some(Action::Close);
                        }
                    });
                });
            }
            Status::Idle | Status::Checking { .. } | Status::UpToDate => {
                action = Some(Action::Close);
            }
        }
    });

    // Clicking outside or pressing Esc is "Later", but never while downloading.
    if modal.should_close() && !app.updater.busy() && action.is_none() {
        action = Some(Action::Close);
    }
    match action {
        Some(Action::Update) => app.start_update(),
        Some(Action::Skip(version)) => {
            app.settings.skipped_version = Some(version);
            app.save_settings();
            app.updater.dismiss();
        }
        Some(Action::Cancel) => app.updater.cancel_download(),
        Some(Action::OpenPage(url)) => {
            crate::platform::open_url(&url);
        }
        Some(Action::Close) => app.updater.dismiss(),
        None => {}
    }
}

enum Action {
    Update,
    Skip(String),
    Cancel,
    OpenPage(String),
    Close,
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}
