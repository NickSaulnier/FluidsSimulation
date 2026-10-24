//! Every registered solver, on every preset, must keep the invariants for a couple of
//! simulated seconds. New solvers are covered automatically by being in the registry.

use sim::{PRESETS, SOLVERS, StepInput, World, invariants};

#[test]
fn every_solver_on_every_preset_keeps_invariants() {
    let input = StepInput::default();
    for (si, solver) in SOLVERS.iter().enumerate() {
        for (pi, preset) in PRESETS.iter().enumerate() {
            let mut world = World::new(si, pi);
            assert!(world.initial_particles > 0, "{} / {}: no particles seeded", solver.name, preset.name);
            for frame in 0..120 {
                world.advance(1.0 / 60.0, &input);
                let violations = invariants::check(&world);
                assert!(
                    violations.is_empty(),
                    "{} / {} frame {frame}: {}",
                    solver.name,
                    preset.name,
                    violations.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "),
                );
            }
        }
    }
}

#[test]
fn switching_solvers_keeps_or_reseeds_particles() {
    let mut world = World::new(0, 0);
    world.advance(1.0 / 60.0, &StepInput::default());
    let n = world.solver.particles().unwrap().pos.len();
    for index in 0..SOLVERS.len() {
        world.switch_solver(index, true);
        assert_eq!(world.solver.particles().unwrap().pos.len(), n);
        world.switch_solver(index, false);
        assert!(invariants::check(&world).is_empty());
    }
}
