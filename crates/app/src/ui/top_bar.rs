//! Top bar: run controls, and the scene and solver choice. These are what you touch
//! every few seconds, so they're always visible. (The tools are in the strip on the
//! canvas.)

use super::shortcuts::{Action, tip};
use super::widgets::icon_button;
use super::{Frame, SolverChoice, anim, presentation, widgets};
use egui::RichText;
use egui_phosphor::regular as icon;
use sim::PRESETS;

pub fn show(ui: &mut egui::Ui, f: &mut Frame<'_>) {
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        transport(ui, f);
        ui.separator();
        look(ui, f);
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
    ui.add_space(4.0);
}

/// The look on screen, large: previous, its name (sliding when it changes) with the
/// list of looks under it, next.
fn look(ui: &mut egui::Ui, f: &mut Frame<'_>) {
    let (d, looks) = (&mut f.state.director, &f.state.looks.looks);
    if icon_button(ui, icon::CARET_LEFT).on_hover_text(tip("Previous look", Action::PreviousLook)).clicked() {
        d.request(d.neighbour(-1, looks.len()));
    }
    let name = d.current.and_then(|i| looks.get(i)).map_or("Custom", |l| l.name.as_str()).to_string();
    let mut pick = None;
    let menu = egui::ComboBox::from_id_salt("top look").selected_text(RichText::new(" ").size(1.0)).width(200.0).height(420.0).show_ui(ui, |ui| {
        for (i, look) in looks.iter().enumerate() {
            if ui.selectable_label(d.current == Some(i), &look.name).clicked() {
                pick = Some(i);
            }
        }
    });
    // The name over the menu's button, sliding when the look changes.
    let button = menu.response.rect;
    let mut name_ui = ui.new_child(egui::UiBuilder::new().max_rect(button.shrink2(egui::vec2(8.0, 0.0))));
    presentation::sliding_name(&mut name_ui, "top bar", &name, button.shrink2(egui::vec2(16.0, 0.0)).size());
    menu.response.on_hover_text("The look on screen: pick another");
    if let Some(i) = pick {
        d.request(i);
    }
    if icon_button(ui, icon::CARET_RIGHT).on_hover_text(tip("Next look", Action::NextLook)).clicked() {
        d.request(d.neighbour(1, looks.len()));
    }
}

fn transport(ui: &mut egui::Ui, f: &mut Frame<'_>) {
    let s = &mut *f.state;
    let what = if s.paused { "Play" } else { "Pause" };
    if morph_button(ui, icon::PAUSE, icon::PLAY, s.paused).on_hover_text(tip(what, Action::TogglePause)).clicked() {
        s.paused = !s.paused;
    }
    let step = ui.add_enabled_ui(s.paused, |ui| icon_button(ui, icon::SKIP_FORWARD)).inner;
    if step.on_hover_text(tip("Advance one step", Action::Step)).on_disabled_hover_text("Pause first").clicked() {
        s.step_once = true;
    }
    if icon_button(ui, icon::ARROW_COUNTER_CLOCKWISE).on_hover_text(tip("Reset the scene", Action::Reset)).clicked() {
        match f.world3d.as_deref_mut() {
            Some(w) => w.reset(),
            None => f.world.reset(),
        }
    }
    ui.add(egui::Slider::new(&mut s.speed, 0.1..=2.0).logarithmic(true).suffix("×").fixed_decimals(1))
        .on_hover_text("Simulation speed");
}

/// The scene (2D or 3D, whichever is running) and the solver, from either family.
pub(super) fn scene_and_solver(ui: &mut egui::Ui, f: &mut Frame<'_>) {
    let world = &mut *f.world;
    ui.label("Scene");
    match f.world3d.as_deref_mut() {
        Some(w) => {
            let mut preset = w.preset_index;
            egui::ComboBox::from_id_salt("preset").selected_text(sim3d::PRESETS[preset].name).width(170.0).show_ui(ui, |ui| {
                for (k, p) in sim3d::PRESETS.iter().enumerate() {
                    ui.selectable_value(&mut preset, k, p.name);
                }
            });
            if preset != w.preset_index {
                w.load_preset(preset);
            }
        }
        None => {
            let mut preset = world.preset_index;
            egui::ComboBox::from_id_salt("preset").selected_text(PRESETS[preset].name).width(170.0).show_ui(ui, |ui| {
                for (k, p) in PRESETS.iter().enumerate() {
                    ui.selectable_value(&mut preset, k, p.name);
                }
            });
            if preset != world.preset_index {
                world.load_preset(preset);
            }
        }
    }

    ui.label("Solver");
    let current = match &f.world3d {
        Some(w) => SolverChoice::ThreeD(w.solver_index),
        None => SolverChoice::TwoD(world.solver_index),
    };
    let name = match current {
        SolverChoice::TwoD(k) => world.solvers[k].name.to_string(),
        SolverChoice::ThreeD(k) => format!("{} {}", icon::CUBE, sim3d::SOLVERS[k].name),
    };
    let mut choice = current;
    let in_3d = f.world3d.is_some();
    egui::ComboBox::from_id_salt("solver").selected_text(name).width(190.0).show_ui(ui, |ui| {
        for (k, s) in world.solvers.iter().enumerate() {
            ui.selectable_value(&mut choice, SolverChoice::TwoD(k), s.name);
        }
        ui.separator();
        for (k, s) in sim3d::SOLVERS.iter().enumerate() {
            ui.selectable_value(&mut choice, SolverChoice::ThreeD(k), format!("{} {}", icon::CUBE, s.name));
        }
        ui.separator();
        // Only particles can be handed over; a grid solver's successor starts from the scene.
        let has_particles = !in_3d && world.solver.particles().is_some();
        ui.add_enabled(has_particles, egui::Checkbox::new(&mut f.state.keep_particles, "Keep particles when switching"))
            .on_hover_text("Hand the current fluid to the new solver instead of restarting the scene.")
            .on_disabled_hover_text("Only a 2D particle solver can hand its fluid over.");
    });
    if choice != current {
        match choice {
            SolverChoice::TwoD(k) if !in_3d => world.switch_solver(k, f.state.keep_particles),
            // Into or out of 3D: the app swaps the worlds.
            _ => f.state.solver_request = Some(choice),
        }
    }
}

/// An icon button showing `a`, or `b` when `show_b`; switching, one shrinks and fades
/// as the other grows into its place.
fn morph_button(ui: &mut egui::Ui, a: &str, b: &str, show_b: bool) -> egui::Response {
    let glow = ui.painter().add(egui::Shape::Noop);
    let response = ui.add(egui::Button::new("").min_size(widgets::ICON_BUTTON));
    widgets::paint_glow(ui, glow, &response);
    let t = anim::toward(ui.ctx(), response.id.with("morph"), show_b, anim::QUICK);
    let colour = ui.style().interact(&response).fg_stroke.color;
    for (glyph, amount) in [(a, 1.0 - t), (b, t)] {
        if amount > 0.01 {
            let size = 18.0 * (0.6 + 0.4 * amount);
            ui.painter().text(response.rect.center(), egui::Align2::CENTER_CENTER, glyph, egui::FontId::proportional(size), colour.gamma_multiply(amount));
        }
    }
    response
}
