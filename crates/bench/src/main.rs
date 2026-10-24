//! Headless runner: `cargo run -p bench --release -- --solver ballistic --frames 600`.
//!
//! Phase 0 version: frame timing and invariant checks. Per-phase breakdowns and
//! thread sweeps come in Phase 3.

use anyhow::{Context, bail};
use clap::Parser;
use sim::{PRESETS, SOLVERS, StepInput, World, invariants};
use std::process::ExitCode;
use std::time::Duration;

#[derive(Parser)]
struct Args {
    /// Solver name (case-insensitive prefix match against the registry).
    #[arg(long, default_value = "ballistic")]
    solver: String,
    /// Preset name (case-insensitive prefix match).
    #[arg(long, default_value = "dam")]
    preset: String,
    #[arg(long, default_value_t = 600)]
    frames: u32,
    /// Rayon worker threads (default: all cores).
    #[arg(long)]
    threads: Option<usize>,
    /// Run invariant checks every frame and fail on the first violation.
    #[arg(long)]
    check: bool,
}

fn find<T>(items: &[T], name: impl Fn(&T) -> &str, query: &str) -> anyhow::Result<usize> {
    let q = query.to_lowercase();
    items
        .iter()
        .position(|it| name(it).to_lowercase().starts_with(&q))
        .with_context(|| {
            let names: Vec<_> = items.iter().map(&name).collect();
            format!("no match for {query:?}; available: {}", names.join(", "))
        })
}

fn run(args: &Args) -> anyhow::Result<()> {
    if let Some(n) = args.threads {
        rayon::ThreadPoolBuilder::new().num_threads(n).build_global()?;
    }
    let si = find(SOLVERS, |s| s.name, &args.solver)?;
    let pi = find(PRESETS, |p| p.name, &args.preset)?;
    let mut world = World::new(si, pi);
    println!(
        "{} / {}: {} particles, {} threads",
        SOLVERS[si].name,
        PRESETS[pi].name,
        world.initial_particles,
        rayon::current_num_threads()
    );

    let input = StepInput::default();
    let mut walls = Vec::with_capacity(args.frames as usize);
    let mut substeps = 0u64;
    let mut dropped = 0.0f64;
    for frame in 0..args.frames {
        let report = world.advance(1.0 / 60.0, &input);
        walls.push(report.wall);
        substeps += u64::from(report.substeps);
        dropped += f64::from(report.time_dropped);
        if args.check {
            let violations = invariants::check(&world);
            if !violations.is_empty() {
                for v in &violations {
                    eprintln!("frame {frame}: {v}");
                }
                bail!("invariants violated at frame {frame}");
            }
        }
    }

    walls.sort();
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    let mean = walls.iter().sum::<Duration>() / walls.len().max(1) as u32;
    let pct = |p: f64| walls[((walls.len() - 1) as f64 * p) as usize];
    println!(
        "frame ms: mean {:.3}  p50 {:.3}  p95 {:.3}  max {:.3}",
        ms(mean),
        ms(pct(0.5)),
        ms(pct(0.95)),
        ms(*walls.last().unwrap())
    );
    println!(
        "substeps/frame {:.2}, sim time dropped {:.3} s of {:.3} s",
        substeps as f64 / f64::from(args.frames),
        dropped,
        f64::from(args.frames) / 60.0
    );
    if args.check {
        println!("invariants: ok");
    }
    Ok(())
}

fn main() -> ExitCode {
    let args = Args::parse();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
