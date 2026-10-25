//! Soak tests: a visualizer run, end to end, from synthetic music. The analyzer, the
//! director (applying looks), the reaction engine and the particle budget drive the
//! world exactly as in the app's full-screen mode, and every half second the run is
//! checked: the simulation invariants (no NaN, nothing outside the domain or inside
//! walls, particle accounting, metrics within tolerance) and the particle budget.
//!
//! The 60-second run is part of the normal tests; the 30-minute one runs on demand:
//! `cargo test --release -p app thirty_minute_soak -- --ignored --nocapture`.

#[cfg(test)]
mod tests {
    use crate::audio::analyzer::tests::synthetic_beat;
    use crate::audio::analyzer::{Analyzer, OnsetKind};
    use crate::audio::reactions::ReactionEngine;
    use crate::looks::{self, LOOKS};
    use crate::stability;
    use crate::ui::UiState;
    use sim::{StepInput, World, invariants};

    const RATE: u32 = 48_000;
    const FRAME: f32 = 1.0 / 60.0;

    /// One "song": 20 s of a 120 bpm beat, then 3 s of silence (a gap between songs).
    fn song() -> Vec<f32> {
        let (mut audio, _, _) = synthetic_beat(RATE, 20.0, 120.0, 0.8);
        audio.extend(std::iter::repeat_n(0.0, 3 * RATE as usize));
        audio
    }

    /// Run the visualizer for `seconds` with look changes at least every `interval`
    /// seconds. Returns the looks applied, in order.
    fn soak(seconds: f32, interval: f32) -> Vec<usize> {
        let audio = song();
        let mut world = World::new(0, 0);
        let mut ui = UiState { presenting: true, ..UiState::default() };
        ui.director.interval_s = interval;
        let mut analyzer = Analyzer::new(RATE);
        let mut engine = ReactionEngine::default();
        engine.idle = true;
        let mut base_gravity = world.scene.gravity;
        let per_frame = (RATE as f32 * FRAME) as usize;
        let (mut cursor, mut visited, mut peak) = (0, Vec::new(), 0usize);
        for frame in 0..(seconds / FRAME) as usize {
            let chunk: Vec<f32> = (0..per_frame).map(|k| audio[(cursor + k) % audio.len()]).collect();
            cursor += per_frame;
            analyzer.process(&chunk, 1.5);
            let onsets = analyzer.take_onsets();
            let kicked = onsets.iter().any(|o| o.kind == OnsetKind::Kick);
            if let Some(look) = ui.director.update(true, Some(analyzer.features()), kicked, FRAME) {
                looks::apply(&LOOKS[look], &mut world, &mut ui);
                base_gravity = world.scene.gravity; // the new scene's own
                visited.push(look);
            }
            let mut out = engine.evaluate(&ui.reactions, Some(analyzer.features()), &onsets, world.scene.size, base_gravity, 1.0, FRAME);
            if !stability::room_for_fluid(&world) {
                out.emitters.clear();
            }
            world.scene.gravity = out.gravity;
            world.advance(FRAME, &StepInput { brushes: out.brushes, emitters: out.emitters });

            if frame % 30 == 0 {
                let t = frame as f32 * FRAME;
                let violations = invariants::check(&world);
                assert!(violations.is_empty(), "{} at {t:.0} s: {violations:?}", world.solver.name());
                if let Some(p) = world.solver.particles() {
                    let budget = stability::particle_budget(world.initial_particles);
                    peak = peak.max(p.pos.len());
                    // Jets stop at the budget; one frame's burst may overshoot a little.
                    assert!(p.pos.len() <= budget + 2_000, "{} at {t:.0} s: {} particles, budget {budget}", world.solver.name(), p.pos.len());
                }
            }
        }
        eprintln!("soak {seconds} s: looks {:?}, peak particles {peak}", visited.iter().map(|&i| LOOKS[i].name).collect::<Vec<_>>());
        visited
    }

    /// A minute with a 15 s interval: several look changes (timed and at song gaps),
    /// all healthy.
    #[test]
    fn one_minute_soak_changes_looks_and_stays_healthy() {
        let visited = soak(60.0, 15.0);
        assert!(visited.len() >= 3, "only {} look changes: {visited:?}", visited.len());
    }

    /// Half an hour, every look visited, all healthy.
    #[test]
    #[ignore = "slow: run on demand"]
    fn thirty_minute_soak() {
        let visited = soak(1800.0, 30.0);
        for (i, look) in LOOKS.iter().enumerate() {
            assert!(visited.contains(&i), "never showed {}", look.name);
        }
    }
}
