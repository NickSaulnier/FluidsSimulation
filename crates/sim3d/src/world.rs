//! `World3d`: the 3D counterpart of `sim::World`. Scene, solver, and the GPU context the
//! solver runs on (normally the renderer's own device).

use crate::scene::{PRESETS, Scene3d};
use glam::Vec3;
use crate::solver::{SOLVERS, Solver3d, StepInput3d};
use sim_gpu::GpuContext;

pub struct World3d {
    pub gpu: GpuContext,
    pub scene: Scene3d,
    pub solver: Box<dyn Solver3d>,
    pub solver_index: usize,
    pub preset_index: usize,
    /// Simulated seconds since the last reset.
    pub time: f64,
    /// The scene's own gravity (`scene.gravity` may be tilted by the music meanwhile).
    pub base_gravity: Vec3,
    /// Counts resets, so watchers can tell a fresh start.
    pub generation: u64,
}

impl World3d {
    pub fn new(gpu: GpuContext, solver_index: usize, preset_index: usize) -> Self {
        let scene = (PRESETS[preset_index].build)();
        let solver = (SOLVERS[solver_index].make)(&gpu);
        let base_gravity = scene.gravity;
        let mut world = Self { gpu, scene, solver, solver_index, preset_index, time: 0.0, base_gravity, generation: 0 };
        world.reset();
        world
    }

    pub fn reset(&mut self) {
        self.solver.reset(&self.gpu, &self.scene);
        self.time = 0.0;
        self.generation += 1;
    }

    pub fn load_preset(&mut self, index: usize) {
        self.preset_index = index;
        self.scene = (PRESETS[index].build)();
        self.base_gravity = self.scene.gravity;
        self.reset();
    }

    /// Problems with the solver's state (non-finite values, values out of range),
    /// read back from the GPU: slow, for tests and soak runs.
    pub fn check(&self) -> Vec<String> {
        self.solver.check(&self.gpu)
    }

    pub fn switch_solver(&mut self, index: usize) {
        self.solver = (SOLVERS[index].make)(&self.gpu);
        self.solver_index = index;
        self.reset();
    }

    /// Step by `dt` seconds: record the solver's passes and submit them.
    pub fn advance(&mut self, dt: f32, input: &StepInput3d) {
        let mut encoder = self.gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("sim3d step") });
        self.solver.step(&self.gpu, &mut encoder, dt, &self.scene, input);
        self.gpu.queue.submit([encoder.finish()]);
        self.solver.submitted(&self.gpu);
        self.time += f64::from(dt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A world runs on a headless device (skipped without a GPU adapter).
    #[test]
    fn a_world_steps_and_switches_scenes() {
        let gpu = match GpuContext::new() {
            Ok(g) => g,
            Err(e) => return eprintln!("skipping: {e:#}"),
        };
        let mut world = World3d::new(gpu, 0, 0);
        world.advance(1.0 / 60.0, &StepInput3d::default());
        assert!(world.time > 0.0);
        world.load_preset(PRESETS.len() - 1);
        assert_eq!(world.time, 0.0);
        assert_eq!(world.preset_index, PRESETS.len() - 1);
    }
}
