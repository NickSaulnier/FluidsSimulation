//! The egui interface: a top bar (run, scene, solver), a status bar, a tabbed side
//! panel, and on the canvas a tool strip and the overlay legend. In presentation mode
//! (the full-screen visualizer) all of that hides, leaving an overlay that appears
//! when the mouse moves. Solver parameters come from `Solver::visit_params`, so nothing here names
//! a specific solver. Whatever is left after the bars and panel is the canvas; its rect
//! is returned so the scene view is fitted into exactly that area.

pub mod browser;
mod cheat_sheet;
mod editor;
mod compass;
mod health;
mod legend;
mod panel;
mod presentation;
pub mod params;
pub mod settings;
pub mod shortcuts;
mod status_bar;
pub mod theme;
pub mod thumbnails;
pub mod title_card;
mod tool_strip;
mod top_bar;
mod widgets;

use crate::audio::Music;
use std::time::{Duration, Instant};
use sim::{Vec2, World};

/// Side panel width in points.
pub const PANEL_WIDTH: f32 = 320.0;
/// Brush radius limits (m), and the factor `[` and `]` change it by.
pub const BRUSH_RADIUS: std::ops::RangeInclusive<f32> = 0.02..=0.3;
pub const BRUSH_STEP: f32 = 1.15;
/// How long the presentation overlay stays after the mouse stops, and notices last.
const REVEAL: Duration = Duration::from_secs(3);
const NOTICE: Duration = Duration::from_secs(4);

/// Where and how the visualizer shows (F11), and how fast frames come.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Screen {
    /// The monitor for full screen, by name (none: the window's own).
    pub monitor: Option<String>,
    /// The visualizer in a window instead of full screen, and that window kept on top.
    pub windowed: bool,
    pub on_top: bool,
    /// Frames per second at most (0: as fast as the display allows).
    pub fps_cap: u32,
}

/// The frame-rate caps offered.
pub const FPS_CAPS: [u32; 5] = [0, 30, 60, 120, 144];

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Tool {
    Push,
    PaintWall,
    EraseWall,
    /// Hold to pour fluid from the cursor, in the direction the mouse moves.
    Faucet,
}

/// A solver picked in the UI that the app acts on: moving between 2D and 3D replaces
/// the world, which the UI can't do by itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SolverChoice {
    TwoD(usize),
    ThreeD(usize),
}

/// Side panel tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Tab {
    Simulate,
    View,
    Music,
    Info,
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
    /// Bloom added to the picture, 0 (none) to 1.
    pub glow: f32,
    /// The scene's resolution relative to the window; 0 picks automatically.
    pub render_scale: f32,
    pub screen: Screen,
    /// The monitors' names, as the app last found them.
    pub monitors: Vec<String>,
    pub show_particles: bool,
    pub faucet_speed: f32,
    /// Draw particles as a continuous liquid surface.
    pub liquid: bool,
    /// In the 3D visualizer, how fast the camera circles the scene, rad/s.
    pub orbit: f32,
    pub blob_size: f32,
    pub surface_threshold: f32,
    pub tab: Tab,
    pub panel_open: bool,
    pub cheat_sheet: bool,
    /// The song playing, as shown (see `audio::media`), when its title card was last
    /// shown, and whether one shows for each new song.
    pub song: Option<title_card::Song>,
    pub title_card: Option<Instant>,
    pub title_cards: bool,
    /// While a video is recording: when it started (for the indicator).
    pub recording: Option<Instant>,
    /// Colour the looks by the song's cover art (listening live).
    pub album_colours: bool,
    /// The look browser and the equation editor windows.
    pub browser_open: bool,
    pub editor_open: bool,
    /// What the browser shows, and the looks' thumbnails.
    pub browser: browser::Filter,
    pub thumbnails: thumbnails::Thumbnails,
    /// The equation editor, and edited code for the app to run (and whether `init` runs
    /// again).
    pub editor: editor::Editor,
    pub code_request: Option<(crate::looks::Code, bool)>,
    /// The fluid's colours.
    pub palette: crate::render::Palette,
    /// How each frame carries the last one (trails, tunnels, echo).
    pub feedback: crate::render::feedback::FeedbackSettings,
    /// The sound drawn on screen (scopes, spectrum, shapes).
    pub waves: crate::render::waves::WaveSettings,
    /// The composite's look: symmetry, colour grading, lens.
    pub composite: crate::render::composite::CompositeSettings,
    /// How the music drives the fluid.
    pub reactions: Vec<crate::audio::reactions::Reaction>,
    /// How much later than it's analysed the audio is heard, milliseconds (see
    /// `Music::latency_ms`).
    pub audio_latency_ms: f32,
    /// The full-screen visualizer: no controls, the scene filling the screen.
    pub presenting: bool,
    /// In presentation mode, the overlay (and cursor) show until then.
    pub reveal_until: Option<Instant>,
    /// A brief message (e.g. the resolution was lowered), until then.
    pub notice: Option<(String, Instant)>,
    /// Picks visualizer looks and changes them with the music.
    pub director: crate::director::Director,
    /// Every look (see `looks`).
    pub looks: crate::looks::Library,
    /// The name typed for saving the current settings as a look, and a save asked for
    /// (the app does it: it holds the solver).
    pub save_name: String,
    pub save_look_request: Option<String>,
    /// A solver change for the app to make after this frame's layout.
    pub solver_request: Option<SolverChoice>,
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
            glow: 0.35,
            render_scale: 0.0,
            screen: Screen::default(),
            monitors: Vec::new(),
            show_particles: true,
            faucet_speed: 2.0,
            liquid: true,
            orbit: 0.12,
            blob_size: 3.0,
            surface_threshold: 0.5,
            tab: Tab::Simulate,
            panel_open: true,
            cheat_sheet: false,
            song: None,
            title_card: None,
            title_cards: true,
            recording: None,
            album_colours: true,
            browser_open: false,
            editor_open: false,
            browser: browser::Filter::default(),
            thumbnails: thumbnails::Thumbnails::default(),
            editor: editor::Editor::default(),
            code_request: None,
            palette: crate::render::Palette::default(),
            feedback: crate::render::feedback::FeedbackSettings::OFF,
            waves: crate::render::waves::WaveSettings::OFF,
            composite: crate::render::composite::CompositeSettings::OFF,
            reactions: crate::audio::reactions::default_reactions(),
            audio_latency_ms: 0.0,
            presenting: false,
            reveal_until: None,
            notice: None,
            director: crate::director::Director::default(),
            looks: crate::looks::Library::default(),
            save_name: String::new(),
            save_look_request: None,
            solver_request: None,
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

    /// Presentation mode: show the overlay and cursor for a few seconds (the mouse moved).
    pub fn reveal(&mut self) {
        self.reveal_until = Some(Instant::now() + REVEAL);
    }

    pub fn revealed(&self) -> bool {
        self.reveal_until.is_some_and(|t| Instant::now() < t)
    }

    /// How much the director favours each look (see `Library::weights`), with the
    /// favourites and tags it's asked to keep to.
    pub fn look_weights(&self) -> Vec<f32> {
        let tags: &[String] = if self.director.tags_only { &self.browser.tags } else { &[] };
        self.looks.weights(self.director.favourites_only, tags)
    }

    pub fn show_notice(&mut self, text: String) {
        self.notice = Some((text, Instant::now() + NOTICE));
    }

    pub fn scale_brush(&mut self, factor: f32) {
        self.brush_radius = (self.brush_radius * factor).clamp(*BRUSH_RADIUS.start(), *BRUSH_RADIUS.end());
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

/// What the UI needs to draw the brush outline under the cursor.
#[derive(Clone, Copy)]
pub struct Cursor {
    /// Physical pixels.
    pub pos: Vec2,
    pub px_per_metre: f32,
}

/// Everything the interface reads or changes in one frame.
pub struct Frame<'a> {
    pub world: &'a mut World,
    /// The 3D world, while a 3D solver is running (it replaces `world` on screen).
    pub world3d: Option<&'a mut sim3d::World3d>,
    pub state: &'a mut UiState,
    pub music: &'a mut Music,
    pub perf: &'a Perf,
    /// Value range of the overlay field drawn last frame, for its legend.
    pub field_range: Option<(f32, f32)>,
    /// GPU time per stage of a recent frame (empty without timestamp queries).
    pub render_times: &'a [(&'static str, std::time::Duration)],
}

/// Lay out the whole interface and return the rect (points) to fit the scene into: the
/// canvas the bars and panel leave over, minus the tool strip's column, so the strip
/// never hides any of the fluid.
pub fn layout(ui: &mut egui::Ui, f: &mut Frame<'_>, cursor: Option<Cursor>) -> egui::Rect {
    if let Some((text, until)) = &f.state.notice {
        if Instant::now() < *until {
            let text = text.clone();
            presentation::notice(ui.ctx(), ui.max_rect(), &text);
        } else {
            f.state.notice = None;
        }
    }
    if f.state.presenting {
        // The whole window is canvas; the overlay shows while the mouse is active (or on
        // it, so it doesn't vanish under the pointer).
        let canvas = ui.max_rect();
        if f.state.revealed() || ui.ctx().is_pointer_over_egui() {
            presentation::overlay(ui.ctx(), canvas, f);
        }
        windows(ui.ctx(), canvas, f);
        return canvas;
    }
    egui::Panel::top("top_bar").show(ui, |ui| top_bar::show(ui, f));
    egui::Panel::bottom("status_bar").show(ui, |ui| status_bar::show(ui, f));
    if f.state.panel_open {
        egui::Panel::right("side_panel").resizable(false).exact_size(PANEL_WIDTH).show(ui, |ui| panel::show(ui, f));
    }
    let canvas = ui.available_rect_before_wrap();
    if f.world3d.is_some() {
        // The 3D tools, but no overlay legend or brush outline.
        tool_strip::show(ui.ctx(), canvas, f.state, true);
        windows(ui.ctx(), canvas, f);
        return canvas;
    }
    tool_strip::show(ui.ctx(), canvas, f.state, false);
    if let Some((field, range)) = overlay_legend(f) {
        legend::show(ui.ctx(), canvas, field, range, &f.state.palette);
    }
    windows(ui.ctx(), canvas, f);

    // Brush outline, clipped to the canvas so it never draws over the controls, and
    // hidden over the on-canvas widgets (strokes don't start there either).
    if let Some(c) = cursor.filter(|_| !ui.ctx().is_pointer_over_egui()) {
        let ppp = ui.ctx().pixels_per_point();
        let centre = egui::pos2(c.pos.x / ppp, c.pos.y / ppp);
        let colour = match f.state.tool {
            Tool::Push => egui::Color32::from_rgb(120, 200, 255),
            Tool::PaintWall => egui::Color32::from_rgb(230, 230, 230),
            Tool::EraseWall => egui::Color32::from_rgb(255, 120, 120),
            Tool::Faucet => egui::Color32::from_rgb(120, 255, 170),
        };
        ui.painter().with_clip_rect(canvas).circle_stroke(
            centre,
            f.state.brush_radius * c.px_per_metre / ppp,
            egui::Stroke::new(1.5, colour),
        );
    }
    canvas.with_min_x((canvas.min.x + tool_strip::RESERVED_WIDTH).min(canvas.max.x))
}

/// The windows over the canvas: the title card, the browser, the editor and the cheat
/// sheet.
fn windows(ctx: &egui::Context, canvas: egui::Rect, f: &mut Frame<'_>) {
    if let Some(since) = f.state.recording {
        let s = since.elapsed().as_secs();
        egui::Area::new(egui::Id::new("recording"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::RIGHT_TOP)
            .fixed_pos(canvas.right_top() + egui::vec2(-16.0, 16.0))
            .interactable(false)
            .show(ctx, |ui| {
                let text = egui::RichText::new(format!("{} REC {}:{:02}", egui_phosphor::regular::RECORD, s / 60, s % 60)).strong().color(egui::Color32::from_rgb(240, 70, 70));
                ui.label(text).on_hover_text(shortcuts::tip("Stop recording", shortcuts::Action::ToggleRecording));
            });
    }
    if let (Some(since), Some(song)) = (f.state.title_card, &f.state.song) {
        let alpha = title_card::opacity(since, Instant::now());
        if alpha > 0.0 {
            title_card::show(ctx, canvas, song, alpha);
        } else if Instant::now() > since {
            f.state.title_card = None;
        }
    }
    if f.state.browser_open {
        browser::show(ctx, f);
    }
    if f.state.editor_open {
        editor::show(ctx, f);
    }
    if f.state.cheat_sheet {
        cheat_sheet::show(ctx, canvas, &mut f.state.cheat_sheet);
    }
}

/// The overlay field to explain in the legend: the one the user picked, if this solver
/// has it and it was drawn last frame (which gives its range).
fn overlay_legend(f: &Frame<'_>) -> Option<(&'static sim::FieldInfo, (f32, f32))> {
    let key = f.state.overlay?;
    let field = f.world.solver.fields().iter().find(|field| field.key == key)?;
    Some((field, f.field_range?))
}

/// Cycle None -> each field -> None.
pub fn next_overlay(current: Option<&'static str>, fields: &[sim::FieldInfo]) -> Option<&'static str> {
    let next = match current.and_then(|key| fields.iter().position(|f| f.key == key)) {
        None => 0,
        Some(k) => k + 1,
    };
    fields.get(next).map(|f| f.key)
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

    #[test]
    fn brush_keys_stay_within_the_slider_range() {
        let mut s = UiState::default();
        for _ in 0..50 {
            s.scale_brush(BRUSH_STEP);
        }
        assert_eq!(s.brush_radius, *BRUSH_RADIUS.end());
        for _ in 0..50 {
            s.scale_brush(1.0 / BRUSH_STEP);
        }
        assert_eq!(s.brush_radius, *BRUSH_RADIUS.start());
    }

    #[test]
    fn gravity_round_trips_through_the_sliders() {
        let mut s = UiState::default();
        for g in [Vec2::new(0.0, -9.81), Vec2::new(3.0, -4.0), Vec2::ZERO] {
            s.set_gravity(g);
            assert!((s.gravity() - g).length() < 1e-4, "{g} -> {}", s.gravity());
        }
    }
}
