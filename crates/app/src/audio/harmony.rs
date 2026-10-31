//! The harmony: which pitch classes sound, and a hue that follows the chords.
//!
//! - **Chroma:** the long FFT's magnitudes from `LO_HZ` to `HI_HZ`, each bin folded onto
//!   the twelve pitch classes (C = 0) by its distance from the nearest semitone (a bin
//!   between two counts half), smoothed over about a beat and scaled so the strongest is
//!   1.
//! - **The hue:** the chroma, slower (over a couple of seconds, so a chord change moves
//!   it and a melody's notes don't), less its mean, as a vector round the circle of
//!   fifths (C at 0, G a twelfth of a turn on, D two...): related chords sit close, so
//!   a change of chord is a small step and a change of key a bigger one. Its length
//!   against the chroma's spread, times how far the peak stands above the mean, is the
//!   tonality: 0 for noise or drums, near 1 for one clear chord. The hue eases toward the vector's angle, the faster the more tonal,
//!   and holds where it is through drums alone, noise and silence. It isn't wrapped (a
//!   turn on is 1 more), so it never jumps.
//! - **The key:** the slow chroma's strongest pitch class, changing only once another
//!   has been strongest for `KEY_HOLD_S` (about a bar).
//! - **Timbre:** the brightness (spectral centroid), over a second and a half, for
//!   colours to follow without flicker.
//!
//! Updated each time the long FFT runs (every few hops). Nothing allocates after `new`.

/// Pitch classes, from C.
pub const CLASSES: usize = 12;
/// The bins counted, Hz: below `LO_HZ` the long FFT's bins are wider than a semitone.
const LO_HZ: f32 = 100.0;
const HI_HZ: f32 = 5000.0;
/// The chroma's smoothing, the slow chroma's (for the hue and the key), and how long
/// the hue takes to reach a new chord at full tonality, seconds.
const CHROMA_S: f32 = 0.4;
const SLOW_S: f32 = 2.0;
const HUE_S: f32 = 0.8;
/// The slow chroma's mean below its peak, as a share of the peak, from which it counts
/// as fully tonal (noise's pitch classes are all much alike).
const CONTRAST: f32 = 0.7;
/// Below this tonality the hue holds.
const TONAL: f32 = 0.15;
const KEY_HOLD_S: f32 = 2.0;
const TIMBRE_S: f32 = 1.5;

/// The harmony as of the latest update.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Harmony {
    /// Per pitch class (C = 0), 0..1, the strongest 1 (all 0 before any sound).
    pub chroma: [f32; CLASSES],
    /// The strongest pitch class, held (0 = C).
    pub key: usize,
    /// 0 (noise, drums) to 1 (one clear chord).
    pub tonality: f32,
    /// The hue the chords give, turns, unwrapped (see the module docs).
    pub hue: f32,
    /// The brightness, smoothed (0 dull to 1 bright).
    pub timbre: f32,
}

impl Default for Harmony {
    fn default() -> Self {
        Self { chroma: [0.0; CLASSES], key: 0, tonality: 0.0, hue: 0.0, timbre: 0.5 }
    }
}

pub struct HarmonyTracker {
    /// Per bin from `first`, its pitch class and weight (0 for none).
    first: usize,
    bins: Vec<(usize, f32)>,
    chroma: [f32; CLASSES],
    slow: [f32; CLASSES],
    /// The candidate for the key, and how long it has been the strongest.
    candidate: usize,
    candidate_for: f32,
    started: bool,
    state: Harmony,
}

impl HarmonyTracker {
    /// For an FFT with bins `bin_hz` apart and `bins` of them (half its size).
    pub fn new(bin_hz: f32, bins: usize) -> Self {
        let first = ((LO_HZ / bin_hz).ceil() as usize).max(1);
        let last = ((HI_HZ / bin_hz).floor() as usize).min(bins.saturating_sub(1)).max(first);
        let bins = (first..=last)
            .map(|k| {
                // Semitones from C0 (16.35 Hz).
                let pitch = 12.0 * (k as f32 * bin_hz / 16.351_6).log2();
                let nearest = pitch.round();
                ((nearest as i64).rem_euclid(CLASSES as i64) as usize, 1.0 - (pitch - nearest).abs())
            })
            .collect();
        Self { first, bins, chroma: [0.0; CLASSES], slow: [0.0; CLASSES], candidate: 0, candidate_for: 0.0, started: false, state: Harmony::default() }
    }

    pub fn state(&self) -> Harmony {
        self.state
    }

    /// From the FFT's power spectrum (`dt` seconds since the last update), unless it's
    /// silent; the brightness updates either way.
    pub fn update(&mut self, power: &[f32], brightness: f32, silent: bool, dt: f32) {
        let s = &mut self.state;
        s.timbre += (brightness - s.timbre) * smoothing(dt, TIMBRE_S);
        if silent {
            return;
        }
        let mut raw = [0.0f32; CLASSES];
        for (&(class, weight), p) in self.bins.iter().zip(&power[self.first..]) {
            raw[class] += weight * p.sqrt();
        }
        let loudest = raw.iter().fold(0.0f32, |m, &c| m.max(c));
        if loudest <= 0.0 {
            return;
        }
        let (fast, slow) = if self.started { (smoothing(dt, CHROMA_S), smoothing(dt, SLOW_S)) } else { (1.0, 1.0) };
        self.started = true;
        for ((r, chroma), slower) in raw.iter().zip(&mut self.chroma).zip(&mut self.slow) {
            let r = r / loudest;
            *chroma += (r - *chroma) * fast;
            *slower += (r - *slower) * slow;
        }
        let top = self.chroma.iter().fold(0.0f32, |m, &c| m.max(c)).max(1e-6);
        s.chroma = self.chroma.map(|c| c / top);

        // The hue: the slow chroma round the circle of fifths.
        let mean = self.slow.iter().sum::<f32>() / CLASSES as f32;
        let (mut x, mut y, mut spread) = (0.0f32, 0.0f32, 0.0f32);
        for (c, &v) in self.slow.iter().enumerate() {
            let angle = std::f32::consts::TAU * ((7 * c) % CLASSES) as f32 / CLASSES as f32;
            let d = v - mean;
            (x, y, spread) = (x + d * angle.cos(), y + d * angle.sin(), spread + d.abs());
        }
        let peak = self.slow.iter().fold(0.0f32, |m, &v| m.max(v));
        let contrast = if peak > 0.0 { ((1.0 - mean / peak) / CONTRAST).min(1.0) } else { 0.0 };
        s.tonality = if spread > 1e-6 { ((x * x + y * y).sqrt() / spread * 2.0).min(1.0) * contrast } else { 0.0 };
        if s.tonality > TONAL {
            let target = y.atan2(x) / std::f32::consts::TAU;
            let step = (target - s.hue).rem_euclid(1.0);
            let step = if step > 0.5 { step - 1.0 } else { step };
            s.hue += step * smoothing(dt, HUE_S) * s.tonality;
        }

        // The key: the slow chroma's strongest, once it has held.
        let strongest = (0..CLASSES).max_by(|&a, &b| self.slow[a].total_cmp(&self.slow[b])).unwrap_or(0);
        if strongest == s.key {
            self.candidate_for = 0.0;
        } else {
            if strongest != self.candidate {
                (self.candidate, self.candidate_for) = (strongest, 0.0);
            }
            self.candidate_for += dt;
            if self.candidate_for >= KEY_HOLD_S {
                s.key = strongest;
                self.candidate_for = 0.0;
            }
        }
    }
}

/// The fraction of the way an exponential smoother moves in `dt` with time constant
/// `tau`.
fn smoothing(dt: f32, tau: f32) -> f32 {
    1.0 - (-dt / tau).exp()
}
