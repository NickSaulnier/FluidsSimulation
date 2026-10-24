//! View tab: how the fluid is drawn, and the field overlay.

use crate::ui::Frame;
use crate::ui::widgets::{self, row};

pub fn show(ui: &mut egui::Ui, f: &mut Frame<'_>) {
    let s = &mut *f.state;
    widgets::group(ui, "Display", |ui| {
        widgets::toggle(ui, &mut s.liquid, "Liquid surface (L)", "Blend particles into a continuous surface instead of drawing discs.");
        if s.liquid {
            row(ui, "Blob size", "Splat radius in particle radii. Bigger = smoother, blobbier.", |ui| {
                ui.add(egui::Slider::new(&mut s.blob_size, 1.5..=6.0));
            });
            row(ui, "Surface level", "Where the surface sits, as a fraction of interior density. Lower = puffier.", |ui| {
                ui.add(egui::Slider::new(&mut s.surface_threshold, 0.15..=0.9));
            });
        }
        widgets::toggle(ui, &mut s.show_particles, "Show particles (H)", "");
    });
    widgets::group(ui, "Overlay", |ui| overlay(ui, f));
}

fn overlay(ui: &mut egui::Ui, f: &mut Frame<'_>) {
    let (world, s) = (&*f.world, &mut *f.state);
    let fields = world.solver.fields();
    let current = s.overlay.and_then(|key| fields.iter().find(|f| f.key == key));
    row(ui, "Field", "Solver field drawn over the scene (O cycles)", |ui| {
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
    } else if s.overlay.is_none() && world.solver.particles().is_none() {
        ui.weak(format!("Showing {}: {} has no particles", fields[0].label.to_lowercase(), world.solver.name()));
    }
    if let (Some(field), Some((lo, hi))) = (current, f.field_range) {
        row(ui, "Opacity", "", |ui| ui.add(egui::Slider::new(&mut s.overlay_opacity, 0.1..=1.0)));
        let legend = match field.kind {
            sim::FieldKind::Categorical => "grey: air   blue: fluid   dark: solid".to_string(),
            sim::FieldKind::Concentration => "transparent: none   white: full".to_string(),
            _ => format!("{lo:.3} .. {hi:.3} {}", field.unit),
        };
        row(ui, "Range", "", |ui| ui.label(legend));
    }
}
