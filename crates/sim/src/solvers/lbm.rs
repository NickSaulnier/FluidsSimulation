//! Lattice Boltzmann (D2Q9): nine particle-distribution populations per cell stream to
//! their neighbours and relax toward equilibrium. There is no pressure solve; pressure
//! is the density (p = ρ c_s²) of a weakly compressible fluid.
//!
//! - BGK collision with a Smagorinsky subgrid model: the local relaxation time grows
//!   with the non-equilibrium stress, which keeps low-viscosity flow stable.
//! - Pull streaming fused with collision, double-buffered, so each row writes only its
//!   own cells (row-parallel without `unsafe`).
//! - Half-way bounce-back at solid cells (no-slip walls between cells).
//! - Forces by the exact-difference method: Δf = f_eq(ρ, u + Δu) - f_eq(ρ, u).
//! - Emitters are velocity inlets, drains are pressure (ρ = 1) outlets, so LBM can run
//!   open flows such as the wind tunnel.
//! - A dye advected through the LBM velocity carries Boussinesq buoyancy, as in the
//!   stable-fluids solver, and is what gets drawn. It uses MacCormack advection, so
//!   the wind tunnel's streaklines stay sharp.
//!
//! Lattice units inside: one cell, one lattice step. Physical speeds map to lattice
//! speeds by `lattice_per_mps`, kept well below the lattice sound speed (1/√3), so each
//! frame sub-cycles many lattice steps.

use crate::grid::Grid2;
use crate::grid::mac::{CENTRE_OFFSET, Stencil};
use crate::params::{FloatSpec, IntSpec, ParamVisitor};
use crate::scene::{Scene, StepInput};
use crate::solver::{FieldInfo, FieldKind, FieldLayout, Init, Metric, ParticleView, Solver, Stats};
use crate::timing::PhaseTimer;
use glam::Vec2;
use rayon::prelude::*;

/// D2Q9 lattice: rest, 4 axis, 4 diagonal directions.
const C: [(i32, i32); 9] = [(0, 0), (1, 0), (0, 1), (-1, 0), (0, -1), (1, 1), (-1, 1), (-1, -1), (1, -1)];
const W: [f32; 9] = [4.0 / 9.0, 1.0 / 9.0, 1.0 / 9.0, 1.0 / 9.0, 1.0 / 9.0, 1.0 / 36.0, 1.0 / 36.0, 1.0 / 36.0, 1.0 / 36.0];
const OPPOSITE: [usize; 9] = [0, 3, 4, 1, 2, 7, 8, 5, 6];
/// Lattice speeds above this are clamped: a last-ditch guard, beyond normal flow.
const MAX_LATTICE_SPEED: f32 = 0.3;
/// Inlets and brushes drive at most this lattice speed (Mach 0.35).
const MAX_SOURCE_SPEED: f32 = 0.2;

type Populations = [f32; 9];

/// Body forces for one frame, already in lattice units (velocity change per step).
struct Forcing {
    /// Per unit dye.
    buoyancy: Vec2,
    /// (centre, radius, lattice velocity, lattice spin (per metre of offset), blend
    /// fraction per step).
    brushes: Vec<(Vec2, f32, Vec2, f32, f32)>,
}

impl Forcing {
    fn du(&self, centre: Vec2, dye: f32, u: Vec2) -> Vec2 {
        let mut du = self.buoyancy * dye;
        for &(pos, radius, velocity, spin, rate) in &self.brushes {
            let d = centre.distance(pos);
            if d < radius {
                let target = (velocity + spin * (centre - pos).perp()).clamp_length_max(MAX_SOURCE_SPEED);
                du += (target - u) * rate * (1.0 - d / radius);
            }
        }
        du
    }
}

#[derive(Clone, Debug)]
pub struct LbmParams {
    pub cells_y: u32,
    /// Base BGK relaxation time; kinematic viscosity is (τ - 1/2) / 3 in lattice units.
    pub tau: f32,
    /// Smagorinsky constant (0 = plain BGK).
    pub smagorinsky: f32,
    /// Lattice speed per m/s. Smaller is more accurate (lower Mach) but needs more
    /// lattice steps per frame.
    pub lattice_per_mps: f32,
    pub max_steps_per_frame: u32,
    /// Boussinesq coefficient for the dye, as in stable fluids: + sinks, - rises.
    pub buoyancy: f32,
}

impl Default for LbmParams {
    fn default() -> Self {
        Self { cells_y: 128, tau: 0.52, smagorinsky: 0.15, lattice_per_mps: 0.05, max_steps_per_frame: 300, buoyancy: 0.5 }
    }
}

const CELLS_Y: IntSpec = IntSpec {
    key: "cells_y",
    label: "Grid cells (height)",
    range: 32..=256,
    restart: true,
    help: "Lattice resolution. Lattice steps per second grow with it too.",
};
const TAU: FloatSpec = FloatSpec {
    key: "tau",
    label: "Relaxation time τ",
    range: 0.501..=1.5,
    log: true,
    restart: false,
    help: "Sets the viscosity: ν = (τ - ½)/3 lattice units. Near ½ is thin and lively (and less stable).",
};
const SMAGORINSKY: FloatSpec = FloatSpec {
    key: "smagorinsky",
    label: "Smagorinsky constant",
    range: 0.0..=0.3,
    log: false,
    restart: false,
    help: "Subgrid turbulence model: adds viscosity where the flow is sheared, for stability.",
};
const LATTICE_PER_MPS: FloatSpec = FloatSpec {
    key: "lattice_per_mps",
    label: "Lattice speed per m/s",
    range: 0.01..=0.15,
    log: true,
    restart: true,
    help: "Lower = more accurate (lower Mach number) but more lattice steps per frame.",
};
const MAX_STEPS: IntSpec = IntSpec {
    key: "max_steps_per_frame",
    label: "Max lattice steps / frame",
    range: 10..=1000,
    restart: false,
    help: "Above this the simulation runs in slow motion rather than dropping frames.",
};
const BUOYANCY: FloatSpec = FloatSpec {
    key: "buoyancy",
    label: "Dye heaviness",
    range: -1.0..=1.0,
    log: false,
    restart: false,
    help: "Positive: dye is heavy and sinks. Negative: it rises. 0: passive tracer.",
};

const FIELDS: &[FieldInfo] = &[
    FieldInfo { key: "dye", label: "Dye", unit: "", kind: FieldKind::Concentration },
    FieldInfo { key: "vorticity", label: "Vorticity", unit: "1/s", kind: FieldKind::Signed },
    FieldInfo { key: "speed", label: "Speed", unit: "m/s", kind: FieldKind::NonNegative },
    FieldInfo { key: "density", label: "Density deviation (ρ - 1)", unit: "", kind: FieldKind::Signed },
];

#[inline]
fn equilibrium(rho: f32, u: Vec2) -> Populations {
    let usq = 1.5 * u.length_squared();
    std::array::from_fn(|q| {
        let cu = 3.0 * (C[q].0 as f32 * u.x + C[q].1 as f32 * u.y);
        W[q] * rho * (1.0 + cu + 0.5 * cu * cu - usq)
    })
}

#[inline]
fn moments(f: &Populations) -> (f32, Vec2) {
    let mut rho = 0.0;
    let mut m = Vec2::ZERO;
    for q in 0..9 {
        rho += f[q];
        m += Vec2::new(C[q].0 as f32, C[q].1 as f32) * f[q];
    }
    (rho, m / rho.max(1e-6))
}

pub struct Lbm {
    pub params: LbmParams,
    nx: usize,
    ny: usize,
    h: f32,
    /// Post-collision populations, and the buffer the next step writes.
    f: Grid2<Populations>,
    f_next: Grid2<Populations>,
    rho: Grid2<f32>,
    /// Velocity in lattice units.
    vel: Grid2<Vec2>,
    dye: Grid2<f32>,
    dye_next: Grid2<f32>,
    dye_back: Grid2<f32>,
    solid: Grid2<bool>,
    solid_generation: Option<u64>,
    /// Per-cell boundary role, rebuilt every frame from the scene and input.
    role: Grid2<Role>,
    stats: Stats,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Role {
    #[default]
    Fluid,
    Solid,
    /// Velocity inlet with this lattice velocity (emitter nozzle).
    Inlet(Vec2),
    /// Pressure outlet (drain).
    Outlet,
}

impl Default for Lbm {
    fn default() -> Self {
        Self::with_params(LbmParams::default())
    }
}

impl Lbm {
    pub fn with_params(params: LbmParams) -> Self {
        let mut s = Self {
            params,
            nx: 2,
            ny: 2,
            h: 1.0,
            f: Grid2::new(2, 2),
            f_next: Grid2::new(2, 2),
            rho: Grid2::new(2, 2),
            vel: Grid2::new(2, 2),
            dye: Grid2::new(2, 2),
            dye_next: Grid2::new(2, 2),
            dye_back: Grid2::new(2, 2),
            solid: Grid2::new(2, 2),
            solid_generation: None,
            role: Grid2::new(2, 2),
            stats: Stats::default(),
        };
        s.resize(Vec2::new(2.0, 1.0));
        s
    }

    fn resize(&mut self, size: Vec2) {
        let ny = self.params.cells_y.max(8) as usize;
        let h = size.y / ny as f32;
        let nx = ((size.x / h).round() as usize).max(8);
        (self.nx, self.ny, self.h) = (nx, ny, h);
        let rest = equilibrium(1.0, Vec2::ZERO);
        self.f = Grid2::new(nx, ny);
        self.f.fill(rest);
        self.f_next = self.f.clone();
        self.rho = Grid2::new(nx, ny);
        self.rho.fill(1.0);
        self.vel = Grid2::new(nx, ny);
        self.dye = Grid2::new(nx, ny);
        self.dye_next = Grid2::new(nx, ny);
        self.dye_back = Grid2::new(nx, ny);
        self.solid = Grid2::new(nx, ny);
        self.solid_generation = None;
        self.role = Grid2::new(nx, ny);
    }

    /// Seconds per lattice step: `lattice_per_mps` lattice units of speed per m/s.
    fn dt_lattice(&self) -> f32 {
        self.params.lattice_per_mps * self.h
    }

    fn centre(&self, i: usize, j: usize) -> Vec2 {
        (Vec2::new(i as f32, j as f32) + 0.5) * self.h
    }

    /// Solids from the scene; inlets from emitters (scene and faucet); outlets from drains.
    fn update_roles(&mut self, scene: &Scene, input: &StepInput) {
        if self.solid_generation != Some(scene.solid.generation()) {
            super::resample_solids(scene, self.h, &mut self.solid);
            self.solid_generation = Some(scene.solid.generation());
        }
        let to_lattice = self.params.lattice_per_mps;
        let (nx, ny, h) = (self.nx, self.ny, self.h);
        let emitters: Vec<_> = scene.emitters.iter().chain(&input.emitters).collect();
        for j in 0..ny {
            for i in 0..nx {
                let c = self.centre(i, j);
                self.role[(i, j)] = if self.solid[(i, j)] && !super::opens_border(scene, c) {
                    self.dye[(i, j)] = 0.0;
                    Role::Solid
                } else if let Some((e, across)) =
                    emitters.iter().find_map(|e| super::nozzle_offset(e, h, c).map(|across| (e, across)))
                {
                    let u = (e.dir.normalize_or(Vec2::NEG_Y) * e.speed * to_lattice).clamp_length_max(MAX_SOURCE_SPEED);
                    self.dye[(i, j)] = super::nozzle_dye(e, across);
                    Role::Inlet(u)
                } else if scene.drains.iter().any(|d| d.contains(c)) {
                    Role::Outlet
                } else {
                    Role::Fluid
                };
            }
        }
    }

    /// One lattice step: pull-stream, boundary conditions, collide, force; into `f_next`.
    fn lattice_step(&mut self, forcing: &Forcing) {
        let (nx, ny, h) = (self.nx, self.ny, self.h);
        let tau0 = self.params.tau;
        let cs2 = self.params.smagorinsky * self.params.smagorinsky;
        let (f, role, dye) = (&self.f, &self.role, &self.dye);
        self.f_next
            .par_rows_mut()
            .zip(self.rho.par_rows_mut())
            .zip(self.vel.par_rows_mut())
            .enumerate()
            .for_each(|(j, ((out_row, rho_row), vel_row))| {
                for i in 0..nx {
                    let r = role[(i, j)];
                    if r == Role::Solid {
                        out_row[i] = equilibrium(1.0, Vec2::ZERO);
                        rho_row[i] = 1.0;
                        vel_row[i] = Vec2::ZERO;
                        continue;
                    }
                    // Pull: population q arrives from the neighbour at x - c_q; from a
                    // solid neighbour, it's our own opposite population bounced back.
                    let mut pre: Populations = [0.0; 9];
                    for q in 0..9 {
                        let (si, sj) = (i as i32 - C[q].0, j as i32 - C[q].1);
                        let inside = si >= 0 && sj >= 0 && (si as usize) < nx && (sj as usize) < ny;
                        pre[q] = if inside && role[(si as usize, sj as usize)] != Role::Solid {
                            f[(si as usize, sj as usize)][q]
                        } else {
                            f[(i, j)][OPPOSITE[q]]
                        };
                    }
                    let (mut rho, mut u) = moments(&pre);
                    match r {
                        // Dirichlet in both density and velocity: a floating density
                        // would let the inlet suck the fluid behind it dry.
                        Role::Inlet(u_in) => {
                            (rho, u) = (1.0, u_in);
                            pre = equilibrium(rho, u);
                        }
                        Role::Outlet => {
                            rho = 1.0;
                            pre = equilibrium(rho, u);
                        }
                        _ => {}
                    }
                    u = u.clamp_length_max(MAX_LATTICE_SPEED);
                    let feq = equilibrium(rho, u);
                    // Smagorinsky: local relaxation time from the non-equilibrium stress.
                    let (mut pxx, mut pxy, mut pyy) = (0.0, 0.0, 0.0);
                    for q in 0..9 {
                        let neq = pre[q] - feq[q];
                        let (cx, cy) = (C[q].0 as f32, C[q].1 as f32);
                        pxx += cx * cx * neq;
                        pxy += cx * cy * neq;
                        pyy += cy * cy * neq;
                    }
                    let stress = (pxx * pxx + 2.0 * pxy * pxy + pyy * pyy).sqrt();
                    let tau = 0.5 * (tau0 + (tau0 * tau0 + 18.0 * std::f32::consts::SQRT_2 * cs2 * stress / rho).sqrt());
                    let mut post: Populations = std::array::from_fn(|q| pre[q] - (pre[q] - feq[q]) / tau);
                    // Exact-difference forcing.
                    let centre = (Vec2::new(i as f32, j as f32) + 0.5) * h;
                    let du = forcing.du(centre, dye[(i, j)], u);
                    if du != Vec2::ZERO {
                        let shifted = equilibrium(rho, (u + du).clamp_length_max(MAX_LATTICE_SPEED));
                        for q in 0..9 {
                            post[q] += shifted[q] - feq[q];
                        }
                        u += du;
                    }
                    out_row[i] = post;
                    rho_row[i] = rho;
                    vel_row[i] = u;
                }
            });
        std::mem::swap(&mut self.f, &mut self.f_next);
    }

    /// Dye advection through the (physical) LBM velocity. MacCormack rather than plain
    /// semi-Lagrangian: the latter smears the streaklines out within half the tunnel.
    fn advect_dye(&mut self, dt: f32) {
        let (h, scale) = (self.h, 1.0 / self.params.lattice_per_mps);
        let (vel, role) = (&self.vel, &self.role);
        let velocity = |p: Vec2| {
            let s = Stencil::on(vel, h, CENTRE_OFFSET, p);
            s.corners().into_iter().zip(s.w).map(|(c, w)| vel[c] * w).sum::<Vec2>() * scale
        };
        let frozen = |i: usize, j: usize| matches!(role[(i, j)], Role::Solid | Role::Inlet(_));
        super::maccormack(&mut self.dye, &mut self.dye_next, &mut self.dye_back, h, dt, velocity, frozen);
    }

    fn normalise_mass(&mut self) {
        let (mut mass, mut cells) = (0.0f64, 0usize);
        for (f, r) in self.f.as_slice().iter().zip(self.role.as_slice()) {
            if *r != Role::Solid {
                mass += f.iter().map(|&x| f64::from(x)).sum::<f64>();
                cells += 1;
            }
        }
        if mass > 0.0 {
            let k = (cells as f64 / mass) as f32;
            self.f.as_mut_slice().iter_mut().for_each(|f| f.iter_mut().for_each(|x| *x *= k));
        }
    }

    fn vorticity(&self, i: usize, j: usize) -> f32 {
        if i == 0 || j == 0 || i + 1 >= self.nx || j + 1 >= self.ny {
            return 0.0;
        }
        let v = &self.vel;
        let dvdx = v[(i + 1, j)].y - v[(i - 1, j)].y;
        let dudy = v[(i, j + 1)].x - v[(i, j - 1)].x;
        (dvdx - dudy) / (2.0 * self.h) / self.params.lattice_per_mps
    }
}

impl Solver for Lbm {
    fn name(&self) -> &'static str {
        "Lattice Boltzmann (dye)"
    }

    fn reset(&mut self, scene: &Scene, init: Init<'_>) {
        self.resize(scene.size);
        super::resample_solids(scene, self.h, &mut self.solid);
        self.solid_generation = Some(scene.solid.generation());
        let (nx, ny, h) = (self.nx, self.ny, self.h);
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
            // Particles become dye, as in stable fluids.
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

    /// LBM sub-cycles internally, so take the whole frame as one substep.
    fn max_dt(&self, _scene: &Scene) -> f32 {
        1.0
    }

    fn step(&mut self, dt: f32, scene: &Scene, input: &StepInput) {
        let mut timer = PhaseTimer::start();
        self.update_roles(scene, input);
        let dt_lb = self.dt_lattice();
        let steps = ((dt / dt_lb).round() as u32).clamp(1, self.params.max_steps_per_frame);
        // Accelerations (m/s²) to lattice velocity change per lattice step.
        let to_lattice = dt_lb * self.params.lattice_per_mps;
        let buoyancy = scene.gravity * (self.params.buoyancy * to_lattice);
        let brushes = input
            .brushes
            .iter()
            .map(|b| {
                let to_lattice = self.params.lattice_per_mps;
                let velocity = (b.velocity * to_lattice).clamp_length_max(MAX_SOURCE_SPEED);
                (b.pos, b.radius, velocity, b.spin * to_lattice, (b.strength * dt_lb).min(1.0))
            })
            .collect();
        let forcing = Forcing { buoyancy, brushes };
        timer.lap("setup");

        for _ in 0..steps {
            self.lattice_step(&forcing);
        }
        timer.lap("lattice");

        // Without an outlet, inlets (the faucet) would keep compressing a closed box.
        // Rescale to unit mean density instead: a gentle sink spread over every cell.
        if !self.role.as_slice().contains(&Role::Outlet) {
            self.normalise_mass();
        }

        self.advect_dye(steps as f32 * dt_lb);
        timer.lap("dye");

        let max_u = self.vel.as_slice().iter().map(|u| u.length()).fold(0.0, f32::max);
        let max_drho = self.rho.as_slice().iter().map(|r| (r - 1.0).abs()).fold(0.0, f32::max);
        self.stats.phases = timer.finish();
        self.stats.metrics = vec![
            // Mach-number guard: normal flow stays well below the clamp.
            Metric { name: "max lattice speed", value: max_u, tolerance: Some(0.25) },
            // Weak compressibility: density should stay near 1; blow-ups show here first.
            Metric { name: "max |ρ - 1|", value: max_drho, tolerance: Some(0.3) },
            Metric { name: "lattice steps / frame", value: steps as f32, tolerance: None },
        ];
    }

    fn visit_params(&mut self, v: &mut dyn ParamVisitor) {
        let p = &mut self.params;
        v.section("Fluid");
        v.float(&TAU, &mut p.tau);
        v.float(&SMAGORINSKY, &mut p.smagorinsky);
        v.float(&BUOYANCY, &mut p.buoyancy);
        v.section("Resolution");
        v.int(&CELLS_Y, &mut p.cells_y);
        v.advanced_section("Lattice");
        v.float(&LATTICE_PER_MPS, &mut p.lattice_per_mps);
        v.int(&MAX_STEPS, &mut p.max_steps_per_frame);
    }

    fn particles(&self) -> Option<ParticleView<'_>> {
        None
    }

    fn fields(&self) -> &'static [FieldInfo] {
        FIELDS
    }

    fn velocity_field(&self, out: &mut Vec<Vec2>) -> Option<FieldLayout> {
        let (nx, ny) = (self.nx, self.ny);
        let scale = 1.0 / self.params.lattice_per_mps;
        out.clear();
        out.extend((0..ny).flat_map(|j| (0..nx).map(move |i| (i, j))).map(|(i, j)| {
            if self.solid[(i, j)] { Vec2::ZERO } else { self.vel[(i, j)] * scale }
        }));
        Some(FieldLayout { nx, ny, origin: Vec2::ZERO, cell: self.h })
    }

    fn fill_field(&self, index: usize, out: &mut Vec<f32>) -> Option<FieldLayout> {
        let (nx, ny) = (self.nx, self.ny);
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
                    "vorticity" => self.vorticity(i, j),
                    "speed" => self.vel[(i, j)].length() / self.params.lattice_per_mps,
                    _ => self.rho[(i, j)] - 1.0,
                };
            }
        }
        Some(FieldLayout { nx, ny, origin: Vec2::ZERO, cell: self.h })
    }

    fn stats(&self) -> &Stats {
        &self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stepper::Stepper;

    #[test]
    fn equilibrium_reproduces_its_moments() {
        let u = Vec2::new(0.07, -0.03);
        let (rho, v) = moments(&equilibrium(1.2, u));
        assert!((rho - 1.2).abs() < 1e-5 && (v - u).length() < 1e-5, "ρ {rho}, u {v}");
    }

    /// A closed box at rest stays at rest (mass is conserved exactly by bounce-back).
    #[test]
    fn closed_box_at_rest_stays_at_rest() {
        let scene = Scene::empty_box(Vec2::new(1.0, 1.0), 64);
        let mut lbm = Lbm::with_params(LbmParams { cells_y: 48, buoyancy: 0.0, ..LbmParams::default() });
        lbm.reset(&scene, Init::FromScene);
        let mass0: f32 = lbm.f.as_slice().iter().map(|f| f.iter().sum::<f32>()).sum();
        for _ in 0..30 {
            Stepper::default().advance(&mut lbm, &scene, 1.0 / 60.0, &StepInput::default());
        }
        let mass1: f32 = lbm.f.as_slice().iter().map(|f| f.iter().sum::<f32>()).sum();
        assert!(lbm.stats.metrics[0].value < 1e-6, "spurious flow {}", lbm.stats.metrics[0].value);
        assert!((mass1 / mass0 - 1.0).abs() < 1e-4, "mass {mass0} -> {mass1}");
    }

    /// A faucet pouring into a closed box (no outlet) must not compress it without
    /// bound: mass normalisation spreads a sink over the whole box.
    #[test]
    fn faucet_into_closed_box_keeps_density_bounded() {
        let scene = crate::presets::dam_break();
        let mut lbm = Lbm::with_params(LbmParams { cells_y: 64, ..LbmParams::default() });
        lbm.reset(&scene, Init::FromScene);
        let faucet = crate::scene::Emitter { pos: Vec2::new(1.5, 0.8), dir: Vec2::NEG_Y, speed: 2.0, width: 0.06 };
        let input = StepInput { emitters: vec![faucet], ..StepInput::default() };
        for _ in 0..300 {
            Stepper::default().advance(&mut lbm, &scene, 1.0 / 60.0, &input);
        }
        let drho = lbm.stats.metrics[1].value;
        assert!(drho < 0.1, "max |ρ - 1| = {drho} after 5 s of faucet");
        let mean: f32 = lbm.rho.as_slice().iter().sum::<f32>() / lbm.rho.as_slice().len() as f32;
        assert!((mean - 1.0).abs() < 0.01, "mean density {mean}");
    }

    /// Flow past a cylinder above Re ≈ 47 sheds vortices: the cross-stream velocity
    /// behind it oscillates (a von Kármán street) at a Strouhal number f·D/U near the
    /// measured 0.16-0.17 for Re ≈ 90 (Williamson 1996). Here D = 0.12 m, U = 1 m/s and
    /// Re = U·D/ν ≈ 86 at 96 cells.
    #[test]
    fn cylinder_wake_sheds_vortices() {
        let scene = crate::presets::wind_tunnel();
        let mut lbm = Lbm::with_params(LbmParams { cells_y: 96, ..LbmParams::default() });
        lbm.reset(&scene, Init::FromScene);
        let stepper = Stepper::default();
        // Probe a few diameters downstream of the cylinder, on its centreline.
        let probe = ((0.95 / lbm.h) as usize, (0.5 / lbm.h) as usize);
        let mut samples = Vec::new();
        for frame in 0..600 {
            stepper.advance(&mut lbm, &scene, 1.0 / 60.0, &StepInput::default());
            if frame > 300 {
                samples.push(lbm.vel[probe].y);
            }
        }
        let crossings = samples.windows(2).filter(|w| w[0].signum() != w[1].signum()).count();
        let amplitude = samples.iter().map(|v| v.abs()).fold(0.0, f32::max);
        assert!(amplitude > 0.002, "wake barely oscillates: amplitude {amplitude} (lattice units)");
        // Two sign changes per period; samples are 1/60 s apart.
        let frequency = crossings as f32 / 2.0 / (samples.len() as f32 / 60.0);
        let strouhal = frequency * 0.12 / 1.0;
        assert!((0.13..0.21).contains(&strouhal), "Strouhal {strouhal} ({crossings} sign changes)");
        assert!(lbm.stats.metrics.iter().all(|m| m.tolerance.is_none_or(|t| m.value <= t)), "{:?}", lbm.stats.metrics);
    }
}
