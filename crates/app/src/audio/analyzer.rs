//! Streaming audio analysis: one pipeline for live capture and for track playback.
//!
//! Every `HOP` samples a Hann-windowed `FFT_SIZE` FFT gives the power spectrum, from which:
//! - **Bands:** energy in five display bands (sub to high), as smoothed amplitude
//!   envelopes normalised by an automatic gain control (each band's own slowly decaying
//!   peak), so quiet and loud tracks both span 0..1.
//! - **Level:** the same for overall loudness.
//! - **Onsets:** kick, snare and hi-hat hits from three detection bands. The kick
//!   detector of the earlier live mode, generalised: the positive frame-to-frame rise in
//!   log energy, picked against a trailing adaptive threshold (mean + sensitivity × std
//!   over about a second), a floor from the last 10 s of onsets, a minimum gap, a
//!   loudness gate for silence, and a minimum rise. Peaks are confirmed one hop late.
//!   The minimum rise matters for wide bands: in steady hiss the adaptive threshold
//!   alone always finds some "unusual" random wobble (measured: about 10 snare and hat
//!   hits a second in noise), but those wobbles stay under 1.3x per hop, while a real
//!   hit jumps several-fold.
//! - **Brightness:** the spectral centroid on a log scale, 0 (dull) to 1 (bright).
//! - **Silence:** quiet for half a second.
//!
//! Times are seconds since the analyzer started, at the centre of each analysis window
//! (where a hit's energy rise is detected).

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use std::collections::VecDeque;
use std::sync::Arc;

pub const FFT_SIZE: usize = 2048;
pub const HOP: usize = 512;
pub const BANDS: usize = 5;
pub const BAND_NAMES: [&str; BANDS] = ["sub", "bass", "low-mid", "mid", "high"];
/// Display band edges, Hz.
const BAND_EDGES: [f32; BANDS + 1] = [20.0, 60.0, 250.0, 500.0, 2000.0, 16000.0];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnsetKind {
    Kick,
    Snare,
    Hat,
}

impl OnsetKind {
    pub const ALL: [OnsetKind; 3] = [OnsetKind::Kick, OnsetKind::Snare, OnsetKind::Hat];

    /// Detection band, Hz.
    fn range(self) -> (f32, f32) {
        match self {
            OnsetKind::Kick => (30.0, 150.0),
            OnsetKind::Snare => (1000.0, 4000.0),
            OnsetKind::Hat => (6000.0, 16000.0),
        }
    }

    /// Band amplitude below which hits aren't picked (silence, quiet intros): about
    /// -54 dBFS. Low enough for a track mastered 20 dB quiet.
    fn gate(self) -> f32 {
        0.002
    }

    pub fn name(self) -> &'static str {
        match self {
            OnsetKind::Kick => "kick",
            OnsetKind::Snare => "snare",
            OnsetKind::Hat => "hat",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Onset {
    pub kind: OnsetKind,
    /// Seconds since the analyzer started.
    pub time: f32,
    /// 0..1, relative to the strongest recent hit of its kind.
    pub strength: f32,
}

/// The analysis as of the latest hop.
#[derive(Clone, Debug, Default)]
pub struct AudioFeatures {
    /// Seconds of audio analysed.
    pub time: f32,
    /// Overall loudness, normalised by the AGC: 0..1.
    pub level: f32,
    /// Overall RMS amplitude, raw (full scale = 1).
    pub rms: f32,
    /// Band envelopes, normalised: 0..1.
    pub bands: [f32; BANDS],
    /// 0 (dull) to 1 (bright).
    pub brightness: f32,
    pub silent: bool,
}

/// Envelope attack and release, seconds.
const ATTACK_S: f32 = 0.01;
const RELEASE_S: f32 = 0.2;
/// The AGC's peak decays with this time constant, seconds.
const AGC_S: f32 = 10.0;
/// Amplitude treated as the quietest worth normalising (about -60 dBFS), so silence
/// isn't amplified into noise.
const AGC_FLOOR: f32 = 1e-3;
/// Overall loudness below this (about -46 dBFS) for `SILENCE_S` counts as silence.
const SILENCE_RMS: f32 = 0.005;
const SILENCE_S: f32 = 0.5;
/// Brightness maps the centroid from this (dull) to that (bright), log scale, Hz.
const DULL_HZ: f32 = 200.0;
const BRIGHT_HZ: f32 = 8000.0;

/// Onset picking (see the module docs).
const CONTEXT_S: f32 = 0.5;
const MIN_GAP_S: f32 = 0.1;
const FLOOR_FRACTION: f32 = 0.2;
const HISTORY_S: f32 = 10.0;
/// A hit must raise its band's energy by at least this factor in one hop (log: 0.4, a
/// factor of 1.5).
const MIN_RISE: f32 = 0.4;

pub struct Analyzer {
    hop_seconds: f32,
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    /// The last `FFT_SIZE` samples.
    frame: VecDeque<f32>,
    since_hop: usize,
    hops: usize,
    buffer: Vec<Complex32>,
    scratch: Vec<Complex32>,
    /// Bin ranges for the display bands and the onset bands.
    band_bins: [(usize, usize); BANDS],
    onset_bins: [(usize, usize); 3],
    /// Power-spectrum normalisation: band energy in mean-square amplitude.
    power_scale: f32,
    bin_hz: f32,
    band_env: [Envelope; BANDS],
    level_env: Envelope,
    pickers: [OnsetPicker; 3],
    quiet_for: f32,
    features: AudioFeatures,
    onsets: Vec<Onset>,
}

impl Analyzer {
    pub fn new(sample_rate: u32) -> Self {
        let rate = sample_rate as f32;
        let fft = FftPlanner::new().plan_fft_forward(FFT_SIZE);
        let window: Vec<f32> =
            (0..FFT_SIZE).map(|k| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * k as f32 / FFT_SIZE as f32).cos()).collect();
        let window_power: f32 = window.iter().map(|w| w * w).sum();
        let bin_hz = rate / FFT_SIZE as f32;
        let bins = |lo: f32, hi: f32| {
            let a = ((lo / bin_hz).round() as usize).clamp(1, FFT_SIZE / 2 - 1);
            let b = ((hi / bin_hz).round() as usize).clamp(a + 1, FFT_SIZE / 2);
            (a, b)
        };
        let scratch_len = fft.get_inplace_scratch_len();
        let hop_seconds = HOP as f32 / rate;
        Self {
            hop_seconds,
            fft,
            window,
            frame: VecDeque::with_capacity(FFT_SIZE),
            since_hop: 0,
            hops: 0,
            buffer: vec![Complex32::default(); FFT_SIZE],
            scratch: vec![Complex32::default(); scratch_len],
            band_bins: std::array::from_fn(|b| bins(BAND_EDGES[b], BAND_EDGES[b + 1])),
            onset_bins: OnsetKind::ALL.map(|k| bins(k.range().0, k.range().1)),
            // One-sided spectrum: x2 for the mirrored half; Parseval with the window's power.
            power_scale: 2.0 / (FFT_SIZE as f32 * window_power),
            bin_hz,
            band_env: Default::default(),
            level_env: Envelope::default(),
            pickers: std::array::from_fn(|_| OnsetPicker::new(hop_seconds)),
            quiet_for: 0.0,
            features: AudioFeatures { silent: true, ..AudioFeatures::default() },
            onsets: Vec::new(),
        }
    }

    pub fn features(&self) -> &AudioFeatures {
        &self.features
    }

    /// Onsets found since the last call.
    pub fn take_onsets(&mut self) -> Vec<Onset> {
        std::mem::take(&mut self.onsets)
    }

    /// Feed mono samples of any chunk size. `sensitivity` is the onset threshold in
    /// standard deviations (higher: only the clearest hits).
    pub fn process(&mut self, mono: &[f32], sensitivity: f32) {
        for &x in mono {
            if self.frame.len() == FFT_SIZE {
                self.frame.pop_front();
            }
            self.frame.push_back(x);
            self.since_hop += 1;
            if self.frame.len() == FFT_SIZE && self.since_hop >= HOP {
                self.since_hop = 0;
                self.hop(sensitivity);
            }
        }
    }

    fn hop(&mut self, sensitivity: f32) {
        for ((out, &x), &w) in self.buffer.iter_mut().zip(&self.frame).zip(&self.window) {
            *out = Complex32::new(x * w, 0.0);
        }
        self.fft.process_with_scratch(&mut self.buffer, &mut self.scratch);
        let power: Vec<f32> = self.buffer[..FFT_SIZE / 2].iter().map(|c| c.norm_sqr() * self.power_scale).collect();
        let energy = |(a, b): (usize, usize)| power[a..b].iter().sum::<f32>();

        let dt = self.hop_seconds;
        let time = self.hops as f32 * dt + 0.5 * FFT_SIZE as f32 * dt / HOP as f32;
        let total: f32 = power[1..].iter().sum();
        let rms = total.sqrt();
        let f = &mut self.features;
        f.time = time;
        f.rms = rms;
        f.level = self.level_env.update(rms, dt);
        for b in 0..BANDS {
            f.bands[b] = self.band_env[b].update(energy(self.band_bins[b]).sqrt(), dt);
        }
        let weighted: f32 = power.iter().enumerate().skip(1).map(|(k, p)| k as f32 * self.bin_hz * p).sum();
        let centroid = if total > 0.0 { weighted / total } else { DULL_HZ };
        let bright = ((centroid.max(1.0) / DULL_HZ).ln() / (BRIGHT_HZ / DULL_HZ).ln()).clamp(0.0, 1.0);
        f.brightness += (bright - f.brightness) * (1.0 - (-dt / RELEASE_S).exp());
        self.quiet_for = if rms < SILENCE_RMS { self.quiet_for + dt } else { 0.0 };
        f.silent = self.quiet_for >= SILENCE_S;

        for (k, kind) in OnsetKind::ALL.into_iter().enumerate() {
            let e = energy(self.onset_bins[k]);
            let audible = e.sqrt() > kind.gate();
            if let Some((hop, strength)) = self.pickers[k].push(e, audible, sensitivity) {
                // `hop` is the previous hop: peaks are confirmed one hop late.
                self.onsets.push(Onset { kind, time: time - (self.hops - hop) as f32 * dt, strength });
            }
        }
        self.hops += 1;
    }
}

/// A smoothed amplitude with an automatic gain control: the output is the envelope over
/// its own slowly decaying peak.
#[derive(Clone, Debug, Default)]
struct Envelope {
    value: f32,
    peak: f32,
}

impl Envelope {
    fn update(&mut self, amplitude: f32, dt: f32) -> f32 {
        let tau = if amplitude > self.value { ATTACK_S } else { RELEASE_S };
        self.value += (amplitude - self.value) * (1.0 - (-dt / tau).exp());
        self.peak = self.value.max(self.peak * (-dt / AGC_S).exp());
        (self.value / self.peak.max(AGC_FLOOR)).clamp(0.0, 1.0)
    }
}

/// Peak picking on one band's onset function (see the module docs).
struct OnsetPicker {
    hop_seconds: f32,
    hops: usize,
    prev_log: Option<f32>,
    /// Onset values over the threshold window, newest last.
    recent: VecDeque<f32>,
    /// Onset values over `HISTORY_S`, for the floor and the strength scale.
    history: VecDeque<f32>,
    /// The previous hop's value and gate state, awaiting confirmation as a peak.
    pending: Option<(f32, bool)>,
    last_hit: Option<usize>,
}

impl OnsetPicker {
    fn new(hop_seconds: f32) -> Self {
        Self {
            hop_seconds,
            hops: 0,
            prev_log: None,
            recent: VecDeque::new(),
            history: VecDeque::new(),
            pending: None,
            last_hit: None,
        }
    }

    /// Push this hop's band energy. Returns (hop index, strength) for a confirmed hit at
    /// the previous hop.
    fn push(&mut self, energy: f32, audible: bool, sensitivity: f32) -> Option<(usize, f32)> {
        let log = (energy + 1e-9).ln();
        let value = self.prev_log.map_or(0.0, |prev| (log - prev).max(0.0));
        self.prev_log = Some(log);
        let context = ((CONTEXT_S / self.hop_seconds) as usize).max(1);
        let history_len = ((HISTORY_S / self.hop_seconds) as usize).max(1);
        push_capped(&mut self.recent, value, 2 * context + 1);
        push_capped(&mut self.history, value, history_len);

        let mut hit = None;
        // The gate passes if either the candidate hop or the confirming one is loud
        // enough: the biggest energy rise comes when the window has only just caught the
        // hit, so in a quiet track the candidate alone can still be under the gate.
        if let Some((candidate, was_audible)) = self.pending.replace((value, audible))
            && (was_audible || audible)
            && candidate > 0.0
            && candidate >= value
        {
            let k = self.hops - 1;
            let before = self.recent.iter().rev().nth(2).copied().unwrap_or(0.0);
            let window: Vec<f32> = self.recent.iter().take(self.recent.len() - 1).copied().collect();
            let mean = window.iter().sum::<f32>() / window.len() as f32;
            let var = window.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / window.len() as f32;
            let mut nonzero: Vec<f32> = self.history.iter().copied().filter(|&x| x > 0.0).collect();
            nonzero.sort_by(f32::total_cmp);
            let strong = nonzero.get((nonzero.len() as f32 * 0.98) as usize).copied().unwrap_or(candidate);
            let min_gap = ((MIN_GAP_S / self.hop_seconds) as usize).max(1);
            let spaced = self.last_hit.is_none_or(|last| k - last >= min_gap);
            if candidate >= before
                && candidate >= MIN_RISE
                && candidate > mean + sensitivity * var.sqrt()
                && candidate >= FLOOR_FRACTION * strong
                && spaced
            {
                let scale = nonzero.last().copied().unwrap_or(candidate).max(1e-6);
                hit = Some((k, (candidate / scale).min(1.0)));
                self.last_hit = Some(k);
            }
        }
        self.hops += 1;
        hit
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
    use std::f32::consts::PI;

    /// A synthetic beat: kick drums (pitch-swept sine, 150 -> 50 Hz, fast decay) on every
    /// beat, hi-hats (bursts of high-passed noise) on the off-beats, and a quiet noise bed,
    /// scaled by `gain`. Returns the samples, the kick times and the hi-hat times.
    pub fn synthetic_beat(sample_rate: u32, seconds: f32, bpm: f32, gain: f32) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
        let rate = sample_rate as f32;
        let n = (seconds * rate) as usize;
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
        let (mut kicks, mut hats) = (Vec::new(), Vec::new());
        let mut t = 0.25;
        while t + beat < seconds {
            kicks.push(t);
            let start = (t * rate) as usize;
            let mut phase = 0.0f32;
            for k in 0..(0.25 * rate) as usize {
                let tt = k as f32 / rate;
                let freq = 50.0 + 100.0 * (-tt * 30.0).exp();
                phase += 2.0 * PI * freq / rate;
                out[start + k] += 0.8 * phase.sin() * (-tt * 12.0).exp();
            }
            let hat_t = t + beat / 2.0;
            hats.push(hat_t);
            let hat = (hat_t * rate) as usize;
            let mut prev = 0.0;
            for k in 0..(0.05 * rate) as usize {
                let white = noise();
                out[hat + k] += 0.4 * (white - prev) * (-(k as f32) / (0.01 * rate)).exp();
                prev = white;
            }
            t += beat;
        }
        out.iter_mut().for_each(|s| *s *= gain);
        (out, kicks, hats)
    }

    fn analyse(signal: &[f32], rate: u32, chunk: usize) -> (Analyzer, Vec<Onset>) {
        let mut a = Analyzer::new(rate);
        let mut onsets = Vec::new();
        for part in signal.chunks(chunk) {
            a.process(part, 1.5);
            onsets.extend(a.take_onsets());
        }
        (a, onsets)
    }

    fn times(onsets: &[Onset], kind: OnsetKind) -> Vec<f32> {
        onsets.iter().filter(|o| o.kind == kind).map(|o| o.time).collect()
    }

    /// Every kick after a second of warm-up is found, with no false kicks, and the
    /// hi-hats show up as hat onsets on the off-beats, not as kicks.
    #[test]
    fn finds_kicks_and_hats() {
        let rate = 48_000;
        let (signal, kicks, hats) = synthetic_beat(rate, 10.0, 120.0, 1.0);
        let (_, onsets) = analyse(&signal, rate, 480);
        let found_kicks = times(&onsets, OnsetKind::Kick);
        let tolerance = 0.03;
        for t in kicks.iter().filter(|&&t| t > 1.0) {
            assert!(found_kicks.iter().any(|k| (k - t).abs() < tolerance), "missed the kick at {t:.3}; found {found_kicks:?}");
        }
        for k in &found_kicks {
            assert!(kicks.iter().any(|t| (k - t).abs() < tolerance), "false kick at {k:.3}");
        }
        let found_hats = times(&onsets, OnsetKind::Hat);
        let hit = hats.iter().filter(|&&t| t > 1.0).filter(|t| found_hats.iter().any(|h| (h - *t).abs() < tolerance)).count();
        let expected = hats.iter().filter(|&&t| t > 1.0).count();
        assert!(hit * 10 >= expected * 9, "found {hit} of {expected} hats: {found_hats:?}");
        assert!(onsets.iter().all(|o| (0.0..=1.0).contains(&o.strength)));
    }

    /// Results don't depend on how the audio is chunked.
    #[test]
    fn chunking_does_not_change_the_result() {
        let rate = 44_100;
        let (signal, _, _) = synthetic_beat(rate, 4.0, 120.0, 1.0);
        let (a, onsets) = analyse(&signal, rate, 441);
        let (b, onsets_b) = analyse(&signal, rate, 4096);
        assert_eq!(onsets, onsets_b);
        assert_eq!(a.features().bands, b.features().bands);
    }

    /// The AGC makes a beat at a tenth of the volume look the same: the same kicks, and
    /// band levels within a few percent.
    #[test]
    fn quiet_and_loud_tracks_look_alike() {
        let rate = 48_000;
        let (loud, _, _) = synthetic_beat(rate, 6.0, 120.0, 1.0);
        let (quiet, _, _) = synthetic_beat(rate, 6.0, 120.0, 0.1);
        let (a, onsets_loud) = analyse(&loud, rate, 1024);
        let (b, onsets_quiet) = analyse(&quiet, rate, 1024);
        assert_eq!(times(&onsets_loud, OnsetKind::Kick).len(), times(&onsets_quiet, OnsetKind::Kick).len());
        for (x, y) in a.features().bands.iter().zip(b.features().bands) {
            assert!((x - y).abs() < 0.05, "loud {:?} vs quiet {:?}", a.features().bands, b.features().bands);
        }
    }

    /// A low tone lights the bass band and reads dull; a high one lights the high band and
    /// reads bright.
    #[test]
    fn tones_land_in_their_bands() {
        let rate = 48_000;
        let tone = |hz: f32| (0..rate).map(|k| 0.3 * (2.0 * PI * hz * k as f32 / rate as f32).sin()).collect::<Vec<f32>>();
        let (low, _) = analyse(&tone(100.0), rate, 1000);
        let (high, _) = analyse(&tone(5000.0), rate, 1000);
        let raw = |a: &Analyzer, b: usize| a.band_env[b].value;
        assert!(raw(&low, 1) > 10.0 * raw(&low, 4), "100 Hz: bass {} vs high {}", raw(&low, 1), raw(&low, 4));
        assert!(raw(&high, 4) > 10.0 * raw(&high, 1), "5 kHz: high {} vs bass {}", raw(&high, 4), raw(&high, 1));
        assert!(low.features().brightness < 0.2 && high.features().brightness > 0.8);
        assert!((low.features().rms - 0.3 / 2f32.sqrt()).abs() < 0.02, "rms {}", low.features().rms);
    }

    /// Steady noise at the beat's noise-bed level (hiss, an ambient intro) fires no hits.
    /// (Generated continuously: a looped clip has a click at every seam, which the
    /// detectors rightly report.)
    #[test]
    fn a_quiet_noise_bed_fires_nothing() {
        let rate = 48_000;
        let mut seed = 0x9E37_79B9u32;
        let bed: Vec<f32> = (0..5 * rate)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                0.02 * (seed as f32 / u32::MAX as f32 * 2.0 - 1.0)
            })
            .collect();
        let (_, onsets) = analyse(&bed, rate, 1000);
        assert!(onsets.is_empty(), "{} hits in a noise bed: {onsets:?}", onsets.len());
    }

    /// Silence: no onsets, flagged silent, bands at zero.
    #[test]
    fn silence_is_silent() {
        let (a, onsets) = analyse(&vec![0.0; 48_000], 48_000, 960);
        assert!(onsets.is_empty());
        assert!(a.features().silent);
        assert!(a.features().bands.iter().all(|&b| b == 0.0));
    }
}
