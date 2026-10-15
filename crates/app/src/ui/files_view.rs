//! The Files drawer on the right side of a session: browse the Pi's folders
//! and copy files both ways.

use egui::{
    Align, Align2, Area, CornerRadius, Id, Key, Layout, Margin, Order, Rect, RichText, Ui,
    pos2,
};
use ssh::RemoteEntry;

use super::icon;
use super::theme::Palette;
use super::widgets::{danger_button, floating, hint_text, icon_button, primary_button, secondary_button, text_field};
use crate::app::App;
use crate::files::{FilesConn, SSH_OFF_HINT, breadcrumbs, format_size};

const WIDTH: f32 = 380.0;

enum Action {
    Close,
    Retry,
    Login,
    Open(String),
    Up,
    Refresh,
    Select(String),
    Upload,
    Download(RemoteEntry),
    StartNewFolder,
    NewFolder(String),
    StartRename(RemoteEntry),
    Rename(String, String),
    CancelEdit,
    AskDelete(RemoteEntry),
    Delete(RemoteEntry),
    CancelDelete,
    CancelTransfer(u64),
    Reveal(std::path::PathBuf),
}

pub fn show(app: &mut App, ctx: &egui::Context, index: usize, area: Rect) {
    if !app.tabs[index].files.open {
        return;
    }
    let p = Palette::of(ctx);
    let name = app.tabs[index].title().to_string();
    let tab_id = app.tabs[index].id;
    let files = &mut app.tabs[index].files;
    let mut actions: Vec<Action> = Vec::new();
    let height = (area.height() - 110.0).max(240.0);
    let hovering_files = ctx.input(|i| !i.raw.hovered_files.is_empty());

    Area::new(Id::new(("files", tab_id)))
        .order(Order::Foreground)
        .pivot(Align2::RIGHT_TOP)
        .fixed_pos(pos2(area.right() - 12.0, area.top() + 70.0))
        .show(ctx, |ui| {
            floating(&p).inner_margin(Margin::same(14)).show(ui, |ui| {
                ui.set_width(WIDTH);
                ui.set_height(height);
                super::widgets::row(ui, 32.0, |ui| {
                    let (mark, _) = ui.allocate_exact_size(egui::vec2(32.0, 32.0), egui::Sense::hover());
                    let center = super::widgets::snap(ui, mark.center());
                    ui.painter().text(center, Align2::CENTER_CENTER, icon::FOLDER_OPEN, super::theme::icon_font(19.0), p.accent);
                    let title = super::widgets::truncated(ui, &format!("Files on {name}"), super::theme::semibold(15.0), p.text, WIDTH - 32.0 - 32.0 - 24.0);
                    // Text column shared with the list below (see `list_row`).
                    let x = mark.left() + super::widgets::LIST_TEXT_COLUMN;
                    super::widgets::paint_left_center(ui, title, x, mark.center().y, p.text);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if icon_button(ui, icon::X, "Close").clicked() {
                            actions.push(Action::Close);
                        }
                    });
                });
                ui.add_space(8.0);

                match &files.conn {
                    FilesConn::Idle | FilesConn::Connecting => {
                        ui.add_space(20.0);
                        ui.horizontal(|ui| {
                            ui.add(egui::Spinner::new().size(18.0).color(p.accent));
                            ui.label("Connecting to the Pi's files…");
                        });
                    }
                    FilesConn::NeedLogin => {
                        hint_text(ui, "Log in to the Pi's secure remote login (SSH) to copy files. This is usually the same username and password you use on the Pi.");
                        if let Some(err) = &files.login.error {
                            super::widgets::error_line(ui, err);
                        }
                        ui.add_space(4.0);
                        super::widgets::field_label(ui, "Username");
                        ui.add(text_field(&mut files.login.username, "pi"));
                        super::widgets::field_label(ui, "Password");
                        let r = ui.add(text_field(&mut files.login.password, "").password(true));
                        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                        ui.add_space(8.0);
                        if primary_button(ui, "Log in").clicked() || enter {
                            actions.push(Action::Login);
                        }
                    }
                    FilesConn::Failed { message, ssh_off } => {
                        ui.add_space(8.0);
                        super::widgets::error_line(ui, message);
                        if *ssh_off {
                            ui.add_space(4.0);
                            hint_text(ui, SSH_OFF_HINT);
                        }
                        ui.add_space(8.0);
                        if secondary_button(ui, "Try again").clicked() {
                            actions.push(Action::Retry);
                        }
                    }
                    FilesConn::Ready(_) => ready(ui, &p, files, hovering_files, &mut actions),
                }
            });
        });

    if let Some(entry) = files.confirm_delete.clone() {
        let modal = egui::Modal::new(Id::new("files-delete")).frame(floating(&p)).show(ctx, |ui| {
            ui.set_width(340.0);
            let what = if entry.is_dir { "folder" } else { "file" };
            ui.label(RichText::new(format!("Delete “{}”?", entry.name)).heading().strong());
            ui.add_space(4.0);
            let more = if entry.is_dir { " and everything in it" } else { "" };
            ui.label(RichText::new(format!("The {what}{more} will be removed from the Pi. This can't be undone.")).color(p.text_weak));
            ui.add_space(12.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if danger_button(ui, "Delete").clicked() {
                    actions.push(Action::Delete(entry.clone()));
                }
                if secondary_button(ui, "Cancel").clicked() {
                    actions.push(Action::CancelDelete);
                }
            });
        });
        if modal.should_close() {
            actions.push(Action::CancelDelete);
        }
    }

    for action in actions {
        apply(app, index, action);
    }
}

fn ready(ui: &mut Ui, p: &Palette, files: &mut crate::files::FilesPanel, hovering: bool, actions: &mut Vec<Action>) {
    // Where we are.
    super::widgets::row(ui, 32.0, |ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.spacing_mut().button_padding.x = super::widgets::LIST_TEXT_COLUMN - 32.0;
        ui.add_enabled_ui(crate::files::parent_dir(&files.cwd).is_some(), |ui| {
            if icon_button(ui, icon::ARROW_UP, "Up one folder").clicked() {
                actions.push(Action::Up);
            }
        });
        let crumbs = breadcrumbs(&files.cwd, &files.home);
        let last = crumbs.len().saturating_sub(1);
        egui::ScrollArea::horizontal().id_salt("crumbs").max_width(WIDTH - 80.0).stick_to_right(true).show(ui, |ui| {
            ui.horizontal(|ui| {
                for (i, (label, path)) in crumbs.into_iter().enumerate() {
                    if i > 0 {
                        ui.label(super::theme::ic(icon::CARET_RIGHT).size(11.0).color(p.text_weak));
                    }
                    let text = if i == last { RichText::new(label).strong() } else { RichText::new(label).color(p.text_weak) };
                    if ui.add(egui::Button::new(text).frame_when_inactive(false).small()).clicked() && i != last {
                        actions.push(Action::Open(path));
                    }
                }
            });
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if files.loading {
                ui.add(egui::Spinner::new().size(14.0));
            } else if icon_button(ui, icon::ARROW_CLOCKWISE, "Refresh").clicked() {
                actions.push(Action::Refresh);
            }
        });
    });

    // What can be done.
    ui.add_space(8.0);
    let selected = files.selected.as_ref().and_then(|s| files.entries.iter().find(|e| &e.path == s)).cloned();
    super::widgets::row(ui, 32.0, |ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if super::widgets::primary_icon_button(ui, icon::UPLOAD_SIMPLE, "Upload").on_hover_text("Copy files from this computer to the Pi").clicked() {
            actions.push(Action::Upload);
        }
        ui.add_enabled_ui(selected.is_some(), |ui| {
            if icon_button(ui, icon::DOWNLOAD_SIMPLE, "Copy to this computer").clicked()
                && let Some(e) = &selected
            {
                actions.push(Action::Download(e.clone()));
            }
            if icon_button(ui, icon::PENCIL_SIMPLE, "Rename").clicked()
                && let Some(e) = &selected
            {
                actions.push(Action::StartRename(e.clone()));
            }
            if icon_button(ui, icon::TRASH, "Delete").clicked()
                && let Some(e) = &selected
            {
                actions.push(Action::AskDelete(e.clone()));
            }
        });
        if icon_button(ui, icon::FOLDER_PLUS, "New folder").clicked() {
            actions.push(Action::StartNewFolder);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let tip = if files.show_hidden { "Hide hidden files" } else { "Show hidden files" };
            if super::widgets::icon_toggle(ui, icon::EYE, tip, files.show_hidden).clicked() {
                files.show_hidden = !files.show_hidden;
            }
        });
    });

    // Typing a name for a new folder or a rename.
    if let Some(name) = &mut files.new_folder {
        name_row(ui, name, "New folder name", Action::NewFolder, actions);
    } else if let Some((path, name)) = &mut files.rename {
        let path = path.clone();
        name_row(ui, name, "New name", move |n| Action::Rename(path, n), actions);
    }
    if let Some(err) = &files.error {
        super::widgets::error_line(ui, err);
    }
    ui.add_space(2.0);

    // The list.
    let transfers_height = files.transfers.len() as f32 * 46.0;
    let list_height = (ui.available_height() - transfers_height - 30.0).max(80.0);
    egui::Frame::new()
        .fill(if hovering { p.accent_soft } else { p.surface_alt })
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(0, 4))
        .show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("files-list").max_height(list_height).auto_shrink([false, false]).show(ui, |ui| {
                if files.entries.is_empty() && !files.loading {
                    ui.add_space(12.0);
                    ui.vertical_centered(|ui| hint_text(ui, "This folder is empty."));
                }
                let show_hidden = files.show_hidden;
                for entry in files.entries.iter().filter(|e| show_hidden || !e.name.starts_with('.')) {
                    let selected = files.selected.as_deref() == Some(entry.path.as_str());
                    let r = row(ui, p, entry, selected);
                    if r.double_clicked() {
                        if entry.is_dir {
                            actions.push(Action::Open(entry.path.clone()));
                        } else {
                            actions.push(Action::Download(entry.clone()));
                        }
                    } else if r.clicked() {
                        actions.push(Action::Select(entry.path.clone()));
                    }
                    r.context_menu(|ui| {
                        if entry.is_dir && super::widgets::menu_item(ui, icon::FOLDER_OPEN, "Open").clicked() {
                            actions.push(Action::Open(entry.path.clone()));
                        }
                        if super::widgets::menu_item(ui, icon::DOWNLOAD_SIMPLE, "Download").clicked() {
                            actions.push(Action::Download(entry.clone()));
                        }
                        if super::widgets::menu_item(ui, icon::PENCIL_SIMPLE, "Rename").clicked() {
                            actions.push(Action::StartRename(entry.clone()));
                        }
                        if super::widgets::danger_menu_item(ui, icon::TRASH, "Delete").clicked() {
                            actions.push(Action::AskDelete(entry.clone()));
                        }
                    });
                }
            });
        });
    hint_text(ui, if hovering { "Drop to copy into this folder" } else { "Tip: drag files from Finder here or onto the Pi's screen." });

    // Copies in progress.
    for t in &files.transfers {
        ui.horizontal(|ui| {
            let arrow = if t.upload { icon::UPLOAD_SIMPLE } else { icon::DOWNLOAD_SIMPLE };
            ui.label(super::theme::ic(arrow).color(p.text_weak));
            ui.vertical(|ui| {
                ui.set_width(WIDTH - 70.0);
                ui.spacing_mut().item_spacing.y = 2.0;
                let (done, total) = t.bytes();
                let status = match &t.result {
                    None if total > 0 => format!("{} of {}", format_size(done), format_size(total)),
                    None => "Starting…".into(),
                    Some(Ok(())) => "Done".into(),
                    Some(Err(e)) => e.clone(),
                };
                ui.horizontal(|ui| {
                    ui.add(egui::Label::new(RichText::new(&t.label).size(12.5)).truncate());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let color = if matches!(t.result, Some(Err(_))) { p.danger } else { p.text_weak };
                        ui.label(RichText::new(status).size(11.5).color(color));
                    });
                });
                let fraction = if matches!(t.result, Some(Ok(()))) { 1.0 } else { t.fraction() };
                ui.add(egui::ProgressBar::new(fraction).desired_height(5.0).fill(p.accent).corner_radius(CornerRadius::same(3)));
            });
            match (&t.result, &t.saved_to) {
                (None, _) => {
                    if icon_button(ui, icon::X, "Cancel").clicked() {
                        actions.push(Action::CancelTransfer(t.id));
                    }
                }
                (Some(Ok(())), Some(path)) => {
                    if icon_button(ui, icon::MAGNIFYING_GLASS, crate::platform::reveal_label()).clicked() {
                        actions.push(Action::Reveal(path.clone()));
                    }
                }
                _ => {}
            }
        });
    }
}

fn name_row(ui: &mut Ui, name: &mut String, hint: &str, done: impl FnOnce(String) -> Action, actions: &mut Vec<Action>) {
    ui.add_space(8.0);
    super::widgets::row(ui, 32.0, |ui| {
        let r = ui.add(text_field(name, hint).desired_width(WIDTH - 110.0));
        if !r.has_focus() && !r.lost_focus() && name.is_empty() {
            r.request_focus();
        }
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        let escape = ui.input(|i| i.key_pressed(Key::Escape));
        if ui.button("OK").clicked() || enter {
            actions.push(done(name.clone()));
        } else if icon_button(ui, icon::X, "Cancel").clicked() || escape {
            actions.push(Action::CancelEdit);
        }
    });
}

fn row(ui: &mut Ui, p: &Palette, entry: &RemoteEntry, selected: bool) -> egui::Response {
    let size = if entry.is_dir { String::new() } else { format_size(entry.size) };
    let date = entry.modified.map(format_date).unwrap_or_default();
    let detail = match (size.is_empty(), date.is_empty()) {
        (true, _) => date,
        (false, true) => size,
        (false, false) => format!("{size} · {date}"),
    };
    let icon_color = if entry.is_dir { p.accent } else { p.text_weak };
    super::widgets::list_row(ui, file_icon(entry), icon_color, &entry.name, &detail, selected)
}

fn file_icon(entry: &RemoteEntry) -> &'static str {
    if entry.is_dir {
        return icon::FOLDER;
    }
    let ext = entry.name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg" => icon::FILE_IMAGE,
        "mp4" | "mov" | "mkv" | "avi" | "h264" => icon::FILE_VIDEO,
        "mp3" | "wav" | "flac" | "ogg" => icon::FILE_AUDIO,
        "zip" | "gz" | "tgz" | "xz" | "bz2" | "tar" | "7z" => icon::FILE_ZIP,
        "py" | "sh" | "rs" | "c" | "h" | "cpp" | "js" | "json" | "toml" | "yaml" | "yml" | "html" | "css" => icon::FILE_CODE,
        "txt" | "md" | "log" | "csv" | "conf" | "cfg" | "ini" => icon::FILE_TEXT,
        _ => icon::FILE,
    }
}

fn format_date(t: std::time::SystemTime) -> String {
    let dt: chrono::DateTime<chrono::Local> = t.into();
    let now = chrono::Local::now();
    if dt.date_naive() == now.date_naive() {
        dt.format("%H:%M").to_string()
    } else if dt.format("%Y").to_string() == now.format("%Y").to_string() {
        dt.format("%-d %b").to_string()
    } else {
        dt.format("%-d %b %Y").to_string()
    }
}

fn apply(app: &mut App, index: usize, action: Action) {
    match action {
        Action::Close => app.tabs[index].files.open = false,
        Action::Retry => {
            app.tabs[index].files.reset();
            app.connect_files(index);
        }
        Action::Login => {
            let files = &mut app.tabs[index].files;
            if files.login.username.trim().is_empty() {
                files.login.error = Some("Please type the Pi's username.".into());
            } else {
                files.use_login_form();
                app.connect_files(index);
            }
        }
        Action::Open(path) => app.with_files(index, |f, ctx| f.list(ctx, path)),
        Action::Up => app.with_files(index, |f, ctx| f.go_up(ctx)),
        Action::Refresh => app.with_files(index, |f, ctx| f.refresh(ctx)),
        Action::Select(path) => app.tabs[index].files.selected = Some(path),
        Action::Upload => app.with_files(index, |f, ctx| f.pick_upload(ctx)),
        Action::Download(entry) => {
            let folder = app.settings.download_folder();
            app.with_files(index, |f, ctx| f.download(ctx, &entry, folder));
        }
        Action::StartNewFolder => {
            let f = &mut app.tabs[index].files;
            f.rename = None;
            f.new_folder = Some(String::new());
        }
        Action::NewFolder(name) => app.with_files(index, |f, ctx| f.make_folder(ctx, &name)),
        Action::StartRename(entry) => {
            let f = &mut app.tabs[index].files;
            f.new_folder = None;
            f.selected = Some(entry.path.clone());
            f.rename = Some((entry.path, entry.name));
        }
        Action::Rename(path, name) => app.with_files(index, |f, ctx| f.rename_to(ctx, path, &name)),
        Action::CancelEdit => {
            let f = &mut app.tabs[index].files;
            f.rename = None;
            f.new_folder = None;
        }
        Action::AskDelete(entry) => app.tabs[index].files.confirm_delete = Some(entry),
        Action::Delete(entry) => {
            app.tabs[index].files.confirm_delete = None;
            app.with_files(index, |f, ctx| f.delete(ctx, entry));
        }
        Action::CancelDelete => app.tabs[index].files.confirm_delete = None,
        Action::CancelTransfer(id) => app.tabs[index].files.cancel(id),
        Action::Reveal(path) => crate::platform::reveal_in_file_manager(&path),
    }
}
