//! The song's energy, 0..1: how loud it is now against its own range so far, for the
//! looks to follow the song's shape (see `looks::dynamics`). The levels the reactions
//! use are normalised by an AGC that brings a quiet passage back up within seconds;
//! this stays low through a whole breakdown.
//!
//! - **Now:** the momentary loudness (LUFS), through an envelope that rises over
//!   `RISE_S` and falls over `FALL_S`.
//! - **The range:** the song's quiet and loud (the `LOW` and `HIGH` quantiles of that
//!   envelope, sampled every `SAMPLE_S` over the last `HISTORY_S`), at least
//!   `MIN_RANGE_DB` wide. Until `SETTLE_S` of the song is heard the range means little
//!   (the intro is the loudest part so far), so the energy starts at `NEUTRAL` and
//!   eases over to it.
//! - **The song's shape on top:** lifted through a build-up, full right after a drop
//!   (fading over `DROP_HOLD_S`), dipped after a breakdown.
//! - **Silence** (a gap between songs) lets it fall, and forgets the range, so the next
//!   song is measured on its own.

use super::sections::{SectionKind, Sections};
use std::collections::VecDeque;

const RISE_S: f32 = 0.3;
const FALL_S: f32 = 1.5;
const SAMPLE_S: f32 = 0.5;
const HISTORY_S: f32 = 90.0;
const LOW: f32 = 0.1;
const HIGH: f32 = 0.95;
const MIN_RANGE_DB: f32 = 8.0;
const SETTLE_S: f32 = 30.0;
/// The energy where the looks are as they're set (see `looks::dynamics`).
pub const NEUTRAL: f32 = 0.7;
/// How much a full build-up adds.
const BUILD_LIFT: f32 = 0.35;
const DROP_HOLD_S: f32 = 8.0;
/// A breakdown takes off this share, recovering over `BREAKDOWN_S`.
const BREAKDOWN_DIP: f32 = 0.5;
const BREAKDOWN_S: f32 = 8.0;

pub struct Dynamics {
    /// The loudness envelope, LUFS (`None` until something is heard).
    now: Option<f32>,
    /// Its samples over the last `HISTORY_S`, and the time to the next; the quiet and
    /// loud quantiles of them (`LOW`, `HIGH`), worked out at each sample (in `sorted`,
    /// kept so analysing allocates nothing).
    history: VecDeque<f32>,
    sorted: Vec<f32>,
    next_sample: f32,
    quantiles: (f32, f32),
    /// Seconds of the song heard.
    heard: f32,
    energy: f32,
}

const SAMPLES: usize = (HISTORY_S / SAMPLE_S) as usize;

impl Default for Dynamics {
    fn default() -> Self {
        Self {
            now: None,
            history: VecDeque::with_capacity(SAMPLES + 1),
            sorted: Vec::with_capacity(SAMPLES + 1),
            next_sample: 0.0,
            quantiles: (0.0, 0.0),
            heard: 0.0,
            energy: 0.0,
        }
    }
}

impl Dynamics {
    /// One hop of `dt` seconds at `time` (the analyzer's clock): the momentary
    /// `loudness`, whether it's `silent`, and the sections so far. Returns the energy.
    pub fn update(&mut self, loudness: f32, silent: bool, sections: &Sections, time: f32, dt: f32) -> f32 {
        if silent {
            self.now = None;
            self.history.clear();
            self.heard = 0.0;
            self.energy *= (-dt / FALL_S).exp();
            return self.energy;
        }
        let now = match self.now {
            None => loudness,
            Some(n) => n + (loudness - n) * (1.0 - (-dt / if loudness > n { RISE_S } else { FALL_S }).exp()),
        };
        self.now = Some(now);
        self.heard += dt;
        self.next_sample -= dt;
        if self.next_sample <= 0.0 {
            self.next_sample += SAMPLE_S;
            self.history.push_back(now);
            if self.history.len() > SAMPLES {
                self.history.pop_front();
            }
            self.sorted.clear();
            self.sorted.extend(self.history.iter().copied());
            self.sorted.sort_unstable_by(f32::total_cmp);
            let quantile = |q: f32| self.sorted[((self.sorted.len() as f32 - 1.0) * q).round() as usize];
            self.quantiles = (quantile(LOW), quantile(HIGH));
        }
        let high = self.quantiles.1.max(now);
        let low = self.quantiles.0.min(high - MIN_RANGE_DB);
        let measured = ((now - low) / (high - low)).clamp(0.0, 1.0);
        let settled = (self.heard / SETTLE_S).min(1.0);
        let mut energy = NEUTRAL + (measured - NEUTRAL) * settled;

        energy += BUILD_LIFT * sections.buildup;
        if let Some(s) = sections.last {
            let age = (time - s.time).max(0.0);
            match s.kind {
                SectionKind::Drop => energy = energy.max((-age / DROP_HOLD_S).exp()),
                SectionKind::Breakdown => energy *= 1.0 - BREAKDOWN_DIP * (-age / BREAKDOWN_S).exp(),
                SectionKind::Change => {}
            }
        }
        self.energy = energy.clamp(0.0, 1.0);
        self.energy
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::sections::Section;

    const HOP: f32 = 0.01;

    /// Feed `seconds` of `loudness`, returning the last energy.
    fn play(d: &mut Dynamics, loudness: f32, seconds: f32, sections: &Sections, time: &mut f32) -> f32 {
        let mut e = 0.0;
        for _ in 0..(seconds / HOP) as usize {
            *time += HOP;
            e = d.update(loudness, false, sections, *time, HOP);
        }
        e
    }

    /// A quiet passage after a loud one stays low for as long as it lasts (the AGC'd
    /// levels would be back up within seconds), and the loud part comes back high.
    #[test]
    fn a_breakdown_stays_low() {
        let (mut d, mut t, none) = (Dynamics::default(), 0.0, Sections::default());
        let start = play(&mut d, -20.0, 1.0, &none, &mut t);
        assert!((start - NEUTRAL).abs() < 0.05, "an unsettled start is neutral: {start}");
        play(&mut d, -9.0, 30.0, &none, &mut t);
        play(&mut d, -20.0, 10.0, &none, &mut t);
        for _ in 0..3 {
            let quiet = play(&mut d, -20.0, 10.0, &none, &mut t);
            assert!(quiet < 0.15, "still quiet: {quiet}");
        }
        let loud = play(&mut d, -9.0, 5.0, &none, &mut t);
        assert!(loud > 0.9, "loud again: {loud}");
    }

    /// A steady song sits high (it's always at its own loudest), a build-up lifts a quiet
    /// part, a drop fills it right after, and a breakdown dips it.
    #[test]
    fn the_song_shape_moves_it() {
        let (mut d, mut t) = (Dynamics::default(), 0.0);
        let mut s = Sections::default();
        play(&mut d, -9.0, 40.0, &s, &mut t);
        let quiet = play(&mut d, -15.0, 5.0, &s, &mut t);
        s.buildup = 1.0;
        let building = play(&mut d, -15.0, 0.1, &s, &mut t);
        assert!(building > quiet + 0.3, "{quiet} building {building}");
        s.buildup = 0.0;
        s.last = Some(Section { kind: SectionKind::Drop, time: t });
        let dropped = play(&mut d, -15.0, 0.1, &s, &mut t);
        assert!(dropped > 0.95, "after a drop {dropped}");
        s.last = Some(Section { kind: SectionKind::Breakdown, time: t });
        let broken = play(&mut d, -9.0, 0.1, &s, &mut t);
        assert!(broken < 0.6, "after a breakdown {broken}");
    }

    /// Silence lets it fall and forgets the song's range.
    #[test]
    fn silence_forgets_the_song() {
        let (mut d, mut t, none) = (Dynamics::default(), 0.0, Sections::default());
        play(&mut d, -9.0, 40.0, &none, &mut t);
        for _ in 0..300 {
            d.update(-70.0, true, &none, t, HOP);
        }
        assert!(d.energy < 0.2);
        let next = play(&mut d, -25.0, 1.0, &none, &mut t);
        assert!((next - NEUTRAL).abs() < 0.05, "a quiet new song starts neutral: {next}");
    }
}
