//! Status bar: a one-line summary under the canvas. The details live in the Info tab.

use super::health::{self, Health};
use super::{Frame, Tab, theme};

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
        // Health of the solver's checked metrics; a click opens the details.
        if let Some((h, metric)) = health::worst(&stats.metrics) {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let text = match h {
                    Health::Ok => "all checks OK".to_string(),
                    Health::Near => format!("{} near its limit", metric.name),
                    Health::Over => format!("{} over its limit", metric.name),
                };
                let colour = if h == Health::Ok { ui.visuals().text_color() } else { h.colour() };
                let clicked = ui
                    .add(egui::Button::new(egui::RichText::new(text).color(colour)).frame(false))
                    .on_hover_text("Solver checks: open the Info tab for details")
                    .clicked();
                health::dot(ui, h);
                if clicked {
                    f.state.panel_open = true;
                    f.state.tab = Tab::Info;
                }
            });
        }
    });
}
