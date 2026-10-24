//! CPU colour mapping for field overlays: values to sRGB bytes, NaN to transparent.

use sim::FieldKind;

/// Diverging blue-white-red ("coolwarm"-like), from -1 to 1.
const DIVERGING: [[u8; 3]; 3] = [[59, 76, 192], [235, 235, 235], [180, 4, 38]];
/// Sequential, viridis-like, from 0 to 1.
const SEQUENTIAL: [[u8; 3]; 5] = [[68, 1, 84], [59, 82, 139], [33, 145, 140], [94, 201, 98], [253, 231, 37]];
/// Concentration (dye), from 0 to 1, with alpha rising alongside.
const INK: [[u8; 3]; 3] = [[30, 100, 240], [90, 195, 255], [235, 248, 255]];
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
        FieldKind::Concentration => (0.0, 1.0),
    }
}

/// Map `values` to RGBA bytes (one pixel each) in `out`.
pub fn colourise(values: &[f32], kind: FieldKind, (lo, hi): (f32, f32), out: &mut Vec<u8>) {
    out.clear();
    out.reserve(values.len() * 4);
    for &v in values {
        let rgba = if !v.is_finite() {
            [0, 0, 0, 0]
        } else {
            match kind {
                FieldKind::Categorical => CATEGORIES[(v.round().max(0.0) as usize).min(CATEGORIES.len() - 1)],
                FieldKind::Concentration => {
                    let t = ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
                    let [r, g, b] = ramp(&INK, t);
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
        colourise(&[0.0, 1.0, 2.0], FieldKind::Concentration, (0.0, 1.0), &mut out);
        assert_eq!(out[3], 0, "no dye is transparent");
        assert_eq!(out[7], 255, "full dye is opaque");
        assert_eq!(&out[4..8], &out[8..12], "over-full dye clamps");
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
        colourise(&[-1.0, 0.0, 1.0, f32::NAN], FieldKind::Signed, (-1.0, 1.0), &mut out);
        assert_eq!(&out[0..4], &[59, 76, 192, 255]);
        assert_eq!(&out[4..8], &[235, 235, 235, 255]);
        assert_eq!(&out[8..12], &[180, 4, 38, 255]);
        assert_eq!(out[15], 0);
        colourise(&[0.0, 10.0], FieldKind::NonNegative, (0.0, 10.0), &mut out);
        assert_eq!(&out[0..3], &SEQUENTIAL[0]);
        assert_eq!(&out[4..7], &SEQUENTIAL[4]);
    }
}
