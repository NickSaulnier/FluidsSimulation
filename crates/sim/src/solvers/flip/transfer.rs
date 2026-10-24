//! Particle <-> grid transfers and cell classification.
//!
//! Free functions over the pieces they touch, rather than `&mut self` methods, so the
//! borrow checker can see that e.g. the grid is mutated while the particles are only read.

use crate::grid::Grid2;
use crate::grid::mac::{CENTRE_OFFSET, CellType, MacGrid, Stencil, U_OFFSET, V_OFFSET};
use glam::Vec2;

/// Accumulated splat weights per face, reused between steps.
pub(super) struct Weights {
    pub u: Grid2<f32>,
    pub v: Grid2<f32>,
}

impl Weights {
    pub fn for_grid(grid: &MacGrid) -> Self {
        Self { u: Grid2::new(grid.nx + 1, grid.ny), v: Grid2::new(grid.nx, grid.ny + 1) }
    }
}

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

/// Bilinear splat of particle velocities onto the faces, normalised by total weight.
pub(super) fn particles_to_grid(grid: &mut MacGrid, weights: &mut Weights, pos: &[Vec2], vel: &[Vec2]) {
    grid.u.fill(0.0);
    grid.v.fill(0.0);
    weights.u.fill(0.0);
    weights.v.fill(0.0);
    let h = grid.h;
    for (&p, &v) in pos.iter().zip(vel) {
        // Stencils first: arguments are evaluated left to right, so building them inline
        // would read `weights.u` while it is already borrowed mutably.
        let (su, sv) = (Stencil::on(&weights.u, h, U_OFFSET, p), Stencil::on(&weights.v, h, V_OFFSET, p));
        splat(&mut grid.u, &mut weights.u, su, v.x);
        splat(&mut grid.v, &mut weights.v, sv, v.y);
    }
    normalize(&mut grid.u, &weights.u);
    normalize(&mut grid.v, &weights.v);
}

fn splat(field: &mut Grid2<f32>, weight: &mut Grid2<f32>, s: Stencil, value: f32) {
    for (c, w) in s.corners().into_iter().zip(s.w) {
        field[c] += w * value;
        weight[c] += w;
    }
}

fn normalize(field: &mut Grid2<f32>, weight: &Grid2<f32>) {
    for (f, &w) in field.as_mut_slice().iter_mut().zip(weight.as_slice()) {
        if w > 0.0 {
            *f /= w;
        }
    }
}

/// Static solids: zero the velocity on every face touching a solid cell.
pub(super) fn enforce_solid_boundaries(grid: &mut MacGrid) {
    let (nx, ny) = (grid.nx as isize, grid.ny as isize);
    for j in 0..ny {
        for i in 0..=nx {
            if grid.cell_type(i - 1, j) == CellType::Solid || grid.cell_type(i, j) == CellType::Solid {
                grid.u[(i as usize, j as usize)] = 0.0;
            }
        }
    }
    for j in 0..=ny {
        for i in 0..nx {
            if grid.cell_type(i, j - 1) == CellType::Solid || grid.cell_type(i, j) == CellType::Solid {
                grid.v[(i as usize, j as usize)] = 0.0;
            }
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
    for ((&p, v), a) in pos.iter().zip(vel.iter_mut()).zip(advect.iter_mut()) {
        let (pic_x, x) = blend_component(grid, &grid.u, u_prev, U_OFFSET, (1, 0), p, v.x, flip_ratio);
        let (pic_y, y) = blend_component(grid, &grid.v, v_prev, V_OFFSET, (0, 1), p, v.y, flip_ratio);
        *v = Vec2::new(x, y);
        *a = Vec2::new(pic_x, pic_y);
    }
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
