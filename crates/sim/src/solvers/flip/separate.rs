//! Particle separation (Ten Minute Physics' `pushParticlesApart`).
//!
//! FLIP particles advected through a bilinearly interpolated MAC velocity cluster at the
//! sub-cell scale, which the grid cannot see; over seconds the fluid visibly loses volume.
//! A few relaxation passes pushing overlapping pairs apart keep the distribution even.

use crate::neighbors::CellBins;
use crate::solvers::PAR_CHUNK;
use glam::Vec2;
use rayon::prelude::*;

/// Push every pair closer than `2 * radius` apart, half the overlap each.
///
/// Jacobi order: each pass computes every particle's total push from a read-only
/// snapshot of the positions (a parallel gather, each particle writing only its own
/// `shift`), then applies them all. Gauss-Seidel order (TMP's) converges a little faster
/// but is inherently sequential.
pub(super) fn push_apart(
    bins: &mut CellBins,
    pos: &mut [Vec2],
    shift: &mut Vec<Vec2>,
    radius: f32,
    iterations: u32,
    size: Vec2,
) {
    let min_dist = 2.0 * radius;
    // Bins are built once per call; particles move less than a radius per pass, so
    // slightly stale bins only miss pairs that weren't overlapping anyway.
    bins.build(pos, 1.1 * min_dist, size);
    shift.resize(pos.len(), Vec2::ZERO);
    for _ in 0..iterations {
        let (bins, snapshot) = (&*bins, &*pos);
        shift.par_iter_mut().with_min_len(PAR_CHUNK).enumerate().for_each(|(a, out)| {
            let pa = snapshot[a];
            let mut push = Vec2::ZERO;
            bins.for_each_candidate(pa, |b| {
                if b == a {
                    return;
                }
                let d = snapshot[b] - pa;
                let d2 = d.length_squared();
                if d2 >= min_dist * min_dist || d2 == 0.0 {
                    return;
                }
                let dist = d2.sqrt();
                push -= d * (0.5 * (min_dist - dist) / dist);
            });
            *out = push;
        });
        pos.par_iter_mut().zip(&*shift).with_min_len(PAR_CHUNK).for_each(|(p, s)| *p += *s);
    }
}
