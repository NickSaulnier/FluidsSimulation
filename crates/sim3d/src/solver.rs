//! The 3D solver interface, parallel to `sim::Solver` but GPU-side: `step` records
//! compute passes into the caller's encoder, and results stay on the GPU.

use crate::scene::Scene3d;
use glam::Vec3;
use sim::{ParamVisitor, Stats};
use sim_gpu::GpuContext;

/// A spherical push in the volume: velocities inside `radius` are pulled toward
/// `velocity` plus a rigid rotation `spin` (axis × rad/s), fading to the rim.
#[derive(Clone, Copy, Debug)]
pub struct Brush3d {
    pub pos: Vec3,
    pub radius: f32,
    pub velocity: Vec3,
    pub spin: Vec3,
    /// Blend rate, 1/s.
    pub strength: f32,
}

/// Per-frame interaction: brushes, and extra burners (the torch tool, reactions).
#[derive(Clone, Debug, Default)]
pub struct StepInput3d {
    pub brushes: Vec<Brush3d>,
    pub burners: Vec<crate::scene::Burner>,
}

pub trait Solver3d {
    fn name(&self) -> &'static str;
    /// Start over from the scene.
    fn reset(&mut self, gpu: &GpuContext, scene: &Scene3d);
    /// Record one step of `dt` seconds into `encoder` (the caller submits it).
    fn step(&mut self, gpu: &GpuContext, encoder: &mut wgpu::CommandEncoder, dt: f32, scene: &Scene3d, input: &StepInput3d);
    /// Parameters, through the same visitor (and so the same panel) as 2D solvers.
    fn visit_params(&mut self, _v: &mut dyn ParamVisitor) {}
    fn stats(&self) -> &Stats;
}

#[derive(Clone, Copy)]
pub struct Solver3dEntry {
    pub name: &'static str,
    pub make: fn(&GpuContext) -> Box<dyn Solver3d>,
}

pub static SOLVERS: &[Solver3dEntry] = &[Solver3dEntry { name: "3D preview (empty box)", make: |_| Box::new(Empty::default()) }];

/// A placeholder until the smoke-and-fire solver lands: an empty volume, so the 3D view,
/// camera and mode switch can be built and checked first.
#[derive(Default)]
pub struct Empty {
    stats: Stats,
}

impl Solver3d for Empty {
    fn name(&self) -> &'static str {
        "3D preview (empty box)"
    }

    fn reset(&mut self, _gpu: &GpuContext, _scene: &Scene3d) {}

    fn step(&mut self, _gpu: &GpuContext, _encoder: &mut wgpu::CommandEncoder, _dt: f32, _scene: &Scene3d, _input: &StepInput3d) {}

    fn stats(&self) -> &Stats {
        &self.stats
    }
}
