//! Pressure projection, by red-black Gauss-Seidel with successive over-relaxation or by
//! Jacobi-preconditioned conjugate gradients, in parallel either way. Air cells have
//! p = 0 (free surface, Dirichlet); faces touching solids are never changed (Neumann).
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

/// How to solve the pressure system.
#[derive(Clone, Copy, Debug)]
pub(super) enum Method {
    /// Red-black Gauss-Seidel with SOR, a fixed number of iterations.
    GaussSeidel { iterations: u32, over_relax: f32 },
    /// Jacobi-preconditioned conjugate gradients until the max residual (m/s) is below
    /// `tolerance`, inside an active-set loop that enforces p >= 0.
    Pcg { max_iterations: u32, tolerance: f32 },
}

/// Plain CG can't express p >= 0 (no suction). Clamping its answer afterwards breaks the
/// balance around clamped cells and the fluid slowly expands (measured: +67% volume in a
/// dam break). Instead, a primal-dual active set: solve; release cells with p < 0 to
/// free-surface cells (p = 0, free to expand); re-admit released cells that have become
/// compressed (they need positive pressure again); re-solve warm-started; repeat until
/// nothing changes. Without re-admission, early releases got stuck and left 0.33 m/s
/// residuals in splashes.
const ACTIVE_SET_ROUNDS: u32 = 8;

/// Scratch grids for the solve, kept between substeps.
pub(super) struct PressureSolver {
    /// Current iterate, as a velocity change (pressure * dt / (rho h) in our units).
    p: Grid2<f32>,
    next: Grid2<f32>,
    /// Minus the (drift-corrected) divergence before projection, m/s.
    rhs: Grid2<f32>,
    /// Number of non-solid neighbours; 0 for cells that aren't fluid (never updated), or
    /// that the CG active set has released to p = 0.
    diag: Grid2<f32>,
    /// `diag` before any active-set releases, so released cells can be re-admitted.
    diag_full: Grid2<f32>,
    // Conjugate-gradient vectors: residual, preconditioned residual, search direction,
    // and the matrix applied to the search direction.
    r: Grid2<f32>,
    z: Grid2<f32>,
    dir: Grid2<f32>,
    a_dir: Grid2<f32>,
}

impl PressureSolver {
    pub fn new(nx: usize, ny: usize) -> Self {
        let g = || Grid2::new(nx, ny);
        Self { p: g(), next: g(), rhs: g(), diag: g(), diag_full: g(), r: g(), z: g(), dir: g(), a_dir: g() }
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
/// Returns the number of iterations used.
pub(super) fn project(
    grid: &mut MacGrid,
    solver: &mut PressureSolver,
    pressure: &mut Grid2<f32>,
    density: &Grid2<f32>,
    dt: f32,
    method: Method,
    drift: Option<Drift>,
) -> u32 {
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

    let iterations = match method {
        Method::GaussSeidel { iterations, over_relax } => {
            for _ in 0..iterations {
                half_sweep(solver, 0, over_relax);
                half_sweep(solver, 1, over_relax);
            }
            iterations
        }
        Method::Pcg { max_iterations, tolerance } => {
            // One iteration budget shared by all rounds. Cells are only released after a
            // converged solve: an unconverged iterate has spurious negative values.
            // Never finish on an active-set change without re-solving: that leaves exactly
            // the imbalance the active set exists to avoid.
            solver.diag_full.copy_from(&solver.diag);
            let mut used = 0;
            for round in 0..ACTIVE_SET_ROUNDS {
                let (iterations, converged) = pcg(solver, max_iterations - used, tolerance);
                used += iterations;
                if !converged || round + 1 == ACTIVE_SET_ROUNDS {
                    clamp_negative_pressures(solver);
                    break;
                }
                if update_active_set(solver, tolerance) == 0 {
                    break;
                }
            }
            used
        }
    };

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
    iterations
}

/// Fallback when CG runs out of iterations: enforce p >= 0 on the values only.
fn clamp_negative_pressures(solver: &mut PressureSolver) {
    solver.p.as_mut_slice().par_iter_mut().for_each(|p| *p = p.max(0.0));
}

/// One active-set update. Active cells with p < 0 are released: p = 0 and out of the
/// system (diag 0); neighbours still count them as open, so they act as a Dirichlet
/// p = 0 boundary, exactly like air. Released cells whose inflow exceeds `tolerance`
/// (compressed at p = 0, so they need positive pressure) are re-admitted. Returns the
/// number of cells that changed.
///
/// Two passes, because a cell's inflow depends on its neighbours' pressures: first read
/// `p` and write each released cell's residual into `r` (scratch), then flip cells.
fn update_active_set(solver: &mut PressureSolver, tolerance: f32) -> usize {
    let PressureSolver { p, rhs, diag, diag_full, r, .. } = solver;
    let (p_ro, rhs_ro, diag_ro, full_ro) = (&*p, &*rhs, &*diag, &*diag_full);
    r.par_rows_mut().enumerate().for_each(|(j, row)| {
        for (i, out) in row.iter_mut().enumerate() {
            // For a released cell (p = 0), residual = rhs - A p = rhs + sum(p_neighbours);
            // positive means net inflow.
            *out = if full_ro[(i, j)] > 0.0 && diag_ro[(i, j)] == 0.0 {
                let mut neighbours = 0.0;
                if i > 0 {
                    neighbours += p_ro[(i - 1, j)];
                }
                if i + 1 < p_ro.nx() {
                    neighbours += p_ro[(i + 1, j)];
                }
                if j > 0 {
                    neighbours += p_ro[(i, j - 1)];
                }
                if j + 1 < p_ro.ny() {
                    neighbours += p_ro[(i, j + 1)];
                }
                rhs_ro[(i, j)] + neighbours
            } else {
                0.0
            };
        }
    });
    let r_ro = &*r;
    p.par_rows_mut()
        .zip(diag.par_rows_mut())
        .enumerate()
        .map(|(j, (p_row, diag_row))| {
            let mut changed = 0;
            for (i, (p, s)) in p_row.iter_mut().zip(diag_row.iter_mut()).enumerate() {
                if *s > 0.0 && *p < 0.0 {
                    (*p, *s) = (0.0, 0.0);
                    changed += 1;
                } else if *s == 0.0 && full_ro[(i, j)] > 0.0 && r_ro[(i, j)] > tolerance {
                    *s = full_ro[(i, j)];
                    changed += 1;
                }
            }
            changed
        })
        .sum()
}

/// `(A v)_i = s_i v_i - sum(v_neighbours)` for fluid cells, 0 elsewhere. Vectors are zero
/// outside fluid cells, so summing every in-range neighbour drops air and solids for free.
#[inline]
fn apply_a(v: &Grid2<f32>, diag: &Grid2<f32>, i: usize, j: usize) -> f32 {
    let s = diag[(i, j)];
    if s == 0.0 {
        return 0.0;
    }
    let (nx, ny) = (v.nx(), v.ny());
    let mut neighbours = 0.0;
    if i > 0 {
        neighbours += v[(i - 1, j)];
    }
    if i + 1 < nx {
        neighbours += v[(i + 1, j)];
    }
    if j > 0 {
        neighbours += v[(i, j - 1)];
    }
    if j + 1 < ny {
        neighbours += v[(i, j + 1)];
    }
    s * v[(i, j)] - neighbours
}

/// Jacobi-preconditioned conjugate gradients on `A p = rhs`, from the warm-started `p`.
/// Stops when the largest residual (an unbalanced flux in m/s, like the invariant) is
/// below `tolerance`. Returns the iterations used and whether it converged.
///
/// Each iteration is three fused row-parallel passes (matvec + dot; update + dot + max;
/// new direction), against two synchronisations per Gauss-Seidel iteration, but it needs
/// far fewer iterations to converge because CG doesn't stall on smooth, deep modes.
/// Dot products accumulate in f64 per row.
fn pcg(solver: &mut PressureSolver, max_iterations: u32, tolerance: f32) -> (u32, bool) {
    let PressureSolver { p, rhs, diag, r, z, dir, a_dir, .. } = solver;
    let (p_ro, rhs_ro, diag_ro) = (&*p, &*rhs, &*diag);

    // r = b - A p, z = M⁻¹ r, d = z.
    let (mut rz, max_r) = r
        .par_rows_mut()
        .zip(z.par_rows_mut())
        .zip(dir.par_rows_mut())
        .enumerate()
        .map(|(j, ((r_row, z_row), d_row))| {
            let (mut rz, mut max_r) = (0.0f64, 0.0f32);
            for i in 0..r_row.len() {
                let s = diag_ro[(i, j)];
                let ri = if s > 0.0 { rhs_ro[(i, j)] - apply_a(p_ro, diag_ro, i, j) } else { 0.0 };
                let zi = if s > 0.0 { ri / s } else { 0.0 };
                (r_row[i], z_row[i], d_row[i]) = (ri, zi, zi);
                rz += f64::from(ri) * f64::from(zi);
                max_r = max_r.max(ri.abs());
            }
            (rz, max_r)
        })
        .reduce(|| (0.0, 0.0), |a, b| (a.0 + b.0, a.1.max(b.1)));
    if max_r <= tolerance {
        return (0, true);
    }

    for k in 0..max_iterations {
        // a_dir = A d, and d·A d.
        let d_ro = &*dir;
        let d_ad: f64 = a_dir
            .par_rows_mut()
            .enumerate()
            .map(|(j, row)| {
                let mut dot = 0.0f64;
                for (i, out) in row.iter_mut().enumerate() {
                    *out = apply_a(d_ro, diag_ro, i, j);
                    dot += f64::from(d_ro[(i, j)]) * f64::from(*out);
                }
                dot
            })
            .sum();
        if d_ad <= 0.0 {
            return (k, true); // converged to rounding, or a singular (all-Neumann) pocket
        }
        let alpha = (rz / d_ad) as f32;

        // p += α d, r -= α A d, z = M⁻¹ r; then r·z and max |r|.
        let ad_ro = &*a_dir;
        let (rz_new, max_r) = p
            .par_rows_mut()
            .zip(r.par_rows_mut())
            .zip(z.par_rows_mut())
            .enumerate()
            .map(|(j, ((p_row, r_row), z_row))| {
                let (mut rz, mut max_r) = (0.0f64, 0.0f32);
                for i in 0..p_row.len() {
                    let s = diag_ro[(i, j)];
                    if s == 0.0 {
                        continue;
                    }
                    p_row[i] += alpha * d_ro[(i, j)];
                    r_row[i] -= alpha * ad_ro[(i, j)];
                    z_row[i] = r_row[i] / s;
                    rz += f64::from(r_row[i]) * f64::from(z_row[i]);
                    max_r = max_r.max(r_row[i].abs());
                }
                (rz, max_r)
            })
            .reduce(|| (0.0, 0.0), |a, b| (a.0 + b.0, a.1.max(b.1)));
        if max_r <= tolerance {
            return (k + 1, true);
        }

        // d = z + β d.
        let beta = (rz_new / rz) as f32;
        rz = rz_new;
        let z_ro = &*z;
        dir.par_rows_mut().enumerate().for_each(|(j, row)| {
            for (i, d) in row.iter_mut().enumerate() {
                *d = z_ro[(i, j)] + beta * *d;
            }
        });
    }
    (max_iterations, false)
}

/// Update every cell of one colour (`(i + j) % 2 == colour`) from its neighbours.
fn half_sweep(solver: &mut PressureSolver, colour: usize, over_relax: f32) {
    let PressureSolver { p, next, rhs, diag, .. } = solver;
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
