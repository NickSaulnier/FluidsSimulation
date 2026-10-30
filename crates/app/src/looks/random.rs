//! Random looks: new ones at the press of a key (`random`), and variations on the one
//! on screen (`vary`). A random look is an ordinary `Look`, filled in within ranges
//! known to stay watchable: the built-in looks' own, and their tuning (brightest
//! feedback with a strong zoom needs dim waves; long bright trails need a gentle zoom,
//! or the picture blows out to white).
//!
//! The fluid's own physical settings are drawn from `PHYSICS`, a range per solver
//! parameter inside its control's; numerics that only trade speed for accuracy
//! (pressure iterations, resolution, CFL) are never touched, so a random look runs as
//! fast and as stably as a built-in. Reactions come from sets that suit the solver, and
//! equations, sometimes, from a small library of snippets with random constants.
//!
//! Some looks get a fractal (`random_fractal`): alone with the feedback, or over a
//! fluid, fainter, often bent by its flow; diving, touring or wandering with the music.

use super::{Code, CodeSource, Energy, Look, Source, compile_code};
use crate::audio::reactions::{Effect, Reaction, Trigger};
use crate::render::composite::{CompositeSettings, Symmetry};
use crate::render::feedback::{EchoFlip, FeedbackMode, FeedbackSettings};
use crate::render::fractal::{self, FractalKind, FractalSettings};
use crate::render::palette::{PALETTES, Palette};
use crate::render::waves::{ShapeSettings, WaveMode, WaveSettings};
use utils::rng::SplitMix64;

/// The author random looks are saved under.
pub const AUTHOR: &str = "random";

/// A solver parameter's watchable range: key, low, high, and whether it's drawn on a
/// log scale.
type Range = (&'static str, f32, f32, bool);

/// The physical parameters a random look sets, by solver (each inside its control's
/// range; a test checks).
pub const PHYSICS: &[(&str, &[Range])] = &[
    ("FLIP", &[("flip_ratio", 0.7, 0.99, false)]),
    ("PBF", &[("viscosity", 0.0, 0.05, false), ("tensile_k", 0.005, 0.03, false)]),
    ("Stable fluids (dye)", &[("buoyancy", -0.4, 0.5, false), ("vorticity", 0.05, 0.9, false), ("dissipation", 0.0, 0.1, false)]),
    ("Stable fluids (GPU)", &[("buoyancy", -0.4, 0.5, false), ("vorticity", 0.05, 0.9, false), ("dissipation", 0.0, 0.1, false)]),
    (
        "Smoke & fire",
        &[
            ("buoyancy", -0.2, 0.2, false),
            ("vorticity", 0.1, 0.8, false),
            ("dissipation", 0.05, 0.3, false),
            ("burn_rate", 1.5, 6.0, true),
            ("heat", 3.0, 12.0, true),
            ("cooling", 1.2, 4.0, true),
            ("soot", 0.1, 0.8, false),
            ("rise", 0.06, 0.25, false),
            ("burner", 0.6, 1.8, true),
        ],
    ),
    ("Lattice Boltzmann (dye)", &[("tau", 0.51, 0.56, false), ("buoyancy", -0.3, 0.6, false)]),
    ("Shallow water", &[("depth", 0.015, 0.06, true), ("damping", 0.15, 0.8, false)]),
    ("MPM jelly", &[("stiffness", 15.0, 60.0, true), ("poisson", 0.15, 0.4, false)]),
    ("MPM snow", &[("stiffness", 15.0, 40.0, true), ("hardening", 5.0, 15.0, false), ("compression", 0.015, 0.05, true), ("stretch", 0.002, 0.008, true)]),
    ("MPM sand", &[("stiffness", 30.0, 90.0, true), ("friction_angle", 25.0, 45.0, false)]),
    (
        "Fire (3D GPU)",
        &[
            ("rise", 0.06, 0.25, false),
            ("vorticity", 0.2, 1.0, false),
            ("cooling", 0.8, 2.5, false),
            ("drag", 0.1, 0.5, false),
            ("burner", 0.7, 1.6, false),
            ("flicker", 0.1, 0.8, false),
            ("burn_rate", 1.5, 6.0, true),
            ("heat", 3.0, 10.0, true),
            ("soot", 0.1, 0.6, false),
        ],
    ),
    (
        "Smoke (3D GPU)",
        &[
            ("rise", 0.08, 0.3, false),
            ("buoyancy", -0.05, 0.08, false),
            ("cooling", 0.3, 1.2, false),
            ("drag", 0.1, 0.4, false),
            ("vorticity", 0.1, 0.8, false),
            ("flicker", 0.0, 0.6, false),
            ("burner", 0.6, 1.6, false),
        ],
    ),
];

/// What a random look runs on, and how often: a solver (empty for no fluid) and the
/// scenes that suit it. The 2D solvers stay in the scenes their built-ins use (the
/// dye solvers need a scene with an inflow or a column to stir; fire needs a burner).
const FLUIDS: &[(&str, &[&str], f32)] = &[
    ("", &[], 3.0),
    ("FLIP", &["Dam break", "Double dam break", "Tank with obstacle", "Fountain", "Martin & Moyce column"], 1.5),
    ("PBF", &["Dam break", "Double dam break", "Tank with obstacle", "Martin & Moyce column"], 1.0),
    ("Stable fluids (dye)", &["Dam break", "Double dam break", "Tank with obstacle", "Martin & Moyce column"], 2.0),
    ("Stable fluids (GPU)", &["Dam break", "Double dam break", "Tank with obstacle", "Martin & Moyce column", "Wind tunnel"], 1.5),
    ("Smoke & fire", &["Campfire"], 1.5),
    ("Lattice Boltzmann (dye)", &["Wind tunnel"], 1.0),
    ("Shallow water", &["Pond", "Breakwater", "Dam break", "Tank with obstacle", "Wind tunnel", "Falling blocks"], 1.5),
    ("MPM jelly", &["Falling blocks", "Double dam break", "Tank with obstacle"], 0.8),
    ("MPM snow", &["Falling blocks", "Dam break", "Double dam break"], 0.8),
    ("MPM sand", &["Dam break", "Double dam break", "Falling blocks", "Tank with obstacle"], 0.8),
    ("Fire (3D GPU)", &["Campfire 3D", "Torch", "Twin burners", "Chimney"], 1.5),
    ("Smoke (3D GPU)", &["Campfire 3D", "Torch", "Twin burners", "Chimney"], 1.0),
];

/// What a solver's fluid is like, for its reactions.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    None,
    Particles,
    Dye,
    Fire2d,
    Fire3d,
    Smoke3d,
    Water,
}

fn kind(solver: &str) -> Kind {
    match solver {
        "" => Kind::None,
        "FLIP" | "PBF" | "PBF (GPU)" | "MPM jelly" | "MPM snow" | "MPM sand" => Kind::Particles,
        "Smoke & fire" => Kind::Fire2d,
        "Fire (3D GPU)" => Kind::Fire3d,
        "Smoke (3D GPU)" => Kind::Smoke3d,
        "Shallow water" => Kind::Water,
        _ => Kind::Dye,
    }
}

/// A reaction a random look may have: trigger, effect and gain range.
type Pick = (Trigger, Effect, f32, f32);

/// The reactions that suit each kind of fluid (from the built-ins); the first is always
/// taken.
fn reaction_set(kind: Kind) -> &'static [Pick] {
    use Effect as E;
    use Trigger as T;
    match kind {
        Kind::None => &[(T::Kick, E::Flash, 0.4, 1.0), (T::Brightness, E::Hue, 0.3, 0.6), (T::Snare, E::Flash, 0.5, 0.8), (T::Hat, E::Flash, 0.3, 0.4)],
        Kind::Particles => &[
            (T::Kick, E::Splash, 0.6, 1.4),
            (T::Bass, E::Slosh, 0.5, 0.8),
            (T::Snare, E::Vortex, 0.5, 0.8),
            (T::Hat, E::Flash, 0.3, 0.8),
            (T::Brightness, E::Hue, 0.2, 0.5),
            (T::Kick, E::Jet, 0.8, 1.2),
        ],
        Kind::Dye => &[
            (T::Snare, E::Vortex, 0.5, 1.4),
            (T::Kick, E::Vortex, 0.4, 1.0),
            (T::Mid, E::Gust, 0.3, 0.6),
            (T::Hat, E::Flash, 0.3, 0.5),
            (T::Brightness, E::Hue, 0.3, 0.6),
            (T::Level, E::Breathe, 0.3, 0.5),
            (T::Beat, E::Vortex, 0.3, 0.5),
        ],
        Kind::Fire2d => &[(T::Kick, E::Jet, 0.4, 1.0), (T::Bass, E::Breathe, 0.4, 0.6), (T::Snare, E::Vortex, 0.5, 0.8), (T::Mid, E::Gust, 0.3, 0.5)],
        Kind::Fire3d => &[
            (T::Kick, E::Jet, 1.0, 1.4),
            (T::Snare, E::Vortex, 0.8, 1.2),
            (T::Level, E::Breathe, 0.3, 0.5),
            (T::Mid, E::Gust, 0.3, 0.5),
            (T::Bass, E::Slosh, 0.3, 0.5),
        ],
        Kind::Water => &[
            (T::Kick, E::Drop, 0.5, 1.2),
            (T::Snare, E::Drop, 0.5, 1.0),
            (T::Snare, E::Vortex, 0.3, 0.6),
            (T::Bass, E::Slosh, 0.2, 0.5),
            (T::Kick, E::Splash, 0.5, 1.0),
            (T::Brightness, E::Hue, 0.2, 0.4),
        ],
        Kind::Smoke3d => &[(T::Snare, E::Vortex, 0.5, 0.7), (T::Kick, E::Splash, 0.5, 0.7), (T::Hat, E::Flash, 0.3, 0.4), (T::Brightness, E::Hue, 0.4, 0.6)],
    }
}

/// The watchable ranges of `solver`'s physical parameters.
pub fn physics(solver: &str) -> &'static [Range] {
    PHYSICS.iter().find(|(s, _)| *s == solver).map_or(&[], |(_, r)| r)
}

/// Draws, on top of the RNG.
struct Dice<'r>(&'r mut SplitMix64);

impl Dice<'_> {
    fn chance(&mut self, p: f32) -> bool {
        self.0.next_f32() < p
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.0.next_f32()
    }

    fn log_range(&mut self, lo: f32, hi: f32) -> f32 {
        (lo.ln() + (hi.ln() - lo.ln()) * self.0.next_f32()).exp()
    }

    /// Either sign, `lo..hi` in size.
    fn signed(&mut self, lo: f32, hi: f32) -> f32 {
        let v = self.range(lo, hi);
        if self.chance(0.5) { -v } else { v }
    }

    fn index(&mut self, n: usize) -> usize {
        (self.0.next_u64() % n.max(1) as u64) as usize
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.index(items.len())]
    }

    fn weighted(&mut self, weights: &[f32]) -> usize {
        let mut x = self.range(0.0, weights.iter().sum());
        for (i, w) in weights.iter().enumerate() {
            if x < *w {
                return i;
            }
            x -= w;
        }
        weights.len() - 1
    }
}

/// A new random look named `name`.
pub fn random(rng: &mut SplitMix64, name: &str) -> Look {
    let d = &mut Dice(rng);
    let (solver, scenes, _) = FLUIDS[d.weighted(&FLUIDS.iter().map(|f| f.2).collect::<Vec<_>>())];
    let kind = kind(solver);
    let fluid = kind != Kind::None;
    let three_d = matches!(kind, Kind::Fire3d | Kind::Smoke3d);
    let scene = if fluid { d.pick(scenes) } else { "" };
    // By key, as a file lists them.
    let mut params: Vec<(String, f32)> = physics(solver).iter().map(|&(key, lo, hi, log)| (key.to_string(), round(if log { d.log_range(lo, hi) } else { d.range(lo, hi) }))).collect();
    params.sort_by(|a, b| a.0.cmp(&b.0));

    let mut feedback = random_feedback(d, fluid);
    let mut waves = random_waves(d, fluid, matches!(kind, Kind::Particles | Kind::Dye | Kind::Water));
    let composite = random_composite(d, fluid);
    let mut reactions = random_reactions(d, kind, solver);
    if d.chance(if !fluid { FRACTAL_ALONE } else if three_d { FRACTAL_OVER_3D } else { FRACTAL_OVER }) {
        waves.fractal = random_fractal(d, kind);
        // Without fluid the fractal is enough to draw: the waves, and the feedback, stay
        // only sometimes.
        if !fluid && d.chance(0.6) {
            waves.mode = WaveMode::Off;
            waves.shapes = [ShapeSettings::OFF; 2];
        }
        if !fluid && d.chance(0.4) {
            feedback.enabled = false;
        }
        if d.chance(0.5) {
            let trigger = d.pick(&[Trigger::Kick, Trigger::Snare]);
            reactions.push(Reaction::new(trigger, Effect::Morph, round(d.range(0.4, 0.9)), true));
        }
    }
    let code = random_code(d, kind, solver, feedback.enabled);
    let mut look = Look {
        name: name.to_string(),
        author: AUTHOR.to_string(),
        tags: vec!["random".to_string(), if fluid { "fluid" } else { "no fluid" }.to_string()],
        energy: Energy::Any,
        solver: solver.to_string(),
        scene: scene.to_string(),
        palette: random_palette(d),
        liquid: kind == Kind::Particles && d.chance(0.5),
        orbit: if three_d { round(d.range(0.05, 0.2)) } else { 0.0 },
        feedback,
        waves,
        composite,
        params,
        reactions,
        code,
        source: Source::Random,
    };
    tame(&mut look);
    look.energy = energy(&look);
    look
}

/// `look` nudged: its numbers by up to ±20 % (staying in range), and sometimes one of
/// its choices swapped (the wave mode, the symmetry, the feedback mode or the palette).
/// Its fluid stays the same; its equations too. Named `name`.
pub fn vary(look: &Look, rng: &mut SplitMix64, name: &str) -> Look {
    let d = &mut Dice(rng);
    let mut v = look.clone();
    (v.name, v.author, v.source) = (name.to_string(), AUTHOR.to_string(), Source::Random);
    if !v.tags.iter().any(|t| t == "random") {
        v.tags.insert(0, "random".to_string());
    }
    let mut nudge = |x: &mut f32| *x *= d.range(0.8, 1.2);
    let fb = &mut v.feedback;
    nudge(&mut fb.trail);
    let mut zoom = fb.zoom - 1.0;
    nudge(&mut zoom);
    fb.zoom = 1.0 + zoom;
    nudge(&mut fb.rotation);
    nudge(&mut fb.drift[0]);
    nudge(&mut fb.drift[1]);
    nudge(&mut fb.warp);
    nudge(&mut fb.bass_zoom);
    nudge(&mut fb.echo);
    let w = &mut v.waves;
    nudge(&mut w.scale);
    nudge(&mut w.size);
    nudge(&mut w.thickness);
    nudge(&mut w.brightness);
    for s in w.shapes.iter_mut().filter(|s| s.enabled) {
        nudge(&mut s.radius);
        nudge(&mut s.spin);
        nudge(&mut s.bass_pulse);
    }
    let f = &mut w.fractal;
    if f.enabled {
        nudge(&mut f.dive);
        nudge(&mut f.wander);
        nudge(&mut f.spin);
        nudge(&mut f.bass_zoom);
        nudge(&mut f.bend);
        nudge(&mut f.colour_speed);
        nudge(&mut f.bands);
        nudge(&mut f.brightness);
    }
    let c = &mut v.composite;
    nudge(&mut c.symmetry_spin);
    nudge(&mut c.hue_speed);
    nudge(&mut c.palette_map);
    nudge(&mut c.vignette);
    nudge(&mut c.aberration);
    for r in &mut v.reactions {
        nudge(&mut r.gain);
    }
    if v.orbit != 0.0 {
        nudge(&mut v.orbit);
    }
    let ranges = physics(&look.solver);
    for (key, value) in &mut v.params {
        if let Some(&(_, lo, hi, _)) = ranges.iter().find(|r| r.0 == key) {
            *value = round((*value * d.range(0.8, 1.2)).clamp(lo, hi));
        }
    }
    if d.chance(0.5) {
        match d.index(4) {
            0 => v.waves.mode = d.pick(&WAVES),
            1 => {
                v.composite.symmetry = d.pick(&Symmetry::ALL);
                v.composite.segments = 2 * d.range(2.0, 5.99) as u32;
            }
            2 => {
                v.feedback.mode = if v.feedback.mode == FeedbackMode::Smear { FeedbackMode::Brightest } else { FeedbackMode::Smear };
            }
            _ => v.palette = random_palette(d),
        }
    }
    if !v.has_fluid() && !v.waves.fractal.enabled {
        // Without fluid the picture is only what's drawn: keep something drawn.
        v.feedback.enabled = true;
        if !v.waves.any() {
            v.waves.mode = WaveMode::Circle;
        }
    }
    tame(&mut v);
    v
}

/// How often a random look has a fractal: with no fluid (then it's often the whole
/// picture), over a 2D fluid, over a 3D one.
const FRACTAL_ALONE: f32 = 0.45;
const FRACTAL_OVER: f32 = 0.2;
const FRACTAL_OVER_3D: f32 = 0.12;

/// The fractals a random look draws, and how often (Julia sets, which change shape as
/// they go, most).
const FRACTAL_KINDS: [(FractalKind, f32); 5] =
    [(FractalKind::Mandelbrot, 2.0), (FractalKind::Julia, 3.0), (FractalKind::BurningShip, 1.0), (FractalKind::Tricorn, 1.0), (FractalKind::Newton, 1.0)];

/// A fractal for a look over a fluid of `kind` (none: the fractal is most of the
/// picture). It moves one of three ways: diving (mostly in), touring the sections, or
/// staying put and turning; a Julia set's point wanders too, mostly.
fn random_fractal(d: &mut Dice, kind: Kind) -> FractalSettings {
    let fractal_kind = FRACTAL_KINDS[d.weighted(&FRACTAL_KINDS.map(|k| k.1))].0;
    // Often the whole set, so it starts out as itself: over a fluid mostly, as a
    // close-up's fine filaments are lost in it.
    let alone = kind == Kind::None;
    let spots = fractal::spots(fractal_kind);
    let (centre, zoom) = if d.chance(if alone { 0.5 } else { 0.8 }) { spots[0] } else { d.pick(spots) };
    let mut f = FractalSettings {
        enabled: true,
        kind: fractal_kind,
        centre,
        zoom,
        rotation: d.signed(0.0, std::f32::consts::PI),
        iterations: if fractal_kind == FractalKind::Newton { d.range(40.0, 100.0) } else { d.range(120.0, 300.0) } as u32,
        julia: fractal::cardioid_edge(d.range(0.0, std::f32::consts::TAU), 0.01),
        opacity: if alone { d.range(0.85, 1.0) } else { d.range(0.7, 0.95) },
        brightness: d.range(0.8, 1.3),
        bands: d.range(1.0, 4.0),
        colour_speed: d.signed(0.02, 0.15),
        bass_zoom: if d.chance(0.4) { d.range(0.1, 0.4) } else { 0.0 },
        // The fluid pushing it about, over a 2D fluid (whose flow is drawn in 2D).
        bend: if matches!(kind, Kind::Particles | Kind::Dye | Kind::Water | Kind::Fire2d) && d.chance(0.5) { d.range(0.2, 0.6) } else { 0.0 },
        ..FractalSettings::OFF
    };
    match d.index(3) {
        0 => f.dive = if d.chance(0.8) { d.range(0.3, 1.2) } else { -d.range(0.3, 0.8) },
        1 => f.tour = true,
        _ => f.spin = d.signed(0.02, 0.15),
    }
    if fractal_kind == FractalKind::Julia && d.chance(0.8) {
        f.wander = d.range(0.3, 1.5);
    }
    f.rounded()
}

/// The wave modes a random look draws.
const WAVES: [WaveMode; 5] = [WaveMode::Line, WaveMode::Circle, WaveMode::Spectrum, WaveMode::MirrorSpectrum, WaveMode::Xy];

fn random_feedback(d: &mut Dice, fluid: bool) -> FeedbackSettings {
    // Without fluid the feedback is the picture; with one it's a choice.
    if fluid && d.chance(0.2) {
        return FeedbackSettings::OFF;
    }
    // Without fluid, smear fades to the black between thin waves: nearly always brightest.
    let brightest = d.chance(if fluid { 0.55 } else { 0.9 });
    let zoom = if fluid {
        d.range(0.97, 1.2)
    } else if d.chance(0.75) {
        d.range(1.03, 1.3)
    } else if d.chance(0.5) {
        d.range(0.94, 0.99)
    } else {
        d.range(1.3, 1.6)
    };
    FeedbackSettings {
        enabled: true,
        mode: if brightest { FeedbackMode::Brightest } else { FeedbackMode::Smear },
        trail: if brightest { d.range(0.5, 0.85) } else { d.range(0.45, 0.9) },
        zoom,
        rotation: if d.chance(0.65) { d.signed(0.05, 0.35) } else { 0.0 },
        drift: if d.chance(0.25) { [d.signed(0.0, 0.1), d.signed(0.0, 0.1)] } else { [0.0, 0.0] },
        warp: if d.chance(0.3) { d.range(0.005, 0.03) } else { 0.0 },
        warp_speed: d.range(0.5, 2.0),
        flow: if fluid { d.range(0.6, 1.2) } else { 1.0 },
        bass_zoom: if d.chance(0.4) { d.range(0.1, 0.5) } else { 0.0 },
        echo: if d.chance(0.2) { d.range(0.15, 0.3) } else { 0.0 },
        echo_zoom: d.range(0.7, 1.5),
        echo_flip: d.pick(&[EchoFlip::None, EchoFlip::Horizontal, EchoFlip::Vertical, EchoFlip::Both]),
    }
    .rounded()
}

/// `pushes`: the fluid can be pushed by a line or circle.
fn random_waves(d: &mut Dice, fluid: bool, pushes: bool) -> WaveSettings {
    let mut w = WaveSettings::OFF;
    let shape_chance = if fluid { 0.15 } else { 0.35 };
    let shape = |d: &mut Dice| {
        if !d.chance(shape_chance) {
            return ShapeSettings::OFF;
        }
        ShapeSettings {
        enabled: true,
        sides: d.range(3.0, 8.99) as u32,
        radius: d.range(0.15, 0.5),
        centre: if d.chance(0.7) { [0.0, 0.0] } else { [d.signed(0.0, 0.4), d.signed(0.0, 0.4)] },
        spin: d.signed(0.0, 0.8),
        thickness: d.range(1.5, 4.0),
        bass_pulse: d.range(0.0, 0.5),
        fill: if d.chance(0.5) { 0.0 } else { d.range(0.1, 0.4) },
        }
    };
    w.shapes = [shape(d), shape(d)];
    let drawn = if fluid { d.chance(0.4) } else { d.chance(0.85) || !w.any() };
    if drawn {
        w.mode = d.pick(&WAVES);
        w.scale = d.range(0.6, 1.3);
        w.size = match w.mode {
            WaveMode::Spectrum | WaveMode::MirrorSpectrum => d.range(0.05, 0.2),
            _ => d.range(0.15, 0.45),
        };
        w.y = if w.mode == WaveMode::Line { d.signed(0.0, 0.3) } else { 0.0 };
        w.thickness = d.range(1.5, 4.0);
        w.brightness = d.range(0.6, 1.3);
        if pushes && matches!(w.mode, WaveMode::Line | WaveMode::Circle) && d.chance(0.3) {
            w.push = d.range(0.2, 0.6);
        }
    }
    w.rounded()
}

fn random_composite(d: &mut Dice, fluid: bool) -> CompositeSettings {
    let mut c = CompositeSettings::OFF;
    if d.chance(0.35) {
        c.symmetry = d.pick(&Symmetry::ALL[1..]);
        c.segments = 2 * d.range(2.0, 5.99) as u32;
        if c.symmetry == Symmetry::Kaleidoscope && d.chance(0.5) {
            c.symmetry_spin = d.signed(0.02, 0.1);
        }
    }
    if d.chance(0.3) {
        c.hue_speed = d.signed(5.0, 20.0);
    }
    c.saturation = d.range(0.9, 1.3);
    c.contrast = d.range(0.95, 1.3);
    c.exposure = d.range(1.0, 1.2);
    if d.chance(if fluid { 0.3 } else { 0.45 }) {
        c.palette_map = d.range(0.2, 0.7);
    }
    if d.chance(0.4) {
        c.aberration = d.range(0.005, 0.02);
    }
    if d.chance(0.6) {
        c.vignette = d.range(0.2, 0.5);
    }
    if d.chance(0.25) {
        c.grain = d.range(0.02, 0.06);
    }
    c.rounded()
}

fn random_reactions(d: &mut Dice, kind: Kind, solver: &str) -> Vec<Reaction> {
    let set = reaction_set(kind);
    let count = d.range(2.0, 4.99) as usize;
    let mut order: Vec<usize> = (1..set.len()).collect();
    for i in (1..order.len()).rev() {
        order.swap(i, d.index(i + 1));
    }
    let mut reactions: Vec<Reaction> =
        std::iter::once(0).chain(order).take(count).map(|i| set[i]).map(|(t, e, lo, hi)| Reaction::new(t, e, round(d.range(lo, hi)), true)).collect();
    // Now and then the bass moves one of the fluid's own settings.
    let ranges = physics(solver);
    if !ranges.is_empty() && d.chance(0.2) {
        let (key, ..) = d.pick(ranges);
        reactions.push(Reaction { param: Some(key.to_string()), ..Reaction::new(Trigger::Bass, Effect::Param, round(d.range(0.3, 0.6)), true) });
    }
    reactions
}

/// Equations from the snippets, sometimes: per frame, a wandering centre, beat pulses,
/// a swinging turn or hue, a pulsing vignette, gravity leaning or swinging (2D fluids);
/// per pixel (with feedback on), a spiral twist, ripples, petals or a faster rim.
fn random_code(d: &mut Dice, kind: Kind, solver: &str, feedback: bool) -> Code {
    let mut frame = Vec::new();
    let mut pixel = None;
    let n = |d: &mut Dice, lo: f32, hi: f32| format!("{}", round(d.range(lo, hi)));
    if d.chance(0.5) {
        let mut kinds: Vec<usize> = (0..5).collect();
        for _ in 0..d.range(1.0, 3.99) as usize {
            let snippet = kinds.remove(d.index(kinds.len()));
            let line = match snippet {
                0 => {
                    let a = n(d, 0.04, 0.12);
                    format!("cx = 0.5 + {a} * sin(time * {})\ncy = 0.5 + {a} * cos(time * {})", n(d, 0.2, 0.9), n(d, 0.2, 0.9))
                }
                1 => format!("zoom = zoom + {} * beat", if kind == Kind::None { n(d, 0.1, 0.4) } else { n(d, 0.03, 0.12) }),
                2 => format!("rot = rot + {} * sin(time * {})", n(d, 0.1, 0.4), n(d, 0.1, 0.6)),
                3 => format!("hue = {} * sin(time * {})", n(d, 20.0, 60.0), n(d, 0.05, 0.3)),
                _ => format!("vignette = {} + {} * beat", n(d, 0.15, 0.35), n(d, 0.1, 0.3)),
            };
            frame.push(line);
        }
        if feedback && d.chance(0.5) {
            pixel = Some(match d.index(4) {
                0 => format!("rot = rot * ({} + {} * rad)", n(d, 0.2, 0.6), n(d, 0.8, 2.0)),
                1 => format!("zoom = zoom + {} * sin(rad * {} - time * {})", n(d, 0.03, 0.08), n(d, 10.0, 24.0), n(d, 1.0, 4.0)),
                2 => format!("zoom = zoom + {} * sin({} * ang + time) * rad", n(d, 0.04, 0.1), d.range(3.0, 7.99) as u32),
                _ => "zoom = 1 + (zoom - 1) * (0.4 + 1.4 * rad)".to_string(),
            });
        }
    }
    // The 2D fluids' gravity leans or swings now and then.
    if matches!(kind, Kind::Particles | Kind::Dye | Kind::Water) && d.chance(0.35) {
        frame.push(if d.chance(0.5) { format!("gravity = {}", n(d, -60.0, 60.0)) } else { format!("gravity = {} * sin(time * {})", n(d, 20.0, 70.0), n(d, 0.1, 0.4)) });
        if d.chance(0.5) {
            frame.push(format!("gravity_strength = {}", n(d, 0.6, 1.5)));
        }
    }
    // And now and then the music moves one of its settings.
    let ranges = physics(solver);
    if !ranges.is_empty() && d.chance(0.15) {
        let (key, lo, hi, _) = d.pick(ranges);
        let mid = round(0.5 * (lo + hi));
        let swing = round(0.25 * (hi - lo));
        frame.push(format!("param_{key} = {mid} + {swing} * (bass_att - 1)"));
    }
    let source = CodeSource { init: None, frame: (!frame.is_empty()).then(|| frame.join("\n")), pixel };
    compile_code(&source).unwrap_or_else(|e| panic!("a random equation didn't compile: {e:?}\n{source:?}"))
}

/// A palette: a built-in one, or (more often) one made from a hue scheme (analogous,
/// complementary or triadic), deep, base and highlight ordered by brightness.
fn random_palette(d: &mut Dice) -> Palette {
    if d.chance(0.4) {
        return d.pick(PALETTES).1;
    }
    let hue = d.range(0.0, 360.0);
    let (deep, high) = match d.index(3) {
        0 => (hue - d.range(20.0, 40.0), hue + d.range(20.0, 40.0)),
        1 => (hue, hue + 180.0),
        _ => (hue + 120.0, hue + 240.0),
    };
    let mut p = Palette { deep: hsv(deep, d.range(0.7, 0.9), d.range(0.3, 0.45)), base: hsv(hue, d.range(0.65, 0.85), d.range(0.8, 0.95)), highlight: hsv(high, d.range(0.15, 0.4), 1.0) };
    // A base much brighter than this washes the picture out once the feedback piles it up.
    let l = luma(p.base);
    if l > MAX_BASE_LUMA {
        p.base = p.base.map(|c| (f32::from(c) * MAX_BASE_LUMA / l).round() as u8);
    }
    let mut colours = [p.deep, p.base, p.highlight];
    colours.sort_by(|a, b| luma(*a).total_cmp(&luma(*b)));
    [p.deep, p.base, p.highlight] = colours;
    // The highlight well above the base, so foam and dense dye show.
    if luma(p.highlight) < luma(p.base) + 0.2 {
        p.highlight = p.highlight.map(|c| (f32::from(c) + 0.6 * (255.0 - f32::from(c))) as u8);
    }
    p
}

/// The brightest a made-up palette's base is (see `luma`).
const MAX_BASE_LUMA: f32 = 0.55;

/// Relative brightness of an sRGB colour, 0 to 1 (on the gamma-encoded values, which is
/// how the palettes are ordered).
pub fn luma(c: [u8; 3]) -> f32 {
    (0.2126 * f32::from(c[0]) + 0.7152 * f32::from(c[1]) + 0.0722 * f32::from(c[2])) / 255.0
}

fn hsv(hue: f32, s: f32, v: f32) -> [u8; 3] {
    let h = hue.rem_euclid(360.0) / 60.0;
    let c = v * s;
    let x = c * (1.0 - (h.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r, g, b].map(|u| ((u + v - c) * 255.0).round().clamp(0.0, 255.0) as u8)
}

/// Keep `look` watchable: every value in its range, and none of the combinations that
/// blew out or went black in the built-ins' tuning.
fn tame(look: &mut Look) {
    let (fluid, bright_palette) = (look.has_fluid(), luma(look.palette.base) > 0.5);
    let fb = &mut look.feedback;
    // Water's picture is its thin, lit wave fronts: any feedback smears them to mush
    // (and brightest piles the glints up into white).
    if kind(&look.solver) == Kind::Water {
        fb.enabled = false;
    }
    fb.trail = fb.trail.clamp(0.3, 0.92);
    fb.zoom = fb.zoom.clamp(0.9, 1.7);
    fb.rotation = fb.rotation.clamp(-0.5, 0.5);
    fb.drift = fb.drift.map(|x| x.clamp(-0.15, 0.15));
    fb.warp = fb.warp.clamp(0.0, 0.04);
    fb.bass_zoom = fb.bass_zoom.clamp(0.0, 0.6);
    fb.echo = fb.echo.clamp(0.0, 0.35);
    let w = &mut look.waves;
    w.scale = w.scale.clamp(0.4, 1.6);
    w.size = w.size.clamp(0.04, 0.5);
    w.thickness = w.thickness.clamp(1.0, 5.0);
    if w.mode != WaveMode::Off {
        w.brightness = w.brightness.clamp(0.4, 1.4);
    }
    // The shapes in use first, as a file lists them.
    w.shapes.sort_by_key(|s| !s.enabled);
    for s in &mut w.shapes {
        s.radius = s.radius.clamp(0.1, 0.6);
        s.spin = s.spin.clamp(-1.0, 1.0);
        s.bass_pulse = s.bass_pulse.clamp(0.0, 0.6);
    }
    let f = &mut w.fractal;
    f.dive = f.dive.clamp(-1.5, 1.5);
    f.wander = f.wander.clamp(0.0, 2.0);
    f.spin = f.spin.clamp(-0.2, 0.2);
    f.bass_zoom = f.bass_zoom.clamp(0.0, 0.5);
    f.bend = f.bend.clamp(0.0, 0.8);
    f.colour_speed = f.colour_speed.clamp(-0.2, 0.2);
    f.bands = f.bands.clamp(0.5, 5.0);
    f.brightness = f.brightness.clamp(0.5, 1.4);
    f.opacity = f.opacity.clamp(0.3, 1.0);
    // A diving or wandering fractal with no fluid is new every frame: feedback, which
    // carries the frames before, smears it into a blur (seen in the app). The dive is
    // the tunnel.
    if f.enabled && !fluid && (f.dive != 0.0 || f.wander != 0.0) {
        fb.enabled = false;
    }
    if f.enabled && fb.enabled {
        // A fractal's glow covers the canvas: brightest feedback piles it up into a pale
        // wash (even dimmed). And it reads only while each new frame is most of the
        // picture: a long trail, zoom, turn or echo blurs it to mush (both seen in the
        // app). So a short smear, nearly still.
        fb.mode = FeedbackMode::Smear;
        fb.trail = fb.trail.min(0.5);
        fb.zoom = fb.zoom.clamp(0.99, 1.02);
        fb.rotation = fb.rotation.clamp(-0.1, 0.1);
        fb.echo = 0.0;
        fb.drift = [0.0; 2];
        fb.bass_zoom = fb.bass_zoom.min(0.15);
    }
    if fb.enabled && fb.mode == FeedbackMode::Brightest {
        // A strong zoom piles bright waves up into white: dim waves, a shorter trail.
        if fb.zoom > 1.35 {
            w.brightness = w.brightness.min(0.8);
            fb.trail = fb.trail.min(0.65);
        }
        // A long bright trail with much zoom washes out.
        if fb.trail > 0.8 {
            fb.zoom = fb.zoom.min(1.15);
        }
        // Echo and bright, long trails together blow out.
        if fb.trail > 0.75 {
            fb.echo = fb.echo.min(0.2);
        }
        // So do bright waves under any zoom or long trail, and filled shapes.
        if fb.zoom > 1.1 || fb.trail > 0.75 {
            w.brightness = w.brightness.min(0.9);
        }
        for s in &mut w.shapes {
            s.fill = s.fill.min(0.15);
        }
        // Particles and dye are bright everywhere: keep their trails short.
        if fluid {
            fb.trail = fb.trail.min(0.65);
        }
    }
    let brightest = fb.enabled && fb.mode == FeedbackMode::Brightest;
    let c = &mut look.composite;
    // A kaleidoscope folds a fractal into a pattern no one could read (seen in the app);
    // a mirror keeps it.
    if look.waves.fractal.enabled && c.symmetry == Symmetry::Kaleidoscope {
        c.symmetry = Symmetry::None;
    }
    if brightest {
        c.exposure = c.exposure.min(1.05);
        if bright_palette {
            c.palette_map = c.palette_map.min(0.3);
        }
    }
    c.symmetry_spin = c.symmetry_spin.clamp(-0.15, 0.15);
    c.hue_speed = c.hue_speed.clamp(-25.0, 25.0);
    c.palette_map = c.palette_map.clamp(0.0, 0.8);
    c.aberration = c.aberration.clamp(0.0, 0.025);
    c.vignette = c.vignette.clamp(0.0, 0.6);
    c.segments = c.segments.clamp(2, 16);
    for r in &mut look.reactions {
        r.gain = r.gain.clamp(0.1, 1.5);
    }
    look.orbit = look.orbit.clamp(0.0, 0.3);
    look.feedback = look.feedback.rounded();
    look.waves = look.waves.rounded();
    look.composite = look.composite.rounded();
    for r in &mut look.reactions {
        r.gain = round(r.gain);
    }
    look.orbit = round(look.orbit);
}

/// Calm, energetic or either, from how hard the look moves.
fn energy(look: &Look) -> Energy {
    let fb = &look.feedback;
    let f = &look.waves.fractal;
    let fractal = if f.enabled { 0.3 * f.dive.abs() + 0.2 * f.wander + f.bass_zoom } else { 0.0 };
    let motion = fractal + if fb.enabled { 2.0 * (fb.zoom - 1.0).abs() + fb.rotation.abs() + fb.bass_zoom } else { 0.0 };
    let hits: f32 = look.reactions.iter().filter(|r| matches!(r.trigger, Trigger::Kick | Trigger::Snare | Trigger::Beat)).map(|r| r.gain).sum();
    match motion + 0.3 * hits {
        x if x > 0.9 => Energy::Energetic,
        x if x < 0.45 => Energy::Calm,
        _ => Energy::Any,
    }
}

/// Three decimals, so a random look's file reads cleanly.
fn round(x: f32) -> f32 {
    (x * 1000.0).round() / 1000.0
}

/// Every number rounded (see `round`).
trait Rounded {
    fn rounded(self) -> Self;
}

impl Rounded for FeedbackSettings {
    fn rounded(self) -> Self {
        Self {
            trail: round(self.trail),
            zoom: round(self.zoom),
            rotation: round(self.rotation),
            drift: self.drift.map(round),
            warp: round(self.warp),
            warp_speed: round(self.warp_speed),
            flow: round(self.flow),
            bass_zoom: round(self.bass_zoom),
            echo: round(self.echo),
            echo_zoom: round(self.echo_zoom),
            ..self
        }
    }
}

impl Rounded for WaveSettings {
    fn rounded(self) -> Self {
        let shape = |s: ShapeSettings| ShapeSettings {
            radius: round(s.radius),
            centre: s.centre.map(round),
            spin: round(s.spin),
            thickness: round(s.thickness),
            bass_pulse: round(s.bass_pulse),
            fill: round(s.fill),
            ..s
        };
        Self {
            scale: round(self.scale),
            size: round(self.size),
            y: round(self.y),
            thickness: round(self.thickness),
            brightness: round(self.brightness),
            push: round(self.push),
            shapes: self.shapes.map(shape),
            fractal: self.fractal.rounded(),
            ..self
        }
    }
}

impl Rounded for FractalSettings {
    fn rounded(self) -> Self {
        Self {
            centre: self.centre.map(|c| (c * 1e6).round() / 1e6),
            zoom: round(self.zoom),
            rotation: round(self.rotation),
            spin: round(self.spin),
            julia: self.julia.map(|c| (c * 1e6).round() / 1e6),
            opacity: round(self.opacity),
            brightness: round(self.brightness),
            bands: round(self.bands),
            colour_speed: round(self.colour_speed),
            wander: round(self.wander),
            bass_zoom: round(self.bass_zoom),
            bend: round(self.bend),
            dive: round(self.dive),
            ..self
        }
    }
}

impl Rounded for CompositeSettings {
    fn rounded(self) -> Self {
        Self {
            symmetry_spin: round(self.symmetry_spin),
            hue: round(self.hue),
            hue_speed: round(self.hue_speed),
            saturation: round(self.saturation),
            contrast: round(self.contrast),
            exposure: round(self.exposure),
            palette_map: round(self.palette_map),
            invert: round(self.invert),
            aberration: round(self.aberration),
            vignette: round(self.vignette),
            grain: round(self.grain),
            ..self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::looks::{Library, file};
    use crate::ui::{UiState, params};

    fn looks(n: usize) -> Vec<Look> {
        let mut rng = SplitMix64::new(7);
        (0..n).map(|i| random(&mut rng, &format!("Random {}", i + 1))).collect()
    }

    /// A thousand random looks all load: each written as a file reads back the same,
    /// equations compiled.
    #[test]
    fn random_looks_round_trip_through_their_files() {
        for look in looks(1000) {
            let text = file::write(&look);
            let back = file::parse(&text, "random.toml", Source::Random).unwrap_or_else(|e| panic!("{e}\n{text}"));
            assert_eq!(back, look, "{text}");
        }
    }

    /// Every value stays in range, the palettes are ordered by brightness, and the
    /// combinations that blow out don't appear.
    #[test]
    fn random_looks_stay_in_range() {
        let all = looks(1000);
        for look in &all {
            let (fb, w, c) = (&look.feedback, &look.waves, &look.composite);
            assert!((0.3..=0.92).contains(&fb.trail) && (0.9..=1.7).contains(&fb.zoom), "{}: {fb:?}", look.name);
            assert!((w.mode == WaveMode::Off || (0.4..=1.4).contains(&w.brightness)) && (0.0..=0.6).contains(&c.vignette), "{}", look.name);
            assert!(look.solver != "Shallow water" || !fb.enabled, "{}: water under feedback", look.name);
            if fb.enabled && fb.mode == FeedbackMode::Brightest {
                assert!(fb.zoom <= 1.35 || (w.brightness <= 0.8 && fb.trail <= 0.65), "{}: bright zoom", look.name);
                assert!(fb.trail <= 0.8 || fb.zoom <= 1.15, "{}: long bright trail", look.name);
                assert!(c.exposure <= 1.05 && w.shapes.iter().all(|s| s.fill <= 0.15), "{}: bright exposure or fill", look.name);
                assert!(!look.has_fluid() || fb.trail <= 0.65, "{}: long trail on a fluid", look.name);
            }
            let p = look.palette;
            assert!(p.name().is_some() || luma(p.base) <= MAX_BASE_LUMA + 0.01, "{}: base too bright {p:?}", look.name);
            assert!(luma(p.deep) <= luma(p.base) && luma(p.base) < luma(p.highlight), "{}: {p:?}", look.name);
            for (key, value) in &look.params {
                let &(_, lo, hi, _) = physics(&look.solver).iter().find(|r| r.0 == key).expect("a known parameter");
                assert!((lo..=hi).contains(value), "{}: {key} = {value}", look.name);
            }
            assert!(!look.reactions.is_empty() && look.reactions.iter().all(|r| (0.1..=1.5).contains(&r.gain)));
            if !look.has_fluid() {
                assert!(w.fractal.enabled || (fb.enabled && w.any()), "{}: nothing drawn", look.name);
            }
            let f = &w.fractal;
            if f.enabled {
                assert!((-1.5..=1.5).contains(&f.dive) && (0.0..=0.8).contains(&f.bend) && (0.3..=1.0).contains(&f.opacity), "{}: {f:?}", look.name);
                assert!(f.dive == 0.0 || !f.tour, "{}: dives and tours", look.name);
                assert!(look.has_fluid() || (f.dive == 0.0 && f.wander == 0.0) || !fb.enabled, "{}: a moving fractal smeared by feedback", look.name);
                if fb.enabled {
                    assert!(fb.mode == FeedbackMode::Smear && fb.trail <= 0.5 && (0.99..=1.02).contains(&fb.zoom) && fb.echo == 0.0, "{}: a fractal under {fb:?}", look.name);
                }
                assert!(c.symmetry != Symmetry::Kaleidoscope, "{}: a fractal in a kaleidoscope", look.name);
                // Its starting place is one worth seeing.
                assert!(fractal::spots(f.kind).iter().any(|s| s.0 == f.centre || (s.0[0] - f.centre[0]).abs() < 1e-5), "{}: {:?}", look.name, f.centre);
            }
            assert_eq!(look.solver_3d().is_some(), look.orbit > 0.0, "{}", look.name);
        }
        // Varied: every kind of fluid, with and without equations, every wave mode.
        for (solver, ..) in FLUIDS {
            assert!(all.iter().any(|l| l.solver == *solver), "never {solver:?}");
        }
        for mode in WAVES {
            assert!(all.iter().any(|l| l.waves.mode == mode), "never {mode:?}");
        }
        // Fractals: every kind, alone and over a fluid, diving in and out, touring, bent.
        let fractals: Vec<&FractalSettings> = all.iter().map(|l| &l.waves.fractal).filter(|f| f.enabled).collect();
        for (kind, _) in FRACTAL_KINDS {
            assert!(fractals.iter().any(|f| f.kind == kind), "never a {kind:?}");
        }
        assert!(all.iter().any(|l| l.waves.fractal.enabled && l.has_fluid()) && all.iter().any(|l| l.waves.fractal.enabled && !l.has_fluid()));
        assert!(fractals.iter().any(|f| f.dive > 0.0) && fractals.iter().any(|f| f.dive < 0.0) && fractals.iter().any(|f| f.tour) && fractals.iter().any(|f| f.bend > 0.0));
        assert!(all.iter().any(|l| l.reactions.iter().any(|r| r.effect == Effect::Morph)));
        assert!(all.iter().any(|l| l.code.frame.is_some()) && all.iter().any(|l| !l.code.pixel.is_empty()) && all.iter().any(|l| l.code.frame.is_none()));
        let energies = [Energy::Calm, Energy::Energetic, Energy::Any];
        assert!(energies.iter().all(|e| all.iter().any(|l| l.energy == *e)), "energies not all used");
    }

    /// Variations stay near their look and in range, keep its fluid, and load.
    #[test]
    fn variations_stay_close_and_in_range() {
        let library = Library::builtin();
        let mut rng = SplitMix64::new(3);
        for look in library.looks.iter().chain(&looks(100)) {
            for _ in 0..5 {
                let v = vary(look, &mut rng, "Random 9");
                assert_eq!((&v.solver, &v.scene, v.author.as_str(), &v.code), (&look.solver, &look.scene, AUTHOR, &look.code), "{}", look.name);
                assert_eq!(v.params.len(), look.params.len());
                if look.feedback.enabled && v.feedback.trail > 0.3 && v.feedback.trail < 0.92 {
                    let ratio = v.feedback.trail / look.feedback.trail;
                    assert!((0.79..=1.21).contains(&ratio) || v.feedback.mode == FeedbackMode::Brightest, "{}: trail {ratio}", look.name);
                }
                let back = file::parse(&file::write(&v), "v.toml", Source::Random).unwrap_or_else(|e| panic!("{}: {e}", look.name));
                assert_eq!(back, v);
            }
        }
    }

    /// Every random physics range is inside its solver's control, for the 2D solvers
    /// (the 3D ones are checked against a GPU world when there is one).
    #[test]
    fn physics_ranges_fit_the_controls() {
        struct Specs(Vec<(&'static str, f32, f32)>);
        impl sim::ParamVisitor for Specs {
            fn float(&mut self, s: &sim::FloatSpec, _: &mut f32) {
                self.0.push((s.key, *s.range.start(), *s.range.end()));
            }
            fn int(&mut self, _: &sim::IntSpec, _: &mut u32) {}
            fn toggle(&mut self, _: &sim::ToggleSpec, _: &mut bool) {}
        }
        let check = |solver: &str, world: &mut dyn params::Params| {
            let mut specs = Specs(Vec::new());
            world.visit_params(&mut specs);
            for &(key, lo, hi, _) in physics(solver) {
                let &(_, min, max) = specs.0.iter().find(|s| s.0 == key).unwrap_or_else(|| panic!("{solver} has no {key}"));
                assert!(min <= lo && hi <= max && lo < hi, "{solver}: {key} {lo}..{hi} outside {min}..{max}");
            }
        };
        let mut world = sim::World::with_solvers(&crate::SOLVERS, 0, 0);
        for (solver, ..) in FLUIDS.iter().filter(|f| !f.0.is_empty() && !f.0.contains("3D")) {
            let index = world.solvers.iter().position(|s| s.name == *solver).unwrap_or_else(|| panic!("no solver {solver}"));
            world.switch_solver(index, false);
            check(solver, &mut world);
        }
        for (scene_solver, scenes, _) in FLUIDS.iter().filter(|f| !f.0.is_empty()) {
            let presets: Vec<&str> =
                if scene_solver.contains("3D") { sim3d::PRESETS.iter().map(|p| p.name).collect() } else { sim::PRESETS.iter().map(|p| p.name).collect() };
            assert!(scenes.iter().all(|s| presets.contains(s)), "{scene_solver}: a scene that doesn't exist");
        }
        for (k, s) in sim3d::SOLVERS.iter().enumerate() {
            assert!(!physics(s.name).is_empty(), "no physics for {}", s.name);
            let Ok(gpu) = sim3d::GpuContext::new() else { return };
            let mut world = sim3d::World3d::new(gpu, k, 0);
            check(s.name, &mut world);
        }
    }

    /// A random look applies to a 2D world with every parameter known, and its
    /// equations set only parameters the solver has.
    #[test]
    fn random_looks_apply() {
        for look in looks(200).iter().filter(|l| l.has_fluid() && l.solver_3d().is_none()) {
            let mut world = sim::World::with_solvers(&crate::SOLVERS, 0, 0);
            let mut ui = UiState::default();
            assert!(crate::looks::apply(look, &mut world, &mut ui).is_empty(), "{}: unknown parameters", look.name);
            for key in look.code.frame.iter().flat_map(|p| &p.params).chain(look.reactions.iter().filter_map(|r| r.param.as_ref())) {
                assert!(params::float_param(&mut world, key, None).is_some(), "{}: no {key}", look.name);
            }
        }
    }
}
