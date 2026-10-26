//! The composite's look (drawn in `post.wgsl`, never fed back): symmetry (mirrors and a
//! kaleidoscope), colour grading (hue, saturation, contrast, exposure), mapping the
//! picture's brightness onto the look's palette, inversion, and lens effects (chromatic
//! aberration, vignette, film grain). Everything off by default: the picture as it is.

use super::Palette;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Symmetry {
    #[default]
    #[serde(alias = "none")]
    None,
    /// The left half, mirrored onto the right.
    #[serde(alias = "mirror_x")]
    MirrorX,
    /// The top half, mirrored onto the bottom.
    #[serde(alias = "mirror_y")]
    MirrorY,
    /// The top-left quarter, mirrored into all four.
    #[serde(alias = "quad")]
    Quad,
    /// A wedge of the picture, mirrored round the centre `segments` times.
    #[serde(alias = "kaleidoscope")]
    Kaleidoscope,
}

impl Symmetry {
    pub const ALL: [Symmetry; 5] = [Symmetry::None, Symmetry::MirrorX, Symmetry::MirrorY, Symmetry::Quad, Symmetry::Kaleidoscope];

    pub fn label(self) -> &'static str {
        match self {
            Symmetry::None => "None",
            Symmetry::MirrorX => "Mirror left-right",
            Symmetry::MirrorY => "Mirror top-bottom",
            Symmetry::Quad => "Mirror four ways",
            Symmetry::Kaleidoscope => "Kaleidoscope",
        }
    }

    /// The shader's number for it.
    pub fn index(self) -> u32 {
        Symmetry::ALL.iter().position(|&s| s == self).unwrap_or(0) as u32
    }
}

/// A look's composite.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompositeSettings {
    pub symmetry: Symmetry,
    /// The kaleidoscope's mirrored wedges, 2 to 16, and how fast it turns, radians per
    /// second.
    pub segments: u32,
    pub symmetry_spin: f32,
    /// Hue turn, degrees, and drift, degrees per second.
    pub hue: f32,
    pub hue_speed: f32,
    /// 1 as is: saturation 0 is grey, contrast around mid-grey, exposure a multiple.
    pub saturation: f32,
    pub contrast: f32,
    pub exposure: f32,
    /// How far the picture's brightness is mapped onto the look's palette (dark to deep,
    /// base, highlight), 0 to 1.
    pub palette_map: f32,
    /// 0 to 1: the colours inverted.
    pub invert: f32,
    /// Colour fringes toward the edges (red out, blue in), 0 to 0.05 of the picture.
    pub aberration: f32,
    /// Darkened corners, 0 to 1; film grain, 0 to 0.3.
    pub vignette: f32,
    pub grain: f32,
}

impl CompositeSettings {
    pub const OFF: Self = Self {
        symmetry: Symmetry::None,
        segments: 6,
        symmetry_spin: 0.0,
        hue: 0.0,
        hue_speed: 0.0,
        saturation: 1.0,
        contrast: 1.0,
        exposure: 1.0,
        palette_map: 0.0,
        invert: 0.0,
        aberration: 0.0,
        vignette: 0.0,
        grain: 0.0,
    };
}

impl Default for CompositeSettings {
    fn default() -> Self {
        Self::OFF
    }
}

/// The composite's moving parts: the kaleidoscope's turn and the hue's drift (both
/// added up, so changing their speeds doesn't jump), and the grain's seed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CompositeClock {
    pub turn: f32,
    pub hue_drift: f32,
    pub seed: f32,
}

impl CompositeClock {
    /// Advance a frame of `dt` seconds (0 while paused: the picture holds, grain too).
    pub fn advance(&mut self, s: &CompositeSettings, dt: f32) {
        self.turn = (self.turn + s.symmetry_spin * dt) % std::f32::consts::TAU;
        self.hue_drift = (self.hue_drift + s.hue_speed * dt) % 360.0;
        if dt > 0.0 {
            self.seed = (self.seed + 1.618) % 1000.0;
        }
    }
}

/// What the composite draws this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Composite {
    pub settings: CompositeSettings,
    pub clock: CompositeClock,
    /// For mapping onto: the look's palette as drawn this frame.
    pub palette: Palette,
}

impl Composite {
    #[cfg(test)]
    pub const NONE: Self = Self { settings: CompositeSettings::OFF, clock: CompositeClock { turn: 0.0, hue_drift: 0.0, seed: 0.0 }, palette: super::palette::PALETTES[0].1 };
}

/// An sRGB colour as linear light, for the shader.
pub fn linear(c: [u8; 3]) -> [f32; 4] {
    let f = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    [f(c[0]), f(c[1]), f(c[2]), 1.0]
}
