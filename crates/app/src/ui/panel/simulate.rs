//! Simulate tab: the world (gravity) and the current solver's parameters.

use crate::ui::Frame;
use crate::ui::params::EguiParams;
use crate::ui::widgets::{self, row};

pub fn show(ui: &mut egui::Ui, f: &mut Frame<'_>) {
    let s = &mut *f.state;
    widgets::group(ui, "World", |ui| {
        row(ui, "Gravity", "Strength of gravity", |ui| {
            ui.add(egui::Slider::new(&mut s.gravity_mag, 0.0..=20.0).suffix(" m/s²"));
        });
        row(ui, "Tilt", "Direction of gravity (the Left and Right arrow keys tilt it too)", |ui| {
            ui.spacing_mut().slider_width = 50.0; // leave room for "Level"
            ui.add(egui::Slider::new(&mut s.gravity_deg, -90.0..=90.0).suffix("°"));
            if ui.small_button("Level").clicked() {
                s.gravity_deg = 0.0;
            }
        });
    });

    ui.add_space(4.0);
    widgets::title(ui, f.world.solver.name());
    let mut params = EguiParams::new(ui);
    f.world.solver.visit_params(&mut params);
    if params.finish() {
        f.world.reset();
    }
}
