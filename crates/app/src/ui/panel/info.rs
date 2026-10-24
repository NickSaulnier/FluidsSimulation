//! Info tab: performance, solver metrics, the step breakdown, and the controls.

use crate::ui::Frame;
use crate::ui::widgets::{self, row};

pub fn show(ui: &mut egui::Ui, f: &mut Frame<'_>) {
    let (world, perf) = (&*f.world, f.perf);
    let stats = world.solver.stats();
    widgets::group(ui, "Performance", |ui| {
        row(ui, "Frame rate", "", |ui| ui.label(format!("{:.0} fps", perf.fps)));
        row(ui, "Sim cost", "", |ui| ui.label(format!("{:.1} ms, {:.1} substeps", perf.sim_ms, perf.substeps)));
        row(ui, "Particles", "", |ui| ui.label(stats.particles.to_string()));
        let slow = if perf.slow_motion > 0.05 { format!("  ({:.0}% speed)", 100.0 * (1.0 - perf.slow_motion)) } else { String::new() };
        row(ui, "Sim time", "", |ui| ui.label(format!("{:.1} s{slow}", world.time)));
    });

    widgets::group(ui, &format!("{} metrics", world.solver.name()), |ui| {
        for m in &stats.metrics {
            row(ui, m.name, m.name, |ui| ui.label(format!("{:.3}", m.value)));
        }
        egui::CollapsingHeader::new("Step breakdown").default_open(false).show(ui, |ui| {
            for (name, d) in &stats.phases {
                row(ui, name, "", |ui| ui.label(format!("{:.2} ms", d.as_secs_f64() * 1e3)));
            }
        });
    });

    widgets::group(ui, "Controls", |ui| {
        for (keys, action) in [
            ("Left drag", "selected tool"),
            ("Right drag", "push"),
            ("Space", "pause / play"),
            ("S", "step (while paused)"),
            ("R", "reset"),
            ("P / N", "next preset / solver"),
            ("O", "next overlay"),
            ("H", "hide / show particles"),
            ("L", "liquid surface on / off"),
            ("M", "play / pause music"),
            ("`", "hide / show this panel"),
            ("Left / Right", "tilt gravity"),
            ("Drop a file", "load music"),
        ] {
            row(ui, keys, "", |ui| ui.label(action));
        }
    });
}
