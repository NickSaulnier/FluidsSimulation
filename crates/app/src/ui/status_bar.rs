//! Status bar: a one-line summary under the canvas. The details live in the Info tab.

use super::{Frame, theme};

pub fn show(ui: &mut egui::Ui, f: &mut Frame<'_>) {
    let (perf, world) = (f.perf, &*f.world);
    let stats = world.solver.stats();
    ui.horizontal(|ui| {
        ui.label(format!("{:.0} fps", perf.fps));
        ui.weak("·");
        ui.label(format!("sim {:.1} ms", perf.sim_ms));
        ui.weak("·");
        match world.solver.particles() {
            Some(_) => ui.label(format!("{} particles", stats.particles)),
            None => ui.label("grid"),
        };
        ui.weak("·");
        ui.label(format!("t = {:.1} s", world.time));
        if perf.slow_motion > 0.05 {
            ui.weak("·");
            ui.colored_label(
                theme::WARN,
                format!("slow motion {:.0}%", 100.0 * (1.0 - perf.slow_motion)),
            )
            .on_hover_text("The simulation can't keep up with real time at these settings.");
        }
        if f.state.paused {
            ui.weak("·");
            ui.label("paused");
        }
    });
}
