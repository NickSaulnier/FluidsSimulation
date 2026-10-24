//! Top bar: run controls, scene and solver choice, and the canvas tools. These are what
//! you touch every few seconds, so they're always visible.

use super::{Frame, Tool};
use sim::PRESETS;

pub fn show(ui: &mut egui::Ui, f: &mut Frame<'_>) {
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        transport(ui, f);
        ui.separator();
        scene_and_solver(ui, f);
        ui.separator();
        tools(ui, f);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let label = if f.state.panel_open { "Hide panel" } else { "Show panel" };
            if ui.button(label).on_hover_text("Show or hide the side panel (`)").clicked() {
                f.state.panel_open = !f.state.panel_open;
            }
        });
    });
    ui.add_space(2.0);
}

fn transport(ui: &mut egui::Ui, f: &mut Frame<'_>) {
    let s = &mut *f.state;
    if ui.button(if s.paused { "Play" } else { "Pause" }).on_hover_text("Space").clicked() {
        s.paused = !s.paused;
    }
    if ui.add_enabled(s.paused, egui::Button::new("Step")).on_hover_text("S (while paused)").clicked() {
        s.step_once = true;
    }
    if ui.button("Reset").on_hover_text("R").clicked() {
        f.world.reset();
    }
    ui.add(egui::Slider::new(&mut s.speed, 0.1..=2.0).logarithmic(true).suffix("×").fixed_decimals(1))
        .on_hover_text("Simulation speed");
}

fn scene_and_solver(ui: &mut egui::Ui, f: &mut Frame<'_>) {
    let world = &mut *f.world;
    ui.label("Scene");
    let mut preset = world.preset_index;
    egui::ComboBox::from_id_salt("preset").selected_text(PRESETS[preset].name).width(170.0).show_ui(ui, |ui| {
        for (k, p) in PRESETS.iter().enumerate() {
            ui.selectable_value(&mut preset, k, p.name);
        }
    });
    if preset != world.preset_index {
        world.load_preset(preset);
    }

    ui.label("Solver");
    let mut index = world.solver_index;
    egui::ComboBox::from_id_salt("solver").selected_text(world.solvers[index].name).width(190.0).show_ui(ui, |ui| {
        for (k, s) in world.solvers.iter().enumerate() {
            ui.selectable_value(&mut index, k, s.name);
        }
        ui.separator();
        ui.checkbox(&mut f.state.keep_particles, "Keep particles when switching")
            .on_hover_text("Hand the current fluid to the new solver instead of restarting the scene.");
    });
    if index != world.solver_index {
        world.switch_solver(index, f.state.keep_particles);
    }
}

fn tools(ui: &mut egui::Ui, f: &mut Frame<'_>) {
    let s = &mut *f.state;
    for (tool, label, help) in [
        (Tool::Push, "Push", "Drag to push the fluid (right drag always pushes)"),
        (Tool::PaintWall, "Wall", "Drag to paint walls"),
        (Tool::EraseWall, "Erase", "Drag to erase walls"),
        (Tool::Faucet, "Faucet", "Hold to pour, along the drag direction"),
    ] {
        ui.selectable_value(&mut s.tool, tool, label).on_hover_text(help);
    }
    ui.add(egui::Slider::new(&mut s.brush_radius, 0.02..=0.3).logarithmic(true).suffix(" m").fixed_decimals(2))
        .on_hover_text("Brush radius");
    match s.tool {
        Tool::Push => {
            ui.add(egui::Slider::new(&mut s.push_strength, 2.0..=80.0).logarithmic(true).fixed_decimals(0))
                .on_hover_text("Push strength");
        }
        Tool::Faucet => {
            ui.add(egui::Slider::new(&mut s.faucet_speed, 0.2..=6.0).suffix(" m/s").fixed_decimals(1))
                .on_hover_text("Faucet speed");
        }
        Tool::PaintWall | Tool::EraseWall => {}
    }
}
