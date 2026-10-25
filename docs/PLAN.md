# Fluid Toy: Plan and Architecture

A weekend plan for a real-time, interactive 2D fluid toy in Rust with wgpu.
It covers recommendations on the open questions, the architecture, a phased schedule
with a playable milestone per phase, the Rust concepts each phase exercises, testing,
risks, and references.

Written 2026-10-23. Crate versions below were checked against crates.io on that date.

---

## 0. TL;DR of the recommendations

| Question | Recommendation |
|---|---|
| Dimensionality | 2D only. Agreed. |
| Where the sim runs | CPU, plain Rust and `rayon`. wgpu renders only. Compute shaders are a stretch goal. Agreed. |
| First solver | **FLIP** (PIC/FLIP blend on a staggered MAC grid), ported from the structure of Ten Minute Physics' FLIP episode. Agreed, with one caveat: FLIP is a particle/grid *hybrid*. If "particle-based" was meant to be strictly meshless, swap the order and do PBF first. |
| Second solver | **PBF** (Position Based Fluids), not WCSPH. It is stable at similar timesteps to FLIP, so it tests the abstraction without making the app handle a 10× smaller dt. It shares no grid code with FLIP. Its parallel pattern (pure gather) is the opposite of FLIP's scatter. |
| Solver zero | A trivial **`Ballistic`** solver (gravity plus bouncing) on Friday. It gets pixels on screen before any fluid math exists, it is the first implementor of the trait, and it stays useful as a test fixture. |
| Solver trait | Object-safe `trait Solver: Send`, held as `Box<dyn Solver>`. Parameters are exposed through a **visitor** (`visit_params(&mut self, &mut dyn ParamVisitor)`), particles through a **borrowed view**, and grid fields through **fill-a-caller-buffer**. See §3. |
| GPU-resident future solver | Don't design for it now. Keep one seam open: output is *pulled* through views and never held across steps. A GPU solver can satisfy that by readback, and get a zero-copy path later through an app-side extension trait. |
| Parallel scatter (P2G) | Serial first. Then a rayon `fold`/`reduce` over thread-local grids (simple and safe, and 2D grids are small). Then optionally a sort-by-cell **gather**, which also yields the neighbor structure PBF needs. Avoid atomics. |
| Parallel pressure solve | Serial Gauss–Seidel with SOR to start (matches the reference). Then **Jacobi with ping-pong buffers** or **red–black with separate red/black arrays**, both safe and parallel. PCG (Bridson ch. 5) is a stretch goal. |
| Timestep | Variable frame time (measured, clamped to ≤ 1/30 s, times a speed slider), split into CFL-limited substeps with a hard cap. If the cap is hit, drop time (slow motion), never stability. The headless bench uses a fixed 1/60. |
| Toolchain | Native Windows MSVC toolchain, not WSL. **Update Rust**: 1.83 is installed, and egui 0.36 needs ≥ 1.95. |

---

## 1. Environment setup (do this first on Friday, ~30 min)

**Why native Windows and not WSL.** WSLg can open windows, but GPU access from WSL goes
through a translation layer (Vulkan-on-D3D12). It is the most common source of "wgpu
can't find an adapter" or very slow presentation. Native MSVC Rust with wgpu's default
DX12 backend (or Vulkan) just works. The repo is already on `D:\`, which is right.
Don't move it into the WSL filesystem, and don't build it from WSL (that would
target Linux).

Steps:

1. `rustup update stable`. The machine has rustc 1.83.0, and egui/egui-wgpu 0.36.x
   declare `rust-version = 1.95`.
2. Add a `rust-toolchain.toml` pinning the channel you end up on (for example
   `channel = "1.95"` or whatever `rustup` installs), so the weekend never sees a
   surprise toolchain change.
3. Editor: VS Code with rust-analyzer, or RustRover. Install the CodeLLDB extension
   (or use the MSVC debugger) for stepping through the solver.
4. Optional: add a Windows Defender exclusion for the `target\` folder. Real-time
   scanning noticeably slows Rust builds. It's your call, since it is a security
   setting.
5. Build profiles (in the workspace `Cargo.toml`, §2). A debug-build fluid sim is
   10–50× slower. Optimize dependencies and give your own code `opt-level = 1`.

Pinned versions (all mutually compatible as of 2026-10-23):

| Crate | Pin | Notes |
|---|---|---|
| `wgpu` | `=30.0.1` | Required by egui-wgpu 0.36.x and 0.37.0 (`^30.0`). |
| `winit` | `=0.30.13` | egui-winit 0.36/0.37 require `^0.30.13`. **winit 0.31 is in beta, so don't use it.** |
| `egui`, `egui-wgpu`, `egui-winit` | `=0.36.2` | Released 2026-09-08. 0.37.0 (2026-10-15) uses the same wgpu/winit and is a fine alternative, but 0.36.2 has had seven weeks to shake out. All three must be the same version. |
| `rayon` | `1.12` | |
| `glam` | `0.34` | Enable its `bytemuck` feature so `&[Vec2]` casts to bytes for GPU upload. If that feature name has changed, check `cargo add glam -F bytemuck`. |
| `bytemuck` | `1` (with `derive`) | egui-wgpu already requires `^1.25`. |
| `pollster` | `0.4` | Blocks on the async adapter and device requests. |
| `anyhow` / `thiserror` | `1` / `2` | `anyhow` in app and bench, `thiserror` in sim. |
| `clap` | `4` (with `derive`) | Bench CLI. |
| `log` / `env_logger` | `0.4` / `0.11` | wgpu validation errors arrive via `log`. **Always have a logger installed.** |

Sanity check after adding deps: `cargo tree -d -e normal | findstr wgpu` should show
exactly one wgpu version. Two versions means egui-wgpu and your direct dep disagree.

---

## 2. Workspace and module layout

```
FluidsSimulation/
├─ Cargo.toml               # workspace, pinned deps, profiles
├─ rust-toolchain.toml
├─ docs/PLAN.md             # this file
└─ crates/
   ├─ sim/                  # NO wgpu, NO egui, NO winit. Pure Rust, testable headless.
   │  ├─ Cargo.toml         # glam, rayon, thiserror
   │  ├─ src/
   │  │  ├─ lib.rs
   │  │  ├─ solver.rs       # Solver trait, ParticleView, FieldInfo/FieldLayout, Init, Stats
   │  │  ├─ params.rs       # ParamVisitor + param specs
   │  │  ├─ scene.rs        # Scene, SolidMask (+ SDF), Emitter, Drain, ForceBrush, StepInput
   │  │  ├─ presets.rs      # dam break, double dam break, tank+obstacle, fountain
   │  │  ├─ world.rs        # World = Scene + Box<dyn Solver> + Stepper; solver switching
   │  │  ├─ stepper.rs      # frame -> CFL substeps
   │  │  ├─ registry.rs     # static list of solvers (the ONLY file touched to add one)
   │  │  ├─ invariants.rs   # checks shared by tests and bench
   │  │  ├─ timing.rs       # tiny phase-timer for Stats
   │  │  ├─ grid/
   │  │  │  ├─ mod.rs       # Grid2<T>
   │  │  │  └─ mac.rs       # MacGrid: staggered u/v, CellType, interpolation
   │  │  ├─ neighbors.rs    # counting-sort cell binning (PBF, FLIP particle separation)
   │  │  └─ solvers/
   │  │     ├─ ballistic.rs
   │  │     ├─ flip/{mod.rs, transfer.rs, pressure.rs, separate.rs}
   │  │     └─ pbf/{mod.rs, kernels.rs}
   │  └─ tests/
   │     ├─ all_solvers.rs  # every registered solver × every preset passes invariants
   │     └─ flip_unit.rs
   ├─ app/                  # winit + wgpu + egui. Knows nothing about FLIP or PBF.
   │  ├─ Cargo.toml
   │  └─ src/
   │     ├─ main.rs         # ApplicationHandler, event loop
   │     ├─ gpu.rs          # instance/adapter/device/queue/surface/config, resize
   │     ├─ view.rs         # world <-> screen transform (aspect fit, DPI)
   │     ├─ input.rs        # mouse/keyboard -> tools -> StepInput + scene edits
   │     ├─ ui.rs           # egui panel; EguiParams: impl ParamVisitor
   │     └─ render/
   │        ├─ mod.rs       # frame orchestration, render pass, egui pass
   │        ├─ particles.rs # instanced circles
   │        ├─ texture_quad.rs # solid mask and field overlays (one pipeline, two uses)
   │        └─ shaders/{particles.wgsl, quad.wgsl}
   └─ bench/                # headless: timing, thread scaling, invariant checks
      ├─ Cargo.toml         # sim, clap, anyhow
      └─ src/main.rs
```

The dependency rule is enforced by Cargo: `sim` doesn't list wgpu or egui, so it
*cannot* reference them. "Adding a solver touches only `sim/src/solvers/<new>` plus one
line in `registry.rs`" is checkable on Sunday. `git diff --stat crates/app` should be
empty after adding PBF.

### Workspace `Cargo.toml`

```toml
[workspace]
resolver = "3"
members = ["crates/sim", "crates/app", "crates/bench"]

[workspace.package]
edition = "2024"
rust-version = "1.95"

[workspace.dependencies]
wgpu       = "=30.0.1"
winit      = "=0.30.13"
egui       = "=0.36.2"
egui-wgpu  = "=0.36.2"
egui-winit = "=0.36.2"
rayon      = "1.12"
glam       = { version = "0.34", features = ["bytemuck"] }
bytemuck   = { version = "1", features = ["derive"] }
pollster   = "0.4"
anyhow     = "1"
thiserror  = "2"
clap       = { version = "4", features = ["derive"] }
log        = "0.4"
env_logger = "0.11"

# Dev builds: our code lightly optimized, all dependencies fully optimized.
[profile.dev]
opt-level = 1
[profile.dev.package."*"]
opt-level = 3

# Faster-to-debug release when profiling: keep symbols.
[profile.release]
debug = "line-tables-only"
```

Member crates use `wgpu.workspace = true`, and so on. Commit `Cargo.lock`. It pins
everything that isn't `=`-pinned.

---

## 3. Core types and traits (`sim` crate)

### 3.1 Grids

```rust
// sim/src/grid/mod.rs
use std::ops::{Index, IndexMut};

/// Dense row-major 2D array. `j` is the row (y), `i` the column (x).
#[derive(Clone, Debug)]
pub struct Grid2<T> {
    nx: usize,
    ny: usize,
    data: Vec<T>,
}

impl<T: Copy + Default> Grid2<T> {
    pub fn new(nx: usize, ny: usize) -> Self {
        Self { nx, ny, data: vec![T::default(); nx * ny] }
    }
    pub fn fill(&mut self, v: T) { self.data.fill(v); }
}

impl<T> Grid2<T> {
    #[inline]
    pub fn idx(&self, i: usize, j: usize) -> usize {
        debug_assert!(i < self.nx && j < self.ny, "({i},{j}) out of {}x{}", self.nx, self.ny);
        j * self.nx + i
    }
    pub fn dims(&self) -> (usize, usize) { (self.nx, self.ny) }
    pub fn as_slice(&self) -> &[T] { &self.data }
    pub fn as_mut_slice(&mut self) -> &mut [T] { &mut self.data }
    /// Rows as disjoint mutable slices, the basis of safe row-parallel loops.
    pub fn rows_mut(&mut self) -> std::slice::ChunksExactMut<'_, T> {
        self.data.chunks_exact_mut(self.nx)
    }
}

impl<T> Index<(usize, usize)> for Grid2<T> {
    type Output = T;
    #[inline] fn index(&self, (i, j): (usize, usize)) -> &T { &self.data[self.idx(i, j)] }
}
impl<T> IndexMut<(usize, usize)> for Grid2<T> {
    #[inline] fn index_mut(&mut self, (i, j): (usize, usize)) -> &mut T {
        let k = self.idx(i, j);
        &mut self.data[k]
    }
}
```

```rust
// sim/src/grid/mac.rs
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum CellType { #[default] Air, Fluid, Solid }

/// Staggered MAC grid. u lives on x-faces at (i·h, (j+½)·h), v on y-faces at ((i+½)·h, j·h).
pub struct MacGrid {
    pub nx: usize,
    pub ny: usize,
    pub h: f32,
    pub u: Grid2<f32>,      // (nx+1) × ny
    pub v: Grid2<f32>,      // nx × (ny+1)
    pub cell: Grid2<CellType>,
}

impl MacGrid {
    /// Bilinear sample of a face-staggered component. `offset` is the sample-lattice
    /// offset in cells: u uses (0, 0.5), v uses (0.5, 0). Off-by-half here is the #1 FLIP bug.
    pub fn sample(field: &Grid2<f32>, h: f32, offset: glam::Vec2, p: glam::Vec2) -> f32 { /* ... */ }
}
```

### 3.2 Scene: solver-agnostic world description

Obstacles and walls are a single **solid mask** at its own fixed resolution
(for example 256×128), independent of any solver's grid. Its border is always solid,
so domain walls and painted obstacles are the same thing. Solvers resample it.
FLIP marks cells solid. PBF uses the signed distance field for collisions.

```rust
// sim/src/scene.rs
use glam::Vec2;

pub struct Scene {
    pub size: Vec2,               // domain is [0, size.x] × [0, size.y], metres
    pub gravity: Vec2,            // tilt tool rotates this
    pub solid: SolidMask,
    pub fluid_blocks: Vec<Rect>,  // initial fill
    pub emitters: Vec<Emitter>,
    pub drains: Vec<Rect>,        // particles entering are removed (fountain needs these)
    pub max_particles: usize,
}

pub struct SolidMask {
    nx: usize,
    ny: usize,
    cell: f32,
    occ: Vec<u8>,                 // 0 = open, 255 = solid (uploads directly as R8Unorm)
    sdf: Vec<f32>,                // signed distance, negative inside solids
    sdf_dirty: bool,
    generation: u64,              // bumped on every edit; solvers cache derived data by it
}

impl SolidMask {
    pub fn paint_disc(&mut self, c: Vec2, r: f32, solid: bool) { /* set occ; dirty; generation += 1 */ }
    pub fn is_solid(&self, p: Vec2) -> bool { /* ... */ }
    pub fn distance(&self, p: Vec2) -> f32 { /* bilinear on sdf */ }
    pub fn normal(&self, p: Vec2) -> Vec2 { /* central-difference gradient of sdf */ }
    pub fn generation(&self) -> u64 { self.generation }
    pub fn occupancy(&self) -> (usize, usize, &[u8]) { (self.nx, self.ny, &self.occ) }
    /// Called once per frame by the stepper, not per query, so queries can stay `&self`.
    pub(crate) fn update_derived(&mut self) {
        if self.sdf_dirty { /* two-pass chamfer or Felzenszwalb EDT */ self.sdf_dirty = false; }
    }
}

pub struct Emitter { pub pos: Vec2, pub dir: Vec2, pub speed: f32, pub width: f32, pub rate: f32, pub enabled: bool }

/// Per-frame user interaction. Solvers interpret it, and particle solvers share a helper.
#[derive(Default)]
pub struct StepInput {
    pub brushes: Vec<ForceBrush>,
    pub pulse: bool,              // fire all emitters once (audio-reactive hook)
}
pub struct ForceBrush { pub pos: Vec2, pub radius: f32, pub velocity: Vec2, pub strength: f32 }
```

**Design note:** `update_derived(&mut self)` once per frame avoids interior mutability
(`RefCell`/`OnceCell` for a lazily rebuilt SDF). Mutate in one known place, then hand
out `&Scene` to everything else. That is idiomatic, and it keeps `Scene: Sync` for
rayon.

### 3.3 Parameters: a visitor, not a string-keyed map

The UI must edit solver parameters without knowing which solver is active. Options
considered:

- **String-keyed `HashMap<String, f32>`**: easy, but the solver reads its own params by
  string lookup in hot code, there are no types, and typos fail at runtime.
- **Descriptor list plus `get`/`set` by id**: OK, but lots of boilerplate and an id enum
  per solver.
- **Visitor (recommended):** the solver owns a plain typed struct and walks it,
  handing out `&mut f32` with a static descriptor. The UI, the bench CLI, and a future
  preset-file loader are just different visitors. No allocation, no lookups, and the
  borrow checker guarantees the UI can't hold the reference past the call.

```rust
// sim/src/params.rs
use std::ops::RangeInclusive;

pub struct FloatSpec {
    pub key: &'static str,            // stable id for CLI/preset files: "flip_ratio"
    pub label: &'static str,          // UI text: "PIC/FLIP blend"
    pub range: RangeInclusive<f32>,
    pub log: bool,
    pub restart: bool,                // changing it requires solver.reset() (e.g. grid resolution)
    pub help: &'static str,
}
pub struct IntSpec    { pub key: &'static str, pub label: &'static str, pub range: RangeInclusive<u32>, pub restart: bool, pub help: &'static str }
pub struct ToggleSpec { pub key: &'static str, pub label: &'static str, pub help: &'static str }

pub trait ParamVisitor {
    fn float(&mut self, spec: &FloatSpec, value: &mut f32);
    fn int(&mut self, spec: &IntSpec, value: &mut u32);
    fn toggle(&mut self, spec: &ToggleSpec, value: &mut bool);
    fn section(&mut self, _title: &'static str) {}
}
```

Solver side:

```rust
// sim/src/solvers/flip/mod.rs
const FLIP_RATIO: FloatSpec = FloatSpec {
    key: "flip_ratio", label: "PIC/FLIP blend", range: 0.0..=1.0, log: false, restart: false,
    help: "0 = PIC (smooth, viscous), 1 = FLIP (lively, noisy)",
};
const RESOLUTION: IntSpec = IntSpec {
    key: "cells_y", label: "Grid cells (height)", range: 16..=256, restart: true, help: "",
};

impl Solver for Flip {
    fn visit_params(&mut self, v: &mut dyn ParamVisitor) {
        let p = &mut self.params;
        v.section("Transfer");
        v.float(&FLIP_RATIO, &mut p.flip_ratio);
        v.section("Pressure");
        v.int(&PRESSURE_ITERS, &mut p.pressure_iters);
        v.float(&OVER_RELAX, &mut p.over_relax);
        v.toggle(&DRIFT_COMP, &mut p.compensate_drift);
        v.section("Grid");
        v.int(&RESOLUTION, &mut p.cells_y);
        // ...
    }
    // ...
}
```

App side (in `app`, so egui never leaks into `sim`):

```rust
// app/src/ui.rs
pub struct EguiParams<'u> {
    pub ui: &'u mut egui::Ui,
    pub restart_requested: bool,
}

impl sim::ParamVisitor for EguiParams<'_> {
    fn float(&mut self, s: &sim::FloatSpec, v: &mut f32) {
        let r = self.ui
            .add(egui::Slider::new(v, s.range.clone()).text(s.label).logarithmic(s.log))
            .on_hover_text(s.help);
        // Restart-type params fire on release, not on every drag tick.
        self.restart_requested |= s.restart && (r.drag_stopped() || (r.changed() && !r.dragged()));
    }
    fn int(&mut self, s: &sim::IntSpec, v: &mut u32) { /* same with Slider<u32> */ }
    fn toggle(&mut self, s: &sim::ToggleSpec, v: &mut bool) { self.ui.checkbox(v, s.label).on_hover_text(s.help); }
    fn section(&mut self, title: &'static str) { self.ui.separator(); self.ui.strong(title); }
}
```

Stretch Rust exercise: write a `macro_rules! params { ... }` that generates the param
struct, its `Default`, the `const` specs, and `visit_params` from one table.

### 3.4 The `Solver` trait

```rust
// sim/src/solver.rs
use glam::Vec2;

/// Borrowed particle state. Valid only until the next `&mut` call on the solver,
/// and the borrow checker enforces that.
pub struct ParticleView<'a> {
    pub pos: &'a [Vec2],          // SoA layout -> bytemuck::cast_slice -> GPU, zero copies
    pub vel: &'a [Vec2],
    pub radius: f32,
}

#[derive(Clone, Copy)]
pub enum FieldKind { Signed, NonNegative, Categorical }

pub struct FieldInfo { pub key: &'static str, pub label: &'static str, pub kind: FieldKind }

/// Where a filled field sits in world space: sample (i, j) is centered at
/// origin + (i + 0.5, j + 0.5) * cell.
pub struct FieldLayout { pub nx: usize, pub ny: usize, pub origin: Vec2, pub cell: f32 }

/// How a solver is (re)initialised: from the scene's fluid blocks, or by adopting
/// another solver's particles (switch solvers mid-splash).
pub enum Init<'a> { FromScene, FromParticles(ParticleView<'a>) }

#[derive(Default, Clone)]
pub struct Stats {
    pub particles: usize,
    pub emitted: usize,                                  // cumulative, for the count invariant
    pub drained: usize,
    pub phases: Vec<(&'static str, std::time::Duration)>, // last step's breakdown
    pub metrics: Vec<Metric>,                            // e.g. max divergence, density error
}

#[derive(Clone)]
pub struct Metric { pub name: &'static str, pub value: f32, pub tolerance: Option<f32> }

pub trait Solver: Send {
    fn name(&self) -> &'static str;

    fn reset(&mut self, scene: &Scene, init: Init<'_>);

    /// Largest substep the solver considers stable *right now* (CFL etc).
    fn max_dt(&self, scene: &Scene) -> f32;

    fn step(&mut self, dt: f32, scene: &Scene, input: &StepInput);

    fn visit_params(&mut self, v: &mut dyn ParamVisitor);

    /// None for pure grid solvers (stable fluids, LBM).
    fn particles(&self) -> Option<ParticleView<'_>>;

    /// Static list of fields this solver can produce, indexed by position.
    fn fields(&self) -> &'static [FieldInfo] { &[] }

    /// Write field `index` into a caller-owned buffer. A buffer rather than a borrowed
    /// slice, because fields are often *derived* (speed, divergence) and need computing.
    /// The caller reuses its Vec, so no per-frame allocation, and the solver needs no
    /// interior mutability to cache results.
    fn fill_field(&self, _index: usize, _out: &mut Vec<f32>) -> Option<FieldLayout> { None }

    fn stats(&self) -> &Stats;
}
```

Why these shapes:

- **Object-safe** (no generic methods, no `Self` returns), so `Box<dyn Solver>` works and
  runtime switching is just a box swap.
- **`Send`**, so the whole `World` can later be moved to a sim thread without touching
  the trait.
- **The scene is passed in, not owned**, so one `Scene` survives solver switches.
  Painted walls persist, and so does gravity tilt.
- **No wgpu types.** A future GPU solver lives in its own crate (`sim-gpu`, depending on
  `sim` and `wgpu`). It implements `Solver` with `particles()` doing a readback (fine
  for the bench and tests), and also implements an app-side
  `trait GpuParticleSource { fn buffers(&self) -> (&wgpu::Buffer, &wgpu::Buffer, u32); }`
  for zero-copy rendering. The registry entry for GPU solvers would hand back both
  facets. Don't build any of this until a GPU solver exists.

### 3.5 Registry, World, Stepper

```rust
// sim/src/registry.rs: the only shared file a new solver touches.
pub struct SolverEntry {
    pub name: &'static str,
    pub make: fn() -> Box<dyn Solver>,
}

pub static SOLVERS: &[SolverEntry] = &[
    SolverEntry { name: "Ballistic (debug)", make: || Box::new(Ballistic::default()) },
    SolverEntry { name: "FLIP",              make: || Box::new(Flip::default()) },
    SolverEntry { name: "PBF",               make: || Box::new(Pbf::default()) },
];
```

(Non-capturing closures coerce to `fn` pointers, which is a nice small Rust fact to
notice here.)

```rust
// sim/src/world.rs
pub struct World {
    pub scene: Scene,
    pub solver: Box<dyn Solver>,
    pub solver_index: usize,
    pub stepper: Stepper,
    pub time: f64,
}

impl World {
    pub fn switch_solver(&mut self, index: usize, keep_particles: bool) {
        let mut next = (SOLVERS[index].make)();
        match self.solver.particles().filter(|_| keep_particles) {
            Some(view) => next.reset(&self.scene, Init::FromParticles(view)),
            None => next.reset(&self.scene, Init::FromScene),
        }
        self.solver = next;   // old solver dropped here, after the borrow above ended
        self.solver_index = index;
    }

    pub fn advance(&mut self, frame_dt: f32, input: &StepInput) -> FrameReport {
        self.scene.solid.update_derived();
        self.stepper.advance(self.solver.as_mut(), &self.scene, frame_dt, input)
    }
}
```

```rust
// sim/src/stepper.rs
pub struct Stepper { pub max_substeps: u32, pub min_substeps: u32 }

pub struct FrameReport { pub substeps: u32, pub dt: f32, pub time_dropped: f32, pub wall: Duration }

impl Stepper {
    pub fn advance(&self, s: &mut dyn Solver, scene: &Scene, target: f32, input: &StepInput) -> FrameReport {
        let t0 = Instant::now();
        let max_dt = s.max_dt(scene).max(1e-6);
        // Even split: n substeps of equal size, n chosen by CFL, then capped.
        let wanted = (target / max_dt).ceil() as u32;
        let n = wanted.clamp(self.min_substeps.max(1), self.max_substeps);
        let dt = (target / n as f32).min(max_dt);         // if capped, dt = max_dt and time is dropped
        for _ in 0..n { s.step(dt, scene, input); }
        FrameReport { substeps: n, dt, time_dropped: target - dt * n as f32, wall: t0.elapsed() }
    }
}
```

**Timestep policy:**

- **App:** `frame_dt = min(measured_wall_dt, 1/30) * speed_slider`. The variable frame
  time keeps real-time feel on 60/144 Hz monitors alike, and the clamp prevents a
  catch-up explosion after a window drag or breakpoint. Single-step advances 1/60.
- **Bench:** fixed `frame_dt = 1/60`, so runs are comparable.
- **CFL (FLIP):** `max_dt = cfl * h / max_particle_speed`, with `cfl ≈ 1.0–2.0`. FLIP
  advects with particles, so it tolerates more than an Eulerian scheme. Ten Minute
  Physics simply uses 2 fixed substeps. Expose `cfl` and `min_substeps` as params.
- **CFL (PBF):** `max_dt` is a param (default 1/120), further limited so that
  `speed * dt < 0.4 * kernel_radius`.
- **If the substep cap is hit,** time is dropped (slow motion) and `time_dropped` shows
  in the UI. Additionally, FLIP clamps particle speed to `k * h / dt` as a last-ditch
  guard. Never take a substep larger than `max_dt`.
- **Not doing:** the "Fix Your Timestep" accumulator with state interpolation. It gives
  deterministic replays, which a toy doesn't need, and it costs a second particle buffer.

---

## 4. Solver designs

### 4.1 FLIP (per substep)

> **As built in Phase 1**, the order differs from the list below, which follows TMP.
> Per substep: classify cells → P2G → save `u_prev` → gravity *on the grid* → solid
> boundaries → warm-started, non-negative pressure projection → G2P → advect particles
> through the grid (PIC) velocity → particle separation → collide with the grid's own
> solid cells. Seeding is a jittered 2×2 per cell, aligned with the grid. Drift
> compensation exists but is off by default. See the Phase 1 findings in §8 for why
> each change was needed.

Following Ten Minute Physics' structure, with Bridson ch. 7 for background:

1. **Integrate particles:** `v += g·dt`, apply force brushes (shared helper), `x += v·dt`.
2. **Separate particles** (optional, on by default): a few iterations pushing
   overlapping particles apart using `neighbors.rs` binning. This keeps the distribution
   even and prevents clumping. Skip it on Saturday morning and add it once the core works.
3. **Collide:** project particles out of solids using the scene SDF (`p -= n · d` when
   `d < r`), and kill the normal velocity component.
4. **Emit / drain:** emitters spawn particles (respecting `max_particles`), drains remove
   them (`swap_remove` keeps arrays dense).
5. **Mark cells:** resample `scene.solid` into `cell` (cached by `solid.generation()`),
   and mark cells containing particles `Fluid`, the rest `Air`.
6. **P2G:** splat particle velocities onto u and v faces with bilinear weights,
   accumulate weights, normalize, and keep `u_prev`/`v_prev` copies.
7. **Boundary:** zero face velocities adjacent to solid cells.
8. **Density for drift compensation:** splat particle count to cell centers. Rest
   density is from the initial fill. Subtracting `k·(ρ − ρ₀)` from the divergence
   counteracts volume loss. (It is in the TMP code and matters visually.)
9. **Pressure projection:** Gauss–Seidel with SOR (`ω ≈ 1.9`) over fluid cells,
   with `p = 0` in air (free surface) and Neumann at solids. Report max post-projection
   divergence as a `Metric`.
10. **G2P:** `v_pic = interp(u_new)`, `v_flip = v_p + interp(u_new − u_prev)`, and
    `v = lerp(v_pic, v_flip, flip_ratio)` with flip_ratio ≈ 0.9–0.95.

**Params:** flip_ratio, cells_y (restart), particles per cell (restart), pressure
iterations, over-relaxation, drift compensation toggle and stiffness, separation
toggle and iterations, cfl, max speed clamp.

**Fields:** pressure, divergence (pre- and post-projection), speed (cell-centered),
cell type (categorical), particle density.

**Parallelization path (Saturday evening):**

| Step | Pattern | Rust tools |
|---|---|---|
| Integrate, collide, G2P | Independent per particle | `pos.par_iter_mut().zip(vel.par_iter_mut())` |
| Mark cells, boundary, density normalize | Independent per cell or row | `grid.rows_mut().into_par_iter()` (`par_chunks_exact_mut`) |
| **P2G** (scatter) | v1: **thread-local grids** via `par_chunks(4096).fold(Accum::new, …).reduce(…)`. A 128×64 grid is ~33k floats per component, so summing a few per-thread copies is cheap. v2 (optional): counting-sort particles by cell, then **gather** per face row with `par_chunks_exact_mut`. No write conflicts, better cache locality, and the same binning PBF needs. | `fold`/`reduce`, `par_chunks_exact_mut`, counting sort |
| Pressure | v1: **Jacobi** with ping-pong buffers (`std::mem::swap`). It needs about twice as many iterations but is trivially safe-parallel. v2: **red–black GS with the checkerboard stored as two arrays** (`p_red`, `p_black`). Updating red writes only `p_red` while reading `p_black`, so the borrow checker is happy with no `unsafe`. Stretch: **PCG** (Bridson ch. 5). Its matvec is a gather, and dot/axpy map to `par_iter().map().sum()`. It gets real low divergence for the invariant check. | `mem::swap`, `split_at_mut`, `par_iter().sum()` |

**Avoid:** `AtomicU32` CAS loops for float adds (slow and noisy), and `unsafe` raw
pointer sharing "because the indices are disjoint". The two safe patterns above cover
it. Note that `fold`/`reduce` changes float summation order, so parallel results differ
from serial in the last bits. Compare with a tolerance, not equality.

### 4.2 PBF (per substep) [Macklin & Müller 2013]

1. Apply gravity and brushes, then predict `x* = x + dt·v`.
2. Bin `x*` into cells of size `h` (kernel radius) using `neighbors.rs` (counting sort:
   `cell_start`, `cell_count`, sorted indices).
3. Repeat for `iterations` (default 3–4):
   - `λᵢ = −Cᵢ / (Σⱼ|∇ⱼCᵢ|² + ε)` where `Cᵢ = ρᵢ/ρ₀ − 1`. This is a **gather** over
     neighbors, so it is `par_iter`.
   - `Δpᵢ = (1/ρ₀) Σⱼ (λᵢ + λⱼ + s_corr) ∇W(xᵢ − xⱼ)`. Also a gather, written into a
     separate `dp` buffer, then applied (Jacobi style).
   - Collide `x*` against the SDF.
4. `v = (x* − x)/dt`, then XSPH viscosity and optional vorticity confinement, then `x = x*`.

**2D kernel constants (a gotcha, since most references give 3D):**
poly6: `W = 4/(π h⁸) · (h² − r²)³`. Spiky gradient magnitude: `−30/(π h⁵) · (h − r)²`.
**Compute ρ₀ numerically** by evaluating the density of a particle in the interior of
the initial lattice. That avoids the "explodes on frame 1" rest-density mismatch.

**Params:** iterations, ε (relaxation), s_corr k and n, XSPH c, vorticity ε,
max dt, particle spacing (restart).

**Fields:** none at first. This is deliberate: it proves the UI handles a solver with an
empty field list. Optional later: splat density to a grid for an overlay.

PBF has no scatter at all. Its parallel code is entirely `par_iter` gathers plus one
counting sort, a useful contrast to FLIP.

### 4.3 Ballistic (Friday)

Gravity, brushes, SDF bounce with restitution, nothing else. Forty lines. It exercises
the trait, the stepper, the registry, the renderer, and the invariant tests before any
fluid math is involved.

---

## 5. App architecture (`app` crate)

```rust
// app/src/main.rs (winit 0.30 ApplicationHandler)
struct App { state: Option<State> }       // None until the window exists

struct State {
    window: Arc<Window>,                   // shared with the surface
    gpu: Gpu,                              // surface: wgpu::Surface<'static>
    renderer: Renderer,
    egui_ctx: egui::Context,
    egui_winit: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
    world: sim::World,
    ui: UiState,                           // paused, speed, overlay choice, tool
    input: InputState,
    last_frame: Instant,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.state.is_some() { return; }
        let attrs = Window::default_attributes().with_title("fluid toy");
        let window = Arc::new(el.create_window(attrs).expect("window"));
        self.state = Some(pollster::block_on(State::new(window)).expect("gpu init"));
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(s) = self.state.as_mut() else { return };
        let egui_took_it = s.egui_winit.on_window_event(&s.window, &event).consumed;
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(size) => s.gpu.resize(size),   // ignore 0×0 (minimized)
            WindowEvent::RedrawRequested => s.frame(),
            other if !egui_took_it => s.input.handle(&other),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _el: &ActiveEventLoop) {
        if let Some(s) = &self.state { s.window.request_redraw(); }  // continuous animation
    }
}
```

`State::frame()`:

1. `dt = last_frame.elapsed()` → build `StepInput` from `input` (brush from mouse
   drag velocity in world units, solid painting applied directly to
   `world.scene.solid`, gravity tilt) → `world.advance(...)` unless paused.
2. Run egui: `egui_ctx.run(raw_input, |ctx| ui::panel(ctx, &mut world, &mut ui))`. The
   panel calls `world.solver.visit_params(&mut EguiParams { .. })`, then handles
   `restart_requested`.
3. Upload: `particles()` → `queue.write_buffer(pos_buf, 0, bytemuck::cast_slice(view.pos))`
   (grow the buffer by doubling when the count exceeds capacity), and the same for `vel`.
   Upload the solid mask texture if `generation` changed. Upload the overlay field if one
   is selected.
4. Render pass: clear → field overlay quad (if any) → particles (instanced) → solid
   mask quad → egui (same pass, or a second pass with `LoadOp::Load`).
5. Present. Handle surface `Lost`/`Outdated` by reconfiguring and skipping the frame.

**Rendering details:**

- **Particles:** two per-instance vertex buffers (`pos: Vec2`, `vel: Vec2`) and a
  4-vertex triangle strip expanded in the vertex shader. The fragment shader
  anti-aliases the disc edge with `fwidth`. Color comes from speed via a uniform
  `max_speed`. Alpha blending is on.

  ```wgsl
  struct Globals { scale: vec2f, offset: vec2f, radius: f32, max_speed: f32, _pad: vec2f };
  @group(0) @binding(0) var<uniform> g: Globals;

  struct VsOut { @builtin(position) clip: vec4f, @location(0) local: vec2f, @location(1) speed: f32 };

  @vertex
  fn vs(@builtin(vertex_index) vi: u32, @location(0) pos: vec2f, @location(1) vel: vec2f) -> VsOut {
      let corner = vec2f(f32(vi & 1u), f32(vi >> 1u)) * 2.0 - 1.0;   // strip: (-1,-1) (1,-1) (-1,1) (1,1)
      var o: VsOut;
      o.clip = vec4f((pos + corner * g.radius) * g.scale + g.offset, 0.0, 1.0);
      o.local = corner;
      o.speed = length(vel);
      return o;
  }

  @fragment
  fn fs(i: VsOut) -> @location(0) vec4f {
      let d = length(i.local);
      let alpha = 1.0 - smoothstep(1.0 - fwidth(d), 1.0, d);
      if alpha <= 0.0 { discard; }
      let t = clamp(i.speed / g.max_speed, 0.0, 1.0);
      return vec4f(mix(vec3f(0.10, 0.35, 0.90), vec3f(0.85, 0.95, 1.0), t), alpha);
  }
  ```

  The Rust-side `Globals` is `#[repr(C)] #[derive(Pod, Zeroable, Clone, Copy)]` with
  the explicit `_pad`. Uniform structs must be a multiple of 16 bytes.
- **Textured quad pipeline** (one pipeline, two uses): solid mask (`R8Unorm`, filterable)
  and field overlays. Colormap overlays **on the CPU into `Rgba8Unorm`** for the weekend.
  `R32Float` isn't filterable without an optional feature, and CPU colormapping of a
  128×64 field is free. Signed fields use a diverging map with symmetric `max|x|`
  normalization. Non-negative fields use a sequential map.
- **View transform** (`view.rs`): fit the domain into the window, preserving aspect, and
  convert mouse **physical** pixels to world coordinates. winit's `CursorMoved` reports
  physical pixels, so take care with DPI scaling.

**UI panel:** solver combo box (from `sim::SOLVERS`) with a "keep particles" checkbox ·
scene preset combo · Reset · Pause / Step · speed slider · tool radio (Push / Paint
wall / Erase wall / Emitter) and brush radius · gravity angle slider (plus arrow keys) ·
overlay combo (from `solver.fields()`, plus "None") and opacity · stats (FPS, sim ms
with per-phase breakdown from `Stats.phases`, substeps, time dropped, particle count,
metrics) · solver params via the visitor.

**Input routing:** if egui reports `consumed`, or `egui_ctx.wants_pointer_input()` /
`is_pointer_over_area()` is true, the mouse doesn't touch the fluid. Otherwise left-drag
applies the current tool. Push makes a `ForceBrush` with velocity from cursor motion
divided by frame dt, capped.

---

## 6. Phased schedule

Times are honest estimates for someone fluent in systems programming but new to wgpu
and still building Rust fluency. Each phase ends with something to play with. **Cut
lines** are listed for when a phase overruns.

### Phase 0: Friday evening (~3–4 h): "balls in a box"

**Do:**
- Toolchain update, workspace skeleton, pinned deps, profiles, `env_logger`.
- `app`: winit `ApplicationHandler`, wgpu surface and device, clear to a color, resize
  and minimize handling.
- `sim`: `Grid2`, `Scene` with a hard-coded box `SolidMask` (border only, SDF can be
  analytic for now), `Solver` trait (start with just `reset`, `step`, `max_dt`,
  `particles`, `stats`), `Ballistic`, `Stepper`, `World`.
- Instanced-circle particle renderer, view transform.

**Milestone:** 20–40k colored discs rain into a box and bounce, at 60 fps, and
resizing and minimizing don't crash.

**Rust concepts:** cargo workspaces and `workspace.dependencies`. `Arc<Window>` and why
`Surface<'static>` needs it (ownership vs lifetimes). `Option<State>` for late init.
Trait objects (`Box<dyn Solver>`). `#[repr(C)]` and bytemuck `Pod` (what makes a type
safe to view as bytes). `anyhow::Result` and `?` in app code.

**Verify:** `cargo test -p sim`: `Ballistic` keeps all particles inside the box for 1000
steps and no NaNs. Manual: resize and minimize, check the log for wgpu validation
errors (there should be none).

**Cut line:** if the wgpu setup eats the evening, render with a single `PointList`
topology first (1-pixel points), then do instanced discs Saturday.

### Phase 1: Saturday morning (~4 h): "it's water"

**Do:**
- `MacGrid`, interpolation, cell marking.
- FLIP steps 1, 3, 5–10 serially (skip separation and emitters). The TMP source is the
  map: port its structure, not its JS idioms.
- Dam-break preset.

**Milestone:** dam break collapses, splashes off the far wall, and settles to a
calm-ish surface.

**Rust concepts:** struct-of-arrays layout, `Index`/`IndexMut` with tuple indices,
`#[inline]` and `debug_assert!` (bounds checks in debug only), iterator adapters
(`zip`, `enumerate`) vs index loops (benchmark both later), `f32` method idioms
(`clamp`, `mul_add`), keeping functions free (`fn p2g(grid: &mut MacGrid, pos: &[Vec2], vel: &[Vec2])`)
so borrows of different `self` fields don't collide. That last one is the most common
Rust-from-C++ friction point in simulation code.

**Verify (unit tests, `sim/tests/flip_unit.rs`):**
- Bilinear weights sum to 1 everywhere, including at boundaries.
- **Constant and linear velocity fields survive P2G→G2P exactly** (PIC reproduces affine
  fields when the grid is fully covered). This test catches the staggering off-by-half.
- Projection of a random divergent field in a closed box reduces max |div| by orders of
  magnitude with enough iterations.
- **Hydrostatic tank:** fluid at rest stays at rest. Max particle speed after 2 s stays
  small, which exposes boundary and pressure sign errors.

**Cut line:** skip drift compensation. The fluid will slowly lose volume, which is fine
for now.

### Phase 2: Saturday afternoon (~4 h): "it's a toy"

**Do:**
- egui integration (egui-winit, egui-wgpu) and panel skeleton.
- `ParamVisitor` and `EguiParams`. Solver dropdown (Ballistic ↔ FLIP already proves
  runtime switching). Pause / Step / Reset. FPS and step time.
- Mouse tools: push, paint and erase walls (`SolidMask::paint_disc`, real SDF via a
  distance transform), gravity tilt.
- Solid-mask texture quad.
- Presets: dam break, double dam break, tank with obstacle.
- (Done in Phase 1: FLIP particle separation, drift compensation, `neighbors.rs`.)

**Milestone:** stir it, slosh it by tilting gravity, draw a wall through a moving
splash, switch to Ballistic and back.

**Rust concepts:** visitor pattern with `&mut dyn Trait`, lifetimes on a struct holding
`&'u mut egui::Ui`, enums with data for tools (`enum Tool { Push, Paint { solid: bool }, ... }`)
and exhaustive `match`, closures passed to egui and the borrow-splitting needed when
one closure touches several fields of `State`, `const` items in a trait-object world.

**Verify:** manual play-testing script (each tool, each preset, switch solvers while
paused and while running). A unit test that a painted disc changes `generation` and
that `distance()` is negative inside it. A test that FLIP with a wall painted
through the fluid ejects all particles from solid within a few steps.

**Cut line:** the gravity-tilt UI can be arrow keys only. Leave the double dam break
for later.

### Phase 3: Saturday evening (~2–3 h): "fast, and checked"

**Do:**
- `bench` binary: `bench --solver flip --preset dam_break --frames 600 --threads 8 --check`
  prints mean, p50, p95, and max step time with a per-phase breakdown, and exits
  non-zero on invariant failures. Add `--sweep-threads` for a scaling table.
- `invariants.rs`: no non-finite positions or velocities, all particles inside the domain
  and not deeper than `r` inside solids, `count == initial + emitted − drained`, and
  every `Metric` with a tolerance within it.
- Phase timers in `Stats.phases`, shown in the UI.
- rayon: particle loops, then P2G fold/reduce, then Jacobi or red–black pressure.
  Measure after each change.

**Milestone:** 40k+ particles at 60 fps in the dam break, and a bench table showing
serial vs parallel per-phase timings.

**Rust concepts:** `Send`/`Sync` and why the compiler rejects a racy closure (try
writing the naive parallel P2G and read the error, which is the lesson), `par_iter_mut`,
`fold`/`reduce`, `par_chunks_exact_mut`, `split_at_mut`, `std::mem::swap`, clap derive,
`std::process::ExitCode`, `ThreadPoolBuilder::build_global` (only once per process).

**Verify:**
- `sim/tests/all_solvers.rs`: **loop over `SOLVERS` × every preset**, run 120 frames
  at reduced particle count, and assert invariants. New solvers are covered
  automatically, which is the architecture's own test.
- Serial vs parallel P2G produce identical grids within `1e-5` relative.
- Bench: FLIP post-projection divergence metric below tolerance at the default
  iteration count.

**Cut line:** stop at fold/reduce and Jacobi. The sorted gather and red–black can
wait.

### Phase 4: Sunday morning (~4 h): "swap the engine"

**Do:**
- `neighbors.rs` (counting sort binning) with tests against brute force.
- PBF solver, with ρ₀ computed numerically, s_corr, XSPH, and the shared brush and SDF
  collision helpers.
- One line in `registry.rs`.
- "Keep particles" hand-off between FLIP and PBF.

**Milestone:** pick PBF from the dropdown mid-splash and keep playing. The same walls,
gravity, brushes, and presets work, and **`git diff --stat crates/app` is empty**.

**Rust concepts:** module organization and visibility (`pub(crate)`), sharing helpers
between solvers without inheritance (free functions and small traits, e.g.
`trait Kernel { fn w(&self, r2: f32) -> f32; fn grad(&self, d: Vec2) -> Vec2; }` with
generic `fn density<K: Kernel>`, and monomorphization vs `dyn`), `chunk_by`/sorting,
borrowing two solvers at once during hand-off.

**Verify:** neighbor search equals the brute-force set on random inputs (a good first
`proptest` if you want it). PBF hydrostatic tank: mean density error under a few
percent. `all_solvers.rs` passes with no edits.

**Cut line:** skip vorticity confinement and the hand-off (switch resets from the scene).

### Phase 5: Sunday afternoon (~3 h): "polish and peek inside"

**Do:**
- Field overlays (pressure, divergence, speed, cell type) via `fields()`/`fill_field`,
  CPU colormaps, opacity slider, and a legend with min/max.
- Emitter and drain: fountain preset, emitter tool, max-particle cap.
- Double dam break. Speed-colored particles if not done.
- Optional: run the sim on its own thread. `World` is `Send`, so this is a real option:
  the render thread gets a snapshot via a channel or a triple-buffer, and input goes the
  other way. It teaches `std::thread::scope`/`mpsc` or `crossbeam-channel`. Only do it
  if UI responsiveness at high particle counts bothers you.

**Milestone:** all must-haves and should-haves done. You can watch divergence light up
where you stir.

**Rust concepts:** caller-owned buffers (`&mut Vec<f32>` reuse, `clear()` vs reallocate),
`Option` combinators in UI code, `swap_remove` for O(1) particle deletion, and, if
threading, ownership transfer across threads.

**Verify:** the overlay of a known field (a test solver or a unit test of the colormap
mapping) lands in the right place, with no half-cell shift or y-flip. Check that the
pressure overlay is high at the bottom of a resting tank. The count invariant holds with
emitters and drains active.

### Phase 6: Sunday evening (open): one stretch goal

Pick by appetite:

1. **Screen-space liquid look** (~2 h, biggest visual payoff, recommended). Render
   particles as soft Gaussian splats with additive blending into an offscreen `R16Float`
   target. Optionally blur. Then a full-screen pass thresholds it into a smooth surface
   with a fake normal from the gradient for highlights. Teaches render-to-texture and
   multi-pass wgpu.
2. **PCG pressure solver** (~2 h). Bridson ch. 5, MIC(0) optional. Watch divergence drop
   and FLIP get visibly less compressible. Fits the applied-math interest.
3. **Validation against data:** dump the dam-break surge-front position over time from
   the bench to CSV and plot it against Martin & Moyce (1952) in Python.
4. **Stable fluids** as a third solver. It reuses `MacGrid` and the pressure solver, has
   no particles, and shows a dye field. Proves the renderer's "no particles, show a field"
   path.
5. **Compute shaders.** Start with G2P or PBF (pure gathers). P2G on the GPU needs
   atomics, and **WGSL has no float atomics**, so you need fixed-point `atomic<i32>`
   accumulation.
6. **WASM.** wgpu supports WebGPU/WebGL2, but **rayon doesn't work on wasm** without
   `wasm-bindgen-rayon` plus cross-origin isolation. Gate rayon behind a default-on
   `parallel` feature in `sim` first.
7. **Audio-reactive.** `rodio` for playback, `cpal` for capture. Kick detection via a
   low-band energy envelope and an adaptive threshold (or spectral flux with `rustfft`),
   setting `StepInput.pulse`.
8. **Lattice Boltzmann** (D2Q9). It needs its own grid of populations, and obstacles
   come straight from `SolidMask`.

---

## 7. Testing strategy (summary)

| Level | What | Where |
|---|---|---|
| Unit | Grid indexing, interpolation weights, affine P2G/G2P round trip, projection reduces divergence, neighbor search equals brute force, SDF sign, kernel normalization (∫W ≈ 1 numerically in 2D) | `sim` `#[cfg(test)]` and `sim/tests/` |
| Physical sanity | Hydrostatic rest (FLIP and PBF), closed-box kinetic energy decays with no forcing, dam-break front advances monotonically | `sim/tests/` |
| Architecture | Every registered solver × every preset × 120 frames passes invariants. Switching solvers in both directions with and without hand-off doesn't panic. | `sim/tests/all_solvers.rs` |
| Parallel correctness | Serial vs parallel within tolerance | `sim/tests/` |
| Performance | Per-phase timings, thread scaling | `bench --release` |
| Visual | Play-testing checklist per phase | manual |

Run sim tests with `cargo test -p sim --release` once they get slow. The headless
crate is why they can run at all.

---

## 8. Risks and gotchas

**Toolchain and versions**
- **rustc 1.83 is installed, and egui 0.36 needs 1.95.** Update first, and pin with
  `rust-toolchain.toml`.
- wgpu ships a breaking major roughly every quarter. **Pin with `=`**, keep egui,
  egui-wgpu, and egui-winit on the same version, and check `cargo tree -d` for duplicate
  wgpu. When upgrading later, move egui and wgpu together, guided by egui-wgpu's `wgpu`
  requirement.
- Tutorials lag. Learn wgpu and most blog posts target older wgpu and winit (pre-0.30
  `EventLoop::run` closures, `Surface<'a>` workarounds). Use them for concepts, and use
  **wgpu's own examples at the `v30.0.1` tag** and egui-wgpu's docs for exact API.
  Expect small signature differences: `request_adapter`/`request_device` arguments and
  return types, the surface texture acquire result type, and `RenderPassDescriptor`
  fields have all shifted across recent majors.
- winit 0.31 betas are on crates.io. Stay on 0.30.13 (egui-winit requires it).
- **wgpu 30 API differences found while building Phase 0** (tutorials won't show these):
  `Instance::new` takes `InstanceDescriptor` by value (built with
  `new_with_display_handle_from_env(Box::new(event_loop.owned_display_handle()))`).
  `get_current_texture()` returns a `CurrentSurfaceTexture` enum (`Success`,
  `Suboptimal`, `Timeout`, `Occluded`, `Outdated`, `Lost`, `Validation`).
  Presentation is `queue.present(frame)`. `VertexState::buffers` and
  `PipelineLayoutDescriptor::bind_group_layouts` take `Option`s. Push constants are now
  `immediate_size`. Pipelines and passes have `multiview_mask`, and
  `RequestAdapterOptions` gained fields (use `..Default::default()`).
- **egui 0.36 differences found in Phase 2.** `Context::run_ui(input, |ui| …)` replaces
  `run(input, |ctx| …)`. Side panels are `egui::Panel::right(id).exact_size(w).show(ui, …)`.
  The capture checks are `egui_wants_pointer_input()` / `egui_wants_keyboard_input()`.
  `egui_wgpu::Renderer::new(device, format, RendererOptions)`. `TexturesDelta::set` holds
  several deltas per texture, and **`TexturesDelta` panics if dropped undrained**, so
  upload it every frame, even frames you skip drawing. egui also wants a non-sRGB
  target: keep the sRGB surface for the scene, add `format.remove_srgb_suffix()` to
  `SurfaceConfiguration::view_formats`, and give the egui pass a view in that format.

**Window and surface plumbing**
- Create the window inside `resumed`, not before `run_app`. Wrap it in `Arc<Window>` and
  pass a clone to `instance.create_surface(...)` to get a `Surface<'static>`.
- Minimizing gives a 0×0 size. Skip `surface.configure` and skip rendering until it is
  non-zero.
- `Lost`/`Outdated` on acquire → reconfigure and skip the frame. Don't `unwrap()`.
- egui-wgpu's `Renderer::render` wants a `RenderPass<'static>`. Call `.forget_lifetime()`
  on the pass you hand it (check the 0.36 signature).
- Present mode `Fifo` (vsync) is the safe default. `AutoNoVsync` is useful for seeing
  true frame cost.
- DPI: winit mouse positions are physical pixels, and egui uses points
  (`pixels_per_point`). Convert once, in `view.rs`.
- No logger, no errors: wgpu validation failures go through `log`. Without
  `env_logger::init()` you get a silent black window.

**GPU data**
- WGSL uniform layout: 16-byte struct alignment and size rules. Pad explicitly, or use
  the `encase` crate.
- `R32Float` textures aren't filterable by default. Use `textureLoad` or a non-filtering
  sampler, or (recommended) colormap on the CPU to `Rgba8Unorm`.
- Instance buffers must grow when the particle count grows (emitters). Reallocate at 2×
  capacity and rebuild nothing else.
- Y-axis: sim y-up vs texture v-down. Pick one convention in `view.rs` and test overlays
  against particles.

**Simulation**
- MAC staggering off-by-half: write the affine round-trip test before debugging visually.
- Boundary faces must be zeroed before projection *and* respected inside it (solid
  neighbors excluded from the Laplacian count).
- Pressure sign errors show up as fluid accelerating *into* walls. The hydrostatic test
  catches them.
- FLIP noise at ratio 1.0, viscous mush at 0.0. Use 0.9–0.95.
- Volume loss in FLIP → drift compensation. Clumping → particle separation.
- Particles inside freshly painted walls: push out along the SDF gradient. Erasing walls
  needs no special case.
- **Phase 1 FLIP findings.** Each of these was found by a failing test or a trace, not by eye:
  1. *Moving particles before projecting sinks a resting fluid.* `v += g·dt; x += v·dt`
     before the projection moves every particle down by g·dt² per substep, even in a
     completely full closed box. Put gravity on the grid and advect after G2P. TMP's
     particle separation and drift compensation mask this.
  2. *Particles and pressure must agree on where walls are.* Kept out of open cells by
     the smooth SDF, wall-adjacent cells sat empty, became air (p = 0), and the fluid
     drained sideways into them. With the opposite rule, particles sat inside solid cells
     and compacted there. Fix: FLIP collides with the faces of its own solid cells
     (`flip/collide.rs`) and seeds every open cell. The scene SDF only rescues particles
     deep inside a freshly painted obstacle.
  3. *Unconverged pressure looks like slow compaction.* 50 cold-started Gauss–Seidel
     iterations can't rebuild hydrostatic pressure 35 cells deep every substep.
     Warm-starting from the previous substep's pressure fixes it at no cost.
  4. *Walls must not pull.* With signed pressure, a splash that reaches the ceiling hangs
     there by suction. Clamping accumulated pressure at ≥ 0 (projected Gauss–Seidel)
     lets fluid separate.
  5. *Drift compensation needs separation, and a seed lattice that doesn't alias.* A
     0.6 h seed spacing beats against the grid (density 2.2–3.2 against a rest value of
     2.8), which the compensation reads as compression. Without particle separation,
     FLIP loses 60–70% of its volume over 10 s. With it, volume holds within a few
     percent and drift compensation becomes unnecessary. So separation and
     `neighbors.rs` moved from Phases 2 and 4 into Phase 1.
- **Phase 3 parallelism findings** (16-core / 32-thread machine, 20k-particle dam break):
  - **Serial → parallel: 16.2 → 5.7 ms per frame (2.8×)**, measured with
    `bench --sweep-threads`. Per-particle gathers (G2P, advect, collide, brushes) are
    plain `par_iter`. P2G uses a fixed number of chunks with persistent partial grids
    and a row-parallel sum. A first try with `fold`/`reduce` allocated fresh grids every
    substep and was slower than serial. Pressure is red–black Gauss–Seidel via
    ping-pong buffers, and separation is Jacobi (gather from a snapshot, then apply).
    None of it needs `unsafe`.
  - **Scaling stops around 8 threads at this size.** The pressure solve synchronises the
    pool about 600 times per frame on a 20k-cell grid; at 16–32 threads that overhead
    exceeds the gain, and coarser tasks made it worse, not better. The app and bench
    default to `min(cores, 8)` threads (`sim::default_threads`, overridable with
    `RAYON_NUM_THREADS`). Particle loops also need `with_min_len`, otherwise rayon
    splits 20k particles 32+ ways. Scaling further would need fewer synchronisation
    points: persistent per-thread row bands with barriers, or PCG.
  - **Measure residuals in m/s, not 1/s.** Divergence per second scales with 1/h, so a
    fixed tolerance broke at high resolution. As an unbalanced flux, the dam break peaks
    at 0.04–0.12 m/s from 5k to 82k particles (tolerance 0.3). A test proves the check
    fires when the projection is switched off.
  - **Benchmark on a quiet machine.** A running copy of the app (on 8 threads at
    240 fps) roughly quadrupled one P2G timing.
- **Smoke and fire** (`solvers/fire.rs`, a mode of stable fluids, registered as "Smoke &
  fire"):
  - **Model:** fuel and temperature move with the flow alongside the dye, which is the
    smoke.
    - Burning releases heat and soot.
    - Hot gas rises against gravity through Boussinesq lift. Tilting makes flames lean,
      and with zero gravity they don't rise (tested).
    - A little heat diffusion lets fire creep.
    - It reuses the grid core: MGPCG, open boundaries, MacCormack advection.
  - **Sources:**
    - Emitters are lit burners (Fountain is a flamethrower; the faucet is a torch).
    - Fluid blocks are solid fuel, lit along the bottom.
    - A new Campfire preset: a slow, wide burner with a vent across the top.
  - **What tuning taught** (each found in the app or by a failing test):
    1. *Fuel blocks as gas didn't burn.* The block's own hot air swept it along, and
       only 21% burned in 2 s. Solid fuel stays put.
    2. *A fire needs heat release to beat its losses.* At heat 1.5 a burning pile
       settled barely above ignition and stalled. It's now 6.
    3. *Burning solid has to stay alight on its own.* The airflow carried heat away
       faster than burning cells released it, and a pile went out at 2.5 s with 60%
       unburned. Lit solid fuel now smoulders like embers until it's spent.
    4. *Exponential cooling couldn't give short flames and a spreading fire at once.*
       A rate fast enough for short flames stalled the pile, and a slow one gave flames
       reaching the ceiling. Radiative-style cooling, dT/dt = −k·T² (implicit, so
       stable at any dt), cools very hot gas fast and warm gas slowly. It solves both.
    5. *A smooth burner gives a laminar jet, not flames.* Varying the burner's fuel
       ±40% along the nozzle and over time (a deterministic hash) seeds the buoyant
       instabilities that make flames flicker.
    6. *Flame visibility starts at T = 1*, so warm but invisible gas doesn't paint
       the whole plume orange.
  - **Rendering:** a new `FieldKind::Fire` carries three values per cell: flame, smoke
    and fuel.
    - The flame uses a fixed blackbody ramp, over light-grey smoke, over dim-amber
      unburned fuel.
    - Temperature, fuel and smoke are separate overlays.
    - Fire ignores the fluid palette, since its colours are physical.
  - **Cost:** 4–6 ms per frame on most presets. The Fountain's fast jet needs about
    3 substeps, about 14–18 ms per frame.
- **Live system-audio mode (Spotify or anything else playing):**
  - **Why not Spotify directly:** it exposes neither raw audio (the streams are DRM'd)
    nor, for new apps since late 2024, its audio-analysis data. So the app captures
    the computer's output instead.
  - **Capture:** on Windows, cpal's WASAPI backend turns an *output* device opened as
    an input stream into loopback capture (`audio/live.rs`). cpal comes re-exported
    by rodio, so there's no new dependency.
    - The callback downmixes to mono into a buffer capped at 1 s; the frame loop
      drains it.
    - WASAPI delivers nothing while the system is silent, so the level meter decays
      on its own.
  - **Detection:** `kicks::LiveDetector` is the offline pipeline made causal:
    - the same low-pass filter, now one sample at a time and shared with the offline
      path;
    - the same onset function;
    - a trailing threshold window, with peaks confirmed one hop (~11 ms) late;
    - the floor and strength scale taken from the last 10 s instead of the whole
      track;
    - a −46 dBFS gate standing in for the track-wide floor during silence and intros.

    Tests check that it finds the synthetic beat's kicks (after 1 s of warm-up) with
    no false ones, that results don't depend on how the audio is chunked, and that it
    stays quiet on silence and quiet noise.
  - **Checked live:** a 120 bpm test beat played through Windows while capturing
    "Headphones" at 48 kHz made the water splash on each kick. Other audio already
    playing on the machine was picked up too: loopback hears everything.
  - **Usage:** Music tab → Source → "System audio (live)", or start with `--live`.
    Dropping a file switches back to track mode, and the app's own track pauses while
    live so the capture doesn't hear it.
  - **Later:**
    - per-app capture, so only Spotify is heard: Windows 10 2004+ process loopback;
    - macOS: ScreenCaptureKit, or BlackHole;
    - Linux: a PulseAudio/PipeWire monitor source;
    - browser: tab audio via `getDisplayMedia`.
- **Follow-ups: MGPCG, open boundaries for stable fluids, shared MacCormack dye:**
  - **Multigrid-preconditioned CG** (`solvers/multigrid.rs`, McAdams et al. 2010).
    - *Design:* one V-cycle as the CG preconditioner, with these ingredients:
      - red-black Gauss–Seidel smoothing: red then black before the coarse correction,
        black then red after;
      - bilinear prolongation, with its transpose as restriction;
      - symmetric sweeps on a ≤6-cell coarsest level;
      - coarse cells labelled Dirichlet if any child is, else active, else solid.

      That combination keeps the preconditioner symmetric positive definite, as CG
      requires.
    - *Closed domains:* in an all-fluid box (singular, no Dirichlet cell), the
      preconditioned residual has its mean removed.
    - *Cold start:* 8 iterations against Jacobi-PCG's 139 on a 96² falling tank. A
      closed all-fluid box converges in 6.
  - **Serial beat parallel for the V-cycle.** Rayon per-pass dispatch on ~20k-cell
    levels cost as much as the work saved. A 200×100 V-cycle took 180–430 µs in
    parallel (depending on the threshold and the thread count), against 180 µs serial
    with damped Jacobi. Then:
    - switching to in-place red-black Gauss–Seidel, with one pre- and one post-sweep
      instead of two damped-Jacobi sweeps each, brought it to 100 µs;
    - the multigrid setup costs 46 µs.
  - **FLIP keeps warm-started Gauss–Seidel.** On the dam break, pressure time per frame
    was:

    | Method | 100 cells high | 200 cells high |
    |---|---|---|
    | Gauss–Seidel, 50 sweeps (warm-started) | 2.9 ms | 15.7 ms |
    | MGPCG | 4.3 ms | 36 ms |
    | Jacobi-PCG | 5.1 ms | n/a |

    - The p ≥ 0 active set re-solves up to 8 times per substep. Rebuilding the
      hierarchy plus a V-cycle each round made MGPCG *slower* (7 ms), until later
      rounds switched to Jacobi: they only correct short-wavelength error around the
      cells that flipped.
    - Warm starting already removes most of the smooth error multigrid is for.
    - FLIP's CG option now defaults to the multigrid preconditioner.
  - **Stable fluids defaults to MGPCG.** With no active set and no free surface,
    multigrid is where it pays:
    - pressure 1.7 ms vs 2.7 ms for 60 Gauss–Seidel sweeps;
    - worst residual 0.005 m/s vs 0.046.
    - Gauss–Seidel can't absorb the wind tunnel's sudden full-height inflow: 0.48 m/s
      residual on frame 0. MGPCG converges.
  - **Open boundaries for stable fluids.**
    - *Outlets:* drains become air cells (p = 0) and remove dye.
    - *Inlets:* emitter nozzles become solid cells whose faces carry the jet velocity.
      The projection never changes faces touching solids, so these are
      prescribed-flux boundaries.
      - Against a wall, a nozzle is a true inflow (the wind tunnel).
      - In open fluid, it draws in as much as it pushes out (a pump), so the faucet in
        a closed box keeps the Neumann problem solvable.
      - The edge case: a faucet nozzle touching a wall in a closed box is an
        unbalanced source.
    - *Result:* stable fluids now sheds its own vortex street in the wind tunnel. The
      test checks that flow at mid-tunnel averages 1 m/s, which a closed box can only
      recirculate to zero.
  - **Shared toolkit.** `maccormack`, `nozzle_offset` and `nozzle_dye` moved to
    `solvers/mod.rs`, so LBM and stable fluids share the limited MacCormack dye
    advection and striped streaklines. Stable fluids' dye is now as sharp as LBM's.
- **Phase 6 (stretch 8) Lattice Boltzmann findings:**
  - **Model.** D2Q9 BGK with a Smagorinsky subgrid term, fused pull-stream + collide
    into a second buffer. Each row writes only its own cells, so rayon parallelises it
    with no `unsafe`. Walls use half-way bounce-back on the resampled `SolidMask`, and
    forces use the exact-difference method. There's no pressure solve: about 9 ms/frame
    at 256×128 and 43 lattice steps (~150 M cell updates/s on 8 threads).
  - **Units.** One m/s = 0.05 lattice units (Mach ≤ 0.1 for normal flow), so a frame
    sub-cycles `dt / (0.05 h)` lattice steps. The solver returns `max_dt = 1` and
    sub-cycles itself. τ = 0.52 gives ν ≈ 10⁻³ m²/s (about 1000× water). That's syrup
    in physical terms, but Re ≈ 100 for the wind tunnel's cylinder, which is what sheds.
  - **Open boundaries.** Emitters are velocity inlets and drains are ρ = 1 outlets,
    which makes a new **Wind tunnel** preset possible. The first inlet let density
    float, and the cells behind it were sucked to ρ ≈ 0.55. It now fixes density and
    velocity (full equilibrium), and the worst |ρ − 1| fell from 0.47 to 0.08.
    - Without any outlet (the faucet in a closed box), mass is renormalised to unit
      mean density every frame, a sink spread over every cell.
  - **Validation.** The wake behind the cylinder oscillates at Strouhal 0.18 at Re ≈ 86.
    Williamson's measurements give 0.16–0.17, and the test asserts 0.13–0.21. A box at
    rest stays at rest with mass conserved to 10⁻⁴.
  - **The dye needed MacCormack advection.** Plain semi-Lagrangian advection blurred the
    inlet's streaklines into uniform blue within half the tunnel. MacCormack (forward,
    back, add half the error, clamp to the source cell's min/max) keeps a sharp
    vortex street for two extra cheap passes. Stable fluids would benefit too.
  - **Presets bring their gravity.** The wind tunnel is weightless, but the app
    overwrote the scene's gravity from the UI sliders every frame. `World` now bumps a
    `scene_generation` on preset load, and the app copies that scene's gravity into
    the sliders once.
  - **Speed guard.** Velocities are clamped at 0.3 lattice units. The reported metric's
    tolerance is 0.25, and inlets and brushes drive at most 0.2. The fountain jet
    (3.2 m/s → 0.16) peaks at 0.215, the closest any preset gets.
- **Phase 6 (stretch 6) GPU compute findings:**
  - **Port PBF first, not FLIP.** Every PBF pass is a neighbour gather. Cell lists with
    `atomic<u32>` counters and fixed capacity (32 per cell) need no GPU prefix sum.
    FLIP's scatter would need fixed-point integer atomics (WGSL has no float atomics)
    and a GPU pressure solve.
  - **Architecture:** a `sim-gpu` crate (sim + wgpu) implements the same `Solver`
    trait, reusing PBF's parameters and derived constants via
    `PbfParams::{constants, visit, max_dt}` and the now-public solver toolkit
    (`Emission`, `apply_brushes`, `seed_particles`, `resample`). `sim` can't name a
    solver in a crate that depends on it, so `World::with_solvers` takes a composed
    list (`sim::SOLVERS` + `sim_gpu::SOLVERS`). Without an adapter, the entry falls back
    to CPU PBF with a warning.
  - **Each substep:** CPU-side emission, drains and brushes; upload; one compute pass
    (wgpu inserts the barriers between dispatches); copy to staging, map, read back.
    The readback keeps `particles()`, and so the renderer, bench and invariants,
    unchanged.
  - **Stay within WebGPU's default of 8 storage buffers per stage** by packing λ and
    density into one `vec2` array, so it runs on any adapter.
  - **Parity and speed:** one substep on the GPU matches the CPU within 0.1 mm, and the
    dam break's compression statistics are identical. 4× faster at the CPU defaults
    (5k particles: 9.1 → 2.25 ms per frame); 11× at FLIP's resolution (20k particles, 20
    iterations: 133 → 11.7 ms). Since the CPU solver was limited to coarse resolution,
    the GPU version defaults to 20k particles and 20 iterations, 93 fps in the app
    mid-splash. Fixed overhead is about 0.5 ms per substep (submit, map, readback); a
    zero-copy path sharing the renderer's device would remove the readback.
  - **wgpu 30:** `get_mapped_range` returns a `Result`; blocking waits are
    `device.poll(PollType::wait_indefinitely())`.
- **Phase 6 (stretch 5) stable fluids findings:**
  - **A full domain needs buoyancy, not gravity.** In a closed, all-fluid domain,
    gravity is a pure gradient that the projection removes, so the dye carries density
    instead (Boussinesq). Heavy dye turns the dam-break presets into gravity currents,
    complete with Kelvin–Helmholtz billows on the upper edge; negative buoyancy gives
    rising smoke.
  - **The p ≥ 0 constraint must be optional.** It's right for a free surface but wrong
    for a closed domain, where pressure is only defined up to a constant. The pressure
    solver moved to the shared `solvers/pressure.rs` and gained a `non_negative` flag,
    as did the residual metric. A closed domain suits Gauss–Seidel: worst residuals
    were 0.007–0.046 m/s, at 2.7–8 ms per frame.
  - **Semi-Lagrangian dye isn't conservative.** The total drifts by about 7% over a
    10 s dam break. Fine for a toy; a conservative scheme (e.g. MacCormack with limiting,
    or flux-form) would fix it.
  - **The particle-free path needed two app changes, both generic and with no
    solver-specific code.** A `FieldKind::Concentration` colour map (fades to
    transparent, sampled linearly), and a rule that a solver without particles is drawn
    through its first field. Registration was still one line. Hand-off splats incoming
    particles into dye; leaving the dye solver reseeds from the scene.
- **Phase 6 (stretch 4) validation findings** (full write-up and plot in
  `docs/validation/README.md`):
  - **Grid-converged FLIP matches Martin & Moyce's front speed within 3%** and leads in
    time by a near-constant ΔT ≈ 0.2, consistent with the experiment's gate-opening
    delay. Coarse FLIP and PBF match within 1–4%, partly because numerical dissipation
    mimics that delay.
  - **Verify experimental data rather than relying on memory.** Only three points of
    the recalled table could be corroborated (against published validations citing
    the paper's Table 2), so the regression test uses only those. Arrival times get a
    15% band (allowing the offset); front speed gets 10%.
  - **Measure the front robustly:** the 99.9th-percentile x in a thin floor band, with
    the column defined from the actual wall faces (not the domain edge, 3 cm away).
  - **Plot with Pillow, not matplotlib,** which wasn't installed (`tools/plot_dam_break.py`).
- **Phase 6 (stretch 3) audio-reactive findings:**
  - **Detect kicks offline, when the track loads.** A background thread decodes, mixes
    to mono, low-passes at 150 Hz, takes frame energy every 512 samples, and builds a
    log-energy onset function. Peaks are picked against an adaptive threshold, at least
    0.1 s apart. Playback then only compares precomputed times with
    `Player::get_pos()`, so the timing is exact, and the "selectivity" slider re-picks
    instantly.
  - **An adaptive threshold alone fires on noise in quiet stretches** (intros,
    breakdowns, the tail), because there it's only relative to noise; the synthetic-track
    test caught a false kick in the tail. Peaks must also clear 20% of the track's strong
    onsets (98th percentile of non-zero values).
  - **Kicks become fluid input:** Splash (an upward force brush, conserves water) or Jet
    (emitter pulses, adds water; pair with the fountain's drains). Lanes cycle across
    the floor. Both reuse the Phase 2 and 5 input paths unchanged.
  - **rodio 0.22 API:** `DeviceSinkBuilder::open_default_sink()`,
    `Player::connect_new(sink.mixer())`, `Decoder::try_from(File)`, f32 samples,
    `NonZero` channel count and sample rate. Pinned to 0.22.2, since 0.23.0 was under
    two weeks old. Tracks load from the command line or by dropping a file on the
    window (winit's `DroppedFile`, no dialog crate needed); `--mute` starts muted.
  - **egui's default font has no "●".** It renders as a box.
- **Phase 6 (stretch 2) conjugate-gradient pressure findings:**
  - **Built:** Jacobi-preconditioned CG, warm-started, stopping at a max-residual
    tolerance in m/s. Each iteration is three fused row-parallel map-reduce passes; dot
    products accumulate in f64 per row. It's selectable in the FLIP panel.
  - **p ≥ 0 needs a primal–dual active set, not clamping.** Clamping CG's answer
    expanded the dam break by 67%. Releasing negative cells without re-admission left
    0.33 m/s residuals in splashes, because early releases got stuck. The working loop:
    solve; release cells with p < 0; re-admit released cells with inflow above the
    tolerance; re-solve warm-started. Never finish on an unsolved change. Only release
    after a converged solve, since unconverged iterates have spurious negatives. Share
    one iteration budget across rounds.
  - **From a cold start CG wins big.** On a 48-cell-deep tank, Gauss–Seidel reaches
    0.011 m/s after 400 iterations, while CG reaches 0.005 after 100 and 2e-4 (the f32
    floor) after 200.
  - **In the simulation, warm-started Gauss–Seidel wins.** At 40k particles on a quiet
    machine, 8 threads: GS (50 iterations) spends 4.7 ms per frame on pressure, with a
    mean residual of 0.009 m/s and a worst of 0.117. CG at a 0.003 tolerance spends
    14.6 ms (3.1× slower), averaging 100 iterations (231 worst), for a mean of 0.004
    and a worst of 0.088. CG doesn't scale any better either: 3.1× against 2.9× from 1
    to 8 threads (measured with the app running, so relative only). The
    hypothesis that fewer synchronisations would scale better was wrong: warm starting
    removed Gauss–Seidel's weakness (slow smooth modes), and CG still needs O(N)
    iterations at 3 synchronisations each. The default stays Gauss–Seidel.
  - **What would actually win:** a multigrid preconditioner (MGPCG) to cut CG's
    iteration count to about 10–20, or multigrid on its own. That's a project of its own.
- **Phase 6 (stretch 1) screen-space liquid findings:**
  - **Normalise the splat field analytically.** With blob radius k × particle radius,
    and every solver spacing particles at two radii, the interior sum of the
    exp(−4(r/R)²) splats is ≈ 0.98 π k²/16, independent of resolution and zoom (checked
    numerically against a lattice sum within 2%). One "surface level" threshold
    therefore works for FLIP's 20k particles and PBF's 5k alike.
  - **Smooth before you threshold or differentiate.** The raw splat sum ripples at
    particle scale, which gave sparkling specular highlights, black specks and fizzy
    edges. A 3×3 tent filter in the composite pass, with a step of half the blob's
    on-screen radius (so it behaves the same at any zoom), fixed all three. Normals are
    flattened deep inside the fluid so highlights stay at the surface.
  - **Accumulate into `Rgba16Float`.** It's blendable, while 32-bit float needs an
    optional feature. The splat pass renders into the texture the composite reads, so
    the two passes need separate bind group layouts: a texture can't be both bound and
    a render attachment in one pass.
  - **PBF benefits most.** At about 5k particles it read as sparse dots; now it reads
    as continuous water.
- **Phase 5 findings:**
  - **Pressure in pascals is a free end-to-end test.** The stored pressure (a velocity
    change per unit time) converts as p = stored × ρ × h. In a resting tank it matches
    ρ·g·depth within 15% in every checked cell (`pressure_field_is_hydrostatic`). That
    covers the projection, its warm start, and the units in one assertion.
  - **Emitters are volume flows (width × speed), not particle rates,** so every solver
    pours the same water at its own resolution. FLIP emits 5,120 particles/s and PBF
    1,280/s from the same nozzle. The existing count invariant (initial + emitted −
    drained) checked all of it unchanged.
  - **Use overflow drains, not floor drains.** Drains at the bottom of the corners
    emptied the fountain pool within seconds. Tall slots just above the surface hold
    the level like an overflow pipe.
  - **The divergence-before-projection overlay doubles as an alignment check.** In a
    resting tank it's zero except for inflow along the floor and outflow under
    obstacles, exactly on the solid boundaries, so any half-cell shift or y-flip would
    show. Overlays sample nearest, one texel per cell.
  - **Synthetic mouse input isn't reliable for testing egui widgets** (keys are).
    Overlays got keyboard shortcuts (O cycles, H hides particles), which is better UX
    anyway.
  - **Skipped:** running the sim on its own thread. At 200+ fps it isn't needed.
- **Phase 4 PBF findings:**
  - **The paper's constants aren't scale-free.** Its k = 0.1 (s_corr) and ε depend on
    its units and spacing. Used as-is in this normalisation, s_corr was about 100× the
    constraint multipliers and the first run reached 244,000 m/s. Both are now expressed
    relative to the rest lattice's Σ|∇C|²: ε as a fraction, s_corr as an equivalent
    compression. Defaults are 0.01 and 0.01; s_corr = 0.1 destabilises.
  - **Don't shrink dt by speed.** Position corrections become velocities as Δx/dt, so
    a CFL-style limit feeds back (smaller dt → bigger velocity → smaller dt) and ran
    away to 8 substeps. Use a fixed small step (1/240 s; 1/120 jitters at 1–3 m/s) with
    only a loose travel cap.
  - **Iterations must scale with depth in particles.** The Jacobi-style solve spreads a
    correction about one kernel radius per iteration. At 0.005 m spacing the dam-break
    column is 160 particles deep, and 4–10 iterations blow up. The default is 0.01 m
    with 6 iterations: about 5k particles, 8.5–11 ms per frame on every preset, at most
    6% compression. PBF is the coarser solver here.
  - **Cache neighbour lists per substep** (flat array, 48 per particle, 10% radius
    margin). This halved the constraint cost compared with walking bins every pass.
  - **The solvers run at different resolutions, so hand-off resamples.** Finer input is
    thinned to one particle per cell (averaged); coarser input is split into clusters.
    All 9 solver pairs switch mid-run with invariants holding.
  - **The architecture check passed:** adding PBF needed one registry line and no change
    to `crates/app`. Separately, Phase 2's Tab shortcut never worked, because egui
    consumes Tab for focus navigation. Next-solver is now N.
- **Curved obstacles and grid collisions (Phase 2).** Face clamps bound penetration by
  one radius only for axis-aligned walls. Where a curved wall cuts an open cell's corner,
  particles can sit about 0.7 cells deep. FLIP's collision now also caps SDF
  penetration at one radius. That empties only cell corners, so it doesn't bring back
  the empty-cell leak. The every-solver × every-preset test caught it on the new
  obstacle preset.
- **The SDF can only measure penetration depth inside a wall up to about half the wall's
  thickness** (it is sampled at cell centres). Phase 0 hit this: with 1-cell border walls
  and 1.5 cells of travel per substep, particles that overshot were pushed out only part
  of the way, and the invariant test caught it. Fix: the border is `BORDER_CELLS = 2`
  thick, and per-substep travel is capped at one mask cell. Keep both in mind when
  choosing FLIP's CFL number and the minimum brush size for painted walls.
- PBF: 2D kernel constants, rest density mismatch, tensile instability (s_corr), and
  neighbor cell size must be ≥ kernel radius.
- Debug builds are unusably slow without the profile overrides in §2.

**Rust-specific**
- Borrowing `self.grid` mutably while iterating `self.particles`: pass fields to free
  functions, or destructure (`let Self { grid, pos, vel, .. } = self;`). This is
  idiomatic, not a workaround.
- `build_global` for rayon can only be called once per process. The bench should call
  it before anything touches rayon, and tests shouldn't call it at all.
- Float sums in `fold`/`reduce` aren't deterministic across thread counts. Compare with
  a tolerance.

**Scope**
- The biggest real risk is spending Saturday on wgpu plumbing. Mitigations: Friday's
  milestone is plumbing only, the Ballistic solver decouples rendering from fluid
  correctness, and every phase has a cut line.

---

## 9. References

**Fluid simulation**
- Robert Bridson, *Fluid Simulation for Computer Graphics*, 2nd ed. (2015). Ch. 2–5 for
  MAC grids, advection, and pressure (PCG in ch. 5). Ch. 7 for particle methods and FLIP.
- Bridson & Müller-Fischer, *Fluid Simulation* SIGGRAPH 2007 course notes (free):
  https://www.cs.ubc.ca/~rbridson/fluidsimulation/fluids_notes.pdf
- Matthias Müller, Ten Minute Physics: "Eulerian fluid simulation" (#17) and "FLIP fluid
  simulation" (#18), with source code:
  https://matthias-research.github.io/pages/tenMinutePhysics/index.html
- Zhu & Bridson, "Animating Sand as a Fluid" (SIGGRAPH 2005). Introduced FLIP to graphics.
- Macklin & Müller, "Position Based Fluids" (SIGGRAPH 2013):
  https://mmacklin.com/pbf_sig_preprint.pdf
- Müller, Charypar & Gross, "Particle-Based Fluid Simulation for Interactive
  Applications" (SCA 2003). SPH kernels.
- Jiang et al., "The Affine Particle-In-Cell Method" (SIGGRAPH 2015). APIC, the natural
  upgrade from PIC/FLIP blending.
- Jos Stam, "Stable Fluids" (SIGGRAPH 1999) and "Real-Time Fluid Dynamics for Games"
  (GDC 2003).
- van der Laan, Green & Sainz, "Screen Space Fluid Rendering with Curvature Flow"
  (I3D 2009). For the stretch renderer.
- Dan Schroeder's interactive lattice Boltzmann notes and demo:
  https://physics.weber.edu/schroeder/fluids/
- Martin & Moyce (1952), *Phil. Trans. R. Soc. A* 244. Dam-break experimental data.

**Rust and graphics**
- Learn wgpu: https://sotrh.github.io/learn-wgpu/ (concepts; check which wgpu version it targets).
- wgpu examples at the pinned tag: https://github.com/gfx-rs/wgpu/tree/v30.0.1/examples
- egui-wgpu and egui-winit API docs at 0.36.2 on docs.rs, and egui's `custom3d_wgpu` demo
  for mixing your own wgpu rendering with egui.
- winit `ApplicationHandler` docs (0.30.13) on docs.rs.
- rayon docs on docs.rs (`ParallelIterator::fold`, `ParallelSliceMut`).
- Glenn Fiedler, "Fix Your Timestep!": https://gafferongames.com/post/fix_your_timestep/
- *Rust for Rustaceans* (Jon Gjengset) for traits, object safety, and API design.
  *Rust Atomics and Locks* (Mara Bos) for why the safe patterns above beat atomics.
