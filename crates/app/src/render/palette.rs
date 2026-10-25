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

#[cfg(test)]
mod tests {
    use super::*;

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
