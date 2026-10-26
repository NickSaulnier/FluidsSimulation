//! Musical time: tempo, beats and bars, from the analyzer's onset strength (see
//! `analyzer`), fed one value per hop.
//!
//! - **Tempo:** the autocorrelation of the onset strength over the last `HISTORY_S`,
//!   searched from `MIN_BPM` to `MAX_BPM`, weighted by a log-normal prior around
//!   `PRIOR_BPM` (as in Ellis 2007 and librosa). Each lag scores its own correlation
//!   plus those two beats and a bar on (music repeats by the bar; a tempo at two thirds
//!   or four thirds of the real one doesn't); then of that tempo, its half and its
//!   double, the one whose own correlation is strongest wins. Updated every `UPDATE_S`,
//!   with a confidence: how much of the onsets' variation repeats at that period.
//! - **Beats:** a clock at that tempo. At each update a comb over the recent onsets
//!   finds where the beats fall (preferring where the clock expects them, so it doesn't
//!   jump to the off-beat), and the clock's phase is pulled toward it while its period
//!   follows the tempo found: a phase-locked loop, which keeps time through a quiet bar
//!   and follows a drifting drummer. A different tempo found for `RETEMPO_UPDATES` in a
//!   row resets the clock. Beats are reported while the confidence holds and the tempo
//!   found has stayed the clock's (or half or double it) for a while: random hits can
//!   look periodic for a moment, but their tempo wanders.
//! - **Bars:** four beats to a bar; the downbeat is the beat position with the strongest
//!   low-frequency onsets over the last bars.
//!
//! An offset (`set_offset`) shifts what's reported to when it's heard: beats are
//! predicted, so they can come early as well as late.

pub const MIN_BPM: f32 = 60.0;
pub const MAX_BPM: f32 = 200.0;
/// Seconds of onset strength the tempo is found in, and how often.
const HISTORY_S: f32 = 8.0;
const UPDATE_S: f32 = 0.5;
/// The tempo prior: centred here, its spread in octaves.
const PRIOR_BPM: f32 = 120.0;
const PRIOR_OCTAVES: f32 = 0.7;
/// A tempo's score adds the correlation two beats on and a bar on, at these weights:
/// music repeats by the bar, which a tempo two thirds or four thirds of the real one
/// doesn't share.
const BEAT_PAIR: f32 = 0.5;
const BAR: f32 = 1.0;
/// Seconds of onsets the comb weighs for the beats' phase, and the weight's fall per beat
/// back.
const COMB_S: f32 = 4.0;
const COMB_FALL: f32 = 0.85;
/// The phase-locked loop: the share of a phase error corrected at an update, and of the
/// difference from the tempo found taken into the clock's period.
const PHASE_GAIN: f32 = 0.5;
const PERIOD_GAIN: f32 = 0.3;
/// The comb prefers beats near where the clock expects them: candidates further than
/// this share of a beat count for less (down to half), so the clock stays on the beat
/// unless the off-beat is clearly stronger.
const PHASE_SPREAD: f32 = 0.15;
/// A tempo this far from the clock's, found the same for this many updates in a row,
/// resets the clock.
const RETEMPO: f32 = 0.05;
const RETEMPO_UPDATES: u32 = 3;
/// Beats are reported while the confidence is at least this, once the tempo found has
/// agreed with the clock for this many updates in a row (random hits can look periodic
/// for a moment, but their tempo doesn't hold).
const MIN_CONFIDENCE: f32 = 0.1;
const STEADY_UPDATES: u32 = 4;
/// Bars: the low onsets this close to a beat count for it, and each beat position's
/// score falls by this much per bar.
const DOWNBEAT_S: f32 = 0.06;
const BAR_FALL: f32 = 0.9;
pub const BEATS_PER_BAR: u64 = 4;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Beat {
    /// Seconds since the analyzer started (when the beat is in the music).
    pub time: f32,
    /// Beats counted since the clock started.
    pub index: u64,
    /// The first beat of a bar.
    pub downbeat: bool,
}

/// Where the music is in time, as of the latest hop (shifted by the offset).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MusicalTime {
    /// Beats per minute; 0 until a tempo is found.
    pub bpm: f32,
    /// How strongly the onsets repeat at that tempo, 0..1.
    pub confidence: f32,
    /// Whether beats are being reported (the confidence holds).
    pub beating: bool,
    /// 0..1 from one beat to the next, and from one downbeat to the next.
    pub beat_phase: f32,
    pub bar_phase: f32,
    /// When the next beat falls, seconds since the analyzer started.
    pub next_beat: f32,
}

pub struct BeatTracker {
    hop_s: f32,
    /// The last `HISTORY_S` of onset strength and of low (kick) onsets, rings with the
    /// next write at `pos`.
    strength: Vec<f32>,
    lows: Vec<f32>,
    pos: usize,
    len: usize,
    hops: usize,
    update_every: usize,
    min_lag: usize,
    max_lag: usize,
    /// Per lag (hops), the tempo prior; scratch for the history (oldest first, the mean
    /// removed) and its autocorrelation.
    prior: Vec<f32>,
    history: Vec<f32>,
    acf: Vec<f32>,
    /// Seconds since the analyzer started, of this hop.
    now: f64,
    offset: f64,
    /// The clock: its period (0 until a tempo is found), the next beat and its index.
    period: f64,
    next_beat: f64,
    index: u64,
    confidence: f32,
    /// A different tempo found (its period), and for how many updates in a row.
    candidate: f64,
    candidate_updates: u32,
    /// Updates in a row the tempo found has agreed with the clock.
    steady: u32,
    /// Beats waiting for their low onsets to be heard (time, index), the beat positions'
    /// scores, and the downbeat's position.
    unscored: [(f64, u64); 8],
    unscored_len: usize,
    bar_scores: [f32; BEATS_PER_BAR as usize],
    downbeat: u64,
    beats: Vec<Beat>,
}

impl BeatTracker {
    pub fn new(hop_s: f32) -> Self {
        let cap = (HISTORY_S / hop_s).ceil() as usize;
        let lag = |bpm: f32| 60.0 / bpm / hop_s;
        let (min_lag, max_lag) = (lag(MAX_BPM).floor() as usize, lag(MIN_BPM).ceil() as usize);
        let prior = (0..=max_lag)
            .map(|l| {
                let octaves = (60.0 / (l.max(1) as f32 * hop_s) / PRIOR_BPM).log2() / PRIOR_OCTAVES;
                (-0.5 * octaves * octaves).exp()
            })
            .collect();
        Self {
            hop_s,
            strength: vec![0.0; cap],
            lows: vec![0.0; cap],
            pos: 0,
            len: 0,
            hops: 0,
            update_every: ((UPDATE_S / hop_s).round() as usize).max(1),
            min_lag,
            max_lag,
            prior,
            history: Vec::with_capacity(cap),
            acf: vec![0.0; 4 * max_lag + 2],
            now: 0.0,
            offset: 0.0,
            period: 0.0,
            next_beat: 0.0,
            index: 0,
            confidence: 0.0,
            candidate: 0.0,
            candidate_updates: 0,
            steady: 0,
            unscored: [(0.0, 0); 8],
            unscored_len: 0,
            bar_scores: [0.0; BEATS_PER_BAR as usize],
            downbeat: 0,
            beats: Vec::with_capacity(8),
        }
    }

    /// Report everything this much later (seconds; negative for earlier), for audio that
    /// is heard after (or before) it's analysed.
    pub fn set_offset(&mut self, seconds: f32) {
        self.offset = f64::from(seconds);
    }

    /// Move the beats reported since the last call into `out`.
    pub fn drain_beats(&mut self, out: &mut Vec<Beat>) {
        out.append(&mut self.beats);
    }

    /// Feed one hop: its time (seconds since the analyzer started), the onset strength
    /// and the low onsets' rise.
    pub fn push(&mut self, time: f32, strength: f32, low: f32) {
        let cap = self.strength.len();
        self.strength[self.pos] = strength;
        self.lows[self.pos] = low;
        self.pos = (self.pos + 1) % cap;
        self.len = (self.len + 1).min(cap);
        self.hops += 1;
        self.now = f64::from(time);

        // Beats, as they're heard.
        while self.period > 0.0 && self.now - self.offset >= self.next_beat {
            let beat = Beat { time: self.next_beat as f32, index: self.index, downbeat: self.index % BEATS_PER_BAR == self.downbeat };
            if self.beating() {
                self.beats.push(beat);
            }
            if self.unscored_len < self.unscored.len() {
                self.unscored[self.unscored_len] = (self.next_beat, self.index);
                self.unscored_len += 1;
            }
            self.index += 1;
            self.next_beat += self.period;
        }
        self.score_downbeats();
        if self.hops.is_multiple_of(self.update_every) && self.len >= cap / 2 {
            self.update();
        }
    }

    /// Where the music is now.
    pub fn time(&self) -> MusicalTime {
        if self.period <= 0.0 {
            return MusicalTime::default();
        }
        let heard = self.now - self.offset;
        let beat_phase = (1.0 - (self.next_beat - heard) / self.period).clamp(0.0, 1.0);
        // Beats since the last downbeat: `index` is the next beat's.
        let into_bar = (self.index + BEATS_PER_BAR - 1 - self.downbeat) % BEATS_PER_BAR;
        MusicalTime {
            bpm: (60.0 / self.period) as f32,
            confidence: self.confidence,
            beating: self.beating(),
            beat_phase: beat_phase as f32,
            bar_phase: ((into_bar as f64 + beat_phase) / BEATS_PER_BAR as f64) as f32,
            next_beat: self.next_beat as f32,
        }
    }

    fn beating(&self) -> bool {
        self.confidence >= MIN_CONFIDENCE && self.steady >= STEADY_UPDATES
    }

    /// The value `back` seconds before now, interpolated (0 outside the history).
    fn at(&self, ring: &[f32], back: f64) -> f32 {
        let h = back / f64::from(self.hop_s);
        if h < 0.0 || h > (self.len - 1) as f64 {
            return 0.0;
        }
        let (i, frac) = (h.floor() as usize, (h - h.floor()) as f32);
        let cap = ring.len();
        let get = |k: usize| ring[(self.pos + cap - 1 - k.min(self.len - 1)) % cap];
        get(i) * (1.0 - frac) + get(i + 1) * frac
    }

    /// Score the beats whose low onsets have now been heard: each beat position keeps a
    /// falling sum of the strongest low onset near its beats.
    fn score_downbeats(&mut self) {
        let mut kept = 0;
        for k in 0..self.unscored_len {
            let (t, index) = self.unscored[k];
            let back = self.now - t;
            if back < f64::from(DOWNBEAT_S) {
                self.unscored[kept] = self.unscored[k];
                kept += 1;
                continue;
            }
            let steps = (2.0 * DOWNBEAT_S / self.hop_s).ceil() as usize;
            let evidence = (0..=steps)
                .map(|s| self.at(&self.lows, back - f64::from(DOWNBEAT_S) + s as f64 * f64::from(self.hop_s)))
                .fold(0.0f32, f32::max);
            let fall = BAR_FALL.powf(1.0 / BEATS_PER_BAR as f32);
            for score in &mut self.bar_scores {
                *score *= fall;
            }
            self.bar_scores[(index % BEATS_PER_BAR) as usize] += evidence;
        }
        self.unscored_len = kept;
        let best = (0..BEATS_PER_BAR as usize).max_by(|&a, &b| self.bar_scores[a].total_cmp(&self.bar_scores[b])).unwrap_or(0);
        if self.bar_scores[best] > 0.0 {
            self.downbeat = best as u64;
        }
    }

    /// Find the tempo in the history, and pull the clock toward it and toward where the
    /// recent beats fall.
    fn update(&mut self) {
        let cap = self.strength.len();
        self.history.clear();
        self.history.extend((0..self.len).map(|k| self.strength[(self.pos + cap - self.len + k) % cap]));
        let n = self.history.len();
        let mean = self.history.iter().sum::<f32>() / n as f32;
        for x in &mut self.history {
            *x -= mean;
        }
        for (lag, a) in self.acf.iter_mut().enumerate() {
            *a = if lag < n { self.history[lag..].iter().zip(&self.history).map(|(x, y)| x * y).sum::<f32>() / (n - lag) as f32 } else { 0.0 };
        }
        // The tempo family: a lag with its double and its quadruple, two beats and a bar
        // (a multiple's lag is uncertain by the multiple times the lag's rounding, so it
        // takes the best correlation within that). Then the octave: of the lag, its half
        // and its double, the best by its own correlation and the prior.
        let near = |centre: usize, radius: usize| self.acf[centre - radius..=(centre + radius).min(self.acf.len() - 1)].iter().fold(f32::MIN, |m, &a| m.max(a));
        let family = |lag: usize| self.prior[lag] * (self.acf[lag] + BEAT_PAIR * near(2 * lag, 1) + BAR * near(4 * lag, 2));
        let Some(best) = (self.min_lag..=self.max_lag).max_by(|&a, &b| family(a).total_cmp(&family(b))) else { return };
        let own = |lag: usize| if (self.min_lag..=self.max_lag).contains(&lag) { self.prior[lag] * self.acf[lag] } else { f32::MIN };
        let best = [best / 2, best, 2 * best].into_iter().max_by(|&a, &b| own(a).total_cmp(&own(b))).unwrap_or(best);
        let peak = own(best);
        let lag = best as f64 + parabola(own(best.saturating_sub(1).max(self.min_lag)), peak, own((best + 1).min(self.max_lag)));
        let confidence = if self.acf[0] > 0.0 { (self.acf[best] / self.acf[0]).clamp(0.0, 1.0) } else { 0.0 };
        self.confidence += (confidence - self.confidence) * 0.5;
        if peak <= 0.0 {
            return;
        }
        let period = lag * f64::from(self.hop_s);
        let near = |a: f64, b: f64| (a / b - 1.0).abs() <= f64::from(RETEMPO);

        // Steady while the tempo found is the clock's, or half or double it (the same beat).
        let related = self.period > 0.0 && [1.0, 2.0, 0.5].into_iter().any(|r| near(period, self.period * r));
        self.steady = if related { self.steady + 1 } else { 0 };
        if self.period > 0.0 && !near(period, self.period) {
            // A different tempo: only once it's been found for a while.
            self.candidate_updates = if near(period, self.candidate) { self.candidate_updates + 1 } else { 1 };
            self.candidate = period;
            if self.candidate_updates < RETEMPO_UPDATES {
                self.correct_phase();
                return;
            }
        }
        if self.period <= 0.0 || !near(period, self.period) {
            // Start (or restart) the clock on the beats found.
            let beat = self.recent_beat(period, None);
            self.period = period;
            self.next_beat = beat + period * ((self.now - self.offset - beat) / period).floor().max(0.0) + period;
            // The updates that found the new tempo count toward its being steady.
            self.steady = self.candidate_updates;
            self.candidate_updates = 0;
            return;
        }
        self.candidate_updates = 0;
        self.period += f64::from(PERIOD_GAIN) * (period - self.period);
        self.correct_phase();
    }

    /// Pull the clock's phase toward where the comb finds the recent beats.
    fn correct_phase(&mut self) {
        let clock = self.next_beat - self.period;
        let beat = self.recent_beat(self.period, Some(clock));
        // The phase error, within half a period either way.
        let error = (beat - clock + 0.5 * self.period).rem_euclid(self.period) - 0.5 * self.period;
        self.next_beat = (self.next_beat + f64::from(PHASE_GAIN) * error).max(clock + 0.5 * self.period);
    }

    /// The time of the most recent beat in the onset strength, for beats `period` apart:
    /// the offset (back from now) whose comb of beats gathers the most onset strength,
    /// preferring offsets near `expected` (a beat time), if given.
    fn recent_beat(&self, period: f64, expected: Option<f64>) -> f64 {
        let hop = f64::from(self.hop_s);
        let steps = (period / hop).ceil() as usize;
        let teeth = (f64::from(COMB_S) / period).ceil().max(1.0) as usize;
        let expected_offset = expected.map(|e| (self.now - e).rem_euclid(period) / hop);
        let comb = |o: usize| -> f32 {
            let sum: f32 = (0..teeth).map(|k| COMB_FALL.powi(k as i32) * self.at(&self.strength, o as f64 * hop + k as f64 * period)).sum();
            let Some(e) = expected_offset else { return sum };
            let d = (o as f64 - e).rem_euclid(steps as f64);
            let d = d.min(steps as f64 - d) / steps as f64;
            sum * (0.5 + 0.5 * (-0.5 * (d / f64::from(PHASE_SPREAD)).powi(2)).exp() as f32)
        };
        let best = (0..steps).max_by(|&a, &b| comb(a).total_cmp(&comb(b))).unwrap_or(0);
        let refine = parabola(comb((best + steps - 1) % steps), comb(best), comb((best + 1) % steps));
        self.now - (best as f64 + refine) * hop
    }
}

/// Where the peak of a parabola through three equally spaced values falls, relative to
/// the middle one (-0.5..0.5).
fn parabola(a: f32, b: f32, c: f32) -> f64 {
    let d = a - 2.0 * b + c;
    if d.abs() < 1e-12 { 0.0 } else { f64::from((0.5 * (a - c) / d).clamp(-0.5, 0.5)) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::analyzer::{Analyzer, Frame};
    use std::f32::consts::PI;
    use utils::rng::SplitMix64;

    const RATE: u32 = 44_100;

    /// A drum pattern: a kick on 1 and 3, a snare on 2 and 4, hats on the eighths (the
    /// off ones `swing` of the way through the beat). The tempo may change over time;
    /// `silent` mutes the drums for a stretch. Returns the audio and the true beats.
    fn drums(seconds: f32, bpm: impl Fn(f32) -> f32, swing: f32, silent: Option<(f32, f32)>, seed: u64) -> (Vec<Frame>, Vec<f32>) {
        let n = (seconds * RATE as f32) as usize;
        let mut out = vec![0.0f32; n];
        let mut rng = SplitMix64::new(seed);
        let add = |out: &mut [f32], t: f32, f: &mut dyn FnMut(f32) -> f32, length: f32| {
            let start = (t * RATE as f32) as usize;
            for k in 0..(length * RATE as f32) as usize {
                if start + k < out.len() {
                    out[start + k] += f(k as f32 / RATE as f32);
                }
            }
        };
        let (mut t, mut beat, mut beats) = (0.3f32, 0usize, Vec::new());
        while t < seconds - 0.5 {
            let period = 60.0 / bpm(t);
            beats.push(t);
            if silent.is_none_or(|(a, b)| !(a..b).contains(&t)) {
                if beat % 2 == 0 {
                    let mut phase = 0.0f32;
                    add(&mut out, t, &mut |tt| {
                        phase += 2.0 * PI * (50.0 + 100.0 * (-tt * 30.0).exp()) / RATE as f32;
                        0.8 * phase.sin() * (-tt * 12.0).exp()
                    }, 0.25);
                } else {
                    add(&mut out, t, &mut |tt| 0.4 * rng.signed() * (-tt * 20.0).exp(), 0.2);
                }
                for hat in [t, t + swing * period] {
                    let mut prev = 0.0;
                    add(&mut out, hat, &mut |tt| {
                        let w = rng.signed();
                        let v = 0.25 * (w - prev) * (-tt * 80.0).exp();
                        prev = w;
                        v
                    }, 0.05);
                }
            }
            t += period;
            beat += 1;
        }
        (out.iter().map(|&x| [x, x]).collect(), beats)
    }

    /// The analyzer's beats and musical time at the end.
    fn track(frames: &[Frame]) -> (Vec<Beat>, MusicalTime) {
        let mut a = Analyzer::new(RATE);
        let mut beats = Vec::new();
        for chunk in frames.chunks(1024) {
            a.process(chunk, 1.5);
            a.drain_beats(&mut beats);
        }
        (beats, a.features().musical)
    }

    /// Every true beat in `from..to` has a reported beat within `tolerance`, and no
    /// reported beat there is far from a true one. Returns the worst error.
    fn beats_match(found: &[Beat], truth: &[f32], from: f32, to: f32, tolerance: f32) -> Result<f32, String> {
        let mut worst = 0.0f32;
        for &t in truth.iter().filter(|&&t| (from..to).contains(&t)) {
            let nearest = found.iter().map(|b| (b.time - t).abs()).fold(f32::MAX, f32::min);
            if nearest > tolerance {
                return Err(format!("the beat at {t:.2} is {:.0} ms from the nearest reported", nearest * 1e3));
            }
            worst = worst.max(nearest);
        }
        for b in found.iter().filter(|b| (from..to).contains(&b.time)) {
            let nearest = truth.iter().map(|&t| (b.time - t).abs()).fold(f32::MAX, f32::min);
            if nearest > tolerance {
                return Err(format!("a beat reported at {:.2} is {:.0} ms from a true one", b.time, nearest * 1e3));
            }
        }
        Ok(worst)
    }

    /// From 70 to 180 BPM, straight and swung: the tempo within 2% (or an octave off, in
    /// at most 3 of the 10), and every beat after the first 8 seconds within 30 ms.
    #[test]
    fn finds_the_tempo_and_the_beats() {
        let mut octave_errors = 0;
        for bpm in [70.0, 90.0, 120.0, 150.0, 180.0] {
            for swing in [0.5, 0.62] {
                let (frames, truth) = drums(20.0, |_| bpm, swing, None, bpm as u64);
                let (beats, time) = track(&frames);
                // An octave error is counted, not failed: the beats are then checked
                // against that octave's grid (every other beat, or with the half-beats).
                let near = |ratio: f32| (time.bpm / (bpm * ratio) - 1.0).abs() < 0.02;
                let grid: Vec<f32> = if near(1.0) {
                    truth.clone()
                } else if near(2.0) {
                    octave_errors += 1;
                    truth.iter().flat_map(|&t| [t, t + 30.0 / bpm]).collect()
                } else if near(0.5) {
                    octave_errors += 1;
                    let phase = usize::from(beats.iter().rev().find(|b| b.time < 19.0).is_some_and(|b| {
                        truth.iter().position(|&t| (t - b.time).abs() < 0.1).is_some_and(|i| i % 2 == 1)
                    }));
                    truth.iter().skip(phase).step_by(2).copied().collect()
                } else {
                    panic!("{bpm} BPM (swing {swing}): found {:.1}", time.bpm);
                };
                if !near(1.0) {
                    eprintln!("{bpm} BPM (swing {swing}): an octave off, at {:.1}", time.bpm);
                }
                if let Err(e) = beats_match(&beats, &grid, 8.0, 19.5, 0.03) {
                    panic!("{bpm} BPM (swing {swing}, found {:.1}): {e}", time.bpm);
                }
            }
        }
        assert!(octave_errors <= 3, "{octave_errors} octave errors");
    }

    /// A bar without drums: the clock keeps time through it, and is still on the beat
    /// after.
    #[test]
    fn keeps_time_through_a_dropped_bar() {
        let bpm = 110.0;
        let bar = 4.0 * 60.0 / bpm;
        let (frames, truth) = drums(20.0, |_| bpm, 0.5, Some((10.0, 10.0 + bar)), 7);
        let (beats, _) = track(&frames);
        if let Err(e) = beats_match(&beats, &truth, 8.0, 19.5, 0.03) {
            panic!("{e}");
        }
    }

    /// The tempo changes from 100 to 130 BPM halfway: the tracker follows, within 2%,
    /// and is back on the beat a few seconds later.
    #[test]
    fn follows_a_tempo_change() {
        let (frames, truth) = drums(24.0, |t| if t < 12.0 { 100.0 } else { 130.0 }, 0.5, None, 9);
        let (beats, time) = track(&frames);
        assert!((time.bpm / 130.0 - 1.0).abs() < 0.02, "found {:.1}", time.bpm);
        if let Err(e) = beats_match(&beats, &truth, 18.0, 23.5, 0.03) {
            panic!("after the change: {e}");
        }
    }

    /// The offset shifts when beats are reported, later or (predicted) earlier, but not
    /// where they fall in the music.
    #[test]
    fn the_offset_shifts_when_beats_are_heard() {
        let (frames, truth) = drums(16.0, |_| 120.0, 0.5, None, 3);
        for offset in [-0.1f32, 0.0, 0.15] {
            let mut a = Analyzer::new(RATE);
            a.set_beat_offset(offset);
            let mut beats = Vec::new();
            let mut lags = Vec::new();
            for chunk in frames.chunks(64) {
                a.process(chunk, 1.5);
                let before = beats.len();
                a.drain_beats(&mut beats);
                lags.extend(beats[before..].iter().map(|b: &Beat| a.features().time - b.time));
            }
            // Reported within a hop (and a chunk) of the beat plus the offset.
            let hop = crate::audio::analyzer::HOP as f32 / RATE as f32;
            assert!(!lags.is_empty() && lags.iter().all(|&l| (l - offset).abs() <= 2.0 * hop), "offset {offset}: lags {lags:?}");
            if let Err(e) = beats_match(&beats, &truth, 8.0, 15.5, 0.03) {
                panic!("offset {offset}: {e}");
            }
        }
    }

    /// Hits at random times have no beat: the tracker says so, and reports (almost) none.
    #[test]
    fn random_hits_have_no_beat() {
        let mut rng = SplitMix64::new(11);
        let n = 20 * RATE as usize;
        let mut out = vec![0.0f32; n];
        let mut t = 0.3f32;
        while t < 19.5 {
            let start = (t * RATE as f32) as usize;
            for k in 0..(0.1 * RATE as f32) as usize {
                out[start + k] += 0.5 * rng.signed() * (-(k as f32) / (0.01 * RATE as f32)).exp();
            }
            // Exponential gaps, 4 a second on average.
            t += -(1.0 - rng.next_f32()).max(1e-6).ln() / 4.0;
        }
        let frames: Vec<Frame> = out.iter().map(|&x| [x, x]).collect();
        let (beats, time) = track(&frames);
        let late = beats.iter().filter(|b| b.time > 8.0).count();
        assert!(!time.beating && late <= 4, "beating {}, confidence {:.2}, {late} beats after 8 s", time.beating, time.confidence);
    }
}
