//! Headless runner: `cargo run -p bench --release -- --solver flip --frames 600`.
//!
//! Reports frame timing with a per-phase breakdown, the worst value of each solver
//! metric, and optionally how it all scales with thread count (`--sweep-threads`).
//! Exits non-zero if `--check` finds an invariant violation.

use anyhow::{Context, bail};
use clap::Parser;
use sim::{PRESETS, SOLVERS, StepInput, World, invariants};
use std::process::ExitCode;
use std::time::Duration;

#[derive(Parser)]
struct Args {
    /// Solver name (case-insensitive prefix match against the registry).
    #[arg(long, default_value = "flip")]
    solver: String,
    /// Preset name (case-insensitive prefix match).
    #[arg(long, default_value = "dam")]
    preset: String,
    #[arg(long, default_value_t = 600)]
    frames: u32,
    /// Rayon worker threads (default: `sim::default_threads()`).
    #[arg(long)]
    threads: Option<usize>,
    /// Run with 1, 2, 4, ... threads up to all cores and print a scaling table.
    #[arg(long)]
    sweep_threads: bool,
    /// Run invariant checks every frame and fail on the first violation.
    #[arg(long)]
    check: bool,
    /// Set a solver parameter by its key, e.g. `--set cells_y=140`. Repeatable.
    #[arg(long = "set", value_name = "KEY=VALUE")]
    sets: Vec<String>,
}

/// Applies `--set key=value` pairs through the solver's parameter visitor, the same
/// interface the UI uses, so the bench needs no knowledge of any particular solver.
struct CliParams<'a> {
    pending: Vec<(&'a str, &'a str)>,
    restart: bool,
    error: Option<String>,
}

impl CliParams<'_> {
    fn take(&mut self, key: &str) -> Option<String> {
        let k = self.pending.iter().position(|(name, _)| *name == key)?;
        Some(self.pending.swap_remove(k).1.to_string())
    }

    fn parse<T: std::str::FromStr>(&mut self, key: &str, value: &str) -> Option<T> {
        let parsed = value.parse().ok();
        if parsed.is_none() {
            self.error = Some(format!("can't parse {value:?} for {key}"));
        }
        parsed
    }
}

impl sim::ParamVisitor for CliParams<'_> {
    fn float(&mut self, spec: &sim::FloatSpec, value: &mut f32) {
        if let Some(raw) = self.take(spec.key)
            && let Some(v) = self.parse::<f32>(spec.key, &raw)
        {
            *value = v.clamp(*spec.range.start(), *spec.range.end());
            self.restart |= spec.restart;
        }
    }

    fn int(&mut self, spec: &sim::IntSpec, value: &mut u32) {
        if let Some(raw) = self.take(spec.key)
            && let Some(v) = self.parse::<u32>(spec.key, &raw)
        {
            *value = v.clamp(*spec.range.start(), *spec.range.end());
            self.restart |= spec.restart;
        }
    }

    fn toggle(&mut self, spec: &sim::ToggleSpec, value: &mut bool) {
        if let Some(raw) = self.take(spec.key)
            && let Some(v) = self.parse::<bool>(spec.key, &raw)
        {
            *value = v;
        }
    }
}

fn apply_sets(world: &mut World, sets: &[String]) -> anyhow::Result<()> {
    let pending = sets
        .iter()
        .map(|s| s.split_once('=').with_context(|| format!("--set expects KEY=VALUE, got {s:?}")))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let mut visitor = CliParams { pending, restart: false, error: None };
    world.solver.visit_params(&mut visitor);
    if let Some(e) = visitor.error {
        bail!(e);
    }
    if let Some((key, _)) = visitor.pending.first() {
        bail!("{} has no parameter {key:?}", world.solver.name());
    }
    if visitor.restart {
        world.reset();
    }
    Ok(())
}

/// Results of one run at one thread count.
struct Run {
    particles: usize,
    frame_ms: Vec<f64>,
    substeps: f64,
    dropped: f64,
    /// Mean ms per frame for each solver phase, in the solver's order.
    phases: Vec<(&'static str, f64)>,
    /// Worst value seen for each solver metric.
    metrics: Vec<(&'static str, f32)>,
}

impl Run {
    fn mean(&self) -> f64 {
        self.frame_ms.iter().sum::<f64>() / self.frame_ms.len() as f64
    }

    fn pct(&self, p: f64) -> f64 {
        let mut v = self.frame_ms.clone();
        v.sort_by(f64::total_cmp);
        v[((v.len() - 1) as f64 * p) as usize]
    }
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

fn run_once(args: &Args, solver: usize, preset: usize) -> anyhow::Result<Run> {
    let mut world = World::new(solver, preset);
    apply_sets(&mut world, &args.sets)?;
    let input = StepInput::default();
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    let mut run = Run {
        particles: world.initial_particles,
        frame_ms: Vec::with_capacity(args.frames as usize),
        substeps: 0.0,
        dropped: 0.0,
        phases: Vec::new(),
        metrics: Vec::new(),
    };
    for frame in 0..args.frames {
        let report = world.advance(1.0 / 60.0, &input);
        run.frame_ms.push(ms(report.wall));
        run.substeps += f64::from(report.substeps);
        run.dropped += f64::from(report.time_dropped);
        for (name, d) in report.phases {
            match run.phases.iter_mut().find(|(n, _)| *n == name) {
                Some((_, total)) => *total += ms(d),
                None => run.phases.push((name, ms(d))),
            }
        }
        for m in &world.solver.stats().metrics {
            match run.metrics.iter_mut().find(|(n, _)| *n == m.name) {
                Some((_, worst)) => *worst = worst.max(m.value),
                None => run.metrics.push((m.name, m.value)),
            }
        }
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
    let frames = f64::from(args.frames);
    run.substeps /= frames;
    run.phases.iter_mut().for_each(|(_, t)| *t /= frames);
    Ok(run)
}

fn run(args: &Args) -> anyhow::Result<()> {
    let si = find(SOLVERS, |s| s.name, &args.solver)?;
    let pi = find(PRESETS, |p| p.name, &args.preset)?;
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let thread_counts: Vec<usize> = if args.sweep_threads {
        let mut v: Vec<usize> = std::iter::successors(Some(1), |&n| (n * 2 < cores).then_some(n * 2)).collect();
        v.push(cores);
        v
    } else {
        vec![args.threads.unwrap_or_else(sim::default_threads)]
    };

    let mut runs = Vec::new();
    for &threads in &thread_counts {
        // A pool per run: the global pool can only be configured once per process.
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build()?;
        runs.push((threads, pool.install(|| run_once(args, si, pi))?));
    }

    let (_, first) = &runs[0];
    println!(
        "{} / {}: {} particles, {} frames, {:.2} substeps/frame, {:.3} s of {:.3} s dropped",
        SOLVERS[si].name,
        PRESETS[pi].name,
        first.particles,
        args.frames,
        first.substeps,
        first.dropped,
        f64::from(args.frames) / 60.0
    );

    // One row per thread count: frame stats, then mean ms/frame per phase.
    let phase_names: Vec<&str> = first.phases.iter().map(|(n, _)| *n).collect();
    print!("{:>7} {:>8} {:>8} {:>8} {:>8}", "threads", "mean", "p50", "p95", "max");
    for name in &phase_names {
        print!(" {:>9}", truncate(name, 9));
    }
    println!();
    let baseline = first.mean();
    for (threads, r) in &runs {
        print!("{:>7} {:>8.2} {:>8.2} {:>8.2} {:>8.2}", threads, r.mean(), r.pct(0.5), r.pct(0.95), r.pct(1.0));
        for name in &phase_names {
            let t = r.phases.iter().find(|(n, _)| n == name).map_or(0.0, |(_, t)| *t);
            print!(" {:>9.2}", t);
        }
        if runs.len() > 1 {
            print!("   x{:.1}", baseline / r.mean());
        }
        println!();
    }
    println!("(all times ms per frame)");
    for (name, worst) in &first.metrics {
        println!("worst {name}: {worst:.3}");
    }
    if args.check {
        println!("invariants: ok");
    }
    Ok(())
}

fn truncate(s: &str, n: usize) -> &str {
    &s[..s.len().min(n)]
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
