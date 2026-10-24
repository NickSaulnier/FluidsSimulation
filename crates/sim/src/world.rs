//! `World` = scene + active solver + stepper. The one entry point the app and bench use.

use crate::presets::PRESETS;
use crate::registry::SOLVERS;
use crate::scene::{Scene, StepInput};
use crate::solver::{Init, Solver};
use crate::stepper::{FrameReport, Stepper};

pub struct World {
    pub scene: Scene,
    pub solver: Box<dyn Solver>,
    pub solver_index: usize,
    pub preset_index: usize,
    pub stepper: Stepper,
    /// Simulated seconds since the last reset.
    pub time: f64,
    /// Particle count right after the last reset, for the conservation invariant.
    pub initial_particles: usize,
}

impl World {
    pub fn new(solver_index: usize, preset_index: usize) -> Self {
        let scene = (PRESETS[preset_index].build)();
        let solver = (SOLVERS[solver_index].make)();
        let mut world = Self {
            scene,
            solver,
            solver_index,
            preset_index,
            stepper: Stepper::default(),
            time: 0.0,
            initial_particles: 0,
        };
        world.reset();
        world
    }

    /// Re-seed the current solver from the current scene (keeps painted walls and gravity).
    pub fn reset(&mut self) {
        self.scene.solid.update_derived();
        self.solver.reset(&self.scene, Init::FromScene);
        self.after_reset();
    }

    /// Rebuild the scene from its preset, then reset the solver.
    pub fn load_preset(&mut self, index: usize) {
        self.preset_index = index;
        self.scene = (PRESETS[index].build)();
        self.reset();
    }

    pub fn switch_solver(&mut self, index: usize, keep_particles: bool) {
        let mut next = (SOLVERS[index].make)();
        match self.solver.particles().filter(|_| keep_particles) {
            Some(view) => next.reset(&self.scene, Init::FromParticles(view)),
            None => next.reset(&self.scene, Init::FromScene),
        }
        self.solver = next; // the old solver drops here, after the borrow above has ended
        self.solver_index = index;
        self.after_reset();
    }

    pub fn advance(&mut self, frame_dt: f32, input: &StepInput) -> FrameReport {
        self.scene.solid.update_derived();
        let report = self.stepper.advance(self.solver.as_mut(), &self.scene, frame_dt, input);
        self.time += f64::from(report.dt * report.substeps as f32);
        report
    }

    fn after_reset(&mut self) {
        self.time = 0.0;
        self.initial_particles = self.solver.particles().map_or(0, |p| p.pos.len());
    }
}
