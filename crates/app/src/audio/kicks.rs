//! Offline kick-drum detection.
//!
//! 1. Low-pass the mono signal (biquad, ~150 Hz): kick drums live there, hi-hats and most
//!    melody don't.
//! 2. Frame energy every `HOP` samples, then a log-energy onset function: the positive
//!    part of the frame-to-frame rise in dB-like units, so loud and quiet passages count
//!    alike.
//! 3. Peak picking against an adaptive threshold (local mean + `sensitivity` x local
//!    standard deviation over about a second), at least `MIN_GAP` apart. The adaptive
//!    threshold alone fires on noise in quiet stretches (intros, breakdowns, the tail),
//!    where it is only relative to noise, so peaks must also clear a floor relative to
//!    the whole track's strong onsets.
//!
//! Done once when a track loads (and re-picked when sensitivity changes), so playback
//! only has to compare precomputed times against the clock.

use std::f32::consts::PI;

const CUTOFF_HZ: f32 = 150.0;
const HOP: usize = 512;
const WINDOW: usize = 1024;
/// Half-width of the adaptive-threshold window, seconds.
const CONTEXT_S: f32 = 0.5;
/// Minimum time between kicks, seconds.
const MIN_GAP_S: f32 = 0.1;
/// Peaks must reach this fraction of the track's strong onsets (98th percentile of the
/// non-zero onset values).
const FLOOR_FRACTION: f32 = 0.2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Kick {
    /// Seconds from the start of the track.
    pub time: f32,
    /// 0..1, relative to the strongest kick in the track.
    pub strength: f32,
}

/// The onset function: one value per hop.
pub struct Onsets {
    pub hop_seconds: f32,
    pub values: Vec<f32>,
}

/// Low-pass, frame, and turn into an onset function.
pub fn onset_function(mono: &[f32], sample_rate: u32) -> Onsets {
    let low = lowpass(mono, sample_rate as f32, CUTOFF_HZ);
    let frames = low.len().saturating_sub(WINDOW) / HOP + 1;
    let energy: Vec<f32> = (0..frames)
        .map(|k| {
            let w = &low[k * HOP..(k * HOP + WINDOW).min(low.len())];
            w.iter().map(|x| x * x).sum::<f32>() / w.len().max(1) as f32
        })
        .collect();
    let log = |e: f32| (e + 1e-7).ln();
    let mut values = vec![0.0; energy.len()];
    for k in 1..energy.len() {
        values[k] = (log(energy[k]) - log(energy[k - 1])).max(0.0);
    }
    Onsets { hop_seconds: HOP as f32 / sample_rate as f32, values }
}

/// Peaks of the onset function above `mean + sensitivity * std` of their surroundings.
/// Lower sensitivity finds more kicks.
pub fn pick_kicks(onsets: &Onsets, sensitivity: f32) -> Vec<Kick> {
    let v = &onsets.values;
    let context = ((CONTEXT_S / onsets.hop_seconds) as usize).max(1);
    let min_gap = ((MIN_GAP_S / onsets.hop_seconds) as usize).max(1);
    let mut nonzero: Vec<f32> = v.iter().copied().filter(|&x| x > 0.0).collect();
    nonzero.sort_by(f32::total_cmp);
    let floor = nonzero.get((nonzero.len() as f32 * 0.98) as usize).map_or(f32::INFINITY, |&x| FLOOR_FRACTION * x);
    let mut picked: Vec<(usize, f32)> = Vec::new();
    for k in 1..v.len().saturating_sub(1) {
        if v[k] <= 0.0 || v[k] < v[k - 1] || v[k] < v[k + 1] {
            continue;
        }
        let window = &v[k.saturating_sub(context)..(k + context + 1).min(v.len())];
        let mean = window.iter().sum::<f32>() / window.len() as f32;
        let var = window.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / window.len() as f32;
        if v[k] <= mean + sensitivity * var.sqrt() || v[k] < floor {
            continue;
        }
        match picked.last_mut() {
            // Too close to the previous kick: keep whichever is stronger.
            Some(last) if k - last.0 < min_gap => {
                if v[k] > last.1 {
                    *last = (k, v[k]);
                }
            }
            _ => picked.push((k, v[k])),
        }
    }
    let strongest = picked.iter().map(|p| p.1).fold(0.0, f32::max).max(1e-6);
    picked.into_iter().map(|(k, s)| Kick { time: k as f32 * onsets.hop_seconds, strength: s / strongest }).collect()
}

/// RBJ biquad low-pass (Q = 1/√2).
fn lowpass(x: &[f32], sample_rate: f32, cutoff: f32) -> Vec<f32> {
    let w0 = 2.0 * PI * cutoff / sample_rate;
    let alpha = w0.sin() / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
    let cos = w0.cos();
    let a0 = 1.0 + alpha;
    let (b0, b1, b2) = ((1.0 - cos) / 2.0 / a0, (1.0 - cos) / a0, (1.0 - cos) / 2.0 / a0);
    let (a1, a2) = (-2.0 * cos / a0, (1.0 - alpha) / a0);
    let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
    x.iter()
        .map(|&x0| {
            let y0 = b0 * x0 + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
            (x2, x1, y2, y1) = (x1, x0, y1, y0);
            y0
        })
        .collect()
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A synthetic beat: kick drums (pitch-swept sine, 150 -> 50 Hz, fast decay) on every
    /// beat, hi-hats (bursts of high-passed noise) on the off-beats, and a quiet noise bed.
    /// Returns the samples and the kick times. Also used to make a demo track.
    pub fn synthetic_beat(sample_rate: u32, seconds: f32, bpm: f32) -> (Vec<f32>, Vec<f32>) {
        let n = (seconds * sample_rate as f32) as usize;
        let beat = 60.0 / bpm;
        let mut out = vec![0.0f32; n];
        let mut seed = 0x1234_5678u32;
        let mut noise = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as f32 / u32::MAX as f32 * 2.0 - 1.0
        };
        for s in out.iter_mut() {
            *s = 0.02 * noise();
        }
        let mut kicks = Vec::new();
        let mut t = 0.25;
        while t + beat < seconds {
            kicks.push(t);
            let start = (t * sample_rate as f32) as usize;
            let mut phase = 0.0f32;
            for k in 0..(0.25 * sample_rate as f32) as usize {
                let tt = k as f32 / sample_rate as f32;
                let freq = 50.0 + 100.0 * (-tt * 30.0).exp();
                phase += 2.0 * PI * freq / sample_rate as f32;
                out[start + k] += 0.8 * phase.sin() * (-tt * 12.0).exp();
            }
            // Hi-hat halfway to the next beat.
            let hat = ((t + beat / 2.0) * sample_rate as f32) as usize;
            let mut prev = 0.0;
            for k in 0..(0.05 * sample_rate as f32) as usize {
                let white = noise();
                let high = white - prev; // crude high-pass
                prev = white;
                out[hat + k] += 0.4 * high * (-(k as f32) / (0.01 * sample_rate as f32)).exp();
            }
            t += beat;
        }
        (out, kicks)
    }

    #[test]
    fn finds_the_kicks_and_ignores_the_hihats() {
        let rate = 44_100;
        let (signal, truth) = synthetic_beat(rate, 10.0, 120.0);
        let kicks = pick_kicks(&onset_function(&signal, rate), 1.5);
        let tolerance = 0.03;
        for t in &truth {
            assert!(kicks.iter().any(|k| (k.time - t).abs() < tolerance), "missed the kick at {t:.3} s; found {kicks:?}");
        }
        for k in &kicks {
            assert!(truth.iter().any(|t| (k.time - t).abs() < tolerance), "false kick at {:.3} s", k.time);
        }
        assert!(kicks.iter().all(|k| (0.0..=1.0).contains(&k.strength)));
    }

    #[test]
    fn silence_has_no_kicks() {
        assert!(pick_kicks(&onset_function(&vec![0.0; 44_100], 44_100), 1.0).is_empty());
        assert!(pick_kicks(&onset_function(&[], 44_100), 1.0).is_empty());
    }
}
