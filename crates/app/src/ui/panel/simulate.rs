//! Simulate tab: the world (gravity) and the current solver's parameters.

use crate::ui::Frame;
use crate::ui::compass::{self, MAX_GRAVITY};
use crate::ui::params::EguiParams;
use crate::ui::widgets::{self, row};

/// Gravity: the dial (any direction, and the strength), exact values, and in 3D which way
/// round the vertical it leans.
fn world(ui: &mut egui::Ui, s: &mut crate::ui::UiState, in_3d: bool) {
    widgets::group(ui, "World", |ui| {
        let help = if in_3d {
            "Drag the needle: its angle is how far gravity leans from straight down, its length the strength. The arrow keys turn and strengthen it."
        } else {
            "Drag the needle to point gravity any way: its length is the strength. The arrow keys turn and strengthen it."
        };
        row(ui, "Gravity", help, |ui| {
            compass::compass(ui, &mut s.gravity_deg, &mut s.gravity_mag);
            ui.vertical(|ui| {
                ui.add(egui::DragValue::new(&mut s.gravity_mag).range(0.0..=MAX_GRAVITY).speed(0.05).suffix(" m/s²"))
                    .on_hover_text("Strength (0: weightless)");
                ui.add(egui::DragValue::new(&mut s.gravity_deg).range(-180.0..=180.0).speed(0.5).suffix("°"))
                    .on_hover_text("Direction: 0° is straight down, 90° right, 180° straight up");
                if ui.small_button("Down").on_hover_text("Straight down again").clicked() {
                    s.gravity_deg = 0.0;
                }
            });
        });
        if in_3d {
            row(ui, "Heading", "Which way round the vertical gravity leans (0° toward +x)", |ui| {
                ui.add(egui::Slider::new(&mut s.gravity_heading, 0.0..=360.0).suffix("°"));
            });
        }
    });
}

pub fn show(ui: &mut egui::Ui, f: &mut Frame<'_>) {
    world(ui, f.state, f.world3d.is_some());
    if let Some(w) = f.world3d.as_deref_mut() {
        ui.add_space(4.0);
        widgets::title(ui, w.solver.name());
        let mut params = EguiParams::new(ui);
        w.solver.visit_params(&mut params);
        if params.finish() {
            w.reset();
        }
        ui.weak("Left drag orbits the camera; the wheel zooms.");
        return;
    }
    ui.add_space(4.0);
    widgets::title(ui, f.world.solver.name());
    let mut params = EguiParams::new(ui);
    f.world.solver.visit_params(&mut params);
    if params.finish() {
        f.world.reset();
    }
}
