//! Particle separation (Ten Minute Physics' `pushParticlesApart`).
//!
//! FLIP particles advected through a bilinearly interpolated MAC velocity cluster at the
//! sub-cell scale, which the grid cannot see; over seconds the fluid visibly loses volume.
//! A few relaxation passes pushing overlapping pairs apart keep the distribution even.

use crate::neighbors::CellBins;
use glam::Vec2;

/// Push every pair closer than `2 * radius` apart symmetrically. Gauss-Seidel order
/// (positions update in place), which converges faster but is inherently sequential.
pub(super) fn push_apart(bins: &mut CellBins, pos: &mut [Vec2], radius: f32, iterations: u32, size: Vec2) {
    let min_dist = 2.0 * radius;
    // Bins are built once per call; particles move less than a radius per pass, so
    // slightly stale bins only miss pairs that weren't overlapping anyway.
    bins.build(pos, 1.1 * min_dist, size);
    for _ in 0..iterations {
        for a in 0..pos.len() {
            let pa = pos[a];
            bins.for_each_candidate(pa, |b| {
                if b == a {
                    return;
                }
                let d = pos[b] - pos[a];
                let d2 = d.length_squared();
                if d2 >= min_dist * min_dist || d2 == 0.0 {
                    return;
                }
                let dist = d2.sqrt();
                let shift = d * (0.5 * (min_dist - dist) / dist);
                pos[a] -= shift;
                pos[b] += shift;
            });
        }
    }
}
