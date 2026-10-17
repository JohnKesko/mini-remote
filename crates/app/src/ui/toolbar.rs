//! The slim floating toolbar at the top of a session, and the connection info panel.

use std::time::{Duration, Instant};

use egui::containers::menu::MenuButton;
use egui::{Align2, Area, CornerRadius, Id, Margin, Order, Rect, RichText, Ui, pos2, vec2};
use rfb::Quality;

use super::icon;
use super::theme::Palette;
use super::widgets::{floating, icon_button, icon_toggle};
use crate::app::App;
use crate::input::keymap::{sc, xk};
use crate::model::Scaling;

/// How long the toolbar stays after the mouse moves away.
const LINGER: Duration = Duration::from_millis(1400);

type Combo = &'static [(u32, u32)];

/// Key combinations offered in the "Send keys" menu.
const COMBOS: &[(&str, Combo)] = &[
    ("Ctrl + Alt + Delete", &[(xk::CONTROL_L, sc::CONTROL_L), (xk::ALT_L, sc::ALT_L), (xk::DELETE, sc::DELETE)]),
    ("Ctrl + Esc", &[(xk::CONTROL_L, sc::CONTROL_L), (xk::ESCAPE, sc::ESCAPE)]),
    ("Alt + Tab", &[(xk::ALT_L, sc::ALT_L), (xk::TAB, sc::TAB)]),
    ("Alt + F4", &[(xk::ALT_L, sc::ALT_L), (xk::F4, sc::F4)]),
    ("Windows key (Pi menu)", &[(xk::SUPER_L, sc::SUPER_L)]),
    ("Print Screen", &[(xk::PRINT, sc::PRINT)]),
    ("Ctrl + Alt + F1", &[(xk::CONTROL_L, sc::CONTROL_L), (xk::ALT_L, sc::ALT_L), (xk::F1, sc::F1)]),
];

enum Action {
    Sidebar,
    ToggleScaling,
    FullScreen,
    Combo(Combo),
    PushClipboard,
    TypeClipboard,
    ShareClipboard(bool),
    Quality(Quality),
    Video(bool),
    Screenshot,
    Refresh,
    Info,
    Files,
    Disconnect,
}

pub fn show(app: &mut App, ui: &mut Ui, index: usize, area: Rect) {
    let ctx = ui.ctx().clone();
    let p = Palette::of(&ctx);
    let fullscreen = app.fullscreen;
    // Debug builds: `MINI_REMOTE_DEBUG_OPEN=...,disconnect` presses Disconnect
    // here, in the middle of drawing, exactly like a real click.
    let debug_press = cfg!(debug_assertions) && app.debug_disconnect_at.is_some_and(|t| Instant::now() >= t);
    if debug_press {
        app.debug_disconnect_at = None;
    }
    let tab = &mut app.tabs[index];
    let now = Instant::now();
    if debug_press {
        tab.toolbar_seen = Some(now);
    }
    let pointer = ctx.input(|i| i.pointer.latest_pos());
    let rect_id = Id::new(("toolbar-rect", tab.id));
    let last_rect: Option<Rect> = ctx.data(|d| d.get_temp(rect_id));

    let reveal_zone = if fullscreen { 4.0 } else { 72.0 };
    let near_top = pointer.is_some_and(|pos| area.contains(pos) && pos.y <= area.top() + reveal_zone);
    let over_bar = pointer.is_some_and(|pos| last_rect.is_some_and(|r| r.expand(16.0).contains(pos)));
    let busy = ctx.any_popup_open() || tab.info_open || tab.files.open || !tab.status.is_connected();
    if near_top || busy || (over_bar && tab.toolbar_seen.is_some()) {
        tab.toolbar_seen = Some(now);
    }
    let visible = tab.toolbar_seen.is_some_and(|t| now.duration_since(t) < LINGER);
    if let Some(t) = tab.toolbar_seen
        && visible
    {
        ctx.request_repaint_after(LINGER.saturating_sub(now.duration_since(t)) + Duration::from_millis(20));
    }
    let opacity = ctx.animate_bool_with_time(Id::new(("toolbar-fade", tab.id)), visible, 0.18);
    if opacity <= 0.01 {
        ctx.data_mut(|d| d.remove::<Rect>(rect_id));
        return;
    }

    let mut action = None;
    let connected = tab.status.is_connected();
    let response = Area::new(Id::new(("toolbar", tab.id)))
        .order(Order::Foreground)
        .pivot(Align2::CENTER_TOP)
        .fixed_pos(pos2(area.center().x, area.top() + 10.0))
        .show(&ctx, |ui| {
            ui.multiply_opacity(opacity);
            floating(&p).inner_margin(Margin::symmetric(8, 5)).corner_radius(CornerRadius::same(12)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    if !fullscreen {
                        let tip = format!("Show or hide the menu ({})", crate::platform::shortcut_label("S"));
                        if icon_button(ui, icon::SIDEBAR_SIMPLE, &tip).clicked() {
                            action = Some(Action::Sidebar);
                        }
                        separator(ui, &p);
                    }
                    let fit = tab.conn.scaling == Scaling::Fit;
                    let (scale_icon, scale_tip) = if fit {
                        (icon::ARROWS_OUT_SIMPLE, "Show at actual size")
                    } else {
                        (icon::ARROWS_IN_SIMPLE, "Fit to window")
                    };
                    if icon_button(ui, scale_icon, scale_tip).clicked() {
                        action = Some(Action::ToggleScaling);
                    }
                    let (fs_icon, fs_tip) = if fullscreen {
                        (icon::CORNERS_IN, format!("Leave full screen ({})", crate::platform::shortcut_label("F")))
                    } else {
                        (icon::CORNERS_OUT, format!("Full screen ({})", crate::platform::shortcut_label("F")))
                    };
                    if icon_button(ui, fs_icon, &fs_tip).clicked() {
                        action = Some(Action::FullScreen);
                    }
                    separator(ui, &p);

                    ui.add_enabled_ui(connected && !tab.conn.view_only, |ui| {
                        menu(ui, icon::KEYBOARD, "Send keys", |ui| {
                            ui.label(RichText::new("Send to the Pi").small().color(p.text_weak));
                            for (label, combo) in COMBOS {
                                if ui.button(*label).clicked() {
                                    action = Some(Action::Combo(combo));
                                }
                            }
                        });
                        menu(ui, icon::CLIPBOARD_TEXT, "Clipboard", |ui| {
                            if super::widgets::menu_item(ui, icon::ARROW_SQUARE_OUT, "Send clipboard to the Pi").clicked() {
                                action = Some(Action::PushClipboard);
                            }
                            if super::widgets::menu_item(ui, icon::KEYBOARD, "Type clipboard text").clicked() {
                                action = Some(Action::TypeClipboard);
                            }
                            ui.separator();
                            let mut share = tab.conn.share_clipboard;
                            if ui.checkbox(&mut share, "Share clipboard automatically").changed() {
                                action = Some(Action::ShareClipboard(share));
                            }
                            ui.label(
                                RichText::new("Tip: in the Pi's Terminal, paste with Command-Shift-V.")
                                    .small()
                                    .color(p.text_weak),
                            );
                        });
                    });
                    ui.add_enabled_ui(connected, |ui| {
                        menu(ui, icon::SLIDERS_HORIZONTAL, "Picture quality", |ui| {
                            ui.label(RichText::new("Picture quality").small().color(p.text_weak));
                            let mut q = tab.conn.quality;
                            for (value, label) in quality_options() {
                                if ui.radio_value(&mut q, value, label).clicked() {
                                    action = Some(Action::Quality(value));
                                }
                            }
                        });
                        let video = tab.conn.video;
                        if icon_toggle(ui, icon::FILM_STRIP, "Smoother video (H.264), works best on Pi 4", video).clicked() {
                            action = Some(Action::Video(!video));
                        }
                        separator(ui, &p);
                        if icon_button(ui, icon::CAMERA, "Save a screenshot").clicked() {
                            action = Some(Action::Screenshot);
                        }
                        if icon_button(ui, icon::ARROW_CLOCKWISE, "Refresh the picture").clicked() {
                            action = Some(Action::Refresh);
                        }
                        if icon_toggle(ui, icon::INFO, "Connection info", tab.info_open).clicked() {
                            action = Some(Action::Info);
                        }
                    });
                    if icon_toggle(ui, icon::FOLDER_SIMPLE, "Files: copy files to and from the Pi", tab.files.open).clicked() {
                        action = Some(Action::Files);
                    }
                    separator(ui, &p);
                    let r = icon_button(ui, icon::SIGN_OUT, "Disconnect");
                    if r.clicked() {
                        action = Some(Action::Disconnect);
                    }
                });
            });
        });
    let bar_rect = response.response.rect;
    ctx.data_mut(|d| d.insert_temp(rect_id, bar_rect));

    if app.tabs[index].info_open {
        info_panel(app, &ctx, index, bar_rect, opacity);
    }

    if debug_press {
        action = Some(Action::Disconnect);
    }
    let tab = &mut app.tabs[index];
    match action {
        Some(Action::Sidebar) => app.cycle_sidebar(),
        Some(Action::ToggleScaling) => {
            tab.conn.scaling = if tab.conn.scaling == Scaling::Fit { Scaling::Actual } else { Scaling::Fit };
            app.tab_connection_changed(index);
        }
        Some(Action::FullScreen) => {
            let on = !app.fullscreen;
            app.set_fullscreen(on);
        }
        Some(Action::Combo(combo)) => tab.send_combo(combo),
        Some(Action::PushClipboard) => app.push_clipboard(index),
        Some(Action::TypeClipboard) => app.type_clipboard(index),
        Some(Action::ShareClipboard(on)) => {
            tab.conn.share_clipboard = on;
            app.tab_connection_changed(index);
        }
        Some(Action::Quality(q)) => {
            tab.set_quality(q);
            app.tab_connection_changed(index);
        }
        Some(Action::Video(on)) => {
            tab.set_video(on);
            app.tab_connection_changed(index);
        }
        Some(Action::Screenshot) => app.take_screenshot(index),
        Some(Action::Refresh) => tab.refresh(),
        Some(Action::Info) => tab.info_open = !tab.info_open,
        Some(Action::Files) => app.toggle_files(index),
        Some(Action::Disconnect) => {
            let id = tab.id;
            app.close_tab(id);
        }
        None => {}
    }
}

pub fn quality_options() -> [(Quality, &'static str); 4] {
    [
        (Quality::Automatic, "Automatic (adapts to your network)"),
        (Quality::Best, "Best picture"),
        (Quality::Balanced, "Balanced"),
        (Quality::Fast, "Fast (for slow networks)"),
    ]
}

fn menu(ui: &mut Ui, symbol: &str, tooltip: &str, content: impl FnOnce(&mut Ui)) {
    let button = egui::Button::new(super::theme::ic(symbol).size(18.0))
        .frame_when_inactive(false)
        .corner_radius(CornerRadius::same(8))
        .min_size(vec2(32.0, 32.0));
    let (response, _) = MenuButton::from_button(button).ui(ui, |ui| {
        ui.set_min_width(220.0);
        content(ui);
    });
    if ui.is_enabled() {
        response.on_hover_text(tooltip);
    }
}

fn separator(ui: &mut Ui, p: &Palette) {
    let (rect, _) = ui.allocate_exact_size(vec2(9.0, 22.0), egui::Sense::hover());
    ui.painter().vline(rect.center().x, rect.y_range(), egui::Stroke::new(1.0, p.border));
}

/// Details about the connection, under the toolbar.
fn info_panel(app: &mut App, ctx: &egui::Context, index: usize, bar: Rect, opacity: f32) {
    let p = Palette::of(ctx);
    let tab = &app.tabs[index];
    let mut rows: Vec<(&str, String)> = Vec::new();
    rows.push(("Address", tab.conn.address_label()));
    if let Some(name) = tab.desktop_name.as_ref().or(tab.server.as_ref().map(|s| &s.name)).filter(|n| !n.is_empty()) {
        rows.push(("Computer name", name.clone()));
    }
    if let Some(s) = &tab.server {
        rows.push(("Screen size", format!("{} × {}", s.width, s.height)));
        rows.push(("Login method", s.security.clone()));
        let encrypted = if tab.tunnelled() {
            "Yes, through secure remote login (SSH)".to_string()
        } else if s.encrypted {
            "Yes".into()
        } else {
            "No".into()
        };
        rows.push(("Encrypted", encrypted));
        rows.push(("Keyboard", if tab.physical_keys_active() { "Physical keys".into() } else { "Characters".into() }));
        let system = tab.remote_system().label();
        let how = if tab.conn.remote_system == crate::model::RemoteSystem::Automatic { " (recognised)" } else { "" };
        rows.push(("Remote computer", format!("{system}{how}")));
        rows.push(("Protocol", format!("VNC {}", s.protocol_version)));
    }
    if let Some(st) = &tab.stats {
        rows.push(("Data rate", format_rate(st.kbps)));
        rows.push(("Updates per second", format!("{:.0}", st.updates_per_sec)));
        rows.push(("Delay", st.latency_ms.map_or("–".into(), |ms| format!("{ms:.0} ms"))));
        if !st.encoding.is_empty() {
            rows.push(("Picture format", st.encoding.to_string()));
        }
        let quality = quality_options().iter().find(|(q, _)| *q == st.effective_quality).map(|(_, l)| *l).unwrap_or("");
        rows.push(("Quality in use", quality.split(" (").next().unwrap_or("").to_string()));
        rows.push(("Data received", format_bytes(st.bytes_total)));
    }

    let mut close = false;
    Area::new(Id::new(("info", tab.id)))
        .order(Order::Foreground)
        .pivot(Align2::CENTER_TOP)
        .fixed_pos(pos2(bar.center().x, bar.bottom() + 8.0))
        .show(ctx, |ui| {
            ui.multiply_opacity(opacity.max(0.9));
            floating(&p).inner_margin(Margin::same(16)).show(ui, |ui| {
                ui.set_width(300.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Connection info").font(super::theme::medium(15.0)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icon_button(ui, icon::X, "Close").clicked() {
                            close = true;
                        }
                    });
                });
                ui.add_space(4.0);
                ui.spacing_mut().interact_size.y = 16.0;
                egui::Grid::new(("info-grid", tab.id)).num_columns(2).spacing(vec2(16.0, 7.0)).show(ui, |ui| {
                    for (label, value) in &rows {
                        ui.label(RichText::new(*label).color(p.text_weak));
                        ui.add(egui::Label::new(value.as_str()).wrap());
                        ui.end_row();
                    }
                });
                if !tab.physical_keys_active() {
                    ui.add_space(6.0);
                    super::widgets::hint_text(ui, super::settings::LAYOUT_HINT);
                }
            });
        });
    if close {
        app.tabs[index].info_open = false;
    }
}

fn format_rate(kbps: f32) -> String {
    if kbps >= 1000.0 { format!("{:.1} Mbit/s", kbps / 1000.0) } else { format!("{kbps:.0} kbit/s") }
}

fn format_bytes(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} MB", b / 1e6)
    } else {
        format!("{:.0} kB", b / 1e3)
    }
}
