//! Info tab: performance, solver metrics, the step breakdown, and the shortcuts.

use crate::ui::health::{self, Health};
use crate::ui::settings::{self, Settings};
use crate::ui::{Frame, theme};
use egui::{Sense, vec2};
use crate::ui::shortcuts::{Action, MOUSE, SHORTCUTS, tip};
use crate::ui::widgets::{self, row};

pub fn show(ui: &mut egui::Ui, f: &mut Frame<'_>) {
    let perf = f.perf;
    let (name, stats, time) = match f.world3d.as_deref() {
        Some(w) => (w.solver.name(), w.solver.stats(), w.time),
        None => (f.world.solver.name(), f.world.solver.stats(), f.world.time),
    };
    widgets::group(ui, "Performance", |ui| {
        row(ui, "Frame rate", "", |ui| ui.label(format!("{:.0} fps", perf.fps)));
        row(ui, "Sim cost", "", |ui| ui.label(format!("{:.1} ms, {:.1} substeps", perf.sim_ms, perf.substeps)));
        row(ui, "Particles", "", |ui| ui.label(stats.particles.to_string()));
        let slow = if perf.slow_motion > 0.05 { format!("  ({:.0}% speed)", 100.0 * (1.0 - perf.slow_motion)) } else { String::new() };
        row(ui, "Sim time", "", |ui| ui.label(format!("{time:.1} s{slow}")));
    });

    widgets::group(ui, &format!("{name} metrics"), |ui| {
        for m in &stats.metrics {
            let help = match m.tolerance {
                Some(t) => format!("{}: limit {t}", m.name),
                None => m.name.to_string(),
            };
            row(ui, m.name, &help, |ui| {
                if let Some(h) = Health::of(m) {
                    health::dot(ui, h).on_hover_text(&help);
                }
                ui.label(format!("{:.3}", m.value));
                if let Some(t) = m.tolerance {
                    ui.weak(format!("/ {t}"));
                }
            });
        }
    });

    widgets::group(ui, "Step breakdown", |ui| {
        let total: f64 = stats.phases.iter().map(|(_, d)| d.as_secs_f64()).sum();
        for (name, d) in &stats.phases {
            let share = if total > 0.0 { (d.as_secs_f64() / total) as f32 } else { 0.0 };
            row(ui, name, "", |ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(80.0, 10.0), Sense::hover());
                ui.painter().rect_filled(rect, 2, theme::GROUP_FILL.gamma_multiply(1.6));
                let filled = egui::Rect::from_min_size(rect.min, vec2(rect.width() * share, rect.height()));
                ui.painter().rect_filled(filled, 2, theme::ACCENT);
                ui.label(format!("{:.2} ms", d.as_secs_f64() * 1e3));
            });
        }
        row(ui, "total", "", |ui| ui.strong(format!("{:.2} ms", total * 1e3)));
    });

    widgets::group(ui, "Shortcuts", |ui| {
        for sc in SHORTCUTS {
            row(ui, sc.keys, "", |ui| ui.label(sc.what));
        }
        for (keys, what) in MOUSE {
            row(ui, keys, "", |ui| ui.label(*what));
        }
        if ui.button(tip("Open the cheat sheet", Action::ToggleCheatSheet)).clicked() {
            f.state.cheat_sheet = true;
        }
    });

    widgets::group(ui, "Settings", |ui| {
        ui.weak("Tool, brush, look and panel settings are remembered between runs.");
        if let Some(path) = settings::path() {
            ui.weak(format!("Saved on exit to {}", path.display()));
        }
        if ui.button("Reset to defaults").on_hover_text("Restore the default tool, brush, look and panel").clicked() {
            Settings::default().apply(f.state);
        }
    });
}
