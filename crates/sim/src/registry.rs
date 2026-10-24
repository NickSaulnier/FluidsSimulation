//! The list of available solvers. Adding a solver touches this file and its own
//! module under `solvers/`, and nothing in the app or renderer. The first entry is
//! the default.

use crate::solver::Solver;
use crate::solvers::{Ballistic, Flip};

pub struct SolverEntry {
    pub name: &'static str,
    pub make: fn() -> Box<dyn Solver>,
}

pub static SOLVERS: &[SolverEntry] = &[
    // Non-capturing closures coerce to `fn` pointers, so they can live in a static.
    SolverEntry { name: "FLIP", make: || Box::new(Flip::default()) },
    SolverEntry { name: "Ballistic (debug)", make: || Box::new(Ballistic::default()) },
];
