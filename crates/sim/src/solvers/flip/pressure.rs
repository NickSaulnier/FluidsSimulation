//! Pressure projection: Gauss-Seidel with successive over-relaxation, acting directly on
//! face velocities (Ten Minute Physics style). Air cells have p = 0 (free surface): their
//! faces are adjusted like any other, which is exactly the Dirichlet condition. Solid
//! faces are never touched (Neumann).
//!
//! Pressure is constrained to be non-negative (projected Gauss-Seidel): water can push on
//! a wall but not pull on it. Without this, fluid that tries to leave a wall is held by
//! suction, and splashes end up hanging from the ceiling.
//!
//! Warm-started: the pressure from the previous substep is applied first and the
//! iterations only solve for the change. Gauss-Seidel is slowest on exactly the smooth,
//! deep modes that hydrostatic pressure consists of; without warm starting, a 50-iteration
//! budget can't rebuild it from zero each substep and deep fluid slowly compacts.

use crate::grid::Grid2;
use crate::grid::mac::{CellType, MacGrid};

/// Volume-drift correction: a cell whose particle density exceeds `rest` gets extra
/// outflow of `gain * (density / rest - 1)` m/s. Choosing `gain = k * h / dt` removes a
/// fraction `k` of the excess per substep, independent of resolution and timestep.
#[derive(Clone, Copy)]
pub(super) struct Drift {
    pub gain: f32,
    pub rest: f32,
}

/// `pressure` persists between calls. It is stored per unit time (the velocity change it
/// applies is `pressure * dt`) so warm starting stays correct when dt varies.
pub(super) fn project(
    grid: &mut MacGrid,
    pressure: &mut Grid2<f32>,
    density: &Grid2<f32>,
    dt: f32,
    iterations: u32,
    over_relax: f32,
    drift: Option<Drift>,
) {
    // Warm start: apply last substep's pressure as a gradient on the faces.
    for j in 0..grid.ny {
        for i in 0..grid.nx {
            if grid.cell[(i, j)] != CellType::Fluid {
                pressure[(i, j)] = 0.0;
                continue;
            }
            let p = pressure[(i, j)] * dt;
            apply_cell_pressure(grid, i, j, p);
        }
    }

    for _ in 0..iterations {
        for j in 0..grid.ny {
            for i in 0..grid.nx {
                if grid.cell[(i, j)] != CellType::Fluid {
                    continue;
                }
                let s = open_sides(grid, i, j).iter().sum::<f32>();
                if s == 0.0 {
                    continue;
                }
                let mut div = grid.net_outflow(i, j);
                if let Some(d) = drift {
                    let compression = density[(i, j)] / d.rest - 1.0;
                    if compression > 0.0 {
                        div -= d.gain * compression;
                    }
                }
                // Accumulated pressure (as a velocity change) must stay >= 0.
                let total = pressure[(i, j)] * dt;
                let p = (total - div / s * over_relax).max(0.0) - total;
                apply_cell_pressure(grid, i, j, p);
                pressure[(i, j)] += p / dt;
            }
        }
    }
}

/// 1.0 for each neighbour (left, right, below, above) that is not solid.
#[inline]
fn open_sides(grid: &MacGrid, i: usize, j: usize) -> [f32; 4] {
    let open = |c: CellType| if c == CellType::Solid { 0.0 } else { 1.0 };
    let (i, j) = (i as isize, j as isize);
    [
        open(grid.cell_type(i - 1, j)),
        open(grid.cell_type(i + 1, j)),
        open(grid.cell_type(i, j - 1)),
        open(grid.cell_type(i, j + 1)),
    ]
}

/// Push the cell's non-solid faces outward by `p` (inward for negative `p`).
#[inline]
fn apply_cell_pressure(grid: &mut MacGrid, i: usize, j: usize, p: f32) {
    let [sx0, sx1, sy0, sy1] = open_sides(grid, i, j);
    grid.u[(i, j)] -= sx0 * p;
    grid.u[(i + 1, j)] += sx1 * p;
    grid.v[(i, j)] -= sy0 * p;
    grid.v[(i, j + 1)] += sy1 * p;
}

/// Largest constraint violation over fluid cells, in 1/s: |div u| where pressure is
/// active, and only compression where pressure is zero (fluid is free to separate there).
pub(super) fn max_residual(grid: &MacGrid, pressure: &Grid2<f32>) -> f32 {
    let mut max = 0.0f32;
    for j in 0..grid.ny {
        for i in 0..grid.nx {
            if grid.cell[(i, j)] == CellType::Fluid {
                let div = grid.net_outflow(i, j);
                let r = if pressure[(i, j)] > 0.0 { div.abs() } else { (-div).max(0.0) };
                max = max.max(r);
            }
        }
    }
    max / grid.h
}
