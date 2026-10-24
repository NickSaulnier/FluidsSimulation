//! The egui control panel. Solver parameters come from `Solver::visit_params`, so this
//! file never names a specific solver.

use crate::audio::{KickAction, Music};
use sim::{PRESETS, Vec2, World};
use std::time::Duration;

/// Panel width in points; the scene view is fitted into the rest of the window.
pub const PANEL_WIDTH: f32 = 340.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Push,
    PaintWall,
    EraseWall,
    /// Hold to pour fluid from the cursor, in the direction the mouse moves.
    Faucet,
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
    /// Key of the solver field shown as an overlay (by key, so it survives switching
    /// to a solver that lists its fields in a different order, or not at all).
    pub overlay: Option<&'static str>,
    pub overlay_opacity: f32,
    pub show_particles: bool,
    pub faucet_speed: f32,
    /// Draw particles as a continuous liquid surface.
    pub liquid: bool,
    pub blob_size: f32,
    pub surface_threshold: f32,
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
            overlay: None,
            overlay_opacity: 0.85,
            show_particles: true,
            faucet_speed: 2.0,
            liquid: true,
            blob_size: 3.0,
            surface_threshold: 0.5,
        }
    }
}

impl UiState {
    pub fn liquid_style(&self) -> Option<crate::render::LiquidStyle> {
        self.liquid
            .then_some(crate::render::LiquidStyle { splat_scale: self.blob_size, threshold: self.surface_threshold })
    }

    pub fn gravity(&self) -> Vec2 {
        let a = self.gravity_deg.to_radians();
        Vec2::new(a.sin(), -a.cos()) * self.gravity_mag
    }

    /// The inverse of `gravity`: point the sliders at a scene's own gravity.
    pub fn set_gravity(&mut self, g: Vec2) {
        self.gravity_mag = g.length();
        self.gravity_deg = if g == Vec2::ZERO { 0.0 } else { g.x.atan2(-g.y).to_degrees().clamp(-90.0, 90.0) };
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

pub fn panel(
    ui: &mut egui::Ui,
    world: &mut World,
    state: &mut UiState,
    music: &mut Music,
    perf: &Perf,
    cursor: Option<Cursor>,
    field_range: Option<(f32, f32)>,
) {
    let window = ui.max_rect();
    let panel = egui::Panel::right("controls").resizable(false).exact_size(PANEL_WIDTH).show(ui, |ui| {
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
            music_section(ui, music);
            ui.separator();
            look(ui, state);
            ui.separator();
            overlay(ui, world, state, field_range);
            ui.separator();
            solver(ui, world, state);
            ui.separator();
            help(ui);
        });
    });

    // Brush outline, clipped to the canvas left of the panel so it never draws over
    // the controls.
    if let Some(c) = cursor {
        let canvas = egui::Rect::from_min_max(window.min, egui::pos2(panel.response.rect.min.x, window.max.y));        let ppp = ui.ctx().pixels_per_point();
        let centre = egui::pos2(c.pos.x / ppp, c.pos.y / ppp);
        let colour = match state.tool {
            Tool::Push => egui::Color32::from_rgb(120, 200, 255),
            Tool::PaintWall => egui::Color32::from_rgb(230, 230, 230),
            Tool::EraseWall => egui::Color32::from_rgb(255, 120, 120),
            Tool::Faucet => egui::Color32::from_rgb(120, 255, 170),
        };
        ui.painter()
            .with_clip_rect(canvas)
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
        ui.radio_value(&mut state.tool, Tool::Faucet, "Faucet");
    });
    ui.add(egui::Slider::new(&mut state.brush_radius, 0.02..=0.3).text("Brush radius").suffix(" m").logarithmic(true));
    match state.tool {
        Tool::Push => {
            ui.add(egui::Slider::new(&mut state.push_strength, 2.0..=80.0).text("Push strength").logarithmic(true));
        }
        Tool::Faucet => {
            ui.add(egui::Slider::new(&mut state.faucet_speed, 0.2..=6.0).text("Faucet speed").suffix(" m/s"));
        }
        _ => {}
    }
}

fn music_section(ui: &mut egui::Ui, music: &mut Music) {
    ui.strong("Music");
    if let Some(name) = music.loading_name() {
        ui.label(format!("Loading {name}..."));
    }
    if let Some(e) = &music.error {
        ui.colored_label(egui::Color32::from_rgb(255, 120, 120), e);
    }
    let Some(track) = &music.track else {
        ui.weak("Drop an audio file on the window to make the water dance to its kick drums.");
        return;
    };
    let (name, duration, kicks) = (track.name.clone(), track.duration, track.kicks.len());
    let pos = music.position().min(duration);
    ui.label(format!("{name}  {}:{:02} / {}:{:02}", pos as u32 / 60, pos as u32 % 60, duration as u32 / 60, duration as u32 % 60));
    ui.horizontal(|ui| {
        if ui.button(if music.is_playing() { "Pause (M)" } else { "Play (M)" }).clicked() {
            music.toggle_play();
        }
        if ui.button("Restart").clicked() {
            music.restart();
        }
        if ui.checkbox(&mut music.muted, "Mute").changed() {
            music.apply_volume();
        }
        let flash = music.last_kick.is_some_and(|t| t.elapsed() < Duration::from_millis(150));
        let colour = if flash { egui::Color32::from_rgb(255, 210, 90) } else { egui::Color32::from_gray(70) };
        // Plain text: egui's default font has no "●" glyph.
        ui.colored_label(colour, "KICK");
    });
    ui.horizontal(|ui| {
        ui.radio_value(&mut music.action, KickAction::Splash, "Splash")
            .on_hover_text("Kicks shove the water upward.");
        ui.radio_value(&mut music.action, KickAction::Jet, "Jet")
            .on_hover_text("Kicks fire jets from the floor (adds water; the Fountain preset drains it).");
    });
    ui.add(egui::Slider::new(&mut music.sensitivity, 0.5..=4.0).text("Selectivity"))
        .on_hover_text(format!("Higher = only the clearest kicks. {kicks} kicks detected."));
    ui.add(egui::Slider::new(&mut music.gain, 0.2..=3.0).text("Strength").logarithmic(true));
}

fn look(ui: &mut egui::Ui, state: &mut UiState) {
    ui.checkbox(&mut state.liquid, "Liquid surface (L)")
        .on_hover_text("Blend particles into a continuous surface instead of drawing discs.");
    if state.liquid {
        ui.add(egui::Slider::new(&mut state.blob_size, 1.5..=6.0).text("Blob size"))
            .on_hover_text("Splat radius in particle radii. Bigger = smoother, blobbier.");
        ui.add(egui::Slider::new(&mut state.surface_threshold, 0.15..=0.9).text("Surface level"))
            .on_hover_text("Where the surface sits, as a fraction of interior density. Lower = puffier.");
    }
}

/// Cycle None -> each field -> None.
pub fn next_overlay(current: Option<&'static str>, fields: &[sim::FieldInfo]) -> Option<&'static str> {
    let next = match current.and_then(|key| fields.iter().position(|f| f.key == key)) {
        None => 0,
        Some(k) => k + 1,
    };
    fields.get(next).map(|f| f.key)
}

fn overlay(ui: &mut egui::Ui, world: &World, state: &mut UiState, range: Option<(f32, f32)>) {
    let fields = world.solver.fields();
    let current = state.overlay.and_then(|key| fields.iter().find(|f| f.key == key));
    egui::ComboBox::from_id_salt("overlay")
        .selected_text(current.map_or("No overlay", |f| f.label))
        .width(220.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut state.overlay, None, "No overlay");
            for f in fields {
                ui.selectable_value(&mut state.overlay, Some(f.key), f.label);
            }
        });
    if fields.is_empty() {
        ui.weak(format!("{} has no grid fields", world.solver.name()));
    } else if state.overlay.is_none() && world.solver.particles().is_none() {
        ui.weak(format!("Showing {}: {} has no particles", fields[0].label.to_lowercase(), world.solver.name()));
    }
    if let (Some(field), Some((lo, hi))) = (current, range) {
        ui.add(egui::Slider::new(&mut state.overlay_opacity, 0.1..=1.0).text("Opacity"));
        let legend = match field.kind {
            sim::FieldKind::Categorical => "grey: air   blue: fluid   dark: solid".to_string(),
            sim::FieldKind::Concentration => "transparent: none   white: full".to_string(),
            _ => format!("{lo:.3} .. {hi:.3} {}", field.unit),
        };
        ui.label(legend);
    }
    ui.checkbox(&mut state.show_particles, "Show particles");
}

fn solver(ui: &mut egui::Ui, world: &mut World, state: &mut UiState) {
    let mut index = world.solver_index;
    egui::ComboBox::from_id_salt("solver")
        .selected_text(world.solvers[index].name)
        .width(180.0)
        .show_ui(ui, |ui| {
            for (k, s) in world.solvers.iter().enumerate() {
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
        ui.label("Left drag: selected tool (Faucet pours along the drag)\nRight drag: push\nSpace: pause   S: step   R: reset\nP: next preset   N: next solver
O: next overlay   H: hide/show particles\nL: liquid surface on/off   M: play/pause music\nDrop an audio file to load music\nLeft/Right arrows: tilt gravity");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_cycle_wraps_through_none() {
        let fields = [
            sim::FieldInfo { key: "a", label: "A", unit: "", kind: sim::FieldKind::Signed },
            sim::FieldInfo { key: "b", label: "B", unit: "", kind: sim::FieldKind::Signed },
        ];
        assert_eq!(next_overlay(None, &fields), Some("a"));
        assert_eq!(next_overlay(Some("a"), &fields), Some("b"));
        assert_eq!(next_overlay(Some("b"), &fields), None);
        // A key the current solver doesn't have (after switching) restarts the cycle.
        assert_eq!(next_overlay(Some("gone"), &fields), Some("a"));
        assert_eq!(next_overlay(None, &[]), None);
    }
}
