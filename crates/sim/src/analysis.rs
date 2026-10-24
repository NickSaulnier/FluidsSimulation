//! Measurements on a running simulation, for validation against experiments.

use crate::presets::MARTIN_MOYCE_WIDTH;
use crate::solver::ParticleView;
use crate::world::World;

/// Martin & Moyce (1952): a column of width `a` and height `2a` (n² = 2) collapsing along
/// a dry floor. Surge-front position Z = x/a against dimensionless time T = t √(2g/a),
/// ensemble means. The three points corroborated against published validations are
/// marked `true`; the rest are the table as commonly reproduced in SPH/MPS papers.
pub const MARTIN_MOYCE_N2_2: &[(f32, f32, bool)] = &[
    (0.41, 1.11, false),
    (0.84, 1.22, false),
    (1.19, 1.44, false),
    (1.43, 1.67, false),
    (1.63, 1.89, false),
    (1.83, 2.11, true),
    (1.98, 2.33, false),
    (2.20, 2.56, false),
    (2.32, 2.78, false),
    (2.51, 3.00, true),
    (2.65, 3.22, false),
    (2.83, 3.44, false),
    (2.98, 3.67, false),
    (3.11, 3.89, true),
];

/// Height of the floor band in which the surge front is measured, metres.
pub const FRONT_BAND: f32 = 0.02;

/// The current (T, Z) of a world running the Martin & Moyce preset, measured from the
/// actual faces of the left wall and floor.
pub fn martin_moyce_point(world: &World) -> Option<(f32, f32)> {
    let interior = world.scene.solid.interior();
    let a = MARTIN_MOYCE_WIDTH;
    let x = surge_front(world.solver.particles()?, interior.min.y, FRONT_BAND)?;
    let t = world.time as f32;
    Some((t * (2.0 * world.scene.gravity.length() / a).sqrt(), (x - interior.min.x) / a))
}

/// Fraction of floor-band particles allowed beyond the reported front, so a few stray
/// spray particles don't define it.
const FRONT_QUANTILE: f32 = 0.999;

/// The leading edge (largest x) of the fluid along the floor: the 99.9th-percentile x
/// of particles within `band` of the floor at `floor_y`. `None` if the band is empty.
pub fn surge_front(particles: ParticleView<'_>, floor_y: f32, band: f32) -> Option<f32> {
    let mut xs: Vec<f32> = particles.pos.iter().filter(|p| p.y < floor_y + band).map(|p| p.x).collect();
    if xs.is_empty() {
        return None;
    }
    xs.sort_by(f32::total_cmp);
    Some(xs[((xs.len() - 1) as f32 * FRONT_QUANTILE) as usize])
}

/// First time a sampled curve `(t, value)` reaches `target`, linearly interpolated
/// between samples. `None` if it never does.
pub fn crossing_time(samples: &[(f32, f32)], target: f32) -> Option<f32> {
    samples.windows(2).find_map(|w| {
        let ((t0, v0), (t1, v1)) = (w[0], w[1]);
        (v0 < target && v1 >= target).then(|| t0 + (target - v0) / (v1 - v0) * (t1 - t0))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec2;

    #[test]
    fn front_ignores_spray_and_particles_above_the_band() {
        let mut pos: Vec<Vec2> = (0..2000).map(|i| Vec2::new(i as f32 * 0.0005, 0.05)).collect();
        pos.push(Vec2::new(1.9, 0.05)); // one stray droplet far ahead on the floor
        pos.push(Vec2::new(1.95, 0.5)); // and one up in the air
        let vel = vec![Vec2::ZERO; pos.len()];
        let front = surge_front(ParticleView { pos: &pos, vel: &vel, radius: 0.001 }, 0.03, 0.05).unwrap();
        assert!((front - 1.0).abs() < 0.01, "front {front}");
    }

    #[test]
    fn crossing_time_interpolates() {
        let s = [(0.0, 1.0), (1.0, 2.0), (2.0, 4.0)];
        assert_eq!(crossing_time(&s, 3.0), Some(1.5));
        assert_eq!(crossing_time(&s, 5.0), None);
    }
}
