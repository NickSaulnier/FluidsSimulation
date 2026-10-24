//! The list of available solvers. Adding a solver touches this file and its own
//! module under `solvers/`, and nothing in the app or renderer. The first entry is
//! the default.

use crate::solver::Solver;
use crate::solvers::{Ballistic, Flip, Lbm, Pbf, StableFluids};

#[derive(Clone, Copy)]
pub struct SolverEntry {
    pub name: &'static str,
    pub make: fn() -> Box<dyn Solver>,
}

pub static SOLVERS: &[SolverEntry] = &[
    // Non-capturing closures coerce to `fn` pointers, so they can live in a static.
    SolverEntry { name: "FLIP", make: || Box::new(Flip::default()) },
    SolverEntry { name: "PBF", make: || Box::new(Pbf::default()) },
    SolverEntry { name: "Stable fluids (dye)", make: || Box::new(StableFluids::default()) },
    SolverEntry { name: "Lattice Boltzmann (dye)", make: || Box::new(Lbm::default()) },
    SolverEntry { name: "Ballistic (debug)", make: || Box::new(Ballistic::default()) },
];
