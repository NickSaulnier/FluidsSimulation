//! The director: picks visualizer looks (see `looks`) and changes them with the music.
//! Pure logic: it reads the analysis and says when to apply which look, and how far a
//! blend has gone; the app does the applying and the drawing.
//!
//! When it's active (full screen, auto on) it changes look on:
//! - **A new song:** when the app says so (`new_song`: the media session reported one,
//!   or a track was loaded), or else guessed from at least `SONG_GAP_S` of silence; then
//!   after `LISTEN_S` of sound.
//! - **The song's sections** (see `audio::sections`): a drop cuts to an energetic look;
//!   a breakdown blends slowly into a calm one; any other change of section moves to
//!   the look's next variant (see `looks::Variant`), blended over a bar, if it has
//!   variants and the last move was at least `MIN_VARIANT_S` ago, or else makes a change
//!   due, once half the interval has passed. A variant keeps the look's fluid, reactions
//!   and equations, and the look's own time (the interval runs on).
//! - **A timer:** once `interval_s` has passed.
//!
//! Section changes wait at least `MIN_LOOK_S` into a look. The next look suits the
//! music's energy (calm looks for quiet passages, energetic ones for loud) and, of
//! those, is the one gone longest unseen, so every look keeps coming round. For a new
//! song that's the energy of its opening seconds, not the end of the song before. Its
//! own picks are weighted (`weights`, from the ratings): a well-rated look comes round
//! sooner, a look weighted 0 (one star, or not a favourite or tagged when those are
//! asked for) only if no other would do.
//!
//! **Blends:** the new look takes over from a still of the old picture over `BLEND_S`
//! (`SLOW_BLEND_S` into a breakdown), through a pattern the app draws (see
//! `render::composite::Blend`). While a change is on its way the app captures the
//! picture every frame (`capturing`, then `captured`), so the still is the last frame
//! before the switch. **Cuts** switch with no blend: a drop (it has already happened),
//! and the hard-cut key.
//!
//! Changes land on the music: with a steady beat, a blend's new look arrives on the
//! next downbeat and a drop's cut on the next beat; without one, on the next kick, or
//! `KICK_WAIT_S` later if no kick comes (ambient music, say), and a cut at once.
//!
//! With auto off (the look locked) it still refreshes the look on new songs and at the
//! interval: the same look blends back in with a fresh scene. Left alone for long, dye
//! mixes to a uniform smear and solid fuel burns out. `back` returns through the looks
//! shown before.

use crate::audio::analyzer::AudioFeatures;
use crate::audio::sections::SectionKind;
use crate::looks::Look;

/// How long a blend takes, and into a breakdown.
pub const BLEND_S: f32 = 2.5;
pub const SLOW_BLEND_S: f32 = 5.0;
/// A change starts getting ready (the old picture captured) this long before its
/// moment.
const LEAD_S: f32 = 0.5;
/// If the app captures nothing (it drew no frame), a change goes anyway this late.
const CAPTURE_WAIT_S: f32 = 0.25;
const SONG_GAP_S: f32 = 2.0;
/// How long a new song is heard before its look is chosen.
const LISTEN_S: f32 = 2.0;
const MIN_LOOK_S: f32 = 15.0;
/// The least time between moves to another variant (and from a look's start).
const MIN_VARIANT_S: f32 = 8.0;
/// How long a change waits for a kick, without a steady beat.
const KICK_WAIT_S: f32 = 4.0;
/// How many looks back `back` can go.
const HISTORY: usize = 50;
const SLOW_S: f32 = 8.0;
/// Slow loudness above this counts as energetic music.
const ENERGETIC: f32 = 0.5;

/// How a change looks: a blend over so many seconds, or a cut.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Transition {
    Blend(f32),
    Cut,
}

/// A look to apply now, and whether by a cut (no blend); or, with `variant`, only a move
/// to that variant of the look on screen (its picture: 0 the look's own).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change {
    pub look: usize,
    pub cut: bool,
    pub variant: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Fade {
    None,
    /// A change on its way: to `target` (only its variant `variant`, if given),
    /// `length` seconds after it began. `captured`: whether the app has the old picture;
    /// `held`: a blend still under way when it began, held where it was.
    Out { t: f32, target: usize, variant: Option<usize>, length: f32, transition: Transition, captured: bool, held: f32 },
    /// The new look blending in over `length` seconds.
    In { t: f32, length: f32 },
}

/// A change waiting for its moment (the bar line, the beat or a kick).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pending {
    target: usize,
    variant: Option<usize>,
    waited: f32,
    transition: Transition,
    /// The director's own pick (not a refresh of the locked look).
    own: bool,
}

#[derive(Clone, Debug)]
pub struct Director {
    /// Change looks automatically (only in full screen).
    pub auto: bool,
    /// Change look at least this often, seconds.
    pub interval_s: f32,
    /// Pick only favourites when changing by itself.
    pub favourites_only: bool,
    /// Pick only looks with the browser's picked tags when changing by itself.
    pub tags_only: bool,
    /// Now and then show a new random look instead of its own pick (the app makes it:
    /// see `picked_itself`).
    pub random_looks: bool,
    /// The look on screen, if a look was applied.
    pub current: Option<usize>,
    /// Its variant on screen (0 its own picture), and seconds since it was.
    variant: usize,
    variant_elapsed: f32,
    elapsed: f32,
    silence: f32,
    slow: f32,
    /// A change is due (the timer ran out, or the music changed).
    due: bool,
    waiting: Option<Pending>,
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
    /// The app reported a new song since the last update.
    told_new_song: bool,
    /// The change under way (or the last) was the director's own pick, not asked for.
    own: bool,
}

impl Default for Director {
    fn default() -> Self {
        Self {
            auto: true,
            interval_s: 90.0,
            favourites_only: false,
            tags_only: false,
            random_looks: false,
            current: None,
            variant: 0,
            variant_elapsed: 0.0,
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
            told_new_song: false,
            own: false,
        }
    }
}

impl Director {
    /// Blend to look `target` (a manual choice, or the director's own), now.
    pub fn request(&mut self, target: usize) {
        self.change(target, 0.0, Transition::Blend(BLEND_S), false, None);
    }

    /// Blend to variant `variant` of the look on screen (0 its own picture), now.
    pub fn request_variant(&mut self, variant: usize) {
        if let Some(current) = self.current {
            self.change(current, 0.0, Transition::Blend(BLEND_S), false, Some(variant));
        }
    }

    /// The variant of the look on screen (0 its own picture).
    pub fn variant(&self) -> usize {
        self.variant
    }

    /// A new song has begun (known, not guessed from silence): change look for it after
    /// hearing its opening, however long the current look has been on.
    pub fn new_song(&mut self) {
        self.told_new_song = true;
    }

    /// Change to look `target` at once, without a blend.
    pub fn cut(&mut self, target: usize) {
        self.fade = Fade::None;
        self.change(target, 0.0, Transition::Cut, false, None);
    }

    /// Blend back to the look shown before this one, if there was one (and step further
    /// back each time). Returns whether there was.
    pub fn back(&mut self) -> bool {
        if matches!(self.fade, Fade::Out { .. }) {
            return false;
        }
        let Some(target) = self.history.pop() else { return false };
        self.request(target);
        self.going_back = true;
        true
    }

    /// The look the director would pick next (for a "next" key): the one gone longest
    /// unseen that suits the music, by `weights` (see `update`).
    pub fn pick(&self, looks: &[Look], weights: &[f32]) -> usize {
        self.choose(looks, weights, self.slow > ENERGETIC)
    }

    /// Whether the change that just landed (see `update`) was the director's own pick,
    /// not one asked for or a refresh of the locked look.
    pub fn picked_itself(&self) -> bool {
        self.own
    }

    /// Show look `look` in place of the one the change that just landed brought (the
    /// app made a random look instead of the director's pick, which then counts as
    /// never shown, so it comes soon).
    pub fn substitute(&mut self, look: usize) {
        if let Some(passed) = self.current.and_then(|c| self.shown.get_mut(c)) {
            *passed = 0;
        }
        self.current = Some(look);
        self.shown.resize(self.shown.len().max(look + 1), 0);
        self.shown[look] = self.applied;
    }

    /// Start a change to `target` (only its variant `variant`, if given), `length`
    /// seconds from now (unless one is already on its way). `own`: the director's own
    /// pick.
    fn change(&mut self, target: usize, length: f32, transition: Transition, own: bool, variant: Option<usize>) {
        if !matches!(self.fade, Fade::Out { .. }) {
            self.own = own && variant.is_none();
            let held = self.blend();
            self.fade = Fade::Out { t: 0.0, target, variant, length, transition, captured: false, held };
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
        if let Fade::Out { target, .. } = self.fade {
            match (now(target), &mut self.fade) {
                (Some(moved), Fade::Out { target, .. }) => *target = moved,
                _ => self.fade = Fade::None,
            }
        }
        self.waiting = self.waiting.and_then(|p| Some(Pending { target: now(p.target)?, ..p }));
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

    /// How far the current look has blended in over the old picture: 0 (all old) to 1
    /// (all new, no blend under way). Eased: it starts and ends gently.
    pub fn blend(&self) -> f32 {
        match self.fade {
            Fade::None => 1.0,
            Fade::Out { held, .. } => held,
            Fade::In { t, length } => {
                let x = (t / length).clamp(0.0, 1.0);
                x * x * (3.0 - 2.0 * x)
            }
        }
    }

    /// Whether a change is under way (on its way, or blending in).
    pub fn changing(&self) -> bool {
        self.fade != Fade::None
    }

    /// Whether the app should capture this frame's picture for a blend.
    pub fn capturing(&self) -> bool {
        matches!(self.fade, Fade::Out { transition: Transition::Blend(_), .. })
    }

    /// The app captured this frame's picture.
    pub fn captured(&mut self) {
        if let Fade::Out { captured, .. } = &mut self.fade {
            *captured = true;
        }
    }

    /// Advance `dt` seconds, choosing among `looks`. `presenting`: in full screen, where
    /// the director acts by itself (changing looks with `auto`, refreshing the locked one
    /// without). `features`: the analysis (none without a source); `kicked`: a kick this
    /// frame. `weights`: how much it favours each look when picking by itself (by index:
    /// 1 as usual, 0 never while another would do; any not listed count 1). Returns the
    /// look to apply now.
    pub fn update(&mut self, looks: &[Look], weights: &[f32], presenting: bool, features: Option<&AudioFeatures>, kicked: bool, dt: f32) -> Option<Change> {
        self.elapsed += dt;
        self.variant_elapsed += dt;
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
        let told = std::mem::take(&mut self.told_new_song);
        let song_change = !silent && self.silence >= SONG_GAP_S;
        self.silence = if silent { self.silence + dt } else { 0.0 };

        match self.fade {
            Fade::Out { t, target, variant, length, transition, captured, held } => {
                let t = t + dt;
                let ready = captured || transition == Transition::Cut || t >= length + CAPTURE_WAIT_S;
                if t >= length && ready {
                    self.fade = match transition {
                        Transition::Blend(length) => Fade::In { t: 0.0, length },
                        Transition::Cut => Fade::None,
                    };
                    if target >= looks.len() {
                        self.fade = Fade::None;
                        return None;
                    }
                    if let Some(variant) = variant {
                        // The same look, another picture: its time runs on.
                        (self.variant, self.variant_elapsed) = (variant, 0.0);
                        return Some(Change { look: target, cut: transition == Transition::Cut, variant: Some(variant) });
                    }
                    if let Some(left) = self.current.filter(|&c| c != target && !self.going_back) {
                        self.history.push(left);
                        if self.history.len() > HISTORY {
                            self.history.remove(0);
                        }
                    }
                    self.going_back = false;
                    (self.current, self.elapsed, self.due) = (Some(target), 0.0, false);
                    (self.variant, self.variant_elapsed) = (0, 0.0);
                    self.applied += 1;
                    self.shown.resize(looks.len(), 0);
                    self.shown[target] = self.applied;
                    return Some(Change { look: target, cut: transition == Transition::Cut, variant: None });
                }
                self.fade = Fade::Out { t, target, variant, length, transition, captured, held };
                return None;
            }
            Fade::In { t, length } => {
                self.fade = if t + dt >= length { Fade::None } else { Fade::In { t: t + dt, length } };
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
        let blend = Transition::Blend(BLEND_S);
        if !self.auto {
            // Locked: refresh the same look, so it doesn't go stale.
            if let Some(current) = self.current
                && ((settled && song_change) || told || self.due)
                && self.waiting.is_none()
            {
                self.waiting = Some(Pending { target: current, variant: None, waited: 0.0, transition: blend, own: false });
            }
            self.land(features, kicked, dt);
            return None;
        }
        if (settled && song_change) || told {
            self.listening = Some((0.0, 0.0));
        }
        if let Some((heard, sum)) = self.listening {
            // Hear the new song's opening before choosing; nothing else cuts in meanwhile.
            let (heard, sum) = (heard + dt, sum + level * dt);
            self.listening = if silent {
                None
            } else if heard >= LISTEN_S {
                let target = self.choose(looks, weights, sum / heard > ENERGETIC);
                self.change(target, 0.0, blend, true, None);
                None
            } else {
                Some((heard, sum))
            };
            return None;
        }
        let pending = |target, transition| Some(Pending { target, variant: None, waited: 0.0, transition, own: true });
        let variants = self.current.and_then(|c| looks.get(c)).map_or(0, |l| l.variants.len());
        match section {
            Some(SectionKind::Drop) if settled => self.waiting = pending(self.choose(looks, weights, true), Transition::Cut),
            Some(SectionKind::Breakdown) if settled => self.waiting = pending(self.choose(looks, weights, false), Transition::Blend(SLOW_BLEND_S)),
            Some(SectionKind::Change) if variants > 0 => {
                if let Some(current) = self.current
                    && self.variant_elapsed >= MIN_VARIANT_S
                    && self.waiting.is_none()
                {
                    // Over a bar, with a steady beat.
                    let bar = features.map(|f| f.musical).filter(|m| m.beating && m.bpm > 0.0).map_or(BLEND_S, |m| 240.0 / m.bpm);
                    let next = (self.variant + 1) % (variants + 1);
                    self.waiting = Some(Pending { target: current, variant: Some(next), waited: 0.0, transition: Transition::Blend(bar), own: false });
                }
            }
            Some(SectionKind::Change) if self.elapsed >= MIN_LOOK_S.max(0.5 * self.interval_s) => self.due = true,
            _ => {}
        }
        if self.due && self.waiting.is_none() {
            self.waiting = pending(self.choose(looks, weights, self.slow > ENERGETIC), blend);
        }
        self.land(features, kicked, dt);
        None
    }

    /// Start a waiting change when it will land on the music: with a steady beat, a
    /// blend's switch on the next downbeat and a cut's on the next beat (getting ready
    /// `LEAD_S` before); without one, a blend on a kick or after waiting a while, a cut
    /// at once.
    fn land(&mut self, features: Option<&AudioFeatures>, kicked: bool, dt: f32) {
        let Some(p) = self.waiting else { return };
        let waited = p.waited + dt;
        let beat = features.map(|f| f.musical).filter(|m| m.beating && m.bpm > 0.0);
        let start = match beat {
            Some(m) => {
                let beat_s = 60.0 / m.bpm;
                let (left, span) = match p.transition {
                    Transition::Cut => ((1.0 - m.beat_phase) * beat_s, beat_s),
                    Transition::Blend(_) => ((1.0 - m.bar_phase) * 4.0 * beat_s, 4.0 * beat_s),
                };
                // A span and a half with no line found (the beat lost): go anyway.
                (left <= LEAD_S || waited > 1.5 * span).then_some(left.min(LEAD_S))
            }
            None => (p.transition == Transition::Cut || kicked || waited >= KICK_WAIT_S).then_some(0.0),
        };
        match start {
            Some(length) => self.change(p.target, length, p.transition, p.own, p.variant),
            None => self.waiting = Some(Pending { waited, ..p }),
        }
    }

    /// The look that suits the music's energy and has gone longest unseen, the time
    /// scaled by its weight; of those weighted above 0 if any suit. Ties go to the first
    /// after the current one.
    fn choose(&self, looks: &[Look], weights: &[f32], energetic: bool) -> usize {
        let n = looks.len().max(1);
        let start = self.current.map_or(0, |c| c + 1);
        let weight = |i: usize| weights.get(i).copied().unwrap_or(1.0);
        // Looks applied since this one last was (all of them, plus one, for never).
        let age = |i: usize| (self.applied + 1 - self.shown.get(i).copied().unwrap_or(0).min(self.applied)) as f32;
        let pick = |weighted: bool| {
            let mut best: Option<(usize, f32)> = None;
            for i in (0..looks.len()).map(|k| (start + k) % n) {
                if Some(i) == self.current || !looks[i].suits(energetic) || (weighted && weight(i) <= 0.0) {
                    continue;
                }
                let score = if weighted { age(i) * weight(i) } else { age(i) };
                if best.is_none_or(|(_, s)| score > s) {
                    best = Some((i, score));
                }
            }
            best.map(|(i, _)| i)
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

    /// A frame as the app runs it: the update, then the picture captured if asked for.
    /// Returns the look applied, if any.
    fn step(d: &mut Director, presenting: bool, f: Option<&AudioFeatures>, kicked: bool) -> Option<usize> {
        let change = d.update(looks(), &[], presenting, f, kicked, FRAME);
        if d.capturing() {
            d.captured();
        }
        change.map(|c| c.look)
    }

    /// Run `seconds` of frames; returns the looks applied.
    fn run(d: &mut Director, f: &AudioFeatures, kick_every: Option<usize>, seconds: f32) -> Vec<usize> {
        let frames = (seconds / FRAME) as usize;
        (0..frames).filter_map(|k| step(d, true, Some(f), kick_every.is_some_and(|n| k % n == 0))).collect()
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
            run(&mut d, &silence(), None, 3.0);
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

    /// A song the app reports changes look after its opening, even a few seconds into
    /// the last look, and with no silence before it.
    #[test]
    fn a_reported_new_song_changes_look() {
        let mut d = Director { current: Some(0), ..Director::default() };
        run(&mut d, &music(0.3), None, 5.0);
        d.new_song();
        let applied = run(&mut d, &music(0.3), None, 3.0);
        assert_eq!(applied.len(), 1, "{applied:?}");
        assert!(run(&mut d, &music(0.3), None, 10.0).is_empty(), "changed again");
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
        f.musical = crate::audio::tempo::MusicalTime { bpm, confidence: 0.5, beating: true, bar_phase: (t / bar).fract(), beat_phase: (4.0 * t / bar).fract(), ..Default::default() };
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

    /// Changes of section move a look with variants on to the next (round to its own
    /// picture), no more often than `MIN_VARIANT_S`, without changing the look; a drop
    /// still changes it, to the new look's own picture.
    #[test]
    fn changes_of_section_move_through_a_looks_variants() {
        let mut with: Vec<Look> = looks().to_vec();
        let picture = with[1].picture();
        with[0].variants = vec![picture, picture];
        let mut d = Director { current: Some(0), ..Director::default() };
        let go = |d: &mut Director, f: &AudioFeatures, seconds: f32| {
            let mut out = Vec::new();
            for _ in 0..(seconds / FRAME) as usize {
                out.extend(d.update(&with, &[], true, Some(f), false, FRAME));
                if d.capturing() {
                    d.captured();
                }
            }
            out
        };
        let variant = |v| vec![Change { look: 0, cut: false, variant: Some(v) }];
        go(&mut d, &music(0.3), 10.0);
        assert_eq!(go(&mut d, &sectioned(0.3, 1, SectionKind::Change), 5.0), variant(1));
        assert_eq!(d.variant(), 1);
        assert!(go(&mut d, &sectioned(0.3, 2, SectionKind::Change), 5.0).is_empty(), "moved again within {MIN_VARIANT_S} s");
        go(&mut d, &music(0.3), 8.0);
        assert_eq!(go(&mut d, &sectioned(0.3, 3, SectionKind::Change), 5.0), variant(2));
        go(&mut d, &music(0.3), 8.0);
        assert_eq!(go(&mut d, &sectioned(0.3, 4, SectionKind::Change), 5.0), variant(0));
        go(&mut d, &music(0.3), 3.0);
        let dropped = go(&mut d, &sectioned(0.3, 5, SectionKind::Drop), 1.0);
        assert!(dropped.len() == 1 && dropped[0].look != 0 && dropped[0].variant.is_none() && d.variant() == 0, "{dropped:?}");
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
                if step(&mut d, true, Some(&beating(0.3, bpm, t)), false).is_some() {
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
            assert_eq!(step(&mut d, false, Some(&music(0.5)), true), None);
        }
        d.request(3);
        let applied: Vec<usize> = (0..120).filter_map(|_| step(&mut d, false, None, false)).collect();
        assert_eq!(applied, vec![3]);
    }

    /// A blend: the old picture is captured first, then the new look takes over from it,
    /// steadily, over the blend's length.
    #[test]
    fn blends_take_over_from_the_old_picture() {
        let mut d = Director::default();
        d.request(2);
        assert_eq!(d.update(looks(), &[], false, None, false, FRAME), None, "switched before the old picture was captured");
        assert!(d.capturing());
        d.captured();
        assert_eq!(d.update(looks(), &[], false, None, false, FRAME), Some(Change { look: 2, cut: false, variant: None }));
        assert!(d.blend() < 0.05, "{}", d.blend());
        let mut last = d.blend();
        for _ in 0..(BLEND_S / FRAME) as usize + 2 {
            step(&mut d, false, None, false);
            assert!(d.blend() >= last, "the blend went back");
            last = d.blend();
        }
        assert_eq!(d.blend(), 1.0);
        assert!(!d.changing());
        assert_eq!(d.current, Some(2));
    }

    /// With a steady beat a drop cuts, on the next beat.
    #[test]
    fn a_drop_cuts_on_the_beat() {
        let bpm = 128.0;
        let beat_s = 60.0 / bpm;
        let mut d = settled(0.3);
        let mut t = 0.21; // not on a beat
        let mut cut = None;
        for _ in 0..(3.0 / FRAME) as usize {
            let mut f = beating(0.3, bpm, t);
            f.sections.changes = 1;
            f.sections.last = Some(crate::audio::sections::Section { kind: SectionKind::Drop, time: 0.0 });
            if let Some(change) = d.update(looks(), &[], true, Some(&f), false, FRAME) {
                cut = Some((change, t));
                break;
            }
            t += FRAME;
        }
        let (change, t) = cut.expect("the drop changed nothing");
        assert!(change.cut, "a drop cuts");
        assert!(looks()[change.look].suits(true));
        let off = ((t / beat_s).round() - t / beat_s).abs() * beat_s;
        assert!(off <= 1.5 * FRAME, "cut {:.0} ms from a beat", off * 1e3);
        assert!(t - 0.21 < beat_s + FRAME, "waited past the next beat");
    }

    /// A look weighted 2 comes round about twice as often as the others.
    #[test]
    fn weights_bring_looks_round_more_often() {
        let n = looks().len();
        let energetic: Vec<usize> = (0..n).filter(|&i| looks()[i].suits(true)).collect();
        let favourite = energetic[1];
        let weights: Vec<f32> = (0..n).map(|i| if i == favourite { 2.0 } else { 1.0 }).collect();
        let mut d = Director { current: Some(0), ..Director::default() };
        let mut counts = vec![0; n];
        for _ in 0..energetic.len() * 6 {
            let target = d.choose(looks(), &weights, true);
            d.request(target);
            run(&mut d, &silence(), None, 3.0);
            counts[d.current.unwrap()] += 1;
        }
        let others = energetic.iter().filter(|&&i| i != favourite).map(|&i| counts[i]).sum::<usize>() as f32 / (energetic.len() - 1) as f32;
        let ratio = counts[favourite] as f32 / others;
        assert!((1.6..=2.6).contains(&ratio), "the favourite came round {ratio:.2} times as often");
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
        let only: Vec<f32> = (0..looks().len()).map(|i| if i == favourite { 1.0 } else { 0.0 }).collect();
        assert_eq!(d.choose(looks(), &only, true), favourite);
        let none = vec![0.0; looks().len()];
        assert!(looks()[d.choose(looks(), &none, true)].suits(true));
    }

    /// Back steps through the looks shown, most recent first; a cut changes at once.
    #[test]
    fn back_returns_through_the_looks_shown() {
        let mut d = Director { current: Some(0), ..Director::default() };
        let go = |d: &mut Director| (0..200).filter_map(|_| step(d, false, None, false)).last();
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
        assert_eq!(d.update(looks(), &[], false, None, false, FRAME), Some(Change { look: 7, cut: true, variant: None }));
        assert_eq!(d.blend(), 1.0, "a cut doesn't blend");
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

    /// The director's own picks are marked as its own (asked-for changes aren't), and
    /// one can be replaced: the replacement is on screen, back returns past it, and the
    /// passed-over pick comes round next.
    #[test]
    fn own_picks_can_be_replaced() {
        let mut d = settled(0.3);
        d.request(4);
        assert_eq!((0..200).filter_map(|_| step(&mut d, true, None, false)).last(), Some(4));
        assert!(!d.picked_itself(), "asked for");
        let applied = run(&mut d, &music(0.3), Some(30), 95.0);
        assert_eq!(applied.len(), 1, "{applied:?}");
        assert!(d.picked_itself(), "the timer's pick");
        let passed = applied[0];
        d.substitute(9);
        assert_eq!(d.current, Some(9));
        // Against the look shown before it, the passed-over pick counts as never shown.
        let weights: Vec<f32> = (0..looks().len()).map(|i| if i == passed || i == 4 { 1.0 } else { 0.0 }).collect();
        if looks()[passed].suits(false) && looks()[4].suits(false) {
            assert_eq!(d.pick(looks(), &weights), passed, "the passed-over pick comes next");
        }
        assert!(d.back());
        assert_eq!((0..200).filter_map(|_| step(&mut d, false, None, false)).last(), Some(4));
    }
}
