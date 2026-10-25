//! FLIP: particles carry velocity, a staggered MAC grid enforces incompressibility.
//!
//! Per substep (Bridson ch. 7; structure after Ten Minute Physics #18):
//! classify cells -> P2G -> gravity on the grid -> solid boundaries -> pressure
//! projection -> G2P (PIC/FLIP blend) -> advect particles through the grid velocity ->
//! separate particles -> collide.
//!
//! Forces go on the grid and particles move only *after* projection. Moving particles
//! with `v + g dt` before projecting (as TMP does) sinks a resting fluid by g dt^2 every
//! substep; TMP hides that with particle separation and drift compensation.

mod collide;
mod separate;
mod transfer;

use crate::grid::Grid2;
use crate::grid::mac::MacGrid;
use crate::neighbors::CellBins;
use crate::params::{FloatSpec, IntSpec, ParamVisitor, ToggleSpec};
use crate::scene::{Scene, StepInput};
use crate::grid::mac::CellType;
use crate::solver::{FieldInfo, FieldKind, FieldLayout, Init, Metric, ParticleView, Solver, Stats};
use crate::timing::PhaseTimer;
use glam::Vec2;
use super::pressure::{self, Drift, Method, PressureSolver};
use transfer::P2gScratch;
use rayon::prelude::*;

#[derive(Clone, Debug)]
pub struct FlipParams {
    /// Grid resolution along the domain height (changing it requires a reset).
    pub cells_y: u32,
    /// 0 = PIC (smooth, viscous), 1 = FLIP (lively, noisy).
    pub flip_ratio: f32,
    /// Gauss-Seidel iterations, or the maximum for conjugate gradients.
    pub pressure_iters: u32,
    pub over_relax: f32,
    /// Conjugate gradients instead of Gauss-Seidel.
    pub use_pcg: bool,
    /// PCG stops when the largest unbalanced flux falls below this (m/s).
    pub pcg_tolerance: f32,
    /// Precondition CG with a multigrid V-cycle (MGPCG) instead of Jacobi.
    pub pcg_multigrid: bool,
    pub compensate_drift: bool,
    /// Fraction of a cell's excess particle density removed per substep.
    pub drift_stiffness: f32,
    /// Max particle travel per substep, in grid cells.
    pub cfl: f32,
    pub separate_particles: bool,
    pub separation_iters: u32,
}

impl Default for FlipParams {
    fn default() -> Self {
        Self {
            cells_y: 100,
            flip_ratio: 0.9,
            pressure_iters: 50,
            over_relax: 1.9,
            // Measured: warm-started Gauss-Seidel beats Jacobi-PCG ~3x and multigrid-PCG
            // ~1.5x here: FLIP's p >= 0 active set re-solves several times per substep,
            // and warm starts leave little smooth error for multigrid (see docs/PLAN.md).
            use_pcg: false,
            pcg_tolerance: 0.01,
            pcg_multigrid: true,
            // Separation alone keeps volume within a few percent over 10 s of dam break;
            // drift compensation on top mostly adds restlessness.
            compensate_drift: false,
            drift_stiffness: 0.05,
            cfl: 2.0,
            separate_particles: true,
            separation_iters: 2,
        }
    }
}

const CELLS_Y: IntSpec = IntSpec {
    key: "cells_y",
    label: "Grid cells (height)",
    range: 16..=200,
    restart: true,
    help: "Resolution. Particle count grows with its square (4 per cell).",
};
const FLIP_RATIO: FloatSpec = FloatSpec {
    key: "flip_ratio",
    label: "PIC/FLIP blend",
    range: 0.0..=1.0,
    log: false,
    restart: false,
    help: "0 = PIC (smooth, viscous), 1 = FLIP (lively, noisy).",
};
const PRESSURE_ITERS: IntSpec = IntSpec {
    key: "pressure_iters",
    label: "Pressure iterations",
    range: 1..=500,
    restart: false,
    help: "Gauss-Seidel sweeps per substep (warm-started, so 30-50 is usually plenty).",
};
const USE_PCG: ToggleSpec = ToggleSpec {
    key: "use_pcg",
    label: "Conjugate gradients",
    help: "Preconditioned CG to a tolerance instead of fixed Gauss-Seidel sweeps. Far faster from a cold start, but warm-started Gauss-Seidel wins here (measured 1.5-3x).",
};
const PCG_MULTIGRID: ToggleSpec = ToggleSpec {
    key: "pcg_multigrid",
    label: "Multigrid preconditioner",
    help: "Precondition CG with a multigrid V-cycle (MGPCG) instead of Jacobi: far fewer iterations, each costlier.",
};
const PCG_TOLERANCE: FloatSpec = FloatSpec {
    key: "pcg_tolerance",
    label: "CG tolerance (m/s)",
    range: 0.0005..=0.1,
    log: true,
    restart: false,
    help: "CG stops when the largest unbalanced flux in any fluid cell is below this.",
};
const OVER_RELAX: FloatSpec = FloatSpec {
    key: "over_relax",
    label: "Over-relaxation",
    range: 1.0..=1.98,
    log: false,
    restart: false,
    help: "SOR factor. Higher converges faster until it doesn't.",
};
const CFL: FloatSpec = FloatSpec {
    key: "cfl",
    label: "CFL (cells per substep)",
    range: 0.25..=4.0,
    log: true,
    restart: false,
    help: "Max particle travel per substep. Lower = more substeps, more stable.",
};
const SEPARATE: ToggleSpec = ToggleSpec {
    key: "separate_particles",
    label: "Separate particles",
    help: "Push overlapping particles apart. Keeps volume; without it FLIP compacts.",
};
const SEPARATION_ITERS: IntSpec = IntSpec {
    key: "separation_iters",
    label: "Separation passes",
    range: 1..=8,
    restart: false,
    help: "Relaxation passes per substep.",
};
const DRIFT: ToggleSpec = ToggleSpec {
    key: "compensate_drift",
    label: "Drift compensation",
    help: "Extra outflow from over-dense cells. Mostly redundant with separation.",
};
const DRIFT_STIFFNESS: FloatSpec = FloatSpec {
    key: "drift_stiffness",
    label: "Drift stiffness",
    range: 0.0..=1.0,
    log: false,
    restart: false,
    help: "Fraction of excess density removed per substep.",
};

/// Invariant bound on the pressure residual (unbalanced flux, m/s). Normal runs peak
/// around 0.1 at the default 50 iterations, at any resolution; a skipped or broken
/// projection gives metres per second in any splash.
const MAX_PRESSURE_RESIDUAL: f32 = 0.3;

/// Water density, for reporting pressure in pascals.
const RHO: f32 = 1000.0;

const FIELDS: &[FieldInfo] = &[
    FieldInfo { key: "pressure", label: "Pressure", unit: "Pa", kind: FieldKind::NonNegative },
    FieldInfo { key: "divergence", label: "Divergence before projection", unit: "1/s", kind: FieldKind::Signed },
    FieldInfo { key: "speed", label: "Speed", unit: "m/s", kind: FieldKind::NonNegative },
    FieldInfo { key: "density", label: "Particles per cell", unit: "", kind: FieldKind::NonNegative },
    FieldInfo { key: "cells", label: "Cell type (air / fluid / solid)", unit: "", kind: FieldKind::Categorical },
];

/// Particles are seeded on a jittered 2x2 sub-lattice per cell, aligned with the grid.
/// A lattice whose spacing doesn't divide the cell size aliases against it, and the
/// resulting beat pattern in the density field looks like compression to the drift
/// correction.
const PARTICLES_PER_AXIS: usize = 2;
const SEED_JITTER: f32 = 0.25;
/// Radius at which neighbouring particles at rest just touch.
const RADIUS_PER_CELL: f32 = 0.5 / PARTICLES_PER_AXIS as f32;

pub struct Flip {
    pub params: FlipParams,
    grid: MacGrid,
    u_prev: Grid2<f32>,
    v_prev: Grid2<f32>,
    /// Persistent pressure (per unit time) for warm-starting the solver.
    pressure: Grid2<f32>,
    pressure_solver: PressureSolver,
    p2g: P2gScratch,
    /// Separation scratch: per-particle displacement.
    shift: Vec<Vec2>,
    density: Grid2<f32>,
    /// Scene solids resampled to this grid; rebuilt when the mask's generation changes.
    solid: Grid2<bool>,
    solid_generation: Option<u64>,
    rest_density: f32,
    pos: Vec<Vec2>,
    vel: Vec<Vec2>,
    /// Per-particle grid (PIC) velocity from the last G2P, used to advect.
    advect: Vec<Vec2>,
    radius: f32,
    bins: CellBins,
    emission: super::Emission,
    stats: Stats,
}

impl Default for Flip {
    fn default() -> Self {
        Self::with_params(FlipParams::default())
    }
}

impl Flip {
    pub fn with_params(params: FlipParams) -> Self {
        // Placeholder 2x2 grid; `reset` sizes everything from the scene.
        let grid = MacGrid::new(2, 2, 1.0);
        Self {
            params,
            u_prev: Grid2::new(3, 2),
            v_prev: Grid2::new(2, 3),
            pressure: Grid2::new(2, 2),
            pressure_solver: PressureSolver::new(2, 2),
            p2g: P2gScratch::default(),
            shift: Vec::new(),
            density: Grid2::new(2, 2),
            solid: Grid2::new(2, 2),
            grid,
            solid_generation: None,
            rest_density: 0.0,
            pos: Vec::new(),
            vel: Vec::new(),
            advect: Vec::new(),
            radius: 0.0,
            bins: CellBins::default(),
            emission: super::Emission::default(),
            stats: Stats::default(),
        }
    }

    fn resize_grid(&mut self, scene: &Scene) {
        let ny = self.params.cells_y.max(4) as usize;
        let h = scene.size.y / ny as f32;
        let nx = ((scene.size.x / h).round() as usize).max(4);
        self.grid = MacGrid::new(nx, ny, h);
        self.u_prev = Grid2::new(nx + 1, ny);
        self.v_prev = Grid2::new(nx, ny + 1);
        self.pressure = Grid2::new(nx, ny);
        self.pressure_solver = PressureSolver::new(nx, ny);
        self.density = Grid2::new(nx, ny);
        self.solid = Grid2::new(nx, ny);
        self.solid_generation = None;
        self.radius = RADIUS_PER_CELL * h;
    }

    /// A cell is solid if its centre is inside a scene solid. Particles collide with
    /// these cells' faces (see `collide`), so particles and pressure agree on the walls.
    /// The outer ring is always solid so the domain stays closed.
    fn refresh_solids(&mut self, scene: &Scene) {
        if self.solid_generation == Some(scene.solid.generation()) {
            return;
        }
        super::resample_solids(scene, self.grid.h, &mut self.solid);
        self.solid_generation = Some(scene.solid.generation());
    }

    /// Fill the scene's fluid blocks with `PARTICLES_PER_AXIS^2` jittered particles per
    /// open cell. Must run after `refresh_solids`.
    fn seed(&mut self, scene: &Scene) {
        let mut rng = crate::rng::SplitMix64::new(0xF1F0);
        let (nx, ny, h) = (self.grid.nx, self.grid.ny, self.grid.h);
        let sub = h / PARTICLES_PER_AXIS as f32;
        self.pos.clear();
        for j in 0..ny {
            for i in 0..nx {
                if self.solid[(i, j)] {
                    continue;
                }
                for sj in 0..PARTICLES_PER_AXIS {
                    for si in 0..PARTICLES_PER_AXIS {
                        let lattice = Vec2::new(i as f32, j as f32) * h + (Vec2::new(si as f32, sj as f32) + 0.5) * sub;
                        let p = lattice + Vec2::new(rng.signed(), rng.signed()) * SEED_JITTER * sub;
                        // Every open cell is fluid territory, even where the scene's
                        // smooth wall cuts through it; `collide` enforces the same rule.
                        if scene.fluid_blocks.iter().any(|b| b.contains(p)) {
                            self.pos.push(p);
                        }
                    }
                }
            }
        }
        self.vel = vec![Vec2::ZERO; self.pos.len()];
    }
}

impl Solver for Flip {
    fn name(&self) -> &'static str {
        "FLIP"
    }

    fn reset(&mut self, scene: &Scene, init: Init<'_>) {
        self.resize_grid(scene);
        self.refresh_solids(scene);
        match init {
            Init::FromScene => self.seed(scene),
            Init::FromParticles(view) => {
                super::resample(view, 2.0 * self.radius, scene.size, &mut self.pos, &mut self.vel);
            }
        }
        // Interior density of the seeding lattice, in particles per cell.
        self.rest_density = (PARTICLES_PER_AXIS * PARTICLES_PER_AXIS) as f32;
        self.advect = vec![Vec2::ZERO; self.pos.len()];
        self.emission.reset();
        self.stats = Stats { particles: self.pos.len(), ..Stats::default() };
    }

    fn max_dt(&self, _scene: &Scene) -> f32 {
        let max_speed = self.vel.iter().map(|v| v.length_squared()).fold(0.0, f32::max).sqrt();
        self.params.cfl * self.grid.h / max_speed.max(1e-3)
    }

    fn step(&mut self, dt: f32, scene: &Scene, input: &StepInput) {
        let mut timer = PhaseTimer::start();
        self.refresh_solids(scene);
        let (emitted, drained) = self.emission.apply(scene, input, dt, 2.0 * self.radius, &mut self.pos, &mut self.vel);
        self.stats.emitted += emitted;
        self.stats.drained += drained;
        self.stats.particles = self.pos.len();
        self.advect.resize(self.pos.len(), Vec2::ZERO);
        super::apply_brushes(&input.brushes, dt, &self.pos, &mut self.vel);

        transfer::mark_cells(&mut self.grid, &self.solid, &self.pos);
        timer.lap("classify");
        transfer::particles_to_grid(&mut self.grid, &mut self.p2g, &self.pos, &self.vel);
        self.grid.zero_solid_faces();
        self.u_prev.copy_from(&self.grid.u);
        self.v_prev.copy_from(&self.grid.v);
        timer.lap("p2g");

        transfer::apply_gravity(&mut self.grid, scene.gravity, dt);
        self.grid.zero_solid_faces();
        let drift = self.params.compensate_drift.then(|| {
            transfer::particle_density(&mut self.density, self.grid.h, &self.pos);
            Drift { gain: self.params.drift_stiffness * self.grid.h / dt, rest: self.rest_density }
        });
        timer.lap("forces");

        let p = &self.params;
        let method = if p.use_pcg {
            Method::Pcg {
                max_iterations: p.pressure_iters,
                tolerance: p.pcg_tolerance,
                non_negative: true,
                multigrid: p.pcg_multigrid,
            }
        } else {
            Method::GaussSeidel { iterations: p.pressure_iters, over_relax: p.over_relax, non_negative: true }
        };
        let pressure_iterations =
            pressure::project(&mut self.grid, &mut self.pressure_solver, &mut self.pressure, &self.density, dt, method, drift);
        let residual = pressure::max_residual(&self.grid, &self.pressure, true);
        timer.lap("pressure");

        transfer::grid_to_particles(
            &self.grid,
            &self.u_prev,
            &self.v_prev,
            &self.pos,
            &mut self.vel,
            &mut self.advect,
            self.params.flip_ratio,
        );
        timer.lap("g2p");

        self.pos.par_iter_mut().zip(&self.advect).with_min_len(super::PAR_CHUNK).for_each(|(p, a)| *p += *a * dt);
        timer.lap("advect");

        if self.params.separate_particles {
            separate::push_apart(
                &mut self.bins,
                &mut self.pos,
                &mut self.shift,
                self.radius,
                self.params.separation_iters,
                scene.size,
            );
        }
        timer.lap("separate");

        collide::collide(&self.solid, self.grid.h, self.radius, scene, &mut self.pos, &mut self.vel);
        timer.lap("collide");

        self.stats.phases = timer.finish();
        self.stats.metrics = vec![Metric {
            name: "pressure residual (m/s)",
            value: residual,
            tolerance: Some(MAX_PRESSURE_RESIDUAL),
        }, Metric { name: "pressure iterations", value: pressure_iterations as f32, tolerance: None }];
    }

    fn visit_params(&mut self, v: &mut dyn ParamVisitor) {
        let p = &mut self.params;
        v.section("Fluid");
        v.float(&FLIP_RATIO, &mut p.flip_ratio);
        v.section("Resolution");
        v.int(&CELLS_Y, &mut p.cells_y);
        v.advanced_section("Pressure solver");
        v.toggle(&USE_PCG, &mut p.use_pcg);
        v.toggle(&PCG_MULTIGRID, &mut p.pcg_multigrid);
        v.int(&PRESSURE_ITERS, &mut p.pressure_iters);
        v.float(&OVER_RELAX, &mut p.over_relax);
        v.float(&PCG_TOLERANCE, &mut p.pcg_tolerance);
        v.advanced_section("Stepping and particles");
        v.float(&CFL, &mut p.cfl);
        v.toggle(&SEPARATE, &mut p.separate_particles);
        v.int(&SEPARATION_ITERS, &mut p.separation_iters);
        v.toggle(&DRIFT, &mut p.compensate_drift);
        v.float(&DRIFT_STIFFNESS, &mut p.drift_stiffness);
    }

    fn particles(&self) -> Option<ParticleView<'_>> {
        Some(ParticleView { pos: &self.pos, vel: &self.vel, radius: self.radius })
    }

    fn fields(&self) -> &'static [FieldInfo] {
        FIELDS
    }

    fn fill_field(&self, index: usize, out: &mut Vec<f32>) -> Option<FieldLayout> {
        let g = &self.grid;
        let (nx, ny, h) = (g.nx, g.ny, g.h);
        out.clear();
        out.resize(nx * ny, f32::NAN);
        let fluid = |i: usize, j: usize| g.cell[(i, j)] == CellType::Fluid;
        let mut each_fluid = |f: &dyn Fn(usize, usize) -> f32| {
            for j in 0..ny {
                for i in 0..nx {
                    if fluid(i, j) {
                        out[j * nx + i] = f(i, j);
                    }
                }
            }
        };
        match FIELDS.get(index)?.key {
            // Stored pressure is a velocity change per unit time; Î”u = Î”t Î”p / (Ï h).
            "pressure" => each_fluid(&|i, j| self.pressure[(i, j)] * RHO * h),
            "divergence" => each_fluid(&|i, j| -self.pressure_solver.rhs()[(i, j)] / h),
            "speed" => each_fluid(&|i, j| {
                let u = 0.5 * (g.u[(i, j)] + g.u[(i + 1, j)]);
                let v = 0.5 * (g.v[(i, j)] + g.v[(i, j + 1)]);
                Vec2::new(u, v).length()
            }),
            "density" => {
                let mut density = Grid2::new(nx, ny);
                transfer::particle_density(&mut density, h, &self.pos);
                for (o, &d) in out.iter_mut().zip(density.as_slice()) {
                    *o = if d > 0.05 { d } else { f32::NAN };
                }
            }
            _ => {
                for (o, c) in out.iter_mut().zip(g.cell.as_slice()) {
                    *o = match c {
                        CellType::Air => 0.0,
                        CellType::Fluid => 1.0,
                        CellType::Solid => 2.0,
                    };
                }
            }
        }
        Some(FieldLayout { nx, ny, origin: Vec2::ZERO, cell: h })
    }

    fn stats(&self) -> &Stats {
        &self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::mac::CellType;
    use crate::rng::SplitMix64;
    use crate::scene::Rect;
    use crate::stepper::Stepper;

    fn closed_box_grid(nx: usize, ny: usize, h: f32) -> (MacGrid, Grid2<bool>) {
        let grid = MacGrid::new(nx, ny, h);
        let mut solid = Grid2::new(nx, ny);
        for j in 0..ny {
            for i in 0..nx {
                solid[(i, j)] = i == 0 || j == 0 || i == nx - 1 || j == ny - 1;
            }
        }
        (grid, solid)
    }

    /// P2G followed by pure-PIC G2P must reproduce an affine velocity field exactly for
    /// particles whose stencils only touch faces with particles on both sides. Any
    /// off-by-half error in the staggering breaks this.
    #[test]
    fn affine_field_survives_p2g_g2p() {
        let (nx, ny, h) = (14, 12, 0.1);
        let (mut grid, solid) = closed_box_grid(nx, ny, h);
        let field = |p: Vec2| Vec2::new(0.3 + 0.5 * p.x - 0.2 * p.y, -0.1 + 0.4 * p.x + 0.7 * p.y);

        // Four particles per cell at the quarter points of every non-solid cell.
        let mut pos = Vec::new();
        for j in 2..2 * (ny - 1) {
            for i in 2..2 * (nx - 1) {
                pos.push((Vec2::new(i as f32, j as f32) + 0.5) * (h / 2.0));
            }
        }
        let original: Vec<Vec2> = pos.iter().map(|&p| field(p)).collect();
        let mut vel = original.clone();

        transfer::mark_cells(&mut grid, &solid, &pos);
        transfer::particles_to_grid(&mut grid, &mut P2gScratch::default(), &pos, &vel);
        let (u_prev, v_prev) = (grid.u.clone(), grid.v.clone());
        let mut advect = vec![Vec2::ZERO; pos.len()];
        transfer::grid_to_particles(&grid, &u_prev, &v_prev, &pos, &mut vel, &mut advect, 0.0);

        let interior = |p: Vec2| (3.0 * h..(nx as f32 - 3.0) * h).contains(&p.x) && (3.0 * h..(ny as f32 - 3.0) * h).contains(&p.y);
        let mut checked = 0;
        for ((p, v), v0) in pos.iter().zip(&vel).zip(&original) {
            if interior(*p) {
                assert!((*v - *v0).length() < 1e-4, "at {p}: got {v}, expected {v0}");
                checked += 1;
            }
        }
        assert!(checked > 100);
    }

    #[test]
    fn projection_removes_divergence() {
        let (nx, ny) = (24, 24);
        let (mut grid, solid) = closed_box_grid(nx, ny, 1.0 / ny as f32);
        for (c, &s) in grid.cell.as_mut_slice().iter_mut().zip(solid.as_slice()) {
            *c = if s { CellType::Solid } else { CellType::Fluid };
        }
        let mut rng = SplitMix64::new(7);
        grid.u.as_mut_slice().iter_mut().for_each(|x| *x = rng.signed());
        grid.v.as_mut_slice().iter_mut().for_each(|x| *x = rng.signed());
        grid.zero_solid_faces();

        let mut p = Grid2::new(nx, ny);
        let before = pressure::max_residual(&grid, &p, true);
        let method = Method::GaussSeidel { iterations: 300, over_relax: 1.9, non_negative: true };
        pressure::project(&mut grid, &mut PressureSolver::new(nx, ny), &mut p, &Grid2::new(nx, ny), 1.0, method, None);
        let after = pressure::max_residual(&grid, &p, true);
        assert!(after < before * 1e-3, "residual {before} -> {after}");
    }

    /// A tank of fluid at rest should stay at rest, and keep its volume. Exposes sign
    /// errors in the pressure solve and leaky boundaries.
    #[test]
    fn hydrostatic_tank_stays_calm() {
        let mut scene = Scene::empty_box(Vec2::new(1.0, 1.0), 64);
        scene.fluid_blocks.push(Rect::new(Vec2::ZERO, Vec2::new(1.0, 0.4)));
        let mut flip = Flip::with_params(FlipParams { cells_y: 40, ..FlipParams::default() });
        flip.reset(&scene, Init::FromScene);
        let mean_y = |f: &Flip| f.pos.iter().map(|p| p.y).sum::<f32>() / f.pos.len() as f32;
        let y0 = mean_y(&flip);

        let stepper = Stepper::default();
        for _ in 0..120 {
            stepper.advance(&mut flip, &scene, 1.0 / 60.0, &StepInput::default());
        }
        let max_speed = flip.vel.iter().map(|v| v.length()).fold(0.0, f32::max);
        let y1 = mean_y(&flip);
        assert!(max_speed < 0.1, "max speed {max_speed} m/s after 2 s at rest");
        assert!((y1 - y0).abs() / y0 < 0.02, "mean height {y0} -> {y1}");
    }

    /// The parallel P2G must agree with a plain serial splat (to rounding), with enough
    /// particles and threads that it really runs as several chunks.
    #[test]
    fn parallel_p2g_matches_serial() {
        let (nx, ny, h) = (60, 40, 0.025);
        let mut rng = SplitMix64::new(11);
        let n = 50_000;
        let pos: Vec<Vec2> = (0..n).map(|_| Vec2::new(0.03 + rng.next_f32() * 1.4, 0.03 + rng.next_f32() * 0.9)).collect();
        let vel: Vec<Vec2> = (0..n).map(|_| Vec2::new(rng.signed(), rng.signed())).collect();
        let mut serial = MacGrid::new(nx, ny, h);
        transfer::particles_to_grid_serial(&mut serial, &pos, &vel);
        let pool = rayon::ThreadPoolBuilder::new().num_threads(8).build().unwrap();
        let mut parallel = MacGrid::new(nx, ny, h);
        pool.install(|| transfer::particles_to_grid(&mut parallel, &mut P2gScratch::default(), &pos, &vel));
        for (a, b) in serial.u.as_slice().iter().chain(serial.v.as_slice()).zip(parallel.u.as_slice().iter().chain(parallel.v.as_slice())) {
            assert!((a - b).abs() <= 1e-5 * a.abs().max(1.0), "serial {a} vs parallel {b}");
        }
    }

    /// The residual invariant must actually catch a broken solver: with the projection
    /// switched off, a dam break exceeds the tolerance within a few frames.
    #[test]
    fn residual_tolerance_flags_a_missing_projection() {
        let scene = crate::presets::dam_break();
        let mut flip = Flip::with_params(FlipParams { cells_y: 50, pressure_iters: 0, ..FlipParams::default() });
        flip.reset(&scene, Init::FromScene);
        let stepper = Stepper::default();
        let mut worst = 0.0f32;
        for _ in 0..30 {
            stepper.advance(&mut flip, &scene, 1.0 / 60.0, &StepInput::default());
            worst = worst.max(flip.stats.metrics[0].value);
        }
        assert!(worst > MAX_PRESSURE_RESIDUAL, "residual only reached {worst} without projection");
    }

    fn calm_tank() -> (Scene, Flip) {
        calm_tank_with(FlipParams::default())
    }

    fn calm_tank_with(params: FlipParams) -> (Scene, Flip) {
        let mut scene = Scene::empty_box(Vec2::new(1.0, 1.0), 64);
        scene.fluid_blocks.push(Rect::new(Vec2::ZERO, Vec2::new(1.0, 0.5)));
        let mut flip = Flip::with_params(FlipParams { cells_y: 40, ..params });
        flip.reset(&scene, Init::FromScene);
        (scene, flip)
    }

    fn pcg_params() -> FlipParams {
        FlipParams { use_pcg: true, pcg_multigrid: false, pressure_iters: 200, ..FlipParams::default() }
    }

    /// A tank-like pressure system: fluid in the lower half of a closed box with air
    /// above, flowing down at 1 m/s plus noise, solid boundaries enforced.
    fn falling_tank_grid() -> MacGrid {
        let (nx, ny) = (96, 96);
        let (mut grid, solid) = closed_box_grid(nx, ny, 1.0 / ny as f32);
        for j in 0..ny {
            for i in 0..nx {
                grid.cell[(i, j)] = if solid[(i, j)] {
                    CellType::Solid
                } else if j < ny / 2 {
                    CellType::Fluid
                } else {
                    CellType::Air
                };
            }
        }
        let mut rng = SplitMix64::new(5);
        grid.u.as_mut_slice().iter_mut().for_each(|x| *x = 0.2 * rng.signed());
        grid.v.as_mut_slice().iter_mut().for_each(|x| *x = -1.0 + 0.2 * rng.signed());
        grid.zero_solid_faces();
        grid
    }

    fn solve(grid: &mut MacGrid, method: Method) -> (u32, f32) {
        let (nx, ny) = (grid.nx, grid.ny);
        let mut p = Grid2::new(nx, ny);
        let used = pressure::project(grid, &mut PressureSolver::new(nx, ny), &mut p, &Grid2::new(nx, ny), 1.0 / 60.0, method, None);
        (used, pressure::max_residual(grid, &p, true))
    }

    #[test]
    fn pcg_converges_to_its_tolerance() {
        let mut grid = falling_tank_grid();
        let (used, residual) = solve(&mut grid, Method::Pcg { max_iterations: 1000, tolerance: 1e-4, non_negative: true, multigrid: false });
        assert!(used < 1000, "PCG didn't converge in 1000 iterations");
        // f32 bottoms out around 2e-4 m/s for velocities of order 1 m/s (see the
        // convergence table: 200 and 400 iterations both stall there).
        assert!(residual < 1e-3, "residual {residual} m/s after {used} iterations");
    }

    /// The point of CG: the smooth, deep pressure modes that stall Gauss-Seidel are what
    /// CG resolves fastest. From a cold start, CG must reach Gauss-Seidel's 200-iteration
    /// residual in far fewer iterations.
    /// The point of CG: the smooth, deep pressure modes that stall Gauss-Seidel are what
    /// CG resolves fastest. From a cold start on a 48-cell-deep tank, CG must match
    /// Gauss-Seidel's 400-iteration residual in under half the iterations (measured:
    /// GS 0.011 m/s at 400; CG 0.005 at 100).
    #[test]
    fn pcg_needs_far_fewer_iterations_than_gauss_seidel() {
        let (_, gs_residual) = solve(&mut falling_tank_grid(), Method::GaussSeidel { iterations: 400, over_relax: 1.9, non_negative: true });
        let (cg_iterations, cg_residual) =
            solve(&mut falling_tank_grid(), Method::Pcg { max_iterations: 400, tolerance: gs_residual, non_negative: true, multigrid: false });
        assert!(cg_residual <= gs_residual * 1.01, "CG residual {cg_residual} vs GS {gs_residual}");
        assert!(cg_iterations < 200, "CG took {cg_iterations} iterations to match 400 of Gauss-Seidel ({gs_residual} m/s)");
    }

    /// MGPCG's point: from a cold start the multigrid-preconditioned CG needs a small
    /// fraction of Jacobi-PCG's iterations to reach the same tolerance (measured: 8 vs
    /// 139 on this 96x96 grid).
    #[test]
    fn multigrid_preconditioner_cuts_cg_iterations() {
        let method = |multigrid| Method::Pcg { max_iterations: 1000, tolerance: 1e-3, non_negative: true, multigrid };
        let (jacobi, jacobi_residual) = solve(&mut falling_tank_grid(), method(false));
        let (mg, mg_residual) = solve(&mut falling_tank_grid(), method(true));
        assert!(mg_residual < 1e-3 && jacobi_residual < 1e-3, "residuals: MG {mg_residual}, Jacobi {jacobi_residual}");
        assert!(mg * 8 < jacobi, "MGPCG took {mg} iterations, Jacobi-PCG {jacobi}");
    }

    /// A closed, all-fluid box (no air: pressure defined only up to a constant, the
    /// stable-fluids case) must still converge with the multigrid preconditioner.
    #[test]
    fn multigrid_cg_handles_a_closed_all_fluid_box() {
        let (nx, ny) = (64, 64);
        let (mut grid, solid) = closed_box_grid(nx, ny, 1.0 / ny as f32);
        for (c, &s) in grid.cell.as_mut_slice().iter_mut().zip(solid.as_slice()) {
            *c = if s { CellType::Solid } else { CellType::Fluid };
        }
        let mut rng = SplitMix64::new(9);
        grid.u.as_mut_slice().iter_mut().for_each(|x| *x = rng.signed());
        grid.v.as_mut_slice().iter_mut().for_each(|x| *x = rng.signed());
        grid.zero_solid_faces();
        let mut p = Grid2::new(nx, ny);
        let method = Method::Pcg { max_iterations: 200, tolerance: 1e-3, non_negative: false, multigrid: true };
        let used = pressure::project(&mut grid, &mut PressureSolver::new(nx, ny), &mut p, &Grid2::new(nx, ny), 1.0, method, None);
        let residual = pressure::max_residual(&grid, &p, false);
        assert!(residual < 1e-3, "residual {residual} m/s after {used} iterations");
        assert!(used < 20, "{used} iterations");
    }

    #[test]
    fn mgpcg_pressure_field_is_hydrostatic() {
        check_hydrostatic(FlipParams { pcg_multigrid: true, ..pcg_params() });
    }

    #[test]
    fn painting_a_wall_through_fluid_ejects_particles() {
        let (mut scene, mut flip) = calm_tank();
        let stepper = Stepper::default();
        scene.solid.paint_disc(Vec2::new(0.5, 0.25), 0.12, true);
        scene.solid.update_derived();
        for _ in 0..30 {
            stepper.advance(&mut flip, &scene, 1.0 / 60.0, &StepInput::default());
        }
        let inside = flip.pos.iter().filter(|&&p| scene.solid.distance(p) < -1.25 * flip.radius).count();
        assert_eq!(inside, 0, "{inside} particles still inside the painted wall");
    }

    #[test]
    fn push_brush_moves_fluid() {
        let (scene, mut flip) = calm_tank();
        let brush = crate::scene::ForceBrush {
            pos: Vec2::new(0.5, 0.25),
            radius: 0.1,
            velocity: Vec2::new(2.0, 0.0),
            strength: 30.0,
            spin: 0.0,
        };
        let input = StepInput { brushes: vec![brush], ..StepInput::default() };
        let stepper = Stepper::default();
        for _ in 0..10 {
            stepper.advance(&mut flip, &scene, 1.0 / 60.0, &input);
        }
        let near: Vec<Vec2> =
            flip.pos.iter().zip(&flip.vel).filter(|(p, _)| p.distance(brush.pos) < 0.05).map(|(_, v)| *v).collect();
        let mean_vx = near.iter().map(|v| v.x).sum::<f32>() / near.len() as f32;
        assert!(mean_vx > 0.3, "mean x velocity under the brush is {mean_vx} m/s");
    }

    /// End-to-end check of the pressure solve, units and all: in a resting tank the
    /// pressure field must match hydrostatics, p = Ï g (surface - y), in pascals.
    #[test]
    fn pressure_field_is_hydrostatic() {
        check_hydrostatic(FlipParams::default());
    }

    #[test]
    fn pcg_pressure_field_is_hydrostatic() {
        check_hydrostatic(pcg_params());
    }

    fn check_hydrostatic(params: FlipParams) {
        let (scene, mut flip) = calm_tank_with(params);
        let stepper = Stepper::default();
        for _ in 0..120 {
            stepper.advance(&mut flip, &scene, 1.0 / 60.0, &StepInput::default());
        }
        let mut field = Vec::new();
        let layout = flip.fill_field(0, &mut field).unwrap();
        let i = layout.nx / 2;
        let column = |x0: f32| flip.pos.iter().filter(move |p| (p.x - x0).abs() < layout.cell).map(|p| p.y);
        let x_centre = (i as f32 + 0.5) * layout.cell;
        let surface = column(x_centre).fold(0.0, f32::max) + flip.radius;
        let mut checked = 0;
        for j in 0..layout.ny {
            let p = field[j * layout.nx + i];
            let y = (j as f32 + 0.5) * layout.cell;
            let depth = surface - y;
            if p.is_nan() || depth < 0.1 {
                continue; // air, or too close to the surface for a meaningful ratio
            }
            let expected = RHO * 9.81 * depth;
            assert!((p / expected - 1.0).abs() < 0.15, "y = {y:.3}: p = {p:.0} Pa, hydrostatic {expected:.0} Pa");
            checked += 1;
        }
        assert!(checked >= 5, "only {checked} cells checked");
    }

    /// Fields line up with the grid: sample (i, j) sits at the centre of cell (i, j).
    #[test]
    fn field_layout_matches_the_grid() {
        let (_, flip) = calm_tank();
        let mut out = Vec::new();
        for index in 0..FIELDS.len() {
            let layout = flip.fill_field(index, &mut out).unwrap();
            assert_eq!(layout, FieldLayout { nx: flip.grid.nx, ny: flip.grid.ny, origin: Vec2::ZERO, cell: flip.grid.h });
            assert_eq!(out.len(), layout.nx * layout.ny);
        }
        assert!(flip.fill_field(FIELDS.len(), &mut out).is_none());
    }

    /// After a dam break settles, the fluid must still occupy about as many cells as it
    /// started with. Catches compaction (no separation), sinking (moving particles before
    /// projection) and leaks into spurious air cells along walls.
    #[test]
    fn dam_break_keeps_its_volume() {
        check_dam_break_volume(FlipParams::default());
    }

    #[test]
    fn pcg_dam_break_keeps_its_volume() {
        check_dam_break_volume(pcg_params());
    }

    fn check_dam_break_volume(params: FlipParams) {
        let scene = crate::presets::dam_break();
        let mut flip = Flip::with_params(FlipParams { cells_y: 50, ..params });
        flip.reset(&scene, Init::FromScene);
        let occupied = |f: &Flip| {
            let h = f.grid.h;
            let cells: std::collections::HashSet<_> = f.pos.iter().map(|p| ((p.x / h) as i32, (p.y / h) as i32)).collect();
            cells.len()
        };
        let before = occupied(&flip);
        let stepper = Stepper::default();
        for _ in 0..300 {
            stepper.advance(&mut flip, &scene, 1.0 / 60.0, &StepInput::default());
        }
        let ratio = occupied(&flip) as f32 / before as f32;
        assert!((0.85..1.2).contains(&ratio), "occupied cells {before} -> {} ({ratio:.2}x)", occupied(&flip));
    }
}
