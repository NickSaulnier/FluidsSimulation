//! Kick-drum detection, offline (a whole track at load) or live (`LiveDetector`, for
//! audio captured as it plays).
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
//! Offline, this runs once when a track loads (and re-picks when sensitivity changes),
//! so playback only has to compare precomputed times against the clock.
//!
//! Live, the same steps run causally: the threshold window trails the current hop
//! instead of being centred on it, peaks are confirmed one hop late, the floor comes
//! from the last several seconds instead of the whole track, and a close second peak
//! can't replace a kick that has already fired. A quiet-signal gate stands in for the
//! track-wide floor during intros and silence.

use std::collections::VecDeque;
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
/// Live: how much recent history stands in for "the whole track" (floor, strength).
const LIVE_HISTORY_S: f32 = 10.0;
/// Live: low-band RMS below this is treated as silence (about -46 dBFS).
const LIVE_GATE_RMS: f32 = 0.005;

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
    let mut filter = Biquad::lowpass(sample_rate as f32, CUTOFF_HZ);
    let low: Vec<f32> = mono.iter().map(|&x| filter.process(x)).collect();
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

/// RBJ biquad low-pass (Q = 1/√2), one sample at a time so it can also run live.
struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    x: [f32; 2],
    y: [f32; 2],
}

impl Biquad {
    fn lowpass(sample_rate: f32, cutoff: f32) -> Self {
        let w0 = 2.0 * PI * cutoff / sample_rate;
        let alpha = w0.sin() / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
        let cos = w0.cos();
        let a0 = 1.0 + alpha;
        Self {
            b: [(1.0 - cos) / 2.0 / a0, (1.0 - cos) / a0, (1.0 - cos) / 2.0 / a0],
            a: [-2.0 * cos / a0, (1.0 - alpha) / a0],
            x: [0.0; 2],
            y: [0.0; 2],
        }
    }

    fn process(&mut self, x0: f32) -> f32 {
        let [b0, b1, b2] = self.b;
        let [a1, a2] = self.a;
        let y0 = b0 * x0 + b1 * self.x[0] + b2 * self.x[1] - a1 * self.y[0] - a2 * self.y[1];
        self.x = [x0, self.x[0]];
        self.y = [y0, self.y[0]];
        y0
    }
}

/// Kick detection on audio arriving in chunks of any size, as it plays.
pub struct LiveDetector {
    filter: Biquad,
    hop_seconds: f32,
    /// The last `WINDOW` low-passed samples.
    window: VecDeque<f32>,
    since_hop: usize,
    hops: usize,
    prev_log_energy: Option<f32>,
    /// Onset values for the threshold window (trailing), newest last.
    recent: VecDeque<f32>,
    /// Onset values over `LIVE_HISTORY_S`, for the floor and the strength scale.
    history: VecDeque<f32>,
    /// The previous hop's onset value and gate state, awaiting the next hop to
    /// confirm it as a peak.
    pending: Option<(f32, bool)>,
    last_kick_hop: Option<usize>,
}

impl LiveDetector {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            filter: Biquad::lowpass(sample_rate as f32, CUTOFF_HZ),
            hop_seconds: HOP as f32 / sample_rate as f32,
            window: VecDeque::with_capacity(WINDOW),
            since_hop: 0,
            hops: 0,
            prev_log_energy: None,
            recent: VecDeque::new(),
            history: VecDeque::new(),
            pending: None,
            last_kick_hop: None,
        }
    }

    /// Feed mono samples; kicks found in them are appended to `out`, timed in seconds
    /// since the detector started (the start of their analysis frame, as offline).
    pub fn process(&mut self, mono: &[f32], sensitivity: f32, out: &mut Vec<Kick>) {
        for &x in mono {
            if self.window.len() == WINDOW {
                self.window.pop_front();
            }
            self.window.push_back(self.filter.process(x));
            self.since_hop += 1;
            if self.window.len() == WINDOW && self.since_hop >= HOP {
                self.since_hop = 0;
                self.hop(sensitivity, out);
            }
        }
    }

    fn hop(&mut self, sensitivity: f32, out: &mut Vec<Kick>) {
        let energy = self.window.iter().map(|x| x * x).sum::<f32>() / WINDOW as f32;
        let log = (energy + 1e-7).ln();
        let value = self.prev_log_energy.map_or(0.0, |prev| (log - prev).max(0.0));
        self.prev_log_energy = Some(log);
        let audible = energy.sqrt() > LIVE_GATE_RMS;

        let context = ((CONTEXT_S / self.hop_seconds) as usize).max(1);
        let history_len = ((LIVE_HISTORY_S / self.hop_seconds) as usize).max(1);
        push_capped(&mut self.recent, value, 2 * context + 1);
        push_capped(&mut self.history, value, history_len);

        // The previous hop is a peak if it's at least as big as both neighbours.
        if let Some((candidate, was_audible)) = self.pending.replace((value, audible))
            && was_audible
            && candidate > 0.0
            && candidate >= value
        {
            let k = self.hops - 1;
            let before = self.recent.iter().rev().nth(2).copied().unwrap_or(0.0);
            // Threshold over the trailing window, which ends at the candidate.
            let window: Vec<f32> = self.recent.iter().take(self.recent.len() - 1).copied().collect();
            let mean = window.iter().sum::<f32>() / window.len() as f32;
            let var = window.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / window.len() as f32;
            let mut nonzero: Vec<f32> = self.history.iter().copied().filter(|&x| x > 0.0).collect();
            nonzero.sort_by(f32::total_cmp);
            let strong = nonzero.get((nonzero.len() as f32 * 0.98) as usize).copied().unwrap_or(candidate);
            let min_gap = ((MIN_GAP_S / self.hop_seconds) as usize).max(1);
            let spaced = self.last_kick_hop.is_none_or(|last| k - last >= min_gap);
            if candidate >= before
                && candidate > mean + sensitivity * var.sqrt()
                && candidate >= FLOOR_FRACTION * strong
                && spaced
            {
                let scale = nonzero.last().copied().unwrap_or(candidate).max(1e-6);
                out.push(Kick { time: k as f32 * self.hop_seconds, strength: (candidate / scale).min(1.0) });
                self.last_kick_hop = Some(k);
            }
        }
        self.hops += 1;
    }
}

fn push_capped(q: &mut VecDeque<f32>, v: f32, cap: usize) {
    if q.len() == cap {
        q.pop_front();
    }
    q.push_back(v);
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

    /// The live detector, fed the same beat in uneven chunks as a capture callback would,
    /// finds the same kicks (after a second of warm-up, which the trailing window and
    /// history need) and is unaffected by how the audio was chunked.
    #[test]
    fn live_detector_finds_the_kicks_in_any_chunking() {
        let rate = 44_100;
        let (signal, truth) = synthetic_beat(rate, 10.0, 120.0);
        let run = |chunk: usize| {
            let mut detector = LiveDetector::new(rate);
            let mut kicks = Vec::new();
            for (i, part) in signal.chunks(chunk).enumerate() {
                // Vary the chunk size, as real callbacks do.
                let split = (i * 37) % part.len().max(1);
                detector.process(&part[..split], 1.5, &mut kicks);
                detector.process(&part[split..], 1.5, &mut kicks);
            }
            kicks
        };
        let kicks = run(441);
        assert_eq!(kicks, run(4096), "chunking changed the result");
        let tolerance = 0.03;
        for t in truth.iter().filter(|&&t| t > 1.0) {
            assert!(kicks.iter().any(|k| (k.time - t).abs() < tolerance), "missed the kick at {t:.3} s; found {kicks:?}");
        }
        for k in &kicks {
            assert!(truth.iter().any(|t| (k.time - t).abs() < tolerance), "false kick at {:.3} s", k.time);
        }
        assert!(kicks.iter().all(|k| (0.0..=1.0).contains(&k.strength)));
    }

    /// Silence, and a quiet noise bed with no beat (an intro), give no live kicks.
    #[test]
    fn live_detector_ignores_silence_and_quiet_noise() {
        let rate = 44_100;
        let mut kicks = Vec::new();
        LiveDetector::new(rate).process(&vec![0.0; 5 * rate as usize], 1.5, &mut kicks);
        assert!(kicks.is_empty(), "{kicks:?}");
        let (beat, _) = synthetic_beat(rate, 5.0, 120.0);
        // The beat generator's noise bed alone: everything before its first kick, looped.
        let bed: Vec<f32> = beat[..(0.2 * rate as f32) as usize].iter().cycle().take(5 * rate as usize).copied().collect();
        LiveDetector::new(rate).process(&bed, 1.5, &mut kicks);
        assert!(kicks.is_empty(), "noise bed fired {} kicks", kicks.len());
    }

    #[test]
    fn silence_has_no_kicks() {
        assert!(pick_kicks(&onset_function(&vec![0.0; 44_100], 44_100), 1.0).is_empty());
        assert!(pick_kicks(&onset_function(&[], 44_100), 1.0).is_empty());
    }
}
