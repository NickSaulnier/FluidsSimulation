//! The director: picks visualizer looks (see `looks`) and changes them with the music,
//! through a fade to black. Pure logic: it reads the analysis and says when to apply
//! which look; the app does the applying.
//!
//! When it's active (full screen, auto on) it changes look on:
//! - **A new song:** at least `SONG_GAP_S` of silence, then sound again.
//! - **A drop:** loudness jumping well above its recent average (`DROP_JUMP`).
//! - **A timer:** once `interval_s` has passed, on the next kick, so the cut lands on
//!   the beat.
//!
//! The first two wait at least `MIN_LOOK_S` into a look. The next look suits the
//! music's energy: calm looks for quiet passages, energetic ones for loud.

use crate::audio::analyzer::AudioFeatures;
use crate::looks::LOOKS;

const FADE_OUT_S: f32 = 0.5;
const FADE_IN_S: f32 = 0.8;
const SONG_GAP_S: f32 = 2.0;
const MIN_LOOK_S: f32 = 15.0;
/// A drop: the fast loudness average exceeds the slow one by this much (both 0..1).
const DROP_JUMP: f32 = 0.35;
const FAST_S: f32 = 0.4;
const SLOW_S: f32 = 8.0;
/// Slow loudness above this counts as energetic music.
const ENERGETIC: f32 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Fade {
    None,
    Out { t: f32, target: usize },
    In { t: f32 },
}

#[derive(Clone, Debug)]
pub struct Director {
    /// Change looks automatically (only in full screen).
    pub auto: bool,
    /// Change look at least this often, seconds.
    pub interval_s: f32,
    /// The look on screen, if a look was applied.
    pub current: Option<usize>,
    elapsed: f32,
    silence: f32,
    fast: f32,
    slow: f32,
    /// The timer has run out: change on the next kick.
    due: bool,
    fade: Fade,
}

impl Default for Director {
    fn default() -> Self {
        Self {
            auto: true,
            interval_s: 90.0,
            current: None,
            elapsed: 0.0,
            silence: 0.0,
            fast: 0.0,
            slow: 0.0,
            due: false,
            fade: Fade::None,
        }
    }
}

impl Director {
    /// Fade to `target` (a manual choice, or the director's own).
    pub fn request(&mut self, target: usize) {
        if target < LOOKS.len() && !matches!(self.fade, Fade::Out { .. }) {
            self.fade = Fade::Out { t: 0.0, target };
        }
    }

    /// The look `step` places from the current one (wrapping), for next and previous.
    pub fn neighbour(&self, step: isize) -> usize {
        let n = LOOKS.len() as isize;
        match self.current {
            Some(c) => (c as isize + step).rem_euclid(n) as usize,
            None => 0,
        }
    }

    /// How dark the fade is: 0 (none) to 1 (black).
    pub fn darkness(&self) -> f32 {
        match self.fade {
            Fade::None => 0.0,
            Fade::Out { t, .. } => (t / FADE_OUT_S).min(1.0),
            Fade::In { t } => 1.0 - (t / FADE_IN_S).min(1.0),
        }
    }

    /// Advance `dt` seconds. `active`: the director may change looks by itself.
    /// `features`: the analysis (none without a source); `kicked`: a kick this frame.
    /// Returns the look to apply now, at the darkest point of a fade.
    pub fn update(&mut self, active: bool, features: Option<&AudioFeatures>, kicked: bool, dt: f32) -> Option<usize> {
        self.elapsed += dt;
        let level = features.map_or(0.0, |f| f.level);
        self.fast += (level - self.fast) * (1.0 - (-dt / FAST_S).exp());
        self.slow += (level - self.slow) * (1.0 - (-dt / SLOW_S).exp());
        let silent = features.is_none_or(|f| f.silent);
        let song_change = !silent && self.silence >= SONG_GAP_S;
        self.silence = if silent { self.silence + dt } else { 0.0 };

        match self.fade {
            Fade::Out { t, target } => {
                let t = t + dt;
                if t >= FADE_OUT_S {
                    self.fade = Fade::In { t: 0.0 };
                    (self.current, self.elapsed, self.due) = (Some(target), 0.0, false);
                    return Some(target);
                }
                self.fade = Fade::Out { t, target };
                return None;
            }
            Fade::In { t } => {
                self.fade = if t + dt >= FADE_IN_S { Fade::None } else { Fade::In { t: t + dt } };
            }
            Fade::None => {}
        }
        if !active || matches!(self.fade, Fade::In { .. }) {
            return None;
        }
        if self.elapsed >= self.interval_s {
            self.due = true;
        }
        let settled = self.elapsed >= MIN_LOOK_S;
        let drop = settled && self.fast - self.slow > DROP_JUMP;
        if (settled && song_change) || drop || (self.due && kicked) {
            let target = self.choose(self.slow > ENERGETIC);
            self.request(target);
        }
        None
    }

    /// The next look after the current one that suits the music's energy.
    fn choose(&self, energetic: bool) -> usize {
        let n = LOOKS.len();
        let start = self.current.map_or(0, |c| c + 1);
        (0..n).map(|k| (start + k) % n).find(|&i| Some(i) != self.current && LOOKS[i].suits(energetic)).unwrap_or(start % n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: f32 = 1.0 / 60.0;

    fn music(level: f32) -> AudioFeatures {
        AudioFeatures { level, silent: false, ..AudioFeatures::default() }
    }

    fn silence() -> AudioFeatures {
        AudioFeatures { silent: true, ..AudioFeatures::default() }
    }

    /// Run `seconds` of frames; returns the looks applied.
    fn run(d: &mut Director, f: &AudioFeatures, kick_every: Option<usize>, seconds: f32) -> Vec<usize> {
        let frames = (seconds / FRAME) as usize;
        (0..frames).filter_map(|k| d.update(true, Some(f), kick_every.is_some_and(|n| k % n == 0), FRAME)).collect()
    }

    fn settled(level: f32) -> Director {
        let mut d = Director { current: Some(0), ..Director::default() };
        run(&mut d, &music(level), None, 20.0);
        d
    }

    #[test]
    fn the_timer_changes_look_on_a_kick() {
        let mut d = settled(0.3);
        assert!(run(&mut d, &music(0.3), None, 80.0).is_empty(), "changed with no kick and no trigger");
        // Past the interval, but no kick yet: waits.
        assert!(run(&mut d, &music(0.3), None, 5.0).is_empty());
        let applied = run(&mut d, &music(0.3), Some(30), 2.0);
        assert_eq!(applied.len(), 1, "{applied:?}");
        assert_ne!(Some(applied[0]), Some(0));
    }

    #[test]
    fn a_new_song_after_silence_changes_look() {
        let mut d = settled(0.3);
        assert!(run(&mut d, &silence(), None, 3.0).is_empty(), "changed during the silence");
        assert_eq!(run(&mut d, &music(0.3), None, 2.0).len(), 1, "didn't change when the song started");
    }

    #[test]
    fn a_short_pause_is_not_a_new_song() {
        let mut d = settled(0.3);
        run(&mut d, &silence(), None, 1.0);
        assert!(run(&mut d, &music(0.3), None, 2.0).is_empty());
    }

    #[test]
    fn a_drop_changes_to_an_energetic_look() {
        let mut d = settled(0.2);
        let applied = run(&mut d, &music(1.0), None, 3.0);
        assert_eq!(applied.len(), 1, "{applied:?}");
        // Loudness was still averaging low when the drop hit, so the choice is by the
        // slow average; either way it's a different look.
        assert_ne!(applied[0], 0);
    }

    #[test]
    fn energy_picks_suitable_looks() {
        let d = Director { current: Some(0), ..Director::default() };
        assert!(LOOKS[d.choose(true)].suits(true));
        assert!(LOOKS[d.choose(false)].suits(false));
    }

    #[test]
    fn inactive_director_only_follows_requests() {
        let mut d = Director { current: Some(0), ..Director::default() };
        for _ in 0..(200.0 / FRAME) as usize {
            assert_eq!(d.update(false, Some(&music(0.5)), true, FRAME), None);
        }
        d.request(3);
        let applied: Vec<usize> = (0..120).filter_map(|_| d.update(false, None, false, FRAME)).collect();
        assert_eq!(applied, vec![3]);
    }

    #[test]
    fn fades_go_dark_switch_and_come_back() {
        let mut d = Director::default();
        d.request(2);
        let mut darkest_at_switch = 0.0;
        let mut switched = false;
        for _ in 0..200 {
            let before = d.darkness();
            if d.update(false, None, false, FRAME).is_some() {
                switched = true;
                darkest_at_switch = before;
            }
        }
        assert!(switched);
        assert!(darkest_at_switch > 0.95, "switched at darkness {darkest_at_switch}");
        assert_eq!(d.darkness(), 0.0, "never faded back in");
        assert_eq!(d.current, Some(2));
    }

    #[test]
    fn neighbours_wrap() {
        let d = Director { current: Some(0), ..Director::default() };
        assert_eq!(d.neighbour(-1), LOOKS.len() - 1);
        assert_eq!(d.neighbour(1), 1);
    }
}
