//! A session tab: the remote screen filling the area, mouse input, the
//! remote mouse pointer, plus the floating toolbar and overlays.

use std::time::Duration;

use egui::{Color32, CursorIcon, Event, Rect, Stroke, Ui, pos2, vec2};

use super::theme::Palette;
use crate::app::App;
use crate::input::mouse::{self, ViewLayout};
use crate::session::SessionTab;

pub fn show(app: &mut App, ui: &mut Ui, index: usize) {
    let p = Palette::of(ui.ctx());
    let area = ui.max_rect();
    ui.painter().rect_filled(area, 0.0, p.screen_bg);
    let ppp = ui.ctx().pixels_per_point();
    let pointer = ui.input(|i| i.pointer.latest_pos());

    let tab = &mut app.tabs[index];
    tab.view_pixels = Some((to_u16(area.width() * ppp), to_u16(area.height() * ppp)));

    let view = tab
        .texture
        .as_ref()
        .map(|t| mouse::layout(area, vec2(t.width as f32, t.height as f32), tab.conn.scaling, 1.0 / ppp, pointer));

    if let (Some(tex), Some(view)) = (&mut tab.texture, view) {
        tex.set_filter(&app.gpu, !view.pixel_exact);
        let live = tab.status.is_connected() && tab.prompt.is_none();
        let tint = if live { Color32::WHITE } else { Color32::from_gray(95) };
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        ui.painter().with_clip_rect(area).image(tex.id, view.rect, uv, tint);
    }

    if let Some(view) = &view
        && tab.accepts_input()
    {
        handle_mouse(ui, tab, view, area);
        draw_cursor(ui, tab, view, area, pointer);
    }

    // A short flash around the edge when the remote computer beeps.
    if let Some(at) = tab.bell_at {
        let t = at.elapsed().as_secs_f32();
        if t < 0.4 {
            let alpha = 1.0 - t / 0.4;
            ui.painter().rect_stroke(area.shrink(2.0), 0.0, Stroke::new(4.0, p.warning.gamma_multiply(alpha)), egui::StrokeKind::Inside);
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        } else {
            tab.bell_at = None;
        }
    }

    // Files dragged from Finder onto the window are copied to the Pi.
    let (hovering_files, dropped) = ui.input(|i| {
        let dropped: Vec<std::path::PathBuf> = i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect();
        (!i.raw.hovered_files.is_empty(), dropped)
    });
    if hovering_files && !app.tabs[index].files.open {
        ui.painter().rect_stroke(area.shrink(6.0), 12.0, Stroke::new(3.0, p.accent), egui::StrokeKind::Inside);
        ui.painter().text(
            area.center(),
            egui::Align2::CENTER_CENTER,
            "Drop to copy to the Pi",
            egui::FontId::proportional(22.0),
            Color32::WHITE,
        );
    }
    if !dropped.is_empty() {
        app.drop_files(index, dropped);
    }

    super::overlays::show(app, ui, index, area);
    super::toolbar::show(app, ui, index, area);
    let ctx = ui.ctx().clone();
    super::files_view::show(app, &ctx, index, area);
}

fn to_u16(v: f32) -> u16 {
    v.round().clamp(0.0, f32::from(u16::MAX)) as u16
}

/// True when the pointer is over something floating above the remote screen:
/// the toolbar, menus, dialogs or the Files panel. The remote screen itself is
/// drawn in the background layer, which `Context::is_pointer_over_egui` also
/// counts as egui, so that check blocked every mouse event to the remote computer.
pub(crate) fn pointer_over_controls(ctx: &egui::Context) -> bool {
    let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) else { return false };
    ctx.layer_id_at(pos).is_some_and(|layer| layer.order != egui::Order::Background)
}

/// Send mouse movement, buttons and scrolling over the remote screen.
fn handle_mouse(ui: &Ui, tab: &mut SessionTab, view: &ViewLayout, area: Rect) {
    let Some((rw, rh)) = tab.texture.as_ref().map(|t| (t.width as u16, t.height as u16)) else { return };
    let over_controls = pointer_over_controls(ui.ctx());
    let latest = ui.input(|i| i.pointer.latest_pos());
    let events = ui.input(|i| i.events.clone());
    for event in events {
        match event {
            Event::PointerMoved(pos) => {
                let inside = area.contains(pos) && !over_controls;
                if inside || tab.mouse.buttons != 0 {
                    let (x, y) = mouse::to_remote(pos, view, rw, rh);
                    if tab.mouse.last != Some((x, y)) {
                        let buttons = tab.mouse.buttons;
                        tab.send_pointer(x, y, buttons);
                    }
                }
            }
            Event::PointerButton { pos, button, pressed, .. } => {
                let bit = mouse::button_bit(button);
                let (x, y) = mouse::to_remote(pos, view, rw, rh);
                if pressed {
                    if area.contains(pos) && !over_controls {
                        tab.mouse.buttons |= bit;
                        let buttons = tab.mouse.buttons;
                        tab.send_pointer(x, y, buttons);
                        // Clicking the screen gives it the keyboard.
                        ui.ctx().memory_mut(|m| {
                            if let Some(id) = m.focused() {
                                m.surrender_focus(id);
                            }
                        });
                    }
                } else if tab.mouse.buttons & bit != 0 {
                    tab.mouse.buttons &= !bit;
                    let buttons = tab.mouse.buttons;
                    tab.send_pointer(x, y, buttons);
                }
            }
            Event::MouseWheel { unit, delta, .. } => {
                let Some(pos) = latest else { continue };
                if !area.contains(pos) || over_controls {
                    continue;
                }
                let (x, y) = mouse::to_remote(pos, view, rw, rh);
                let buttons = tab.mouse.buttons;
                for click in tab.mouse.wheel.add(unit, delta) {
                    tab.send_pointer(x, y, buttons | click);
                    tab.send_pointer(x, y, buttons);
                }
            }
            Event::WindowFocused(false) => {
                tab.mouse.wheel.reset();
            }
            _ => {}
        }
    }
}

/// Draw the remote computer's mouse pointer where the local mouse is, and
/// hide the local one over the remote screen.
fn draw_cursor(ui: &Ui, tab: &SessionTab, view: &ViewLayout, area: Rect, pointer: Option<egui::Pos2>) {
    let Some(pos) = pointer else { return };
    if !area.contains(pos) || pointer_over_controls(ui.ctx()) || !tab.cursor.known {
        return;
    }
    ui.ctx().set_cursor_icon(CursorIcon::None);
    let (Some(texture), Some(image)) = (&tab.cursor.texture, &tab.cursor.image) else { return };
    // Same size as on the remote screen, but never tiny.
    let scale = view.scale.max(0.6);
    let hotspot = vec2(f32::from(image.hotspot_x), f32::from(image.hotspot_y)) * scale;
    let size = vec2(f32::from(image.width), f32::from(image.height)) * scale;
    let rect = Rect::from_min_size(pos - hotspot, size);
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("remote-cursor")));
    painter.with_clip_rect(area).image(texture.id(), rect, uv, Color32::WHITE);
}

#[cfg(test)]
mod tests {
    use egui::{Area, CentralPanel, Context, Event, Id, Order, PointerButton, RawInput, pos2, vec2};

    use super::pointer_over_controls;

    /// One frame laid out like a session: the screen fills the window and a
    /// small floating toolbar sits on top of it.
    fn frame(ctx: &Context, pointer: egui::Pos2) -> (bool, bool) {
        let input = RawInput {
            screen_rect: Some(egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))),
            events: vec![
                Event::PointerMoved(pointer),
                Event::PointerButton { pos: pointer, button: PointerButton::Primary, pressed: true, modifiers: Default::default() },
            ],
            ..Default::default()
        };
        let mut result = (false, false);
        let mut output = ctx.run_ui(input, |ui| {
            CentralPanel::default().show(ui, |ui| {
                ui.allocate_space(ui.available_size());
            });
            Area::new(Id::new("toolbar")).order(Order::Foreground).fixed_pos(pos2(300.0, 10.0)).show(ui.ctx(), |ui| {
                ui.allocate_space(vec2(200.0, 40.0));
            });
            result = (pointer_over_controls(ui.ctx()), ui.ctx().is_pointer_over_egui());
        });
        // No GPU here, so drop the font texture updates.
        output.textures_delta.clear();
        result
    }

    #[test]
    fn remote_screen_gets_the_mouse_but_the_toolbar_does_not() {
        let ctx = Context::default();
        // Layer positions come from the previous frame, so run each case twice.
        frame(&ctx, pos2(400.0, 400.0));
        let (over_controls, over_egui) = frame(&ctx, pos2(400.0, 400.0));
        assert!(!over_controls, "the remote screen must receive the mouse");
        // The old check said "over egui" here, which blocked the mouse.
        assert!(over_egui);

        frame(&ctx, pos2(350.0, 25.0));
        let (over_controls, _) = frame(&ctx, pos2(350.0, 25.0));
        assert!(over_controls, "clicks on the toolbar must not reach the remote screen");
    }
}
