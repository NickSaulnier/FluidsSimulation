//! Keeps a presentation in real time. If the simulation keeps falling behind (slow
//! motion), lower the solver's resolution a step rather than let it drift out of sync
//! with the music. Only in presentation mode: in the toy, slow motion is a fair trade
//! for detail, and the user can see and change the settings.

use crate::ui::params;
use sim::World;

/// Slow motion above this fraction counts as falling behind...
const SLOW: f32 = 0.1;
/// ...when it lasts this long, seconds.
const SUSTAIN_S: f32 = 3.0;
/// After a change, wait this long before judging again (the new resolution needs time
/// to show its cost).
const COOLDOWN_S: f32 = 10.0;
/// Each step: grids get this much coarser, particles this much further apart.
const GRID_STEP: f32 = 0.8;
const SPACING_STEP: f32 = 1.25;

#[derive(Default)]
pub struct Governor {
    slow_for: f32,
    since_change: f32,
}

impl Governor {
    /// Feed this frame's slow-motion fraction (0 = real time). True when it's time to
    /// lower the resolution.
    pub fn update(&mut self, slow_motion: f32, dt: f32) -> bool {
        self.since_change += dt;
        self.slow_for = if slow_motion > SLOW { self.slow_for + dt } else { 0.0 };
        if self.slow_for >= SUSTAIN_S && self.since_change >= COOLDOWN_S {
            (self.slow_for, self.since_change) = (0.0, 0.0);
            return true;
        }
        false
    }

    /// Start judging afresh (a new scene or solver).
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Lower the current solver's resolution one step and reset the scene. Returns what
/// changed, or `None` if it's already as coarse as it goes (or has no resolution
/// setting).
pub fn lower_resolution(world: &mut World) -> Option<String> {
    if let Some((cells, min)) = params::int_param_range(world, "cells_y") {
        let lower = ((cells as f32 * GRID_STEP).round() as u32).max(min);
        if lower < cells {
            params::int_param(world, "cells_y", Some(lower));
            world.reset();
            return Some(format!("Lowered the grid to {lower} cells high to keep up"));
        }
        return None;
    }
    if let Some((spacing, max)) = params::float_param_range(world, "spacing") {
        let wider = (spacing * SPACING_STEP).min(max);
        if wider > spacing {
            params::float_param(world, "spacing", Some(wider));
            world.reset();
            return Some(format!("Spread the particles to {:.1} mm to keep up", 1000.0 * wider));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: f32 = 1.0 / 60.0;

    fn run(g: &mut Governor, slow: f32, seconds: f32) -> usize {
        (0..(seconds / FRAME) as usize).filter(|_| g.update(slow, FRAME)).count()
    }

    #[test]
    fn acts_only_on_sustained_slowness_with_a_cooldown() {
        let mut g = Governor::default();
        assert_eq!(run(&mut g, 0.05, 30.0), 0, "real time enough");
        let mut g = Governor { since_change: COOLDOWN_S, ..Governor::default() };
        assert_eq!(run(&mut g, 0.3, 2.0), 0, "not sustained yet");
        assert_eq!(run(&mut g, 0.3, 1.5), 1, "sustained");
        assert_eq!(run(&mut g, 0.3, 9.0), 0, "cooling down");
        assert_eq!(run(&mut g, 0.3, 2.0), 1, "judged again after the cooldown");
    }

    #[test]
    fn brief_slowdowns_dont_add_up() {
        let mut g = Governor { since_change: COOLDOWN_S, ..Governor::default() };
        for _ in 0..10 {
            assert_eq!(run(&mut g, 0.3, 2.0), 0);
            assert_eq!(run(&mut g, 0.0, 0.5), 0);
        }
    }

    #[test]
    fn lowers_a_grid_solver_and_stops_at_the_minimum() {
        let mut world = World::new(0, 0); // FLIP: cells_y 100, minimum 16
        let first = lower_resolution(&mut world).expect("should lower");
        assert!(first.contains("80"), "{first}");
        let mut steps = 1;
        while lower_resolution(&mut world).is_some() {
            steps += 1;
            assert!(steps < 20, "never reached the minimum");
        }
        assert_eq!(params::int_param(&mut world, "cells_y", None), Some(16));
    }
}
