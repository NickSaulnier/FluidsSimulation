//! UI settings remembered between runs: the tool and brush, the look (including the
//! fluid's colours), and the panel.
//! Saved as JSON on exit, loaded at start. The scene, solver and gravity are not part
//! of it: a preset brings its own.
//!
//! Missing or unknown fields are fine (`#[serde(default)]`), so adding a setting later
//! doesn't invalidate old files, and out-of-range values are clamped on load.

use super::{BRUSH_RADIUS, Tab, Tool, UiState};
use crate::render::Palette;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const FILE: &str = "settings.json";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub tool: Tool,
    pub brush_radius: f32,
    pub push_strength: f32,
    pub faucet_speed: f32,
    pub speed: f32,
    pub keep_particles: bool,
    pub liquid: bool,
    pub blob_size: f32,
    pub surface_threshold: f32,
    pub show_particles: bool,
    pub overlay_opacity: f32,
    pub panel_open: bool,
    pub tab: Tab,
    pub ui_size: super::UiSize,
    pub reduce_motion: bool,
    pub folds: super::widgets::Folds,
    pub palette: Palette,
    pub reactions: Vec<crate::audio::reactions::Reaction>,
    pub audio_latency_ms: f32,
    pub auto_looks: bool,
    pub look_interval_s: f32,
    pub favourites_only: bool,
    pub title_cards: bool,
    pub album_colours: bool,
    /// The browser's picked tags, and whether the director keeps to them.
    pub look_tags: Vec<String>,
    pub tags_only: bool,
    pub random_looks: bool,
    pub look_marks: std::collections::BTreeMap<String, crate::looks::Mark>,
    pub glow: f32,
    pub render_scale: f32,
    pub screen: super::Screen,
    pub feedback: crate::render::feedback::FeedbackSettings,
    pub waves: crate::render::waves::WaveSettings,
    pub composite: crate::render::composite::CompositeSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self::from_state(&UiState::default())
    }
}

impl Settings {
    pub fn from_state(s: &UiState) -> Self {
        Self {
            tool: s.tool,
            brush_radius: s.brush_radius,
            push_strength: s.push_strength,
            faucet_speed: s.faucet_speed,
            speed: s.speed,
            keep_particles: s.keep_particles,
            liquid: s.liquid,
            blob_size: s.blob_size,
            surface_threshold: s.surface_threshold,
            show_particles: s.show_particles,
            overlay_opacity: s.overlay_opacity,
            panel_open: s.panel_open,
            tab: s.tab,
            ui_size: s.ui_size,
            reduce_motion: s.reduce_motion,
            folds: s.folds.clone(),
            palette: s.palette,
            reactions: s.reactions.clone(),
            audio_latency_ms: s.audio_latency_ms,
            auto_looks: s.director.auto,
            look_interval_s: s.director.interval_s,
            favourites_only: s.director.favourites_only,
            title_cards: s.title_cards,
            album_colours: s.album_colours,
            look_tags: s.browser.tags.clone(),
            tags_only: s.director.tags_only,
            random_looks: s.director.random_looks,
            look_marks: s.looks.marks.clone(),
            glow: s.glow,
            render_scale: s.render_scale,
            screen: s.screen.clone(),
            feedback: s.feedback,
            waves: s.waves,
            composite: s.composite,
        }
    }

    /// Copy into `s`, clamping each value to the range its control allows.
    pub fn apply(&self, s: &mut UiState) {
        let clamp = |v: f32, lo: f32, hi: f32| if v.is_finite() { v.clamp(lo, hi) } else { lo };
        s.tool = self.tool;
        s.brush_radius = clamp(self.brush_radius, *BRUSH_RADIUS.start(), *BRUSH_RADIUS.end());
        s.push_strength = clamp(self.push_strength, 2.0, 80.0);
        s.faucet_speed = clamp(self.faucet_speed, 0.2, 6.0);
        s.speed = clamp(self.speed, 0.1, 2.0);
        s.keep_particles = self.keep_particles;
        s.liquid = self.liquid;
        s.blob_size = clamp(self.blob_size, 1.5, 6.0);
        s.surface_threshold = clamp(self.surface_threshold, 0.15, 0.9);
        s.show_particles = self.show_particles;
        s.overlay_opacity = clamp(self.overlay_opacity, 0.1, 1.0);
        s.panel_open = self.panel_open;
        s.tab = self.tab;
        s.ui_size = self.ui_size;
        s.reduce_motion = self.reduce_motion;
        s.folds = self.folds.clone();
        s.palette = self.palette;
        s.reactions = self.reactions.clone();
        s.audio_latency_ms = clamp(self.audio_latency_ms, *crate::audio::LATENCY_MS.start(), *crate::audio::LATENCY_MS.end());
        s.director.auto = self.auto_looks;
        s.director.interval_s = clamp(self.look_interval_s, 30.0, 300.0);
        s.director.favourites_only = self.favourites_only;
        s.title_cards = self.title_cards;
        s.album_colours = self.album_colours;
        s.browser.tags = self.look_tags.clone();
        s.director.tags_only = self.tags_only;
        s.director.random_looks = self.random_looks;
        s.looks.marks = self.look_marks.clone();
        s.glow = clamp(self.glow, 0.0, 1.0);
        s.render_scale = if self.render_scale > 0.0 { clamp(self.render_scale, 0.5, 1.0) } else { 0.0 };
        s.screen = super::Screen { fps_cap: if super::FPS_CAPS.contains(&self.screen.fps_cap) { self.screen.fps_cap } else { 0 }, ..self.screen.clone() };
        let f = &self.feedback;
        s.feedback = crate::render::feedback::FeedbackSettings {
            trail: clamp(f.trail, 0.0, 0.98),
            zoom: clamp(f.zoom, 0.5, 2.0),
            rotation: clamp(f.rotation, -1.5, 1.5),
            drift: f.drift.map(|d| clamp(d, -0.5, 0.5)),
            warp: clamp(f.warp, 0.0, 0.05),
            warp_speed: clamp(f.warp_speed, 0.0, 4.0),
            flow: clamp(f.flow, 0.0, 3.0),
            bass_zoom: clamp(f.bass_zoom, 0.0, 1.0),
            echo: clamp(f.echo, 0.0, 1.0),
            echo_zoom: clamp(f.echo_zoom, 0.5, 3.0),
            ..*f
        };
        let w = &self.waves;
        s.waves = crate::render::waves::WaveSettings {
            scale: clamp(w.scale, 0.0, 2.0),
            size: clamp(w.size, 0.05, 1.0),
            y: clamp(w.y, -1.0, 1.0),
            thickness: clamp(w.thickness, 0.5, 12.0),
            brightness: clamp(w.brightness, 0.0, 4.0),
            push: clamp(w.push, 0.0, 1.0),
            shapes: w.shapes.map(|sh| crate::render::waves::ShapeSettings {
                sides: sh.sides.clamp(3, 12),
                radius: clamp(sh.radius, 0.02, 1.0),
                centre: sh.centre.map(|c| clamp(c, -1.5, 1.5)),
                spin: clamp(sh.spin, -3.0, 3.0),
                thickness: clamp(sh.thickness, 0.0, 12.0),
                bass_pulse: clamp(sh.bass_pulse, 0.0, 1.0),
                fill: clamp(sh.fill, 0.0, 1.0),
                ..sh
            }),
            art: crate::render::art::ArtSettings {
                size: clamp(w.art.size, 0.05, 1.0),
                opacity: clamp(w.art.opacity, 0.0, 1.0),
                centre: w.art.centre.map(|c| clamp(c, -1.5, 1.5)),
                spin: clamp(w.art.spin, -3.0, 3.0),
                bass_pulse: clamp(w.art.bass_pulse, 0.0, 1.0),
                brightness: clamp(w.art.brightness, 0.0, 4.0),
                ..w.art
            },
            fractal: crate::render::fractal::FractalSettings {
                centre: w.fractal.centre.map(|c| clamp(c, -2.5, 2.5)),
                zoom: clamp(w.fractal.zoom, 0.2, crate::render::fractal::MAX_ZOOM),
                julia: w.fractal.julia.map(|c| clamp(c, -2.0, 2.0)),
                rotation: clamp(w.fractal.rotation, -3.15, 3.15),
                spin: clamp(w.fractal.spin, -1.0, 1.0),
                iterations: w.fractal.iterations.clamp(16, crate::render::fractal::MAX_ITERATIONS),
                bands: clamp(w.fractal.bands, 0.0, 8.0),
                colour_speed: clamp(w.fractal.colour_speed, -1.0, 1.0),
                opacity: clamp(w.fractal.opacity, 0.0, 1.0),
                brightness: clamp(w.fractal.brightness, 0.0, 4.0),
                wander: clamp(w.fractal.wander, -4.0, 4.0),
                bass_zoom: clamp(w.fractal.bass_zoom, 0.0, 1.0),
                dive: clamp(w.fractal.dive, -3.0, 3.0),
                ..w.fractal
            },
            ..*w
        };
        let c = &self.composite;
        s.composite = crate::render::composite::CompositeSettings {
            segments: c.segments.clamp(2, 16),
            symmetry_spin: clamp(c.symmetry_spin, -2.0, 2.0),
            hue: clamp(c.hue, -180.0, 180.0),
            hue_speed: clamp(c.hue_speed, -90.0, 90.0),
            saturation: clamp(c.saturation, 0.0, 2.0),
            contrast: clamp(c.contrast, 0.5, 2.0),
            exposure: clamp(c.exposure, 0.25, 2.0),
            palette_map: clamp(c.palette_map, 0.0, 1.0),
            invert: clamp(c.invert, 0.0, 1.0),
            aberration: clamp(c.aberration, 0.0, 0.05),
            vignette: clamp(c.vignette, 0.0, 1.0),
            grain: clamp(c.grain, 0.0, 0.3),
            ..*c
        };
        for r in &mut s.reactions {
            r.gain = clamp(r.gain, 0.0, 3.0);
        }
    }
}

/// Where the settings live: `%APPDATA%\fluid-toy` on Windows, `$XDG_CONFIG_HOME/fluid-toy`
/// or `~/.config/fluid-toy` elsewhere.
pub fn path() -> Option<PathBuf> {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let base = if cfg!(windows) {
        env("APPDATA")
    } else {
        env("XDG_CONFIG_HOME").or_else(|| env("HOME").map(|h| h.join(".config")))
    };
    Some(base?.join("fluid-toy").join(FILE))
}

/// The saved settings, if there are any (and they parse; a bad file is ignored).
pub fn load() -> Option<Settings> {
    let path = path()?;
    let text = std::fs::read_to_string(&path).ok()?;
    match serde_json::from_str(&text) {
        Ok(s) => Some(s),
        Err(e) => {
            log::warn!("ignoring unreadable settings file {}: {e}", path.display());
            None
        }
    }
}

pub fn save(s: &UiState) -> anyhow::Result<()> {
    let path = path().ok_or_else(|| anyhow::anyhow!("no config directory"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(&Settings::from_state(s))?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let palette = crate::render::palette::PALETTES[1].1;
        let mut state =
            UiState { tool: Tool::Faucet, brush_radius: 0.15, liquid: false, tab: Tab::Info, palette, ..UiState::default() };
        state.panel_open = false;
        state.looks.marks.insert("Prism".into(), crate::looks::Mark { rating: 4, favourite: true });
        state.director.favourites_only = true;
        state.browser.tags = vec!["water".into()];
        state.director.tags_only = true;
        state.director.random_looks = true;
        state.screen = crate::ui::Screen { monitor: Some("DISPLAY2".into()), windowed: true, on_top: true, fps_cap: 60 };
        let json = serde_json::to_string(&Settings::from_state(&state)).unwrap();
        let mut back = UiState::default();
        serde_json::from_str::<Settings>(&json).unwrap().apply(&mut back);
        assert_eq!(Settings::from_state(&back), Settings::from_state(&state));
    }

    #[test]
    fn tolerates_missing_and_unknown_fields() {
        let s: Settings = serde_json::from_str(r#"{ "tool": "PaintWall", "from_the_future": 3 }"#).unwrap();
        assert_eq!(s.tool, Tool::PaintWall);
        assert_eq!(s.brush_radius, UiState::default().brush_radius);
    }

    #[test]
    fn clamps_out_of_range_values() {
        let s = Settings { brush_radius: 50.0, speed: f32::NAN, blob_size: -1.0, ..Settings::default() };
        let mut state = UiState::default();
        s.apply(&mut state);
        assert_eq!(state.brush_radius, *BRUSH_RADIUS.end());
        assert_eq!(state.speed, 0.1);
        assert_eq!(state.blob_size, 1.5);
    }
}
