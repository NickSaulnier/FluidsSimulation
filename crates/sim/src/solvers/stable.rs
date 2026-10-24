//! Stable fluids (Stam 1999) on the MAC grid: one fluid filling the whole domain, with
//! a dye that carries buoyancy. There are no particles; the renderer shows the dye.
//!
//! Per substep: cell types (open boundaries) -> brushes -> buoyancy -> vorticity
//! confinement -> semi-Lagrangian advection of velocity (RK2 backtrace) -> pressure
//! projection -> MacCormack advection of the dye through the projected velocity.
//!
//! Open boundaries: drains are outlets (p = 0, like air, and dye leaving through them
//! is removed). Emitter nozzles are boundary cells whose faces carry the jet velocity:
//! against a wall that's a true inflow (the wind tunnel), in open fluid a pump that
//! pushes out as much as it draws in (the faucet), so a closed box stays solvable.
//!
//! In a closed, all-fluid domain plain gravity is a gradient that the projection removes
//! completely, so gravity acts through the dye instead (Boussinesq buoyancy). Heavy dye
//! sinks and spreads as a gravity current, which makes the dam-break presets collapse
//! like dam breaks; negative buoyancy gives rising smoke.

use super::pressure::{self, Method, PressureSolver};
use crate::grid::Grid2;
use crate::grid::mac::{CENTRE_OFFSET, CellType, MacGrid, Stencil, U_OFFSET, V_OFFSET};
use crate::params::{FloatSpec, IntSpec, ParamVisitor, ToggleSpec};
use crate::scene::{Scene, StepInput};
use crate::solver::{FieldInfo, FieldKind, FieldLayout, Init, Metric, ParticleView, Solver, Stats};
use crate::timing::PhaseTimer;
use glam::Vec2;
use rayon::prelude::*;

#[derive(Clone, Debug)]
pub struct StableParams {
    pub cells_y: u32,
    /// Gauss-Seidel sweeps, or the maximum CG iterations with `multigrid`.
    pub pressure_iters: u32,
    pub over_relax: f32,
    /// Multigrid-preconditioned CG to `MGPCG_TOLERANCE` instead of fixed Gauss-Seidel.
    pub multigrid: bool,
    /// Boussinesq coefficient: acceleration along gravity per unit dye, as a fraction of
    /// g. Positive sinks (heavy ink), negative rises (warm smoke).
    pub buoyancy: f32,
    /// Vorticity confinement strength.
    pub vorticity: f32,
    /// Fraction of dye lost per second.
    pub dissipation: f32,
    /// Max travel per substep, in cells. Semi-Lagrangian advection is unconditionally
    /// stable, so this is about accuracy, not stability.
    pub cfl: f32,
}

impl Default for StableParams {
    fn default() -> Self {
        Self {
            cells_y: 128,
            pressure_iters: 60,
            over_relax: 1.9,
            multigrid: true,
            buoyancy: 0.5,
            vorticity: 0.15,
            dissipation: 0.0,
            cfl: 3.0,
        }
    }
}

const CELLS_Y: IntSpec = IntSpec {
    key: "cells_y",
    label: "Grid cells (height)",
    range: 32..=256,
    restart: true,
    help: "Resolution of the velocity, pressure and dye grids.",
};
const PRESSURE_ITERS: IntSpec = IntSpec {
    key: "pressure_iters",
    label: "Pressure iterations",
    range: 1..=500,
    restart: false,
    help: "Red-black Gauss-Seidel sweeps per substep (warm-started), or the CG iteration cap.",
};
const MULTIGRID: ToggleSpec = ToggleSpec {
    key: "multigrid",
    label: "Multigrid CG (MGPCG)",
    help: "Solve pressure by multigrid-preconditioned CG to a tolerance instead of fixed Gauss-Seidel sweeps.",
};
/// MGPCG stopping tolerance: largest unbalanced flux, m/s.
const MGPCG_TOLERANCE: f32 = 0.005;
const OVER_RELAX: FloatSpec = FloatSpec {
    key: "over_relax",
    label: "Over-relaxation",
    range: 1.0..=1.98,
    log: false,
    restart: false,
    help: "SOR factor.",
};
const BUOYANCY: FloatSpec = FloatSpec {
    key: "buoyancy",
    label: "Dye heaviness",
    range: -1.0..=1.0,
    log: false,
    restart: false,
    help: "Positive: dye is heavy and sinks (ink). Negative: dye is light and rises (smoke).",
};
const VORTICITY: FloatSpec = FloatSpec {
    key: "vorticity",
    label: "Vorticity confinement",
    range: 0.0..=1.0,
    log: false,
    restart: false,
    help: "Re-injects the small swirls that semi-Lagrangian advection smears out.",
};
const DISSIPATION: FloatSpec = FloatSpec {
    key: "dissipation",
    label: "Dye fade (1/s)",
    range: 0.0..=1.0,
    log: false,
    restart: false,
    help: "Fraction of the dye that fades away per second.",
};
const CFL: FloatSpec = FloatSpec {
    key: "cfl",
    label: "CFL (cells per substep)",
    range: 0.5..=8.0,
    log: true,
    restart: false,
    help: "Max travel per substep. Advection is stable at any CFL; lower is more accurate.",
};

/// Water density, for reporting pressure in pascals.
const RHO: f32 = 1000.0;

const FIELDS: &[FieldInfo] = &[
    FieldInfo { key: "dye", label: "Dye", unit: "", kind: FieldKind::Concentration },
    FieldInfo { key: "speed", label: "Speed", unit: "m/s", kind: FieldKind::NonNegative },
    FieldInfo { key: "vorticity", label: "Vorticity", unit: "1/s", kind: FieldKind::Signed },
    FieldInfo { key: "pressure", label: "Pressure", unit: "Pa", kind: FieldKind::Signed },
];

pub struct StableFluids {
    pub params: StableParams,
    grid: MacGrid,
    u_next: Grid2<f32>,
    v_next: Grid2<f32>,
    dye: Grid2<f32>,
    dye_next: Grid2<f32>,
    dye_back: Grid2<f32>,
    curl: Grid2<f32>,
    /// Nozzle cells this step, with their jet velocity.
    inlets: Vec<(usize, usize, Vec2)>,
    solid: Grid2<bool>,
    solid_generation: Option<u64>,
    pressure: Grid2<f32>,
    pressure_solver: PressureSolver,
    stats: Stats,
}

impl Default for StableFluids {
    fn default() -> Self {
        Self::with_params(StableParams::default())
    }
}

impl StableFluids {
    pub fn with_params(params: StableParams) -> Self {
        let mut s = Self {
            params,
            grid: MacGrid::new(2, 2, 1.0),
            u_next: Grid2::new(3, 2),
            v_next: Grid2::new(2, 3),
            dye: Grid2::new(2, 2),
            dye_next: Grid2::new(2, 2),
            dye_back: Grid2::new(2, 2),
            curl: Grid2::new(2, 2),
            inlets: Vec::new(),
            solid: Grid2::new(2, 2),
            solid_generation: None,
            pressure: Grid2::new(2, 2),
            pressure_solver: PressureSolver::new(2, 2),
            stats: Stats::default(),
        };
        s.resize(Vec2::new(2.0, 1.0));
        s
    }

    fn resize(&mut self, size: Vec2) {
        let ny = self.params.cells_y.max(8) as usize;
        let h = size.y / ny as f32;
        let nx = ((size.x / h).round() as usize).max(8);
        self.grid = MacGrid::new(nx, ny, h);
        self.u_next = Grid2::new(nx + 1, ny);
        self.v_next = Grid2::new(nx, ny + 1);
        self.dye = Grid2::new(nx, ny);
        self.dye_next = Grid2::new(nx, ny);
        self.dye_back = Grid2::new(nx, ny);
        self.curl = Grid2::new(nx, ny);
        self.solid = Grid2::new(nx, ny);
        self.solid_generation = None;
        self.pressure = Grid2::new(nx, ny);
        self.pressure_solver = PressureSolver::new(nx, ny);
    }

    /// Cell types for this step. Every non-solid cell is fluid (there is no free
    /// surface), except drains (air: p = 0, dye removed) and emitter nozzles (solid,
    /// with the jet velocity on their faces; see `set_inlet_faces`).
    fn update_cells(&mut self, scene: &Scene, input: &StepInput) {
        if self.solid_generation != Some(scene.solid.generation()) {
            super::resample_solids(scene, self.grid.h, &mut self.solid);
            self.solid_generation = Some(scene.solid.generation());
        }
        let h = self.grid.h;
        let emitters: Vec<_> = scene.emitters.iter().chain(&input.emitters).collect();
        self.inlets.clear();
        for j in 0..self.grid.ny {
            for i in 0..self.grid.nx {
                let c = self.centre(i, j);
                let nozzle = emitters.iter().find_map(|e| super::nozzle_offset(e, h, c).map(|across| (e, across)));
                let kind = if self.solid[(i, j)] {
                    self.dye[(i, j)] = 0.0;
                    CellType::Solid
                } else if let Some((e, across)) = nozzle {
                    self.dye[(i, j)] = super::nozzle_dye(e, across);
                    self.inlets.push((i, j, e.dir.normalize_or(Vec2::NEG_Y) * e.speed));
                    CellType::Solid
                } else if scene.drains.iter().any(|d| d.contains(c)) {
                    self.dye[(i, j)] = 0.0;
                    CellType::Air
                } else {
                    CellType::Fluid
                };
                self.grid.cell[(i, j)] = kind;
            }
        }
    }

    /// Every face of a nozzle cell carries the jet velocity. The projection never
    /// changes faces touching solids, so these act as prescribed-flux boundaries.
    fn set_inlet_faces(&mut self) {
        let g = &mut self.grid;
        for &(i, j, v) in &self.inlets {
            (g.u[(i, j)], g.u[(i + 1, j)]) = (v.x, v.x);
            (g.v[(i, j)], g.v[(i, j + 1)]) = (v.y, v.y);
        }
    }

    fn centre(&self, i: usize, j: usize) -> Vec2 {
        (Vec2::new(i as f32, j as f32) + 0.5) * self.grid.h
    }

    /// Brushes blend face velocities toward theirs.
    fn apply_brushes(&mut self, input: &StepInput, dt: f32) {
        let h = self.grid.h;
        for b in &input.brushes {
            let blend = |p: Vec2, current: f32, target: f32| {
                let d = p.distance(b.pos);
                if d >= b.radius {
                    return current;
                }
                current + (target - current) * (b.strength * (1.0 - d / b.radius) * dt).min(1.0)
            };
            let (nx, ny) = (self.grid.nx, self.grid.ny);
            for j in 0..ny {
                for i in 0..=nx {
                    let p = (Vec2::new(i as f32, j as f32) + U_OFFSET) * h;
                    self.grid.u[(i, j)] = blend(p, self.grid.u[(i, j)], b.velocity.x);
                }
            }
            for j in 0..=ny {
                for i in 0..nx {
                    let p = (Vec2::new(i as f32, j as f32) + V_OFFSET) * h;
                    self.grid.v[(i, j)] = blend(p, self.grid.v[(i, j)], b.velocity.y);
                }
            }
        }
    }

    /// Boussinesq: each face accelerates along gravity in proportion to the dye on it.
    fn apply_buoyancy(&mut self, gravity: Vec2, dt: f32) {
        let k = self.params.buoyancy * dt;
        if k == 0.0 {
            return;
        }
        let (nx, ny) = (self.grid.nx, self.grid.ny);
        let dye = &self.dye;
        let at = |i: isize, j: isize| {
            if i < 0 || j < 0 || i >= nx as isize || j >= ny as isize { 0.0 } else { dye[(i as usize, j as usize)] }
        };
        self.grid.u.par_rows_mut().enumerate().for_each(|(j, row)| {
            for (i, u) in row.iter_mut().enumerate() {
                let (i, j) = (i as isize, j as isize);
                *u += k * gravity.x * 0.5 * (at(i - 1, j) + at(i, j));
            }
        });
        self.grid.v.par_rows_mut().enumerate().for_each(|(j, row)| {
            for (i, v) in row.iter_mut().enumerate() {
                let (i, j) = (i as isize, j as isize);
                *v += k * gravity.y * 0.5 * (at(i, j - 1) + at(i, j));
            }
        });
    }

    /// Fedkiw et al. 2001: push along N × ω, N = ∇|ω| normalised, to restore swirls that
    /// numerical diffusion removes.
    fn confine_vorticity(&mut self, dt: f32) {
        let eps = self.params.vorticity;
        if eps == 0.0 {
            return;
        }
        let g = &self.grid;
        let (nx, ny, h) = (g.nx, g.ny, g.h);
        let uc = |i: usize, j: usize| 0.5 * (g.u[(i, j)] + g.u[(i + 1, j)]);
        let vc = |i: usize, j: usize| 0.5 * (g.v[(i, j)] + g.v[(i, j + 1)]);
        self.curl.par_rows_mut().enumerate().for_each(|(j, row)| {
            for (i, w) in row.iter_mut().enumerate() {
                *w = if i == 0 || j == 0 || i + 1 == nx || j + 1 == ny {
                    0.0
                } else {
                    (vc(i + 1, j) - vc(i - 1, j) - uc(i, j + 1) + uc(i, j - 1)) / (2.0 * h)
                };
            }
        });
        let curl = &self.curl;
        let force = |i: usize, j: usize| -> Vec2 {
            if i < 2 || j < 2 || i + 2 >= nx || j + 2 >= ny {
                return Vec2::ZERO;
            }
            let grad = Vec2::new(
                curl[(i + 1, j)].abs() - curl[(i - 1, j)].abs(),
                curl[(i, j + 1)].abs() - curl[(i, j - 1)].abs(),
            );
            let n = grad.normalize_or_zero();
            let w = curl[(i, j)];
            eps * h * Vec2::new(n.y * w, -n.x * w)
        };
        let (u, v) = (&mut self.grid.u, &mut self.grid.v);
        u.par_rows_mut().enumerate().for_each(|(j, row)| {
            for (i, u) in row.iter_mut().enumerate().skip(1).take(nx - 1) {
                *u += dt * 0.5 * (force(i - 1, j).x + force(i, j).x);
            }
        });
        v.par_rows_mut().enumerate().skip(1).take(ny - 1).for_each(|(j, row)| {
            for (i, v) in row.iter_mut().enumerate() {
                *v += dt * 0.5 * (force(i, j - 1).y + force(i, j).y);
            }
        });
    }

    /// Semi-Lagrangian advection of both velocity components through themselves, with a
    /// midpoint (RK2) backtrace.
    fn advect_velocity(&mut self, dt: f32) {
        let g = &self.grid;
        let h = g.h;
        let vel = |p: Vec2| {
            Vec2::new(Stencil::on(&g.u, h, U_OFFSET, p).sample(&g.u), Stencil::on(&g.v, h, V_OFFSET, p).sample(&g.v))
        };
        let back = |p: Vec2| {
            let mid = p - 0.5 * dt * vel(p);
            p - dt * vel(mid)
        };
        self.u_next.par_rows_mut().enumerate().for_each(|(j, row)| {
            for (i, out) in row.iter_mut().enumerate() {
                let p = back((Vec2::new(i as f32, j as f32) + U_OFFSET) * h);
                *out = Stencil::on(&g.u, h, U_OFFSET, p).sample(&g.u);
            }
        });
        self.v_next.par_rows_mut().enumerate().for_each(|(j, row)| {
            for (i, out) in row.iter_mut().enumerate() {
                let p = back((Vec2::new(i as f32, j as f32) + V_OFFSET) * h);
                *out = Stencil::on(&g.v, h, V_OFFSET, p).sample(&g.v);
            }
        });
        std::mem::swap(&mut self.grid.u, &mut self.u_next);
        std::mem::swap(&mut self.grid.v, &mut self.v_next);
    }

    /// MacCormack (see `super::maccormack`): plain semi-Lagrangian blurs the dye.
    fn advect_dye(&mut self, dt: f32) {
        let g = &self.grid;
        let h = g.h;
        let velocity = |q: Vec2| {
            Vec2::new(Stencil::on(&g.u, h, U_OFFSET, q).sample(&g.u), Stencil::on(&g.v, h, V_OFFSET, q).sample(&g.v))
        };
        let frozen = |i: usize, j: usize| g.cell[(i, j)] == CellType::Solid;
        super::maccormack(&mut self.dye, &mut self.dye_next, &mut self.dye_back, h, dt, velocity, frozen);
        let keep = (1.0 - self.params.dissipation * dt).clamp(0.0, 1.0);
        if keep < 1.0 {
            for (d, c) in self.dye.as_mut_slice().iter_mut().zip(self.grid.cell.as_slice()) {
                if *c != CellType::Solid {
                    *d *= keep;
                }
            }
        }
    }

    fn max_speed(&self) -> f32 {
        let max_abs = |g: &Grid2<f32>| g.as_slice().iter().fold(0.0f32, |m, &x| m.max(x.abs()));
        max_abs(&self.grid.u).max(max_abs(&self.grid.v))
    }
}

impl Solver for StableFluids {
    fn name(&self) -> &'static str {
        "Stable fluids (dye)"
    }

    fn reset(&mut self, scene: &Scene, init: Init<'_>) {
        self.resize(scene.size);
        self.update_cells(scene, &StepInput::default());
        let (nx, ny, h) = (self.grid.nx, self.grid.ny, self.grid.h);
        match init {
            Init::FromScene => {
                for j in 0..ny {
                    for i in 0..nx {
                        let c = self.centre(i, j);
                        let inside = scene.fluid_blocks.iter().any(|b| b.contains(c));
                        self.dye[(i, j)] = if inside && !self.solid[(i, j)] { 1.0 } else { 0.0 };
                    }
                }
            }
            // Another solver's particles become dye: their density per cell, relative to
            // a densely filled cell, capped at 1.
            Init::FromParticles(view) => {
                let per_cell = (h / (2.0 * view.radius)).powi(2);
                for &p in view.pos {
                    let s = Stencil::on(&self.dye, h, CENTRE_OFFSET, p);
                    for (c, w) in s.corners().into_iter().zip(s.w) {
                        self.dye[c] += w / per_cell;
                    }
                }
                self.dye.as_mut_slice().iter_mut().for_each(|d| *d = d.min(1.0));
            }
        }
        self.stats = Stats::default();
    }

    fn max_dt(&self, _scene: &Scene) -> f32 {
        self.params.cfl * self.grid.h / self.max_speed().max(1e-3)
    }

    fn step(&mut self, dt: f32, scene: &Scene, input: &StepInput) {
        let mut timer = PhaseTimer::start();
        self.update_cells(scene, input);
        self.apply_brushes(input, dt);
        self.apply_buoyancy(scene.gravity, dt);
        self.confine_vorticity(dt);
        self.grid.zero_solid_faces();
        self.set_inlet_faces();
        timer.lap("forces");

        self.advect_velocity(dt);
        self.grid.zero_solid_faces();
        self.set_inlet_faces();
        timer.lap("advect velocity");

        let p = &self.params;
        let method = if p.multigrid {
            Method::Pcg { max_iterations: p.pressure_iters, tolerance: MGPCG_TOLERANCE, non_negative: false, multigrid: true }
        } else {
            Method::GaussSeidel { iterations: p.pressure_iters, over_relax: p.over_relax, non_negative: false }
        };
        let no_density = &self.dye; // unused without drift compensation
        pressure::project(&mut self.grid, &mut self.pressure_solver, &mut self.pressure, no_density, dt, method, None);
        let residual = pressure::max_residual(&self.grid, &self.pressure, false);
        timer.lap("pressure");

        self.advect_dye(dt);
        timer.lap("advect dye");

        self.stats.phases = timer.finish();
        self.stats.metrics = vec![
            Metric { name: "pressure residual (m/s)", value: residual, tolerance: Some(0.3) },
            Metric { name: "total dye (cells)", value: self.dye.as_slice().iter().sum(), tolerance: None },
        ];
    }

    fn visit_params(&mut self, v: &mut dyn ParamVisitor) {
        let p = &mut self.params;
        v.section("Dye");
        v.float(&BUOYANCY, &mut p.buoyancy);
        v.float(&DISSIPATION, &mut p.dissipation);
        v.float(&VORTICITY, &mut p.vorticity);
        v.section("Solver");
        v.toggle(&MULTIGRID, &mut p.multigrid);
        v.int(&PRESSURE_ITERS, &mut p.pressure_iters);
        v.float(&OVER_RELAX, &mut p.over_relax);
        v.float(&CFL, &mut p.cfl);
        v.section("Grid");
        v.int(&CELLS_Y, &mut p.cells_y);
    }

    fn particles(&self) -> Option<ParticleView<'_>> {
        None
    }

    fn fields(&self) -> &'static [FieldInfo] {
        FIELDS
    }

    fn fill_field(&self, index: usize, out: &mut Vec<f32>) -> Option<FieldLayout> {
        let g = &self.grid;
        let (nx, ny, h) = (g.nx, g.ny, g.h);
        out.clear();
        out.resize(nx * ny, f32::NAN);
        let key = FIELDS.get(index)?.key;
        for j in 0..ny {
            for i in 0..nx {
                if self.solid[(i, j)] {
                    continue;
                }
                out[j * nx + i] = match key {
                    "dye" => self.dye[(i, j)],
                    "speed" => {
                        let u = 0.5 * (g.u[(i, j)] + g.u[(i + 1, j)]);
                        let v = 0.5 * (g.v[(i, j)] + g.v[(i, j + 1)]);
                        Vec2::new(u, v).length()
                    }
                    "vorticity" => self.curl[(i, j)],
                    _ => self.pressure[(i, j)] * RHO * h,
                };
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
    use crate::scene::Rect;
    use crate::stepper::Stepper;

    fn dye_centroid(s: &StableFluids) -> Vec2 {
        let (mut sum, mut weight) = (Vec2::ZERO, 0.0);
        for j in 0..s.grid.ny {
            for i in 0..s.grid.nx {
                let d = s.dye[(i, j)];
                sum += s.centre(i, j) * d;
                weight += d;
            }
        }
        sum / weight
    }

    fn run(buoyancy: f32, block: Rect, seconds: f32) -> (StableFluids, Vec2, Vec2) {
        let mut scene = Scene::empty_box(Vec2::new(2.0, 1.0), 128);
        scene.fluid_blocks.push(block);
        let mut s = StableFluids::with_params(StableParams { cells_y: 64, buoyancy, ..StableParams::default() });
        s.reset(&scene, Init::FromScene);
        let before = dye_centroid(&s);
        let stepper = Stepper::default();
        for _ in 0..(seconds * 60.0) as usize {
            stepper.advance(&mut s, &scene, 1.0 / 60.0, &StepInput::default());
        }
        let after = dye_centroid(&s);
        (s, before, after)
    }

    /// Heavy dye released at the left wall slumps and runs along the floor: a gravity
    /// current, the all-fluid analogue of a dam break.
    #[test]
    fn heavy_dye_slumps_into_a_gravity_current() {
        let (s, before, after) = run(0.5, Rect::new(Vec2::ZERO, Vec2::new(0.6, 0.8)), 2.0);
        assert!(after.y < before.y - 0.05, "centroid {before} -> {after}");
        assert!(after.x > before.x + 0.1, "should spread along the floor: {before} -> {after}");
        assert!(s.dye.as_slice().iter().all(|d| d.is_finite()));
    }

    /// Light dye (smoke) rises.
    #[test]
    fn light_dye_rises() {
        let (_, before, after) = run(-0.5, Rect::new(Vec2::new(0.8, 0.0), Vec2::new(1.2, 0.3)), 1.5);
        assert!(after.y > before.y + 0.05, "centroid {before} -> {after}");
    }

    /// Open boundaries: the wind tunnel's inlet (nozzle against the wall) and outlet
    /// (drain, p = 0) carry a through-flow. Without them a closed box can only
    /// recirculate, and the flow at mid-tunnel would average zero.
    #[test]
    fn wind_tunnel_flows_through() {
        let scene = crate::presets::wind_tunnel();
        let mut s = StableFluids::with_params(StableParams { cells_y: 64, ..StableParams::default() });
        s.reset(&scene, Init::FromScene);
        let stepper = Stepper::default();
        for _ in 0..120 {
            stepper.advance(&mut s, &scene, 1.0 / 60.0, &StepInput::default());
        }
        let g = &s.grid;
        let i = (1.5 / g.h) as usize;
        let open: Vec<f32> = (0..g.ny).filter(|&j| g.cell[(i, j)] == CellType::Fluid).map(|j| g.u[(i, j)]).collect();
        let mean = open.iter().sum::<f32>() / open.len() as f32;
        assert!((0.8..1.2).contains(&mean), "mean flow across x = 1.5 m is {mean} m/s (inlet 1 m/s)");
        assert!(s.stats.metrics[0].value < 0.05, "residual {}", s.stats.metrics[0].value);
    }

    /// The projected velocity is divergence-free (it's a closed, all-fluid domain, so
    /// there's no free surface to excuse expansion).
    #[test]
    fn velocity_stays_divergence_free() {
        let (s, _, _) = run(0.5, Rect::new(Vec2::ZERO, Vec2::new(0.6, 0.8)), 1.0);
        let residual = s.stats.metrics[0].value;
        assert!(residual < 0.05, "max residual {residual} m/s");
    }
}
