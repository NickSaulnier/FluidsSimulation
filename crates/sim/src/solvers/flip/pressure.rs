//! Pressure projection: red-black Gauss-Seidel with successive over-relaxation, in
//! parallel. Air cells have p = 0 (free surface, Dirichlet); faces touching solids are
//! never changed (Neumann).
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
use rayon::prelude::*;

/// Volume-drift correction: a cell whose particle density exceeds `rest` gets extra
/// outflow of `gain * (density / rest - 1)` m/s. Choosing `gain = k * h / dt` removes a
/// fraction `k` of the excess per substep, independent of resolution and timestep.
#[derive(Clone, Copy)]
pub(super) struct Drift {
    pub gain: f32,
    pub rest: f32,
}

/// Scratch grids for the solve, kept between substeps.
pub(super) struct PressureSolver {
    /// Current iterate, as a velocity change (pressure * dt / (rho h) in our units).
    p: Grid2<f32>,
    next: Grid2<f32>,
    /// Minus the (drift-corrected) divergence before projection, m/s.
    rhs: Grid2<f32>,
    /// Number of non-solid neighbours; 0 for cells that aren't fluid (never updated).
    diag: Grid2<f32>,
}

impl PressureSolver {
    pub fn new(nx: usize, ny: usize) -> Self {
        Self { p: Grid2::new(nx, ny), next: Grid2::new(nx, ny), rhs: Grid2::new(nx, ny), diag: Grid2::new(nx, ny) }
    }

    /// Minus the net outflow (m/s) of each fluid cell before the last projection.
    pub fn rhs(&self) -> &Grid2<f32> {
        &self.rhs
    }
}

/// Project the grid velocity, warm-started from `pressure` (stored per unit time so the
/// warm start stays right when dt changes; updated on return).
///
/// Solves for cell pressures `P` (as velocity changes) such that every fluid cell ends up
/// divergence-free: `div_i + s_i P_i - sum(P_neighbours) = 0`, i.e.
/// `P_i = (sum(P_neighbours) - div_i) / s_i`, with P = 0 in air and solid neighbours
/// excluded from `s_i`. Then applies the gradient to the faces once.
///
/// Red-black Gauss-Seidel: same-coloured cells never neighbour each other, so a
/// half-sweep over one colour can read `p` and write `next` row by row in parallel, then
/// swap. Exactly equivalent to updating in place, but each row owns its output, which is
/// what the borrow checker needs to allow the parallel loop.
#[allow(clippy::too_many_arguments)]
pub(super) fn project(
    grid: &mut MacGrid,
    solver: &mut PressureSolver,
    pressure: &mut Grid2<f32>,
    density: &Grid2<f32>,
    dt: f32,
    iterations: u32,
    over_relax: f32,
    drift: Option<Drift>,
) {
    let (nx, ny) = (grid.nx, grid.ny);

    // Set up the system, warm-starting P from last substep.
    {
        let g: &MacGrid = grid;
        let store: &Grid2<f32> = pressure;
        let PressureSolver { p, rhs, diag, .. } = solver;
        p.par_rows_mut().zip(rhs.par_rows_mut()).zip(diag.par_rows_mut()).enumerate().for_each(
            |(j, ((p_row, rhs_row), diag_row))| {
                for i in 0..nx {
                    if g.cell[(i, j)] != CellType::Fluid {
                        (p_row[i], rhs_row[i], diag_row[i]) = (0.0, 0.0, 0.0);
                        continue;
                    }
                    let mut div = g.net_outflow(i, j);
                    if let Some(d) = drift {
                        let compression = density[(i, j)] / d.rest - 1.0;
                        if compression > 0.0 {
                            div -= d.gain * compression;
                        }
                    }
                    let s: f32 = open_sides(g, i, j).iter().sum();
                    p_row[i] = store[(i, j)] * dt;
                    rhs_row[i] = -div;
                    diag_row[i] = s;
                }
            },
        );
    }

    for _ in 0..iterations {
        half_sweep(solver, 0, over_relax);
        half_sweep(solver, 1, over_relax);
    }

    // Apply the pressure gradient to every face not touching a solid.
    let p = &solver.p;
    let cell = &grid.cell;
    let at = |i: isize, j: isize| -> Option<(usize, usize)> {
        (i >= 0 && j >= 0 && (i as usize) < nx && (j as usize) < ny).then_some((i as usize, j as usize))
    };
    let solid = |i: isize, j: isize| at(i, j).is_none_or(|c| cell[c] == CellType::Solid);
    let pv = |i: isize, j: isize| at(i, j).map_or(0.0, |c| p[c]);
    grid.u.par_rows_mut().enumerate().for_each(|(j, row)| {
        let j = j as isize;
        for (i, u) in row.iter_mut().enumerate() {
            let i = i as isize;
            if !solid(i - 1, j) && !solid(i, j) {
                *u += pv(i - 1, j) - pv(i, j);
            }
        }
    });
    grid.v.par_rows_mut().enumerate().for_each(|(j, row)| {
        let j = j as isize;
        for (i, v) in row.iter_mut().enumerate() {
            let i = i as isize;
            if !solid(i, j - 1) && !solid(i, j) {
                *v += pv(i, j - 1) - pv(i, j);
            }
        }
    });

    pressure.par_rows_mut().enumerate().for_each(|(j, row)| {
        for (i, out) in row.iter_mut().enumerate() {
            *out = p[(i, j)] / dt;
        }
    });
}

/// Update every cell of one colour (`(i + j) % 2 == colour`) from its neighbours.
fn half_sweep(solver: &mut PressureSolver, colour: usize, over_relax: f32) {
    let PressureSolver { p, next, rhs, diag } = solver;
    let (nx, ny) = (p.nx(), p.ny());
    let (p_ro, rhs, diag) = (&*p, &*rhs, &*diag);
    next.par_rows_mut().enumerate().for_each(|(j, row)| {
        for (i, out) in row.iter_mut().enumerate() {
            let old = p_ro[(i, j)];
            let s = diag[(i, j)];
            *out = if (i + j) % 2 == colour && s > 0.0 {
                let mut sum = 0.0;
                if i > 0 {
                    sum += p_ro[(i - 1, j)];
                }
                if i + 1 < nx {
                    sum += p_ro[(i + 1, j)];
                }
                if j > 0 {
                    sum += p_ro[(i, j - 1)];
                }
                if j + 1 < ny {
                    sum += p_ro[(i, j + 1)];
                }
                let gauss_seidel = (sum + rhs[(i, j)]) / s;
                // SOR, then the non-negativity constraint (projected Gauss-Seidel).
                (old + over_relax * (gauss_seidel - old)).max(0.0)
            } else {
                old
            };
        }
    });
    std::mem::swap(p, next);
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

/// Largest constraint violation over fluid cells, as an unbalanced flux in m/s: |net
/// outflow| where pressure is active, and only inflow where pressure is zero (fluid is
/// free to separate there). In m/s rather than 1/s (divergence) so one tolerance works
/// at every grid resolution.
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
    max
}
