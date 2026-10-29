# Fractals Plan

Status: **phases A and B done** (2026-10-29); decisions open (§4). Fractal visualizations as an option
in looks, grouped into phases. Each phase is committed on its own.

## 1. Goal

Fractals as a new layer of the picture, alongside the fluid, the waves and the cover art:
- Escape-time fractals (Mandelbrot, Julia and others), coloured with the look's palette,
  so they belong to the same picture as the fluid.
- Moving with the music: the Julia point wandering, zoom pulsing, jumps on drops.
- Carried by the feedback and bent by the fluid's flow, so they're part of this app
  rather than pasted on top.

## 2. Where things stand

| Piece | Today |
|---|---|
| Layers | The scene is drawn into the post-processing's HDR scene texture: the fluid (2D or 3D, or nothing for a look with no fluid), then the cover art (`render::art`), then the waves and shapes (`render::waves`), then the feedback warps the result, and the walls go on top (`main.rs`, `render()`). |
| Cover art | The closest model for a fractal layer: one shader, its settings (`ArtSettings`) part of a look's waves (`[art]` in the file), drawn before the feedback so the warp carries it. |
| Looks with no fluid | Already supported (the "no fluid" tag): the scene is cleared and only the layers and feedback draw. |
| Equations | Per-frame programs set the motion, the frame outputs (`FRAME_OUTPUTS`), `q1` to `q8` and solver parameters; per-pixel ones are compiled to WGSL (`expr`). |
| Flow | The feedback already has a screen-space flow texture (the fluid's velocity, uv per second: `feedback.flow_target`). |
| 3D | The 3D renderer raymarches the gas volume (`render3d/march.wgsl`) with an orbiting camera. |
| Randomizer | Tables of ranges and weights per kind of look, with tame rules against blowing out (`looks/random.rs`). |

## 3. The work

### 3.1 The fractal layer (2D)

- `render/fractal.rs` and `fractal.wgsl`, modelled on `render/art.rs`: one full-screen
  pass, each pixel iterating the fractal.
- **Kinds:** Mandelbrot, Julia, Burning Ship, Newton (z³ − 1), and one of Phoenix or
  Tricorn.
- **Colour:** smooth (fractional) iteration counts, through the look's palette, with an
  offset that cycles over time.
- **Settings** (`FractalSettings`): kind, centre, zoom, rotation, iterations, the Julia
  point `c`, opacity, brightness, colour speed. A `[fractal]` section in look files,
  carried through `LookFrame`, blending between looks, and loading and saving.
- **Where it draws:** after the fluid, before the cover art and waves, so the feedback
  carries it and the walls stay on top.
- **Precision:** f32 gives about 7 digits, so the picture breaks into blocks past about
  10⁵× zoom. Zoom is capped there; deep zooms (perturbation) are out of scope.

### 3.2 Music and equations

- **Frame outputs:** `fractal_zoom`, `fractal_rot`, `fractal_x`, `fractal_y`, `julia_re`,
  `julia_im`, `fractal_iter`, so looks can drive the fractal like anything else.
- **Built-in motion,** with no equations needed:
  - The Julia point travels along the Mandelbrot set's edge (the cardioid and the main
    bulbs), where the shapes are richest, at a speed set by the music.
  - Zoom pulses with the bass.
  - A drop or a new section jumps to another interesting spot, from a list per kind.
- **A reaction effect,** `fractal_morph`, so a kick or snare can kick the Julia point.

### 3.3 Tied to the fluid

- **The flow bends the fractal:** the fractal is sampled at each pixel shifted by the
  local flow (the feedback's flow texture), so the fluid visibly pushes it around.
- **Optional, harder:** the fractal releasing dye into GPU stable fluids. That means
  passing a texture into the solver; last, or dropped.

### 3.4 Editor, docs and randomizer

- **Editor:** a Fractal section in the look editor, with the existing controls.
- **Docs:** `[fractal]` and the new frame outputs in `looks/README.md`.
- **Randomizer:** fractal ranges in `looks/random.rs`, so it sometimes picks a fractal
  over or instead of a fluid; a tame rule so a fractal under strong feedback doesn't
  blow out.
- **Load:** fewer iterations when the GPU is under strain (`governor`). A full-screen
  fractal at 4K is the heaviest pass on this list.

### 3.5 Looks and checks

- **Built-in looks,** 3 or 4: a Julia set with no fluid under tunnel feedback, a
  Mandelbrot behind GPU stable fluids ink, a Burning Ship with mirror symmetry.
- **Tests:** settings load and save, and partial ones fill in defaults; equation outputs
  reach the fractal; the Julia point's path stays in range; a rendered frame (on a GPU,
  through `render::grab`) is neither black nor blown out.
- **In the app:** frame rate in full screen, and switching looks.

### 3.6 3D fractals (optional)

- A raymarched Mandelbulb or Mandelbox as a backdrop for 3D looks, the camera orbiting
  it. A separate shader, borrowing the camera and orbit from `render3d`. The most work
  and the most GPU load, so last.

## 4. Decisions (open)

1. **3D:** is §3.6 in scope, or 2D only for now?
2. **Fluid coupling:** the flow bending the fractal (§3.3) only, or the fractal
   releasing dye too?
3. **Kinds:** the five in §3.1, or others too (Lyapunov, IFS flames)? Those are
   different algorithms and add work.

## 5. Phases

| Phase | Scope | Visible result |
|---|---|---|
| **A. Fractal layer** | The shader and its settings, the `[fractal]` section, drawn before the feedback (§3.1). | Fractals in any look, warped by the feedback. |
| **B. Music** | Frame outputs, the Julia point's path, zoom pulses, jumps on drops, the reaction effect (§3.2). | Fractals that move with the music. |
| **C. Fluid coupling** | The flow bending the fractal; dye from the fractal, if chosen (§3.3). | The fluid pushing the fractal around. |
| **D. Editor and randomizer** | The editor section, docs, randomizer ranges and tame rule, load (§3.4). | Fractals from the dice and editable by hand. |
| **E. Looks** | Built-in looks, tests, checks in the app (§3.5). | Fractal looks ready to play. |
| **F. 3D** (optional) | Mandelbulb or Mandelbox (§3.6). | 3D fractals to orbit. |

Suggested order: A to E in order (each builds on the one before); F only if chosen.

## 6. Progress

- **Phase A, the fractal layer (done):** `render/fractal.rs` and `fractal.wgsl`.
  - **Kinds:** Mandelbrot, Julia, Burning Ship, Tricorn and Newton (z³ − 1); Tricorn
    over Phoenix, as it needs no extra state per pixel.
  - **The pass:** one triangle over the canvas, after the fluid and before the cover
    art, waves and feedback, so the feedback carries it and the walls stay on top. Four
    supersamples a pixel on a rotated grid, so filaments don't shimmer for the feedback
    to smear.
  - **Colour:** the smooth iteration count, on a log scale from the far outside to the
    set's edge (so the bands stay even as the counts climb), through the palette as a
    loop (deep, base, highlight, base, deep), cycling at `colour_speed`. Alpha follows
    the same depth: clear far outside, opaque toward the edge, and clear inside the set,
    so the fluid shows through both. Newton is coloured by the root each pixel finds,
    smoothed by how deep its last step landed (quadratic convergence).
  - **Settings** (`FractalSettings`, part of the waves like the cover art): kind,
    centre, zoom (1: the canvas is 3 units high), rotation, spin, iterations (up to
    1000), the Julia point, opacity, brightness, bands and colour speed. A `[fractal]`
    section in look files (unknown keys are errors, and it's written back only when
    on), and a Fractal group under the View tab's Waves.
  - **Zoom** is capped at 10⁵ (f32's limit), and the centre and Julia point at ±4.
  - **Tests:** the section loads, defaults to off, rejects a misspelt key and survives
    writing; zoom stays in range; the uniforms match the shader's size; the kinds keep
    the shader's numbers.
  - **Checked in the app** (debug build, full screen) with four trial looks: a Julia
    set with tunnel feedback and no fluid, the Mandelbrot set over GPU stable fluids
    ink (123 fps), a Burning Ship at 25× and Newton (240 fps, the cap, without
    fluid). Newton's colours first came out in flat steps: its smoothing term was
    clamped the wrong way, so always 0.
  - Not yet: frame outputs and the music (phase B), the format in `looks/README.md`,
    the randomizer and built-in looks (phases D and E).
- **Phase B, music and equations (done):** `fractal::Motion`, the frame outputs and the
  Morph reaction.
  - **The fractal's own movement** (`Motion`), applied to the look's settings before
    its equations run, so they start from it and can still override it:
    - `wander`: a Julia set's point travels along the edge of the Mandelbrot set's main
      cardioid, starting from the edge nearest the look's own point, about laps a
      minute at full loudness (30 % of that in silence). It sits 0.01 outside the edge
      (lacy, broken sets), and the bass moves it from 0.02 inside to 0.03 outside.
    - `bass_zoom`: the zoom follows MilkDrop's `bass_att`, from half to double.
    - `tour`: at each new section of the song (the analyser's section changes; those
      before it starts don't count), it glides to another spot from a list per kind
      (time constant 0.6 s, in log zoom); a Julia set's point jumps to a random place
      on the edge instead.
    - Off, or with none of these set, the settings pass through unchanged (a test).
  - **Tour spots:** 7 for the Mandelbrot set, 5 for the Burning Ship, 4 each for the
    Tricorn and Newton, each starting with the whole set. Chosen by scoring candidates
    on a 64 × 36 grid; a test keeps them honest: every spot shows the edge (some of the
    set but not mostly the set, or slow escapes; for Newton all three roots and slow
    points), using a CPU copy of the shader's iterations.
  - **The Morph reaction** (`effect = "morph"`), one-shot: a kick that falls over
    0.35 s, throwing a Julia set's point along the edge (0.5 rad/s at full kick: about
    a tenth of a radian a hit) and zooming in by up to a quarter. It reaches the
    fractal a frame late (the reactions run after the look's frame).
  - **Frame outputs:** `fractal_x`, `fractal_y`, `fractal_zoom`, `fractal_rot`,
    `fractal_iter`, `fractal_opacity`, `julia_re`, `julia_im`, clamped like the
    settings.
  - **Docs:** `[fractal]`, the outputs and `morph` in `looks/README.md` (planned for
    phase D, but the outputs needed them).
  - **Editor:** Wander (Julia only), Bass zoom and Tour in the Fractal group.
  - **Tests:** the spots; settings passing through; the Julia point staying on the edge
    and travelling further when loud; bass and Morph zooming in; the tour moving at a
    section and not before; Morph's kick and decay.
  - **Checked in the app:** the Mandelbrot set over GPU stable fluids ink, pulsing with
    the bass and turned by an equation (144 fps); a Julia set wandering at 1 with Morph
    on the kick, its spirals tightening as it goes. At first Morph threw the point at
    2.5 rad/s (nearly a radian a kick), and with wander 3 under tunnel feedback the
    shape changed so fast that the feedback smeared it into a blur: the leap is now
    0.5. Under feedback, keep wander at about 1 or less. The tour wasn't seen in the app
    (the test song had no section change in the time watched); its tests cover it.
  - One full test run sat for over 10 minutes and was stopped; three reruns took the
    usual time (the app's tests about a minute). It looked like the GPU tests waiting
    on a busy GPU rather than a test of this phase.
