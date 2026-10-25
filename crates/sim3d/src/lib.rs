//! 3D fluids on the GPU (see docs/3D_PLAN.md).
//!
//! Kept apart from `sim` (which is 2D and CPU-side) and `sim-gpu` (2D solvers that read
//! their results back). 3D solvers keep everything on the GPU and share the renderer's
//! device, so their volumes can be drawn directly. They reuse `sim`'s parameter visitor
//! and stats, so the app's parameter panel and metrics work for them unchanged.

pub mod scene;
pub mod smoke;
pub mod solver;
pub mod world;

pub use glam::Vec3;
pub use scene::{Aabb, Burner, PRESETS, Preset3d, Scene3d, Shape};
pub use sim_gpu::GpuContext;
pub use smoke::{Smoke3d, SmokeParams};
pub use solver::{Brush3d, SOLVERS, Solver3d, Solver3dEntry, StepInput3d};
pub use world::World3d;
