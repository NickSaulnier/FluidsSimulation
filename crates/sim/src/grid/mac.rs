//! Staggered MAC grid and bilinear stencils.
//!
//! Layout (h = cell size, y up):
//! - cell (i, j) is centred at ((i + 1/2) h, (j + 1/2) h), `nx x ny` cells;
//! - `u(i, j)` lives on the left face of cell (i, j), at (i h, (j + 1/2) h): `(nx+1) x ny`;
//! - `v(i, j)` lives on the bottom face of cell (i, j), at ((i + 1/2) h, j h): `nx x (ny+1)`.

use super::Grid2;
use glam::Vec2;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum CellType {
    #[default]
    Air,
    Fluid,
    Solid,
}

/// Where a field's samples sit within a cell, in cell units.
pub const U_OFFSET: Vec2 = Vec2::new(0.0, 0.5);
pub const V_OFFSET: Vec2 = Vec2::new(0.5, 0.0);
pub const CENTRE_OFFSET: Vec2 = Vec2::new(0.5, 0.5);

pub struct MacGrid {
    pub nx: usize,
    pub ny: usize,
    pub h: f32,
    pub u: Grid2<f32>,
    pub v: Grid2<f32>,
    pub cell: Grid2<CellType>,
}

impl MacGrid {
    pub fn new(nx: usize, ny: usize, h: f32) -> Self {
        Self { nx, ny, h, u: Grid2::new(nx + 1, ny), v: Grid2::new(nx, ny + 1), cell: Grid2::new(nx, ny) }
    }

    /// Cell type with out-of-range cells treated as solid, so face loops need no edge cases.
    #[inline]
    pub fn cell_type(&self, i: isize, j: isize) -> CellType {
        if i < 0 || j < 0 || i >= self.nx as isize || j >= self.ny as isize {
            CellType::Solid
        } else {
            self.cell[(i as usize, j as usize)]
        }
    }

    /// Static solids: zero the velocity on every face touching a solid cell.
    pub fn zero_solid_faces(&mut self) {
        let (nx, ny) = (self.nx as isize, self.ny as isize);
        for j in 0..ny {
            for i in 0..=nx {
                if self.cell_type(i - 1, j) == CellType::Solid || self.cell_type(i, j) == CellType::Solid {
                    self.u[(i as usize, j as usize)] = 0.0;
                }
            }
        }
        for j in 0..=ny {
            for i in 0..nx {
                if self.cell_type(i, j - 1) == CellType::Solid || self.cell_type(i, j) == CellType::Solid {
                    self.v[(i as usize, j as usize)] = 0.0;
                }
            }
        }
    }

    /// Net outflow of cell (i, j) in m/s (divide by h for the true divergence in 1/s).
    #[inline]
    pub fn net_outflow(&self, i: usize, j: usize) -> f32 {
        self.u[(i + 1, j)] - self.u[(i, j)] + self.v[(i, j + 1)] - self.v[(i, j)]
    }
}

/// The four samples around a point and their bilinear weights. Corners are
/// `(i, j), (i+1, j), (i, j+1), (i+1, j+1)` in that order.
#[derive(Clone, Copy, Debug)]
pub struct Stencil {
    pub i: usize,
    pub j: usize,
    pub w: [f32; 4],
}

impl Stencil {
    /// Stencil for point `p` on a sample lattice at `(index + offset) * h`, of size `nx x ny`.
    /// Points outside the lattice are clamped onto its edge.
    #[inline]
    pub fn new(p: Vec2, h: f32, offset: Vec2, nx: usize, ny: usize) -> Self {
        let g = p / h - offset;
        let i = (g.x.floor().max(0.0) as usize).min(nx - 2);
        let j = (g.y.floor().max(0.0) as usize).min(ny - 2);
        let tx = (g.x - i as f32).clamp(0.0, 1.0);
        let ty = (g.y - j as f32).clamp(0.0, 1.0);
        let (sx, sy) = (1.0 - tx, 1.0 - ty);
        Self { i, j, w: [sx * sy, tx * sy, sx * ty, tx * ty] }
    }

    /// Stencil for `p` on the lattice of an existing grid.
    #[inline]
    pub fn on<T>(grid: &Grid2<T>, h: f32, offset: Vec2, p: Vec2) -> Self {
        Self::new(p, h, offset, grid.nx(), grid.ny())
    }

    #[inline]
    pub fn corners(&self) -> [(usize, usize); 4] {
        let (i, j) = (self.i, self.j);
        [(i, j), (i + 1, j), (i, j + 1), (i + 1, j + 1)]
    }

    #[inline]
    pub fn sample(&self, field: &Grid2<f32>) -> f32 {
        self.corners().iter().zip(self.w).map(|(&c, w)| field[c] * w).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use utils::rng::SplitMix64;

    #[test]
    fn weights_sum_to_one_everywhere() {
        let g = MacGrid::new(20, 10, 0.1);
        let mut rng = SplitMix64::new(1);
        for _ in 0..10_000 {
            // Include points outside the domain to exercise clamping.
            let p = Vec2::new(rng.next_f32() * 2.4 - 0.2, rng.next_f32() * 1.4 - 0.2);
            for (field_dims, offset) in [((21, 10), U_OFFSET), ((20, 11), V_OFFSET), ((20, 10), CENTRE_OFFSET)] {
                let s = Stencil::new(p, g.h, offset, field_dims.0, field_dims.1);
                let sum: f32 = s.w.iter().sum();
                assert!((sum - 1.0).abs() < 1e-5, "sum {sum} at {p}");
                assert!(s.w.iter().all(|&w| (0.0..=1.0).contains(&w)));
            }
        }
    }

    #[test]
    fn sampling_a_linear_field_is_exact() {
        let (nx, ny, h) = (16, 8, 0.25);
        let mut u = Grid2::<f32>::new(nx + 1, ny);
        let f = |p: Vec2| 1.0 + 2.0 * p.x - 3.0 * p.y;
        for j in 0..ny {
            for i in 0..=nx {
                u[(i, j)] = f((Vec2::new(i as f32, j as f32) + U_OFFSET) * h);
            }
        }
        for p in [Vec2::new(0.3, 0.4), Vec2::new(3.9, 1.8), Vec2::new(2.0, 1.0)] {
            let s = Stencil::on(&u, h, U_OFFSET, p);
            assert!((s.sample(&u) - f(p)).abs() < 1e-4, "at {p}");
        }
    }
}
