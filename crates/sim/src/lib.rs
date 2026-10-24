//! Headless 2D fluid simulation: scenes, the `Solver` trait, and solver implementations.
//!
//! This crate deliberately knows nothing about windows, GPUs or UI. The app and the
//! bench both drive it through [`World`].

pub mod grid;
pub mod invariants;
pub mod presets;
pub mod registry;
pub mod rng;
pub mod scene;
pub mod solver;
pub mod solvers;
pub mod stepper;
pub mod world;

pub use glam::Vec2;
pub use presets::{PRESETS, Preset};
pub use registry::{SOLVERS, SolverEntry};
pub use scene::{ForceBrush, Rect, Scene, SolidMask, StepInput};
pub use solver::{Init, Metric, ParticleView, Solver, Stats};
pub use stepper::{FrameReport, Stepper};
pub use world::World;
