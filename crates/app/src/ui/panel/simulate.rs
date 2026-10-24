//! Simulate tab: the world (gravity) and the current solver's parameters.

use crate::ui::Frame;
use crate::ui::compass::{self, MAX_GRAVITY};
use crate::ui::params::EguiParams;
use crate::ui::widgets::{self, row};

pub fn show(ui: &mut egui::Ui, f: &mut Frame<'_>) {
    let s = &mut *f.state;
    widgets::group(ui, "World", |ui| {
        row(ui, "Gravity", "Drag the needle, or type exact values (the Left and Right arrow keys tilt too).", |ui| {
            compass::compass(ui, &mut s.gravity_deg, &mut s.gravity_mag);
            ui.vertical(|ui| {
                ui.add(egui::DragValue::new(&mut s.gravity_mag).range(0.0..=MAX_GRAVITY).speed(0.05).suffix(" m/s²"))
                    .on_hover_text("Strength");
                ui.add(egui::DragValue::new(&mut s.gravity_deg).range(-90.0..=90.0).speed(0.5).suffix("°"))
                    .on_hover_text("Tilt: 0° is straight down, positive leans right");
                if ui.small_button("Level").clicked() {
                    s.gravity_deg = 0.0;
                }
            });
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
