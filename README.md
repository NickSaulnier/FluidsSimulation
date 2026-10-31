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
- `--import <file or folder>` converts classic MilkDrop presets (`.milk`) into looks and
  exits; dropping them on the window does the same.
- `--audit` plays every look in turn against the same music (a generated song, or a
  track given), scores its frames (dark, blown out, flat, still, following the music,
  frame rate) and writes a contact sheet, worst first, to `Pictures\fluid-toy\audit`;
  `--audit-looks=TEXT` keeps to looks with that in their names, `--audit-random=N`
  audits random looks instead.

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
| Stable fluids (GPU) | The same on wgpu compute, at three times the resolution, for crisp ink at full screen |
| Smoke & fire | Stable fluids with fuel, temperature, burning and smoke |
| Lattice Boltzmann (dye) | D2Q9 LBM with a Smagorinsky model |
| Shallow water | A water surface seen from above (height field), lit: waves, wakes and ripples; the music's drops ring out on it |
| MPM jelly, MPM snow, MPM sand | MLS-MPM particles carrying a material: jelly wobbles and bounces, snow packs and breaks into clumps, sand flows and piles |
| Ballistic (debug) | Gravity and bouncing, as a test fixture |
| Fire (3D GPU), Smoke (3D GPU) | 3D grid fluids on the GPU, raymarched, with an orbiting camera |

2D scenes: dam break, double dam break, tank with obstacle, fountain, Martin & Moyce
column, wind tunnel, campfire, pond, breakwater, falling blocks; and four made for the
music, open at the edges so nothing settles: five bands (a source per band round the
window, each pouring with its own band), tempo vortex (two sources going round the
middle once a bar), two jets (bass from the left, treble from the right, curling where
they meet) and rain (open water for the drops). 3D scenes, in the open air (smoke rises out of the picture): campfire, torch, twin
burners, chimney.

The fluid fills the window edge to edge, the scene taking the window's shape, and
bounces off its invisible edges. Gravity points any way, at any strength (in 3D, with a
heading too).

Tools: push, paint and erase walls (shaded, in the palette's colours, with a preview
under the brush and Ctrl+Z to undo), a faucet. Fields (pressure,
speed, vorticity, temperature...) can be overlaid with a legend. A governor lowers the
resolution in full screen if the simulation falls behind real time.

### Music

- Analysis: a log-spaced spectrum, five bands, MilkDrop's bass/mid/treb, EBU R128
  loudness, kick, snare and hi-hat detection (SuperFlux), tempo, beat and bar phase,
  section changes, drops, breakdowns and build-ups, the pitch classes (chroma).
- A latency setting (with a flash on every beat to set it by ear) for wireless
  headphones and big audio buffers.
- Reactions map triggers (kick, snare, hat, beat, bands, level, brightness) to effects
  on the fluid (splash, jet, vortex, gust, slosh, breathe, flash, hue, zoom, or any
  solver parameter). Two triggers run on the beat's own clock instead of the hits heard,
  so their motion lands on the beat: pulse (rising into each beat, peaking on it) and
  bar (a step each downbeat); with no steady beat they rest, and the hits carry on.
- The song's shape: an energy, from how loud the song is now against its own quiet and
  loud parts, lifted through a build-up and full after a drop. Every look follows it
  (as much as its `dynamics` says): gentler reactions, slower motion and dimmer colour
  through a breakdown, a zoom creeping in through a build-up, a flash and a burst at
  the drop. Equations read it as `energy`.
- The harmony: the twelve pitch classes sounding (the chroma) and the strongest, and a
  hue from the chords, round the circle of fifths so related chords are close colours.
  A look with `harmony` turns its colours with the chord changes (from its own, as it
  starts) and grows richer and brighter with a bright sound; off by default, on in
  Aurora, Ink nebula and Mandala and in about a third of random looks. Equations read
  `chroma_0` to `chroma_11` and `key`.

### The visualizer

- **Picture:** an HDR scene at a render scale, bloom, MilkDrop's feedback (zoom, turn,
  drift, wobble, echo, and trails that follow the fluid's own flow), waves and shapes
  drawn from the sound, and a composite (kaleidoscope and mirrors, hue, saturation,
  contrast, palette mapping, invert, chromatic aberration, vignette, grain).
- **Fractals:** Mandelbrot, Julia, Burning Ship, Tricorn and Newton, and in 3D the
  Mandelbulb and the Menger sponge (raymarched, behind the gas in 3D looks), in the
  look's colours, over the fluid or alone. They move with the music: a Julia set's shape
  wanders (a Mandelbulb's lobes), the zoom pulses with the bass, a tour moves on at each
  new section, and a dive travels endlessly into the fractal (or out), looping where the
  picture repeats itself. The fluid's flow can bend them.
- **Looks:** a look bundles a fluid (or none), colours, the picture's settings,
  reactions and equations. There are 48 built in, 16 of them without fluid; they keep to
  the fluids that look good in full screen (no MPM, no test rigs). Each is a
  TOML file (format in [crates/app/looks/README.md](crates/app/looks/README.md)); your
  own go in `%APPDATA%\fluid-toy\looks` and reload when saved. "Save as" writes the
  current settings as a look.
- **Equations:** a small language run per frame on the CPU and per pixel on the GPU
  (compiled to WGSL), reading the music and setting the warp, waves, shapes, composite
  and solver parameters. The editor (E) runs edits as you type.
- **The director:** in full screen it changes looks on new songs, drops and
  breakdowns, and at least every interval. A look can carry variants (other
  colours, symmetry, feedback, waves or fractal over the same fluid): other changes
  of section move it on to its next variant, blended over a bar, instead of changing
  the look. Mandala, Ink nebula and Julia tunnel have two, and so does every random
  look; the equation editor lists them. Changes blend the old picture into the new
  look (a dissolve, from the middle out, in patches, or a zoom blur) and land on the
  downbeat; drops cut on the beat. Better-rated looks come round more often, and it can
  keep to favourites or to picked tags.
- **Songs:** listening live, the app reads what the player reports (Windows' media
  session: Spotify, browsers, media players). Each song gets a title card with its
  cover, a new look from the director at once, and the looks' colours from its cover;
  looks can draw the cover itself into the picture, for the feedback and the fluid to
  carry.
- **The browser** (B): every look with a thumbnail (taken the first time a look plays),
  search, tags, stars and favourites.
- **MilkDrop presets:** classic `.milk` presets convert into looks: their warp, decay,
  waveform, shapes and equations (run in MilkDrop's units). MilkDrop 2 shaders, custom
  waves and borders are left out, and listed at the top of each file.
- **Screenshots and recordings:** F12 saves the picture (without the interface) as a
  PNG in `Pictures\fluid-toy`. F10 records a video into `Videos\fluid-toy` (1080p,
  30 fps, with the track's sound when one is playing); it needs
  [ffmpeg](https://ffmpeg.org) on the PATH.
- **Random looks:** D (or the dice) makes a new look: any fluid or none, its physics,
  colours, feedback, waves, symmetry, reactions and sometimes equations, all within
  ranges that stay watchable. Shift+D varies the look on screen. Save as keeps one;
  the director can mix them in now and then (a setting, off by default). A random look
  that comes out dark, empty or murky in its first seconds is replaced by another.
- **Screen:** the display to show on (the app moves there at once; a map of your
  displays to pick from), the visualizer in a window kept on top, a frame-rate cap.
- **Interface:** a dark, high-contrast theme in Inter and Orbitron, three sizes, and
  motion throughout (sliding panels, switches, popping and filling icons), with a
  "Reduce motion" setting.

## Keys

F1 shows them all in the app. The main ones:

| Key | What |
|---|---|
| Space | pause (in full screen: next look) |
| F11 | full-screen visualizer; Esc leaves it |
| Shift+F11 | move to the next display |
| `,` `.` | previous / next look |
| H | hide particles (in full screen: next look, cut without a fade) |
| L | liquid surface (in full screen: keep this look) |
| A | change looks automatically |
| Backspace | back to the look before |
| D, Shift+D | a new random look; a variation on this one |
| `+` `-` | rate the look |
| B, E | the look browser, the equation editor |
| T | the song's title card |
| F12, F10 | a screenshot; start / stop recording |
| M | play / pause the music |
| R, P, N | reset, next scene, next solver |
| 1 to 4 | push, wall, erase, faucet |
| Ctrl+Z | undo the last wall stroke |
| ← → | turn gravity |
| ↑ ↓ | stronger / weaker gravity |
| `` ` `` | hide / show the side panel |

## The code

| Crate | What |
|---|---|
| `crates/sim` | Headless 2D solvers and scenes. Knows nothing of graphics. |
| `crates/sim-gpu` | GPU solvers implementing `sim::Solver` (PBF, stable fluids). |
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
[ANALYZER_PLAN.md](docs/ANALYZER_PLAN.md), [MILKDROP_PLAN.md](docs/MILKDROP_PLAN.md),
[IMPROVEMENTS_PLAN.md](docs/IMPROVEMENTS_PLAN.md), [FRACTALS_PLAN.md](docs/FRACTALS_PLAN.md),
[VISUALS_PLAN.md](docs/VISUALS_PLAN.md) (pictures that follow the music better) and
[FIDELITY_PLAN.md](docs/FIDELITY_PLAN.md) (a sharper, cleaner picture; planned).
Not done (the optional extras left for later): Spout, MIDI and OSC.
