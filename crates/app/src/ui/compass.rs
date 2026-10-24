//! Gravity compass: drag the needle to set gravity's direction and strength in one
//! gesture. The needle points along gravity (down = straight down); its length is the
//! strength, up to `MAX_GRAVITY` at the rim. Gravity can tilt up to 90° either way, so
//! the usable half of the dial is the lower one. Double-click resets to Earth gravity.

use super::theme;
use egui::{Color32, Sense, Stroke, Vec2, vec2};

pub const MAX_GRAVITY: f32 = 20.0;
pub const EARTH: f32 = 9.81;
const SIZE: f32 = 64.0;

/// Gravity (tilt in degrees, positive toward +x; magnitude) from a pointer offset from
/// the dial's centre, in screen coordinates (y down), for a dial of `radius`.
pub fn gravity_from_offset(offset: Vec2, radius: f32) -> (f32, f32) {
    let tilt = offset.x.atan2(offset.y).to_degrees().clamp(-90.0, 90.0);
    let magnitude = (offset.length() / radius).min(1.0) * MAX_GRAVITY;
    (tilt, magnitude)
}

/// The needle tip's offset from the centre (screen coordinates) for a gravity.
pub fn offset_from_gravity(tilt_deg: f32, magnitude: f32, radius: f32) -> Vec2 {
    let a = tilt_deg.to_radians();
    vec2(a.sin(), a.cos()) * (magnitude / MAX_GRAVITY).clamp(0.0, 1.0) * radius
}

/// The dial. Returns its response, changed when dragged or reset.
pub fn compass(ui: &mut egui::Ui, tilt_deg: &mut f32, magnitude: &mut f32) -> egui::Response {
    let (rect, mut response) = ui.allocate_exact_size(vec2(SIZE, SIZE), Sense::click_and_drag());
    let centre = rect.center();
    let radius = SIZE / 2.0 - 3.0;

    if response.double_clicked() {
        (*tilt_deg, *magnitude) = (0.0, EARTH);
        response.mark_changed();
    } else if let Some(pointer) = response.interact_pointer_pos().filter(|_| response.dragged() || response.clicked()) {
        (*tilt_deg, *magnitude) = gravity_from_offset(pointer - centre, radius);
        response.mark_changed();
    }

    let painter = ui.painter_at(rect);
    let visuals = ui.visuals();
    painter.circle_filled(centre, radius, visuals.extreme_bg_color);
    // The reachable lower half, and a ring at Earth gravity for reference.
    let half: Vec<_> = (0..=32)
        .map(|k| centre + offset_from_gravity(-90.0 + 180.0 * k as f32 / 32.0, MAX_GRAVITY, radius))
        .chain(std::iter::once(centre))
        .collect();
    painter.add(egui::Shape::convex_polygon(half, theme::GROUP_FILL.gamma_multiply(1.4), Stroke::NONE));
    painter.circle_stroke(centre, radius * EARTH / MAX_GRAVITY, Stroke::new(1.0, Color32::from_gray(80)));
    painter.circle_stroke(centre, radius, Stroke::new(1.0, Color32::from_gray(90)));

    let tip = centre + offset_from_gravity(*tilt_deg, *magnitude, radius);
    let colour = if response.hovered() || response.dragged() { Color32::WHITE } else { theme::ACCENT };
    painter.line_segment([centre, tip], Stroke::new(2.0, colour));
    painter.circle_filled(tip, 3.5, colour);
    painter.circle_filled(centre, 2.0, Color32::from_gray(160));

    response.on_hover_text("Drag to set gravity's direction and strength. The ring marks Earth gravity; double-click to reset.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn straight_down_at_the_earth_ring_is_earth_gravity() {
        let r = 30.0;
        let (tilt, mag) = gravity_from_offset(vec2(0.0, r * EARTH / MAX_GRAVITY), r);
        assert!(tilt.abs() < 1e-4 && (mag - EARTH).abs() < 1e-3, "{tilt} {mag}");
    }

    #[test]
    fn right_of_centre_tilts_toward_plus_x_and_up_is_clamped() {
        assert!((gravity_from_offset(vec2(10.0, 10.0), 30.0).0 - 45.0).abs() < 1e-3);
        assert_eq!(gravity_from_offset(vec2(5.0, -20.0), 30.0).0, 90.0);
        assert_eq!(gravity_from_offset(vec2(-5.0, -20.0), 30.0).0, -90.0);
        assert_eq!(gravity_from_offset(vec2(0.0, 500.0), 30.0).1, MAX_GRAVITY);
    }

    #[test]
    fn offset_and_gravity_are_inverses() {
        for (tilt, mag) in [(0.0, 9.81), (30.0, 5.0), (-75.0, 18.0)] {
            let (t, m) = gravity_from_offset(offset_from_gravity(tilt, mag, 30.0), 30.0);
            assert!((t - tilt).abs() < 1e-3 && (m - mag).abs() < 1e-3, "{tilt},{mag} -> {t},{m}");
        }
    }
}
