//! View tab: how the fluid is drawn, and the field overlay.

use crate::ui::Frame;

pub fn show(ui: &mut egui::Ui, f: &mut Frame<'_>) {
    let s = &mut *f.state;
    ui.strong("Display");
    ui.checkbox(&mut s.liquid, "Liquid surface (L)")
        .on_hover_text("Blend particles into a continuous surface instead of drawing discs.");
    if s.liquid {
        ui.add(egui::Slider::new(&mut s.blob_size, 1.5..=6.0).text("Blob size"))
            .on_hover_text("Splat radius in particle radii. Bigger = smoother, blobbier.");
        ui.add(egui::Slider::new(&mut s.surface_threshold, 0.15..=0.9).text("Surface level"))
            .on_hover_text("Where the surface sits, as a fraction of interior density. Lower = puffier.");
    }
    ui.checkbox(&mut s.show_particles, "Show particles (H)");
    ui.separator();
    overlay(ui, f);
}

fn overlay(ui: &mut egui::Ui, f: &mut Frame<'_>) {
    let (world, s) = (&*f.world, &mut *f.state);
    ui.strong("Overlay");
    let fields = world.solver.fields();
    let current = s.overlay.and_then(|key| fields.iter().find(|f| f.key == key));
    egui::ComboBox::from_id_salt("overlay")
        .selected_text(current.map_or("None", |f| f.label))
        .width(220.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut s.overlay, None, "None");
            for f in fields {
                ui.selectable_value(&mut s.overlay, Some(f.key), f.label);
            }
        })
        .response
        .on_hover_text("O cycles through the fields");
    if fields.is_empty() {
        ui.weak(format!("{} has no grid fields", world.solver.name()));
    } else if s.overlay.is_none() && world.solver.particles().is_none() {
        ui.weak(format!("Showing {}: {} has no particles", fields[0].label.to_lowercase(), world.solver.name()));
    }
    if let (Some(field), Some((lo, hi))) = (current, f.field_range) {
        ui.add(egui::Slider::new(&mut s.overlay_opacity, 0.1..=1.0).text("Opacity"));
        let legend = match field.kind {
            sim::FieldKind::Categorical => "grey: air   blue: fluid   dark: solid".to_string(),
            sim::FieldKind::Concentration => "transparent: none   white: full".to_string(),
            _ => format!("{lo:.3} .. {hi:.3} {}", field.unit),
        };
        ui.label(legend);
    }
}
