//! Headless 2D fluid simulation: scenes, the `Solver` trait, and solver implementations.
//!
//! This crate deliberately knows nothing about windows, GPUs or UI. The app and the
//! bench both drive it through [`World`].

pub mod analysis;
pub mod grid;
pub mod invariants;
pub mod neighbors;
pub mod params;
pub mod presets;
pub mod registry;
pub mod scene;
pub mod solver;
pub mod solvers;
pub mod stepper;
pub mod timing;
pub mod world;

/// Default rayon thread count: all cores, capped at 8, unless `RAYON_NUM_THREADS` says
/// otherwise. The solvers synchronise hundreds of times per frame on small grids, and
/// beyond ~8 threads that costs more than the extra cores bring (see the bench's
/// `--sweep-threads`). Larger grids would scale further.
pub fn default_threads() -> usize {
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    std::env::var("RAYON_NUM_THREADS").ok().and_then(|s| s.parse().ok()).filter(|&n| n > 0).unwrap_or(cores.min(8))
}

pub use glam::Vec2;
pub use params::{FloatSpec, IntSpec, ParamVisitor, ToggleSpec};
pub use presets::{DEFAULT_DOMAIN, PRESETS, Preset, domain_for};
pub use registry::{SOLVERS, SolverEntry};
pub use scene::{BORDER_CELLS, Emitter, ForceBrush, Rect, Scene, SolidMask, StepInput};
pub use solver::{FieldInfo, FieldKind, FieldLayout, Init, Metric, ParticleView, Solver, Stats};
pub use stepper::{FrameReport, Stepper};
pub use world::World;
