//! Geometric multigrid V-cycle, used as the preconditioner for conjugate gradients
//! (MGPCG: McAdams, Sifakis & Teran 2010).
//!
//! Gauss-Seidel and Jacobi-preconditioned CG both fix errors one cell's reach per
//! sweep, so smooth, domain-sized pressure modes take O(n) iterations to settle. A
//! V-cycle smooths the short-wavelength error on the fine grid, then hands what's left
//! to a grid half the size, where it's short-wavelength again, recursively. CG
//! preconditioned with one V-cycle converges in a nearly resolution-independent number
//! of iterations.
//!
//! The preconditioner must be symmetric positive definite for CG, which fixes the
//! ingredients: red-black Gauss-Seidel smoothing, red then black before the coarse
//! correction and black then red after (each the other's adjoint); bilinear
//! prolongation and its exact transpose as restriction; and symmetric sweeps from zero
//! as the coarsest "solve".
//!
//! Boundaries coarsen as in McAdams et al.: a coarse cell is Dirichlet (air, or a cell
//! the active set released) if any child is, else active if any child is, else solid.
//!
//! Serial on purpose. At these grid sizes (~20k fine cells) one pass is ~20 µs of work
//! and a V-cycle is about ten passes; rayon's per-pass dispatch cost matched or beat the
//! parallel speedup at every level (measured: 180 µs serial vs 180-430 µs parallel).

use crate::grid::Grid2;

const PRE_SWEEPS: u32 = 1;
const POST_SWEEPS: u32 = 1;
/// Symmetric (forward + backward) sweep pairs on the coarsest level.
const COARSEST_SWEEPS: u32 = 10;
/// Stop coarsening once the shorter side is at most this many cells.
const COARSEST_SIZE: usize = 6;
/// Bilinear prolongation weights for the fine cells 2I-1, 2I, 2I+1, 2I+2 around coarse
/// cell I (cell-centred grids). Used as a restriction they sum to 2 per axis, 4 per
/// coarse cell: its area in fine cells. That's the right scaling for the unscaled
/// operator (h² times the Laplacian) rediscretised on the coarse grid.
const W: [f32; 4] = [0.25, 0.75, 0.75, 0.25];
const FORWARD: [usize; 2] = [0, 1];
const BACKWARD: [usize; 2] = [1, 0];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Kind {
    #[default]
    Solid,
    Dirichlet,
    Active,
}

struct Level {
    nx: usize,
    ny: usize,
    kind: Vec<Kind>,
    /// Non-solid neighbours of active cells, 0 elsewhere: the operator's diagonal.
    diag: Vec<f32>,
    x: Vec<f32>,
    b: Vec<f32>,
    residual: Vec<f32>,
}

impl Level {
    fn new(nx: usize, ny: usize) -> Self {
        let n = nx * ny;
        Self { nx, ny, kind: vec![Kind::Solid; n], diag: vec![0.0; n], x: vec![0.0; n], b: vec![0.0; n], residual: vec![0.0; n] }
    }

    fn update_diag(&mut self) {
        let (nx, ny) = (self.nx, self.ny);
        for j in 0..ny {
            for i in 0..nx {
                let k = j * nx + i;
                self.diag[k] = if self.kind[k] != Kind::Active {
                    0.0
                } else {
                    let open = |k: usize| if self.kind[k] == Kind::Solid { 0.0 } else { 1.0 };
                    let mut n = 0.0;
                    if i > 0 {
                        n += open(k - 1);
                    }
                    if i + 1 < nx {
                        n += open(k + 1);
                    }
                    if j > 0 {
                        n += open(k - nx);
                    }
                    if j + 1 < ny {
                        n += open(k + nx);
                    }
                    n
                };
            }
        }
    }

    /// Sum of `v` over the four neighbours of cell `k = (i, j)`.
    #[inline]
    fn neighbours(&self, v: &[f32], i: usize, j: usize, k: usize) -> f32 {
        let mut n = 0.0;
        if i > 0 {
            n += v[k - 1];
        }
        if i + 1 < self.nx {
            n += v[k + 1];
        }
        if j > 0 {
            n += v[k - self.nx];
        }
        if j + 1 < self.ny {
            n += v[k + self.nx];
        }
        n
    }

    /// In-place red-black Gauss-Seidel on `A x = b`, colours in the given order.
    /// Non-active cells of `x` stay 0, so they drop out of the neighbour sums.
    fn smooth(&mut self, order: [usize; 2]) {
        let (nx, ny) = (self.nx, self.ny);
        let mut x = std::mem::take(&mut self.x);
        for colour in order {
            for j in 0..ny {
                for i in ((colour + j) % 2..nx).step_by(2) {
                    let k = j * nx + i;
                    let s = self.diag[k];
                    if s > 0.0 {
                        x[k] = (self.b[k] + self.neighbours(&x, i, j, k)) / s;
                    }
                }
            }
        }
        self.x = x;
    }

    /// `residual = b - A x` on active cells, 0 elsewhere.
    fn compute_residual(&mut self) {
        let (nx, ny) = (self.nx, self.ny);
        for j in 0..ny {
            for i in 0..nx {
                let k = j * nx + i;
                let s = self.diag[k];
                self.residual[k] = if s > 0.0 { self.b[k] - (s * self.x[k] - self.neighbours(&self.x, i, j, k)) } else { 0.0 };
            }
        }
    }
}

#[derive(Default)]
pub(super) struct Multigrid {
    levels: Vec<Level>,
    /// Without any Dirichlet cell (a closed, all-fluid domain) the system is singular,
    /// defined up to a constant; the preconditioned residual then has its mean removed.
    has_dirichlet: bool,
}

impl Multigrid {
    /// Build the hierarchy for the fine system: `diag > 0` marks unknowns, `solid` walls,
    /// and everything else is Dirichlet (p = 0). Call again whenever `diag` changes.
    pub fn setup(&mut self, diag: &Grid2<f32>, solid: &Grid2<bool>) {
        let (nx, ny) = (diag.nx(), diag.ny());
        if self.levels.first().is_none_or(|l| l.nx != nx || l.ny != ny) {
            self.levels.clear();
            let (mut w, mut h) = (nx, ny);
            loop {
                self.levels.push(Level::new(w, h));
                if w.min(h) <= COARSEST_SIZE {
                    break;
                }
                (w, h) = (w.div_ceil(2), h.div_ceil(2));
            }
        }
        let fine = &mut self.levels[0];
        for ((k, &s), &wall) in fine.kind.iter_mut().zip(diag.as_slice()).zip(solid.as_slice()) {
            *k = if wall {
                Kind::Solid
            } else if s > 0.0 {
                Kind::Active
            } else {
                Kind::Dirichlet
            };
        }
        self.has_dirichlet = fine.kind.contains(&Kind::Dirichlet);
        fine.update_diag();
        for l in 1..self.levels.len() {
            let (finer, coarser) = self.levels.split_at_mut(l);
            let (fine, coarse) = (&finer[l - 1], &mut coarser[0]);
            for cj in 0..coarse.ny {
                for ci in 0..coarse.nx {
                    let (mut any_active, mut any_dirichlet) = (false, false);
                    for (i, j) in [(2 * ci, 2 * cj), (2 * ci + 1, 2 * cj), (2 * ci, 2 * cj + 1), (2 * ci + 1, 2 * cj + 1)] {
                        if i < fine.nx && j < fine.ny {
                            let k = fine.kind[j * fine.nx + i];
                            any_active |= k == Kind::Active;
                            any_dirichlet |= k == Kind::Dirichlet;
                        }
                    }
                    coarse.kind[cj * coarse.nx + ci] = if any_dirichlet {
                        Kind::Dirichlet
                    } else if any_active {
                        Kind::Active
                    } else {
                        Kind::Solid
                    };
                }
            }
            coarse.update_diag();
        }
    }

    /// `z ≈ A⁻¹ r`: one V-cycle from zero. `setup` must have been called for this system.
    pub fn apply(&mut self, r: &Grid2<f32>, z: &mut Grid2<f32>) {
        let fine = &mut self.levels[0];
        fine.b.copy_from_slice(r.as_slice());
        v_cycle(&mut self.levels);
        let fine = &self.levels[0];
        z.as_mut_slice().copy_from_slice(&fine.x);
        if !self.has_dirichlet {
            let (sum, n) = z
                .as_slice()
                .iter()
                .zip(&fine.diag)
                .filter(|(_, s)| **s > 0.0)
                .fold((0.0f64, 0usize), |(sum, n), (z, _)| (sum + f64::from(*z), n + 1));
            let mean = (sum / n.max(1) as f64) as f32;
            for (z, s) in z.as_mut_slice().iter_mut().zip(&fine.diag) {
                if *s > 0.0 {
                    *z -= mean;
                }
            }
        }
    }
}

/// Solve `levels[0]` approximately, from x = 0, recursing into the coarser levels.
fn v_cycle(levels: &mut [Level]) {
    let (fine, coarser) = levels.split_first_mut().expect("at least one level");
    fine.x.fill(0.0);
    let Some(coarse) = coarser.first_mut() else {
        for _ in 0..COARSEST_SWEEPS {
            fine.smooth(FORWARD);
            fine.smooth(BACKWARD);
        }
        return;
    };
    for _ in 0..PRE_SWEEPS {
        fine.smooth(FORWARD);
    }
    fine.compute_residual();
    restrict(fine, coarse);
    v_cycle(coarser);
    prolong_add(&coarser[0], fine);
    for _ in 0..POST_SWEEPS {
        fine.smooth(BACKWARD);
    }
}

/// Coarse right-hand side = transpose of `prolong_add` applied to the fine residual.
fn restrict(fine: &Level, coarse: &mut Level) {
    let (fnx, fny) = (fine.nx as isize, fine.ny as isize);
    for cj in 0..coarse.ny {
        for ci in 0..coarse.nx {
            let ck = cj * coarse.nx + ci;
            if coarse.diag[ck] == 0.0 {
                coarse.b[ck] = 0.0;
                continue;
            }
            let mut sum = 0.0;
            for (b, wy) in W.iter().enumerate() {
                let j = 2 * cj as isize - 1 + b as isize;
                if j < 0 || j >= fny {
                    continue;
                }
                for (a, wx) in W.iter().enumerate() {
                    let i = 2 * ci as isize - 1 + a as isize;
                    if i >= 0 && i < fnx {
                        sum += wx * wy * fine.residual[j as usize * fine.nx + i as usize];
                    }
                }
            }
            coarse.b[ck] = sum;
        }
    }
}

/// Bilinear interpolation of the coarse correction onto active fine cells: 3/4 from the
/// coarse cell containing it, 1/4 from the next one toward it, per axis.
fn prolong_add(coarse: &Level, fine: &mut Level) {
    let (cnx, cny) = (coarse.nx as isize, coarse.ny as isize);
    let at = |i: isize, j: isize| if i >= 0 && j >= 0 && i < cnx && j < cny { coarse.x[j as usize * coarse.nx + i as usize] } else { 0.0 };
    for j in 0..fine.ny {
        for i in 0..fine.nx {
            let k = j * fine.nx + i;
            if fine.diag[k] == 0.0 {
                continue;
            }
            let (ci, cj) = ((i / 2) as isize, (j / 2) as isize);
            let di = if i % 2 == 0 { -1 } else { 1 };
            let dj = if j % 2 == 0 { -1 } else { 1 };
            fine.x[k] += 0.5625 * at(ci, cj) + 0.1875 * (at(ci + di, cj) + at(ci, cj + dj)) + 0.0625 * at(ci + di, cj + dj);
        }
    }
}

