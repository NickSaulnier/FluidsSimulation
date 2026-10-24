pub mod ballistic;

pub use ballistic::Ballistic;

use crate::rng::SplitMix64;
use crate::scene::Scene;
use glam::Vec2;

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
