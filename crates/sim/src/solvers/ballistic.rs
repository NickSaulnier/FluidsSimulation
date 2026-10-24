//! Solver zero: non-interacting particles under gravity, bouncing off solids.
//!
//! No fluid physics at all. It exists to exercise the trait, stepper, renderer and
//! invariant checks before any fluid math does, and it stays useful as a fixture.

use crate::params::{FloatSpec, ParamVisitor};
use crate::scene::{Scene, StepInput};
use crate::solver::{Init, ParticleView, Solver, Stats};
use glam::Vec2;
use std::time::Instant;

pub struct Ballistic {
    pos: Vec<Vec2>,
    vel: Vec<Vec2>,
    radius: f32,
    /// Fraction of normal speed kept on bounce.
    restitution: f32,
    /// Fraction of tangential speed kept on bounce.
    friction: f32,
    emission: super::Emission,
    stats: Stats,
}

impl Default for Ballistic {
    fn default() -> Self {
        Self {
            pos: Vec::new(),
            vel: Vec::new(),
            radius: 0.0025,
            restitution: 0.6,
            friction: 0.98,
            emission: super::Emission::default(),
            stats: Stats::default(),
        }
    }
}

const RADIUS: FloatSpec = FloatSpec {
    key: "radius",
    label: "Particle radius (m)",
    range: 0.001..=0.01,
    log: true,
    restart: true,
    help: "Also sets the seeding spacing, so it changes the particle count.",
};
const RESTITUTION: FloatSpec = FloatSpec {
    key: "restitution",
    label: "Bounciness",
    range: 0.0..=1.0,
    log: false,
    restart: false,
    help: "Fraction of normal speed kept on impact.",
};
const FRICTION: FloatSpec = FloatSpec {
    key: "friction",
    label: "Slipperiness",
    range: 0.0..=1.0,
    log: false,
    restart: false,
    help: "Fraction of tangential speed kept on impact (1 = frictionless).",
};

impl Solver for Ballistic {
    fn name(&self) -> &'static str {
        "Ballistic (debug)"
    }

    fn reset(&mut self, scene: &Scene, init: Init<'_>) {
        match init {
            Init::FromScene => {
                super::seed_particles(scene, 2.0 * self.radius, 0.3, &mut self.pos);
                // A little random sideways kick so the block breaks up as it bounces.
                let mut rng = crate::rng::SplitMix64::new(0xBA11);
                self.vel = (0..self.pos.len()).map(|_| Vec2::new(rng.signed(), rng.signed()) * 0.3).collect();
            }
            Init::FromParticles(view) => {
                self.pos = view.pos.to_vec();
                self.vel = view.vel.to_vec();
            }
        }
        self.emission.reset();
        self.stats = Stats { particles: self.pos.len(), ..Stats::default() };
    }

    fn max_dt(&self, scene: &Scene) -> f32 {
        // Travel at most one mask cell per substep, so overshoot into a wall stays within
        // the depth the SDF can measure (see `scene::BORDER_CELLS`).
        let max_speed = self.vel.iter().map(|v| v.length_squared()).fold(0.0, f32::max).sqrt();
        scene.solid.cell_size() / max_speed.max(1e-3)
    }

    fn step(&mut self, dt: f32, scene: &Scene, input: &StepInput) {
        let t0 = Instant::now();
        let (emitted, drained) = self.emission.apply(scene, input, dt, 2.0 * self.radius, &mut self.pos, &mut self.vel);
        self.stats.emitted += emitted;
        self.stats.drained += drained;
        self.stats.particles = self.pos.len();
        super::apply_brushes(&input.brushes, dt, &self.pos, &mut self.vel);
        for (p, v) in self.pos.iter_mut().zip(self.vel.iter_mut()) {
            *v += scene.gravity * dt;
            *p += *v * dt;
        }
        let interior = scene.solid.interior();
        super::collide_with_solids(scene, interior, self.radius, self.restitution, self.friction, &mut self.pos, &mut self.vel);
        self.stats.phases = vec![("integrate+collide", t0.elapsed())];
    }

    fn visit_params(&mut self, v: &mut dyn ParamVisitor) {
        v.float(&RADIUS, &mut self.radius);
        v.float(&RESTITUTION, &mut self.restitution);
        v.float(&FRICTION, &mut self.friction);
    }

    fn particles(&self) -> Option<ParticleView<'_>> {
        Some(ParticleView { pos: &self.pos, vel: &self.vel, radius: self.radius })
    }

    fn stats(&self) -> &Stats {
        &self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets::dam_break;

    #[test]
    fn particles_stay_in_the_box() {
        let scene = dam_break();
        let mut s = Ballistic::default();
        s.reset(&scene, Init::FromScene);
        let n = s.particles().unwrap().pos.len();
        assert!(n > 1000);
        for _ in 0..1000 {
            let dt = s.max_dt(&scene).min(1.0 / 120.0);
            s.step(dt, &scene, &StepInput::default());
        }
        let view = s.particles().unwrap();
        assert_eq!(view.pos.len(), n);
        for (p, v) in view.pos.iter().zip(view.vel) {
            assert!(p.is_finite() && v.is_finite());
            assert!(scene.solid.distance(*p) > -view.radius, "particle {p} inside a wall");
        }
    }
}
