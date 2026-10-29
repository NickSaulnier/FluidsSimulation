//! MLS-MPM (Hu et al. 2018, "A Moving Least Squares Material Point Method"): particles
//! that carry a material's state, with a background grid to move them. Three materials,
//! each its own solver: jelly (elastic: it wobbles and bounces), snow (it packs, hardens
//! as it does, and breaks into clumps; Stomakhin et al. 2013) and sand (it flows and
//! piles at its friction angle; Drucker-Prager, Klár et al. 2016).
//!
//! Each particle carries a position, a velocity, an affine velocity C (APIC), a
//! deformation gradient F and, for snow, its plastic volume change Jp. Per substep:
//! particles to grid (each particle's momentum and stress spread over the 3x3 nodes
//! round it, quadratic B-spline weights) -> the grid's velocities: gravity, brushes,
//! walls (slip, with Coulomb friction for sand) -> grid to particles (velocity, C, and
//! F advanced by (I + dt C)) -> plasticity (snow and sand) -> moved, kept out of walls.
//!
//! Explicit and elastic: the substep is bounded by the material's sound speed, so it
//! takes many small, cheap substeps (`max_substeps`). Stiffness is per unit density
//! (m²/s²), so it reads as a sound speed squared: sqrt(stiffness) m/s.

use crate::params::{FloatSpec, IntSpec, ParamVisitor};
use crate::scene::{Scene, StepInput};
use crate::solver::{Init, Metric, ParticleView, Solver, Stats};
use crate::timing::PhaseTimer;
use glam::{Mat2, Vec2};
use rayon::prelude::*;

use super::PAR_CHUNK;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Material {
    Jelly,
    Snow,
    Sand,
}

#[derive(Clone, Debug)]
pub struct MpmParams {
    pub cells_y: u32,
    /// Young's modulus over density, m²/s².
    pub stiffness: f32,
    pub poisson: f32,
    /// Snow: how much harder it gets as it's packed (ξ).
    pub hardening: f32,
    /// Snow: how far it compresses, and stretches, before it gives (θc, θs).
    pub compression: f32,
    pub stretch: f32,
    /// Sand: friction angle, degrees.
    pub friction_angle: f32,
    /// Max particle travel per substep, in cells.
    pub cfl: f32,
}

impl MpmParams {
    pub fn for_material(material: Material) -> Self {
        let base = Self { cells_y: 64, stiffness: 40.0, poisson: 0.2, hardening: 10.0, compression: 0.025, stretch: 0.0045, friction_angle: 35.0, cfl: 0.5 };
        match material {
            Material::Jelly => Self { stiffness: 30.0, poisson: 0.3, ..base },
            Material::Snow => Self { stiffness: 25.0, ..base },
            Material::Sand => Self { stiffness: 60.0, poisson: 0.3, ..base },
        }
    }

    /// Lamé's μ and λ.
    fn lame(&self) -> (f32, f32) {
        let (e, nu) = (self.stiffness, self.poisson);
        (e / (2.0 * (1.0 + nu)), e * nu / ((1.0 + nu) * (1.0 - 2.0 * nu)))
    }
}

const CELLS_Y: IntSpec = IntSpec {
    key: "cells_y",
    label: "Grid cells (height)",
    range: 24..=160,
    restart: true,
    help: "Resolution. Particles grow with its square (4 per cell), and so does the cost.",
};
const STIFFNESS: FloatSpec = FloatSpec {
    key: "stiffness",
    label: "Stiffness (m²/s²)",
    range: 5.0..=300.0,
    log: true,
    restart: false,
    help: "Young's modulus over density: how hard it is. Stiffer needs smaller substeps (its square root is its sound speed).",
};
const POISSON: FloatSpec = FloatSpec {
    key: "poisson",
    label: "Poisson ratio",
    range: 0.0..=0.45,
    log: false,
    restart: false,
    help: "How much it bulges sideways when squeezed: 0 not at all, toward 0.5 like rubber.",
};
const HARDENING: FloatSpec = FloatSpec {
    key: "hardening",
    label: "Hardening",
    range: 0.0..=20.0,
    log: false,
    restart: false,
    help: "How much harder snow gets as it's packed, and softer as it's pulled apart.",
};
const COMPRESSION: FloatSpec = FloatSpec {
    key: "compression",
    label: "Critical compression",
    range: 0.005..=0.1,
    log: true,
    restart: false,
    help: "How far snow squeezes before it packs for good. Larger: wetter, more like slush.",
};
const STRETCH: FloatSpec = FloatSpec {
    key: "stretch",
    label: "Critical stretch",
    range: 0.001..=0.02,
    log: true,
    restart: false,
    help: "How far snow stretches before it breaks. Smaller: more brittle, crumbling into clumps.",
};
const FRICTION_ANGLE: FloatSpec = FloatSpec {
    key: "friction_angle",
    label: "Friction angle (°)",
    range: 10.0..=50.0,
    log: false,
    restart: false,
    help: "The steepest slope sand piles at.",
};
const CFL: FloatSpec = FloatSpec {
    key: "cfl",
    label: "CFL (cells per substep)",
    range: 0.1..=0.8,
    log: false,
    restart: false,
    help: "Max travel per substep, for the fastest particle or sound wave.",
};

/// Particles per grid cell, along each axis.
const PER_CELL: f32 = 2.0;
/// Density, kg/m² (stress is per unit density, so it only scales the momentum).
const DENSITY: f32 = 1.0;
/// Packed snow gets at most this many times stiffer: real snow goes on hardening, but
/// each doubling of stiffness costs 40 % more substeps.
const HARDEN_CAP: f32 = 4.0;
/// Sand's friction against walls (Coulomb).
const WALL_FRICTION: f32 = 0.5;

/// A 2x2 matrix split as U Σ Vᵀ (U and V rotations). Σ may hold a negative value for an
/// inverted F.
fn svd(f: Mat2) -> (Mat2, Vec2, Mat2) {
    let (a, c, b, d) = (f.x_axis.x, f.x_axis.y, f.y_axis.x, f.y_axis.y);
    // Polar: F = R S, R a rotation, S symmetric.
    let r = Mat2::from_angle((c - b).atan2(a + d));
    let s = r.transpose() * f;
    // S = V Σ Vᵀ.
    let (s00, s01, s11) = (s.x_axis.x, s.y_axis.x, s.y_axis.y);
    let v = Mat2::from_angle(0.5 * (2.0 * s01).atan2(s00 - s11));
    let sigma = v.transpose() * s * v;
    (r * v, Vec2::new(sigma.x_axis.x, sigma.y_axis.y), v)
}

fn diag(s: Vec2) -> Mat2 {
    Mat2::from_diagonal(s)
}

/// `a` ⊗ `b` (a bᵀ).
fn outer(a: Vec2, b: Vec2) -> Mat2 {
    Mat2::from_cols(a * b.x, a * b.y)
}

/// Quadratic B-spline weights for the three nodes from `base`, at `fx` (in cells from
/// `base`, 0.5 to 1.5).
fn weights(fx: Vec2) -> [Vec2; 3] {
    [0.5 * (1.5 - fx) * (1.5 - fx), 0.75 - (fx - 1.0) * (fx - 1.0), 0.5 * (fx - 0.5) * (fx - 0.5)]
}

pub struct Mpm {
    pub material: Material,
    pub params: MpmParams,
    h: f32,
    /// Nodes across and up (node (i, j) at (i, j) × h).
    gx: usize,
    gy: usize,
    pos: Vec<Vec2>,
    vel: Vec<Vec2>,
    affine: Vec<Mat2>,
    deformation: Vec<Mat2>,
    /// Snow: plastic volume ratio.
    plastic: Vec<f32>,
    /// The grid: momentum then velocity (xy) and mass (z), per node.
    grid: Vec<glam::Vec3>,
    /// Per node: the wall's outward normal there, if the node is in a wall.
    walls: Vec<Option<Vec2>>,
    walls_generation: Option<u64>,
    emission: super::Emission,
    stats: Stats,
}

impl Mpm {
    pub fn new(material: Material) -> Self {
        Self {
            material,
            params: MpmParams::for_material(material),
            h: 1.0,
            gx: 0,
            gy: 0,
            pos: Vec::new(),
            vel: Vec::new(),
            affine: Vec::new(),
            deformation: Vec::new(),
            plastic: Vec::new(),
            grid: Vec::new(),
            walls: Vec::new(),
            walls_generation: None,
            emission: super::Emission::default(),
            stats: Stats::default(),
        }
    }

    fn spacing(&self) -> f32 {
        self.h / PER_CELL
    }

    fn radius(&self) -> f32 {
        0.5 * self.spacing()
    }

    /// The fastest a sound wave runs in the material, m/s.
    fn sound_speed(&self) -> f32 {
        let (mu, lambda) = self.params.lame();
        let hardening = if self.material == Material::Snow {
            // Packed snow is stiffer; allow for the hardest particle.
            self.plastic.iter().fold(1.0f32, |m, &jp| m.max((self.params.hardening * (1.0 - jp)).exp())).min(HARDEN_CAP)
        } else {
            1.0
        };
        ((lambda + 2.0 * mu) * hardening / DENSITY).sqrt()
    }

    /// The wall normal at each node, when the solid mask has changed.
    fn update_walls(&mut self, scene: &Scene) {
        let generation = scene.solid.generation();
        if self.walls_generation == Some(generation) {
            return;
        }
        let (gx, h) = (self.gx, self.h);
        self.walls = (0..gx * self.gy)
            .map(|k| {
                let p = Vec2::new((k % gx) as f32, (k / gx) as f32) * h;
                (scene.solid.distance(p) < 0.25 * h).then(|| scene.solid.normal(p))
            })
            .collect();
        self.walls_generation = Some(generation);
    }

    /// Remove particles in drains, from every per-particle array. Returns how many.
    fn drain(&mut self, scene: &Scene) -> usize {
        if scene.drains.is_empty() {
            return 0;
        }
        let before = self.pos.len();
        let mut i = 0;
        while i < self.pos.len() {
            if scene.drains.iter().any(|d| d.contains(self.pos[i])) {
                self.pos.swap_remove(i);
                self.vel.swap_remove(i);
                self.affine.swap_remove(i);
                self.deformation.swap_remove(i);
                self.plastic.swap_remove(i);
            } else {
                i += 1;
            }
        }
        before - self.pos.len()
    }

    /// Kirchhoff stress τ for deformation `f` (and snow's plastic ratio `jp`): fixed
    /// corotated for jelly and snow, Hencky (log strain) for sand.
    fn stress(&self, f: Mat2, jp: f32) -> Mat2 {
        let (mut mu, mut lambda) = self.params.lame();
        match self.material {
            Material::Sand => {
                let (u, sigma, _) = svd(f);
                let eps = Vec2::new(sigma.x.abs().max(1e-4).ln(), sigma.y.abs().max(1e-4).ln());
                let tau = 2.0 * mu * eps + Vec2::splat(lambda * (eps.x + eps.y));
                u * diag(tau) * u.transpose()
            }
            _ => {
                if self.material == Material::Snow {
                    let harden = (self.params.hardening * (1.0 - jp)).exp().min(HARDEN_CAP);
                    (mu, lambda) = (mu * harden, lambda * harden);
                }
                let (u, _, v) = svd(f);
                let r = u * v.transpose();
                let j = f.determinant();
                2.0 * mu * (f - r) * f.transpose() + Mat2::from_diagonal(Vec2::splat(lambda * j * (j - 1.0)))
            }
        }
    }

    /// Plasticity: snow's singular values clamped to its critical compression and
    /// stretch (what's clamped off goes into Jp); sand's log strain returned to the
    /// Drucker-Prager cone (pulled apart, it lets go entirely).
    fn plasticity(material: Material, params: &MpmParams, lame: (f32, f32), f: &mut Mat2, jp: &mut f32) {
        match material {
            Material::Jelly => {}
            Material::Snow => {
                let (u, sigma, v) = svd(*f);
                let clamped = sigma.clamp(Vec2::splat(1.0 - params.compression), Vec2::splat(1.0 + params.stretch));
                *jp = (*jp * (sigma.x * sigma.y) / (clamped.x * clamped.y)).clamp(0.6, 20.0);
                *f = u * diag(clamped) * v.transpose();
            }
            Material::Sand => {
                let (mu, lambda) = lame;
                let (u, sigma, v) = svd(*f);
                let eps = Vec2::new(sigma.x.abs().max(1e-4).ln(), sigma.y.abs().max(1e-4).ln());
                let trace = eps.x + eps.y;
                let deviatoric = eps - Vec2::splat(0.5 * trace);
                let norm = deviatoric.length();
                let new_eps = if trace >= 0.0 || norm < 1e-8 {
                    // Pulled apart (or evenly squeezed): no shear to resist.
                    if trace >= 0.0 { Vec2::ZERO } else { eps }
                } else {
                    let sin = params.friction_angle.to_radians().sin();
                    let alpha = (2.0f32 / 3.0).sqrt() * 2.0 * sin / (3.0 - sin);
                    let excess = norm + (2.0 * lambda + 2.0 * mu) / (2.0 * mu) * trace * alpha;
                    if excess <= 0.0 { eps } else { eps - excess * deviatoric / norm }
                };
                *f = u * diag(Vec2::new(new_eps.x.exp(), new_eps.y.exp())) * v.transpose();
            }
        }
    }

    /// Particles to grid: mass, momentum and stress, summed over per-thread grids.
    fn particles_to_grid(&mut self, dt: f32) {
        let (h, gx, gy) = (self.h, self.gx, self.gy);
        let inv_h = 1.0 / h;
        let volume = self.spacing() * self.spacing();
        let mass = volume * DENSITY;
        let this = &*self;
        let sum = (0..self.pos.len())
            .into_par_iter()
            .with_min_len(PAR_CHUNK)
            .fold(
                || vec![glam::Vec3::ZERO; gx * gy],
                |mut grid, p| {
                    let x = this.pos[p];
                    let base = (x * inv_h - 0.5).floor();
                    let fx = x * inv_h - base;
                    let w = weights(fx);
                    let stress = this.stress(this.deformation[p], this.plastic[p]);
                    let affine = -dt * volume * 4.0 * inv_h * inv_h * stress + mass * this.affine[p];
                    let (bi, bj) = (base.x as isize, base.y as isize);
                    for dj in 0..3 {
                        for di in 0..3 {
                            let (i, j) = (bi + di as isize, bj + dj as isize);
                            if i < 0 || j < 0 || i as usize >= gx || j as usize >= gy {
                                continue;
                            }
                            let weight = w[di].x * w[dj].y;
                            let dpos = (Vec2::new(di as f32, dj as f32) - fx) * h;
                            let momentum = weight * (mass * this.vel[p] + affine * dpos);
                            grid[j as usize * gx + i as usize] += momentum.extend(weight * mass);
                        }
                    }
                    grid
                },
            )
            .reduce(
                || vec![glam::Vec3::ZERO; gx * gy],
                |mut a, b| {
                    a.iter_mut().zip(&b).for_each(|(x, y)| *x += *y);
                    a
                },
            );
        self.grid = sum;
    }

    /// The grid's velocities: momentum over mass, gravity, brushes, then walls.
    fn update_grid(&mut self, scene: &Scene, input: &StepInput, dt: f32) {
        let (gx, h) = (self.gx, self.h);
        let g = scene.gravity;
        let sand = self.material == Material::Sand;
        let walls = &self.walls;
        self.grid.par_iter_mut().enumerate().with_min_len(1024).for_each(|(k, node)| {
            if node.z <= 0.0 {
                *node = glam::Vec3::ZERO;
                return;
            }
            let p = Vec2::new((k % gx) as f32, (k / gx) as f32) * h;
            let mut v = node.truncate() / node.z + g * dt;
            for b in &input.brushes {
                let d = p.distance(b.pos);
                if d < b.radius {
                    v += (b.target(p) - v) * (b.strength * (1.0 - d / b.radius) * dt).min(1.0);
                }
            }
            if let Some(n) = walls[k] {
                let vn = v.dot(n);
                if vn < 0.0 {
                    let tangent = v - vn * n;
                    v = if sand {
                        // Coulomb: the wall's push slows the sliding, down to a stop.
                        let t = tangent.length();
                        if t <= -WALL_FRICTION * vn { Vec2::ZERO } else { tangent * (1.0 + WALL_FRICTION * vn / t) }
                    } else {
                        tangent
                    };
                }
            }
            *node = v.extend(node.z);
        });
    }

    /// Grid to particles: velocity and C from the grid, F advanced, plasticity, moved.
    fn grid_to_particles(&mut self, dt: f32) {
        let (h, gx, gy) = (self.h, self.gx, self.gy);
        let inv_h = 1.0 / h;
        let (material, params, lame) = (self.material, &self.params, self.params.lame());
        let grid = &self.grid;
        self.pos
            .par_iter_mut()
            .zip(self.vel.par_iter_mut())
            .zip(self.affine.par_iter_mut())
            .zip(self.deformation.par_iter_mut().zip(self.plastic.par_iter_mut()))
            .with_min_len(PAR_CHUNK)
            .for_each(|(((x, v), c), (f, jp))| {
                let base = (*x * inv_h - 0.5).floor();
                let fx = *x * inv_h - base;
                let w = weights(fx);
                let (mut new_v, mut new_c) = (Vec2::ZERO, Mat2::ZERO);
                let (bi, bj) = (base.x as isize, base.y as isize);
                for dj in 0..3 {
                    for di in 0..3 {
                        let (i, j) = (bi + di as isize, bj + dj as isize);
                        if i < 0 || j < 0 || i as usize >= gx || j as usize >= gy {
                            continue;
                        }
                        let weight = w[di].x * w[dj].y;
                        let gv = grid[j as usize * gx + i as usize].truncate();
                        let dpos = (Vec2::new(di as f32, dj as f32) - fx) * h;
                        new_v += weight * gv;
                        new_c += 4.0 * inv_h * inv_h * weight * outer(gv, dpos);
                    }
                }
                (*v, *c) = (new_v, new_c);
                *f = (Mat2::IDENTITY + dt * new_c) * *f;
                Self::plasticity(material, params, lame, f, jp);
                if !f.is_finite() || f.determinant().abs() < 1e-3 {
                    (*f, *jp) = (Mat2::IDENTITY, 1.0);
                }
                *x += dt * new_v;
            });
    }
}

impl Solver for Mpm {
    fn name(&self) -> &'static str {
        match self.material {
            Material::Jelly => "MPM jelly",
            Material::Snow => "MPM snow",
            Material::Sand => "MPM sand",
        }
    }

    fn reset(&mut self, scene: &Scene, init: Init<'_>) {
        let ny = self.params.cells_y.max(8) as usize;
        self.h = scene.size.y / ny as f32;
        self.gx = (scene.size.x / self.h).round() as usize + 1;
        self.gy = ny + 1;
        self.walls_generation = None;
        self.update_walls(scene);
        match init {
            Init::FromScene => super::seed_particles(scene, self.spacing(), 0.3, &mut self.pos),
            Init::FromParticles(view) => {
                let mut vel = Vec::new();
                super::resample(view, self.spacing(), scene.size, &mut self.pos, &mut vel);
            }
        }
        let n = self.pos.len();
        self.vel = vec![Vec2::ZERO; n];
        self.affine = vec![Mat2::ZERO; n];
        self.deformation = vec![Mat2::IDENTITY; n];
        self.plastic = vec![1.0; n];
        self.emission.reset();
        self.stats = Stats { particles: n, ..Stats::default() };
    }

    fn max_dt(&self, _scene: &Scene) -> f32 {
        let fastest = self.vel.iter().map(|v| v.length_squared()).fold(0.0, f32::max).sqrt();
        self.params.cfl * self.h / (fastest + self.sound_speed()).max(1e-3)
    }

    fn max_substeps(&self) -> u32 {
        40
    }

    fn step(&mut self, dt: f32, scene: &Scene, input: &StepInput) {
        let mut timer = PhaseTimer::start();
        self.update_walls(scene);
        let drained = self.drain(scene);
        let before = self.pos.len();
        let (emitted, _) = self.emission.apply(scene, input, dt, self.spacing(), &mut self.pos, &mut self.vel);
        let n = self.pos.len();
        debug_assert_eq!(n, before + emitted);
        self.affine.resize(n, Mat2::ZERO);
        self.deformation.resize(n, Mat2::IDENTITY);
        self.plastic.resize(n, 1.0);
        self.stats.emitted += emitted;
        self.stats.drained += drained;
        self.stats.particles = n;
        timer.lap("emit");

        self.particles_to_grid(dt);
        timer.lap("particles to grid");
        self.update_grid(scene, input, dt);
        timer.lap("grid");
        self.grid_to_particles(dt);
        let interior = scene.solid.interior();
        super::collide_with_solids(scene, interior, self.radius(), 0.0, 1.0, &mut self.pos, &mut self.vel);
        timer.lap("grid to particles");

        self.stats.phases = timer.finish();
        let volume = self.deformation.iter().map(|f| f.determinant()).sum::<f32>() / n.max(1) as f32;
        self.stats.metrics = vec![Metric { name: "mean volume (det F)", value: volume, tolerance: None }];
    }

    fn visit_params(&mut self, v: &mut dyn ParamVisitor) {
        let p = &mut self.params;
        v.section("Material");
        v.float(&STIFFNESS, &mut p.stiffness);
        v.float(&POISSON, &mut p.poisson);
        match self.material {
            Material::Jelly => {}
            Material::Snow => {
                v.float(&HARDENING, &mut p.hardening);
                v.float(&COMPRESSION, &mut p.compression);
                v.float(&STRETCH, &mut p.stretch);
            }
            Material::Sand => v.float(&FRICTION_ANGLE, &mut p.friction_angle),
        }
        v.section("Resolution");
        v.int(&CELLS_Y, &mut p.cells_y);
        v.advanced_section("Solver");
        v.float(&CFL, &mut p.cfl);
    }

    fn particles(&self) -> Option<ParticleView<'_>> {
        Some(ParticleView { pos: &self.pos, vel: &self.vel, radius: self.radius() })
    }

    fn stats(&self) -> &Stats {
        &self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets;
    use crate::scene::Rect;
    use crate::stepper::Stepper;

    fn run(s: &mut Mpm, scene: &Scene, seconds: f32) {
        let stepper = Stepper::default();
        for _ in 0..(seconds * 60.0) as usize {
            stepper.advance(s, scene, 1.0 / 60.0, &StepInput::default());
        }
    }

    /// A block of the material, standing on the floor of a tank.
    fn block(material: Material) -> (Mpm, Scene) {
        let mut scene = Scene::empty_box(presets::DEFAULT_DOMAIN, 128);
        let floor = scene.solid.interior().min.y;
        scene.fluid_blocks.push(Rect::new(Vec2::new(0.8, floor), Vec2::new(1.2, floor + 0.4)));
        let mut s = Mpm::new(material);
        s.params.cells_y = 48;
        s.reset(&scene, Init::FromScene);
        (s, scene)
    }

    fn height(s: &Mpm) -> f32 {
        s.pos.iter().map(|p| p.y).fold(0.0, f32::max)
    }

    fn width(s: &Mpm) -> f32 {
        let (lo, hi) = s.pos.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p.x), hi.max(p.x)));
        hi - lo
    }

    #[test]
    fn svd_rebuilds_the_matrix() {
        for f in [Mat2::from_cols_array(&[1.2, 0.3, -0.4, 0.9]), Mat2::from_cols_array(&[0.7, 0.0, 0.0, 1.4]), Mat2::from_angle(0.8) * 1.1] {
            let (u, s, v) = svd(f);
            let back = u * diag(s) * v.transpose();
            assert!((back - f).abs_diff_eq(Mat2::ZERO, 1e-5), "{f} -> {back}");
            assert!((u.determinant() - 1.0).abs() < 1e-5 && (v.determinant() - 1.0).abs() < 1e-5);
        }
    }

    /// Jelly stands: a block on the floor settles a little under its weight, then holds
    /// its height and width.
    #[test]
    fn jelly_stands_and_wobbles() {
        let (mut s, scene) = block(Material::Jelly);
        let top = height(&s);
        run(&mut s, &scene, 2.0);
        let settled = height(&s);
        assert!(settled > top - 0.12 && settled < top + 0.01, "top {top} -> {settled}");
        assert!(width(&s) < 0.55, "it spread like a liquid: {}", width(&s));
    }

    /// Sand slumps into a pile: lower and wider, but not flat.
    #[test]
    fn sand_piles() {
        let (mut s, scene) = block(Material::Sand);
        let top = height(&s);
        run(&mut s, &scene, 3.0);
        let floor = scene.solid.interior().min.y;
        assert!(height(&s) < top - 0.05, "it stood like a solid: {top} -> {}", height(&s));
        assert!(width(&s) > 0.45, "it didn't spread: {}", width(&s));
        assert!(height(&s) - floor > 0.08, "it ran flat like a liquid: {}", height(&s) - floor);
    }

    /// Snow dropped on the floor packs: its particles end closer together than they
    /// started (Jp below 1), and it doesn't bounce back up.
    #[test]
    fn snow_packs() {
        let mut scene = Scene::empty_box(presets::DEFAULT_DOMAIN, 128);
        scene.fluid_blocks.push(Rect::new(Vec2::new(0.8, 0.5), Vec2::new(1.2, 0.8)));
        let mut s = Mpm::new(Material::Snow);
        s.params.cells_y = 48;
        s.reset(&scene, Init::FromScene);
        run(&mut s, &scene, 2.0);
        let packed = s.plastic.iter().filter(|&&jp| jp < 0.98).count();
        assert!(packed > s.pos.len() / 10, "only {packed} of {} packed", s.pos.len());
        assert!(height(&s) < 0.5, "it bounced: {}", height(&s));
    }

    /// Every material keeps its particles finite, in the domain and out of walls,
    /// through a dam break and the tank's obstacle.
    #[test]
    fn every_material_stays_sound() {
        for material in [Material::Jelly, Material::Snow, Material::Sand] {
            for scene in [presets::dam_break(), presets::tank_with_obstacle()] {
                let mut s = Mpm::new(material);
                s.params.cells_y = 48;
                s.reset(&scene, Init::FromScene);
                run(&mut s, &scene, 1.5);
                assert!(s.pos.iter().chain(&s.vel).all(|v| v.is_finite()), "{material:?}: not finite");
                assert!(s.pos.iter().all(|p| p.cmpge(Vec2::ZERO).all() && p.cmple(scene.size).all()), "{material:?}: escaped");
                assert!(s.vel.iter().all(|v| v.length() < 20.0), "{material:?}: blew up");
            }
        }
    }
}
