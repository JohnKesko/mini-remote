//! Shared building blocks. Every page is made from these, so heights,
//! paddings and vertical centring are the same everywhere:
//! buttons, text fields, drop-downs and segmented controls are
//! [`CONTROL_HEIGHT`] tall, rows are laid out left to right and centred
//! on one line, and in debug builds rows check that they really are.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use egui::{
    Align, Align2, Color32, CornerRadius, Frame, Galley, Layout, Margin, Rect, Response, RichText, Sense, Stroke, Ui,
    UiBuilder, pos2, vec2,
};

use super::theme::{CONTROL_HEIGHT, Palette, RADIUS, RADIUS_SMALL, ic, icon_font, medium, semibold};

/// Height of a settings or form row.
pub const ROW_HEIGHT: f32 = 48.0;
/// Width of the label column in forms.
pub const FORM_LABEL_WIDTH: f32 = 168.0;
/// Room after each form input for an optional button (show password, pick key).
pub const FORM_TRAILING: f32 = 40.0;
/// Space between controls in a row.
pub const GAP: f32 = 8.0;
/// Where text starts in list rows (after a 32 px icon column and a 4 px gap).
pub const LIST_TEXT_COLUMN: f32 = 36.0;

// ----- Alignment checks (debug builds) -----------------------------------------

/// Debug builds with `MINI_REMOTE_DEBUG_LAYOUT=1`: draw widget outlines and
/// check that rows built with these helpers are really centred.
pub fn debug_layout() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| cfg!(debug_assertions) && std::env::var_os("MINI_REMOTE_DEBUG_LAYOUT").is_some_and(|v| !v.is_empty() && v != "0"))
}

/// Warn (once per row kind) when the centres of `rects` differ from `center_y`
/// by more than half a pixel.
pub fn check_centres(row: &str, center_y: f32, rects: &[Rect]) {
    if !debug_layout() {
        return;
    }
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    for r in rects {
        let off = r.center().y - center_y;
        if off.abs() > 0.5 {
            let key = format!("{row}:{off:.1}");
            if SEEN.get_or_init(Default::default).lock().map(|mut s| s.insert(key)).unwrap_or(false) {
                log::warn!("layout: in {row} a widget is {off:+.1} px off the row's centre line ({r:?})");
            }
        }
    }
}

/// Lay widgets out inside a rect that was already allocated, without moving
/// the parent's cursor again.
pub fn in_rect<R>(ui: &mut Ui, rect: Rect, layout: Layout, add: impl FnOnce(&mut Ui) -> R) -> R {
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect).layout(layout));
    add(&mut child)
}

/// A row of exactly `height`, laid out left to right with everything centred
/// on the row's middle line.
pub fn row<R>(ui: &mut Ui, height: f32, add: impl FnOnce(&mut Ui) -> R) -> (Rect, R) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let inner = in_rect(ui, rect, Layout::left_to_right(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        add(ui)
    });
    (rect, inner)
}

/// Move a point onto whole screen pixels, so text and icons stay crisp.
pub fn snap(ui: &Ui, p: egui::Pos2) -> egui::Pos2 {
    use egui::emath::GuiRounding;
    p.round_to_pixels(ui.ctx().pixels_per_point())
}

// ----- Buttons ---------------------------------------------------------------------

fn sized(ui: &mut Ui, button: egui::Button, height: f32) -> Response {
    // Buttons are never shorter than interact_size, so set it to the height wanted.
    ui.scope(|ui| {
        ui.spacing_mut().interact_size.y = height;
        ui.add(button.min_size(vec2(0.0, height)))
    })
    .inner
}

/// The main action of a page or dialog, filled with the accent colour.
pub fn primary_button(ui: &mut Ui, text: impl Into<String>) -> Response {
    let p = Palette::of(ui.ctx());
    let button = egui::Button::new(RichText::new(text.into()).font(medium(14.0)).color(p.on_accent))
        .fill(p.accent)
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(RADIUS_SMALL))
        .min_size(vec2(88.0, CONTROL_HEIGHT));
    sized(ui, button, CONTROL_HEIGHT)
}

/// The main action with an icon in front, e.g. "Upload".
pub fn primary_icon_button(ui: &mut Ui, icon: &str, text: &str) -> Response {
    let p = Palette::of(ui.ctx());
    let atoms = (ic(icon).size(15.0).color(p.on_accent), RichText::new(text).font(medium(14.0)).color(p.on_accent));
    let button =
        egui::Button::new(atoms).fill(p.accent).stroke(Stroke::NONE).corner_radius(CornerRadius::same(RADIUS_SMALL));
    sized(ui, button, CONTROL_HEIGHT)
}

/// A quieter button.
pub fn secondary_button(ui: &mut Ui, text: &str) -> Response {
    let button = egui::Button::new(RichText::new(text).font(medium(14.0)))
        .corner_radius(CornerRadius::same(RADIUS_SMALL))
        .min_size(vec2(72.0, CONTROL_HEIGHT));
    sized(ui, button, CONTROL_HEIGHT)
}

/// A quieter button with an icon in front.
pub fn secondary_icon_button(ui: &mut Ui, icon: &str, text: &str, height: f32) -> Response {
    let atoms = (ic(icon).size(14.0), RichText::new(text).font(medium(13.5)));
    let button = egui::Button::new(atoms).corner_radius(CornerRadius::same(RADIUS_SMALL));
    sized(ui, button, height)
}

/// A button that does something that can't be undone.
pub fn danger_button(ui: &mut Ui, text: impl Into<String>) -> Response {
    let p = Palette::of(ui.ctx());
    let button = egui::Button::new(RichText::new(text.into()).font(medium(14.0)).color(Color32::WHITE))
        .fill(p.danger)
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(RADIUS_SMALL))
        .min_size(vec2(88.0, CONTROL_HEIGHT));
    sized(ui, button, CONTROL_HEIGHT)
}

/// A square button showing just an icon, with a tooltip.
pub fn icon_button(ui: &mut Ui, icon: &str, tooltip: &str) -> Response {
    icon_button_sized(ui, icon, tooltip, CONTROL_HEIGHT, false)
}

/// Icon button that shows an "on" state.
pub fn icon_toggle(ui: &mut Ui, icon: &str, tooltip: &str, on: bool) -> Response {
    icon_button_sized(ui, icon, tooltip, CONTROL_HEIGHT, on)
}

pub fn icon_button_sized(ui: &mut Ui, icon: &str, tooltip: &str, side: f32, on: bool) -> Response {
    let p = Palette::of(ui.ctx());
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
        let center = snap(ui, rect.center());
        ui.painter().text(center, Align2::CENTER_CENTER, icon, icon_font((side * 0.56).round()), color);
    }
    let response =
        if ui.is_enabled() { response.on_hover_text(tooltip) } else { response.on_disabled_hover_text(tooltip) };
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A menu entry with an icon in front.
pub fn menu_item(ui: &mut Ui, icon: &str, text: &str) -> Response {
    ui.button((ic(icon), text))
}

/// A red menu entry with an icon in front (for deleting).
pub fn danger_menu_item(ui: &mut Ui, icon: &str, text: &str) -> Response {
    let color = Palette::of(ui.ctx()).danger;
    ui.button((ic(icon).color(color), RichText::new(text).color(color)))
}

// ----- Inputs ----------------------------------------------------------------------

/// A single-line text field, as tall as a button, text centred vertically.
pub fn text_field<'a>(text: &'a mut String, hint: &str) -> egui::TextEdit<'a> {
    egui::TextEdit::singleline(text)
        .hint_text(hint)
        .margin(Margin::symmetric(10, 0))
        .min_size(vec2(0.0, CONTROL_HEIGHT))
        .vertical_align(Align::Center)
        .desired_width(f32::INFINITY)
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
        ui.painter().circle_filled(pos2(x, rect.center().y), radius - 3.0, Color32::WHITE);
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Pick one of a few options, shown side by side. As tall as a button.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) -> Response {
    let p = Palette::of(ui.ctx());
    let mut changed = false;
    let inner = Frame::new()
        .fill(if p.dark { Color32::from_rgb(0x2a, 0x2a, 0x32) } else { Color32::from_rgb(0xe9, 0xe9, 0xf0) })
        .corner_radius(CornerRadius::same(RADIUS_SMALL))
        .inner_margin(Margin::same(3))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.spacing_mut().interact_size.y = CONTROL_HEIGHT - 6.0;
            ui.horizontal(|ui| {
                // Keep the options in reading order inside right-aligned rows too.
                let reversed = ui.layout().main_dir() == egui::Direction::RightToLeft;
                let ordered: Vec<&(T, &str)> =
                    if reversed { options.iter().rev().collect() } else { options.iter().collect() };
                for (option, label) in ordered {
                    let selected = *value == *option;
                    let text = RichText::new(*label).font(medium(13.5)).color(if selected { p.text } else { p.text_weak });
                    let button = egui::Button::new(text)
                        .fill(if selected { p.surface } else { Color32::TRANSPARENT })
                        .stroke(if selected { Stroke::new(1.0, p.border) } else { Stroke::NONE })
                        .corner_radius(CornerRadius::same(RADIUS_SMALL - 2))
                        .min_size(vec2(0.0, CONTROL_HEIGHT - 6.0));
                    if ui.add(button).clicked() && !selected {
                        *value = *option;
                        changed = true;
                    }
                }
            });
        });
    let mut response = inner.response;
    if changed {
        response.mark_changed();
    }
    response
}

// ----- Text --------------------------------------------------------------------

/// Small grey explanation text.
pub fn hint_text(ui: &mut Ui, text: &str) -> Response {
    let p = Palette::of(ui.ctx());
    ui.add(egui::Label::new(RichText::new(text).size(12.0).color(p.text_weak)).wrap())
}

/// A red line explaining what went wrong.
pub fn error_line(ui: &mut Ui, text: &str) {
    let color = Palette::of(ui.ctx()).danger;
    ui.horizontal_top(|ui| {
        ui.label(ic(egui_phosphor::regular::WARNING_CIRCLE).color(color));
        ui.add(egui::Label::new(RichText::new(text).color(color)).wrap());
    });
}

/// A label above a field (in narrow places like dialogs).
pub fn field_label(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).font(medium(13.0)));
}

/// A warning box with an icon.
pub fn warning_box(ui: &mut Ui, text: &str) {
    let p = Palette::of(ui.ctx());
    Frame::new()
        .fill(if p.dark { Color32::from_rgb(0x3a, 0x30, 0x18) } else { Color32::from_rgb(0xfd, 0xf4, 0xe1) })
        .corner_radius(CornerRadius::same(RADIUS_SMALL))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.label(ic(egui_phosphor::regular::WARNING).color(p.warning).size(16.0));
                ui.add(egui::Label::new(RichText::new(text).size(13.0)).wrap());
            });
        });
}

/// Lay out one line of text, cut off with "…" if wider than `max_width`.
pub fn truncated(ui: &Ui, text: &str, font: egui::FontId, color: Color32, max_width: f32) -> std::sync::Arc<Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_width.max(10.0));
    ui.painter().layout_job(job)
}

/// Paint a galley with its left edge at `x`, centred on `center_y`, on whole pixels.
pub fn paint_left_center(ui: &Ui, galley: std::sync::Arc<Galley>, x: f32, center_y: f32, color: Color32) -> Rect {
    let pos = snap(ui, pos2(x, center_y - galley.size().y / 2.0));
    let rect = Rect::from_min_size(pos, galley.size());
    ui.painter().galley(pos, galley, color);
    rect
}

// ----- Containers --------------------------------------------------------------

/// A rounded card on the surface colour.
pub fn card(p: &Palette) -> Frame {
    Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(RADIUS + 2))
        .inner_margin(Margin::same(16))
        .shadow(egui::Shadow {
            offset: [0, 2],
            blur: 10,
            spread: 0,
            color: Color32::from_black_alpha(if p.dark { 60 } else { 14 }),
        })
}

/// A floating panel over the remote screen (overlays, toolbar).
pub fn floating(p: &Palette) -> Frame {
    Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::same(24))
        .shadow(egui::Shadow {
            offset: [0, 10],
            blur: 30,
            spread: 0,
            color: Color32::from_black_alpha(if p.dark { 140 } else { 60 }),
        })
}

/// A settings-style group: an icon and title above a rounded card.
pub fn section_card<R>(ui: &mut Ui, symbol: &str, title: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    let p = Palette::of(ui.ctx());
    ui.add_space(24.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
    let y = rect.center().y;
    let icon_pos = snap(ui, pos2(rect.left() + 8.0, y));
    ui.painter().text(icon_pos, Align2::CENTER_CENTER, symbol, icon_font(16.0), p.accent);
    let galley = ui.painter().layout_no_wrap(title.to_string(), semibold(15.0), p.text);
    paint_left_center(ui, galley, rect.left() + 24.0, y, p.text);
    ui.add_space(8.0);
    Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(RADIUS + 2))
        .inner_margin(Margin::symmetric(16, 0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            add(ui)
        })
        .inner
}

/// One row in a settings card: label and a short description on the left,
/// the control on the right, both centred on the row's middle line.
pub fn setting_row(ui: &mut Ui, label: &str, description: Option<&str>, control: impl FnOnce(&mut Ui) -> Response) {
    let p = Palette::of(ui.ctx());
    let width = ui.available_width();
    let left_width = (width * 0.55).max(160.0);
    let title = ui.painter().layout_no_wrap(label.to_string(), medium(14.0), p.text);
    let desc = description.map(|d| {
        ui.painter().layout(d.to_string(), egui::FontId::proportional(12.0), p.text_weak, left_width)
    });
    let block = title.size().y + desc.as_ref().map_or(0.0, |d| 2.0 + d.size().y);
    let height = ROW_HEIGHT.max((block + 16.0).ceil());
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let center = rect.center().y;

    let title_pos = snap(ui, pos2(rect.left(), center - block / 2.0));
    let top = title_pos.y;
    let title_h = title.size().y;
    ui.painter().galley(title_pos, title, p.text);
    if let Some(desc) = desc {
        ui.painter().galley(pos2(rect.left(), top + title_h + 2.0), desc, p.text_weak);
    }

    let right = Rect::from_min_max(pos2(rect.left() + left_width, rect.top()), rect.max);
    let response = in_rect(ui, right, Layout::right_to_left(Align::Center), control);
    check_centres("settings row", center, &[response.rect]);
}

/// One row of a form: the label in a fixed column on the left, the input
/// filling the rest (minus room for an optional button after it), all
/// centred on one line. `add` gets the input width and returns the input.
pub fn form_row(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui, f32) -> Response) {
    let p = Palette::of(ui.ctx());
    let (rect, response) = row(ui, ROW_HEIGHT, |ui| {
        let (label_rect, _) = ui.allocate_exact_size(vec2(FORM_LABEL_WIDTH - GAP, CONTROL_HEIGHT), Sense::hover());
        let galley = ui.painter().layout_no_wrap(label.to_string(), medium(14.0), p.text);
        paint_left_center(ui, galley, label_rect.left(), label_rect.center().y, p.text);
        let input_width = ui.available_width() - FORM_TRAILING;
        add(ui, input_width)
    });
    check_centres("form row", rect.center().y, &[response.rect]);
}

/// Something under a form input (a checkbox or a hint), lined up with the inputs.
pub fn form_extra<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let inner = ui.horizontal(|ui| {
        ui.add_space(FORM_LABEL_WIDTH);
        ui.spacing_mut().item_spacing.x = 0.0;
        add(ui)
    });
    ui.add_space(12.0);
    inner.inner
}

/// A short muted line under a form input, lined up with the input.
pub fn form_hint(ui: &mut Ui, text: &str) {
    ui.add_space(-6.0);
    form_extra(ui, |ui| hint_text(ui, text));
}

/// A thin line between rows of a card.
pub fn row_divider(ui: &mut Ui) {
    let p = Palette::of(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.center().y, Stroke::new(1.0, p.border.gamma_multiply(0.7)));
}

/// A short muted line inside a card, between rows.
pub fn row_hint(ui: &mut Ui, text: &str) {
    ui.add_space(10.0);
    hint_text(ui, text);
    ui.add_space(10.0);
}

/// One line of a list (files): icon, name, and muted details on the right,
/// all centred on the row's middle line. The whole row is clickable.
pub fn list_row(ui: &mut Ui, icon: &str, icon_color: Color32, name: &str, detail: &str, selected: bool) -> Response {
    let p = Palette::of(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), CONTROL_HEIGHT), Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if selected {
            p.accent.gamma_multiply(if p.dark { 0.35 } else { 0.18 })
        } else if response.hovered() {
            p.border.gamma_multiply(0.6)
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, CornerRadius::same(6), fill);
        let y = rect.center().y;
        let icon_pos = snap(ui, pos2(rect.left() + 16.0, y));
        ui.painter().text(icon_pos, Align2::CENTER_CENTER, icon, icon_font(17.0), icon_color);
        let info = ui.painter().layout_no_wrap(detail.to_string(), egui::FontId::proportional(12.0), p.text_weak);
        let info_x = rect.right() - 8.0 - info.size().x;
        let name_width = info_x - rect.left() - LIST_TEXT_COLUMN - 8.0;
        let name_galley = truncated(ui, name, egui::FontId::proportional(13.5), p.text, name_width);
        let a = paint_left_center(ui, name_galley, rect.left() + LIST_TEXT_COLUMN, y, p.text);
        let b = paint_left_center(ui, info, info_x, y, p.text_weak);
        check_centres("list row", y, &[a, b]);
    }
    response
}

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgba_unmultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}
