//! The fluid's colours: one palette drives the particle discs, the liquid surface and
//! the dye of grid solvers, so a colour choice looks the same whichever solver runs.
//! (The overlay colour maps for analysis fields stay fixed: they encode data.)
//!
//! A palette can also come from a picture (`Palette::from_picture`): the song's cover
//! art, so each song gets its own colours.

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

/// How long a palette takes to ease most of the way to a new one, seconds.
const EASE_S: f32 = 0.8;

/// A palette easing toward whatever it's asked to follow, so colours change smoothly
/// (from one song's cover to the next, or back to the look's own).
#[derive(Clone, Copy, Debug, Default)]
pub struct PaletteEase {
    now: Option<[[f32; 3]; 3]>,
}

impl PaletteEase {
    /// Ease `dt` seconds toward `target` (start there, the first time).
    pub fn follow(&mut self, target: &Palette, dt: f32) -> Palette {
        let goal = [target.deep, target.base, target.highlight].map(|c| c.map(f32::from));
        let now = self.now.get_or_insert(goal);
        let k = 1.0 - (-dt / EASE_S).exp();
        for (c, g) in now.iter_mut().zip(goal) {
            for (v, g) in c.iter_mut().zip(g) {
                *v += (g - *v) * k;
            }
        }
        let byte = |c: [f32; 3]| c.map(|v| v.round().clamp(0.0, 255.0) as u8);
        Palette { deep: byte(now[0]), base: byte(now[1]), highlight: byte(now[2]) }
    }
}

/// How many colours a picture is reduced to before choosing its palette's three.
const PICTURE_COLOURS: usize = 6;
/// A colour needs this share of the picture to be chosen as its highlight or deep.
const MIN_SHARE: f32 = 0.03;

impl Palette {
    /// A palette from a picture (RGBA, 8 bits a channel): its pixels reduced to a few
    /// colours (k-means), then `base` the commonest colourful one, `highlight` the
    /// brightest (lifted toward white), and `deep` the base darkened toward the darkest.
    /// Ordered by brightness whatever the picture, so it always reads as a palette.
    pub fn from_picture(rgba: &[u8]) -> Palette {
        let pixels: Vec<[f32; 3]> = rgba.as_chunks::<4>().0.iter().step_by((rgba.len() / 4 / 4096).max(1)).map(|p| [p[0], p[1], p[2]].map(|v| f32::from(v) / 255.0)).collect();
        if pixels.is_empty() {
            return Palette::default();
        }
        let luma = |c: &[f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        let saturation = |c: &[f32; 3]| {
            let (hi, lo) = (c[0].max(c[1]).max(c[2]), c[0].min(c[1]).min(c[2]));
            if hi > 0.0 { (hi - lo) / hi } else { 0.0 }
        };
        // k-means, started from the brightness quantiles (so it's the same every time).
        let mut by_luma = pixels.clone();
        by_luma.sort_by(|a, b| luma(a).total_cmp(&luma(b)));
        let k = PICTURE_COLOURS.min(pixels.len());
        let mut centres: Vec<[f32; 3]> = (0..k).map(|i| by_luma[(2 * i + 1) * by_luma.len() / (2 * k)]).collect();
        let mut counts = vec![0usize; k];
        for _ in 0..10 {
            let mut sums = vec![[0.0f32; 3]; k];
            counts = vec![0; k];
            for p in &pixels {
                let distance = |c: &[f32; 3]| (0..3).map(|i| (p[i] - c[i]).powi(2)).sum::<f32>();
                let nearest = (0..k).min_by(|&a, &b| distance(&centres[a]).total_cmp(&distance(&centres[b]))).expect("k > 0");
                counts[nearest] += 1;
                for i in 0..3 {
                    sums[nearest][i] += p[i];
                }
            }
            for c in 0..k {
                if counts[c] > 0 {
                    centres[c] = sums[c].map(|v| v / counts[c] as f32);
                }
            }
        }
        let share = |c: usize| counts[c] as f32 / pixels.len() as f32;
        let present: Vec<usize> = (0..k).filter(|&c| share(c) >= MIN_SHARE).collect();
        let present = if present.is_empty() { (0..k).collect() } else { present };
        let best = |score: &dyn Fn(usize) -> f32| present.iter().copied().max_by(|&a, &b| score(a).total_cmp(&score(b))).expect("some colour");
        let highlight = best(&|c| luma(&centres[c]) + 0.3 * saturation(&centres[c]));
        let darkest = best(&|c| -luma(&centres[c]));
        let base = best(&|c| {
            let l = luma(&centres[c]);
            let mid = if (0.12..0.85).contains(&l) { 1.0 } else { 0.3 };
            share(c) * (0.15 + saturation(&centres[c])).powi(2) * mid * if c == highlight { 0.5 } else { 1.0 }
        });
        let mut base_c = centres[base];
        // Keep the three apart: the base mid-bright, the highlight near white, the deep dark.
        let lift = |c: [f32; 3], to: f32| {
            let l = luma(&c);
            if l >= to { c } else { c.map(|v| v + (1.0 - v) * ((to - l) / (1.0 - l).max(1e-3))) }
        };
        let scale_to = |c: [f32; 3], to: f32| {
            let l = luma(&c);
            if l < 0.01 { [to; 3] } else { c.map(|v| (v * to / l).min(1.0)) }
        };
        let l = luma(&base_c);
        if l < 0.25 {
            base_c = scale_to(base_c, 0.3);
        } else if l > 0.7 {
            base_c = scale_to(base_c, 0.6);
        }
        let highlight_c = lift(centres[highlight], (luma(&base_c) + 0.3).min(0.9));
        let dark = centres[darkest];
        let deep_c: [f32; 3] = std::array::from_fn(|i| 0.5 * (base_c[i] * 0.35) + 0.5 * dark[i].min(base_c[i]));
        let deep_c = if luma(&deep_c) > 0.5 * luma(&base_c) { scale_to(deep_c, 0.4 * luma(&base_c)) } else { deep_c };
        let byte = |c: [f32; 3]| c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
        Palette { deep: byte(deep_c), base: byte(base_c), highlight: byte(highlight_c) }
    }
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

    /// A picture's palette: the commonest colour as the base, the brightest as the
    /// highlight, a dark deep; always ordered by brightness, even from a grey picture.
    #[test]
    fn pictures_give_palettes() {
        let luma = |c: [u8; 3]| 0.2126 * f32::from(c[0]) + 0.7152 * f32::from(c[1]) + 0.0722 * f32::from(c[2]);
        let picture = |parts: &[([u8; 3], usize)]| -> Vec<u8> { parts.iter().flat_map(|&(c, n)| std::iter::repeat_n([c[0], c[1], c[2], 255], n)).flatten().collect() };
        let ordered = |p: Palette| luma(p.deep) < luma(p.base) && luma(p.base) < luma(p.highlight);
        // Mostly orange, some navy, a little pale yellow.
        let p = Palette::from_picture(&picture(&[([230, 120, 30], 6000), ([20, 30, 80], 3000), ([250, 240, 170], 1000)]));
        assert!(ordered(p), "{p:?}");
        assert!(p.base[0] > p.base[2] + 80, "the base isn't orange: {p:?}");
        assert!(p.highlight[0] > 220 && p.highlight[1] > 200, "the highlight isn't pale: {p:?}");
        let grey = Palette::from_picture(&picture(&[([128, 128, 128], 5000)]));
        assert!(ordered(grey), "{grey:?}");
        let black = Palette::from_picture(&picture(&[([0, 0, 0], 4000), ([200, 30, 40], 400)]));
        assert!(ordered(black) && black.base[0] > black.base[1], "a red on black: {black:?}");
        assert_eq!(Palette::from_picture(&[]), Palette::default());
    }

    #[test]
    fn the_ease_starts_at_its_target_and_follows() {
        let (a, b) = (PALETTES[0].1, PALETTES[1].1);
        let mut ease = PaletteEase::default();
        assert_eq!(ease.follow(&a, 0.016), a);
        let partway = ease.follow(&b, 0.4);
        assert!(partway != a && partway != b, "{partway:?}");
        for _ in 0..300 {
            ease.follow(&b, 0.016);
        }
        assert_eq!(ease.follow(&b, 0.016), b);
    }

    #[test]
    fn dye_ramp_runs_from_base_to_highlight() {
        let p = Palette::default();
        let ramp = p.dye_ramp();
        assert_eq!((ramp[0], ramp[2]), (p.base, p.highlight));
    }
}
