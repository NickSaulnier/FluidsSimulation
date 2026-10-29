//! CPU colour mapping for field overlays: values to sRGB bytes, NaN to transparent.

use sim::FieldKind;

/// Diverging blue-white-red ("coolwarm"-like), from -1 to 1.
const DIVERGING: [[u8; 3]; 3] = [[59, 76, 192], [235, 235, 235], [180, 4, 38]];
/// Sequential, viridis-like, from 0 to 1.
const SEQUENTIAL: [[u8; 3]; 5] = [[68, 1, 84], [59, 82, 139], [33, 145, 140], [94, 201, 98], [253, 231, 37]];
/// Flame, from barely glowing to hottest: deep red, orange, yellow, near white (a
/// blackbody-like ramp; fire keeps its colours whatever the fluid palette).
const FLAME: [[u8; 3]; 4] = [[120, 18, 2], [222, 64, 8], [255, 160, 28], [255, 242, 196]];
const SMOKE: [u8; 3] = [168, 168, 176];
/// Unburned fuel: a dim amber, so you can see what's about to burn.
const FUEL: [u8; 3] = [120, 78, 34];
/// Cell types: air, fluid, solid.
const CATEGORIES: [[u8; 4]; 3] = [[200, 200, 210, 60], [70, 130, 230, 255], [110, 110, 120, 255]];

/// The range a field is normalised over: symmetric for signed fields, from zero for
/// non-negative ones. Ignores NaN; never empty.
pub fn range(values: &[f32], kind: FieldKind) -> (f32, f32) {
    let finite = values.iter().copied().filter(|v| v.is_finite());
    match kind {
        FieldKind::Signed => {
            let m = finite.map(f32::abs).fold(0.0, f32::max).max(1e-6);
            (-m, m)
        }
        FieldKind::NonNegative => (0.0, finite.fold(0.0, f32::max).max(1e-6)),
        FieldKind::Categorical => (0.0, (CATEGORIES.len() - 1) as f32),
        FieldKind::Concentration | FieldKind::Fire | FieldKind::Water => (0.0, 1.0),
    }
}

/// Map `values` to RGBA bytes (one pixel each) in `out`. Concentration (dye) uses the
/// fluid palette's `dye` ramp, with alpha rising alongside; water takes the palette's
/// own colours (deep, base, highlight) in `dye`; the other kinds use fixed analysis
/// colour maps.
pub fn colourise(values: &[f32], kind: FieldKind, (lo, hi): (f32, f32), dye: &[[u8; 3]; 3], out: &mut Vec<u8>) {
    out.clear();
    if kind == FieldKind::Water {
        // (height, slope x, slope y) triples, one pixel each.
        out.reserve(values.len() / 3 * 4);
        for &[height, sx, sy] in values.as_chunks::<3>().0 {
            out.extend_from_slice(&water_pixel(height, sx, sy, dye));
        }
        return;
    }
    if kind == FieldKind::Fire {
        // (flame, smoke, fuel) triples, one pixel each.
        out.reserve(values.len() / 3 * 4);
        for &[flame, smoke, fuel] in values.as_chunks::<3>().0 {
            out.extend_from_slice(&fire_pixel(flame, smoke, fuel));
        }
        return;
    }
    out.reserve(values.len() * 4);
    for &v in values {
        let rgba = if !v.is_finite() {
            [0, 0, 0, 0]
        } else {
            match kind {
                FieldKind::Categorical => CATEGORIES[(v.round().max(0.0) as usize).min(CATEGORIES.len() - 1)],
                FieldKind::Fire | FieldKind::Water => unreachable!("handled above"),
                FieldKind::Concentration => {
                    let t = ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
                    let [r, g, b] = ramp(dye, t);
                    // Fade out where there's little dye, so the background shows through.
                    [r, g, b, (255.0 * t.powf(0.6)).round() as u8]
                }
                _ => {
                    let t = ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
                    let [r, g, b] = if kind == FieldKind::Signed { ramp(&DIVERGING, t) } else { ramp(&SEQUENTIAL, t) };
                    [r, g, b, 255]
                }
            }
        };
        out.extend_from_slice(&rgba);
    }
}

/// Slopes are drawn this many times steeper, so ripples a millimetre high still catch
/// the light.
const WATER_RELIEF: f32 = 4.0;

/// Lit water: the palette's deep colour in the troughs to its base on the crests,
/// shaded by a light from the upper left, with a highlight glinting off the slopes
/// that face it and foam on the steepest. `height` in rest depths above rest; the
/// slopes in metres per metre. NaN (a wall) is transparent.
fn water_pixel(height: f32, sx: f32, sy: f32, [deep, base, highlight]: &[[u8; 3]; 3]) -> [u8; 4] {
    if !(height.is_finite() && sx.is_finite() && sy.is_finite()) {
        return [0, 0, 0, 0];
    }
    let n = glam::Vec3::new(-WATER_RELIEF * sx, -WATER_RELIEF * sy, 1.0).normalize();
    let light = glam::Vec3::new(-0.45, 0.55, 0.7).normalize();
    // Lit as flat water is 1; slopes toward the light brighter, away darker.
    let shade = (1.0 + 1.6 * (n.dot(light) - light.z)).clamp(0.35, 1.6);
    let glint = n.dot((light + glam::Vec3::Z).normalize()).max(0.0).powf(80.0);
    // Foam only where the water is truly steep (a breaking front, not a drop's ripple).
    let foam = ((sx.hypot(sy) - 0.6) / 0.6).clamp(0.0, 1.0);
    let body = (0.45 + 1.2 * height).clamp(0.0, 1.0);
    let mut c = [0.0f32; 3];
    for k in 0..3 {
        let colour = f32::from(deep[k]) + (f32::from(base[k]) - f32::from(deep[k])) * body;
        let lit = colour * shade;
        let sparkle = (glint * 1.4 + 0.7 * foam).min(1.0);
        c[k] = lit + (f32::from(highlight[k]) - lit) * sparkle;
    }
    let [r, g, b] = c.map(|v| v.round().clamp(0.0, 255.0) as u8);
    [r, g, b, 255]
}

/// Flame over smoke over fuel ("over" compositing), each 0 to 1; NaN is transparent.
fn fire_pixel(flame: f32, smoke: f32, fuel: f32) -> [u8; 4] {
    if !(flame.is_finite() && smoke.is_finite() && fuel.is_finite()) {
        return [0, 0, 0, 0];
    }
    let flame = flame.clamp(0.0, 1.0);
    // Back to front: fuel, then smoke, then flame; premultiplied accumulation.
    let layers = [
        (FUEL, 0.5 * fuel.clamp(0.0, 1.0)),
        (SMOKE, 0.7 * smoke.clamp(0.0, 1.0)),
        (ramp(&FLAME, flame), flame.powf(0.6)),
    ];
    let (mut rgb, mut alpha) = ([0.0f32; 3], 0.0f32);
    for (colour, a) in layers {
        for c in 0..3 {
            rgb[c] = f32::from(colour[c]) * a + rgb[c] * (1.0 - a);
        }
        alpha = a + alpha * (1.0 - a);
    }
    if alpha <= 0.0 {
        return [0, 0, 0, 0];
    }
    let channel = |c: usize| (rgb[c] / alpha).round() as u8;
    [channel(0), channel(1), channel(2), (255.0 * alpha).round() as u8]
}

/// Piecewise-linear interpolation through evenly spaced colour stops, t in [0, 1].
fn ramp(stops: &[[u8; 3]], t: f32) -> [u8; 3] {
    let x = t * (stops.len() - 1) as f32;
    let k = (x as usize).min(stops.len() - 2);
    let f = x - k as f32;
    let mix = |a: u8, b: u8| (f32::from(a) * (1.0 - f) + f32::from(b) * f).round() as u8;
    let (a, b) = (stops[k], stops[k + 1]);
    [mix(a[0], b[0]), mix(a[1], b[1]), mix(a[2], b[2])]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concentration_fades_to_transparent() {
        let mut out = Vec::new();
        let dye = crate::render::Palette::default().dye_ramp();
        colourise(&[0.0, 1.0, 2.0], FieldKind::Concentration, (0.0, 1.0), &dye, &mut out);
        assert_eq!(out[3], 0, "no dye is transparent");
        assert_eq!(out[7], 255, "full dye is opaque");
        assert_eq!(&out[4..8], &out[8..12], "over-full dye clamps");
        assert_eq!(&out[4..7], &dye[2], "full dye is the palette's highlight");
    }

    #[test]
    fn fire_draws_flame_over_smoke_over_fuel_one_pixel_per_triple() {
        let mut out = Vec::new();
        let dye = crate::render::Palette::default().dye_ramp();
        // (flame, smoke, fuel): nothing, smoke only, fuel only, full flame over everything, NaN.
        let values = [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, f32::NAN, f32::NAN, f32::NAN];
        colourise(&values, FieldKind::Fire, (0.0, 1.0), &dye, &mut out);
        assert_eq!(out.len(), 20);
        assert_eq!(out[3], 0, "nothing there: transparent");
        assert_eq!(&out[4..8], &[168, 168, 176, 179], "smoke is translucent grey");
        assert_eq!(&out[8..12], &[120, 78, 34, 128], "fuel is a dim amber");
        assert_eq!(&out[12..16], &[255, 242, 196, 255], "full flame hides what's behind it");
        assert_eq!(out[19], 0, "NaN (solid) is transparent");
    }

    #[test]
    fn ranges_ignore_nan_and_are_symmetric_for_signed_fields() {
        let v = [f32::NAN, -3.0, 1.0, 2.0];
        assert_eq!(range(&v, FieldKind::Signed), (-3.0, 3.0));
        assert_eq!(range(&v, FieldKind::NonNegative), (0.0, 2.0));
        assert_eq!(range(&[f32::NAN], FieldKind::NonNegative), (0.0, 1e-6));
    }

    #[test]
    fn colours_hit_the_ends_of_the_ramps_and_nan_is_transparent() {
        let mut out = Vec::new();
        let dye = crate::render::Palette::default().dye_ramp();
        colourise(&[-1.0, 0.0, 1.0, f32::NAN], FieldKind::Signed, (-1.0, 1.0), &dye, &mut out);
        assert_eq!(&out[0..4], &[59, 76, 192, 255]);
        assert_eq!(&out[4..8], &[235, 235, 235, 255]);
        assert_eq!(&out[8..12], &[180, 4, 38, 255]);
        assert_eq!(out[15], 0);
        colourise(&[0.0, 10.0], FieldKind::NonNegative, (0.0, 10.0), &dye, &mut out);
        assert_eq!(&out[0..3], &SEQUENTIAL[0]);
        assert_eq!(&out[4..7], &SEQUENTIAL[4]);
    }

    /// Water: opaque, crests nearer the base colour than troughs, slopes facing the
    /// light brighter than slopes facing away, a glint toward the highlight, and walls
    /// (NaN) transparent.
    #[test]
    fn water_is_lit() {
        let palette = [[10, 20, 60], [40, 120, 220], [240, 250, 255]];
        let pixel = |h: f32, sx: f32, sy: f32| {
            let mut out = Vec::new();
            colourise(&[h, sx, sy], FieldKind::Water, (0.0, 1.0), &palette, &mut out);
            [out[0], out[1], out[2], out[3]]
        };
        let sum = |p: [u8; 4]| u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]);
        let flat = pixel(0.0, 0.0, 0.0);
        assert_eq!(flat[3], 255);
        assert!(sum(pixel(0.3, 0.0, 0.0)) > sum(flat) && sum(pixel(-0.3, 0.0, 0.0)) < sum(flat), "crests lighter than troughs");
        // The light is up and to the left: a surface rising toward it faces it.
        assert!(sum(pixel(0.0, 0.1, -0.1)) > sum(flat) && sum(pixel(0.0, -0.1, 0.1)) < sum(flat), "lit by the slope");
        let glint = pixel(0.0, 0.07, -0.085);
        assert!(sum(glint) > sum(pixel(0.0, 0.1, -0.1)), "a glint where the slope mirrors the light");
        assert_eq!(pixel(f32::NAN, 0.0, 0.0)[3], 0);
    }
}

