//! Solver implementations, plus the toolkit they share. The toolkit is public so that
//! solvers in other crates (e.g. `sim-gpu`, which needs wgpu) can reuse it.

pub mod ballistic;
pub mod fire;
pub mod flip;
pub mod lbm;
mod multigrid;
pub mod pbf;
mod pressure;
pub mod stable;

pub use ballistic::Ballistic;
pub use flip::{Flip, FlipParams};
pub use lbm::{Lbm, LbmParams};
pub use pbf::{Pbf, PbfParams};
pub use stable::{StableFluids, StableParams};

use crate::grid::Grid2;
use crate::grid::mac::{CENTRE_OFFSET, Stencil};
use utils::rng::SplitMix64;
use crate::scene::{Emitter, ForceBrush, Rect, Scene, StepInput};
use crate::solver::ParticleView;
use glam::Vec2;
use rayon::prelude::*;

/// Minimum particles per rayon task in particle loops. Without it rayon splits a 20k
/// particle loop 32+ ways and the per-task overhead outweighs the work (measured: loops
/// got slower beyond 8 threads).
pub(crate) const PAR_CHUNK: usize = 1024;

/// Whether a drain opens the domain's border at `c`: drains reaching into the border
/// (outside the interior, where the walls are always solid) make it open there, so a
/// scene can let gas out through its edges, unseen. Painted walls stay solid.
pub fn opens_border(scene: &Scene, c: Vec2) -> bool {
    !scene.solid.interior().contains(c) && scene.drains.iter().any(|d| d.contains(c))
}

/// Resample the scene's solids onto a grid of cell size `h`: a cell is solid if its
/// centre is inside a scene solid. The outer ring is always solid so the domain stays
/// closed even where the scene's walls are thinner than a cell.
pub fn resample_solids(scene: &Scene, h: f32, out: &mut Grid2<bool>) {
    let (nx, ny) = (out.nx(), out.ny());
    for j in 0..ny {
        for i in 0..nx {
            let centre = (Vec2::new(i as f32, j as f32) + 0.5) * h;
            let border = i == 0 || j == 0 || i == nx - 1 || j == ny - 1;
            out[(i, j)] = border || scene.solid.distance(centre) < 0.0;
        }
    }
}

/// Seed particles on a jittered lattice inside the scene's fluid blocks, skipping solids.
pub fn seed_particles(scene: &Scene, spacing: f32, jitter: f32, out: &mut Vec<Vec2>) {
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

/// Adopt another solver's particles at this solver's own `spacing`.
///
/// Solvers run at different resolutions (FLIP packs 4 particles per cell, PBF is much
/// coarser), and a particle set at the wrong density either explodes or collapses.
/// Finer input is thinned to one particle per `spacing`-sized cell (averaging position
/// and velocity); coarser input is split into a small square cluster per particle.
pub fn resample(view: ParticleView<'_>, spacing: f32, size: Vec2, pos: &mut Vec<Vec2>, vel: &mut Vec<Vec2>) {
    pos.clear();
    vel.clear();
    let source = 2.0 * view.radius;
    if source < 0.75 * spacing {
        let nx = (size.x / spacing).ceil() as usize;
        let ny = (size.y / spacing).ceil() as usize;
        let mut cells = vec![(Vec2::ZERO, Vec2::ZERO, 0u32); nx * ny];
        for (&p, &v) in view.pos.iter().zip(view.vel) {
            let i = ((p.x / spacing) as usize).min(nx - 1);
            let j = ((p.y / spacing) as usize).min(ny - 1);
            let cell = &mut cells[j * nx + i];
            *cell = (cell.0 + p, cell.1 + v, cell.2 + 1);
        }
        for (p, v, n) in cells.into_iter().filter(|c| c.2 > 0) {
            pos.push(p / n as f32);
            vel.push(v / n as f32);
        }
    } else if source > 1.5 * spacing {
        let m = (source / spacing).round() as usize;
        for (&p, &v) in view.pos.iter().zip(view.vel) {
            for sj in 0..m {
                for si in 0..m {
                    let offset = (Vec2::new(si as f32, sj as f32) + 0.5) / m as f32 - 0.5;
                    pos.push(p + offset * source);
                    vel.push(v);
                }
            }
        }
    } else {
        pos.extend_from_slice(view.pos);
        vel.extend_from_slice(view.vel);
    }
}

/// Emitter and drain bookkeeping shared by the particle solvers.
pub struct Emission {
    /// Fractional particles owed per emitter (scene emitters first, then input's).
    owed: Vec<f32>,
    rng: SplitMix64,
}

impl Default for Emission {
    fn default() -> Self {
        Self { owed: Vec::new(), rng: SplitMix64::new(0xE417) }
    }
}

impl Emission {
    pub fn reset(&mut self) {
        self.owed.clear();
    }

    /// Remove particles inside drains, then spawn from every emitter at this solver's
    /// `spacing` (a continuous stream of width x speed per second). Returns
    /// `(emitted, drained)`. Call before any per-particle scratch is sized.
    pub fn apply(
        &mut self,
        scene: &Scene,
        input: &StepInput,
        dt: f32,
        spacing: f32,
        pos: &mut Vec<Vec2>,
        vel: &mut Vec<Vec2>,
    ) -> (usize, usize) {
        let before = pos.len();
        if !scene.drains.is_empty() {
            let mut i = 0;
            while i < pos.len() {
                if scene.drains.iter().any(|d| d.contains(pos[i])) {
                    pos.swap_remove(i);
                    vel.swap_remove(i);
                } else {
                    i += 1;
                }
            }
        }
        let drained = before - pos.len();

        let emitters: Vec<_> = scene.emitters.iter().chain(&input.emitters).copied().collect();
        self.owed.resize(emitters.len(), 0.0);
        let mut emitted = 0;
        for (owed, e) in self.owed.iter_mut().zip(&emitters) {
            *owed += e.width * e.speed / (spacing * spacing) * dt;
            let wanted = owed.floor() as usize;
            *owed -= wanted as f32;
            let room = scene.max_particles.saturating_sub(pos.len());
            let dir = e.dir.normalize_or(Vec2::NEG_Y);
            let across = dir.perp();
            for _ in 0..wanted.min(room) {
                // Spread across the nozzle and along this substep's travel, so the stream
                // has no visible banding.
                let p = e.pos + across * (self.rng.next_f32() - 0.5) * e.width + dir * self.rng.next_f32() * e.speed * dt;
                if scene.solid.distance(p) > 0.5 * spacing {
                    pos.push(p);
                    vel.push(dir * e.speed);
                    emitted += 1;
                }
            }
        }
        (emitted, drained)
    }
}

/// Blend particle velocities toward each brush's target velocity, with linear falloff.
pub fn apply_brushes(brushes: &[ForceBrush], dt: f32, pos: &[Vec2], vel: &mut [Vec2]) {
    for b in brushes {
        let r2 = b.radius * b.radius;
        pos.par_iter().zip(vel.par_iter_mut()).with_min_len(PAR_CHUNK).for_each(|(p, v)| {
            let d2 = p.distance_squared(b.pos);
            if d2 < r2 {
                let falloff = 1.0 - d2.sqrt() / b.radius;
                let blend = (b.strength * falloff * dt).min(1.0);
                *v += (b.target(*p) - *v) * blend;
            }
        });
    }
}

/// Push particles of the given radius out of scene solids along the SDF normal.
/// On contact the normal velocity is reflected scaled by `restitution` (0 = stick to
/// the surface) and the tangential velocity scaled by `friction` (1 = free slip).
/// Particle centres are also clamped to `interior` shrunk by `radius`.
pub fn collide_with_solids(
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

/// Wide nozzles release dye in stripes this far apart (wind-tunnel streaklines);
/// narrower ones release solid dye.
const STRIPE_PITCH: f32 = 0.08;
const STRIPED_WIDTH: f32 = 0.25;

/// For grid solvers that model emitters as boundary cells: if the cell centred at `c`
/// (cell size `h`) is in the emitter's nozzle, its offset across the stream. The
/// nozzle is a slab one cell either side of `pos`, `width` across.
pub fn nozzle_offset(e: &Emitter, h: f32, c: Vec2) -> Option<f32> {
    let dir = e.dir.normalize_or(Vec2::NEG_Y);
    let d = c - e.pos;
    let across = d.dot(dir.perp());
    (across.abs() <= 0.5 * e.width.max(h) && d.dot(dir).abs() <= h).then_some(across)
}

/// Dye concentration a nozzle releases at offset `across` from its centre line.
pub fn nozzle_dye(e: &Emitter, across: f32) -> f32 {
    let stripe = ((across / (0.5 * STRIPE_PITCH)).floor() as i32).rem_euclid(2) == 0;
    if e.width < STRIPED_WIDTH || stripe { 1.0 } else { 0.0 }
}

/// MacCormack advection (Selle et al. 2008) of a cell-centred scalar `q` through
/// `velocity` (m/s at a point): a semi-Lagrangian step forward, one back, and half the
/// round-trip error added back, clamped to the values the forward step interpolated
/// from so it can't overshoot. Much less diffusive than plain semi-Lagrangian, which
/// smears a streakline out within a few dozen cells. `frozen` cells (solids, inlets)
/// keep their value and still act as sources for their neighbours.
pub fn maccormack(
    q: &mut Grid2<f32>,
    forward: &mut Grid2<f32>,
    back: &mut Grid2<f32>,
    h: f32,
    dt: f32,
    velocity: impl Fn(Vec2) -> Vec2 + Sync,
    frozen: impl Fn(usize, usize) -> bool + Sync,
) {
    let departure = |i: usize, j: usize, dt: f32| {
        let p = (Vec2::new(i as f32, j as f32) + CENTRE_OFFSET) * h;
        let mid = p - 0.5 * dt * velocity(p);
        p - dt * velocity(mid)
    };
    let semi_lagrangian = |src: &Grid2<f32>, out: &mut Grid2<f32>, dt: f32| {
        out.par_rows_mut().enumerate().for_each(|(j, row)| {
            for (i, o) in row.iter_mut().enumerate() {
                *o = if frozen(i, j) {
                    src[(i, j)]
                } else {
                    Stencil::on(src, h, CENTRE_OFFSET, departure(i, j, dt)).sample(src)
                };
            }
        });
    };
    semi_lagrangian(q, forward, dt);
    semi_lagrangian(forward, back, -dt);
    let (q_ro, forward) = (&*q, &*forward);
    back.par_rows_mut().enumerate().for_each(|(j, row)| {
        for (i, o) in row.iter_mut().enumerate() {
            if frozen(i, j) {
                *o = q_ro[(i, j)];
                continue;
            }
            let corrected = forward[(i, j)] + 0.5 * (q_ro[(i, j)] - *o);
            let s = Stencil::on(q_ro, h, CENTRE_OFFSET, departure(i, j, dt));
            let (lo, hi) =
                s.corners().into_iter().fold((f32::MAX, f32::MIN), |(lo, hi), c| (lo.min(q_ro[c]), hi.max(q_ro[c])));
            *o = corrected.clamp(lo, hi);
        }
    });
    std::mem::swap(q, back);
}
