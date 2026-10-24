//! Splits a frame's worth of simulated time into CFL-limited substeps.

use crate::scene::{Scene, StepInput};
use crate::solver::Solver;
use std::time::{Duration, Instant};

pub struct Stepper {
    pub min_substeps: u32,
    pub max_substeps: u32,
}

impl Default for Stepper {
    fn default() -> Self {
        Self { min_substeps: 1, max_substeps: 8 }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FrameReport {
    pub substeps: u32,
    pub dt: f32,
    /// Simulated time skipped because the substep cap was hit (the sim ran in slow motion).
    pub time_dropped: f32,
    pub wall: Duration,
}

impl Stepper {
    pub fn advance(&self, solver: &mut dyn Solver, scene: &Scene, target: f32, input: &StepInput) -> FrameReport {
        let t0 = Instant::now();
        let max_dt = solver.max_dt(scene).max(1e-6);
        // Even split: n equal substeps, n chosen by CFL, then capped. When capped, each
        // substep is still at most max_dt and the remainder is dropped: slow motion, not blow-up.
        let wanted = (target / max_dt).ceil() as u32;
        let n = wanted.clamp(self.min_substeps.max(1), self.max_substeps);
        let dt = (target / n as f32).min(max_dt);
        for _ in 0..n {
            solver.step(dt, scene, input);
        }
        FrameReport { substeps: n, dt, time_dropped: target - dt * n as f32, wall: t0.elapsed() }
    }
}
