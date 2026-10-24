//! Position Based Fluids (Macklin & Müller 2013).
//!
//! Per substep: apply forces and predict positions -> bin neighbours -> for a few
//! iterations, solve per-particle density constraints and move the predictions ->
//! velocity from the position change -> XSPH viscosity -> commit.
//!
//! Every pass is a gather over neighbours (each particle writes only its own entry),
//! the opposite of FLIP's particle-to-grid scatter, so the parallel code is plain
//! `par_iter`. It shares nothing with FLIP except the neighbour bins and the scene.

mod kernels;

use crate::neighbors::CellBins;
use crate::params::{FloatSpec, IntSpec, ParamVisitor};
use crate::scene::{Scene, StepInput};
use crate::solver::{Init, Metric, ParticleView, Solver, Stats};
use crate::solvers::PAR_CHUNK;
use crate::timing::PhaseTimer;
use glam::Vec2;
use kernels::Kernels;
use rayon::prelude::*;

#[derive(Clone, Debug)]
pub struct PbfParams {
    /// Seeding spacing in metres; the kernel radius is `KERNEL_PER_SPACING` times this.
    pub spacing: f32,
    pub iterations: u32,
    /// Constraint softening (the paper's ε), as a fraction of the rest lattice's
    /// constraint stiffness Σ|∇C|². Scale-free, unlike the paper's absolute value.
    pub relaxation: f32,
    /// Artificial pressure (the paper's s_corr), as an equivalent compression at Δq.
    /// Scale-free, unlike the paper's k, whose meaning depends on units and spacing.
    pub tensile_k: f32,
    /// XSPH viscosity.
    pub viscosity: f32,
    /// Largest substep in seconds.
    pub max_dt: f32,
    /// Max particle travel per substep, in kernel radii. Only a safety net: limiting dt
    /// by speed feeds back (smaller dt turns the same position correction into a larger
    /// velocity, which shrinks dt further), so keep it loose.
    pub cfl: f32,
}

impl Default for PbfParams {
    fn default() -> Self {
        // Tuned on a resting tank and the dam break: 1/120 s substeps jitter at 1-3 m/s
        // (position corrections become velocities as dx/dt), 1/240 is calm; s_corr helps
        // at 0.01 and destabilises at 0.1. The Jacobi-style constraint solve only spreads
        // a correction about one kernel radius per iteration, so stability needs
        // iterations proportional to fluid depth in particles: at 0.005 m spacing the
        // dam-break column is 160 particles deep and 4-10 iterations blow up, while
        // 0.01 m with 6 iterations (~5k particles, ~8 ms/frame) is stable. PBF is the
        // coarser solver here.
        Self { spacing: 0.01, iterations: 6, relaxation: 0.01, tensile_k: 0.01, viscosity: 0.01, max_dt: 1.0 / 240.0, cfl: 1.0 }
    }
}

/// Kernel radius in units of the seed spacing: about 12 neighbours in 2D.
const KERNEL_PER_SPACING: f32 = 2.0;
/// s_corr reference distance (Δq) as a fraction of the kernel radius, and exponent n.
const TENSILE_DQ: f32 = 0.2;
const TENSILE_N: i32 = 4;
/// Neighbour-list capacity per particle. A rest lattice has 12 within the kernel; even
/// strongly compressed fluid stays well under this. Extra neighbours are dropped.
const MAX_NEIGHBOURS: usize = 48;
/// Neighbour lists are built once per substep with this margin on the kernel radius, so
/// they stay valid while the constraint iterations move particles a little.
const NEIGHBOUR_MARGIN: f32 = 1.1;

const SPACING: FloatSpec = FloatSpec {
    key: "spacing",
    label: "Particle spacing (m)",
    range: 0.0025..=0.02,
    log: true,
    restart: true,
    help: "Resolution. Particle count grows with 1/spacing².",
};
const ITERATIONS: IntSpec = IntSpec {
    key: "iterations",
    label: "Solver iterations",
    range: 1..=20,
    restart: false,
    help: "Density-constraint iterations per substep. More = stiffer, less compressible.",
};
const RELAXATION: FloatSpec = FloatSpec {
    key: "relaxation",
    label: "Relaxation ε",
    range: 1e-4..=1.0,
    log: true,
    restart: false,
    help: "Softens the constraint solve. Too small can jitter; too large is squishy.",
};
const TENSILE_K: FloatSpec = FloatSpec {
    key: "tensile_k",
    label: "Anti-clumping (s_corr k)",
    range: 0.0..=0.5,
    log: false,
    restart: false,
    help: "Artificial pressure that keeps particles from clustering at the surface.",
};
const VISCOSITY: FloatSpec = FloatSpec {
    key: "viscosity",
    label: "XSPH viscosity",
    range: 0.0..=0.2,
    log: false,
    restart: false,
    help: "Blends each velocity toward its neighbours'.",
};
const MAX_DT: FloatSpec = FloatSpec {
    key: "max_dt",
    label: "Max substep (s)",
    range: 1.0 / 960.0..=1.0 / 60.0,
    log: true,
    restart: false,
    help: "Upper bound on the substep; smaller = more substeps, more stable.",
};

/// Quantities derived from the parameters, shared by every PBF implementation (CPU and
/// GPU) so they are tuned identically.
#[derive(Clone, Copy, Debug)]
pub struct PbfConstants {
    pub kernel_radius: f32,
    /// Density of an interior particle of the rest lattice (kernel units, mass 1).
    pub rest_density: f32,
    /// Σ|∇C|² for an interior lattice particle; the scale for ε and s_corr.
    pub lattice_stiffness: f32,
    /// W(Δq) for the s_corr ratio.
    pub w_dq: f32,
    /// 2D kernel coefficients: poly6 = 4/(π h⁸), spiky gradient = -30/(π h⁵).
    pub poly6: f32,
    pub spiky_grad: f32,
    pub tensile_n: i32,
}

impl PbfParams {
    pub fn constants(&self) -> PbfConstants {
        let h = KERNEL_PER_SPACING * self.spacing;
        let kernels = Kernels::new(h);
        let rest_density = Pbf::lattice_density(&kernels, self.spacing);
        PbfConstants {
            kernel_radius: h,
            rest_density,
            lattice_stiffness: Pbf::lattice_stiffness(&kernels, self.spacing, rest_density),
            w_dq: kernels.w((TENSILE_DQ * h).powi(2)),
            poly6: 4.0 / (std::f32::consts::PI * h.powi(8)),
            spiky_grad: -30.0 / (std::f32::consts::PI * h.powi(5)),
            tensile_n: TENSILE_N,
        }
    }

    /// Walk the parameters (the same list for every PBF implementation).
    pub fn visit(&mut self, v: &mut dyn ParamVisitor) {
        v.section("Fluid");
        v.float(&TENSILE_K, &mut self.tensile_k);
        v.float(&VISCOSITY, &mut self.viscosity);
        v.section("Resolution");
        v.float(&SPACING, &mut self.spacing);
        v.advanced_section("Solver");
        v.int(&ITERATIONS, &mut self.iterations);
        v.float(&RELAXATION, &mut self.relaxation);
        v.float(&MAX_DT, &mut self.max_dt);
    }

    /// Largest stable substep for the given peak particle speed.
    pub fn max_dt(&self, max_speed: f32) -> f32 {
        self.max_dt.min(self.cfl * KERNEL_PER_SPACING * self.spacing / max_speed.max(1e-3))
    }
}

pub struct Pbf {
    pub params: PbfParams,
    kernels: Kernels,
    rest_density: f32,
    /// Σ|∇C|² for an interior particle of the rest lattice: the natural scale of the
    /// constraint denominator, used to make ε and s_corr independent of spacing.
    lattice_stiffness: f32,
    pos: Vec<Vec2>,
    vel: Vec<Vec2>,
    /// Predicted positions during the solve.
    pred: Vec<Vec2>,
    lambda: Vec<f32>,
    delta: Vec<Vec2>,
    density: Vec<f32>,
    bins: CellBins,
    /// Flat neighbour lists: particle i's neighbours are
    /// `neighbours[i * MAX_NEIGHBOURS..][..neighbour_count[i]]`.
    neighbours: Vec<u32>,
    neighbour_count: Vec<u8>,
    emission: super::Emission,
    stats: Stats,
}

impl Default for Pbf {
    fn default() -> Self {
        Self::with_params(PbfParams::default())
    }
}

impl Pbf {
    pub fn with_params(params: PbfParams) -> Self {
        let kernels = Kernels::new(KERNEL_PER_SPACING * params.spacing);
        Self {
            params,
            kernels,
            rest_density: 1.0,
            lattice_stiffness: 1.0,
            pos: Vec::new(),
            vel: Vec::new(),
            pred: Vec::new(),
            lambda: Vec::new(),
            delta: Vec::new(),
            density: Vec::new(),
            bins: CellBins::default(),
            neighbours: Vec::new(),
            neighbour_count: Vec::new(),
            emission: super::Emission::default(),
            stats: Stats::default(),
        }
    }

    fn radius(&self) -> f32 {
        0.5 * self.params.spacing
    }

    /// Density of an interior particle of the perfect seeding lattice. Deriving ρ₀ from
    /// the kernel this way (instead of choosing a number) means the initial state is
    /// exactly at rest density, so nothing explodes on the first step.
    fn lattice_density(kernels: &Kernels, spacing: f32) -> f32 {
        lattice_offsets(kernels, spacing).map(|d| kernels.w(d.length_squared())).sum()
    }

    /// Σ_j |∇_j C_i|² + |∇_i C_i|² for an interior lattice particle (the second term is
    /// zero by symmetry).
    fn lattice_stiffness(kernels: &Kernels, spacing: f32, rho0: f32) -> f32 {
        let grads = lattice_offsets(kernels, spacing).map(|d| kernels.grad_w(d) / rho0);
        let (sum, sum_sq) = grads.fold((Vec2::ZERO, 0.0), |(s, q), g| (s + g, q + g.length_squared()));
        sum_sq + sum.length_squared()
    }

    /// Gather each particle's neighbours (within the kernel radius plus a margin) from
    /// the bins into the flat lists, in parallel: each particle fills only its own slots.
    fn build_neighbours(&mut self, scene: &Scene) {
        let reach = NEIGHBOUR_MARGIN * self.kernels.radius();
        self.bins.build(&self.pred, reach, scene.size);
        let n = self.pred.len();
        self.neighbours.resize(n * MAX_NEIGHBOURS, 0);
        self.neighbour_count.resize(n, 0);
        let (pred, bins, reach2) = (&self.pred, &self.bins, reach * reach);
        self.neighbours
            .par_chunks_mut(MAX_NEIGHBOURS)
            .zip(self.neighbour_count.par_iter_mut())
            .with_min_len(PAR_CHUNK)
            .enumerate()
            .for_each(|(i, (slots, count))| {
                let pi = pred[i];
                let mut c = 0;
                bins.for_each_candidate(pi, |j| {
                    if j != i && c < MAX_NEIGHBOURS && pi.distance_squared(pred[j]) < reach2 {
                        slots[c] = j as u32;
                        c += 1;
                    }
                });
                *count = c as u8;
            });
    }

    /// Keep predicted positions out of solids. Velocities are rebuilt from positions at
    /// the end of the substep, so the collision's velocity response is irrelevant here.
    fn collide(&mut self, scene: &Scene) {
        let interior = scene.solid.interior();
        super::collide_with_solids(scene, interior, self.radius(), 0.0, 1.0, &mut self.pred, &mut self.vel);
    }
}

impl Solver for Pbf {
    fn name(&self) -> &'static str {
        "PBF"
    }

    fn reset(&mut self, scene: &Scene, init: Init<'_>) {
        self.kernels = Kernels::new(KERNEL_PER_SPACING * self.params.spacing);
        self.rest_density = Self::lattice_density(&self.kernels, self.params.spacing);
        self.lattice_stiffness = Self::lattice_stiffness(&self.kernels, self.params.spacing, self.rest_density);
        match init {
            Init::FromScene => {
                // A perfect lattice (no jitter) starts exactly at rest density.
                super::seed_particles(scene, self.params.spacing, 0.0, &mut self.pos);
                self.vel = vec![Vec2::ZERO; self.pos.len()];
            }
            Init::FromParticles(view) => {
                super::resample(view, self.params.spacing, scene.size, &mut self.pos, &mut self.vel);
            }
        }
        let n = self.pos.len();
        self.pred = self.pos.clone();
        self.lambda = vec![0.0; n];
        self.delta = vec![Vec2::ZERO; n];
        self.density = vec![0.0; n];
        self.emission.reset();
        self.stats = Stats { particles: n, ..Stats::default() };
    }

    fn max_dt(&self, _scene: &Scene) -> f32 {
        let max_speed = self.vel.iter().map(|v| v.length_squared()).fold(0.0, f32::max).sqrt();
        self.params.max_dt(max_speed)
    }

    fn step(&mut self, dt: f32, scene: &Scene, input: &StepInput) {
        let mut timer = PhaseTimer::start();
        let k = self.kernels;
        let rho0 = self.rest_density;

        let (emitted, drained) =
            self.emission.apply(scene, input, dt, self.params.spacing, &mut self.pos, &mut self.vel);
        self.stats.emitted += emitted;
        self.stats.drained += drained;
        let n = self.pos.len();
        self.stats.particles = n;
        self.pred.resize(n, Vec2::ZERO);
        self.lambda.resize(n, 0.0);
        self.delta.resize(n, Vec2::ZERO);
        self.density.resize(n, 0.0);

        // Forces, then predict.
        super::apply_brushes(&input.brushes, dt, &self.pos, &mut self.vel);
        let g = scene.gravity;
        self.pred.par_iter_mut().zip(&self.pos).zip(self.vel.par_iter_mut()).with_min_len(PAR_CHUNK).for_each(
            |((x_star, &x), v)| {
                *v += g * dt;
                *x_star = x + *v * dt;
            },
        );
        self.collide(scene);
        self.build_neighbours(scene);
        timer.lap("predict");

        // s_corr = -k (W(r) / W(Δq))^n, the paper's artificial pressure, with k given as
        // an equivalent compression and converted to λ units by the lattice stiffness.
        let w_dq = k.w((TENSILE_DQ * k.radius()).powi(2));
        let tensile_k = self.params.tensile_k / self.lattice_stiffness;
        let relaxation = self.params.relaxation * self.lattice_stiffness;
        let w_self = k.w(0.0);
        for _ in 0..self.params.iterations {
            let (pred, nbrs, counts) = (&self.pred, &self.neighbours, &self.neighbour_count);
            let neighbours_of = |i: usize| nbrs[i * MAX_NEIGHBOURS..][..usize::from(counts[i])].iter().map(|&j| j as usize);

            // λ_i = -C_i / (Σ_j |∇_j C_i|² + ε), with C_i = ρ_i/ρ₀ - 1 clamped at 0:
            // the constraint only pushes apart, so the free surface doesn't clump.
            self.lambda.par_iter_mut().zip(self.density.par_iter_mut()).with_min_len(PAR_CHUNK).enumerate().for_each(
                |(i, (lambda, density))| {
                    let pi = pred[i];
                    let (mut rho, mut grad_i, mut sum_grad2) = (w_self, Vec2::ZERO, 0.0);
                    for j in neighbours_of(i) {
                        let d = pi - pred[j];
                        rho += k.w(d.length_squared());
                        let grad = k.grad_w(d) / rho0;
                        grad_i += grad;
                        sum_grad2 += grad.length_squared();
                    }
                    *density = rho;
                    let c = (rho / rho0 - 1.0).max(0.0);
                    *lambda = -c / (sum_grad2 + grad_i.length_squared() + relaxation);
                },
            );

            // Δp_i = (1/ρ₀) Σ_j (λ_i + λ_j + s_corr) ∇W(p_i - p_j)
            let lambda = &self.lambda;
            self.delta.par_iter_mut().with_min_len(PAR_CHUNK).enumerate().for_each(|(i, delta)| {
                let pi = pred[i];
                let mut dp = Vec2::ZERO;
                for j in neighbours_of(i) {
                    let d = pi - pred[j];
                    let r2 = d.length_squared();
                    if r2 >= k.radius() * k.radius() {
                        continue;
                    }
                    let s_corr = -tensile_k * (k.w(r2) / w_dq).powi(TENSILE_N);
                    dp += (lambda[i] + lambda[j] + s_corr) * k.grad_w(d);
                }
                *delta = dp / rho0;
            });
            self.pred.par_iter_mut().zip(&self.delta).with_min_len(PAR_CHUNK).for_each(|(p, d)| *p += *d);
            self.collide(scene);
        }
        timer.lap("constraints");

        // Velocity from the position change, then XSPH viscosity (reads the new
        // velocities from a snapshot, writes into `delta` as scratch, then swaps in).
        let inv_dt = 1.0 / dt;
        self.vel.par_iter_mut().zip(&self.pred).zip(&self.pos).with_min_len(PAR_CHUNK).for_each(|((v, &x_star), &x)| {
            *v = (x_star - x) * inv_dt;
        });
        let c = self.params.viscosity;
        if c > 0.0 {
            let (pred, vel, nbrs, counts) = (&self.pred, &self.vel, &self.neighbours, &self.neighbour_count);
            self.delta.par_iter_mut().with_min_len(PAR_CHUNK).enumerate().for_each(|(i, out)| {
                let pi = pred[i];
                let mut blend = Vec2::ZERO;
                for &j in &nbrs[i * MAX_NEIGHBOURS..][..usize::from(counts[i])] {
                    let j = j as usize;
                    blend += (vel[j] - vel[i]) * k.w((pi - pred[j]).length_squared());
                }
                *out = vel[i] + c * blend / rho0;
            });
            std::mem::swap(&mut self.vel, &mut self.delta);
        }
        std::mem::swap(&mut self.pos, &mut self.pred);
        timer.lap("velocity");

        let compression = self.density.par_iter().map(|&rho| (rho / rho0 - 1.0).max(0.0)).sum::<f32>()
            / self.density.len().max(1) as f32;
        self.stats.phases = timer.finish();
        self.stats.metrics = vec![Metric { name: "mean compression", value: compression, tolerance: None }];
    }

    fn visit_params(&mut self, v: &mut dyn ParamVisitor) {
        self.params.visit(v);
    }

    fn particles(&self) -> Option<ParticleView<'_>> {
        Some(ParticleView { pos: &self.pos, vel: &self.vel, radius: self.radius() })
    }

    fn stats(&self) -> &Stats {
        &self.stats
    }
}

/// Offsets to every lattice neighbour within the kernel radius, excluding the centre.
fn lattice_offsets(kernels: &Kernels, spacing: f32) -> impl Iterator<Item = Vec2> {
    let reach = (kernels.radius() / spacing).ceil() as i32;
    let h2 = kernels.radius() * kernels.radius();
    (-reach..=reach)
        .flat_map(move |j| (-reach..=reach).map(move |i| Vec2::new(i as f32, j as f32) * spacing))
        .filter(move |d| d.length_squared() < h2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::Rect;
    use crate::stepper::Stepper;

    #[test]
    fn rest_density_matches_an_interior_lattice_particle() {
        let pbf = Pbf::default();
        let rho = Pbf::lattice_density(&pbf.kernels, pbf.params.spacing);
        // ~12 neighbours inside the kernel; poly6 is normalised to integrate to 1,
        // so ρ₀ ≈ 1 / spacing² for a lattice dense enough to sample it well.
        let continuum = 1.0 / pbf.params.spacing.powi(2);
        assert!((rho / continuum - 1.0).abs() < 0.15, "ρ₀ = {rho}, continuum {continuum}");
    }

    /// A resting tank stays at rest and close to rest density.
    #[test]
    fn hydrostatic_tank_is_calm_and_incompressible() {
        let mut scene = Scene::empty_box(Vec2::new(1.0, 1.0), 64);
        scene.fluid_blocks.push(Rect::new(Vec2::ZERO, Vec2::new(1.0, 0.3)));
        let mut pbf = Pbf::with_params(PbfParams { spacing: 0.01, ..PbfParams::default() });
        pbf.reset(&scene, Init::FromScene);
        let stepper = Stepper::default();
        for _ in 0..120 {
            stepper.advance(&mut pbf, &scene, 1.0 / 60.0, &StepInput::default());
        }
        let max_speed = pbf.vel.iter().map(|v| v.length()).fold(0.0, f32::max);
        let compression = pbf.stats.metrics[0].value;
        assert!(max_speed < 0.15, "max speed {max_speed} m/s after 2 s at rest");
        assert!(compression < 0.03, "mean compression {compression}");
    }
}
