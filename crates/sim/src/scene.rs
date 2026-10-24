//! Solver-agnostic description of the world: domain, gravity, obstacles, initial fluid.

use crate::grid::Grid2;
use glam::Vec2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub min: Vec2,
    pub max: Vec2,
}

impl Rect {
    pub fn new(min: Vec2, max: Vec2) -> Self {
        Self { min, max }
    }

    pub fn size(&self) -> Vec2 {
        self.max - self.min
    }

    pub fn contains(&self, p: Vec2) -> bool {
        p.cmpge(self.min).all() && p.cmplt(self.max).all()
    }

    pub fn intersect(&self, other: &Rect) -> Rect {
        Rect::new(self.min.max(other.min), self.max.min(other.max))
    }
}

pub struct Scene {
    /// The domain is `[0, size.x] x [0, size.y]`, in metres, y up.
    pub size: Vec2,
    pub gravity: Vec2,
    pub solid: SolidMask,
    /// Regions filled with fluid on reset.
    pub fluid_blocks: Vec<Rect>,
}

impl Scene {
    /// An empty box: domain walls only, no fluid.
    pub fn empty_box(size: Vec2, mask_cells_y: usize) -> Self {
        Self {
            size,
            gravity: Vec2::new(0.0, -9.81),
            solid: SolidMask::new_box(size, size.y / mask_cells_y as f32),
            fluid_blocks: Vec::new(),
        }
    }
}

/// Thickness of the always-solid domain border, in mask cells. The SDF can only measure
/// penetration up to roughly the wall thickness minus half a cell, so this bounds how
/// far a particle may overshoot in one substep (FLIP moves up to `cfl` grid cells, which
/// is ~2.5 mask cells at default settings).
pub const BORDER_CELLS: usize = 4;

/// Obstacles and walls as one occupancy grid with a signed distance field.
///
/// The border cells are always solid, so domain walls and painted obstacles are the
/// same thing. Solvers resample this at their own resolution.
pub struct SolidMask {
    cell: f32,
    /// 0 = open, 255 = solid. Uploads directly as an `R8Unorm` texture.
    occ: Grid2<u8>,
    /// Signed distance sampled at cell centres, metres, negative inside solids.
    sdf: Grid2<f32>,
    sdf_dirty: bool,
    /// Bumped on every edit, so solvers can cache derived data by it.
    generation: u64,
}

impl SolidMask {
    pub fn new_box(size: Vec2, cell: f32) -> Self {
        let nx = (size.x / cell).round() as usize;
        let ny = (size.y / cell).round() as usize;
        let mut occ = Grid2::new(nx, ny);
        let b = BORDER_CELLS;
        for j in 0..ny {
            for i in 0..nx {
                if i < b || j < b || i >= nx - b || j >= ny - b {
                    occ[(i, j)] = 255;
                }
            }
        }
        let mut mask = Self { cell, occ, sdf: Grid2::new(nx, ny), sdf_dirty: true, generation: 0 };
        mask.update_derived();
        mask
    }

    pub fn cell_size(&self) -> f32 {
        self.cell
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The open box inside the always-solid border. Clamping particles to it makes
    /// tunnelling through the domain walls impossible at any speed.
    pub fn interior(&self) -> Rect {
        let wall = BORDER_CELLS as f32 * self.cell;
        let size = Vec2::new(self.occ.nx() as f32, self.occ.ny() as f32) * self.cell;
        Rect::new(Vec2::splat(wall), size - wall)
    }

    /// `(nx, ny, bytes)`, row-major with row 0 at the bottom (y = 0).
    pub fn occupancy(&self) -> (usize, usize, &[u8]) {
        (self.occ.nx(), self.occ.ny(), self.occ.as_slice())
    }

    /// Mark (or clear) every cell whose centre lies within `radius` of `center`.
    /// The border stays solid regardless.
    pub fn paint_disc(&mut self, center: Vec2, radius: f32, solid: bool) {
        let (nx, ny) = (self.occ.nx(), self.occ.ny());
        let b = BORDER_CELLS;
        let lo = ((center - radius) / self.cell).floor().max(Vec2::splat(b as f32));
        let hi = ((center + radius) / self.cell).ceil();
        let (i0, j0) = (lo.x as usize, lo.y as usize);
        let (i1, j1) = ((hi.x as usize).min(nx - b), (hi.y as usize).min(ny - b));
        let value = if solid { 255 } else { 0 };
        for j in j0..j1 {
            for i in i0..i1 {
                let c = (Vec2::new(i as f32, j as f32) + 0.5) * self.cell;
                if c.distance_squared(center) <= radius * radius {
                    self.occ[(i, j)] = value;
                }
            }
        }
        self.sdf_dirty = true;
        self.generation += 1;
    }

    pub fn is_solid(&self, p: Vec2) -> bool {
        let (i, j) = self.cell_of(p);
        self.occ[(i, j)] != 0
    }

    /// Signed distance to the nearest solid boundary (bilinear between cell centres).
    pub fn distance(&self, p: Vec2) -> f32 {
        let (nx, ny) = (self.sdf.nx(), self.sdf.ny());
        let g = (p / self.cell - 0.5).clamp(Vec2::ZERO, Vec2::new((nx - 1) as f32, (ny - 1) as f32));
        let (i, j) = ((g.x as usize).min(nx - 2), (g.y as usize).min(ny - 2));
        let (fx, fy) = (g.x - i as f32, g.y - j as f32);
        let s = &self.sdf;
        let bottom = s[(i, j)] * (1.0 - fx) + s[(i + 1, j)] * fx;
        let top = s[(i, j + 1)] * (1.0 - fx) + s[(i + 1, j + 1)] * fx;
        bottom * (1.0 - fy) + top * fy
    }

    /// Outward unit normal of the nearest solid (points from solid into open space).
    pub fn normal(&self, p: Vec2) -> Vec2 {
        let e = self.cell;
        let dx = self.distance(p + Vec2::X * e) - self.distance(p - Vec2::X * e);
        let dy = self.distance(p + Vec2::Y * e) - self.distance(p - Vec2::Y * e);
        Vec2::new(dx, dy).normalize_or_zero()
    }

    /// Rebuild the SDF if the mask changed. Called once per frame by `World`, not per
    /// query, so the query methods can stay `&self` with no interior mutability.
    pub(crate) fn update_derived(&mut self) {
        if !self.sdf_dirty {
            return;
        }
        // Chamfer distance (cell units) from open cells to the nearest solid, and from
        // solid cells to the nearest open cell. Each is 0 on its own "source" side.
        let to_solid = chamfer(&self.occ, |v| v != 0);
        let to_open = chamfer(&self.occ, |v| v == 0);
        for (k, d) in self.sdf.as_mut_slice().iter_mut().enumerate() {
            // Cell centres adjacent to the boundary are half a cell away from it.
            *d = if self.occ.as_slice()[k] == 0 {
                (to_solid[k] - 0.5) * self.cell
            } else {
                -(to_open[k] - 0.5) * self.cell
            };
        }
        self.sdf_dirty = false;
    }

    fn cell_of(&self, p: Vec2) -> (usize, usize) {
        let g = (p / self.cell).floor().max(Vec2::ZERO);
        ((g.x as usize).min(self.occ.nx() - 1), (g.y as usize).min(self.occ.ny() - 1))
    }
}

/// Two-pass 3x3 chamfer distance transform (weights 1 and sqrt 2), in cell units.
/// Approximate (a few percent error on diagonals), which is plenty for collisions.
fn chamfer(occ: &Grid2<u8>, is_source: impl Fn(u8) -> bool) -> Vec<f32> {
    let (nx, ny) = (occ.nx(), occ.ny());
    let far = (nx + ny) as f32;
    let mut d: Vec<f32> = occ.as_slice().iter().map(|&v| if is_source(v) { 0.0 } else { far }).collect();
    let diag = std::f32::consts::SQRT_2;
    let at = |i: usize, j: usize| j * nx + i;

    for j in 0..ny {
        for i in 0..nx {
            let mut best = d[at(i, j)];
            if i > 0 { best = best.min(d[at(i - 1, j)] + 1.0); }
            if j > 0 {
                best = best.min(d[at(i, j - 1)] + 1.0);
                if i > 0 { best = best.min(d[at(i - 1, j - 1)] + diag); }
                if i + 1 < nx { best = best.min(d[at(i + 1, j - 1)] + diag); }
            }
            d[at(i, j)] = best;
        }
    }
    for j in (0..ny).rev() {
        for i in (0..nx).rev() {
            let mut best = d[at(i, j)];
            if i + 1 < nx { best = best.min(d[at(i + 1, j)] + 1.0); }
            if j + 1 < ny {
                best = best.min(d[at(i, j + 1)] + 1.0);
                if i + 1 < nx { best = best.min(d[at(i + 1, j + 1)] + diag); }
                if i > 0 { best = best.min(d[at(i - 1, j + 1)] + diag); }
            }
            d[at(i, j)] = best;
        }
    }
    d
}

/// A circular "push" applied by the mouse: velocities inside `radius` are pulled toward
/// `velocity` at `strength` per second, fading linearly to zero at the rim.
#[derive(Clone, Copy, Debug)]
pub struct ForceBrush {
    pub pos: Vec2,
    pub radius: f32,
    pub velocity: Vec2,
    /// Blend rate in 1/s.
    pub strength: f32,
}

/// Per-frame user interaction. Each solver interprets it in its own way.
#[derive(Default, Clone, Debug)]
pub struct StepInput {
    pub brushes: Vec<ForceBrush>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mask() -> SolidMask {
        SolidMask::new_box(Vec2::new(2.0, 1.0), 1.0 / 64.0)
    }

    #[test]
    fn box_sdf_signs() {
        let m = mask();
        let c = m.cell_size();
        let wall = BORDER_CELLS as f32 * c;
        assert!(m.distance(Vec2::new(1.0, 0.5)) > 0.4);
        // The wall face is at x = wall; distance there should be ~0.
        assert!(m.distance(Vec2::new(wall, 0.5)).abs() < 0.1 * c);
        // Half a cell into the open, distance ~ half a cell.
        assert!((m.distance(Vec2::new(wall + 0.5 * c, 0.5)) - 0.5 * c).abs() < 0.1 * c);
        // A full cell into the wall is still measured correctly (this is what the
        // thick border buys).
        assert!((m.distance(Vec2::new(wall - c, 0.5)) + c).abs() < 0.1 * c);
    }

    #[test]
    fn normals_point_into_the_domain() {
        let m = mask();
        let wall = BORDER_CELLS as f32 * m.cell_size();
        // Just outside and just inside the wall face.
        for offset in [1.0, -1.0] {
            let d = wall + offset * m.cell_size();
            let n = m.normal(Vec2::new(d, 0.5));
            assert!(n.x > 0.9, "left wall normal {n} at x = {d}");
            let n = m.normal(Vec2::new(1.0, d));
            assert!(n.y > 0.9, "floor normal {n} at y = {d}");
        }
    }

    #[test]
    fn painting_marks_solid_and_bumps_generation() {
        let mut m = mask();
        let g0 = m.generation();
        let p = Vec2::new(1.0, 0.5);
        m.paint_disc(p, 0.1, true);
        m.update_derived();
        assert!(m.generation() > g0);
        assert!(m.is_solid(p));
        assert!(m.distance(p) < -0.05);
        m.paint_disc(p, 0.2, false);
        m.update_derived();
        assert!(!m.is_solid(p));
        assert!(m.distance(p) > 0.3);
    }
}
