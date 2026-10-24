//! Top bar: run controls, and the scene and solver choice. These are what you touch
//! every few seconds, so they're always visible. (The tools are in the strip on the
//! canvas.)

use super::Frame;
use super::shortcuts::{Action, tip};
use egui::RichText;
use egui_phosphor::regular as icon;

/// An icon-only button.
fn icon_button(ui: &mut egui::Ui, glyph: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(glyph).size(16.0)).min_size(egui::vec2(28.0, 22.0)))
}
use sim::PRESETS;

pub fn show(ui: &mut egui::Ui, f: &mut Frame<'_>) {
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        transport(ui, f);
        ui.separator();
        scene_and_solver(ui, f);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let panel = if f.state.panel_open { "Hide the side panel" } else { "Show the side panel" };
            if icon_button(ui, icon::SIDEBAR_SIMPLE).on_hover_text(tip(panel, Action::TogglePanel)).clicked() {
                f.state.panel_open = !f.state.panel_open;
            }
            if icon_button(ui, icon::KEYBOARD).on_hover_text(tip("Keyboard and mouse shortcuts", Action::ToggleCheatSheet)).clicked() {
                f.state.cheat_sheet = !f.state.cheat_sheet;
            }
        });
    });
    ui.add_space(2.0);
}

fn transport(ui: &mut egui::Ui, f: &mut Frame<'_>) {
    let s = &mut *f.state;
    let (glyph, what) = if s.paused { (icon::PLAY, "Play") } else { (icon::PAUSE, "Pause") };
    if icon_button(ui, glyph).on_hover_text(tip(what, Action::TogglePause)).clicked() {
        s.paused = !s.paused;
    }
    let step = ui.add_enabled_ui(s.paused, |ui| icon_button(ui, icon::SKIP_FORWARD)).inner;
    if step.on_hover_text(tip("Advance one step", Action::Step)).on_disabled_hover_text("Pause first").clicked() {
        s.step_once = true;
    }
    if icon_button(ui, icon::ARROW_COUNTER_CLOCKWISE).on_hover_text(tip("Reset the scene", Action::Reset)).clicked() {
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
        // Only particles can be handed over; a grid solver's successor starts from the scene.
        let has_particles = world.solver.particles().is_some();
        ui.add_enabled(has_particles, egui::Checkbox::new(&mut f.state.keep_particles, "Keep particles when switching"))
            .on_hover_text("Hand the current fluid to the new solver instead of restarting the scene.")
            .on_disabled_hover_text("Only a particle solver can hand its fluid over.");
    });
    if index != world.solver_index {
        world.switch_solver(index, f.state.keep_particles);
    }
}
