//! The egui interface: a top bar (run, scene, solver, tools), a status bar, and a tabbed
//! side panel. Solver parameters come from `Solver::visit_params`, so nothing here names
//! a specific solver. Whatever is left after the bars and panel is the canvas; its rect
//! is returned so the scene view is fitted into exactly that area.

mod panel;
mod params;
mod status_bar;
pub mod theme;
mod top_bar;
mod widgets;

use crate::audio::Music;
use sim::{Vec2, World};

/// Side panel width in points.
pub const PANEL_WIDTH: f32 = 320.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Push,
    PaintWall,
    EraseWall,
    /// Hold to pour fluid from the cursor, in the direction the mouse moves.
    Faucet,
}

/// Side panel tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Simulate,
    View,
    Music,
    Info,
}

pub struct UiState {
    pub paused: bool,
    pub step_once: bool,
    /// Simulated seconds per real second.
    pub speed: f32,
    pub tool: Tool,
    pub brush_radius: f32,
    /// Push brush blend rate, 1/s.
    pub push_strength: f32,
    /// Gravity tilt: 0 = straight down, positive leans toward +x.
    pub gravity_deg: f32,
    pub gravity_mag: f32,
    pub keep_particles: bool,
    /// Key of the solver field shown as an overlay (by key, so it survives switching
    /// to a solver that lists its fields in a different order, or not at all).
    pub overlay: Option<&'static str>,
    pub overlay_opacity: f32,
    pub show_particles: bool,
    pub faucet_speed: f32,
    /// Draw particles as a continuous liquid surface.
    pub liquid: bool,
    pub blob_size: f32,
    pub surface_threshold: f32,
    pub tab: Tab,
    pub panel_open: bool,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            paused: false,
            step_once: false,
            speed: 1.0,
            tool: Tool::Push,
            brush_radius: 0.08,
            push_strength: 25.0,
            gravity_deg: 0.0,
            gravity_mag: 9.81,
            keep_particles: true,
            overlay: None,
            overlay_opacity: 0.85,
            show_particles: true,
            faucet_speed: 2.0,
            liquid: true,
            blob_size: 3.0,
            surface_threshold: 0.5,
            tab: Tab::Simulate,
            panel_open: true,
        }
    }
}

impl UiState {
    pub fn liquid_style(&self) -> Option<crate::render::LiquidStyle> {
        self.liquid
            .then_some(crate::render::LiquidStyle { splat_scale: self.blob_size, threshold: self.surface_threshold })
    }

    pub fn gravity(&self) -> Vec2 {
        let a = self.gravity_deg.to_radians();
        Vec2::new(a.sin(), -a.cos()) * self.gravity_mag
    }

    /// The inverse of `gravity`: point the sliders at a scene's own gravity.
    pub fn set_gravity(&mut self, g: Vec2) {
        self.gravity_mag = g.length();
        self.gravity_deg = if g == Vec2::ZERO { 0.0 } else { g.x.atan2(-g.y).to_degrees().clamp(-90.0, 90.0) };
    }
}

/// Frame-rate numbers averaged over a short window.
#[derive(Clone, Copy, Default)]
pub struct Perf {
    pub fps: f32,
    pub sim_ms: f32,
    pub substeps: f32,
    /// Fraction of real time the sim couldn't keep up with (0 = real time).
    pub slow_motion: f32,
}

/// What the UI needs to draw the brush outline under the cursor.
#[derive(Clone, Copy)]
pub struct Cursor {
    /// Physical pixels.
    pub pos: Vec2,
    pub px_per_metre: f32,
}

/// Everything the interface reads or changes in one frame.
pub struct Frame<'a> {
    pub world: &'a mut World,
    pub state: &'a mut UiState,
    pub music: &'a mut Music,
    pub perf: &'a Perf,
    /// Value range of the overlay field drawn last frame, for its legend.
    pub field_range: Option<(f32, f32)>,
}

/// Lay out the whole interface and return the canvas rect (points): the area left over
/// for the scene.
pub fn layout(ui: &mut egui::Ui, f: &mut Frame<'_>, cursor: Option<Cursor>) -> egui::Rect {
    egui::Panel::top("top_bar").show(ui, |ui| top_bar::show(ui, f));
    egui::Panel::bottom("status_bar").show(ui, |ui| status_bar::show(ui, f));
    if f.state.panel_open {
        egui::Panel::right("side_panel").resizable(false).exact_size(PANEL_WIDTH).show(ui, |ui| panel::show(ui, f));
    }
    let canvas = ui.available_rect_before_wrap();

    // Brush outline, clipped to the canvas so it never draws over the controls.
    if let Some(c) = cursor {
        let ppp = ui.ctx().pixels_per_point();
        let centre = egui::pos2(c.pos.x / ppp, c.pos.y / ppp);
        let colour = match f.state.tool {
            Tool::Push => egui::Color32::from_rgb(120, 200, 255),
            Tool::PaintWall => egui::Color32::from_rgb(230, 230, 230),
            Tool::EraseWall => egui::Color32::from_rgb(255, 120, 120),
            Tool::Faucet => egui::Color32::from_rgb(120, 255, 170),
        };
        ui.painter().with_clip_rect(canvas).circle_stroke(
            centre,
            f.state.brush_radius * c.px_per_metre / ppp,
            egui::Stroke::new(1.5, colour),
        );
    }
    canvas
}

/// Cycle None -> each field -> None.
pub fn next_overlay(current: Option<&'static str>, fields: &[sim::FieldInfo]) -> Option<&'static str> {
    let next = match current.and_then(|key| fields.iter().position(|f| f.key == key)) {
        None => 0,
        Some(k) => k + 1,
    };
    fields.get(next).map(|f| f.key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_cycle_wraps_through_none() {
        let fields = [
            sim::FieldInfo { key: "a", label: "A", unit: "", kind: sim::FieldKind::Signed },
            sim::FieldInfo { key: "b", label: "B", unit: "", kind: sim::FieldKind::Signed },
        ];
        assert_eq!(next_overlay(None, &fields), Some("a"));
        assert_eq!(next_overlay(Some("a"), &fields), Some("b"));
        assert_eq!(next_overlay(Some("b"), &fields), None);
        // A key the current solver doesn't have (after switching) restarts the cycle.
        assert_eq!(next_overlay(Some("gone"), &fields), Some("a"));
        assert_eq!(next_overlay(None, &[]), None);
    }

    #[test]
    fn gravity_round_trips_through_the_sliders() {
        let mut s = UiState::default();
        for g in [Vec2::new(0.0, -9.81), Vec2::new(3.0, -4.0), Vec2::ZERO] {
            s.set_gravity(g);
            assert!((s.gravity() - g).length() < 1e-4, "{g} -> {}", s.gravity());
        }
    }
}
