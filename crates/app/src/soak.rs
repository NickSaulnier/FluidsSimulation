//! Soak tests: a visualizer run, end to end, from synthetic music. The analyzer, the
//! director (applying looks), the reaction engine and the particle budget drive the
//! world exactly as in the app's full-screen mode, and every half second the run is
//! checked: the simulation invariants (no NaN, nothing outside the domain or inside
//! walls, particle accounting, metrics within tolerance) and the particle budget.
//!
//! 3D looks run on a headless GPU world, with the reactions lifted into the volume as
//! in the app (the camera turning slowly), and their state is read back and checked
//! every 10 s (reading 1.8M cells back every half second would dominate the run).
//! Without a GPU adapter the 3D looks are applied but not run.
//!
//! The 60-second run is part of the normal tests; the 30-minute one runs on demand:
//! `cargo test --release -p app thirty_minute_soak -- --ignored --nocapture`.

#[cfg(test)]
mod tests {
    use crate::audio::analyzer::tests::synthetic_beat;
    use crate::audio::analyzer::{Analyzer, OnsetKind};
    use crate::audio::reactions::ReactionEngine;
    use crate::looks::{self, LOOKS};
    use crate::ui::UiState;
    use crate::{react3d, stability};
    use glam::Vec3;
    use sim::{StepInput, World, invariants};
    use std::f32::consts::TAU;

    const RATE: u32 = 48_000;
    const FRAME: f32 = 1.0 / 60.0;
    /// How often a 3D look's state is read back and checked, frames.
    const CHECK_3D_EVERY: usize = 600;
    /// The soak's stand-in for the camera's orbit, rad/s.
    const ORBIT: f32 = 0.12;

    /// One "song": a soft verse (a bare 120 bpm beat), a chorus (the beat over a
    /// sustained bass line, loud enough to count as energetic), then 3 s of silence (a
    /// gap between songs). Without the chorus the energetic looks would never come up.
    fn song(verse_s: f32, chorus_s: f32) -> Vec<f32> {
        let (verse, _, _) = synthetic_beat(RATE, verse_s, 120.0, 0.3);
        let (mut chorus, _, _) = synthetic_beat(RATE, chorus_s, 120.0, 0.8);
        for (k, s) in chorus.iter_mut().enumerate() {
            let t = k as f32 / RATE as f32;
            *s += 0.35 * (TAU * 110.0 * t).sin() + 0.15 * (TAU * 220.0 * t).sin();
        }
        let mut audio = verse;
        audio.extend(chorus);
        audio.extend(std::iter::repeat_n(0.0, 3 * RATE as usize));
        audio
    }

    /// Songs of different shapes, looped. One song on repeat would line the look changes
    /// up with the same moments every time, and the director's rotation could then miss
    /// some looks for good.
    fn playlist() -> Vec<f32> {
        [(15.0, 15.0), (8.0, 22.0), (20.0, 9.0), (11.0, 17.0)].into_iter().flat_map(|(v, c)| song(v, c)).collect()
    }

    /// Run the visualizer for `seconds` with look changes at least every `interval`
    /// seconds. Returns the looks applied, in order.
    fn soak(seconds: f32, interval: f32) -> Vec<usize> {
        let audio = playlist();
        let mut world = World::new(0, 0);
        let mut ui = UiState { presenting: true, ..UiState::default() };
        ui.director.interval_s = interval;
        let mut analyzer = Analyzer::new(RATE);
        let mut engine = ReactionEngine::default();
        engine.idle = true;
        let mut base_gravity = world.scene.gravity;
        let per_frame = (RATE as f32 * FRAME) as usize;
        let (mut cursor, mut visited, mut peak) = (0, Vec::new(), 0usize);
        // The 3D world, made on the first 3D look and kept; whether it's the one running.
        let mut world3d: Option<sim3d::World3d> = None;
        let (mut in_3d, mut checks_3d, mut no_gpu) = (false, 0, false);
        for frame in 0..(seconds / FRAME) as usize {
            let t = frame as f32 * FRAME;
            let chunk: Vec<f32> = (0..per_frame).map(|k| audio[(cursor + k) % audio.len()]).collect();
            cursor += per_frame;
            analyzer.process(&chunk, 1.5);
            let onsets = analyzer.take_onsets();
            let kicked = onsets.iter().any(|o| o.kind == OnsetKind::Kick);
            if let Some(look) = ui.director.update(true, Some(analyzer.features()), kicked, FRAME) {
                let l = &LOOKS[look];
                visited.push(look);
                in_3d = false;
                match l.solver_3d() {
                    Some(solver) => {
                        if world3d.is_none() && !no_gpu {
                            match sim3d::GpuContext::new() {
                                Ok(gpu) => world3d = Some(sim3d::World3d::new(gpu, solver, 0)),
                                Err(e) => {
                                    eprintln!("not running the 3D looks: {e:#}");
                                    no_gpu = true;
                                }
                            }
                        }
                        if let Some(w) = &mut world3d {
                            w.switch_solver(solver);
                            looks::apply_3d(l, w, &mut ui);
                            in_3d = true;
                        }
                    }
                    None => {
                        looks::apply(l, &mut world, &mut ui);
                        base_gravity = world.scene.gravity; // the new scene's own
                    }
                }
            }
            if let (true, Some(w)) = (in_3d, &mut world3d) {
                let size = w.scene.size;
                let out = engine.evaluate(
                    &ui.reactions,
                    Some(analyzer.features()),
                    &onsets,
                    react3d::front_view(size),
                    react3d::front_gravity(w.base_gravity),
                    1.0,
                    FRAME,
                );
                let right = Vec3::new((ORBIT * t).cos(), 0.0, -(ORBIT * t).sin());
                let (input, gravity) = react3d::lift(&out, size, right, w.base_gravity);
                w.scene.gravity = gravity;
                w.advance(FRAME, &input);
                if frame % CHECK_3D_EVERY == 0 {
                    let problems = w.check();
                    assert!(problems.is_empty(), "{} at {t:.0} s: {problems:?}", w.solver.name());
                    checks_3d += 1;
                }
                continue;
            }
            let mut out = engine.evaluate(&ui.reactions, Some(analyzer.features()), &onsets, world.scene.size, base_gravity, 1.0, FRAME);
            if !stability::room_for_fluid(&world) {
                out.emitters.clear();
            }
            world.scene.gravity = out.gravity;
            world.advance(FRAME, &StepInput { brushes: out.brushes, emitters: out.emitters });

            if frame % 30 == 0 {
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
        eprintln!(
            "soak {seconds} s: looks {:?}, peak particles {peak}, 3D checks {checks_3d}",
            visited.iter().map(|&i| LOOKS[i].name).collect::<Vec<_>>()
        );
        visited
    }

    /// A minute with a 15 s interval: several look changes (timed and at song gaps),
    /// all healthy.
    #[test]
    fn one_minute_soak_changes_looks_and_stays_healthy() {
        let visited = soak(60.0, 15.0);
        assert!(visited.len() >= 3, "only {} look changes: {visited:?}", visited.len());
    }

    /// Half an hour, every look visited, all healthy. The interval is shorter than the
    /// songs, so timed changes land mid-song, in the loud choruses too.
    #[test]
    #[ignore = "slow: run on demand"]
    fn thirty_minute_soak() {
        let visited = soak(1800.0, 20.0);
        for (i, look) in LOOKS.iter().enumerate() {
            assert!(visited.contains(&i), "never showed {}", look.name);
        }
    }
}
