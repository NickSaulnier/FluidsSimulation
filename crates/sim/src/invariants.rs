//! Solver-agnostic sanity checks, shared by integration tests and the bench.

use crate::world::World;
use glam::Vec2;
use thiserror::Error;

#[derive(Debug, Error, PartialEq)]
pub enum Violation {
    #[error("particle {index} has a non-finite position or velocity")]
    NonFinite { index: usize },
    #[error("{count} particles are outside the domain")]
    OutOfDomain { count: usize },
    #[error("{count} particles are deeper than one radius inside a solid")]
    InSolid { count: usize },
    #[error("particle count {actual}, expected {expected} (initial + emitted - drained)")]
    CountMismatch { expected: usize, actual: usize },
    #[error("metric {name} = {value} exceeds tolerance {tolerance}")]
    Metric { name: &'static str, value: f32, tolerance: f32 },
}

pub fn check(world: &World) -> Vec<Violation> {
    let mut out = Vec::new();
    let stats = world.solver.stats();

    if let Some(view) = world.solver.particles() {
        if let Some(index) = view.pos.iter().zip(view.vel).position(|(p, v)| !(p.is_finite() && v.is_finite())) {
            out.push(Violation::NonFinite { index });
        }
        let size = world.scene.size;
        let outside = view.pos.iter().filter(|p| !(p.cmpge(Vec2::ZERO).all() && p.cmple(size).all())).count();
        if outside > 0 {
            out.push(Violation::OutOfDomain { count: outside });
        }
        let in_solid = view.pos.iter().filter(|&&p| world.scene.solid.distance(p) < -view.radius).count();
        if in_solid > 0 {
            out.push(Violation::InSolid { count: in_solid });
        }
        let expected = world.initial_particles + stats.emitted - stats.drained;
        if view.pos.len() != expected {
            out.push(Violation::CountMismatch { expected, actual: view.pos.len() });
        }
    }

    for m in &stats.metrics {
        if let Some(tolerance) = m.tolerance
            && (m.value.is_nan() || m.value > tolerance)
        {
            out.push(Violation::Metric { name: m.name, value: m.value, tolerance });
        }
    }
    out
}
