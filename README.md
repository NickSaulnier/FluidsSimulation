# Fluid toy

A real-time fluid simulator in Rust, grown into a MilkDrop-style music visualizer. You can
pour, push and tilt 2D and 3D fluids by hand, or let the music drive them: the app
hears the beat, the bass and the song's sections, and looks change with them.

Built on wgpu (rendering and GPU compute), winit and egui. Windows is where it's
developed and tested; nothing in it is Windows-only except the live audio capture
(WASAPI loopback, through rodio/cpal).

## Running it

Rust 1.99.1 (pinned in `rust-toolchain.toml`; rustup fetches it).

```bash
cargo run --release -p app
```

```bash
cargo run --release -p app -- path/to/song.mp3 --visualizer
```

- A track (mp3, ogg, flac, wav) can be given on the command line or dropped on the
  window.
- `--live` listens to whatever the computer is playing (Spotify, a browser...).
- `--visualizer` starts full screen with the controls hidden, listening live unless a
  track is given.
- `--mute` analyses the track without playing it.

Use `--release`: a debug build of the solvers is far too slow (the dev profile already
optimises the dependencies and the `sim` crate).

## What's in it

### Fluids

| Solver | Kind |
|---|---|
| FLIP | Particle/grid hybrid on a staggered grid; PCG pressure solve with a multigrid preconditioner |
| PBF | Position Based Fluids, particles only |
| PBF (GPU) | The same on wgpu compute |
| Stable fluids (dye) | Grid fluid carrying dye, with buoyancy and vorticity confinement |
| Smoke & fire | Stable fluids with fuel, temperature, burning and smoke |
| Lattice Boltzmann (dye) | D2Q9 LBM with a Smagorinsky model |
| Ballistic (debug) | Gravity and bouncing, as a test fixture |
| Fire (3D GPU), Smoke (3D GPU) | 3D grid fluids on the GPU, raymarched, with an orbiting camera |

2D scenes: dam break, double dam break, tank with obstacle, fountain, Martin & Moyce
column, wind tunnel, campfire. 3D scenes: campfire, torch, twin burners, chimney.

Tools: push, paint and erase walls, a faucet; gravity can be tilted. Fields (pressure,
speed, vorticity, temperature...) can be overlaid with a legend. A governor lowers the
resolution in full screen if the simulation falls behind real time.

### Music

- Analysis: a log-spaced spectrum, five bands, MilkDrop's bass/mid/treb, EBU R128
  loudness, kick, snare and hi-hat detection (SuperFlux), tempo, beat and bar phase,
  section changes, drops, breakdowns and build-ups.
- A latency setting (with a flash on every beat to set it by ear) for wireless
  headphones and big audio buffers.
- Reactions map triggers (kick, snare, hat, beat, bands, level, brightness) to effects
  on the fluid (splash, jet, vortex, gust, slosh, breathe, flash, hue, or any solver
  parameter).

### The visualizer

- **Picture:** an HDR scene at a render scale, bloom, MilkDrop's feedback (zoom, turn,
  drift, wobble, echo, and trails that follow the fluid's own flow), waves and shapes
  drawn from the sound, and a composite (kaleidoscope and mirrors, hue, saturation,
  contrast, palette mapping, invert, chromatic aberration, vignette, grain).
- **Looks:** a look bundles a fluid (or none), colours, the picture's settings,
  reactions and equations. There are 32 built in, 12 of them without fluid. Each is a
  TOML file (format in [crates/app/looks/README.md](crates/app/looks/README.md)); your
  own go in `%APPDATA%\fluid-toy\looks` and reload when saved. "Save as" writes the
  current settings as a look.
- **Equations:** a small language run per frame on the CPU and per pixel on the GPU
  (compiled to WGSL), reading the music and setting the warp, waves, shapes, composite
  and solver parameters. The editor (E) runs edits as you type.
- **The director:** in full screen it changes looks on new songs, drops and
  breakdowns, and at least every interval. Changes blend the old picture into the new
  look (a dissolve, from the middle out, in patches, or a zoom blur) and land on the
  downbeat; drops cut on the beat. Better-rated looks come round more often, and it can
  keep to favourites or to picked tags.
- **The browser** (B): every look with a thumbnail (taken the first time a look plays),
  search, tags, stars and favourites.
- **Screen:** the monitor for full screen, the visualizer in a window kept on top, a
  frame-rate cap.

## Keys

F1 shows them all in the app. The main ones:

| Key | What |
|---|---|
| Space | pause (in full screen: next look) |
| F11 | full-screen visualizer; Esc leaves it |
| `,` `.` | previous / next look |
| H | hide particles (in full screen: next look, cut without a fade) |
| L | liquid surface (in full screen: keep this look) |
| A | change looks automatically |
| Backspace | back to the look before |
| `+` `-` | rate the look |
| B, E | the look browser, the equation editor |
| T | the song's title |
| M | play / pause the music |
| R, P, N | reset, next scene, next solver |
| 1 to 4 | push, wall, erase, faucet |
| `` ` `` | hide / show the side panel |

## The code

| Crate | What |
|---|---|
| `crates/sim` | Headless 2D solvers and scenes. Knows nothing of graphics. |
| `crates/sim-gpu` | GPU solvers implementing `sim::Solver` (PBF). |
| `crates/sim3d` | 3D GPU solvers, scenes and their world. |
| `crates/app` | The app (`fluid-toy`): rendering, interface, audio analysis, looks, equations, director. |
| `crates/bench` | Headless runs: timing per phase, thread scaling, invariant checks. |
| `crates/utils` | Small shared helpers (a deterministic RNG). |

Solvers expose their parameters through a visitor, so the interface draws a control
for every parameter without naming any solver.

## Tests and benchmarks

```bash
cargo test --release
```

- Solver invariants and validation against the dam-break experiment (Martin & Moyce;
  see [docs/validation](docs/validation/README.md)).
- Audio detection scored against labelled clips, with a baseline that fails the test
  if the scores fall (`UPDATE_ANALYZER_BASELINE=1` rewrites it).
- Rendering passes checked on the GPU (skipped without one), looks, equations, the
  director, the interface's settings and shortcuts.

```bash
cargo run --release -p bench -- --solver flip --preset dam --frames 600
```

A 30-minute soak test of the visualizer runs with
`cargo test --release -p app thirty_minute_soak -- --ignored --nocapture`.

## Plans

The design notes and progress, phase by phase, are in `docs/`:
[PLAN.md](docs/PLAN.md) (the fluid toy), [3D_PLAN.md](docs/3D_PLAN.md),
[UI_REDESIGN.md](docs/UI_REDESIGN.md), [VISUALIZER_PLAN.md](docs/VISUALIZER_PLAN.md),
[ANALYZER_PLAN.md](docs/ANALYZER_PLAN.md) and [MILKDROP_PLAN.md](docs/MILKDROP_PLAN.md).
Still to come there: song metadata and album art, and classic `.milk` preset import.
