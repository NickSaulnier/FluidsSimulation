//! Martin & Moyce (1952) dam-break validation, kept as a regression test.
//!
//! Grid-converged FLIP runs ahead of the experiment by a near-constant ΔT ≈ 0.2 (the
//! usual explanation is the experiment's finite gate-opening time), so arrival times
//! get a 15% band; the front *speed*, which a time offset doesn't affect, must match
//! within 10%. See docs/validation/README.md.

use sim::analysis::{MARTIN_MOYCE_N2_2, crossing_time, martin_moyce_point};
use sim::{PRESETS, SOLVERS, StepInput, World};

fn front_curve(solver: &str) -> Vec<(f32, f32)> {
    let si = SOLVERS.iter().position(|s| s.name == solver).expect("solver registered");
    let pi = PRESETS.iter().position(|p| p.name.starts_with("Martin")).expect("preset registered");
    let mut world = World::new(si, pi);
    let input = StepInput::default();
    (0..30)
        .filter_map(|_| {
            world.advance(1.0 / 60.0, &input);
            martin_moyce_point(&world)
        })
        .collect()
}

#[test]
fn surge_front_matches_martin_and_moyce() {
    let corroborated: Vec<(f32, f32)> =
        MARTIN_MOYCE_N2_2.iter().filter(|p| p.2).map(|&(t, z, _)| (t, z)).collect();
    for solver in ["FLIP", "PBF"] {
        let curve = front_curve(solver);
        let arrival = |z: f32| crossing_time(&curve, z).unwrap_or_else(|| panic!("{solver}: front never reached Z = {z}"));
        for &(t_ref, z) in &corroborated {
            let t = arrival(z);
            assert!((t / t_ref - 1.0).abs() < 0.15, "{solver}: reached Z = {z} at T = {t:.2}, experiment {t_ref}");
        }
        let (&(t1, z1), &(t2, z2)) = (&corroborated[1], &corroborated[2]);
        let speed = (z2 - z1) / (arrival(z2) - arrival(z1));
        let speed_ref = (z2 - z1) / (t2 - t1);
        assert!((speed / speed_ref - 1.0).abs() < 0.10, "{solver}: front speed dZ/dT = {speed:.2}, experiment {speed_ref:.2}");
    }
}
