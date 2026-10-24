//! Info tab: performance, solver metrics, the step breakdown, and the controls.

use crate::ui::Frame;

pub fn show(ui: &mut egui::Ui, f: &mut Frame<'_>) {
    let (world, perf) = (&*f.world, f.perf);
    let stats = world.solver.stats();
    ui.strong("Performance");
    egui::Grid::new("perf").num_columns(2).show(ui, |ui| {
        ui.label("Frame rate");
        ui.label(format!("{:.0} fps", perf.fps));
        ui.end_row();
        ui.label("Sim cost");
        ui.label(format!("{:.1} ms, {:.1} substeps", perf.sim_ms, perf.substeps));
        ui.end_row();
        ui.label("Particles");
        ui.label(stats.particles.to_string());
        ui.end_row();
        ui.label("Sim time");
        let slow = if perf.slow_motion > 0.05 { format!("  ({:.0}% speed)", 100.0 * (1.0 - perf.slow_motion)) } else { String::new() };
        ui.label(format!("{:.1} s{slow}", world.time));
        ui.end_row();
    });
    ui.separator();

    ui.strong(format!("{} metrics", world.solver.name()));
    egui::Grid::new("metrics").num_columns(2).show(ui, |ui| {
        for m in &stats.metrics {
            ui.label(m.name);
            ui.label(format!("{:.3}", m.value));
            ui.end_row();
        }
    });
    egui::CollapsingHeader::new("Step breakdown").default_open(false).show(ui, |ui| {
        egui::Grid::new("phases").num_columns(2).show(ui, |ui| {
            for (name, d) in &stats.phases {
                ui.label(*name);
                ui.label(format!("{:.2} ms", d.as_secs_f64() * 1e3));
                ui.end_row();
            }
        });
    });
    ui.separator();

    ui.strong("Controls");
    ui.label(
        "Left drag: selected tool (Faucet pours along the drag)\nRight drag: push\nSpace: pause   S: step   R: reset\n\
         P: next preset   N: next solver\nO: next overlay   H: hide/show particles\nL: liquid surface on/off   M: play/pause music\n\
         `: hide/show panel\nLeft/Right arrows: tilt gravity\nDrop an audio file to load music",
    );
}
