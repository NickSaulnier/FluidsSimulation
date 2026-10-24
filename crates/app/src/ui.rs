//! The egui control panel. Solver parameters come from `Solver::visit_params`, so this
//! file never names a specific solver.

use sim::{PRESETS, SOLVERS, Vec2, World};

/// Panel width in points; the scene view is fitted into the rest of the window.
pub const PANEL_WIDTH: f32 = 340.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Push,
    PaintWall,
    EraseWall,
}

pub struct UiState {
    pub paused: bool,
    pub step_once: bool,
    /// Simulated seconds per real second.
    pub speed: f32,
    pub tool: Tool,
    pub brush_radius: f32,
    /// Push brush blend rate, 1/s.
    pub push_strength: f32,
    /// Gravity tilt: 0 = straight down, positive leans toward +x.
    pub gravity_deg: f32,
    pub gravity_mag: f32,
    pub keep_particles: bool,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            paused: false,
            step_once: false,
            speed: 1.0,
            tool: Tool::Push,
            brush_radius: 0.08,
            push_strength: 25.0,
            gravity_deg: 0.0,
            gravity_mag: 9.81,
            keep_particles: true,
        }
    }
}

impl UiState {
    pub fn gravity(&self) -> Vec2 {
        let a = self.gravity_deg.to_radians();
        Vec2::new(a.sin(), -a.cos()) * self.gravity_mag
    }
}

/// Frame-rate numbers averaged over a short window.
#[derive(Clone, Copy, Default)]
pub struct Perf {
    pub fps: f32,
    pub sim_ms: f32,
    pub substeps: f32,
    /// Fraction of real time the sim couldn't keep up with (0 = real time).
    pub slow_motion: f32,
}

/// What the panel needs to draw the brush outline under the cursor.
#[derive(Clone, Copy)]
pub struct Cursor {
    /// Physical pixels.
    pub pos: Vec2,
    pub px_per_metre: f32,
}

pub fn panel(ui: &mut egui::Ui, world: &mut World, state: &mut UiState, perf: &Perf, cursor: Option<Cursor>) {
    egui::Panel::right("controls").resizable(false).exact_size(PANEL_WIDTH).show(ui, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("Fluid toy");
            stats(ui, world, perf);
            ui.separator();
            playback(ui, world, state);
            ui.separator();
            scene(ui, world, state);
            ui.separator();
            tools(ui, state);
            ui.separator();
            solver(ui, world, state);
            ui.separator();
            help(ui);
        });
    });

    // Brush outline, drawn on the background layer so it sits under the panel.
    if let Some(c) = cursor {
        let ppp = ui.ctx().pixels_per_point();
        let centre = egui::pos2(c.pos.x / ppp, c.pos.y / ppp);
        let colour = match state.tool {
            Tool::Push => egui::Color32::from_rgb(120, 200, 255),
            Tool::PaintWall => egui::Color32::from_rgb(230, 230, 230),
            Tool::EraseWall => egui::Color32::from_rgb(255, 120, 120),
        };
        ui.painter()
            .circle_stroke(centre, state.brush_radius * c.px_per_metre / ppp, egui::Stroke::new(1.5, colour));
    }
}

fn stats(ui: &mut egui::Ui, world: &World, perf: &Perf) {
    let stats = world.solver.stats();
    egui::Grid::new("stats").num_columns(2).show(ui, |ui| {
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
}

fn playback(ui: &mut egui::Ui, world: &mut World, state: &mut UiState) {
    ui.horizontal(|ui| {
        if ui.button(if state.paused { "Play" } else { "Pause" }).clicked() {
            state.paused = !state.paused;
        }
        if ui.add_enabled(state.paused, egui::Button::new("Step")).clicked() {
            state.step_once = true;
        }
        if ui.button("Reset").clicked() {
            world.reset();
        }
    });
    ui.add(egui::Slider::new(&mut state.speed, 0.1..=2.0).text("Speed").logarithmic(true));
}

fn scene(ui: &mut egui::Ui, world: &mut World, state: &mut UiState) {
    let mut preset = world.preset_index;
    egui::ComboBox::from_id_salt("preset")
        .selected_text(PRESETS[preset].name)
        .width(180.0)
        .show_ui(ui, |ui| {
            for (k, p) in PRESETS.iter().enumerate() {
                ui.selectable_value(&mut preset, k, p.name);
            }
        });
    if preset != world.preset_index {
        world.load_preset(preset);
    }
    ui.horizontal(|ui| {
        ui.add(egui::Slider::new(&mut state.gravity_deg, -90.0..=90.0).text("Tilt").suffix("°"));
        if ui.small_button("Level").clicked() {
            state.gravity_deg = 0.0;
        }
    });
    ui.add(egui::Slider::new(&mut state.gravity_mag, 0.0..=20.0).text("Gravity").suffix(" m/s²"));
}

fn tools(ui: &mut egui::Ui, state: &mut UiState) {
    ui.horizontal(|ui| {
        ui.radio_value(&mut state.tool, Tool::Push, "Push");
        ui.radio_value(&mut state.tool, Tool::PaintWall, "Paint wall");
        ui.radio_value(&mut state.tool, Tool::EraseWall, "Erase");
    });
    ui.add(egui::Slider::new(&mut state.brush_radius, 0.02..=0.3).text("Brush radius").suffix(" m").logarithmic(true));
    ui.add(egui::Slider::new(&mut state.push_strength, 2.0..=80.0).text("Push strength").logarithmic(true));
}

fn solver(ui: &mut egui::Ui, world: &mut World, state: &mut UiState) {
    let mut index = world.solver_index;
    egui::ComboBox::from_id_salt("solver")
        .selected_text(SOLVERS[index].name)
        .width(180.0)
        .show_ui(ui, |ui| {
            for (k, s) in SOLVERS.iter().enumerate() {
                ui.selectable_value(&mut index, k, s.name);
            }
        });
    if index != world.solver_index {
        world.switch_solver(index, state.keep_particles);
    }
    ui.checkbox(&mut state.keep_particles, "Keep particles when switching");

    let mut params = EguiParams { ui, restart_requested: false };
    world.solver.visit_params(&mut params);
    if params.restart_requested {
        world.reset();
    }
}

fn help(ui: &mut egui::Ui) {
    egui::CollapsingHeader::new("Controls").default_open(false).show(ui, |ui| {
        ui.label("Left drag: selected tool\nRight drag: push\nSpace: pause   S: step   R: reset\nP: next preset   N: next solver\nLeft/Right arrows: tilt gravity");
    });
}

/// Renders solver parameters as egui widgets.
struct EguiParams<'u> {
    ui: &'u mut egui::Ui,
    restart_requested: bool,
}

impl EguiParams<'_> {
    /// Parameters that need a restart apply when the drag ends (or on a click / typed
    /// value), not on every intermediate slider position.
    fn note_change(&mut self, r: &egui::Response, restart: bool) {
        if restart && (r.drag_stopped() || (r.changed() && !r.dragged())) {
            self.restart_requested = true;
        }
    }
}

impl sim::ParamVisitor for EguiParams<'_> {
    fn float(&mut self, s: &sim::FloatSpec, v: &mut f32) {
        let r = self
            .ui
            .add(egui::Slider::new(v, s.range.clone()).text(s.label).logarithmic(s.log))
            .on_hover_text(s.help);
        self.note_change(&r, s.restart);
    }

    fn int(&mut self, s: &sim::IntSpec, v: &mut u32) {
        let r = self.ui.add(egui::Slider::new(v, s.range.clone()).text(s.label)).on_hover_text(s.help);
        self.note_change(&r, s.restart);
    }

    fn toggle(&mut self, s: &sim::ToggleSpec, v: &mut bool) {
        self.ui.checkbox(v, s.label).on_hover_text(s.help);
    }

    fn section(&mut self, title: &'static str) {
        self.ui.add_space(4.0);
        self.ui.strong(title);
    }
}
