//! View tab: how the fluid is drawn, and the field overlay.

use crate::ui::Frame;
use crate::ui::shortcuts::{Action, tip};
use crate::ui::widgets::{self, row};

pub fn show(ui: &mut egui::Ui, f: &mut Frame<'_>) {
    let s = &mut *f.state;
    let solver = &f.world.solver;
    widgets::group(ui, "Display", |ui| {
        // Grid solvers have no particles to draw: they show a field (the dye) instead.
        if solver.particles().is_none() {
            let field = solver.fields().first().map_or("grid", |f| f.label);
            ui.weak(format!("{} is a grid solver: it draws its {} field.", solver.name(), field.to_lowercase()));
            return;
        }
        widgets::toggle(
            ui,
            &mut s.liquid,
            &tip("Liquid surface", Action::ToggleLiquid),
            "Blend particles into a continuous surface instead of drawing discs.",
        );
        if s.liquid {
            row(ui, "Blob size", "Splat radius in particle radii. Bigger = smoother, blobbier.", |ui| {
                ui.add(egui::Slider::new(&mut s.blob_size, 1.5..=6.0));
            });
            row(ui, "Surface level", "Where the surface sits, as a fraction of interior density. Lower = puffier.", |ui| {
                ui.add(egui::Slider::new(&mut s.surface_threshold, 0.15..=0.9));
            });
        }
        widgets::toggle(ui, &mut s.show_particles, &tip("Show particles", Action::ToggleParticles), "");
    });
    widgets::group(ui, "Overlay", |ui| overlay(ui, f));
}

fn overlay(ui: &mut egui::Ui, f: &mut Frame<'_>) {
    let (world, s) = (&*f.world, &mut *f.state);
    let fields = world.solver.fields();
    let current = s.overlay.and_then(|key| fields.iter().find(|f| f.key == key));
    row(ui, "Field", &tip("Solver field drawn over the scene; cycle with", Action::NextOverlay), |ui| {
        egui::ComboBox::from_id_salt("overlay").selected_text(current.map_or("None", |f| f.label)).width(150.0).show_ui(
            ui,
            |ui| {
                ui.selectable_value(&mut s.overlay, None, "None");
                for f in fields {
                    ui.selectable_value(&mut s.overlay, Some(f.key), f.label);
                }
            },
        );
    });
    if fields.is_empty() {
        ui.weak(format!("{} has no grid fields", world.solver.name()));
    }
    // The legend for the field is on the canvas, next to what it describes.
    if current.is_some() {
        row(ui, "Opacity", "", |ui| ui.add(egui::Slider::new(&mut s.overlay_opacity, 0.1..=1.0)));
    }
}
