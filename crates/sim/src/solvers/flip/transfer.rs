//! Particle <-> grid transfers and cell classification.
//!
//! Free functions over the pieces they touch, rather than `&mut self` methods, so the
//! borrow checker can see that e.g. the grid is mutated while the particles are only read.

use crate::grid::Grid2;
use crate::grid::mac::{CENTRE_OFFSET, CellType, MacGrid, Stencil, U_OFFSET, V_OFFSET};
use glam::Vec2;
use crate::solvers::PAR_CHUNK;
use rayon::prelude::*;

/// Solid cells come from the (cached) resampled scene mask; every other cell containing
/// a particle is fluid, the rest air.
pub(super) fn mark_cells(grid: &mut MacGrid, solid: &Grid2<bool>, pos: &[Vec2]) {
    for (c, &s) in grid.cell.as_mut_slice().iter_mut().zip(solid.as_slice()) {
        *c = if s { CellType::Solid } else { CellType::Air };
    }
    let (nx, ny, h) = (grid.nx, grid.ny, grid.h);
    for p in pos {
        let i = ((p.x / h).max(0.0) as usize).min(nx - 1);
        let j = ((p.y / h).max(0.0) as usize).min(ny - 1);
        let c = &mut grid.cell[(i, j)];
        if *c == CellType::Air {
            *c = CellType::Fluid;
        }
    }
}

/// Velocity and weight sums on the faces for one chunk of particles.
struct FaceSums {
    u: Grid2<f32>,
    v: Grid2<f32>,
    wu: Grid2<f32>,
    wv: Grid2<f32>,
}

impl FaceSums {
    fn zeros(nx: usize, ny: usize) -> Self {
        Self { u: Grid2::new(nx + 1, ny), v: Grid2::new(nx, ny + 1), wu: Grid2::new(nx + 1, ny), wv: Grid2::new(nx, ny + 1) }
    }

    fn matches(&self, nx: usize, ny: usize) -> bool {
        self.v.nx() == nx && self.u.ny() == ny
    }

    fn clear(&mut self) {
        self.u.fill(0.0);
        self.v.fill(0.0);
        self.wu.fill(0.0);
        self.wv.fill(0.0);
    }

    fn splat(&mut self, h: f32, p: Vec2, vel: Vec2) {
        let su = Stencil::on(&self.u, h, U_OFFSET, p);
        let sv = Stencil::on(&self.v, h, V_OFFSET, p);
        for (c, w) in su.corners().into_iter().zip(su.w) {
            self.u[c] += w * vel.x;
            self.wu[c] += w;
        }
        for (c, w) in sv.corners().into_iter().zip(sv.w) {
            self.v[c] += w * vel.y;
            self.wv[c] += w;
        }
    }
}

/// Per-chunk partial sums for P2G, kept between substeps to avoid reallocating.
#[derive(Default)]
pub(super) struct P2gScratch {
    parts: Vec<FaceSums>,
}

/// Particles per P2G chunk (at most one chunk per thread).
const P2G_CHUNK: usize = 4096;

/// Bilinear splat of particle velocities onto the faces, normalised by total weight.
///
/// A scatter: many particles write the same face. Rather than locks or atomics, the
/// particles are split into a few chunks, each accumulating into its own partial grids,
/// and a row-parallel pass then sums the partials into the result. Chunking is fixed for
/// a given thread count, so results are deterministic for that count; they match the
/// serial version to rounding.
pub(super) fn particles_to_grid(grid: &mut MacGrid, scratch: &mut P2gScratch, pos: &[Vec2], vel: &[Vec2]) {
    let (nx, ny, h) = (grid.nx, grid.ny, grid.h);
    let n = pos.len();
    let jobs = n.div_ceil(P2G_CHUNK).clamp(1, rayon::current_num_threads());
    let chunk = n.div_ceil(jobs).max(1);
    if scratch.parts.first().is_some_and(|p| !p.matches(nx, ny)) {
        scratch.parts.clear();
    }
    scratch.parts.resize_with(jobs.max(scratch.parts.len()), || FaceSums::zeros(nx, ny));

    let used = n.div_ceil(chunk);
    scratch.parts[..used].par_iter_mut().zip(pos.par_chunks(chunk)).zip(vel.par_chunks(chunk)).for_each(
        |((acc, ps), vs)| {
            acc.clear();
            for (&p, &v) in ps.iter().zip(vs) {
                acc.splat(h, p, v);
            }
        },
    );

    let parts = &scratch.parts[..used];
    let combine = |field: fn(&FaceSums) -> (&Grid2<f32>, &Grid2<f32>), out: &mut Grid2<f32>| {
        out.par_rows_mut().enumerate().for_each(|(j, row)| {
            for (i, o) in row.iter_mut().enumerate() {
                let (mut sum, mut weight) = (0.0, 0.0);
                for part in parts {
                    let (f, w) = field(part);
                    sum += f[(i, j)];
                    weight += w[(i, j)];
                }
                *o = if weight > 0.0 { sum / weight } else { 0.0 };
            }
        });
    };
    combine(|p| (&p.u, &p.wu), &mut grid.u);
    combine(|p| (&p.v, &p.wv), &mut grid.v);
}

/// Serial reference for tests.
#[cfg(test)]
pub(super) fn particles_to_grid_serial(grid: &mut MacGrid, pos: &[Vec2], vel: &[Vec2]) {
    let mut sums = FaceSums::zeros(grid.nx, grid.ny);
    for (&p, &v) in pos.iter().zip(vel) {
        sums.splat(grid.h, p, v);
    }
    for (out, f, w) in [(&mut grid.u, &sums.u, &sums.wu), (&mut grid.v, &sums.v, &sums.wv)] {
        for ((o, &s), &w) in out.as_mut_slice().iter_mut().zip(f.as_slice()).zip(w.as_slice()) {
            *o = if w > 0.0 { s / w } else { 0.0 };
        }
    }
}

/// PIC/FLIP blend back onto the particles. `u_prev`/`v_prev` are the grid velocities
/// right after P2G; FLIP adds only the change forces and projection made since.
///
/// Also writes the plain interpolated (PIC) grid velocity to `advect`: particles should
/// move through the divergence-free grid field, not with their noisier FLIP velocity.
pub(super) fn grid_to_particles(
    grid: &MacGrid,
    u_prev: &Grid2<f32>,
    v_prev: &Grid2<f32>,
    pos: &[Vec2],
    vel: &mut [Vec2],
    advect: &mut [Vec2],
    flip_ratio: f32,
) {
    // A gather: each particle reads the grid and writes only itself.
    pos.par_iter().zip(vel.par_iter_mut()).zip(advect.par_iter_mut()).with_min_len(PAR_CHUNK).for_each(|((&p, v), a)| {
        let (pic_x, x) = blend_component(grid, &grid.u, u_prev, U_OFFSET, (1, 0), p, v.x, flip_ratio);
        let (pic_y, y) = blend_component(grid, &grid.v, v_prev, V_OFFSET, (0, 1), p, v.y, flip_ratio);
        *v = Vec2::new(x, y);
        *a = Vec2::new(pic_x, pic_y);
    });
}

#[allow(clippy::too_many_arguments)]
fn blend_component(
    grid: &MacGrid,
    field: &Grid2<f32>,
    prev: &Grid2<f32>,
    offset: Vec2,
    normal: (isize, isize),
    p: Vec2,
    old: f32,
    flip_ratio: f32,
) -> (f32, f32) {
    let s = Stencil::on(field, grid.h, offset, p);
    let (mut wsum, mut pic, mut delta) = (0.0, 0.0, 0.0);
    for (c, w) in s.corners().into_iter().zip(s.w) {
        // A face carries meaningful velocity only if a cell on either side of it is not air.
        let (i, j) = (c.0 as isize, c.1 as isize);
        if grid.cell_type(i, j) != CellType::Air || grid.cell_type(i - normal.0, j - normal.1) != CellType::Air {
            wsum += w;
            pic += w * field[c];
            delta += w * (field[c] - prev[c]);
        }
    }
    if wsum <= 0.0 {
        return (old, old);
    }
    let pic = pic / wsum;
    let flip = old + delta / wsum;
    (pic, pic + (flip - pic) * flip_ratio)
}

/// Body forces act on the grid, before projection, so the projection can cancel what the
/// fluid can't actually do (e.g. fall through the floor).
pub(super) fn apply_gravity(grid: &mut MacGrid, gravity: Vec2, dt: f32) {
    grid.u.as_mut_slice().iter_mut().for_each(|u| *u += gravity.x * dt);
    grid.v.as_mut_slice().iter_mut().for_each(|v| *v += gravity.y * dt);
}

/// Particles per cell, bilinearly splatted to cell centres (for drift compensation).
pub(super) fn particle_density(density: &mut Grid2<f32>, h: f32, pos: &[Vec2]) {
    density.fill(0.0);
    for &p in pos {
        let s = Stencil::on(density, h, CENTRE_OFFSET, p);
        for (c, w) in s.corners().into_iter().zip(s.w) {
            density[c] += w;
        }
    }
}
