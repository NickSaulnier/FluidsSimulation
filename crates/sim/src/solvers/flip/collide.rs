//! Particle collisions against FLIP's own solid cells.
//!
//! The walls particles see must be exactly the walls the pressure solve sees: the faces
//! of solid cells. If particles were kept out of some sliver of an open cell (say, by the
//! scene's smooth SDF), cells along every wall would sit empty, be classified as air
//! (p = 0, a free surface), and the fluid would drain sideways into them.

use crate::grid::Grid2;
use crate::scene::Scene;
use crate::solvers::PAR_CHUNK;
use glam::Vec2;
use rayon::prelude::*;

pub(super) fn collide(solid: &Grid2<bool>, h: f32, radius: f32, scene: &Scene, pos: &mut [Vec2], vel: &mut [Vec2]) {
    let (nx, ny) = (solid.nx() as isize, solid.ny() as isize);
    let is_solid = |i: isize, j: isize| i < 0 || j < 0 || i >= nx || j >= ny || solid[(i as usize, j as usize)];
    let r = radius;
    let grid_max = Vec2::new(nx as f32, ny as f32) * h;

    pos.par_iter_mut().zip(vel.par_iter_mut()).with_min_len(PAR_CHUNK).for_each(|(p, v)| {
        // Deep inside an obstacle (painted over the fluid, or a fast particle overshot):
        // let the SDF carry it back to the surface first.
        let d = scene.solid.distance(*p);
        if d < -0.5 * h {
            *p += scene.solid.normal(*p) * (r - d);
        }
        // Never leave the grid (the outer ring of cells is always solid).
        *p = p.clamp(Vec2::splat(h + r), grid_max - h - r);

        let mut i = (p.x / h) as isize;
        let mut j = (p.y / h) as isize;
        if is_solid(i, j) {
            // Step into the open 4-neighbour whose shared face is nearest, landing on
            // that face; the face clamp below then moves it `r` into the open cell.
            let local = *p / h - Vec2::new(i as f32, j as f32);
            let exits = [(-1, 0, local.x), (1, 0, 1.0 - local.x), (0, -1, local.y), (0, 1, 1.0 - local.y)];
            let nearest = exits.iter().filter(|e| !is_solid(i + e.0, j + e.1)).min_by(|a, b| a.2.total_cmp(&b.2));
            if let Some(&(di, dj, _)) = nearest {
                match (di, dj) {
                    (-1, _) => p.x = i as f32 * h,
                    (1, _) => p.x = (i + 1) as f32 * h,
                    (_, -1) => p.y = j as f32 * h,
                    _ => p.y = (j + 1) as f32 * h,
                }
                i += di;
                j += dj;
            }
        }

        // Clamp against the solid faces of the particle's cell, removing velocity into
        // them (free slip along the wall).
        let (x0, y0) = (i as f32 * h, j as f32 * h);
        if is_solid(i - 1, j) && p.x < x0 + r {
            p.x = x0 + r;
            v.x = v.x.max(0.0);
        }
        if is_solid(i + 1, j) && p.x > x0 + h - r {
            p.x = x0 + h - r;
            v.x = v.x.min(0.0);
        }
        if is_solid(i, j - 1) && p.y < y0 + r {
            p.y = y0 + r;
            v.y = v.y.max(0.0);
        }
        if is_solid(i, j + 1) && p.y > y0 + h - r {
            p.y = y0 + h - r;
            v.y = v.y.min(0.0);
        }

        // Where a curved wall cuts across an open cell's corner, the face clamps above
        // allow up to ~0.7 h of penetration. Cap it at one radius using the smooth SDF;
        // that only empties the deepest corner of a cell, never a whole cell.
        let d = scene.solid.distance(*p);
        if d < -r {
            let n = scene.solid.normal(*p);
            *p += n * (-r - d);
            let vn = v.dot(n);
            if vn < 0.0 {
                *v -= n * vn;
            }
        }
    });
}
