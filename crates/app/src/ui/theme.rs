//! Colours, fonts and spacing: calm indigo accent, rounded corners, light and dark.

use std::sync::Arc;

use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, FontTweak, Margin, Shadow, Stroke, TextStyle,
    Theme, Visuals,
};

/// Inter (SIL Open Font License), bundled with the app.
const INTER_REGULAR: &[u8] = include_bytes!("../../../../assets/fonts/Inter-Regular.ttf");
const INTER_MEDIUM: &[u8] = include_bytes!("../../../../assets/fonts/Inter-Medium.ttf");
const INTER_SEMIBOLD: &[u8] = include_bytes!("../../../../assets/fonts/Inter-SemiBold.ttf");

/// Font family names for the heavier Inter weights, and for icons.
const MEDIUM: &str = "Medium";
const SEMIBOLD: &str = "SemiBold";
const ICONS: &str = "Icons";

/// The icon font (Phosphor). Icons need their own family because Inter has
/// glyphs of its own at some of the same code points.
pub fn icon_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(ICONS.into()))
}

/// An icon as text, for labels and buttons.
pub fn ic(icon: &str) -> egui::RichText {
    egui::RichText::new(icon).family(FontFamily::Name(ICONS.into()))
}

/// Inter at medium weight (tab labels, card titles, buttons).
pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(MEDIUM.into()))
}

/// Inter at semi-bold weight (titles).
pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMIBOLD.into()))
}

/// The one accent colour.
pub const ACCENT: Color32 = Color32::from_rgb(0x55, 0x5c, 0xe0);
const ACCENT_DARK_MODE: Color32 = Color32::from_rgb(0x7c, 0x83, 0xf2);

pub const RADIUS: u8 = 10;
/// Height shared by buttons, text fields, drop-downs and segmented controls.
pub const CONTROL_HEIGHT: f32 = 32.0;
/// Height of small controls (header buttons, card footers).
pub const SMALL_HEIGHT: f32 = 28.0;
pub const RADIUS_SMALL: u8 = 8;

/// Colours used by the app's own widgets.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub dark: bool,
    /// Window background.
    pub bg: Color32,
    /// Cards, dialogs.
    pub surface: Color32,
    /// Tab bar and quiet areas.
    pub surface_alt: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_weak: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub on_accent: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub danger: Color32,
    pub idle: Color32,
    /// Behind the remote screen.
    pub screen_bg: Color32,
}

impl Palette {
    pub fn light() -> Self {
        Palette {
            dark: false,
            bg: Color32::from_rgb(0xf6, 0xf6, 0xf9),
            surface: Color32::WHITE,
            surface_alt: Color32::from_rgb(0xec, 0xec, 0xf1),
            border: Color32::from_rgb(0xdd, 0xdd, 0xe5),
            text: Color32::from_rgb(0x1d, 0x1d, 0x27),
            text_weak: Color32::from_rgb(0x6b, 0x6b, 0x7b),
            accent: ACCENT,
            accent_soft: Color32::from_rgb(0xe8, 0xe9, 0xfc),
            on_accent: Color32::WHITE,
            success: Color32::from_rgb(0x22, 0xa0, 0x63),
            warning: Color32::from_rgb(0xd9, 0x8e, 0x04),
            danger: Color32::from_rgb(0xd9, 0x3f, 0x45),
            idle: Color32::from_rgb(0xb4, 0xb4, 0xc0),
            screen_bg: Color32::from_rgb(0x2a, 0x2a, 0x30),
        }
    }

    pub fn dark() -> Self {
        Palette {
            dark: true,
            bg: Color32::from_rgb(0x17, 0x17, 0x1c),
            surface: Color32::from_rgb(0x22, 0x22, 0x29),
            surface_alt: Color32::from_rgb(0x1d, 0x1d, 0x23),
            border: Color32::from_rgb(0x33, 0x33, 0x3d),
            text: Color32::from_rgb(0xec, 0xec, 0xf1),
            text_weak: Color32::from_rgb(0x9a, 0x9a, 0xa8),
            accent: ACCENT_DARK_MODE,
            accent_soft: Color32::from_rgb(0x2c, 0x2d, 0x4a),
            on_accent: Color32::WHITE,
            success: Color32::from_rgb(0x3c, 0xc4, 0x80),
            warning: Color32::from_rgb(0xf0, 0xb1, 0x2e),
            danger: Color32::from_rgb(0xf0, 0x5d, 0x62),
            idle: Color32::from_rgb(0x5c, 0x5c, 0x6a),
            screen_bg: Color32::from_rgb(0x0e, 0x0e, 0x11),
        }
    }

    pub fn of(ctx: &egui::Context) -> Self {
        if ctx.theme() == Theme::Dark { Palette::dark() } else { Palette::light() }
    }
}

/// Text style for big page titles.
pub fn title_style() -> TextStyle {
    TextStyle::Name("Title".into())
}

/// Inter first, then egui's own fonts and the icon font as fallbacks
/// (for emoji, symbols and icons). Monospace stays egui's Hack.
fn fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    // Inter sits a little high in its line box; nudge it down so text looks
    // centred in buttons and fields.
    let tweak = FontTweak { y_offset_factor: 0.04, ..FontTweak::default() };
    for (name, bytes) in [("Inter", INTER_REGULAR), ("Inter-Medium", INTER_MEDIUM), ("Inter-SemiBold", INTER_SEMIBOLD)] {
        fonts.font_data.insert(name.into(), Arc::new(FontData::from_static(bytes).tweak(tweak.clone())));
    }
    if let Some(list) = fonts.families.get_mut(&FontFamily::Proportional) {
        list.insert(0, "Inter".into());
    }
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    let fallbacks: Vec<String> = fonts
        .families
        .get(&FontFamily::Proportional)
        .map(|l| l.iter().filter(|n| n.as_str() != "Inter").cloned().collect())
        .unwrap_or_default();
    for (family, font) in [(MEDIUM, "Inter-Medium"), (SEMIBOLD, "Inter-SemiBold"), (ICONS, "phosphor")] {
        let mut list = vec![font.to_string()];
        list.extend(fallbacks.iter().filter(|n| n.as_str() != font).cloned());
        fonts.families.insert(FontFamily::Name(family.into()), list);
    }
    fonts
}

/// Set up fonts and the look for both light and dark mode.
pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(fonts());

    for (theme, palette) in [(Theme::Light, Palette::light()), (Theme::Dark, Palette::dark())] {
        ctx.style_mut_of(theme, |style| {
            style.visuals = visuals(&palette);
            style.text_styles = [
                (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
                (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
                (TextStyle::Button, medium(14.0)),
                (TextStyle::Heading, semibold(20.0)),
                (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
                (title_style(), semibold(27.0)),
            ]
            .into();
            let s = &mut style.spacing;
            s.item_spacing = egui::vec2(8.0, 8.0);
            s.button_padding = egui::vec2(12.0, 4.0);
            s.interact_size.y = CONTROL_HEIGHT;
            s.window_margin = Margin::same(20);
            s.menu_margin = Margin::same(6);
            s.combo_width = 200.0;
            s.text_edit_width = 260.0;
            s.icon_width = 16.0;
            s.icon_width_inner = 9.0;
            style.interaction.tooltip_delay = 0.4;
            if super::widgets::debug_layout() {
                // Outline every widget and flag anything not on whole pixels.
                style.debug.show_interactive_widgets = true;
                style.debug.show_unaligned = true;
            }
            // Labels are not selectable, so clicks reach the cards around them.
            style.interaction.selectable_labels = false;
        });
    }
}

fn visuals(p: &Palette) -> Visuals {
    let mut v = if p.dark { Visuals::dark() } else { Visuals::light() };
    let r = CornerRadius::same(RADIUS_SMALL);
    v.panel_fill = p.bg;
    v.window_fill = p.surface;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(14);
    v.menu_corner_radius = CornerRadius::same(RADIUS);
    v.window_shadow = Shadow { offset: [0, 8], blur: 28, spread: 0, color: Color32::from_black_alpha(if p.dark { 110 } else { 40 }) };
    v.popup_shadow = Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(if p.dark { 90 } else { 30 }) };
    v.extreme_bg_color = if p.dark { Color32::from_rgb(0x19, 0x19, 0x1f) } else { Color32::WHITE };
    v.faint_bg_color = p.surface_alt;
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = p.accent.gamma_multiply(if p.dark { 0.55 } else { 0.35 });
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.override_text_color = None;
    v.error_fg_color = p.danger;
    v.warn_fg_color = p.warning;
    v.text_cursor.stroke = Stroke::new(2.0, p.accent);
    v.text_options.font_hinting = true;
    v.text_options.color_transfer_function = text_transfer(p.dark);

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.surface;
    w.noninteractive.weak_bg_fill = p.surface;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = r;

    let control = if p.dark { Color32::from_rgb(0x2c, 0x2c, 0x35) } else { Color32::from_rgb(0xf0, 0xf0, 0xf5) };
    let control_hover = if p.dark { Color32::from_rgb(0x36, 0x36, 0x41) } else { Color32::from_rgb(0xe6, 0xe6, 0xee) };
    w.inactive.bg_fill = control;
    w.inactive.weak_bg_fill = control;
    w.inactive.bg_stroke = Stroke::new(1.0, p.border);
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.corner_radius = r;

    w.hovered.bg_fill = control_hover;
    w.hovered.weak_bg_fill = control_hover;
    w.hovered.bg_stroke = Stroke::new(1.0, p.accent.gamma_multiply(0.6));
    w.hovered.fg_stroke = Stroke::new(1.5, p.text);
    w.hovered.corner_radius = r;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = p.accent_soft;
    w.active.weak_bg_fill = p.accent_soft;
    w.active.bg_stroke = Stroke::new(1.0, p.accent);
    w.active.fg_stroke = Stroke::new(1.5, p.text);
    w.active.corner_radius = r;
    w.active.expansion = 0.0;

    w.open.bg_fill = control_hover;
    w.open.weak_bg_fill = control_hover;
    w.open.bg_stroke = Stroke::new(1.0, p.border);
    w.open.fg_stroke = Stroke::new(1.0, p.text);
    w.open.corner_radius = r;
    v
}

/// How glyph coverage turns into text opacity: a steeper curve keeps light
/// text on dark backgrounds from looking thin, a gentle one keeps dark text on
/// light backgrounds firm without looking bold. Debug builds can try others with
/// `MINI_REMOTE_TEXT_TRANSFER=off|tcmcs|gamma:<value>`.
fn text_transfer(dark: bool) -> egui::epaint::FontColorTransferFunction {
    use egui::epaint::FontColorTransferFunction as F;
    if cfg!(debug_assertions)
        && let Ok(v) = std::env::var("MINI_REMOTE_TEXT_TRANSFER")
    {
        match v.as_str() {
            "off" => return F::Off,
            "tcmcs" => return F::TwoCoverageMinusCoverageSq,
            other => {
                if let Some(g) = other.strip_prefix("gamma:").and_then(|g| g.parse().ok()) {
                    return F::Gamma(g);
                }
            }
        }
    }
    // Chosen by comparing zoomed screenshots: plain coverage looks washed out
    // for dark-on-light text, the full curve a little heavy; 0.7 is in between.
    if dark { F::TwoCoverageMinusCoverageSq } else { F::Gamma(0.7) }
}
