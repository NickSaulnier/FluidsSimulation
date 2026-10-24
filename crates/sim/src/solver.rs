//! The solver abstraction. The app, renderer and bench only ever see `dyn Solver`.

use crate::params::ParamVisitor;
use crate::scene::{Scene, StepInput};
use glam::Vec2;
use std::time::Duration;

/// Borrowed particle state. Valid only until the next `&mut` call on the solver,
/// and the borrow checker enforces that.
#[derive(Clone, Copy)]
pub struct ParticleView<'a> {
    /// Struct-of-arrays, so `bytemuck::cast_slice` uploads these with zero copies.
    pub pos: &'a [Vec2],
    pub vel: &'a [Vec2],
    pub radius: f32,
}

/// How a field's values should be read (and coloured).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    /// Centred on zero (e.g. divergence): diverging colour map, symmetric range.
    Signed,
    /// Zero or more (e.g. pressure, speed): sequential colour map from zero.
    NonNegative,
    /// Small integer categories (e.g. cell type).
    Categorical,
}

#[derive(Clone, Copy, Debug)]
pub struct FieldInfo {
    pub key: &'static str,
    pub label: &'static str,
    pub unit: &'static str,
    pub kind: FieldKind,
}

/// Where a filled field sits in world space: sample (i, j) is centred at
/// `origin + (i + 1/2, j + 1/2) * cell`, row-major with row 0 at the bottom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldLayout {
    pub nx: usize,
    pub ny: usize,
    pub origin: Vec2,
    pub cell: f32,
}

/// How a solver is (re)initialised: from the scene's fluid blocks, or by adopting
/// another solver's particles (switch solvers mid-splash).
pub enum Init<'a> {
    FromScene,
    FromParticles(ParticleView<'a>),
}

#[derive(Clone, Debug)]
pub struct Metric {
    pub name: &'static str,
    pub value: f32,
    /// If set, invariant checks fail when `value` exceeds it.
    pub tolerance: Option<f32>,
}

#[derive(Default, Clone, Debug)]
pub struct Stats {
    pub particles: usize,
    /// Cumulative since reset, for the particle-count invariant.
    pub emitted: usize,
    pub drained: usize,
    /// Breakdown of the last step.
    pub phases: Vec<(&'static str, Duration)>,
    pub metrics: Vec<Metric>,
}

pub trait Solver: Send {
    fn name(&self) -> &'static str;

    fn reset(&mut self, scene: &Scene, init: Init<'_>);

    /// Largest substep the solver considers stable right now (CFL and the like).
    fn max_dt(&self, scene: &Scene) -> f32;

    fn step(&mut self, dt: f32, scene: &Scene, input: &StepInput);

    /// Walk the tunable parameters (see `params`). A parameter whose spec has
    /// `restart: true` only takes effect after the next `reset`.
    fn visit_params(&mut self, visitor: &mut dyn ParamVisitor);

    /// `None` for pure grid solvers (stable fluids, LBM).
    fn particles(&self) -> Option<ParticleView<'_>>;

    /// Grid fields this solver can show as overlays, indexed by position.
    fn fields(&self) -> &'static [FieldInfo] {
        &[]
    }

    /// Write field `index` into a caller-owned buffer (`NaN` = no data, drawn
    /// transparent). A buffer rather than a borrowed slice because fields are often
    /// derived (speed, divergence): the caller reuses its allocation, and the solver
    /// needs no interior mutability to cache results.
    fn fill_field(&self, _index: usize, _out: &mut Vec<f32>) -> Option<FieldLayout> {
        None
    }

    fn stats(&self) -> &Stats;
}
