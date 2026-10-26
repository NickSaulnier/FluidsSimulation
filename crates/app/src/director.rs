//! The director: picks visualizer looks (see `looks`) and changes them with the music,
//! through a fade to black. Pure logic: it reads the analysis and says when to apply
//! which look; the app does the applying.
//!
//! When it's active (full screen, auto on) it changes look on:
//! - **A new song:** at least `SONG_GAP_S` of silence, then `LISTEN_S` of sound.
//! - **The song's sections** (see `audio::sections`): a drop changes at once to an
//!   energetic look; a breakdown to a calm one; any other change of section makes a
//!   change due, once half the interval has passed.
//! - **A timer:** once `interval_s` has passed.
//!
//! Section changes wait at least `MIN_LOOK_S` into a look. The next look suits the
//! music's energy (calm looks for quiet passages, energetic ones for loud) and, of
//! those, is the one gone longest unseen, so every look keeps coming round. For a new
//! song that's the energy of its opening seconds, not the end of the song before.
//!
//! Changes land on bar lines: with a steady beat, the fade starts so the new look
//! arrives on the next downbeat (the fade shortened to the time left); without one, on
//! the next kick, or `KICK_WAIT_S` later if no kick comes (ambient music, say). Drops
//! don't wait: the drop has just happened.
//!
//! Its own picks skip the looks rated lowest, and keep to the favourites when asked
//! (`eligible`, unless none would do). `back` returns through the looks shown before;
//! `cut` changes without a fade.
//!
//! With auto off (the look locked) it still refreshes the look on new songs and at the
//! interval: the same look fades back in with a fresh scene. Left alone for long, dye
//! mixes to a uniform smear and solid fuel burns out.

use crate::audio::analyzer::AudioFeatures;
use crate::audio::sections::SectionKind;
use crate::looks::Look;

const FADE_OUT_S: f32 = 0.5;
const FADE_IN_S: f32 = 0.8;
const SONG_GAP_S: f32 = 2.0;
/// How long a new song is heard before its look is chosen.
const LISTEN_S: f32 = 2.0;
const MIN_LOOK_S: f32 = 15.0;
/// How long a change waits for a kick, without a steady beat.
const KICK_WAIT_S: f32 = 4.0;
/// How many looks back `back` can go.
const HISTORY: usize = 50;
/// The shortest fade-out when it's fitted to the bar.
const MIN_FADE_OUT_S: f32 = 0.15;
const SLOW_S: f32 = 8.0;
/// Slow loudness above this counts as energetic music.
const ENERGETIC: f32 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Fade {
    None,
    /// Fading out over `length` seconds (0: a cut, with no fade back in either).
    Out { t: f32, target: usize, length: f32 },
    In { t: f32 },
}

#[derive(Clone, Debug)]
pub struct Director {
    /// Change looks automatically (only in full screen).
    pub auto: bool,
    /// Change look at least this often, seconds.
    pub interval_s: f32,
    /// Pick only favourites when changing by itself.
    pub favourites_only: bool,
    /// The look on screen, if a look was applied.
    pub current: Option<usize>,
    elapsed: f32,
    silence: f32,
    slow: f32,
    /// A change is due (the timer ran out, or the music changed).
    due: bool,
    /// A change waiting for the bar line (or a kick): the look, and how long it's waited.
    waiting: Option<(usize, f32)>,
    /// Section changes the analysis has reported so far, as last seen.
    sections_seen: u32,
    fade: Fade,
    /// A new song began: seconds of it heard so far, and its loudness summed over them.
    listening: Option<(f32, f32)>,
    /// When each look was last applied, counted in looks applied (0: never).
    shown: Vec<u32>,
    applied: u32,
    /// The looks shown before the current one, oldest first, and whether the change
    /// under way is a step back through them (which isn't recorded).
    history: Vec<usize>,
    going_back: bool,
}

impl Default for Director {
    fn default() -> Self {
        Self {
            auto: true,
            interval_s: 90.0,
            favourites_only: false,
            current: None,
            elapsed: 0.0,
            silence: 0.0,
            slow: 0.0,
            due: false,
            waiting: None,
            sections_seen: 0,
            fade: Fade::None,
            listening: None,
            shown: Vec::new(),
            applied: 0,
            history: Vec::new(),
            going_back: false,
        }
    }
}

impl Director {
    /// Fade to look `target` (a manual choice, or the director's own).
    pub fn request(&mut self, target: usize) {
        self.fade_to(target, FADE_OUT_S);
    }

    /// Change to look `target` at once, without a fade.
    pub fn cut(&mut self, target: usize) {
        self.fade = Fade::Out { t: 0.0, target, length: 0.0 };
        self.waiting = None;
        self.going_back = false;
    }

    /// Fade back to the look shown before this one, if there was one (and step further
    /// back each time). Returns whether there was.
    pub fn back(&mut self) -> bool {
        if matches!(self.fade, Fade::Out { .. }) {
            return false;
        }
        let Some(target) = self.history.pop() else { return false };
        self.fade_to(target, FADE_OUT_S);
        self.going_back = true;
        true
    }

    /// The look the director would pick next (for a "next" key): the one gone longest
    /// unseen that suits the music, of those `eligible` (see `update`).
    pub fn pick(&self, looks: &[Look], eligible: &[bool]) -> usize {
        self.choose(looks, eligible, self.slow > ENERGETIC)
    }

    /// Fade out over `length` seconds to `target`.
    fn fade_to(&mut self, target: usize, length: f32) {
        if !matches!(self.fade, Fade::Out { .. }) {
            self.fade = Fade::Out { t: 0.0, target, length };
            self.waiting = None;
            self.going_back = false;
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
        if let Fade::Out { t, target, length } = self.fade {
            self.fade = match now(target) {
                Some(target) => Fade::Out { t, target, length },
                None => Fade::None,
            };
        }
        self.waiting = self.waiting.and_then(|(target, waited)| Some((now(target)?, waited)));
        let mut shown = vec![0; looks.len()];
        for (i, &when) in self.shown.iter().enumerate() {
            if let Some(j) = now(i) {
                shown[j] = when;
            }
        }
        self.shown = shown;
        self.history = self.history.iter().filter_map(|&i| now(i)).collect();
    }

    /// Seconds since the current look was applied.
    pub fn elapsed(&self) -> f32 {
        self.elapsed
    }

    /// How dark the fade is: 0 (none) to 1 (black).
    pub fn darkness(&self) -> f32 {
        match self.fade {
            Fade::None => 0.0,
            Fade::Out { length, .. } if length <= 0.0 => 0.0,
            Fade::Out { t, length, .. } => (t / length).min(1.0),
            Fade::In { t } => 1.0 - (t / FADE_IN_S).min(1.0),
        }
    }

    /// Advance `dt` seconds, choosing among `looks`. `presenting`: in full screen, where
    /// the director acts by itself (changing looks with `auto`, refreshing the locked one
    /// without). `features`: the analysis (none without a source); `kicked`: a kick this
    /// frame. `eligible`: which looks it may pick by itself (by index; any not listed
    /// may). Returns the look to apply now, at the darkest point of a fade.
    pub fn update(&mut self, looks: &[Look], eligible: &[bool], presenting: bool, features: Option<&AudioFeatures>, kicked: bool, dt: f32) -> Option<usize> {
        self.elapsed += dt;
        let level = features.map_or(0.0, |f| f.level);
        self.slow += (level - self.slow) * (1.0 - (-dt / SLOW_S).exp());
        // A new section since last frame (a new analyzer starts its count again).
        let sections = features.map(|f| f.sections).unwrap_or_default();
        if sections.changes < self.sections_seen {
            self.sections_seen = sections.changes;
        }
        let section = (sections.changes > self.sections_seen).then_some(sections.last).flatten().map(|s| s.kind);
        self.sections_seen = sections.changes;
        let silent = features.is_none_or(|f| f.silent);
        let song_change = !silent && self.silence >= SONG_GAP_S;
        self.silence = if silent { self.silence + dt } else { 0.0 };

        match self.fade {
            Fade::Out { t, target, length } => {
                let t = t + dt;
                if t >= length {
                    self.fade = if length > 0.0 { Fade::In { t: 0.0 } } else { Fade::None };
                    if target >= looks.len() {
                        return None;
                    }
                    if let Some(left) = self.current.filter(|&c| c != target && !self.going_back) {
                        self.history.push(left);
                        if self.history.len() > HISTORY {
                            self.history.remove(0);
                        }
                    }
                    self.going_back = false;
                    (self.current, self.elapsed, self.due) = (Some(target), 0.0, false);
                    self.applied += 1;
                    self.shown.resize(looks.len(), 0);
                    self.shown[target] = self.applied;
                    return Some(target);
                }
                self.fade = Fade::Out { t, target, length };
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
                && self.waiting.is_none()
            {
                self.waiting = Some((current, 0.0));
            }
            self.land_on_the_bar(features, kicked, dt);
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
                let target = self.choose(looks, eligible, sum / heard > ENERGETIC);
                self.request(target);
                None
            } else {
                Some((heard, sum))
            };
            return None;
        }
        match section {
            Some(SectionKind::Drop) if settled => {
                // The drop has just happened: change now.
                let target = self.choose(looks, eligible, true);
                self.request(target);
                return None;
            }
            Some(SectionKind::Breakdown) if settled => self.waiting = Some((self.choose(looks, eligible, false), 0.0)),
            Some(SectionKind::Change) if self.elapsed >= MIN_LOOK_S.max(0.5 * self.interval_s) => self.due = true,
            _ => {}
        }
        if self.due && self.waiting.is_none() {
            self.waiting = Some((self.choose(looks, eligible, self.slow > ENERGETIC), 0.0));
        }
        self.land_on_the_bar(features, kicked, dt);
        None
    }

    /// Start a waiting change when it will land on the bar line: with a steady beat,
    /// fading out over what's left of the bar once that's a fade's length; without one,
    /// on a kick, or after waiting a while.
    fn land_on_the_bar(&mut self, features: Option<&AudioFeatures>, kicked: bool, dt: f32) {
        let Some((target, waited)) = self.waiting else { return };
        let waited = waited + dt;
        let beat = features.map(|f| f.musical).filter(|m| m.beating && m.bpm > 0.0);
        let fade = match beat {
            Some(m) => {
                let bar = 4.0 * 60.0 / m.bpm;
                let left = (1.0 - m.bar_phase) * bar;
                // A bar and a half with no line found (the beat lost): go anyway.
                (left <= FADE_OUT_S || waited > 1.5 * bar).then_some(left.clamp(MIN_FADE_OUT_S, FADE_OUT_S))
            }
            None => (kicked || waited >= KICK_WAIT_S).then_some(FADE_OUT_S),
        };
        match fade {
            Some(length) => self.fade_to(target, length),
            None => self.waiting = Some((target, waited)),
        }
    }

    /// The look gone longest unseen that suits the music's energy, of the `eligible`
    /// ones if any suit; ties go to the first after the current one.
    fn choose(&self, looks: &[Look], eligible: &[bool], energetic: bool) -> usize {
        let n = looks.len().max(1);
        let start = self.current.map_or(0, |c| c + 1);
        let pick = |strict: bool| {
            (0..looks.len())
                .map(|k| (start + k) % n)
                .filter(|&i| Some(i) != self.current && looks[i].suits(energetic) && (!strict || eligible.get(i).copied().unwrap_or(true)))
                .min_by_key(|&i| self.shown.get(i).copied().unwrap_or(0))
        };
        pick(true).or_else(|| pick(false)).unwrap_or(start % n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::sections::SectionKind;
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
        (0..frames).filter_map(|k| d.update(looks(), &[], true, Some(f), kick_every.is_some_and(|n| k % n == 0), FRAME)).collect()
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
            let target = d.choose(looks(), &[], true);
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

    /// Music reporting `changes` section changes, the last of `kind`.
    fn sectioned(level: f32, changes: u32, kind: SectionKind) -> AudioFeatures {
        let mut f = music(level);
        f.sections.changes = changes;
        f.sections.last = Some(crate::audio::sections::Section { kind, time: 0.0 });
        f
    }

    /// Music with a steady beat at `bpm`, `t` seconds in (bars from 0).
    fn beating(level: f32, bpm: f32, t: f32) -> AudioFeatures {
        let mut f = music(level);
        let bar = 4.0 * 60.0 / bpm;
        f.musical = crate::audio::tempo::MusicalTime { bpm, confidence: 0.5, beating: true, bar_phase: (t / bar).fract(), ..Default::default() };
        f
    }

    #[test]
    fn a_drop_changes_at_once_to_an_energetic_look() {
        let mut d = settled(0.2);
        let mut applied = run(&mut d, &sectioned(0.2, 1, SectionKind::Drop), None, 1.0);
        applied.extend(run(&mut d, &sectioned(0.2, 1, SectionKind::Drop), None, 1.0));
        assert_eq!(applied.len(), 1, "{applied:?}");
        assert!(looks()[applied[0]].suits(true) && applied[0] != 0, "{}", looks()[applied[0]].name);
    }

    #[test]
    fn a_breakdown_changes_to_a_calm_look() {
        let mut d = settled(0.8);
        let mut applied = run(&mut d, &sectioned(0.8, 1, SectionKind::Breakdown), Some(30), 1.0);
        applied.extend(run(&mut d, &sectioned(0.8, 1, SectionKind::Breakdown), Some(30), 1.0));
        assert_eq!(applied.len(), 1, "{applied:?}");
        assert!(looks()[applied[0]].suits(false), "{} isn't calm", looks()[applied[0]].name);
    }

    /// A plain change of section early in a look waits; past half the interval it changes.
    #[test]
    fn a_plain_change_counts_past_half_the_interval() {
        let mut d = settled(0.3);
        assert!(run(&mut d, &sectioned(0.3, 1, SectionKind::Change), Some(30), 2.0).is_empty(), "changed 22 s into a 90 s interval");
        run(&mut d, &music(0.3), None, 30.0);
        assert_eq!(run(&mut d, &sectioned(0.3, 2, SectionKind::Change), Some(30), 2.0).len(), 1, "didn't change past half the interval");
    }

    /// With a steady beat, the timer's change switches on a downbeat.
    #[test]
    fn changes_land_on_the_bar_line() {
        for bpm in [90.0, 128.0, 174.0] {
            let mut d = settled(0.3);
            let bar = 4.0 * 60.0 / bpm;
            let mut t = 0.37; // not on a bar line
            let mut switched = None;
            for _ in 0..(80.0 / FRAME) as usize {
                if d.update(looks(), &[], true, Some(&beating(0.3, bpm, t)), false, FRAME).is_some() {
                    switched = Some(t);
                    break;
                }
                t += FRAME;
            }
            let t = switched.unwrap_or_else(|| panic!("{bpm} BPM: never changed"));
            let off = ((t / bar).round() - t / bar).abs() * bar;
            assert!(off <= 1.5 * FRAME, "{bpm} BPM: switched {:.0} ms from a bar line", off * 1e3);
        }
    }

    #[test]
    fn energy_picks_suitable_looks() {
        let d = Director { current: Some(0), ..Director::default() };
        assert!(looks()[d.choose(looks(), &[], true)].suits(true));
        assert!(looks()[d.choose(looks(), &[], false)].suits(false));
    }

    #[test]
    fn inactive_director_only_follows_requests() {
        let mut d = Director { current: Some(0), ..Director::default() };
        for _ in 0..(200.0 / FRAME) as usize {
            assert_eq!(d.update(looks(), &[], false, Some(&music(0.5)), true, FRAME), None);
        }
        d.request(3);
        let applied: Vec<usize> = (0..120).filter_map(|_| d.update(looks(), &[], false, None, false, FRAME)).collect();
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
            if d.update(looks(), &[], false, None, false, FRAME).is_some() {
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

    /// Its own picks keep to the eligible looks while any suit; with none, any will do.
    #[test]
    fn picks_keep_to_eligible_looks() {
        let d = Director { current: Some(0), ..Director::default() };
        let favourite = looks().iter().rposition(|l| l.suits(true)).unwrap();
        let only: Vec<bool> = (0..looks().len()).map(|i| i == favourite).collect();
        assert_eq!(d.choose(looks(), &only, true), favourite);
        let none = vec![false; looks().len()];
        assert!(looks()[d.choose(looks(), &none, true)].suits(true));
    }

    /// Back steps through the looks shown, most recent first; a cut changes at once.
    #[test]
    fn back_returns_through_the_looks_shown() {
        let mut d = Director { current: Some(0), ..Director::default() };
        let go = |d: &mut Director| (0..120).filter_map(|_| d.update(looks(), &[], false, None, false, FRAME)).last();
        for target in [3, 5] {
            d.request(target);
            go(&mut d);
        }
        assert!(d.back());
        assert_eq!(go(&mut d), Some(3));
        assert!(d.back());
        assert_eq!(go(&mut d), Some(0));
        assert!(!d.back(), "nothing further back");
        d.cut(7);
        assert_eq!(d.update(looks(), &[], false, None, false, FRAME), Some(7));
        assert_eq!(d.darkness(), 0.0, "a cut doesn't fade");
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
