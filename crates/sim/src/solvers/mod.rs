pub mod ballistic;
pub mod flip;

pub use ballistic::Ballistic;
pub use flip::{Flip, FlipParams};

use crate::rng::SplitMix64;
use crate::scene::{ForceBrush, Rect, Scene};
use glam::Vec2;
use rayon::prelude::*;

/// Minimum particles per rayon task in particle loops. Without it rayon splits a 20k
/// particle loop 32+ ways and the per-task overhead outweighs the work (measured: loops
/// got slower beyond 8 threads).
pub(crate) const PAR_CHUNK: usize = 1024;

/// Seed particles on a jittered lattice inside the scene's fluid blocks, skipping solids.
pub(crate) fn seed_particles(scene: &Scene, spacing: f32, jitter: f32, out: &mut Vec<Vec2>) {
    let mut rng = SplitMix64::new(0x5EED);
    out.clear();
    for block in &scene.fluid_blocks {
        let n = (block.size() / spacing).floor();
        for j in 0..n.y as usize {
            for i in 0..n.x as usize {
                let lattice = block.min + (Vec2::new(i as f32, j as f32) + 0.5) * spacing;
                let p = lattice + Vec2::new(rng.signed(), rng.signed()) * jitter * spacing;
                if scene.solid.distance(p) > 0.5 * spacing {
                    out.push(p);
                }
            }
        }
    }
}

/// Blend particle velocities toward each brush's velocity, with linear falloff.
pub(crate) fn apply_brushes(brushes: &[ForceBrush], dt: f32, pos: &[Vec2], vel: &mut [Vec2]) {
    for b in brushes {
        let r2 = b.radius * b.radius;
        pos.par_iter().zip(vel.par_iter_mut()).with_min_len(PAR_CHUNK).for_each(|(p, v)| {
            let d2 = p.distance_squared(b.pos);
            if d2 < r2 {
                let falloff = 1.0 - d2.sqrt() / b.radius;
                let blend = (b.strength * falloff * dt).min(1.0);
                *v += (b.velocity - *v) * blend;
            }
        });
    }
}

/// Push particles of the given radius out of scene solids along the SDF normal.
/// On contact the normal velocity is reflected scaled by `restitution` (0 = stick to
/// the surface) and the tangential velocity scaled by `friction` (1 = free slip).
/// Particle centres are also clamped to `interior` shrunk by `radius`.
pub(crate) fn collide_with_solids(
    scene: &Scene,
    interior: Rect,
    radius: f32,
    restitution: f32,
    friction: f32,
    pos: &mut [Vec2],
    vel: &mut [Vec2],
) {
    let (lo, hi) = (interior.min + radius, interior.max - radius);
    for (p, v) in pos.iter_mut().zip(vel.iter_mut()) {
        // Painted obstacles: the SDF. It only sees penetration up to about the obstacle's
        // half-thickness, which the solvers' CFL limits keep us within.
        let d = scene.solid.distance(*p);
        if d < radius {
            let n = scene.solid.normal(*p);
            *p += n * (radius - d);
            let vn = v.dot(n);
            if vn < 0.0 {
                let tangential = *v - n * vn;
                *v = tangential * friction - n * vn * restitution;
            }
        }
        // Domain walls: an exact analytic clamp, robust at any speed.
        for axis in 0..2 {
            if p[axis] < lo[axis] {
                p[axis] = lo[axis];
                v[axis] = v[axis].max(-v[axis] * restitution);
            } else if p[axis] > hi[axis] {
                p[axis] = hi[axis];
                v[axis] = v[axis].min(-v[axis] * restitution);
            }
        }
    }
}
