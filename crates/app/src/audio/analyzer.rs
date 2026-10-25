//! Streaming audio analysis: one pipeline for live capture and for track playback.
//!
//! The input is stereo frames. Every `HOP` frames a Hann-windowed `FFT_SIZE` real FFT of
//! the mono mix gives the power spectrum, from which:
//! - **Bands:** energy in five display bands (sub to high), as smoothed amplitude
//!   envelopes normalised by an automatic gain control (each band's own slowly decaying
//!   peak), so quiet and loud tracks both span 0..1.
//! - **Level:** the same for overall loudness.
//! - **Spectrum:** 64 log-spaced triangular bands from 20 Hz to 16 kHz, tilted up 3 dB
//!   per octave (music's energy falls about that fast) and shown over the 60 dB below
//!   their shared recent peak, for spectrum displays.
//! - **MilkDrop's levels:** bass, mid and treble each over its own running average, so
//!   1 is typical for the song, and their smoothed `_att` versions.
//! - **Onsets:** kick, snare and hi-hat hits from three detection bands. The positive
//!   frame-to-frame rise in log energy, picked against a trailing adaptive threshold
//!   (mean + sensitivity × std over about a second), a floor from the last 10 s of
//!   onsets (their 98th percentile, so one huge hit doesn't raise it), a minimum gap, a
//!   loudness gate for silence, and a minimum rise. Peaks are confirmed one hop late.
//!   The minimum rise matters for wide bands: in steady hiss the adaptive threshold
//!   alone always finds some "unusual" random wobble (measured: about 10 snare and hat
//!   hits a second in noise), but those wobbles stay under 1.3x per hop, while a real
//!   hit jumps several-fold. A hit's strength is relative to a decaying maximum (a few
//!   seconds' fall), so one huge hit doesn't make the next ones read weak for long.
//! - **Brightness:** the spectral centroid on a log scale, 0 (dull) to 1 (bright).
//! - **Stereo:** balance and width, from the channels' levels and correlation.
//! - **Silence:** quiet for half a second.
//!
//! The last `WAVEFORM_LEN` frames are kept for waveform displays. Nothing allocates per
//! hop: every buffer is made once.
//!
//! Times are seconds since the analyzer started, at the centre of each analysis window
//! (where a hit's energy rise is detected).

use realfft::num_complex::Complex32;
use realfft::{RealFftPlanner, RealToComplex};
use std::collections::VecDeque;
use std::sync::Arc;

pub const FFT_SIZE: usize = 2048;
pub const HOP: usize = 512;
pub const BANDS: usize = 5;
pub const BAND_NAMES: [&str; BANDS] = ["sub", "bass", "low-mid", "mid", "high"];
/// Display band edges, Hz.
const BAND_EDGES: [f32; BANDS + 1] = [20.0, 60.0, 250.0, 500.0, 2000.0, 16000.0];
/// The log-spaced spectrum: bands, range (Hz), and the dB shown below its peak.
pub const SPECTRUM_BANDS: usize = 64;
const SPECTRUM_LO_HZ: f32 = 20.0;
const SPECTRUM_HI_HZ: f32 = 16000.0;
const SPECTRUM_RANGE_DB: f32 = 60.0;
/// The spectrum's tilt is 0 dB here (and +3 dB per octave above).
const TILT_HZ: f32 = 1000.0;
/// Frames kept for the waveform view.
pub const WAVEFORM_LEN: usize = 1024;
/// MilkDrop's bass, mid and treble bands, Hz; the running average's time constant, the
/// `_att` smoothing, and the cap on a level (a sudden hit after silence).
const MILK_EDGES: [f32; 4] = [20.0, 250.0, 2000.0, 16000.0];
const MILK_AVERAGE_S: f32 = 3.0;
const MILK_ATT_S: f32 = 0.25;
const MILK_MAX: f32 = 4.0;
/// Balance and width are smoothed with this time constant, seconds.
const STEREO_S: f32 = 0.2;

/// One stereo frame: left, right.
pub type Frame = [f32; 2];

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

/// MilkDrop's levels: each band over its own running average (1 is typical for this
/// song, 2 twice as loud), capped at 4, and the same smoothed (`_att`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MilkLevels {
    pub bass: f32,
    pub mid: f32,
    pub treb: f32,
    pub bass_att: f32,
    pub mid_att: f32,
    pub treb_att: f32,
}

/// The analysis as of the latest hop.
#[derive(Clone, Debug)]
pub struct AudioFeatures {
    /// Seconds of audio analysed.
    pub time: f32,
    /// Overall loudness, normalised by the AGC: 0..1.
    pub level: f32,
    /// Overall RMS amplitude of the mono mix, raw (full scale = 1).
    pub rms: f32,
    /// Band envelopes, normalised: 0..1.
    pub bands: [f32; BANDS],
    /// The log-spaced spectrum, 20 Hz to 16 kHz, 0..1: as of this hop, and smoothed for
    /// drawing (fast attack, 120 ms release).
    pub spectrum: [f32; SPECTRUM_BANDS],
    pub spectrum_smooth: [f32; SPECTRUM_BANDS],
    pub milk: MilkLevels,
    /// 0 (dull) to 1 (bright).
    pub brightness: f32,
    /// Left-right balance, -1 (left only) to 1 (right only).
    pub balance: f32,
    /// Stereo width: 0 mono, about 0.5 for unrelated channels, 1 for opposite ones.
    pub width: f32,
    pub silent: bool,
}

impl Default for AudioFeatures {
    fn default() -> Self {
        Self {
            time: 0.0,
            level: 0.0,
            rms: 0.0,
            bands: [0.0; BANDS],
            spectrum: [0.0; SPECTRUM_BANDS],
            spectrum_smooth: [0.0; SPECTRUM_BANDS],
            milk: MilkLevels::default(),
            brightness: 0.0,
            balance: 0.0,
            width: 0.0,
            silent: false,
        }
    }
}

/// Envelope attack and release, seconds.
const ATTACK_S: f32 = 0.01;
const RELEASE_S: f32 = 0.2;
const SPECTRUM_RELEASE_S: f32 = 0.12;
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
/// A hit's strength is relative to the strongest recent rise, which falls with this
/// time constant, seconds.
const STRENGTH_S: f32 = 3.0;
/// A hit must raise its band's energy by at least this factor in one hop (log: 0.4, a
/// factor of 1.5).
const MIN_RISE: f32 = 0.4;
/// The floor's percentile comes from a histogram of onset values over 0..`HIST_MAX`
/// (log rises; bigger ones count in the top bin).
const HIST_BINS: usize = 256;
const HIST_MAX: f32 = 8.0;

/// One spectrum band: its first FFT bin and the weights of the bins from there.
struct Filter {
    start: usize,
    weights: Vec<f32>,
    /// Their sum, and the tilt for the band's centre.
    total: f32,
    tilt: f32,
}

pub struct Analyzer {
    hop_seconds: f32,
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    /// The last `FFT_SIZE` mono samples, a ring (oldest at `ring_pos` once full).
    ring: Vec<f32>,
    ring_pos: usize,
    filled: usize,
    since_hop: usize,
    hops: usize,
    input: Vec<f32>,
    output: Vec<Complex32>,
    scratch: Vec<Complex32>,
    power: Vec<f32>,
    /// Bin ranges for the display, onset and MilkDrop bands.
    band_bins: [(usize, usize); BANDS],
    onset_bins: [(usize, usize); 3],
    milk_bins: [(usize, usize); 3],
    filters: Vec<Filter>,
    /// Power-spectrum normalisation: band energy in mean-square amplitude.
    power_scale: f32,
    bin_hz: f32,
    band_env: [Envelope; BANDS],
    level_env: Envelope,
    /// The spectrum's shared AGC peak (power density, tilted).
    spectrum_peak: f32,
    milk_average: [f32; 3],
    /// Sums of left², right² and left·right since the last hop.
    stereo_sums: [f32; 3],
    /// The last `WAVEFORM_LEN` frames, a ring (oldest at `wave_pos`).
    wave: Vec<Frame>,
    wave_pos: usize,
    pickers: [OnsetPicker; 3],
    quiet_for: f32,
    features: AudioFeatures,
    onsets: Vec<Onset>,
}

impl Analyzer {
    pub fn new(sample_rate: u32) -> Self {
        let rate = sample_rate as f32;
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(FFT_SIZE);
        let window: Vec<f32> =
            (0..FFT_SIZE).map(|k| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * k as f32 / FFT_SIZE as f32).cos()).collect();
        let window_power: f32 = window.iter().map(|w| w * w).sum();
        let bin_hz = rate / FFT_SIZE as f32;
        let bins = |lo: f32, hi: f32| {
            let a = ((lo / bin_hz).round() as usize).clamp(1, FFT_SIZE / 2 - 1);
            let b = ((hi / bin_hz).round() as usize).clamp(a + 1, FFT_SIZE / 2);
            (a, b)
        };
        let hop_seconds = HOP as f32 / rate;
        let (input, output, scratch) = (fft.make_input_vec(), fft.make_output_vec(), fft.make_scratch_vec());
        Self {
            hop_seconds,
            window,
            ring: vec![0.0; FFT_SIZE],
            ring_pos: 0,
            filled: 0,
            since_hop: 0,
            hops: 0,
            input,
            output,
            scratch,
            power: vec![0.0; FFT_SIZE / 2],
            band_bins: std::array::from_fn(|b| bins(BAND_EDGES[b], BAND_EDGES[b + 1])),
            onset_bins: OnsetKind::ALL.map(|k| bins(k.range().0, k.range().1)),
            milk_bins: std::array::from_fn(|b| bins(MILK_EDGES[b], MILK_EDGES[b + 1])),
            filters: filter_bank(bin_hz),
            fft,
            // One-sided spectrum: x2 for the mirrored half; Parseval with the window's power.
            power_scale: 2.0 / (FFT_SIZE as f32 * window_power),
            bin_hz,
            band_env: Default::default(),
            level_env: Envelope::default(),
            spectrum_peak: 0.0,
            milk_average: [0.0; 3],
            stereo_sums: [0.0; 3],
            wave: vec![[0.0; 2]; WAVEFORM_LEN],
            wave_pos: 0,
            pickers: std::array::from_fn(|_| OnsetPicker::new(hop_seconds)),
            quiet_for: 0.0,
            features: AudioFeatures { silent: true, ..AudioFeatures::default() },
            onsets: Vec::with_capacity(16),
        }
    }

    pub fn features(&self) -> &AudioFeatures {
        &self.features
    }

    /// Move the onsets found since the last call into `out`.
    pub fn drain_onsets(&mut self, out: &mut Vec<Onset>) {
        out.append(&mut self.onsets);
    }

    /// The last `WAVEFORM_LEN` frames, oldest first, into `out` (replacing its contents).
    pub fn waveform(&self, out: &mut Vec<Frame>) {
        out.clear();
        out.extend_from_slice(&self.wave[self.wave_pos..]);
        out.extend_from_slice(&self.wave[..self.wave_pos]);
    }

    /// Feed stereo frames, any number at a time. `sensitivity` is the onset threshold in
    /// standard deviations (higher: only the clearest hits).
    pub fn process(&mut self, frames: &[Frame], sensitivity: f32) {
        for &frame in frames {
            self.push(frame, sensitivity);
        }
    }

    /// Feed mono samples (the same signal on both sides): for tests' synthetic signals.
    #[cfg(test)]
    pub fn process_mono(&mut self, mono: &[f32], sensitivity: f32) {
        for &x in mono {
            self.push([x, x], sensitivity);
        }
    }

    fn push(&mut self, [left, right]: Frame, sensitivity: f32) {
        self.wave[self.wave_pos] = [left, right];
        self.wave_pos = (self.wave_pos + 1) % WAVEFORM_LEN;
        self.stereo_sums[0] += left * left;
        self.stereo_sums[1] += right * right;
        self.stereo_sums[2] += left * right;
        self.ring[self.ring_pos] = 0.5 * (left + right);
        self.ring_pos = (self.ring_pos + 1) % FFT_SIZE;
        self.filled = (self.filled + 1).min(FFT_SIZE);
        self.since_hop += 1;
        if self.filled == FFT_SIZE && self.since_hop >= HOP {
            self.since_hop = 0;
            self.hop(sensitivity);
        }
    }

    fn hop(&mut self, sensitivity: f32) {
        for (k, (out, &w)) in self.input.iter_mut().zip(&self.window).enumerate() {
            *out = self.ring[(self.ring_pos + k) % FFT_SIZE] * w;
        }
        self.fft.process_with_scratch(&mut self.input, &mut self.output, &mut self.scratch).expect("buffers made by the plan");
        for (p, c) in self.power.iter_mut().zip(&self.output) {
            *p = c.norm_sqr() * self.power_scale;
        }
        let power = &self.power;
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
        f.brightness += (bright - f.brightness) * smoothing(dt, RELEASE_S);
        self.quiet_for = if rms < SILENCE_RMS { self.quiet_for + dt } else { 0.0 };
        f.silent = self.quiet_for >= SILENCE_S;

        // The spectrum: each band's tilted power density, in dB below the shared peak.
        let mut loudest = 0.0f32;
        for (s, filter) in f.spectrum.iter_mut().zip(&self.filters) {
            let e: f32 = filter.weights.iter().zip(&power[filter.start..]).map(|(w, p)| w * p).sum();
            let density = e / filter.total * filter.tilt;
            loudest = loudest.max(density);
            *s = density;
        }
        self.spectrum_peak = loudest.max(self.spectrum_peak * (-dt / AGC_S).exp());
        let peak_db = 10.0 * self.spectrum_peak.max(1e-12).log10();
        let release = smoothing(dt, SPECTRUM_RELEASE_S);
        for (s, smooth) in f.spectrum.iter_mut().zip(&mut f.spectrum_smooth) {
            let db = 10.0 * s.max(1e-12).log10();
            *s = ((db - peak_db + SPECTRUM_RANGE_DB) / SPECTRUM_RANGE_DB).clamp(0.0, 1.0);
            *smooth = if *s > *smooth { *s } else { *smooth + (*s - *smooth) * release };
        }

        // MilkDrop's levels: each band's amplitude over its running average.
        let mut levels = [0.0f32; 3];
        for (j, level) in levels.iter_mut().enumerate() {
            let amplitude = energy(self.milk_bins[j]).sqrt();
            let average = &mut self.milk_average[j];
            *average = if *average == 0.0 { amplitude } else { *average + (amplitude - *average) * smoothing(dt, MILK_AVERAGE_S) };
            *level = (amplitude / average.max(AGC_FLOOR * 0.1)).min(MILK_MAX);
        }
        let att = smoothing(dt, MILK_ATT_S);
        let m = &mut f.milk;
        (m.bass, m.mid, m.treb) = (levels[0], levels[1], levels[2]);
        m.bass_att += (m.bass - m.bass_att) * att;
        m.mid_att += (m.mid - m.mid_att) * att;
        m.treb_att += (m.treb - m.treb_att) * att;

        // Stereo, from this hop's frames (skipped in silence, where it's undefined).
        let [ll, rr, lr] = std::mem::take(&mut self.stereo_sums);
        if ll + rr > 1e-9 {
            let (l, r) = (ll.sqrt(), rr.sqrt());
            let balance = (r - l) / (r + l);
            let correlation = if ll * rr > 1e-18 { lr / (ll * rr).sqrt() } else { 1.0 };
            let width = (0.5 * (1.0 - correlation)).clamp(0.0, 1.0);
            let k = smoothing(dt, STEREO_S);
            f.balance += (balance - f.balance) * k;
            f.width += (width - f.width) * k;
        }

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

/// The fraction of the way an exponential smoother moves in `dt` with time constant
/// `tau`.
fn smoothing(dt: f32, tau: f32) -> f32 {
    1.0 - (-dt / tau).exp()
}

/// The spectrum's bands: triangles centred on log-spaced frequencies, each reaching
/// to its neighbours' centres, so neighbouring weights sum to 1 across the range. A
/// band narrower than a bin (at the bottom, where bins are 20-odd Hz wide) instead
/// interpolates between the two bins around its centre.
fn filter_bank(bin_hz: f32) -> Vec<Filter> {
    let ratio = (SPECTRUM_HI_HZ / SPECTRUM_LO_HZ).powf(1.0 / (SPECTRUM_BANDS - 1) as f32);
    let centre = |i: isize| SPECTRUM_LO_HZ * ratio.powi(i as i32);
    let last_bin = FFT_SIZE / 2 - 1;
    (0..SPECTRUM_BANDS as isize)
        .map(|i| {
            let (lo, c, hi) = (centre(i - 1), centre(i), centre(i + 1));
            let first = ((lo / bin_hz).floor() as usize + 1).max(1);
            let last = ((hi / bin_hz).ceil() as usize).min(last_bin);
            let weight = |k: usize| {
                let f = k as f32 * bin_hz;
                if f <= lo || f >= hi { 0.0 } else if f <= c { (f - lo) / (c - lo) } else { (hi - f) / (hi - c) }
            };
            let weights: Vec<f32> = (first..=last.max(first)).map(weight).collect();
            let total: f32 = weights.iter().sum();
            let tilt = c / TILT_HZ;
            if total >= 1.0 {
                return Filter { start: first, weights, total, tilt };
            }
            let x = (c / bin_hz).clamp(1.0, (last_bin - 1) as f32);
            let k = x.floor();
            Filter { start: k as usize, weights: vec![1.0 - (x - k), x - k], total: 1.0, tilt }
        })
        .collect()
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
        self.value += (amplitude - self.value) * smoothing(dt, tau);
        self.peak = self.value.max(self.peak * (-dt / AGC_S).exp());
        (self.value / self.peak.max(AGC_FLOOR)).clamp(0.0, 1.0)
    }
}

/// Peak picking on one band's onset function (see the module docs). Its statistics are
/// kept as it goes: running sums for the threshold window, a histogram for the 10 s
/// floor, a decaying maximum for the strength scale.
struct OnsetPicker {
    hop_seconds: f32,
    hops: usize,
    prev_log: Option<f32>,
    /// Onset values over the threshold window, newest last, and their sum and sum of
    /// squares.
    recent: VecDeque<f32>,
    recent_cap: usize,
    sums: (f64, f64),
    /// Over the last `HISTORY_S`: each hop's histogram bin (`NO_BIN` for a zero value),
    /// oldest first, and the counts per bin.
    history: VecDeque<u16>,
    history_cap: usize,
    counts: [u32; HIST_BINS],
    nonzero: u32,
    /// The strongest recent onset value, decaying.
    scale: f32,
    min_gap: usize,
    /// The previous hop's value and gate state, awaiting confirmation as a peak.
    pending: Option<(f32, bool)>,
    last_hit: Option<usize>,
}

const NO_BIN: u16 = u16::MAX;

impl OnsetPicker {
    fn new(hop_seconds: f32) -> Self {
        let recent_cap = 2 * ((CONTEXT_S / hop_seconds) as usize).max(1) + 1;
        let history_cap = ((HISTORY_S / hop_seconds) as usize).max(1);
        Self {
            hop_seconds,
            hops: 0,
            prev_log: None,
            recent: VecDeque::with_capacity(recent_cap),
            recent_cap,
            sums: (0.0, 0.0),
            history: VecDeque::with_capacity(history_cap),
            history_cap,
            counts: [0; HIST_BINS],
            nonzero: 0,
            scale: 0.0,
            min_gap: ((MIN_GAP_S / hop_seconds) as usize).max(1),
            pending: None,
            last_hit: None,
        }
    }

    fn remember(&mut self, value: f32) {
        if self.recent.len() == self.recent_cap
            && let Some(old) = self.recent.pop_front()
        {
            self.sums.0 -= f64::from(old);
            self.sums.1 -= f64::from(old) * f64::from(old);
        }
        self.recent.push_back(value);
        self.sums.0 += f64::from(value);
        self.sums.1 += f64::from(value) * f64::from(value);

        if self.history.len() == self.history_cap
            && let Some(old) = self.history.pop_front()
            && old != NO_BIN
        {
            self.counts[usize::from(old)] -= 1;
            self.nonzero -= 1;
        }
        let bin = if value > 0.0 { ((value / HIST_MAX * HIST_BINS as f32) as usize).min(HIST_BINS - 1) as u16 } else { NO_BIN };
        if bin != NO_BIN {
            self.counts[usize::from(bin)] += 1;
            self.nonzero += 1;
        }
        self.history.push_back(bin);
        self.scale = value.max(self.scale * (-self.hop_seconds / STRENGTH_S).exp());
    }

    /// The `p`th quantile of the nonzero values over the history (a bin's centre), or
    /// `None` if there are none.
    fn quantile(&self, p: f32) -> Option<f32> {
        let rank = (self.nonzero as f32 * p) as u32;
        let mut seen = 0;
        for (bin, &count) in self.counts.iter().enumerate() {
            seen += count;
            if seen > rank {
                return Some((bin as f32 + 0.5) * HIST_MAX / HIST_BINS as f32);
            }
        }
        None
    }

    /// The threshold window (the recent values but the newest): mean and standard
    /// deviation.
    fn window_stats(&self) -> (f32, f32) {
        let n = (self.recent.len() - 1).max(1) as f64;
        let newest = f64::from(*self.recent.back().unwrap_or(&0.0));
        let mean = (self.sums.0 - newest) / n;
        let var = ((self.sums.1 - newest * newest) / n - mean * mean).max(0.0);
        (mean as f32, var.sqrt() as f32)
    }

    /// Push this hop's band energy. Returns (hop index, strength) for a confirmed hit at
    /// the previous hop.
    fn push(&mut self, energy: f32, audible: bool, sensitivity: f32) -> Option<(usize, f32)> {
        let log = (energy + 1e-9).ln();
        let value = self.prev_log.map_or(0.0, |prev| (log - prev).max(0.0));
        self.prev_log = Some(log);
        self.remember(value);

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
            let before = if self.recent.len() >= 3 { self.recent[self.recent.len() - 3] } else { 0.0 };
            let (mean, std) = self.window_stats();
            let strong = self.quantile(0.98).unwrap_or(candidate);
            let spaced = self.last_hit.is_none_or(|last| k - last >= self.min_gap);
            if candidate >= before
                && candidate >= MIN_RISE
                && candidate > mean + sensitivity * std
                && candidate >= FLOOR_FRACTION * strong
                && spaced
            {
                hit = Some((k, (candidate / self.scale.max(1e-6)).min(1.0)));
                self.last_hit = Some(k);
            }
        }
        self.hops += 1;
        hit
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use utils::rng::SplitMix64;
    use std::f32::consts::PI;

    /// A synthetic beat: kick drums (pitch-swept sine, 150 -> 50 Hz, fast decay) on every
    /// beat, hi-hats (bursts of high-passed noise) on the off-beats, and a quiet noise bed,
    /// scaled by `gain`. Returns the samples, the kick times and the hi-hat times.
    pub fn synthetic_beat(sample_rate: u32, seconds: f32, bpm: f32, gain: f32) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
        let rate = sample_rate as f32;
        let n = (seconds * rate) as usize;
        let beat = 60.0 / bpm;
        let mut out = vec![0.0f32; n];
        let mut rng = SplitMix64::new(0x1234_5678);
        for s in out.iter_mut() {
            *s = 0.02 * rng.signed();
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
                let white = rng.signed();
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
            a.process_mono(part, 1.5);
            a.drain_onsets(&mut onsets);
        }
        (a, onsets)
    }

    fn tone(rate: u32, hz: f32, seconds: f32, amplitude: f32) -> Vec<f32> {
        (0..(seconds * rate as f32) as usize).map(|k| amplitude * (2.0 * PI * hz * k as f32 / rate as f32).sin()).collect()
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
        let mut rng = SplitMix64::new(0x9E37_79B9);
        let bed: Vec<f32> = (0..5 * rate).map(|_| 0.02 * rng.signed()).collect();
        let (_, onsets) = analyse(&bed, rate, 1000);
        assert!(onsets.is_empty(), "{} hits in a noise bed: {onsets:?}", onsets.len());
    }

    /// The spectrum's bands cover the range: neighbouring triangles' weights sum to 1
    /// between the first and last centres, and every band has some weight.
    #[test]
    fn the_spectrum_bands_tile_the_range() {
        let bin_hz = 48_000.0 / FFT_SIZE as f32;
        let filters = filter_bank(bin_hz);
        assert_eq!(filters.len(), SPECTRUM_BANDS);
        assert!(filters.iter().all(|f| f.total > 0.0 && f.weights.iter().all(|w| (0.0..=1.0).contains(w))));
        let mut sum = vec![0.0f32; FFT_SIZE / 2];
        for f in filters.iter().filter(|f| f.total > 1.0) {
            for (k, w) in f.weights.iter().enumerate() {
                sum[f.start + k] += w;
            }
        }
        // From where the bands are wider than a bin (a few hundred Hz) to the top centre.
        for (k, s) in sum.iter().enumerate().filter(|(k, _)| (500.0..15_000.0).contains(&(*k as f32 * bin_hz))) {
            assert!((s - 1.0).abs() < 1e-4, "weights at {:.0} Hz sum to {s}", k as f32 * bin_hz);
        }
    }

    /// A tone lights the spectrum at its own frequency, low or high.
    #[test]
    fn tones_peak_in_the_spectrum_where_they_are() {
        let rate = 48_000;
        let ratio = (SPECTRUM_HI_HZ / SPECTRUM_LO_HZ).powf(1.0 / (SPECTRUM_BANDS - 1) as f32);
        for hz in [80.0, 1000.0, 6000.0] {
            let (a, _) = analyse(&tone(rate, hz, 1.0, 0.3), rate, 1000);
            let s = &a.features().spectrum;
            let peak = (0..SPECTRUM_BANDS).max_by(|&i, &j| s[i].total_cmp(&s[j])).unwrap();
            let expected = ((hz / SPECTRUM_LO_HZ).ln() / ratio.ln()).round() as usize;
            assert!(peak.abs_diff(expected) <= 1, "{hz} Hz peaked in band {peak}, expected {expected}");
            assert_eq!(s[peak], 1.0, "the loudest band sits at the top of the range");
        }
    }

    /// The waveform view returns the last frames, oldest first.
    #[test]
    fn the_waveform_is_the_latest_frames_in_order() {
        let mut a = Analyzer::new(48_000);
        let frames: Vec<Frame> = (0..WAVEFORM_LEN + 100).map(|k| [k as f32, -(k as f32)]).collect();
        a.process(&frames, 1.5);
        let mut out = Vec::new();
        a.waveform(&mut out);
        assert_eq!(out.len(), WAVEFORM_LEN);
        assert_eq!(out[0], [100.0, -100.0]);
        assert_eq!(out[WAVEFORM_LEN - 1], [(WAVEFORM_LEN + 99) as f32, -((WAVEFORM_LEN + 99) as f32)]);
    }

    /// Balance follows the louder side; width is 0 for identical channels and near 1 for
    /// opposite ones.
    #[test]
    fn stereo_balance_and_width() {
        let rate = 48_000;
        let signal = tone(rate, 440.0, 1.0, 0.3);
        let run = |frame: &dyn Fn(f32) -> Frame| {
            let mut a = Analyzer::new(rate);
            a.process(&signal.iter().map(|&x| frame(x)).collect::<Vec<_>>(), 1.5);
            (a.features().balance, a.features().width)
        };
        let (balance, _) = run(&|x| [x, 0.0]);
        assert!(balance < -0.95, "left only: balance {balance}");
        let (balance, width) = run(&|x| [x, x]);
        assert!(balance.abs() < 0.01 && width < 0.01, "mono: balance {balance}, width {width}");
        let (_, width) = run(&|x| [x, -x]);
        assert!(width > 0.95, "opposite channels: width {width}");
    }

    /// MilkDrop's bass is about 1 on a steady signal and jumps when it gets louder.
    #[test]
    fn milk_levels_are_relative_to_the_song() {
        let rate = 48_000;
        let mut a = Analyzer::new(rate);
        a.process_mono(&tone(rate, 100.0, 8.0, 0.1), 1.5);
        let steady = a.features().milk;
        assert!((steady.bass - 1.0).abs() < 0.05 && (steady.bass_att - 1.0).abs() < 0.1, "steady: {steady:?}");
        a.process_mono(&tone(rate, 100.0, 0.3, 0.4), 1.5);
        assert!(a.features().milk.bass > 2.0, "louder: {:?}", a.features().milk);
    }

    /// The picker's running statistics match a direct computation.
    #[test]
    fn running_statistics_match_a_direct_computation() {
        let mut p = OnsetPicker::new(HOP as f32 / 48_000.0);
        let mut rng = SplitMix64::new(42);
        let mut all = Vec::new();
        for _ in 0..3000 {
            let e = (1.0 + 3.0 * rng.next_f32()).exp();
            p.push(e, true, 1.5);
            all.push(*p.recent.back().unwrap());
        }
        let window: Vec<f32> = p.recent.iter().take(p.recent.len() - 1).copied().collect();
        let mean = window.iter().sum::<f32>() / window.len() as f32;
        let std = (window.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / window.len() as f32).sqrt();
        let (m, s) = p.window_stats();
        assert!((m - mean).abs() < 1e-4 && (s - std).abs() < 1e-4, "running ({m}, {s}) vs direct ({mean}, {std})");
        let mut nonzero: Vec<f32> = all[all.len() - p.history_cap..].iter().copied().filter(|&x| x > 0.0).collect();
        nonzero.sort_by(f32::total_cmp);
        let exact = nonzero[(nonzero.len() as f32 * 0.98) as usize];
        let approx = p.quantile(0.98).unwrap();
        assert!((approx - exact).abs() <= HIST_MAX / HIST_BINS as f32, "98th percentile {approx} vs {exact}");
    }

    /// After warming up, analysing allocates nothing.
    #[test]
    fn analysing_allocates_nothing() {
        let rate = 48_000;
        let (signal, _, _) = synthetic_beat(rate, 4.0, 120.0, 1.0);
        let frames: Vec<Frame> = signal.iter().map(|&x| [x, 0.5 * x]).collect();
        let (warm, rest) = frames.split_at(2 * rate as usize);
        let mut a = Analyzer::new(rate);
        let mut onsets = Vec::with_capacity(256);
        a.process(warm, 1.5);
        a.drain_onsets(&mut onsets);
        onsets.clear();
        let count = crate::test_alloc::allocations(|| {
            for chunk in rest.chunks(800) {
                a.process(chunk, 1.5);
                a.drain_onsets(&mut onsets);
            }
        });
        assert_eq!(count, 0, "allocated {count} times");
        assert!(!onsets.is_empty(), "the beat should still be heard");
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
