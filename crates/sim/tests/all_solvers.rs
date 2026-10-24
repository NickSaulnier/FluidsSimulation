//! Every registered solver, on every preset, must keep the invariants for a couple of
//! simulated seconds. New solvers are covered automatically by being in the registry.

use sim::{PRESETS, SOLVERS, StepInput, World, invariants};

#[test]
fn every_solver_on_every_preset_keeps_invariants() {
    let input = StepInput::default();
    for (si, solver) in SOLVERS.iter().enumerate() {
        for (pi, preset) in PRESETS.iter().enumerate() {
            let mut world = World::new(si, pi);
            // Presets that start empty (the wind tunnel) fill from their emitters.
            let should_seed = world.solver.particles().is_some() && !world.scene.fluid_blocks.is_empty();
            assert!(!should_seed || world.initial_particles > 0, "{} / {}: no particles seeded", solver.name, preset.name);
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

/// Switch between every pair of solvers mid-run, keeping particles. Solvers resample
/// to their own resolution, so counts may change, but the fluid must survive and keep
/// the invariants. Grid-only solvers (no particles) turn incoming particles into dye,
/// and a particle solver taking over from one reseeds from the scene.
#[test]
fn switching_solvers_hands_off_particles() {
    let input = StepInput::default();
    for (from, source) in SOLVERS.iter().enumerate() {
        for (to, target) in SOLVERS.iter().enumerate() {
            let mut world = World::new(from, 0);
            for _ in 0..20 {
                world.advance(1.0 / 60.0, &input);
            }
            let before = world.solver.particles().map(|p| p.pos.len());
            world.switch_solver(to, true);
            let after = world.solver.particles().map(|p| p.pos.len());
            let (a, b) = (source.name, target.name);
            if let (Some(before), Some(after)) = (before, after) {
                // No lower bound beyond "some survive": Ballistic piles non-interacting
                // particles into a thin strip, which a coarser solver rightly thins ~10x.
                assert!(after > 0 && after < before * 8, "{a} -> {b}: {before} -> {after} particles");
            }
            for frame in 0..30 {
                world.advance(1.0 / 60.0, &input);
                let violations = invariants::check(&world);
                assert!(violations.is_empty(), "{a} -> {b}, frame {frame}: {:?}", violations);
            }
            world.switch_solver(to, false);
            assert!(invariants::check(&world).is_empty(), "{a} -> {b}: reseed");
        }
    }
}
