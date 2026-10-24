//! Simulate tab: the world (gravity) and the current solver's parameters.

use crate::ui::Frame;
use crate::ui::params::EguiParams;

pub fn show(ui: &mut egui::Ui, f: &mut Frame<'_>) {
    let s = &mut *f.state;
    ui.strong("World");
    ui.horizontal(|ui| {
        ui.add(egui::Slider::new(&mut s.gravity_deg, -90.0..=90.0).text("Tilt").suffix("°"))
            .on_hover_text("Left/Right arrows tilt too");
        if ui.small_button("Level").clicked() {
            s.gravity_deg = 0.0;
        }
    });
    ui.add(egui::Slider::new(&mut s.gravity_mag, 0.0..=20.0).text("Gravity").suffix(" m/s²"));
    ui.separator();

    ui.strong(f.world.solver.name());
    let mut params = EguiParams { ui, restart_requested: false };
    f.world.solver.visit_params(&mut params);
    if params.restart_requested {
        f.world.reset();
    }
}
