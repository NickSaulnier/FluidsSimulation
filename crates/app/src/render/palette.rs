//! The fluid's colours: one palette drives the particle discs, the liquid surface and
//! the dye of grid solvers, so a colour choice looks the same whichever solver runs.
//! (The overlay colour maps for analysis fields stay fixed: they encode data.)

use serde::{Deserialize, Serialize};

/// Three sRGB colours: `deep` for thick liquid, `base` for the fluid's own colour, and
/// `highlight` for fast flow, foam and dense dye.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Palette {
    pub deep: [u8; 3],
    pub base: [u8; 3],
    pub highlight: [u8; 3],
}

impl Default for Palette {
    fn default() -> Self {
        PALETTES[0].1
    }
}

pub const PALETTES: &[(&str, Palette)] = &[
    ("Ocean", Palette { deep: [13, 52, 145], base: [48, 124, 219], highlight: [218, 239, 255] }),
    ("Lava", Palette { deep: [110, 12, 4], base: [222, 72, 14], highlight: [255, 222, 120] }),
    ("Slime", Palette { deep: [14, 78, 28], base: [64, 186, 58], highlight: [214, 255, 170] }),
    ("Ink", Palette { deep: [52, 14, 96], base: [146, 52, 210], highlight: [250, 196, 255] }),
    ("Mercury", Palette { deep: [44, 48, 56], base: [128, 136, 150], highlight: [244, 246, 250] }),
];

impl Palette {
    /// The preset this palette is, if any.
    pub fn name(&self) -> Option<&'static str> {
        PALETTES.iter().find(|(_, p)| p == self).map(|(name, _)| *name)
    }

    /// The dye ramp, from a trace of dye to full: the base colour, a lighter blend, and
    /// the highlight. (Transparency fades in separately.)
    pub fn dye_ramp(&self) -> [[u8; 3]; 3] {
        let mix = |a: u8, b: u8| (f32::from(a) + 0.4 * (f32::from(b) - f32::from(a))).round() as u8;
        let (b, h) = (self.base, self.highlight);
        [b, [mix(b[0], h[0]), mix(b[1], h[1]), mix(b[2], h[2])], h]
    }

    /// The palette with its hue rotated by `hue_deg` and lightened toward white by
    /// `flash` (0..1): how music reactions colour the fluid without touching the
    /// chosen palette.
    pub fn modulated(&self, hue_deg: f32, flash: f32) -> Self {
        if hue_deg == 0.0 && flash <= 0.0 {
            return *self;
        }
        let f = |c: [u8; 3]| lighten(rotate_hue(c, hue_deg), 0.6 * flash.clamp(0.0, 1.0));
        Self { deep: f(self.deep), base: f(self.base), highlight: f(self.highlight) }
    }

    /// The three colours in linear space, padded to `vec4` for a uniform buffer.
    pub fn linear(&self) -> [[f32; 4]; 3] {
        let lin = |c: [u8; 3]| {
            let f = |v: u8| {
                let s = f32::from(v) / 255.0;
                if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
            };
            [f(c[0]), f(c[1]), f(c[2]), 1.0]
        };
        [lin(self.deep), lin(self.base), lin(self.highlight)]
    }
}

/// Rotate a colour's hue (HSV) by `deg`, keeping saturation and value.
fn rotate_hue(c: [u8; 3], deg: f32) -> [u8; 3] {
    let [r, g, b] = c.map(|v| f32::from(v) / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let delta = max - min;
    if delta <= 0.0 {
        return c;
    }
    let hue = if max == r {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    let h = (hue + deg).rem_euclid(360.0) / 60.0;
    let x = delta * (1.0 - (h.rem_euclid(2.0) - 1.0).abs());
    let (r1, g1, b1) = match h as u32 {
        0 => (delta, x, 0.0),
        1 => (x, delta, 0.0),
        2 => (0.0, delta, x),
        3 => (0.0, x, delta),
        4 => (x, 0.0, delta),
        _ => (delta, 0.0, x),
    };
    [r1, g1, b1].map(|v| ((v + min) * 255.0).round().clamp(0.0, 255.0) as u8)
}

fn lighten(c: [u8; 3], amount: f32) -> [u8; 3] {
    c.map(|v| (f32::from(v) + (255.0 - f32::from(v)) * amount).round() as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modulation_rotates_hue_and_lightens() {
        let p = Palette::default();
        assert_eq!(p.modulated(0.0, 0.0), p);
        assert_eq!(rotate_hue([255, 0, 0], 120.0), [0, 255, 0]);
        assert_eq!(rotate_hue([128, 128, 128], 90.0), [128, 128, 128], "grey has no hue");
        let full_turn = p.modulated(360.0, 0.0);
        assert!(full_turn.base.iter().zip(p.base).all(|(a, b)| a.abs_diff(b) <= 1));
        let flashed = p.modulated(0.0, 1.0);
        assert!(flashed.base.iter().zip(p.base).all(|(a, b)| *a >= b) && flashed.base != p.base);
    }

    #[test]
    fn presets_are_named_and_custom_colours_are_not() {
        assert_eq!(Palette::default().name(), Some("Ocean"));
        let custom = Palette { base: [1, 2, 3], ..Palette::default() };
        assert_eq!(custom.name(), None);
    }

    #[test]
    fn linear_conversion_hits_the_ends_and_the_srgb_midpoint() {
        let p = Palette { deep: [0, 0, 0], base: [188, 188, 188], highlight: [255, 255, 255] };
        let [deep, base, highlight] = p.linear();
        assert_eq!(deep[0], 0.0);
        assert!((base[0] - 0.5).abs() < 0.01, "sRGB 188 is about half linear, got {}", base[0]);
        assert!((highlight[0] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn dye_ramp_runs_from_base_to_highlight() {
        let p = Palette::default();
        let ramp = p.dye_ramp();
        assert_eq!((ramp[0], ramp[2]), (p.base, p.highlight));
    }
}
