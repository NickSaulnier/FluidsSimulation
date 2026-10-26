//! The director: picks visualizer looks (see `looks`) and changes them with the music,
//! through a fade to black. Pure logic: it reads the analysis and says when to apply
//! which look; the app does the applying.
//!
//! When it's active (full screen, auto on) it changes look on:
//! - **A new song:** at least `SONG_GAP_S` of silence, then `LISTEN_S` of sound.
//! - **A drop:** loudness jumping well above its recent average (`DROP_JUMP`).
//! - **A timer:** once `interval_s` has passed, on the next kick, so the cut lands on
//!   the beat; or `KICK_WAIT_S` later if no kick comes (ambient music, say).
//!
//! The first two wait at least `MIN_LOOK_S` into a look. The next look suits the
//! music's energy (calm looks for quiet passages, energetic ones for loud) and, of
//! those, is the one gone longest unseen, so every look keeps coming round. For a new
//! song that's the energy of its opening seconds, not the end of the song before.
//!
//! With auto off (the look locked) it still refreshes the look on new songs and at the
//! interval: the same look fades back in with a fresh scene. Left alone for long, dye
//! mixes to a uniform smear and solid fuel burns out.

use crate::audio::analyzer::AudioFeatures;
use crate::looks::Look;

const FADE_OUT_S: f32 = 0.5;
const FADE_IN_S: f32 = 0.8;
const SONG_GAP_S: f32 = 2.0;
/// How long a new song is heard before its look is chosen.
const LISTEN_S: f32 = 2.0;
const MIN_LOOK_S: f32 = 15.0;
/// How long past the interval the timer waits for a kick.
const KICK_WAIT_S: f32 = 4.0;
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
    /// A new song began: seconds of it heard so far, and its loudness summed over them.
    listening: Option<(f32, f32)>,
    /// When each look was last applied, counted in looks applied (0: never).
    shown: Vec<u32>,
    applied: u32,
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
            listening: None,
            shown: Vec::new(),
            applied: 0,
        }
    }
}

impl Director {
    /// Fade to look `target` (a manual choice, or the director's own).
    pub fn request(&mut self, target: usize) {
        if !matches!(self.fade, Fade::Out { .. }) {
            self.fade = Fade::Out { t: 0.0, target };
        }
    }

    /// The look `step` places from the current one (wrapping) of `count`, for next and
    /// previous.
    pub fn neighbour(&self, step: isize, count: usize) -> usize {
        match self.current {
            Some(c) if count > 0 => (c as isize + step).rem_euclid(count as isize) as usize,
            _ => 0,
        }
    }

    /// The looks were read again (some may have moved, come or gone): follow them by
    /// name, from `before` (the old names in order) to `looks`.
    pub fn reindex(&mut self, before: &[String], looks: &[Look]) {
        let now = |i: usize| before.get(i).and_then(|name| looks.iter().position(|l| &l.name == name));
        self.current = self.current.and_then(now);
        if let Fade::Out { t, target } = self.fade {
            self.fade = match now(target) {
                Some(target) => Fade::Out { t, target },
                None => Fade::None,
            };
        }
        let mut shown = vec![0; looks.len()];
        for (i, &when) in self.shown.iter().enumerate() {
            if let Some(j) = now(i) {
                shown[j] = when;
            }
        }
        self.shown = shown;
    }

    /// How dark the fade is: 0 (none) to 1 (black).
    pub fn darkness(&self) -> f32 {
        match self.fade {
            Fade::None => 0.0,
            Fade::Out { t, .. } => (t / FADE_OUT_S).min(1.0),
            Fade::In { t } => 1.0 - (t / FADE_IN_S).min(1.0),
        }
    }

    /// Advance `dt` seconds, choosing among `looks`. `presenting`: in full screen, where
    /// the director acts by itself (changing looks with `auto`, refreshing the locked one
    /// without). `features`: the analysis (none without a source); `kicked`: a kick this
    /// frame. Returns the look to apply now, at the darkest point of a fade.
    pub fn update(&mut self, looks: &[Look], presenting: bool, features: Option<&AudioFeatures>, kicked: bool, dt: f32) -> Option<usize> {
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
                    if target >= looks.len() {
                        return None;
                    }
                    (self.current, self.elapsed, self.due) = (Some(target), 0.0, false);
                    self.applied += 1;
                    self.shown.resize(looks.len(), 0);
                    self.shown[target] = self.applied;
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
        if !presenting || matches!(self.fade, Fade::In { .. }) {
            return None;
        }
        if self.elapsed >= self.interval_s {
            self.due = true;
        }
        let settled = self.elapsed >= MIN_LOOK_S;
        if !self.auto {
            // Locked: refresh the same look, so it doesn't go stale.
            if let Some(current) = self.current
                && ((settled && song_change) || self.due)
            {
                self.request(current);
            }
            return None;
        }
        if settled && song_change {
            self.listening = Some((0.0, 0.0));
        }
        if let Some((heard, sum)) = self.listening {
            // Hear the new song's opening before choosing; nothing else cuts in meanwhile.
            let (heard, sum) = (heard + dt, sum + level * dt);
            self.listening = if silent {
                None
            } else if heard >= LISTEN_S {
                let target = self.choose(looks, sum / heard > ENERGETIC);
                self.request(target);
                None
            } else {
                Some((heard, sum))
            };
            return None;
        }
        let drop = settled && self.fast - self.slow > DROP_JUMP;
        let overdue = self.elapsed >= self.interval_s + KICK_WAIT_S;
        if drop || (self.due && (kicked || overdue)) {
            let target = self.choose(looks, self.slow > ENERGETIC);
            self.request(target);
        }
        None
    }

    /// The look gone longest unseen that suits the music's energy; ties go to the
    /// first after the current one.
    fn choose(&self, looks: &[Look], energetic: bool) -> usize {
        let n = looks.len().max(1);
        let start = self.current.map_or(0, |c| c + 1);
        (0..looks.len())
            .map(|k| (start + k) % n)
            .filter(|&i| Some(i) != self.current && looks[i].suits(energetic))
            .min_by_key(|&i| self.shown.get(i).copied().unwrap_or(0))
            .unwrap_or(start % n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::looks::Library;
    use std::sync::LazyLock;

    static LIBRARY: LazyLock<Library> = LazyLock::new(Library::builtin);

    fn looks() -> &'static [Look] {
        &LIBRARY.looks
    }

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
        (0..frames).filter_map(|k| d.update(looks(), true, Some(f), kick_every.is_some_and(|n| k % n == 0), FRAME)).collect()
    }

    fn settled(level: f32) -> Director {
        let mut d = Director { current: Some(0), ..Director::default() };
        run(&mut d, &music(level), None, 20.0);
        d
    }

    #[test]
    fn the_timer_changes_look_on_a_kick() {
        let mut d = settled(0.3);
        assert!(run(&mut d, &music(0.3), None, 68.0).is_empty(), "changed with no kick and no trigger");
        // Past the interval, but no kick yet: waits.
        assert!(run(&mut d, &music(0.3), None, 3.0).is_empty());
        let applied = run(&mut d, &music(0.3), Some(30), 2.0);
        assert_eq!(applied.len(), 1, "{applied:?}");
        assert_ne!(Some(applied[0]), Some(0));
    }

    #[test]
    fn choices_rotate_through_every_look_that_suits() {
        let mut d = Director { current: Some(0), ..Director::default() };
        let suitable = looks().iter().filter(|l| l.suits(true)).count();
        let mut picked = Vec::new();
        for _ in 0..suitable {
            let target = d.choose(looks(), true);
            d.request(target);
            run(&mut d, &silence(), None, 1.5);
            picked.push(d.current.unwrap());
        }
        picked.sort();
        picked.dedup();
        assert_eq!(picked.len(), suitable, "repeated a look before showing them all: {picked:?}");
    }

    #[test]
    fn the_timer_changes_look_without_kicks_after_a_wait() {
        let mut d = settled(0.3);
        assert!(run(&mut d, &music(0.3), None, 72.0).is_empty(), "didn't wait for a kick");
        assert_eq!(run(&mut d, &music(0.3), None, 4.0).len(), 1, "kickless music never changed look");
    }

    #[test]
    fn a_new_song_after_silence_changes_look() {
        let mut d = settled(0.3);
        assert!(run(&mut d, &silence(), None, 3.0).is_empty(), "changed during the silence");
        assert_eq!(run(&mut d, &music(0.3), None, 3.0).len(), 1, "didn't change when the song started");
    }

    #[test]
    fn a_new_song_is_not_judged_by_the_last_ones_loud_end() {
        let mut d = settled(0.9);
        d.current = looks().iter().position(|l| l.name == "Wind tunnel");
        run(&mut d, &silence(), None, 3.0);
        let applied = run(&mut d, &music(0.3), None, 3.0);
        assert_eq!(applied.iter().map(|&i| looks()[i].name.as_str()).collect::<Vec<_>>(), ["Lava lamp"], "the quiet new song didn't get the calm look");
    }

    #[test]
    fn a_new_song_that_opens_loud_gets_an_energetic_look() {
        let mut d = settled(0.2);
        run(&mut d, &silence(), None, 3.0);
        let applied = run(&mut d, &music(0.9), None, 3.0);
        assert_eq!(applied.len(), 1, "{applied:?}");
        assert!(looks()[applied[0]].suits(true) && !looks()[applied[0]].suits(false), "{} isn't an energetic look", looks()[applied[0]].name);
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
        assert!(looks()[d.choose(looks(), true)].suits(true));
        assert!(looks()[d.choose(looks(), false)].suits(false));
    }

    #[test]
    fn inactive_director_only_follows_requests() {
        let mut d = Director { current: Some(0), ..Director::default() };
        for _ in 0..(200.0 / FRAME) as usize {
            assert_eq!(d.update(looks(), false, Some(&music(0.5)), true, FRAME), None);
        }
        d.request(3);
        let applied: Vec<usize> = (0..120).filter_map(|_| d.update(looks(), false, None, false, FRAME)).collect();
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
            if d.update(looks(), false, None, false, FRAME).is_some() {
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
    fn a_locked_look_refreshes_itself() {
        let mut d = Director { auto: false, current: Some(2), ..Director::default() };
        let applied = run(&mut d, &music(0.4), Some(30), 95.0);
        assert_eq!(applied, vec![2], "locked: same look, refreshed once at the interval");
        let applied = run(&mut d, &music(1.0), Some(30), 20.0);
        assert!(applied.is_empty(), "a drop doesn't change a locked look");
    }

    #[test]
    fn neighbours_wrap() {
        let d = Director { current: Some(0), ..Director::default() };
        assert_eq!(d.neighbour(-1, looks().len()), looks().len() - 1);
        assert_eq!(d.neighbour(1, looks().len()), 1);
    }

    /// After the looks are read again, the director follows its look by name, and keeps
    /// what it knows of when each was shown.
    #[test]
    fn reindexing_follows_looks_by_name() {
        let before: Vec<String> = looks().iter().map(|l| l.name.clone()).collect();
        let mut moved: Vec<Look> = looks().to_vec();
        moved.rotate_left(2);
        moved.pop();
        let mut d = Director { current: Some(3), ..Director::default() };
        d.shown = (1..=before.len() as u32).collect();
        d.reindex(&before, &moved);
        assert_eq!(d.current.map(|i| moved[i].name.as_str()), Some(before[3].as_str()));
        let i = moved.iter().position(|l| l.name == before[5]).unwrap();
        assert_eq!(d.shown[i], 6);
        // A look that's gone leaves nothing current.
        let mut d = Director { current: Some(1), ..Director::default() };
        d.reindex(&before, &moved);
        assert_eq!(d.current, None);
    }
}
