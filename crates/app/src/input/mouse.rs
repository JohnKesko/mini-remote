//! Mouse: where the remote picture sits in the window, mapping positions to
//! remote pixels, and turning smooth scrolling into wheel clicks.

use egui::{Pos2, Rect, Vec2, pos2};
use rfb::buttons;

use crate::model::Scaling;

/// Where the remote picture is drawn, in window points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewLayout {
    /// The picture rectangle (may be larger than the visible area when panning).
    pub rect: Rect,
    /// Points per remote pixel.
    pub scale: f32,
    /// True when one remote pixel is exactly one screen pixel.
    pub pixel_exact: bool,
}

/// Place a `remote` sized picture into `area`.
///
/// `points_per_pixel` is 1 / (screen pixels per point). `pointer` is used to
/// pan around a picture that is larger than the area in actual-size mode.
pub fn layout(
    area: Rect,
    remote: Vec2,
    scaling: Scaling,
    points_per_pixel: f32,
    pointer: Option<Pos2>,
) -> ViewLayout {
    if remote.x <= 0.0 || remote.y <= 0.0 || area.width() <= 0.0 || area.height() <= 0.0 {
        return ViewLayout { rect: Rect::from_min_size(area.min, Vec2::ZERO), scale: 1.0, pixel_exact: true };
    }
    match scaling {
        Scaling::Fit => {
            let scale = (area.width() / remote.x).min(area.height() / remote.y);
            let size = remote * scale;
            let mut rect = Rect::from_center_size(area.center(), size);
            // Snap to whole screen pixels so the picture stays crisp.
            rect = snap(rect, points_per_pixel);
            let pixel_exact = (scale - points_per_pixel).abs() < 1e-4;
            ViewLayout { rect, scale, pixel_exact }
        }
        Scaling::Actual => {
            let scale = points_per_pixel;
            let size = remote * scale;
            let mut min = pos2(0.0, 0.0);
            for axis in 0..2 {
                let (a_min, a_len, len) = (area.min[axis], area.size()[axis], size[axis]);
                min[axis] = if len <= a_len {
                    a_min + (a_len - len) / 2.0
                } else {
                    // Pan with the mouse: at the left edge show the left side, and so on.
                    let t = pointer.map_or(0.5, |p| ((p[axis] - a_min) / a_len).clamp(0.0, 1.0));
                    a_min - (len - a_len) * t
                };
            }
            let rect = snap(Rect::from_min_size(min, size), points_per_pixel);
            ViewLayout { rect, scale, pixel_exact: true }
        }
    }
}

fn snap(rect: Rect, points_per_pixel: f32) -> Rect {
    if points_per_pixel <= 0.0 {
        return rect;
    }
    let ppp = 1.0 / points_per_pixel;
    let min = pos2((rect.min.x * ppp).round() / ppp, (rect.min.y * ppp).round() / ppp);
    Rect::from_min_size(min, rect.size())
}

/// Remote pixel under a window position, clamped to the remote screen.
pub fn to_remote(pos: Pos2, view: &ViewLayout, remote_w: u16, remote_h: u16) -> (u16, u16) {
    if remote_w == 0 || remote_h == 0 || view.scale <= 0.0 {
        return (0, 0);
    }
    let x = ((pos.x - view.rect.min.x) / view.scale).floor();
    let y = ((pos.y - view.rect.min.y) / view.scale).floor();
    let x = x.clamp(0.0, f32::from(remote_w - 1)) as u16;
    let y = y.clamp(0.0, f32::from(remote_h - 1)) as u16;
    (x, y)
}

/// Window position of a remote pixel (top-left corner of it).
#[cfg(test)]
pub fn from_remote(x: f32, y: f32, view: &ViewLayout) -> Pos2 {
    view.rect.min + egui::vec2(x, y) * view.scale
}

/// VNC button bit for an egui mouse button.
pub fn button_bit(button: egui::PointerButton) -> u16 {
    match button {
        egui::PointerButton::Primary => buttons::LEFT,
        egui::PointerButton::Secondary => buttons::RIGHT,
        egui::PointerButton::Middle => buttons::MIDDLE,
        egui::PointerButton::Extra1 => buttons::BACK,
        egui::PointerButton::Extra2 => buttons::FORWARD,
    }
}

/// How far (in points) a trackpad must scroll for one wheel click.
const POINTS_PER_CLICK: f32 = 30.0;
/// Upper limit so a big fling does not flood the remote computer.
const MAX_CLICKS_PER_EVENT: u32 = 10;

/// Collects smooth scrolling until it adds up to whole wheel clicks.
#[derive(Clone, Copy, Debug, Default)]
pub struct WheelAccumulator {
    acc: Vec2,
}

/// Wheel clicks to send: one button bit per click.
pub type WheelClicks = Vec<u16>;

impl WheelAccumulator {
    /// Add one scroll event. `delta` follows egui: positive y moves the
    /// content down (scrolling up), positive x moves it right (scrolling left).
    pub fn add(&mut self, unit: egui::MouseWheelUnit, delta: Vec2) -> WheelClicks {
        let per_click = match unit {
            egui::MouseWheelUnit::Point => POINTS_PER_CLICK,
            egui::MouseWheelUnit::Line => 1.0,
            egui::MouseWheelUnit::Page => 1.0 / 3.0,
        };
        let mut out = Vec::new();
        let axes = [(1usize, buttons::WHEEL_UP, buttons::WHEEL_DOWN), (0, buttons::WHEEL_LEFT, buttons::WHEEL_RIGHT)];
        for (axis, positive, negative) in axes {
            let d = delta[axis] / per_click;
            if d == 0.0 {
                continue;
            }
            // Changing direction starts afresh, so a reversal reacts at once.
            if self.acc[axis] != 0.0 && self.acc[axis].signum() != d.signum() {
                self.acc[axis] = 0.0;
            }
            self.acc[axis] += d;
            let whole = self.acc[axis].trunc();
            if whole != 0.0 {
                self.acc[axis] -= whole;
                let count = (whole.abs() as u32).min(MAX_CLICKS_PER_EVENT);
                let bit = if whole > 0.0 { positive } else { negative };
                out.extend(std::iter::repeat_n(bit, count as usize));
            }
        }
        out
    }

    pub fn reset(&mut self) {
        self.acc = Vec2::ZERO;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{MouseWheelUnit, vec2};

    #[test]
    fn fit_scales_down_and_centres() {
        let area = Rect::from_min_size(pos2(0.0, 40.0), vec2(1000.0, 600.0));
        let v = layout(area, vec2(1920.0, 1080.0), Scaling::Fit, 0.5, None);
        assert!((v.scale - 1000.0 / 1920.0).abs() < 1e-5);
        assert!((v.rect.width() - 1000.0).abs() < 1e-3);
        assert!((v.rect.center().y - 340.0).abs() < 1.0);
        assert!(!v.pixel_exact);
    }

    #[test]
    fn actual_size_is_pixel_exact() {
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 800.0));
        let v = layout(area, vec2(800.0, 600.0), Scaling::Actual, 0.5, None);
        assert_eq!(v.scale, 0.5);
        assert!(v.pixel_exact);
        assert_eq!(v.rect.size(), vec2(400.0, 300.0));
        assert_eq!(v.rect.center(), pos2(500.0, 400.0));
    }

    #[test]
    fn actual_size_pans_with_the_mouse() {
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(500.0, 500.0));
        let left = layout(area, vec2(1000.0, 400.0), Scaling::Actual, 1.0, Some(pos2(0.0, 250.0)));
        assert_eq!(left.rect.min.x, 0.0);
        let right = layout(area, vec2(1000.0, 400.0), Scaling::Actual, 1.0, Some(pos2(500.0, 250.0)));
        assert_eq!(right.rect.max.x, 500.0);
    }

    #[test]
    fn position_mapping() {
        let view = ViewLayout { rect: Rect::from_min_size(pos2(100.0, 50.0), vec2(960.0, 540.0)), scale: 0.5, pixel_exact: false };
        assert_eq!(to_remote(pos2(100.0, 50.0), &view, 1920, 1080), (0, 0));
        assert_eq!(to_remote(pos2(580.0, 320.0), &view, 1920, 1080), (960, 540));
        // Outside the picture is clamped to the edge.
        assert_eq!(to_remote(pos2(0.0, 0.0), &view, 1920, 1080), (0, 0));
        assert_eq!(to_remote(pos2(5000.0, 5000.0), &view, 1920, 1080), (1919, 1079));
        let p = from_remote(960.0, 540.0, &view);
        assert_eq!(p, pos2(580.0, 320.0));
    }

    #[test]
    fn wheel_lines_are_clicks() {
        let mut w = WheelAccumulator::default();
        assert_eq!(w.add(MouseWheelUnit::Line, vec2(0.0, 1.0)), vec![buttons::WHEEL_UP]);
        assert_eq!(w.add(MouseWheelUnit::Line, vec2(0.0, -2.0)), vec![buttons::WHEEL_DOWN, buttons::WHEEL_DOWN]);
        assert_eq!(w.add(MouseWheelUnit::Line, vec2(1.0, 0.0)), vec![buttons::WHEEL_LEFT]);
        assert_eq!(w.add(MouseWheelUnit::Line, vec2(-1.0, 0.0)), vec![buttons::WHEEL_RIGHT]);
    }

    #[test]
    fn trackpad_adds_up() {
        let mut w = WheelAccumulator::default();
        for _ in 0..2 {
            assert!(w.add(MouseWheelUnit::Point, vec2(0.0, -10.0)).is_empty());
        }
        assert_eq!(w.add(MouseWheelUnit::Point, vec2(0.0, -10.0)), vec![buttons::WHEEL_DOWN]);
        // Reversing direction drops what was collected the other way.
        assert!(w.add(MouseWheelUnit::Point, vec2(0.0, -20.0)).is_empty());
        assert!(w.add(MouseWheelUnit::Point, vec2(0.0, 20.0)).is_empty());
        assert_eq!(w.add(MouseWheelUnit::Point, vec2(0.0, 10.0)), vec![buttons::WHEEL_UP]);
    }

    #[test]
    fn big_flings_are_limited() {
        let mut w = WheelAccumulator::default();
        assert_eq!(w.add(MouseWheelUnit::Point, vec2(0.0, 100_000.0)).len(), MAX_CLICKS_PER_EVENT as usize);
    }

    #[test]
    fn fractional_lines() {
        let mut w = WheelAccumulator::default();
        assert!(w.add(MouseWheelUnit::Line, vec2(0.0, 0.4)).is_empty());
        assert_eq!(w.add(MouseWheelUnit::Line, vec2(0.0, 0.7)), vec![buttons::WHEEL_UP]);
    }
}
