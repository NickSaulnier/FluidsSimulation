//! Visualizer looks: hand-tuned bundles of a solver, a scene, colours, look settings,
//! solver parameters and reactions, which the director (see `director`) switches
//! between with the music. Solvers and scenes are named, not indexed, so the library
//! survives registry changes.

use crate::audio::reactions::{Effect, Reaction, Trigger};
use crate::render::palette::PALETTES;
use crate::ui::{UiState, params};
use sim::{PRESETS, World};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Energy {
    /// For quiet, ambient passages.
    Calm,
    /// For loud, driving ones.
    Energetic,
    /// Works for either.
    Any,
}

pub struct Look {
    pub name: &'static str,
    pub solver: &'static str,
    pub scene: &'static str,
    pub palette: &'static str,
    /// Liquid surface (particle solvers).
    pub liquid: bool,
    pub energy: Energy,
    /// Solver parameters, by key (ints are rounded).
    pub params: &'static [(&'static str, f32)],
    pub reactions: fn() -> Vec<Reaction>,
}

impl Look {
    pub fn suits(&self, energetic: bool) -> bool {
        match self.energy {
            Energy::Any => true,
            Energy::Calm => !energetic,
            Energy::Energetic => energetic,
        }
    }
}

fn r(trigger: Trigger, effect: Effect, gain: f32) -> Reaction {
    Reaction::new(trigger, effect, gain, true)
}

fn param(trigger: Trigger, gain: f32, key: &str) -> Reaction {
    Reaction { param: Some(key.to_string()), ..r(trigger, Effect::Param, gain) }
}

pub static LOOKS: &[Look] = &[
    Look {
        name: "Ocean pulse",
        solver: "FLIP",
        scene: "Tank with obstacle",
        palette: "Ocean",
        liquid: true,
        energy: Energy::Any,
        params: &[],
        reactions: || {
            vec![
                r(Trigger::Kick, Effect::Splash, 1.2),
                r(Trigger::Bass, Effect::Slosh, 0.8),
                r(Trigger::Snare, Effect::Vortex, 0.6),
                r(Trigger::Hat, Effect::Flash, 0.5),
                r(Trigger::Brightness, Effect::Hue, 0.3),
            ]
        },
    },
    Look {
        name: "Ink storm",
        solver: "Stable fluids (dye)",
        scene: "Double dam break",
        palette: "Ink",
        liquid: false,
        energy: Energy::Energetic,
        params: &[("buoyancy", 0.2), ("vorticity", 0.3)],
        reactions: || {
            vec![
                r(Trigger::Snare, Effect::Vortex, 1.4),
                r(Trigger::Kick, Effect::Vortex, 0.8),
                r(Trigger::Mid, Effect::Gust, 0.6),
                r(Trigger::Hat, Effect::Flash, 0.4),
                r(Trigger::Brightness, Effect::Hue, 0.6),
            ]
        },
    },
    Look {
        name: "Firestorm",
        solver: "Smoke & fire",
        scene: "Campfire",
        palette: "Lava",
        liquid: false,
        energy: Energy::Energetic,
        params: &[],
        reactions: || {
            vec![
                r(Trigger::Kick, Effect::Jet, 1.0),
                r(Trigger::Bass, Effect::Breathe, 0.6),
                r(Trigger::Snare, Effect::Vortex, 0.7),
                r(Trigger::Mid, Effect::Gust, 0.4),
            ]
        },
    },
    Look {
        name: "Wind tunnel",
        solver: "Lattice Boltzmann (dye)",
        scene: "Wind tunnel",
        palette: "Slime",
        liquid: false,
        energy: Energy::Any,
        params: &[],
        reactions: || {
            vec![
                r(Trigger::Snare, Effect::Vortex, 1.0),
                r(Trigger::Kick, Effect::Vortex, 0.6),
                r(Trigger::Hat, Effect::Flash, 0.5),
                r(Trigger::Brightness, Effect::Hue, 0.4),
            ]
        },
    },
    Look {
        name: "Lava lamp",
        solver: "Stable fluids (dye)",
        scene: "Dam break",
        palette: "Lava",
        liquid: false,
        energy: Energy::Calm,
        params: &[("buoyancy", 0.4), ("vorticity", 0.05)],
        reactions: || {
            vec![
                param(Trigger::Bass, 0.8, "buoyancy"),
                r(Trigger::Level, Effect::Breathe, 0.4),
                r(Trigger::Brightness, Effect::Hue, 0.4),
                r(Trigger::Kick, Effect::Splash, 0.5),
            ]
        },
    },
    Look {
        name: "Mercury rain",
        solver: "PBF",
        scene: "Tank with obstacle",
        palette: "Mercury",
        liquid: false,
        energy: Energy::Any,
        params: &[],
        reactions: || {
            vec![
                r(Trigger::Kick, Effect::Splash, 1.4),
                r(Trigger::Bass, Effect::Slosh, 0.5),
                r(Trigger::Hat, Effect::Flash, 0.8),
                r(Trigger::Snare, Effect::Vortex, 0.5),
            ]
        },
    },
];

/// Switch the world and UI to `look`. Unknown solver or scene names (a renamed preset)
/// leave that part as it is.
pub fn apply(look: &Look, world: &mut World, ui: &mut UiState) {
    if let Some(solver) = world.solvers.iter().position(|s| s.name == look.solver)
        && solver != world.solver_index
    {
        world.switch_solver(solver, false);
    }
    for &(key, value) in look.params {
        if params::float_param(world, key, Some(value)).is_none() {
            params::int_param(world, key, Some(value.round() as u32));
        }
    }
    // Loading the scene also resets the solver, which applies restart-only parameters.
    match PRESETS.iter().position(|p| p.name == look.scene) {
        Some(scene) => world.load_preset(scene),
        None => world.reset(),
    }
    if let Some((_, palette)) = PALETTES.iter().find(|(name, _)| *name == look.palette) {
        ui.palette = *palette;
    }
    ui.liquid = look.liquid;
    ui.show_particles = true;
    ui.overlay = None;
    ui.reactions = (look.reactions)();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every look names a real solver, scene, palette and parameter, and has reactions.
    #[test]
    fn every_look_refers_to_things_that_exist() {
        for look in LOOKS {
            assert!(sim::SOLVERS.iter().any(|s| s.name == look.solver), "{}: no solver {}", look.name, look.solver);
            assert!(PRESETS.iter().any(|p| p.name == look.scene), "{}: no scene {}", look.name, look.scene);
            assert!(PALETTES.iter().any(|(n, _)| *n == look.palette), "{}: no palette {}", look.name, look.palette);
            assert!(!(look.reactions)().is_empty(), "{}: no reactions", look.name);
            let mut world = World::new(0, 0);
            let mut ui = UiState::default();
            apply(look, &mut world, &mut ui);
            assert_eq!(world.solvers[world.solver_index].name, look.solver);
            assert_eq!(PRESETS[world.preset_index].name, look.scene);
            for &(key, value) in look.params {
                let got = params::float_param(&mut world, key, None).or(params::int_param(&mut world, key, None).map(|v| v as f32));
                assert_eq!(got, Some(value), "{}: parameter {key}", look.name);
            }
            for reaction in (look.reactions)() {
                if let Some(key) = &reaction.param {
                    assert!(params::float_param(&mut world, key, None).is_some(), "{}: reaction on unknown {key}", look.name);
                }
            }
        }
    }

    #[test]
    fn there_are_looks_for_quiet_and_loud_music() {
        assert!(LOOKS.iter().filter(|l| l.suits(false)).count() >= 2);
        assert!(LOOKS.iter().filter(|l| l.suits(true)).count() >= 2);
    }
}
