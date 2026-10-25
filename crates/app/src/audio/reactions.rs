//! Reactions: how the music drives the fluid. Each reaction maps one trigger (a hit, a
//! band, the level or the brightness) to one effect (a splash, a vortex, gravity
//! rocking, a colour shift, a solver parameter...) at some gain.
//!
//! Any trigger can drive any effect:
//! - Hits (kick, snare, hat) fire one-shot effects directly, and drive continuous
//!   effects through a short decaying pulse.
//! - Envelopes (bands, level, brightness) drive continuous effects directly, and fire
//!   one-shot effects when they surge past a threshold (with hysteresis).
//!
//! The engine only produces what the app already feeds the simulation: brushes and
//! emitters, a gravity vector, a palette modulation and parameter values by key.
//!
//! With `idle` set (the full-screen visualizer), long silence (or no source at all) gets
//! a slow wandering swirl, so a pause between songs doesn't freeze the picture.

use super::analyzer::{AudioFeatures, Onset, OnsetKind};
use serde::{Deserialize, Serialize};
use sim::{Emitter, ForceBrush, Vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Trigger {
    Kick,
    Snare,
    Hat,
    Sub,
    Bass,
    LowMid,
    Mid,
    High,
    Level,
    Brightness,
}

impl Trigger {
    pub const ALL: [Trigger; 10] = [
        Trigger::Kick,
        Trigger::Snare,
        Trigger::Hat,
        Trigger::Sub,
        Trigger::Bass,
        Trigger::LowMid,
        Trigger::Mid,
        Trigger::High,
        Trigger::Level,
        Trigger::Brightness,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Trigger::Kick => "Kick",
            Trigger::Snare => "Snare",
            Trigger::Hat => "Hi-hat",
            Trigger::Sub => "Sub",
            Trigger::Bass => "Bass",
            Trigger::LowMid => "Low-mid",
            Trigger::Mid => "Mid",
            Trigger::High => "High",
            Trigger::Level => "Level",
            Trigger::Brightness => "Brightness",
        }
    }

    fn hit(self) -> Option<OnsetKind> {
        match self {
            Trigger::Kick => Some(OnsetKind::Kick),
            Trigger::Snare => Some(OnsetKind::Snare),
            Trigger::Hat => Some(OnsetKind::Hat),
            _ => None,
        }
    }

    /// The envelope value, 0..1, for envelope triggers.
    fn envelope(self, f: &AudioFeatures) -> f32 {
        match self {
            Trigger::Sub => f.bands[0],
            Trigger::Bass => f.bands[1],
            Trigger::LowMid => f.bands[2],
            Trigger::Mid => f.bands[3],
            Trigger::High => f.bands[4],
            Trigger::Level => f.level,
            Trigger::Brightness => f.brightness,
            Trigger::Kick | Trigger::Snare | Trigger::Hat => 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Effect {
    /// Shove the fluid upward from the floor.
    Splash,
    /// An emitter burst from the floor (adds fluid; a burner in the fire solver).
    Jet,
    /// Stir a swirl at a random spot, alternating direction.
    Vortex,
    /// Brighten the fluid's colours.
    Flash,
    /// Sideways wind across the domain, slowly changing direction.
    Gust,
    /// Rock gravity side to side.
    Slosh,
    /// Gravity strength follows the trigger.
    Breathe,
    /// Shift the fluid palette's hue.
    Hue,
    /// Modulate a solver parameter (`Reaction::param`).
    Param,
}

impl Effect {
    pub const ALL: [Effect; 9] = [
        Effect::Splash,
        Effect::Jet,
        Effect::Vortex,
        Effect::Flash,
        Effect::Gust,
        Effect::Slosh,
        Effect::Breathe,
        Effect::Hue,
        Effect::Param,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Effect::Splash => "Splash",
            Effect::Jet => "Jet",
            Effect::Vortex => "Vortex",
            Effect::Flash => "Flash",
            Effect::Gust => "Gust",
            Effect::Slosh => "Slosh",
            Effect::Breathe => "Breathe",
            Effect::Hue => "Hue",
            Effect::Param => "Parameter",
        }
    }

    pub fn help(self) -> &'static str {
        match self {
            Effect::Splash => "Shove the fluid upward from the floor.",
            Effect::Jet => "Fire a jet from the floor (adds fluid; a burner for fire).",
            Effect::Vortex => "Stir a swirl at a random spot, alternating direction.",
            Effect::Flash => "Brighten the fluid's colours for a moment.",
            Effect::Gust => "Sideways wind across the domain, slowly changing direction.",
            Effect::Slosh => "Rock gravity side to side.",
            Effect::Breathe => "Gravity gets stronger and weaker with the trigger.",
            Effect::Hue => "Shift the hue of the fluid's colours.",
            Effect::Param => "Modulate a solver setting around its value.",
        }
    }

    /// Fired once per hit (or surge), rather than following the trigger continuously.
    fn one_shot(self) -> bool {
        matches!(self, Effect::Splash | Effect::Jet | Effect::Vortex | Effect::Flash)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reaction {
    pub enabled: bool,
    pub trigger: Trigger,
    pub effect: Effect,
    pub gain: f32,
    /// For `Effect::Param`: the solver parameter's key.
    #[serde(default)]
    pub param: Option<String>,
}

impl Reaction {
    pub fn new(trigger: Trigger, effect: Effect, gain: f32, enabled: bool) -> Self {
        Self { enabled, trigger, effect, gain, param: None }
    }
}

/// The starting set: one hand-built look that works on any solver.
pub fn default_reactions() -> Vec<Reaction> {
    vec![
        Reaction::new(Trigger::Kick, Effect::Splash, 1.0, true),
        Reaction::new(Trigger::Snare, Effect::Vortex, 1.0, true),
        Reaction::new(Trigger::Hat, Effect::Flash, 0.6, true),
        Reaction::new(Trigger::Bass, Effect::Slosh, 0.6, true),
        Reaction::new(Trigger::Brightness, Effect::Hue, 0.5, true),
        Reaction::new(Trigger::Mid, Effect::Gust, 0.5, false),
        Reaction::new(Trigger::Level, Effect::Breathe, 0.5, false),
    ]
}

/// What the reactions add this frame.
#[derive(Clone, Debug, Default)]
pub struct ReactionOutput {
    pub brushes: Vec<ForceBrush>,
    pub emitters: Vec<Emitter>,
    pub gravity: Vec2,
    /// Palette hue rotation, degrees.
    pub hue_shift: f32,
    /// Palette brightening, 0..1.
    pub flash: f32,
    /// Solver parameters to set this frame: (key, value as a multiple of its base).
    pub params: Vec<(String, f32)>,
}

/// How long one-shot effects last, seconds.
const SPLASH_S: f32 = 0.09;
const VORTEX_S: f32 = 0.3;
/// Hit pulses (driving continuous effects) and flashes decay with these time constants.
const HIT_DECAY_S: f32 = 0.2;
const FLASH_DECAY_S: f32 = 0.15;
/// Envelope triggers fire one-shot effects above this, and re-arm below the lower value.
const SURGE_ON: f32 = 0.75;
const SURGE_OFF: f32 = 0.5;
/// Slosh rocks with this period, seconds, up to this tilt (degrees) at full scale.
const SLOSH_PERIOD_S: f32 = 2.0;
const SLOSH_MAX_DEG: f32 = 30.0;
/// The gust turns around over this period, seconds.
const GUST_PERIOD_S: f32 = 16.0;
const LANES: [f32; 3] = [0.3, 0.7, 0.5];
/// Idle drift starts after this much silence and fades in over `IDLE_FADE_S`, seconds.
const IDLE_AFTER_S: f32 = 5.0;
const IDLE_FADE_S: f32 = 2.0;

struct Impulse {
    effect: Effect,
    age: f32,
    strength: f32,
    pos: Vec2,
    spin: f32,
}

/// The reactions' running state: pulses in flight, hit envelopes, the clock.
#[derive(Default)]
pub struct ReactionEngine {
    /// Drift gently during long silences (set by the app each frame).
    pub idle: bool,
    silent_for: f32,
    time: f32,
    impulses: Vec<Impulse>,
    hit_env: [f32; 3],
    flash: f32,
    /// Per reaction: armed for the next surge (envelope triggers).
    armed: Vec<bool>,
    lane: usize,
    spin_sign: f32,
    seed: u32,
}

impl ReactionEngine {
    /// Evaluate `reactions` for one frame of `dt` seconds. `features` is `None` without
    /// an audio source; `onsets` are this frame's hits. `intensity` scales everything.
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate(
        &mut self,
        reactions: &[Reaction],
        features: Option<&AudioFeatures>,
        onsets: &[Onset],
        domain: Vec2,
        base_gravity: Vec2,
        intensity: f32,
        dt: f32,
    ) -> ReactionOutput {
        self.time += dt;
        let mut out = ReactionOutput { gravity: base_gravity, ..ReactionOutput::default() };
        let silent = features.is_none_or(|f| f.silent);
        self.silent_for = if silent { self.silent_for + dt } else { 0.0 };
        if let Some(drift) = self.idle_drift(domain) {
            out.brushes.push(drift);
        }
        let Some(features) = features else {
            self.impulses.clear();
            return out;
        };
        let decay = (-dt / HIT_DECAY_S).exp();
        self.hit_env.iter_mut().for_each(|e| *e *= decay);
        for o in onsets {
            let k = OnsetKind::ALL.iter().position(|&kind| kind == o.kind).unwrap_or(0);
            self.hit_env[k] = self.hit_env[k].max(o.strength);
        }
        self.armed.resize(reactions.len(), true);

        let (mut tilt_deg, mut gravity_scale) = (0.0f32, 1.0f32);
        for (r_index, r) in reactions.iter().enumerate().filter(|(_, r)| r.enabled) {
            let gain = r.gain * intensity;
            // The trigger's continuous value, and this frame's firings (with strengths).
            let (value, firings): (f32, Vec<f32>) = match r.trigger.hit() {
                Some(kind) => {
                    let k = OnsetKind::ALL.iter().position(|&x| x == kind).unwrap_or(0);
                    (self.hit_env[k], onsets.iter().filter(|o| o.kind == kind).map(|o| o.strength).collect())
                }
                None => {
                    let v = r.trigger.envelope(features);
                    let fired = self.armed[r_index] && v > SURGE_ON;
                    if fired {
                        self.armed[r_index] = false;
                    } else if v < SURGE_OFF {
                        self.armed[r_index] = true;
                    }
                    (v, if fired { vec![v] } else { Vec::new() })
                }
            };
            if r.effect.one_shot() {
                for strength in firings {
                    self.fire(r.effect, strength * gain, domain);
                }
                continue;
            }
            match r.effect {
                Effect::Gust => {
                    let dir = (std::f32::consts::TAU * self.time / GUST_PERIOD_S).sin().signum();
                    out.brushes.push(ForceBrush {
                        pos: 0.5 * domain,
                        radius: 0.6 * domain.y,
                        velocity: Vec2::new(dir * 2.0 * value * gain, 0.0),
                        strength: 3.0 * value,
                        spin: 0.0,
                    });
                }
                Effect::Slosh => {
                    tilt_deg += SLOSH_MAX_DEG * value * gain * (std::f32::consts::TAU * self.time / SLOSH_PERIOD_S).sin();
                }
                Effect::Breathe => gravity_scale *= (1.0 + gain * (value - 0.5)).clamp(0.2, 2.0),
                Effect::Hue => out.hue_shift += 180.0 * gain * (value - 0.5),
                Effect::Param => {
                    if let Some(key) = &r.param {
                        out.params.push((key.clone(), (1.0 + gain * (2.0 * value - 1.0)).max(0.0)));
                    }
                }
                Effect::Splash | Effect::Jet | Effect::Vortex | Effect::Flash => unreachable!("one-shot"),
            }
        }
        out.gravity = Vec2::from_angle(tilt_deg.to_radians()).rotate(base_gravity) * gravity_scale;

        self.flash *= (-dt / FLASH_DECAY_S).exp();
        out.flash = self.flash.min(1.0);
        for imp in &mut self.impulses {
            imp.age += dt;
        }
        self.impulses.retain(|i| i.age < if i.effect == Effect::Vortex { VORTEX_S } else { SPLASH_S });
        for imp in &self.impulses {
            match imp.effect {
                Effect::Splash => out.brushes.push(ForceBrush {
                    pos: imp.pos,
                    radius: 0.22 * domain.y,
                    velocity: Vec2::new(0.0, 1.5 + 3.5 * imp.strength),
                    strength: 45.0,
                    spin: 0.0,
                }),
                Effect::Jet => out.emitters.push(Emitter {
                    pos: imp.pos,
                    dir: Vec2::Y,
                    speed: 2.0 + 3.0 * imp.strength,
                    width: 0.04,
                }),
                Effect::Vortex => out.brushes.push(ForceBrush {
                    pos: imp.pos,
                    radius: 0.18 * domain.y,
                    velocity: Vec2::ZERO,
                    strength: 25.0,
                    spin: imp.spin,
                }),
                _ => {}
            }
        }
        out
    }

    /// A slow swirl wandering on a Lissajous path, turning around now and then, once the
    /// silence has lasted `IDLE_AFTER_S` (and only with `idle` set).
    fn idle_drift(&self, domain: Vec2) -> Option<ForceBrush> {
        if !self.idle || self.silent_for < IDLE_AFTER_S {
            return None;
        }
        let fade = ((self.silent_for - IDLE_AFTER_S) / IDLE_FADE_S).min(1.0);
        let t = self.time;
        let offset = Vec2::new(0.3 * domain.x * (0.13 * t).sin(), 0.22 * domain.y * (0.21 * t).sin());
        let turn = if (0.05 * t).sin() >= 0.0 { 1.0 } else { -1.0 };
        Some(ForceBrush { pos: 0.5 * domain + offset, radius: 0.2 * domain.y, velocity: Vec2::ZERO, strength: 2.0 * fade, spin: 3.0 * turn })
    }

    fn fire(&mut self, effect: Effect, strength: f32, domain: Vec2) {
        let lane = LANES[self.lane % LANES.len()];
        match effect {
            Effect::Splash | Effect::Jet => {
                self.lane += 1;
                let y = if effect == Effect::Splash { 0.15 } else { 0.07 };
                self.impulses.push(Impulse { effect, age: 0.0, strength, pos: Vec2::new(lane * domain.x, y * domain.y), spin: 0.0 });
            }
            Effect::Vortex => {
                self.spin_sign = if self.spin_sign > 0.0 { -1.0 } else { 1.0 };
                let pos = Vec2::new((0.2 + 0.6 * self.random()) * domain.x, (0.25 + 0.5 * self.random()) * domain.y);
                let spin = self.spin_sign * (6.0 + 10.0 * strength);
                self.impulses.push(Impulse { effect, age: 0.0, strength, pos, spin });
            }
            Effect::Flash => self.flash = self.flash.max(strength),
            _ => {}
        }
    }

    /// 0..1, deterministic (xorshift).
    fn random(&mut self) -> f32 {
        if self.seed == 0 {
            self.seed = 0x2545_F491;
        }
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        self.seed as f32 / u32::MAX as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOMAIN: Vec2 = Vec2::new(2.0, 1.0);
    const G: Vec2 = Vec2::new(0.0, -9.81);

    fn features(bass: f32, brightness: f32) -> AudioFeatures {
        let mut f = AudioFeatures { brightness, level: 0.5, ..AudioFeatures::default() };
        f.bands[1] = bass;
        f
    }

    fn kick(strength: f32) -> Onset {
        Onset { kind: OnsetKind::Kick, time: 0.0, strength }
    }

    #[test]
    fn without_audio_nothing_happens() {
        let mut e = ReactionEngine::default();
        let out = e.evaluate(&default_reactions(), None, &[kick(1.0)], DOMAIN, G, 1.0, 1.0 / 60.0);
        assert!(out.brushes.is_empty() && out.emitters.is_empty());
        assert_eq!(out.gravity, G);
    }

    #[test]
    fn a_kick_splashes_briefly() {
        let mut e = ReactionEngine::default();
        let r = [Reaction::new(Trigger::Kick, Effect::Splash, 1.0, true)];
        let f = features(0.0, 0.5);
        let first = e.evaluate(&r, Some(&f), &[kick(1.0)], DOMAIN, G, 1.0, 1.0 / 60.0);
        assert_eq!(first.brushes.len(), 1);
        assert!(first.brushes[0].velocity.y > 0.0);
        for _ in 0..10 {
            e.evaluate(&r, Some(&f), &[], DOMAIN, G, 1.0, 1.0 / 60.0);
        }
        assert!(e.evaluate(&r, Some(&f), &[], DOMAIN, G, 1.0, 1.0 / 60.0).brushes.is_empty(), "splash outlived 90 ms");
    }

    #[test]
    fn vortices_alternate_direction() {
        let mut e = ReactionEngine::default();
        let r = [Reaction::new(Trigger::Kick, Effect::Vortex, 1.0, true)];
        let f = features(0.0, 0.5);
        let a = e.evaluate(&r, Some(&f), &[kick(0.5)], DOMAIN, G, 1.0, 1.0 / 60.0).brushes;
        let b = e.evaluate(&r, Some(&f), &[kick(0.5)], DOMAIN, G, 1.0, 1.0 / 60.0).brushes;
        // The first vortex is still stirring when the second starts.
        assert_eq!((a.len(), b.len()), (1, 2));
        assert!(a[0].spin * b[1].spin < 0.0, "spins {} and {}", a[0].spin, b[1].spin);
    }

    #[test]
    fn bass_sloshes_gravity_without_changing_its_strength() {
        let mut e = ReactionEngine::default();
        let r = [Reaction::new(Trigger::Bass, Effect::Slosh, 1.0, true)];
        // A quarter of the slosh period in: the tilt is at its maximum.
        let out = e.evaluate(&r, Some(&features(1.0, 0.5)), &[], DOMAIN, G, 1.0, SLOSH_PERIOD_S / 4.0);
        let tilt = out.gravity.angle_to(G).to_degrees().abs();
        assert!((tilt - SLOSH_MAX_DEG).abs() < 0.1, "tilt {tilt}");
        assert!((out.gravity.length() - G.length()).abs() < 1e-4);
        let calm = e.evaluate(&r, Some(&features(0.0, 0.5)), &[], DOMAIN, G, 1.0, 0.1);
        assert!((calm.gravity - G).length() < 1e-5);
    }

    #[test]
    fn an_envelope_surge_fires_once_until_it_drops() {
        let mut e = ReactionEngine::default();
        let r = [Reaction::new(Trigger::Bass, Effect::Splash, 1.0, true)];
        let count = |e: &mut ReactionEngine, bass: f32| {
            let before = e.impulses.len();
            e.evaluate(&r, Some(&features(bass, 0.5)), &[], DOMAIN, G, 1.0, 0.001);
            e.impulses.len() > before
        };
        assert!(count(&mut e, 0.9), "surge didn't fire");
        assert!(!count(&mut e, 0.95), "fired again while still high");
        assert!(!count(&mut e, 0.3));
        assert!(count(&mut e, 0.9), "didn't re-arm after dropping");
    }

    #[test]
    fn brightness_shifts_hue_and_hats_flash() {
        let mut e = ReactionEngine::default();
        let r = [
            Reaction::new(Trigger::Brightness, Effect::Hue, 1.0, true),
            Reaction::new(Trigger::Hat, Effect::Flash, 1.0, true),
        ];
        let hat = Onset { kind: OnsetKind::Hat, time: 0.0, strength: 0.8 };
        let out = e.evaluate(&r, Some(&features(0.0, 1.0)), &[hat], DOMAIN, G, 1.0, 0.001);
        assert!((out.hue_shift - 90.0).abs() < 1e-3, "hue {}", out.hue_shift);
        assert!(out.flash > 0.75, "flash {}", out.flash);
        let neutral = e.evaluate(&r, Some(&features(0.0, 0.5)), &[], DOMAIN, G, 1.0, 1.0);
        assert!(neutral.hue_shift.abs() < 1e-4 && neutral.flash < 0.01);
    }

    #[test]
    fn idle_drift_starts_after_a_long_silence_and_stops_with_music() {
        let mut e = ReactionEngine { idle: true, ..ReactionEngine::default() };
        let quiet = AudioFeatures { silent: true, ..AudioFeatures::default() };
        let r = default_reactions();
        let step = |e: &mut ReactionEngine, f: Option<&AudioFeatures>, s: f32| e.evaluate(&r, f, &[], DOMAIN, G, 1.0, s);
        assert!(step(&mut e, Some(&quiet), 4.0).brushes.is_empty(), "drifting before the silence was long");
        let drift = step(&mut e, Some(&quiet), 3.0).brushes;
        assert_eq!(drift.len(), 1);
        assert!(drift[0].spin != 0.0 && drift[0].strength > 0.0);
        assert_eq!(step(&mut e, None, 1.0).brushes.len(), 1, "no source also drifts");
        assert!(step(&mut e, Some(&features(0.3, 0.5)), 0.01).brushes.is_empty(), "music stops the drift");
        let mut off = ReactionEngine::default();
        assert!(off.evaluate(&r, None, &[], DOMAIN, G, 1.0, 10.0).brushes.is_empty(), "idle off: no drift");
    }

    #[test]
    fn disabled_reactions_do_nothing() {
        let mut e = ReactionEngine::default();
        let r = [Reaction::new(Trigger::Kick, Effect::Splash, 1.0, false)];
        let out = e.evaluate(&r, Some(&features(0.0, 0.5)), &[kick(1.0)], DOMAIN, G, 1.0, 0.01);
        assert!(out.brushes.is_empty());
    }
}
