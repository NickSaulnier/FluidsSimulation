//! Streaming audio analysis: one pipeline for live capture and for track playback.
//!
//! The input is stereo frames. Every `HOP` frames a Hann-windowed `FFT_SIZE` real FFT of
//! the mono mix gives the power spectrum, and from it a filter bank: 64 log-spaced
//! triangular bands from 20 Hz to 16 kHz. Below 300 Hz the short FFT's bins are wider
//! than the bands, so there the bands take their shape from a `LONG_FFT_SIZE` FFT (about
//! 6 Hz bins, run every few hops) and their level from the short one, which keeps them
//! as quick as the rest. From the bank:
//! - **Bands:** energy in five display bands (sub to high), as smoothed amplitude
//!   envelopes normalised by an automatic gain control (each band's own slowly decaying
//!   peak), so quiet and loud tracks both span 0..1.
//! - **Level:** the same for overall loudness as heard: the power K-weighted (EBU R128's
//!   curve, applied per bin), so a bass-heavy and a bright mix of the same RMS differ as
//!   they sound.
//! - **Loudness:** EBU R128 momentary and short-term loudness in LUFS (`ebur128`).
//! - **Spectrum:** the bank, tilted up 3 dB per octave (music's energy falls about that
//!   fast) and shown over the 60 dB below its recent peak, for spectrum displays.
//! - **MilkDrop's levels:** bass, mid and treble each over its own running average, so
//!   1 is typical for the song, and their smoothed `_att` versions.
//! - **Onsets:** kick, snare and hi-hat hits. Per kind, SuperFlux (Böck and Widmer,
//!   2013) over the bands of its range: each band's rise in compressed log energy over
//!   the maximum of it and its neighbours a hop earlier, so pitch wobble doesn't count;
//!   the kick takes the median over its bands (a kick raises them all, a bass note a
//!   few), the others the mean. What the other kinds are known to spill into a kind's
//!   range is taken off (a kick's click lifts the snare's range, a snare's top the
//!   hat's), with the spill learned from each kind's own hits. The result is picked
//!   against a trailing adaptive threshold (mean + sensitivity × std over about a
//!   second), a floor from the last 10 s of onsets (their 98th percentile, so one huge
//!   hit doesn't raise it), a minimum gap, a loudness gate for silence, and a minimum
//!   rise. Peaks are confirmed one hop late. The minimum rise matters for wide bands: in
//!   steady hiss the adaptive threshold alone always finds some "unusual" random wobble
//!   (measured: about 10 snare and hat hits a second in noise), but those wobbles stay
//!   small, while a real hit jumps several-fold. A hit's strength is relative to a
//!   decaying maximum (a few seconds' fall), so one huge hit doesn't make the next ones
//!   read weak for long. Hits are timed from the short FFT alone (the long one lags).
//! - **Brightness:** the spectral centroid on a log scale, 0 (dull) to 1 (bright).
//! - **Stereo:** balance and width, from the channels' levels and correlation.
//! - **Silence:** momentary loudness under -50 LUFS for half a second.
//!
//! The last `WAVEFORM_LEN` frames are kept for waveform displays. Nothing allocates per
//! hop: every buffer is made once.
//!
//! Times are seconds since the analyzer started, at the centre of each analysis window
//! (where a hit's energy rise is detected).

use super::dynamics::Dynamics;
use super::sections::{self, SectionTracker, Sections};
use super::tempo::{Beat, BeatTracker, MusicalTime};
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
/// The long FFT for the low bands (about 6 Hz bins), run every `LONG_EVERY` hops. The
/// filter bank's bands centred below `LOW_SPLIT_HZ` take their shape from it and their
/// level from the short FFT, so they're sharp in frequency without its latency.
pub const LONG_FFT_SIZE: usize = 8192;
const LONG_EVERY: usize = 4;
const LOW_SPLIT_HZ: f32 = 300.0;
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

    /// Detection band, Hz. The kick's stops at 120 Hz, under a snare's body.
    fn range(self) -> (f32, f32) {
        match self {
            OnsetKind::Kick => (30.0, 120.0),
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
    /// Overall loudness as heard (K-weighted, as EBU R128 weighs it, but over this hop
    /// alone), normalised by the AGC: 0..1.
    pub level: f32,
    /// Overall RMS amplitude of the mono mix, raw (full scale = 1).
    pub rms: f32,
    /// EBU R128 loudness, LUFS: momentary (the last 400 ms, updated every 100 ms) and
    /// short-term (3 s, every 500 ms); `LUFS_FLOOR` for silence.
    pub loudness: f32,
    pub loudness_short: f32,
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
    /// Tempo, beats and bars (see `tempo`).
    pub musical: MusicalTime,
    /// Section changes and build-ups (see `sections`).
    pub sections: Sections,
    /// The song's energy: how loud it is now against its own range, with its build-ups,
    /// drops and breakdowns, 0..1 (see `dynamics`).
    pub energy: f32,
}

impl Default for AudioFeatures {
    fn default() -> Self {
        Self {
            time: 0.0,
            level: 0.0,
            rms: 0.0,
            loudness: LUFS_FLOOR,
            loudness_short: LUFS_FLOOR,
            bands: [0.0; BANDS],
            spectrum: [0.0; SPECTRUM_BANDS],
            spectrum_smooth: [0.0; SPECTRUM_BANDS],
            milk: MilkLevels::default(),
            brightness: 0.0,
            balance: 0.0,
            width: 0.0,
            silent: false,
            musical: MusicalTime::default(),
            sections: Sections::default(),
            energy: 0.0,
        }
    }
}

/// Envelope attack and release, seconds.
const ATTACK_S: f32 = 0.01;
const RELEASE_S: f32 = 0.2;
const SPECTRUM_RELEASE_S: f32 = 0.12;
/// The AGC's peak decays with this time constant, seconds.
const AGC_S: f32 = 10.0;
/// Amplitude treated as the quietest worth normalising (about -66 dBFS), so silence
/// isn't amplified into noise.
const AGC_FLOOR: f32 = 5e-4;
/// Momentary loudness below this for `SILENCE_S` counts as silence.
const SILENCE_LUFS: f32 = -50.0;
const SILENCE_S: f32 = 0.5;
/// Loudness is read this often (EBU R128 meters update at 10 Hz), the short-term less
/// often (each reading sums its whole window: 3 s of samples, the analyzer's biggest
/// cost at 10 Hz), and reported no lower than `LUFS_FLOOR`.
const LOUDNESS_EVERY_S: f32 = 0.1;
const SHORT_TERM_EVERY: usize = 5;
pub const LUFS_FLOOR: f32 = -70.0;
/// Brightness maps the centroid from this (dull) to that (bright), log scale, Hz.
const DULL_HZ: f32 = 200.0;
const BRIGHT_HZ: f32 = 8000.0;

/// Onset picking (see the module docs). The minimum gap lets sixteenth notes through
/// at 170 BPM (88 ms apart).
const CONTEXT_S: f32 = 0.5;
const MIN_GAP_S: f32 = 0.07;
const FLOOR_FRACTION: f32 = 0.2;
const HISTORY_S: f32 = 10.0;
/// A hit's strength is relative to the strongest recent rise, which falls with this
/// time constant, seconds.
const STRENGTH_S: f32 = 3.0;
/// SuperFlux: the rise is measured against this many hops back, and band energies are
/// compressed as ln(1 + energy / floor), the floor this far (-40 dB) below the kind's
/// recent peak band, so quiet bands' noise barely counts whatever the track's volume.
/// (Measured: -50 dB lets a snare's leakage into the kick bands read as kicks; -30 dB
/// loses quiet kicks after a loud hit.)
const FLUX_LAG: usize = 1;
const FLUX_FLOOR: f32 = 1e-4;
/// Per kind, whether its band rises are combined by their median rather than their
/// mean. A kick raises every band of its range; a bass note raises the few its
/// harmonics fall in, which the median ignores. Snares and hats are noise, broad by
/// nature, and their quiet hits raise only part of the range, so they take the mean.
const FLUX_MEDIAN: [bool; 3] = [true, false, false];
/// Spill between kinds (see `Analyzer::spill`): how fast the learned ratio follows a
/// lower one and a higher one, and how much of the spill it predicts is taken off each
/// kind (kicks lose nothing: measured, it only cost real kicks).
const SPILL_DOWN: f32 = 0.3;
const SPILL_UP: f32 = 0.1;
const SPILL_MARGIN: [f32; 3] = [0.0, 1.5, 1.0];
/// A hit's onset value (a log rise, typically over the kind's bands) must be at least
/// this: about a factor of 1.5 in one hop.
const MIN_RISE: f32 = 0.4;
/// The floor's percentile comes from a histogram of onset values over 0..`HIST_MAX`
/// (log rises; bigger ones count in the top bin).
const HIST_BINS: usize = 256;
const HIST_MAX: f32 = 8.0;

/// One spectrum band: its first FFT bin and the weights of the bins from there.
struct Filter {
    start: usize,
    weights: Vec<f32>,
    /// Their sum.
    total: f32,
}

/// One FFT size's transform, window and buffers, with its power spectrum.
struct Transform {
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    input: Vec<f32>,
    output: Vec<Complex32>,
    scratch: Vec<Complex32>,
    /// Normalised to mean-square amplitude: a band's sum is its energy.
    power: Vec<f32>,
    power_scale: f32,
    bin_hz: f32,
    filters: Vec<Filter>,
}

impl Transform {
    fn new(size: usize, rate: f32) -> Self {
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(size);
        let window: Vec<f32> = (0..size).map(|k| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * k as f32 / size as f32).cos()).collect();
        let window_power: f32 = window.iter().map(|w| w * w).sum();
        let bin_hz = rate / size as f32;
        Self {
            input: fft.make_input_vec(),
            output: fft.make_output_vec(),
            scratch: fft.make_scratch_vec(),
            fft,
            window,
            power: vec![0.0; size / 2],
            // One-sided spectrum: x2 for the mirrored half; Parseval with the window's power.
            power_scale: 2.0 / (size as f32 * window_power),
            bin_hz,
            filters: filter_bank(bin_hz, size),
        }
    }

    /// The power spectrum of the newest `window.len()` samples of `ring` (a ring whose
    /// oldest sample is at `pos`).
    fn run(&mut self, ring: &[f32], pos: usize) {
        let start = pos + ring.len() - self.window.len();
        for (k, (out, &w)) in self.input.iter_mut().zip(&self.window).enumerate() {
            *out = ring[(start + k) % ring.len()] * w;
        }
        self.fft.process_with_scratch(&mut self.input, &mut self.output, &mut self.scratch).expect("buffers made by the plan");
        for (p, c) in self.power.iter_mut().zip(&self.output) {
            *p = c.norm_sqr() * self.power_scale;
        }
    }

    /// The bins from `lo` to `hi` Hz.
    fn bins(&self, lo: f32, hi: f32) -> (usize, usize) {
        let a = ((lo / self.bin_hz).round() as usize).clamp(1, self.power.len() - 1);
        let b = ((hi / self.bin_hz).round() as usize).clamp(a + 1, self.power.len());
        (a, b)
    }

    /// Filter `band`'s energy and width, Hz.
    fn band(&self, band: usize) -> (f32, f32) {
        let f = &self.filters[band];
        (f.weights.iter().zip(&self.power[f.start..]).map(|(w, p)| w * p).sum(), f.total * self.bin_hz)
    }
}

pub struct Analyzer {
    hop_seconds: f32,
    /// The last `LONG_FFT_SIZE` mono samples, a ring (oldest at `ring_pos` once full).
    ring: Vec<f32>,
    ring_pos: usize,
    filled: usize,
    since_hop: usize,
    hops: usize,
    short: Transform,
    long: Transform,
    /// The filter bank's bands centred below `LOW_SPLIT_HZ`, and the bins of each FFT
    /// they span (for scaling the long FFT's shape to the short one's level).
    low_bands: usize,
    low_bins: [(usize, usize); 2],
    /// The long FFT's low bands as of its last run, and their total over `low_bins`;
    /// `None` until the ring is full.
    long_low: Option<([f32; SPECTRUM_BANDS], f32)>,
    /// This hop's filter-bank energies and widths (Hz), and the energies from the short
    /// FFT alone (for timing hits: the long FFT's shape lags).
    bank: [f32; SPECTRUM_BANDS],
    short_bank: [f32; SPECTRUM_BANDS],
    widths: [f32; SPECTRUM_BANDS],
    /// Filter-bank band ranges for the display, onset and MilkDrop bands.
    band_ranges: [(usize, usize); BANDS],
    onset_ranges: [(usize, usize); 3],
    milk_ranges: [(usize, usize); 3],
    band_env: [Envelope; BANDS],
    level_env: Envelope,
    /// Per short-FFT bin, the K-weighting's power gain.
    k_weights: Vec<f32>,
    /// EBU R128 loudness: the meter, the frames since the last hop (interleaved stereo),
    /// and the hops between readings.
    meter: ebur128::EbuR128,
    meter_frames: Vec<f32>,
    loudness_every: usize,
    /// The spectrum's shared AGC peak (power density, tilted).
    spectrum_peak: f32,
    milk_average: [f32; 3],
    /// Sums of left², right² and left·right since the last hop.
    stereo_sums: [f32; 3],
    /// The last `WAVEFORM_LEN` frames, a ring (oldest at `wave_pos`).
    wave: Vec<Frame>,
    wave_pos: usize,
    pickers: [OnsetPicker; 3],
    /// SuperFlux: the compressed filter-bank logs of the last `FLUX_LAG + 1` hops (a ring,
    /// this hop's at `hops % (FLUX_LAG + 1)`), and each hit kind's decaying peak band
    /// energy, which sets its compression floor.
    flux_logs: [[f32; SPECTRUM_BANDS]; FLUX_LAG + 1],
    flux_peaks: [f32; 3],
    /// The onset strength for the beat tracker: per band, a decaying peak (each band's own
    /// compression floor) and the compressed logs of the last two hops.
    strength_peaks: [f32; SPECTRUM_BANDS],
    strength_logs: [[f32; SPECTRUM_BANDS]; 2],
    tracker: BeatTracker,
    /// The song's sections, and the filter bank's low-end bands.
    sections: SectionTracker,
    low_end: usize,
    dynamics: Dynamics,
    /// Spill: `spill[from][to]`, the share of kind `from`'s rise that shows in kind `to`'s
    /// range (a kick's click in the snare's, a snare's top in the hat's), learned at
    /// `from`'s hits as a lower envelope (so the moments both sound together count
    /// little); and the previous hop's rises, as hits are confirmed a hop late.
    spill: [[f32; 3]; 3],
    prev_rises: [f32; 3],
    quiet_for: f32,
    features: AudioFeatures,
    onsets: Vec<Onset>,
}

impl Analyzer {
    pub fn new(sample_rate: u32) -> Self {
        let rate = sample_rate as f32;
        let hop_seconds = HOP as f32 / rate;
        let short = Transform::new(FFT_SIZE, rate);
        let long = Transform::new(LONG_FFT_SIZE, rate);
        let low_bands = (0..SPECTRUM_BANDS).take_while(|&b| band_centre(b as isize) < LOW_SPLIT_HZ).count();
        let (low_lo, low_hi) = (band_centre(-1), band_centre(low_bands as isize));
        Self {
            hop_seconds,
            ring: vec![0.0; LONG_FFT_SIZE],
            ring_pos: 0,
            filled: 0,
            since_hop: 0,
            hops: 0,
            low_bands,
            low_bins: [short.bins(low_lo, low_hi), long.bins(low_lo, low_hi)],
            short,
            long,
            long_low: None,
            bank: [0.0; SPECTRUM_BANDS],
            short_bank: [0.0; SPECTRUM_BANDS],
            widths: [1.0; SPECTRUM_BANDS],
            band_ranges: std::array::from_fn(|b| bank_range(BAND_EDGES[b], BAND_EDGES[b + 1])),
            onset_ranges: OnsetKind::ALL.map(|k| bank_range(k.range().0, k.range().1)),
            milk_ranges: std::array::from_fn(|b| bank_range(MILK_EDGES[b], MILK_EDGES[b + 1])),
            band_env: Default::default(),
            level_env: Envelope::default(),
            k_weights: k_weights(rate, FFT_SIZE),
            meter: ebur128::EbuR128::new(2, sample_rate, ebur128::Mode::M | ebur128::Mode::S).expect("a stereo meter at any audio rate"),
            meter_frames: Vec::with_capacity(2 * FFT_SIZE),
            loudness_every: ((LOUDNESS_EVERY_S / hop_seconds).round() as usize).max(1),
            spectrum_peak: 0.0,
            milk_average: [0.0; 3],
            stereo_sums: [0.0; 3],
            wave: vec![[0.0; 2]; WAVEFORM_LEN],
            wave_pos: 0,
            pickers: std::array::from_fn(|_| OnsetPicker::new(hop_seconds)),
            flux_logs: [[0.0; SPECTRUM_BANDS]; FLUX_LAG + 1],
            flux_peaks: [0.0; 3],
            strength_peaks: [0.0; SPECTRUM_BANDS],
            strength_logs: [[0.0; SPECTRUM_BANDS]; 2],
            tracker: BeatTracker::new(hop_seconds),
            sections: SectionTracker::new(hop_seconds),
            low_end: (0..SPECTRUM_BANDS).take_while(|&b| band_centre(b as isize) < sections::LOW_HZ).count(),
            dynamics: Dynamics::default(),
            spill: [[0.0; 3]; 3],
            prev_rises: [0.0; 3],
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

    /// Move the beats reported since the last call into `out`.
    pub fn drain_beats(&mut self, out: &mut Vec<Beat>) {
        self.tracker.drain_beats(out);
    }

    /// Report beats and musical time this much later (seconds; negative for earlier): for
    /// audio heard after (or before) it's analysed.
    pub fn set_beat_offset(&mut self, seconds: f32) {
        self.tracker.set_offset(seconds);
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
        self.meter_frames.extend_from_slice(&[left, right]);
        self.ring[self.ring_pos] = 0.5 * (left + right);
        self.ring_pos = (self.ring_pos + 1) % LONG_FFT_SIZE;
        self.filled = (self.filled + 1).min(LONG_FFT_SIZE);
        self.since_hop += 1;
        if self.filled >= FFT_SIZE && self.since_hop >= HOP {
            self.since_hop = 0;
            self.hop(sensitivity);
        }
    }

    /// This hop's filter bank: every band from the short FFT, except that once the long
    /// FFT has run, the low bands take its shape scaled to the short FFT's level over
    /// the same range.
    fn fill_bank(&mut self) {
        for b in 0..SPECTRUM_BANDS {
            (self.bank[b], self.widths[b]) = self.short.band(b);
        }
        self.short_bank = self.bank;
        if self.filled == LONG_FFT_SIZE && self.hops.is_multiple_of(LONG_EVERY) {
            self.long.run(&self.ring, self.ring_pos);
            let mut low = [0.0; SPECTRUM_BANDS];
            for (b, e) in low.iter_mut().enumerate().take(self.low_bands) {
                *e = self.long.band(b).0;
            }
            let (a, z) = self.low_bins[1];
            self.long_low = Some((low, self.long.power[a..z].iter().sum()));
        }
        if let Some((low, long_total)) = &self.long_low {
            let (a, z) = self.low_bins[0];
            let short_total: f32 = self.short.power[a..z].iter().sum();
            let scale = if *long_total > 0.0 { short_total / long_total } else { 0.0 };
            for (b, &e) in low.iter().enumerate().take(self.low_bands) {
                self.bank[b] = e * scale;
                self.widths[b] = self.long.filters[b].total * self.long.bin_hz;
            }
        }
    }

    /// Each hit kind's SuperFlux rise this hop: per band of its range, the rise in
    /// compressed log energy over the band and its neighbours `FLUX_LAG` hops ago (the
    /// maximum of the three, so a note bending into the next band doesn't count),
    /// combined over the range (see `FLUX_MEDIAN`).
    fn superflux(&mut self, dt: f32) -> [f32; 3] {
        let n = FLUX_LAG + 1;
        let (now, then) = (self.hops % n, (self.hops + 1) % n);
        let mut values = [0.0; 3];
        for (k, &(a, z)) in self.onset_ranges.iter().enumerate() {
            let peak = self.short_bank[a..z].iter().fold(0.0f32, |m, &e| m.max(e));
            self.flux_peaks[k] = peak.max(self.flux_peaks[k] * (-dt / AGC_S).exp());
            let floor = (self.flux_peaks[k] * FLUX_FLOOR).max(1e-12);
            for b in a..z {
                self.flux_logs[now][b] = (1.0 + self.short_bank[b] / floor).ln();
            }
            if self.hops < FLUX_LAG {
                continue;
            }
            let prev = &self.flux_logs[then];
            let mut rises = [0.0f32; SPECTRUM_BANDS];
            for (r, b) in rises.iter_mut().zip(a..z) {
                let before = prev[b.saturating_sub(1)].max(prev[b]).max(prev[(b + 1).min(SPECTRUM_BANDS - 1)]);
                *r = (self.flux_logs[now][b] - before).max(0.0);
            }
            let rises = &mut rises[..z - a];
            values[k] = if FLUX_MEDIAN[k] {
                *rises.select_nth_unstable_by(rises.len() / 2, f32::total_cmp).1
            } else {
                rises.iter().sum::<f32>() / rises.len() as f32
            };
        }
        values
    }

    /// The onset strength over the whole spectrum, for the beat tracker: SuperFlux's rise
    /// per band (as for the hits), averaged over every band, each compressed against its
    /// own recent peak so the quiet treble counts as much as the loud bass.
    fn onset_strength(&mut self, dt: f32) -> f32 {
        let (now, then) = (self.hops % 2, (self.hops + 1) % 2);
        let fall = (-dt / AGC_S).exp();
        for b in 0..SPECTRUM_BANDS {
            let e = self.short_bank[b];
            self.strength_peaks[b] = e.max(self.strength_peaks[b] * fall);
            self.strength_logs[now][b] = (1.0 + e / (self.strength_peaks[b] * FLUX_FLOOR).max(1e-12)).ln();
        }
        if self.hops == 0 {
            return 0.0;
        }
        let (logs, prev) = (&self.strength_logs[now], &self.strength_logs[then]);
        let rise: f32 = (0..SPECTRUM_BANDS)
            .map(|b| {
                let before = prev[b.saturating_sub(1)].max(prev[b]).max(prev[(b + 1).min(SPECTRUM_BANDS - 1)]);
                (logs[b] - before).max(0.0)
            })
            .sum();
        rise / SPECTRUM_BANDS as f32
    }

    /// Each kind's onset value: its rise less what the other kinds' rises are known to
    /// spill into its range, so one hit doesn't read as two kinds.
    fn unspill(&self, rises: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|to| {
            let spilled: f32 = (0..3).filter(|&from| from != to).map(|from| self.spill[from][to] * rises[from]).sum();
            (rises[to] - SPILL_MARGIN[to] * spilled).max(0.0)
        })
    }

    fn hop(&mut self, sensitivity: f32) {
        self.short.run(&self.ring, self.ring_pos);
        self.fill_bank();
        let dt = self.hop_seconds;
        let rises = self.superflux(dt);
        let strength = self.onset_strength(dt);
        let time = self.hops as f32 * dt + 0.5 * FFT_SIZE as f32 * dt / HOP as f32;
        self.tracker.push(time, strength, rises[0]);
        let values = self.unspill(rises);
        let power = &self.short.power;
        let bank = &self.bank;
        let energy = |(a, z): (usize, usize)| bank[a..z].iter().sum::<f32>();

        let total: f32 = power[1..].iter().sum();
        let rms = total.sqrt();
        let f = &mut self.features;
        f.time = time;
        f.rms = rms;
        let heard: f32 = power.iter().zip(&self.k_weights).skip(1).map(|(p, w)| p * w).sum();
        f.level = self.level_env.update(heard.sqrt(), dt);
        // The meter can only fail on frames that aren't whole (they always are).
        let _ = self.meter.add_frames_f32(&self.meter_frames);
        self.meter_frames.clear();
        if self.hops.is_multiple_of(self.loudness_every) {
            let lufs = |r: Result<f64, ebur128::Error>| r.map_or(LUFS_FLOOR, |l| (l as f32).max(LUFS_FLOOR));
            f.loudness = lufs(self.meter.loudness_momentary());
            if self.hops.is_multiple_of(self.loudness_every * SHORT_TERM_EVERY) {
                f.loudness_short = lufs(self.meter.loudness_shortterm());
            }
        }
        for b in 0..BANDS {
            f.bands[b] = self.band_env[b].update(energy(self.band_ranges[b]).sqrt(), dt);
        }
        let bin_hz = self.short.bin_hz;
        let weighted: f32 = power.iter().enumerate().skip(1).map(|(k, p)| k as f32 * bin_hz * p).sum();
        let centroid = if total > 0.0 { weighted / total } else { DULL_HZ };
        let bright = ((centroid.max(1.0) / DULL_HZ).ln() / (BRIGHT_HZ / DULL_HZ).ln()).clamp(0.0, 1.0);
        f.brightness += (bright - f.brightness) * smoothing(dt, RELEASE_S);
        self.quiet_for = if f.loudness < SILENCE_LUFS { self.quiet_for + dt } else { 0.0 };
        f.silent = self.quiet_for >= SILENCE_S;

        // The spectrum: each band's tilted power density, in dB below the shared peak.
        let mut loudest = 0.0f32;
        for (b, s) in f.spectrum.iter_mut().enumerate() {
            let density = bank[b] / self.widths[b] * band_centre(b as isize) / TILT_HZ;
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
            let amplitude = energy(self.milk_ranges[j]).sqrt();
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
            let audible = energy(self.onset_ranges[k]).sqrt() > kind.gate();
            if let Some((hop, strength)) = self.pickers[k].push(values[k], audible, sensitivity) {
                learn_spill(&mut self.spill, &self.prev_rises, k);
                // `hop` is the previous hop: peaks are confirmed one hop late.
                self.onsets.push(Onset { kind, time: time - (self.hops - hop) as f32 * dt, strength });
            }
        }
        self.prev_rises = rises;
        self.features.musical = self.tracker.time();
        self.sections.push(time, &self.bank, self.low_end, heard, strength);
        self.features.sections = self.sections.state();
        let f = &self.features;
        self.features.energy = self.dynamics.update(f.loudness, f.silent, &f.sections, time, dt);
        self.hops += 1;
    }
}

/// After a hit of kind `kind` with these rises, learn how much of its rise spilled into
/// the other kinds' ranges.
fn learn_spill(spill: &mut [[f32; 3]; 3], rises: &[f32; 3], kind: usize) {
    if rises[kind] <= 0.0 {
        return;
    }
    for to in (0..3).filter(|&to| to != kind) {
        let ratio = (rises[to] / rises[kind]).min(2.0);
        let s = &mut spill[kind][to];
        *s += (ratio - *s) * if ratio < *s { SPILL_DOWN } else { SPILL_UP };
    }
}

/// The K-weighting's power gain at each bin of an FFT: the two filters of EBU R128 (ITU-R
/// BS.1770), a high shelf of about +4 dB above 2 kHz and a high-pass at 38 Hz, as
/// libebur128 designs them for any sample rate.
fn k_weights(rate: f32, fft_size: usize) -> Vec<f32> {
    let rate = f64::from(rate);
    let (f0, gain_db, q) = (1681.974450955533, 3.999843853973347, 0.7071752369554196);
    let k = (std::f64::consts::PI * f0 / rate).tan();
    let vh = 10f64.powf(gain_db / 20.0);
    let vb = vh.powf(0.4996667741545416);
    let a0 = 1.0 + k / q + k * k;
    let shelf = ([(vh + vb * k / q + k * k) / a0, 2.0 * (k * k - vh) / a0, (vh - vb * k / q + k * k) / a0], [
        1.0,
        2.0 * (k * k - 1.0) / a0,
        (1.0 - k / q + k * k) / a0,
    ]);
    let (f0, q) = (38.13547087602444, 0.5003270373238773);
    let k = (std::f64::consts::PI * f0 / rate).tan();
    let a0 = 1.0 + k / q + k * k;
    let high_pass = ([1.0, -2.0, 1.0], [1.0, 2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0]);
    // |b(z)|² / |a(z)|² on the unit circle.
    let power = |(b, a): &([f64; 3], [f64; 3]), w: f64| {
        let at = |c: &[f64; 3]| {
            let (re, im) = (c[0] + c[1] * w.cos() + c[2] * (2.0 * w).cos(), -(c[1] * w.sin() + c[2] * (2.0 * w).sin()));
            re * re + im * im
        };
        at(b) / at(a)
    };
    (0..fft_size / 2)
        .map(|bin| {
            let w = 2.0 * std::f64::consts::PI * bin as f64 / fft_size as f64;
            (power(&shelf, w) * power(&high_pass, w)) as f32
        })
        .collect()
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
fn filter_bank(bin_hz: f32, fft_size: usize) -> Vec<Filter> {
    let centre = band_centre;
    let last_bin = fft_size / 2 - 1;
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
            if total >= 1.0 {
                return Filter { start: first, weights, total };
            }
            let x = (c / bin_hz).clamp(1.0, (last_bin - 1) as f32);
            let k = x.floor();
            Filter { start: k as usize, weights: vec![1.0 - (x - k), x - k], total: 1.0 }
        })
        .collect()
}

/// The centre of the filter bank's band `i`, Hz (log-spaced; also past the ends, for
/// their edges).
fn band_centre(i: isize) -> f32 {
    let ratio = (SPECTRUM_HI_HZ / SPECTRUM_LO_HZ).powf(1.0 / (SPECTRUM_BANDS - 1) as f32);
    SPECTRUM_LO_HZ * ratio.powi(i as i32)
}

/// The filter bank's bands centred from `lo` up to `hi` Hz (the top band included when
/// `hi` is the top of the range).
fn bank_range(lo: f32, hi: f32) -> (usize, usize) {
    let below = |hz: f32| (0..SPECTRUM_BANDS).take_while(|&b| band_centre(b as isize) < hz * 0.9999).count();
    let top = if hi >= SPECTRUM_HI_HZ { SPECTRUM_BANDS } else { below(hi) };
    (below(lo), top.max(below(lo) + 1))
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

    /// Push this hop's onset value (a rise in log energy). Returns (hop index, strength)
    /// for a confirmed hit at the previous hop.
    fn push(&mut self, value: f32, audible: bool, sensitivity: f32) -> Option<(usize, f32)> {
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
        let filters = filter_bank(bin_hz, FFT_SIZE);
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
            p.push((3.0 * rng.next_f32() - 1.5).max(0.0), true, 1.5);
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
        let (signal, _, _) = synthetic_beat(rate, 14.0, 120.0, 1.0);
        let frames: Vec<Frame> = signal.iter().map(|&x| [x, 0.5 * x]).collect();
        // Long enough for the beat tracker to have started.
        let (warm, rest) = frames.split_at(9 * rate as usize);
        let mut a = Analyzer::new(rate);
        let (mut onsets, mut beats) = (Vec::with_capacity(256), Vec::with_capacity(64));
        a.process(warm, 1.5);
        a.drain_onsets(&mut onsets);
        a.drain_beats(&mut beats);
        onsets.clear();
        beats.clear();
        let count = crate::test_alloc::allocations(|| {
            for chunk in rest.chunks(800) {
                a.process(chunk, 1.5);
                a.drain_onsets(&mut onsets);
                a.drain_beats(&mut beats);
            }
        });
        assert_eq!(count, 0, "allocated {count} times");
        assert!(!onsets.is_empty(), "the beat should still be heard");
        assert!(!beats.is_empty(), "and its beats reported");
    }

    /// EBU Tech 3341's first test: a 1 kHz sine at -23 dBFS on both channels reads -23
    /// LUFS, momentary and short-term.
    #[test]
    fn a_reference_tone_reads_its_loudness() {
        let rate = 48_000;
        let amplitude = 10f32.powf(-23.0 / 20.0);
        let (a, _) = analyse(&tone(rate, 1000.0, 4.0, amplitude), rate, 1000);
        let f = a.features();
        assert!((f.loudness + 23.0).abs() < 0.1, "momentary {} LUFS", f.loudness);
        assert!((f.loudness_short + 23.0).abs() < 0.1, "short-term {} LUFS", f.loudness_short);
    }

    /// The level hears as EBU R128 does: at the same RMS, a deep bass tone counts for less
    /// than a 1 kHz one, and a bright one for more.
    #[test]
    fn the_level_is_k_weighted() {
        let rate = 48_000;
        let heard = |hz: f32| analyse(&tone(rate, hz, 1.0, 0.1), rate, 1000).0.level_env.value;
        let (deep, mid, bright) = (heard(30.0), heard(1000.0), heard(5000.0));
        let db = |x: f32| 20.0 * (x / mid).log10();
        assert!(db(deep) < -4.0, "30 Hz at {:.1} dB", db(deep));
        assert!((2.0..5.0).contains(&db(bright)), "5 kHz at {:.1} dB", db(bright));
    }

    /// Silence follows loudness: a tone at -60 dBFS is silent, one at -40 dBFS isn't.
    #[test]
    fn silence_is_quiet_as_heard() {
        let rate = 48_000;
        let at = |dbfs: f32| analyse(&tone(rate, 1000.0, 2.0, 10f32.powf(dbfs / 20.0)), rate, 1000).0.features().silent;
        assert!(at(-60.0), "-60 dBFS should be silent");
        assert!(!at(-40.0), "-40 dBFS should be heard");
    }

    /// A sustained bass line, with vibrato and a slide between notes, fires no kicks: its
    /// energy holds steady while its pitch moves. (A plucked bass note in the kick's range
    /// still reads as a kick: at the short FFT's resolution, 30 to 120 Hz is four bins.)
    #[test]
    fn a_sustained_bass_line_fires_no_kicks() {
        let rate = 48_000;
        let mut phase = 0.0f32;
        let bass: Vec<f32> = (0..6 * rate)
            .map(|k| {
                let t = k as f32 / rate as f32;
                let note = if t < 3.0 { 55.0 } else { 55.0 + 18.0 * ((t - 3.0) / 0.5).min(1.0) };
                phase += 2.0 * PI * note * (1.0 + 0.03 * (2.0 * PI * 5.5 * t).sin()) / rate as f32;
                // A sawtooth's first harmonics, fading in over 50 ms.
                let fade = (t / 0.05).min(1.0);
                fade * 0.3 * (1..6).map(|h| (h as f32 * phase).sin() / h as f32).sum::<f32>()
            })
            .collect();
        let (_, onsets) = analyse(&bass, rate, 1000);
        let kicks = times(&onsets, OnsetKind::Kick);
        assert!(kicks.iter().all(|&t| t < 0.5), "kicks in a bass line: {kicks:?}");
    }

    /// The long FFT resolves low tones the short one can't: 45 and 80 Hz together show as
    /// two peaks with a dip between.
    #[test]
    fn the_low_bands_resolve_close_tones() {
        let rate = 48_000;
        let (a, b) = (tone(rate, 45.0, 1.0, 0.2), tone(rate, 80.0, 1.0, 0.2));
        let mix: Vec<f32> = a.iter().zip(&b).map(|(x, y)| x + y).collect();
        let (an, _) = analyse(&mix, rate, 1000);
        let s = &an.features().spectrum;
        let band = |hz: f32| (0..SPECTRUM_BANDS).min_by(|&i, &j| (band_centre(i as isize) / hz).ln().abs().total_cmp(&(band_centre(j as isize) / hz).ln().abs())).unwrap();
        let (low, dip, high) = (s[band(45.0)], s[band(60.0)], s[band(80.0)]);
        // 0.1 on the spectrum's scale is 6 dB.
        assert!(dip < low - 0.1 && dip < high - 0.1, "45 Hz {low:.2}, 60 Hz {dip:.2}, 80 Hz {high:.2}");
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
