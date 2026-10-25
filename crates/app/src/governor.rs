//! Keeps a presentation in real time. If the simulation keeps falling behind (slow
//! motion), lower the solver's resolution a step rather than let it drift out of sync
//! with the music. Only in presentation mode: in the toy, slow motion is a fair trade
//! for detail, and the user can see and change the settings.
//!
//! A 3D step never falls behind in simulated time (it's queued on the GPU); what drops
//! is the frame rate, so in 3D the frames' own slowness is judged instead.

use crate::ui::params::{self, Params};

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
/// 3D frames slower than this (50 fps) count toward falling behind.
const SLOWEST_3D_FRAME_S: f32 = 1.0 / 50.0;

/// How far a 3D frame of `real_dt` seconds falls short, as a slow-motion fraction (0
/// at 50 fps and better, 0.5 at 25 fps).
pub fn frame_slowness(real_dt: f32) -> f32 {
    (1.0 - SLOWEST_3D_FRAME_S / real_dt.max(1e-6)).max(0.0)
}

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
pub fn lower_resolution(world: &mut (impl Params + ?Sized)) -> Option<String> {
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
    fn slow_3d_frames_count_as_falling_behind() {
        assert_eq!(frame_slowness(1.0 / 60.0), 0.0);
        assert!((frame_slowness(1.0 / 25.0) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn lowers_a_grid_solver_and_stops_at_the_minimum() {
        let mut world = sim::World::new(0, 0); // FLIP: cells_y 100, minimum 16
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
