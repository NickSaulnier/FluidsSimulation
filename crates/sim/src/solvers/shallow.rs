//! Shallow water: a water surface seen from above, as a height field. Waves run across
//! it at the speed sqrt(g × depth), reflect off the edges and the walls (islands, in this
//! view), and leave wakes behind them; brushes push the water, and tilting gravity
//! sloshes it from side to side.
//!
//! The shallow water equations on a staggered grid (as in Müller, "Fluid Simulation for
//! Computer Graphics", 2008): the depth at cell centres, the velocity on the faces. Per
//! substep: cell types -> brushes and the tilt -> semi-Lagrangian advection of the
//! velocity -> the depth moved by upwind fluxes (so water is neither made nor lost) ->
//! the velocity accelerated down the surface's slope -> damping.
//!
//! Scenes are read from above: water everywhere at the resting depth, the fluid blocks
//! raised above it (a dam break becomes a bore spreading out), emitters push currents
//! and lift the water at their nozzles (the faucet tool too), drains absorb waves, and
//! the music's splashes and jets make ripples. The water slowly settles back to the
//! amount it started with, so emitters don't flood it.

use crate::grid::Grid2;
use crate::grid::mac::{CENTRE_OFFSET, Stencil, U_OFFSET, V_OFFSET};
use crate::params::{FloatSpec, IntSpec, ParamVisitor};
use crate::scene::{Scene, StepInput};
use crate::solver::{FieldInfo, FieldKind, FieldLayout, Init, Metric, ParticleView, Solver, Stats};
use crate::timing::PhaseTimer;
use glam::Vec2;
use rayon::prelude::*;

/// Gravity across the surface, the one that makes the waves (straight into the picture,
/// whatever the scene's gravity, which tilts the water instead).
const G: f32 = 9.81;
/// How much of the scene's gravity, turned away from straight down, tilts the water.
const TILT: f32 = 0.2;
/// Fluid blocks stand this far above the resting depth, as a fraction of it.
const BLOCK_RISE: f32 = 0.5;
/// An emitter's nozzle holds the water this far above the resting depth (as a fraction
/// of it), and pushes it at this fraction of the emitter's speed.
const NOZZLE_RISE: f32 = 0.6;
const NOZZLE_PUSH: f32 = 0.35;
/// How fast each face's velocity eases toward its neighbours' average, 1/s: a small
/// viscosity that wipes out grid-scale chop (short waves fade as the square of their
/// shortness) and hardly touches the waves you see.
const SMOOTH_RATE: f32 = 4.0;
/// Brushes push the water no faster than this many times the wave speed: much faster
/// and the surface breaks into shocks (the music's splashes push at several metres a
/// second, made for particles).
const BRUSH_LIMIT: f32 = 1.5;
/// How fast drains pull the water back to rest, and the whole surface back to the
/// amount of water it started with, 1/s.
const DRAIN_RATE: f32 = 8.0;
const SETTLE_RATE: f32 = 0.3;
/// How fast a drop of strength 1 dents the surface, in rest depths per second at its
/// centre (the water rushes back in as it lands, so the dent is far shallower).
const DROP_RATE: f32 = 60.0;

#[derive(Clone, Debug)]
pub struct ShallowParams {
    pub cells_y: u32,
    /// Resting depth, metres: deeper water carries faster waves.
    pub depth: f32,
    /// Fraction of the water's motion lost per second.
    pub damping: f32,
    /// Max travel per substep, in cells, for the fastest wave.
    pub cfl: f32,
}

impl Default for ShallowParams {
    fn default() -> Self {
        Self { cells_y: 192, depth: 0.03, damping: 0.3, cfl: 0.5 }
    }
}

const CELLS_Y: IntSpec = IntSpec {
    key: "cells_y",
    label: "Grid cells (height)",
    range: 64..=512,
    restart: true,
    help: "Resolution of the water surface.",
};
const DEPTH: FloatSpec = FloatSpec {
    key: "depth",
    label: "Depth (m)",
    range: 0.01..=0.5,
    log: true,
    restart: false,
    help: "How deep the water rests. Waves run at sqrt(g × depth): deeper is faster.",
};
const DAMPING: FloatSpec = FloatSpec {
    key: "damping",
    label: "Damping (1/s)",
    range: 0.0..=2.0,
    log: false,
    restart: false,
    help: "How fast waves die away. 0: they run until the walls wear them down.",
};
const CFL: FloatSpec = FloatSpec {
    key: "cfl",
    label: "CFL (cells per substep)",
    range: 0.1..=0.9,
    log: false,
    restart: false,
    help: "How far the fastest wave travels per substep. Above about 1 the surface blows up.",
};

const FIELDS: &[FieldInfo] = &[
    FieldInfo { key: "water", label: "Water", unit: "", kind: FieldKind::Water },
    FieldInfo { key: "height", label: "Height above rest", unit: "m", kind: FieldKind::Signed },
    FieldInfo { key: "speed", label: "Speed", unit: "m/s", kind: FieldKind::NonNegative },
];

/// What a cell is, this substep.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Cell {
    #[default]
    Water,
    Solid,
    Drain,
    /// An emitter's nozzle: the water's velocity there, held.
    Nozzle(Vec2),
}

pub struct ShallowWater {
    pub params: ShallowParams,
    nx: usize,
    ny: usize,
    h: f32,
    depth: Grid2<f32>,
    u: Grid2<f32>,
    v: Grid2<f32>,
    cells: Grid2<Cell>,
    /// Which faces lie between two open cells (the rest carry nothing), and the nozzles'
    /// faces with the velocity they hold.
    open_u: Grid2<bool>,
    open_v: Grid2<bool>,
    nozzle_faces: Vec<(bool, usize, usize, f32)>,
    /// The solids as last resampled, by the mask's generation.
    solid: Grid2<bool>,
    solid_generation: Option<u64>,
    /// Scratch.
    depth_next: Grid2<f32>,
    u_next: Grid2<f32>,
    v_next: Grid2<f32>,
    /// The water the scene started with (depth summed over cells).
    volume: f32,
    stats: Stats,
}

impl Default for ShallowWater {
    fn default() -> Self {
        Self::with_params(ShallowParams::default())
    }
}

impl ShallowWater {
    pub fn with_params(params: ShallowParams) -> Self {
        let g = || Grid2::new(2, 2);
        Self {
            params,
            nx: 2,
            ny: 2,
            h: 1.0,
            depth: g(),
            u: Grid2::new(3, 2),
            v: Grid2::new(2, 3),
            cells: Grid2::new(2, 2),
            open_u: Grid2::new(3, 2),
            open_v: Grid2::new(2, 3),
            nozzle_faces: Vec::new(),
            solid: Grid2::new(2, 2),
            solid_generation: None,
            depth_next: g(),
            u_next: Grid2::new(3, 2),
            v_next: Grid2::new(2, 3),
            volume: 0.0,
            stats: Stats::default(),
        }
    }

    fn resize(&mut self, size: Vec2) {
        let ny = self.params.cells_y.max(8) as usize;
        let h = size.y / ny as f32;
        let nx = ((size.x / h).round() as usize).max(8);
        (self.nx, self.ny, self.h) = (nx, ny, h);
        self.depth = Grid2::new(nx, ny);
        self.depth_next = Grid2::new(nx, ny);
        self.u = Grid2::new(nx + 1, ny);
        self.u_next = Grid2::new(nx + 1, ny);
        self.v = Grid2::new(nx, ny + 1);
        self.v_next = Grid2::new(nx, ny + 1);
        self.cells = Grid2::new(nx, ny);
        self.open_u = Grid2::new(nx + 1, ny);
        self.open_v = Grid2::new(nx, ny + 1);
        self.solid = Grid2::new(nx, ny);
        self.solid_generation = None;
    }

    fn centre(&self, i: usize, j: usize) -> Vec2 {
        (Vec2::new(i as f32, j as f32) + 0.5) * self.h
    }

    /// Solids (resampled when the mask changes), drains and emitter nozzles.
    fn update_cells(&mut self, scene: &Scene, input: &StepInput) {
        let generation = scene.solid.generation();
        if self.solid_generation != Some(generation) {
            super::resample_solids(scene, self.h, &mut self.solid);
            self.solid_generation = Some(generation);
        }
        let emitters: Vec<_> = scene.emitters.iter().chain(&input.emitters).collect();
        let h = self.h;
        let solid = &self.solid;
        self.cells.par_rows_mut().enumerate().for_each(|(j, row)| {
            for (i, cell) in row.iter_mut().enumerate() {
                let c = (Vec2::new(i as f32, j as f32) + 0.5) * h;
                *cell = if solid[(i, j)] {
                    Cell::Solid
                } else if let Some(e) = emitters.iter().find(|e| super::nozzle_offset(e, h, c).is_some()) {
                    Cell::Nozzle(e.dir.normalize_or(Vec2::NEG_Y) * e.speed * NOZZLE_PUSH)
                } else if scene.drains.iter().any(|d| d.contains(c)) {
                    Cell::Drain
                } else {
                    Cell::Water
                };
            }
        });
        let (nx, ny) = (self.nx, self.ny);
        let cells = &self.cells;
        let open = |i: usize, j: usize| cells[(i, j)] != Cell::Solid;
        self.open_u.par_rows_mut().enumerate().for_each(|(j, row)| {
            for (i, o) in row.iter_mut().enumerate() {
                *o = i > 0 && i < nx && open(i - 1, j) && open(i, j);
            }
        });
        self.open_v.par_rows_mut().enumerate().for_each(|(j, row)| {
            for (i, o) in row.iter_mut().enumerate() {
                *o = j > 0 && j < ny && open(i, j - 1) && open(i, j);
            }
        });
        self.nozzle_faces.clear();
        for j in 0..ny {
            for i in 0..nx {
                if let Cell::Nozzle(vel) = self.cells[(i, j)] {
                    for (fi, fj) in [(i, j), (i + 1, j)] {
                        if self.open_u[(fi, fj)] {
                            self.nozzle_faces.push((true, fi, fj, vel.x));
                        }
                    }
                    for (fi, fj) in [(i, j), (i, j + 1)] {
                        if self.open_v[(fi, fj)] {
                            self.nozzle_faces.push((false, fi, fj, vel.y));
                        }
                    }
                }
            }
        }
    }

    fn open(&self, i: usize, j: usize) -> bool {
        self.cells[(i, j)] != Cell::Solid
    }

    /// The tilt accelerates every face; brushes blend the faces under them toward their
    /// targets; drops dent the surface.
    fn apply_forces(&mut self, scene: &Scene, input: &StepInput, dt: f32) {
        let g = scene.gravity;
        let tilt = TILT * (g - Vec2::new(0.0, -g.length())) * dt;
        if tilt.x != 0.0 {
            self.u.as_mut_slice().iter_mut().for_each(|u| *u += tilt.x);
        }
        if tilt.y != 0.0 {
            self.v.as_mut_slice().iter_mut().for_each(|v| *v += tilt.y);
        }
        let h = self.h;
        let rest = self.params.depth;
        let fastest = BRUSH_LIMIT * (G * rest).sqrt();
        for drop in &input.drops {
            let lo = (drop.pos - drop.radius) / h;
            let hi = (drop.pos + drop.radius) / h;
            for j in (lo.y.max(0.0) as usize)..=(hi.y as usize).min(self.ny - 1) {
                for i in (lo.x.max(0.0) as usize)..=(hi.x as usize).min(self.nx - 1) {
                    let r = self.centre(i, j).distance(drop.pos) / drop.radius;
                    if r < 1.0 && self.cells[(i, j)] != Cell::Solid {
                        let dent = DROP_RATE * drop.strength * rest * (1.0 - r * r).powi(2) * dt;
                        self.depth[(i, j)] = (self.depth[(i, j)] - dent).max(0.0);
                    }
                }
            }
        }
        for b in &input.brushes {
            for (grid, offset, axis) in [(&mut self.u, U_OFFSET, 0), (&mut self.v, V_OFFSET, 1)] {
                // Only the faces within the brush's reach.
                let lo = ((b.pos - b.radius) / h - offset).floor().max(Vec2::ZERO);
                let hi = ((b.pos + b.radius) / h - offset).ceil();
                let (i1, j1) = ((hi.x as usize).min(grid.nx() - 1), (hi.y as usize).min(grid.ny() - 1));
                for j in lo.y as usize..=j1 {
                    for i in lo.x as usize..=i1 {
                        let p = (Vec2::new(i as f32, j as f32) + offset) * h;
                        let d = p.distance(b.pos);
                        if d < b.radius {
                            let blend = (b.strength * (1.0 - d / b.radius) * dt).min(1.0);
                            grid[(i, j)] += (b.target(p).clamp_length_max(fastest)[axis] - grid[(i, j)]) * blend;
                        }
                    }
                }
            }
        }
    }

    /// Semi-Lagrangian advection of both velocity components: one step back along the
    /// velocity at the face (its own component, and the other averaged from the four
    /// faces round it). Waves, not the current, carry most of the motion here, so a
    /// first-order backtrace is plenty, and five times cheaper than sampling the field.
    fn advect_velocity(&mut self, dt: f32) {
        let (nx, ny, h) = (self.nx, self.ny, self.h);
        let mut u_next = std::mem::replace(&mut self.u_next, Grid2::new(0, 0));
        let mut v_next = std::mem::replace(&mut self.v_next, Grid2::new(0, 0));
        let (u, v) = (&self.u, &self.v);
        u_next.par_rows_mut().enumerate().for_each(|(j, row)| {
            for (i, o) in row.iter_mut().enumerate() {
                let (l, r) = (i.saturating_sub(1), i.min(nx - 1));
                let across = 0.25 * (v[(l, j)] + v[(r, j)] + v[(l, j + 1)] + v[(r, j + 1)]);
                let p = (Vec2::new(i as f32, j as f32) + U_OFFSET) * h - dt * Vec2::new(u[(i, j)], across);
                *o = Stencil::on(u, h, U_OFFSET, p).sample(u);
            }
        });
        v_next.par_rows_mut().enumerate().for_each(|(j, row)| {
            for (i, o) in row.iter_mut().enumerate() {
                let (b, t) = (j.saturating_sub(1), j.min(ny - 1));
                let across = 0.25 * (u[(i, b)] + u[(i + 1, b)] + u[(i, t)] + u[(i + 1, t)]);
                let p = (Vec2::new(i as f32, j as f32) + V_OFFSET) * h - dt * Vec2::new(across, v[(i, j)]);
                *o = Stencil::on(v, h, V_OFFSET, p).sample(v);
            }
        });
        self.u_next = std::mem::replace(&mut self.u, u_next);
        self.v_next = std::mem::replace(&mut self.v, v_next);
    }

    /// Faces between solid and water carry nothing; nozzles hold their velocity.
    fn enforce_faces(&mut self) {
        for (grid, open) in [(&mut self.u, &self.open_u), (&mut self.v, &self.open_v)] {
            grid.as_mut_slice().par_iter_mut().zip(open.as_slice().par_iter()).with_min_len(4096).for_each(|(x, &o)| {
                if !o {
                    *x = 0.0;
                }
            });
        }
        for &(x_face, i, j, value) in &self.nozzle_faces {
            if x_face {
                self.u[(i, j)] = value;
            } else {
                self.v[(i, j)] = value;
            }
        }
    }

    /// Move the water by upwind fluxes through the faces (conservative: what leaves one
    /// cell enters the next), then lift nozzles and settle drains.
    fn advect_depth(&mut self, dt: f32) {
        let (nx, ny, h) = (self.nx, self.ny, self.h);
        let mut next = std::mem::replace(&mut self.depth_next, Grid2::new(0, 0));
        let this = &*self;
        let flux_u = |i: usize, j: usize| {
            let u = this.u[(i, j)];
            if u == 0.0 {
                0.0
            } else if u > 0.0 {
                u * this.depth[(i - 1, j)]
            } else {
                u * this.depth[(i, j)]
            }
        };
        let flux_v = |i: usize, j: usize| {
            let v = this.v[(i, j)];
            if v == 0.0 {
                0.0
            } else if v > 0.0 {
                v * this.depth[(i, j - 1)]
            } else {
                v * this.depth[(i, j)]
            }
        };
        let rest = this.params.depth;
        next.par_rows_mut().enumerate().for_each(|(j, row)| {
            for (i, o) in row.iter_mut().enumerate() {
                let d = this.depth[(i, j)];
                *o = match this.cells[(i, j)] {
                    Cell::Solid => 0.0,
                    cell => {
                        let out = flux_u(i + 1, j) - flux_u(i, j) + flux_v(i, j + 1) - flux_v(i, j);
                        let d = (d - dt / h * out).max(0.0);
                        match cell {
                            Cell::Nozzle(_) => d.max(rest * (1.0 + NOZZLE_RISE)),
                            Cell::Drain => d + (rest - d) * (DRAIN_RATE * dt).min(1.0),
                            _ => d,
                        }
                    }
                };
            }
        });
        debug_assert_eq!((next.nx(), next.ny()), (nx, ny));
        self.depth_next = std::mem::replace(&mut self.depth, next);
    }

    /// Accelerate the water down the surface's slope; damp it; settle drains; and keep
    /// every face below the speed the substep can carry.
    fn accelerate(&mut self, dt: f32) {
        let h = self.h;
        let keep = (-self.params.damping * dt).exp();
        let limit = 0.5 * h / dt;
        let this_depth = &self.depth;
        let cells = &self.cells;
        let drained = |c: Cell| if c == Cell::Drain { (1.0 - DRAIN_RATE * dt).max(0.0) } else { 1.0 };
        let (nx, ny) = (self.nx, self.ny);
        self.u.par_rows_mut().enumerate().for_each(|(j, row)| {
            for (i, u) in row.iter_mut().enumerate().take(nx).skip(1) {
                let slope = (this_depth[(i, j)] - this_depth[(i - 1, j)]) / h;
                let damp = keep * drained(cells[(i, j)]) * drained(cells[(i - 1, j)]);
                *u = ((*u - G * slope * dt) * damp).clamp(-limit, limit);
            }
        });
        self.v.par_rows_mut().enumerate().take(ny).skip(1).for_each(|(j, row)| {
            for (i, v) in row.iter_mut().enumerate() {
                let slope = (this_depth[(i, j)] - this_depth[(i, j - 1)]) / h;
                let damp = keep * drained(cells[(i, j)]) * drained(cells[(i, j - 1)]);
                *v = ((*v - G * slope * dt) * damp).clamp(-limit, limit);
            }
        });
    }

    /// Ease every face toward the average of its four neighbours (see `SMOOTH_RATE`).
    fn smooth_velocity(&mut self, dt: f32) {
        let a = (SMOOTH_RATE * dt).min(0.25);
        for (grid, scratch) in [(&mut self.u, &mut self.u_next), (&mut self.v, &mut self.v_next)] {
            let (nx, ny) = (grid.nx(), grid.ny());
            let src = &*grid;
            scratch.par_rows_mut().enumerate().for_each(|(j, row)| {
                for (i, o) in row.iter_mut().enumerate() {
                    let here = src[(i, j)];
                    if i == 0 || j == 0 || i == nx - 1 || j == ny - 1 {
                        *o = here;
                        continue;
                    }
                    let around = 0.25 * (src[(i - 1, j)] + src[(i + 1, j)] + src[(i, j - 1)] + src[(i, j + 1)]);
                    *o = here + a * (around - here);
                }
            });
            std::mem::swap(grid, scratch);
        }
    }

    /// Ease the whole surface back toward the water it started with (emitters add some,
    /// drains take some away), evenly over the open cells.
    fn settle(&mut self, dt: f32) {
        let open = self.cells.as_slice().iter().filter(|c| **c != Cell::Solid).count();
        if open == 0 {
            return;
        }
        let shift = (self.volume - self.total_water()) / open as f32 * (SETTLE_RATE * dt).min(1.0);
        for (d, c) in self.depth.as_mut_slice().iter_mut().zip(self.cells.as_slice()) {
            if *c != Cell::Solid {
                *d = (*d + shift).max(0.0);
            }
        }
    }

    /// Depth summed over the open cells (cells of water, not cubic metres).
    fn total_water(&self) -> f32 {
        self.depth.as_slice().iter().zip(self.cells.as_slice()).filter(|(_, c)| **c != Cell::Solid).map(|(d, _)| d).sum()
    }

    /// The surface's slope at cell (i, j), from its open neighbours.
    fn slope(&self, i: usize, j: usize) -> Vec2 {
        let d = |i: usize, j: usize, fallback: f32| if self.open(i, j) { self.depth[(i, j)] } else { fallback };
        let here = self.depth[(i, j)];
        let (l, r) = (d(i.saturating_sub(1), j, here), d((i + 1).min(self.nx - 1), j, here));
        let (b, t) = (d(i, j.saturating_sub(1), here), d(i, (j + 1).min(self.ny - 1), here));
        Vec2::new(r - l, t - b) / (2.0 * self.h)
    }

    fn cell_velocity(&self, i: usize, j: usize) -> Vec2 {
        Vec2::new(0.5 * (self.u[(i, j)] + self.u[(i + 1, j)]), 0.5 * (self.v[(i, j)] + self.v[(i, j + 1)]))
    }

    fn max_speed(&self) -> f32 {
        let max_abs = |g: &Grid2<f32>| g.as_slice().iter().fold(0.0f32, |m, &x| m.max(x.abs()));
        max_abs(&self.u).max(max_abs(&self.v))
    }

    fn max_depth(&self) -> f32 {
        self.depth.as_slice().iter().copied().fold(0.0, f32::max)
    }
}

impl Solver for ShallowWater {
    fn name(&self) -> &'static str {
        "Shallow water"
    }

    fn reset(&mut self, scene: &Scene, init: Init<'_>) {
        self.resize(scene.size);
        self.update_cells(scene, &StepInput::default());
        let (nx, ny, h) = (self.nx, self.ny, self.h);
        let rest = self.params.depth;
        // How much of each cell is raised: the scene's fluid blocks, or another solver's
        // particles (their density, relative to a densely filled cell).
        let mut raised = Grid2::<f32>::new(nx, ny);
        match init {
            Init::FromScene => {
                for j in 0..ny {
                    for i in 0..nx {
                        let c = self.centre(i, j);
                        raised[(i, j)] = if scene.fluid_blocks.iter().any(|b| b.contains(c)) { 1.0 } else { 0.0 };
                    }
                }
            }
            Init::FromParticles(view) => {
                let per_cell = (h / (2.0 * view.radius)).powi(2);
                for &p in view.pos {
                    let s = Stencil::on(&raised, h, CENTRE_OFFSET, p);
                    for (c, w) in s.corners().into_iter().zip(s.w) {
                        raised[c] += w / per_cell;
                    }
                }
            }
        }
        for j in 0..ny {
            for i in 0..nx {
                self.depth[(i, j)] = if self.open(i, j) { rest * (1.0 + BLOCK_RISE * raised[(i, j)].min(1.0)) } else { 0.0 };
            }
        }
        self.u.fill(0.0);
        self.v.fill(0.0);
        self.volume = self.total_water();
        self.stats = Stats::default();
    }

    fn max_dt(&self, _scene: &Scene) -> f32 {
        let wave = (G * self.max_depth().max(self.params.depth)).sqrt();
        self.params.cfl * self.h / (wave + self.max_speed())
    }

    fn step(&mut self, dt: f32, scene: &Scene, input: &StepInput) {
        let mut timer = PhaseTimer::start();
        self.update_cells(scene, input);
        self.apply_forces(scene, input, dt);
        self.enforce_faces();
        timer.lap("forces");
        self.advect_velocity(dt);
        self.enforce_faces();
        timer.lap("advect velocity");
        self.advect_depth(dt);
        self.settle(dt);
        timer.lap("advect depth");
        self.accelerate(dt);
        // Clear the faces against walls first, so their values don't leak into the water.
        self.enforce_faces();
        self.smooth_velocity(dt);
        self.enforce_faces();
        timer.lap("waves");
        self.stats.phases = timer.finish();
        let rest = self.params.depth.max(1e-6);
        self.stats.metrics = vec![
            Metric { name: "water (relative to the start)", value: self.total_water() / self.volume.max(1e-6), tolerance: None },
            Metric { name: "highest wave (× rest depth)", value: self.max_depth() / rest, tolerance: Some(20.0) },
        ];
    }

    fn visit_params(&mut self, v: &mut dyn ParamVisitor) {
        let p = &mut self.params;
        v.section("Water");
        v.float(&DEPTH, &mut p.depth);
        v.float(&DAMPING, &mut p.damping);
        v.section("Resolution");
        v.int(&CELLS_Y, &mut p.cells_y);
        v.advanced_section("Solver");
        v.float(&CFL, &mut p.cfl);
    }

    fn particles(&self) -> Option<ParticleView<'_>> {
        None
    }

    fn fields(&self) -> &'static [FieldInfo] {
        FIELDS
    }

    fn velocity_field(&self, out: &mut Vec<Vec2>) -> Option<FieldLayout> {
        let (nx, ny) = (self.nx, self.ny);
        out.clear();
        out.extend((0..ny).flat_map(|j| (0..nx).map(move |i| (i, j))).map(|(i, j)| if self.open(i, j) { self.cell_velocity(i, j) } else { Vec2::ZERO }));
        Some(FieldLayout { nx, ny, origin: Vec2::ZERO, cell: self.h })
    }

    fn fill_field(&self, index: usize, out: &mut Vec<f32>) -> Option<FieldLayout> {
        let (nx, ny) = (self.nx, self.ny);
        let info = FIELDS.get(index)?;
        let layout = FieldLayout { nx, ny, origin: Vec2::ZERO, cell: self.h };
        let rest = self.params.depth.max(1e-6);
        out.clear();
        if info.kind == FieldKind::Water {
            // Three values per cell: height above rest (in rest depths), and the slope.
            out.resize(3 * nx * ny, f32::NAN);
            for j in 0..ny {
                for i in 0..nx {
                    if self.open(i, j) {
                        let s = self.slope(i, j);
                        let k = 3 * (j * nx + i);
                        out[k..k + 3].copy_from_slice(&[self.depth[(i, j)] / rest - 1.0, s.x, s.y]);
                    }
                }
            }
            return Some(layout);
        }
        out.resize(nx * ny, f32::NAN);
        for j in 0..ny {
            for i in 0..nx {
                if self.open(i, j) {
                    out[j * nx + i] = match info.key {
                        "height" => self.depth[(i, j)] - rest,
                        _ => self.cell_velocity(i, j).length(),
                    };
                }
            }
        }
        Some(layout)
    }

    fn stats(&self) -> &Stats {
        &self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets;
    use crate::scene::{Emitter, ForceBrush};

    fn run(s: &mut ShallowWater, scene: &Scene, input: &StepInput, seconds: f32) {
        let mut t = 0.0;
        while t < seconds {
            let dt = s.max_dt(scene).min(1.0 / 60.0);
            s.step(dt, scene, input);
            t += dt;
        }
    }

    fn calm() -> (ShallowWater, Scene) {
        let scene = Scene::empty_box(presets::DEFAULT_DOMAIN, 128);
        let mut s = ShallowWater::with_params(ShallowParams { cells_y: 64, ..ShallowParams::default() });
        s.reset(&scene, Init::FromScene);
        (s, scene)
    }

    /// Height of the water at `p` above rest, metres.
    fn height(s: &ShallowWater, p: Vec2) -> f32 {
        let (i, j) = ((p.x / s.h) as usize, (p.y / s.h) as usize);
        s.depth[(i, j)] - s.params.depth
    }

    /// Still water stays still, flat and all there.
    #[test]
    fn still_water_stays_still() {
        let (mut s, scene) = calm();
        run(&mut s, &scene, &StepInput::default(), 2.0);
        assert!(s.max_speed() < 1e-4, "moving at {}", s.max_speed());
        assert!((s.total_water() / s.volume - 1.0).abs() < 1e-4);
    }

    /// A dam break: the raised block spreads out as a wave at about the shallow water
    /// speed, and the water is all still there.
    #[test]
    fn a_raised_block_spreads_at_the_wave_speed() {
        let scene = presets::dam_break();
        let mut s = ShallowWater::with_params(ShallowParams { cells_y: 64, damping: 0.0, ..ShallowParams::default() });
        s.reset(&scene, Init::FromScene);
        let volume = s.total_water();
        let speed = (G * s.params.depth).sqrt();
        // The block ends at x = 0.7; after 0.5 s the front is near 0.7 + 0.5 × speed.
        run(&mut s, &scene, &StepInput::default(), 0.5);
        let front = 0.7 + 0.5 * speed;
        let ahead = height(&s, Vec2::new(front - 0.08, 0.3));
        let beyond = height(&s, Vec2::new(front + 0.25, 0.3));
        assert!(ahead > 0.1 * s.params.depth, "no wave at {front}: {ahead}");
        assert!(beyond.abs() < 0.05 * s.params.depth, "the wave ran ahead of itself: {beyond}");
        assert!((s.total_water() / volume - 1.0).abs() < 1e-3, "water made or lost");
    }

    /// A brush pushing across the water heaps it up ahead and leaves a trough behind.
    #[test]
    fn a_brush_pushes_a_wave() {
        let (mut s, scene) = calm();
        let brush = ForceBrush { pos: Vec2::new(1.0, 0.5), radius: 0.1, velocity: Vec2::new(1.0, 0.0), strength: 20.0, spin: 0.0 };
        run(&mut s, &scene, &StepInput { brushes: vec![brush], ..StepInput::default() }, 0.2);
        assert!(height(&s, Vec2::new(1.12, 0.5)) > height(&s, Vec2::new(0.88, 0.5)) + 0.01 * s.params.depth);
    }

    /// Tilting gravity piles the water up on the low side.
    #[test]
    fn tilting_sloshes_the_water() {
        let (mut s, mut scene) = calm();
        scene.gravity = Vec2::new(9.81, -9.81).normalize() * 9.81;
        run(&mut s, &scene, &StepInput::default(), 0.5);
        assert!(height(&s, Vec2::new(1.9, 0.5)) > height(&s, Vec2::new(0.1, 0.5)) + 0.05 * s.params.depth);
    }

    /// A wall stops the water: none gets inside, and the faces round it carry nothing.
    #[test]
    fn walls_stay_dry() {
        let mut scene = Scene::empty_box(presets::DEFAULT_DOMAIN, 128);
        scene.solid.paint_disc(Vec2::new(1.0, 0.5), 0.15, true);
        scene.solid.update_derived();
        let mut s = ShallowWater::with_params(ShallowParams { cells_y: 64, ..ShallowParams::default() });
        s.reset(&scene, Init::FromScene);
        let brush = ForceBrush { pos: Vec2::new(0.6, 0.5), radius: 0.2, velocity: Vec2::new(1.5, 0.0), strength: 20.0, spin: 0.0 };
        run(&mut s, &scene, &StepInput { brushes: vec![brush], ..StepInput::default() }, 1.0);
        assert_eq!(s.depth[((1.0 / s.h) as usize, (0.5 / s.h) as usize)], 0.0);
    }

    /// An emitter lifts and pushes the water, and the surface settles back toward the
    /// water it started with rather than flooding.
    #[test]
    fn emitters_make_currents_without_flooding() {
        let (mut s, scene) = calm();
        let tap = Emitter { pos: Vec2::new(0.5, 0.5), dir: Vec2::X, speed: 2.0, width: 0.1 };
        let input = StepInput { emitters: vec![tap], ..StepInput::default() };
        run(&mut s, &scene, &input, 4.0);
        let downstream = s.cell_velocity((0.62 / s.h) as usize, (0.5 / s.h) as usize);
        assert!(downstream.x > 0.05, "no current: {downstream}");
        assert!(s.total_water() / s.volume < 1.2, "flooding: {}", s.total_water() / s.volume);
    }

    /// A drop dents the surface, and the dent rings out: a ring higher than the water
    /// round it, a little later.
    #[test]
    fn a_drop_rings_out() {
        let scene = Scene::empty_box(presets::DEFAULT_DOMAIN, 128);
        let mut s = ShallowWater::default();
        s.reset(&scene, Init::FromScene);
        let centre = Vec2::new(1.0, 0.5);
        let drop = crate::scene::Droplet { pos: centre, radius: 0.06, strength: 1.0 };
        run(&mut s, &scene, &StepInput { drops: vec![drop], ..StepInput::default() }, 0.1);
        assert!(height(&s, centre) < -0.15 * s.params.depth, "no dent: {}", height(&s, centre));
        run(&mut s, &scene, &StepInput::default(), 0.25);
        let ring = (G * s.params.depth).sqrt() * 0.3;
        let at = |r: f32| (0..8).map(|k| height(&s, centre + Vec2::from_angle(k as f32 * 0.785) * r)).sum::<f32>() / 8.0;
        let crest = (1..12).map(|k| at(ring * k as f32 / 6.0)).fold(f32::MIN, f32::max);
        assert!(crest > 0.02 * s.params.depth, "no ring: {crest}");
    }

    /// The water field: three values per open cell, NaN in walls.
    #[test]
    fn the_water_field_has_height_and_slope() {
        let scene = presets::dam_break();
        let mut s = ShallowWater::with_params(ShallowParams { cells_y: 64, ..ShallowParams::default() });
        s.reset(&scene, Init::FromScene);
        let mut out = Vec::new();
        let layout = s.fill_field(0, &mut out).unwrap();
        assert_eq!(out.len(), 3 * layout.nx * layout.ny);
        assert!(out[0].is_nan(), "the border is a wall");
        let k = 3 * ((0.3 / s.h) as usize * layout.nx + (0.3 / s.h) as usize);
        assert!((out[k] - BLOCK_RISE).abs() < 1e-4, "inside the block: {}", out[k]);
    }
}
