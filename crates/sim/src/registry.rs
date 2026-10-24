//! The list of available solvers. Adding a solver touches this file and its own
//! module under `solvers/`, and nothing in the app or renderer.

use crate::solver::Solver;
use crate::solvers::Ballistic;

pub struct SolverEntry {
    pub name: &'static str,
    pub make: fn() -> Box<dyn Solver>,
}

pub static SOLVERS: &[SolverEntry] = &[
    // Non-capturing closures coerce to `fn` pointers, so they can live in a static.
    SolverEntry { name: "Ballistic (debug)", make: || Box::new(Ballistic::default()) },
];
