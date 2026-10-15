//! Small reusable pieces: buttons, cards, toggles, status dots.

use egui::{
    Align2, Color32, CornerRadius, FontId, Frame, Margin, Response, RichText, Sense, Stroke, Ui, WidgetText, vec2,
};

use super::theme::{Palette, RADIUS, RADIUS_SMALL};

/// The main action of a screen or dialog, filled with the accent colour.
pub fn primary_button(ui: &mut Ui, text: impl Into<String>) -> Response {
    let p = Palette::of(ui.ctx());
    let button = egui::Button::new(RichText::new(text.into()).color(p.on_accent).strong())
        .fill(p.accent)
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(RADIUS_SMALL))
        .min_size(vec2(96.0, 34.0));
    ui.add(button)
}

/// A quieter button.
pub fn secondary_button(ui: &mut Ui, text: impl Into<WidgetText>) -> Response {
    let button = egui::Button::new(text).corner_radius(CornerRadius::same(RADIUS_SMALL)).min_size(vec2(80.0, 34.0));
    ui.add(button)
}

/// A button that does something that can't be undone.
pub fn danger_button(ui: &mut Ui, text: impl Into<String>) -> Response {
    let p = Palette::of(ui.ctx());
    let button = egui::Button::new(RichText::new(text.into()).color(Color32::WHITE).strong())
        .fill(p.danger)
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(RADIUS_SMALL))
        .min_size(vec2(96.0, 34.0));
    ui.add(button)
}

/// A square button showing just an icon, with a tooltip.
pub fn icon_button(ui: &mut Ui, icon: &str, tooltip: &str) -> Response {
    icon_button_sized(ui, icon, tooltip, 18.0, false)
}

/// Icon button that shows an "on" state.
pub fn icon_toggle(ui: &mut Ui, icon: &str, tooltip: &str, on: bool) -> Response {
    icon_button_sized(ui, icon, tooltip, 18.0, on)
}

pub fn icon_button_sized(ui: &mut Ui, icon: &str, tooltip: &str, size: f32, on: bool) -> Response {
    let p = Palette::of(ui.ctx());
    let side = size + 14.0;
    let (rect, response) = ui.allocate_exact_size(vec2(side, side), Sense::click());
    if ui.is_rect_visible(rect) {
        let enabled = ui.is_enabled();
        let fill = if on {
            p.accent_soft
        } else if enabled && response.hovered() {
            ui.visuals().widgets.hovered.bg_fill
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, CornerRadius::same(RADIUS_SMALL), fill);
        let color = if !enabled {
            p.text_weak.gamma_multiply(0.6)
        } else if on {
            p.accent
        } else {
            p.text
        };
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, icon, FontId::proportional(size), color);
    }
    let response = if ui.is_enabled() {
        response.on_hover_text(tooltip)
    } else {
        response.on_disabled_hover_text(tooltip)
    };
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A rounded card on the surface colour.
pub fn card(p: &Palette) -> Frame {
    Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(RADIUS + 2))
        .inner_margin(Margin::same(16))
        .shadow(egui::Shadow { offset: [0, 2], blur: 10, spread: 0, color: Color32::from_black_alpha(if p.dark { 60 } else { 14 }) })
}

/// A floating panel over the remote screen (overlays, toolbar).
pub fn floating(p: &Palette) -> Frame {
    Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::same(24))
        .shadow(egui::Shadow { offset: [0, 10], blur: 30, spread: 0, color: Color32::from_black_alpha(if p.dark { 140 } else { 60 }) })
}

/// A coloured dot showing connection status.
pub fn status_dot(ui: &mut Ui, color: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
    response
}

/// iOS-style on/off switch.
pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let p = Palette::of(ui.ctx());
    let size = vec2(38.0, 22.0);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_responsive(response.id, *on);
        let off_color = if p.dark { Color32::from_rgb(0x44, 0x44, 0x50) } else { Color32::from_rgb(0xcf, 0xcf, 0xda) };
        let mut bg = lerp_color(off_color, p.accent, t);
        if !ui.is_enabled() {
            bg = bg.gamma_multiply(0.5);
        }
        let radius = rect.height() / 2.0;
        ui.painter().rect_filled(rect, CornerRadius::same(radius as u8), bg);
        let x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), t);
        ui.painter().circle_filled(egui::pos2(x, rect.center().y), radius - 3.0, Color32::WHITE);
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A switch with a label and an optional plain-English explanation below.
pub fn toggle_row(ui: &mut Ui, on: &mut bool, label: &str, hint: Option<&str>) -> Response {
    let mut response = None;
    ui.horizontal(|ui| {
        let r = toggle(ui, on);
        ui.add_space(4.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let label_resp = ui.add(egui::Label::new(label).sense(Sense::click()));
            if label_resp.clicked() && ui.is_enabled() {
                *on = !*on;
            }
            if let Some(hint) = hint {
                hint_text(ui, hint);
            }
            response = Some(if label_resp.clicked() { label_resp } else { r });
        });
    });
    response.expect("row was shown")
}

/// Pick one of a few options, shown side by side.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) -> bool {
    let p = Palette::of(ui.ctx());
    let mut changed = false;
    Frame::new()
        .fill(if p.dark { Color32::from_rgb(0x2a, 0x2a, 0x32) } else { Color32::from_rgb(0xe9, 0xe9, 0xf0) })
        .corner_radius(CornerRadius::same(RADIUS_SMALL))
        .inner_margin(Margin::same(3))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| {
                for (option, label) in options {
                    let selected = *value == *option;
                    let text = RichText::new(*label).color(if selected { p.text } else { p.text_weak });
                    let button = egui::Button::new(text)
                        .fill(if selected { p.surface } else { Color32::TRANSPARENT })
                        .stroke(if selected { Stroke::new(1.0, p.border) } else { Stroke::NONE })
                        .corner_radius(CornerRadius::same(RADIUS_SMALL - 2))
                        .min_size(vec2(0.0, 26.0));
                    if ui.add(button).clicked() && !selected {
                        *value = *option;
                        changed = true;
                    }
                }
            });
        });
    changed
}

/// Small grey explanation text.
pub fn hint_text(ui: &mut Ui, text: &str) -> Response {
    let p = Palette::of(ui.ctx());
    ui.add(egui::Label::new(RichText::new(text).small().color(p.text_weak)).wrap())
}

/// A heading for a group of settings.
pub fn section_title(ui: &mut Ui, text: &str) {
    let p = Palette::of(ui.ctx());
    ui.add_space(6.0);
    ui.label(RichText::new(text).strong().size(13.0).color(p.text_weak));
    ui.add_space(2.0);
}

/// A label above a form field.
pub fn field_label(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(13.0).strong());
}

/// A wide single-line text field with comfortable height.
pub fn text_field<'a>(text: &'a mut String, hint: &str) -> egui::TextEdit<'a> {
    egui::TextEdit::singleline(text)
        .hint_text(hint)
        .margin(Margin::symmetric(10, 7))
        .desired_width(f32::INFINITY)
}

/// A warning box with an icon.
pub fn warning_box(ui: &mut Ui, text: &str) {
    let p = Palette::of(ui.ctx());
    Frame::new()
        .fill(if p.dark { Color32::from_rgb(0x3a, 0x30, 0x18) } else { Color32::from_rgb(0xfd, 0xf4, 0xe1) })
        .corner_radius(CornerRadius::same(RADIUS_SMALL))
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.label(RichText::new(egui_phosphor::regular::WARNING).color(p.warning).size(16.0));
                ui.add(egui::Label::new(RichText::new(text).size(13.0)).wrap());
            });
        });
}

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgba_unmultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

/// Space so content of `width` ends up centred in the available width.
pub fn centre_offset(ui: &Ui, width: f32) -> f32 {
    ((ui.available_width() - width) / 2.0).max(0.0)
}
