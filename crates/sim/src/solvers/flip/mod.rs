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
mod pressure;
mod separate;
mod transfer;

use crate::grid::Grid2;
use crate::grid::mac::MacGrid;
use crate::neighbors::CellBins;
use crate::scene::{Scene, StepInput};
use crate::solver::{Init, Metric, ParticleView, Solver, Stats};
use crate::timing::PhaseTimer;
use glam::Vec2;
use pressure::Drift;
use transfer::Weights;

#[derive(Clone, Debug)]
pub struct FlipParams {
    /// Grid resolution along the domain height (changing it requires a reset).
    pub cells_y: u32,
    /// 0 = PIC (smooth, viscous), 1 = FLIP (lively, noisy).
    pub flip_ratio: f32,
    pub pressure_iters: u32,
    pub over_relax: f32,
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
    weights: Weights,
    /// Persistent pressure (per unit time) for warm-starting the solver.
    pressure: Grid2<f32>,
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
            weights: Weights::for_grid(&grid),
            pressure: Grid2::new(2, 2),
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
        self.weights = Weights::for_grid(&self.grid);
        self.pressure = Grid2::new(nx, ny);
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
        let (nx, ny, h) = (self.grid.nx, self.grid.ny, self.grid.h);
        for j in 0..ny {
            for i in 0..nx {
                let centre = (Vec2::new(i as f32, j as f32) + 0.5) * h;
                let border = i == 0 || j == 0 || i == nx - 1 || j == ny - 1;
                self.solid[(i, j)] = border || scene.solid.distance(centre) < 0.0;
            }
        }
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
                self.pos = view.pos.to_vec();
                self.vel = view.vel.to_vec();
            }
        }
        // Interior density of the seeding lattice, in particles per cell.
        self.rest_density = (PARTICLES_PER_AXIS * PARTICLES_PER_AXIS) as f32;
        self.advect = vec![Vec2::ZERO; self.pos.len()];
        self.stats = Stats { particles: self.pos.len(), ..Stats::default() };
    }

    fn max_dt(&self, _scene: &Scene) -> f32 {
        let max_speed = self.vel.iter().map(|v| v.length_squared()).fold(0.0, f32::max).sqrt();
        self.params.cfl * self.grid.h / max_speed.max(1e-3)
    }

    fn step(&mut self, dt: f32, scene: &Scene, _input: &StepInput) {
        let mut timer = PhaseTimer::start();
        self.refresh_solids(scene);

        transfer::mark_cells(&mut self.grid, &self.solid, &self.pos);
        transfer::particles_to_grid(&mut self.grid, &mut self.weights, &self.pos, &self.vel);
        transfer::enforce_solid_boundaries(&mut self.grid);
        self.u_prev.copy_from(&self.grid.u);
        self.v_prev.copy_from(&self.grid.v);
        timer.lap("p2g");

        transfer::apply_gravity(&mut self.grid, scene.gravity, dt);
        transfer::enforce_solid_boundaries(&mut self.grid);
        let drift = self.params.compensate_drift.then(|| {
            transfer::particle_density(&mut self.density, self.grid.h, &self.pos);
            Drift { gain: self.params.drift_stiffness * self.grid.h / dt, rest: self.rest_density }
        });
        timer.lap("forces");

        pressure::project(
            &mut self.grid,
            &mut self.pressure,
            &self.density,
            dt,
            self.params.pressure_iters,
            self.params.over_relax,
            drift,
        );
        let residual = pressure::max_residual(&self.grid, &self.pressure);
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

        for (p, a) in self.pos.iter_mut().zip(&self.advect) {
            *p += *a * dt;
        }
        timer.lap("advect");

        if self.params.separate_particles {
            separate::push_apart(&mut self.bins, &mut self.pos, self.radius, self.params.separation_iters, scene.size);
        }
        timer.lap("separate");

        collide::collide(&self.solid, self.grid.h, self.radius, scene, &mut self.pos, &mut self.vel);
        timer.lap("collide");

        self.stats.phases = timer.finish();
        self.stats.metrics = vec![Metric { name: "pressure residual (1/s)", value: residual, tolerance: None }];
    }

    fn particles(&self) -> Option<ParticleView<'_>> {
        Some(ParticleView { pos: &self.pos, vel: &self.vel, radius: self.radius })
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

        let mut weights = Weights::for_grid(&grid);
        transfer::mark_cells(&mut grid, &solid, &pos);
        transfer::particles_to_grid(&mut grid, &mut weights, &pos, &vel);
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
        transfer::enforce_solid_boundaries(&mut grid);

        let mut p = Grid2::new(nx, ny);
        let before = pressure::max_residual(&grid, &p);
        pressure::project(&mut grid, &mut p, &Grid2::new(nx, ny), 1.0, 300, 1.9, None);
        let after = pressure::max_residual(&grid, &p);
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

    /// After a dam break settles, the fluid must still occupy about as many cells as it
    /// started with. Catches compaction (no separation), sinking (moving particles before
    /// projection) and leaks into spurious air cells along walls.
    #[test]
    fn dam_break_keeps_its_volume() {
        let scene = crate::presets::dam_break();
        let mut flip = Flip::with_params(FlipParams { cells_y: 50, ..FlipParams::default() });
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
