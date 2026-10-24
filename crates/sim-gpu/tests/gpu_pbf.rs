//! GPU PBF against the CPU solver it mirrors. Skipped (with a note) if no GPU adapter.

use sim::solvers::{Pbf, PbfParams};
use sim::{Init, PRESETS, Rect, Scene, Solver, StepInput, Stepper, Vec2, World, invariants};
use sim_gpu::{GpuContext, GpuPbf};

fn gpu() -> Option<GpuContext> {
    match GpuContext::new() {
        Ok(g) => Some(g),
        Err(e) => {
            eprintln!("skipping GPU test: {e:#}");
            None
        }
    }
}

fn tank() -> Scene {
    let mut scene = Scene::empty_box(Vec2::new(1.0, 1.0), 64);
    scene.fluid_blocks.push(Rect::new(Vec2::ZERO, Vec2::new(1.0, 0.3)));
    scene
}

/// From identical seeds and parameters, one substep on the GPU lands every particle where
/// the CPU solver puts it, up to float summation order.
#[test]
fn gpu_matches_cpu_after_one_substep() {
    let Some(gpu) = gpu() else { return };
    let scene = tank();
    let params = PbfParams { spacing: 0.01, ..PbfParams::default() };
    let mut cpu = Pbf::with_params(params.clone());
    let mut gpu = GpuPbf::new(gpu, params);
    cpu.reset(&scene, Init::FromScene);
    gpu.reset(&scene, Init::FromScene);
    let input = StepInput::default();
    cpu.step(1.0 / 240.0, &scene, &input);
    gpu.step(1.0 / 240.0, &scene, &input);
    let (a, b) = (cpu.particles().unwrap(), gpu.particles().unwrap());
    assert_eq!(a.pos.len(), b.pos.len());
    let worst = a.pos.iter().zip(b.pos).map(|(p, q)| p.distance(*q)).fold(0.0, f32::max);
    assert!(worst < 1e-4, "largest CPU/GPU position difference {worst} m");
}

/// The CPU solver's resting-tank test, on the GPU.
#[test]
fn gpu_hydrostatic_tank_is_calm_and_incompressible() {
    let Some(gpu) = gpu() else { return };
    let scene = tank();
    let mut pbf = GpuPbf::new(gpu, PbfParams { spacing: 0.01, ..PbfParams::default() });
    pbf.reset(&scene, Init::FromScene);
    let stepper = Stepper::default();
    for _ in 0..120 {
        stepper.advance(&mut pbf, &scene, 1.0 / 60.0, &StepInput::default());
    }
    let view = pbf.particles().unwrap();
    let max_speed = view.vel.iter().map(|v| v.length()).fold(0.0, f32::max);
    let compression = pbf.stats().metrics[0].value;
    assert!(max_speed < 0.15, "max speed {max_speed} m/s after 2 s at rest");
    assert!(compression < 0.03, "mean compression {compression}");
}

/// Every preset at the GPU defaults (FLIP's resolution, 20 iterations) keeps the
/// invariants: no NaNs, nothing outside the domain or inside walls, counts conserved.
#[test]
fn gpu_pbf_keeps_invariants_on_every_preset() {
    if gpu().is_none() {
        return;
    }
    let input = StepInput::default();
    for (pi, preset) in PRESETS.iter().enumerate() {
        let mut world = World::with_solvers(sim_gpu::SOLVERS, 0, pi);
        assert_eq!(world.solver.name(), "PBF (GPU)");
        for frame in 0..120 {
            world.advance(1.0 / 60.0, &input);
            let violations = invariants::check(&world);
            assert!(violations.is_empty(), "{} frame {frame}: {violations:?}", preset.name);
        }
    }
}
