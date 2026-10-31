# Fractals Plan

Status: **done** (2026-10-30): phases A to F and the dive; two decisions still open (§4: the fractal's dye, other kinds). Fractal visualizations as an option
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

### 3.2b The dive (asked for after phase B)

- Fractals travel endlessly into themselves (or out), moving with the music but never
  stopping. f32 holds only about 10⁵× of zoom, so a dive loops instead of zooming for
  ever: it flies to a point where the picture repeats itself (a Misiurewicz point of the
  Mandelbrot set, a Julia set's repelling fixed point, the centre of Newton's star) and
  goes round the loop between two zooms that look the same.

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
| **B2. Dive** | Endless travel in or out: loops at self-repeating points, a flight there, the colours kept still (§3.2b). | Fractals forever travelling into themselves. |
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
- **The dive (done, asked for after phase B):** `dive` in `[fractal]` and `Motion`'s
  dive (§3.2b).
  - **Why loops:** zooming for ever runs out of f32 at about 10⁵×. Around certain
    points the picture repeats itself: zoomed in by |λ| and turned by arg λ it's the
    same picture again, a whole number of iterations deeper. A dive goes round that
    loop for ever, jumping back by exactly one turn of it, which can't be seen.
  - **The points:**
    - Mandelbrot: nine Misiurewicz points, whose critical orbit falls into a repelling
      cycle. The set near one repeats itself as the cycle's multiplier says (Tan Lei),
      `period` iterations deeper each time round. Found among the low-order ones (as
      polynomial roots, then refined) for being branch points or tips worth seeing.
    - Julia: the β fixed point (z² + c = z), multiplier 2β, one iteration a turn. It
      moves as the Julia point wanders, and the loop follows it.
    - Burning Ship and Tricorn: they match the Mandelbrot set on the real axis, so two
      of its real Misiurewicz points loop for them too, over two turns of the cycle (λ
      is negative there and they aren't symmetric about the axis).
    - Newton: 0. ∞ is a repelling fixed point of Newton's map (multiplier 3/2) and 0
      maps to it through 1/(3z²), so the six-armed star repeats every √1.5.
  - **Seamless:** a test checks every point: one turn in shows the picture it started
    from, its period deeper (median gap under a quarter of an iteration: well under 1 %
    of the palette at these depths; inside and outside agree on 99 % of points). Each
    loop's widest view was chosen numerically as the widest that matches.
  - **Colours stay still:** `FractalSettings::shift` (set by `Motion`, not saved) takes
    the depth off the counts before colouring: the typical count around the loop (24
    points on a ring, eased) plus the period times the loop's phase. So the background
    stays clear at any depth, and the colours don't jump at the wrap.
  - **Leaning:** at a tip, half the canvas would be dark, so the view leans toward the
    set's arm (the counts around the point say which way; at a branch point the arms
    balance and it stays centred), by up to 0.8 of a half-height. The lean is in screen
    terms, so it scales and turns with the loop and the wrap stays seamless; the loop
    narrows by as much, so the canvas reaches no further from the point.
  - **The flight there:** from the look's own `centre` and `zoom` to the loop, along van
    Wijk and Nuij's optimal zoom-and-pan path (zooming out on the way as far as the
    distance needs), at the dive's pace, always in. With `tour`, each new section flies
    on to another of the kind's points (a Julia set's point jumps along the edge).
  - **With the music:** zoom doublings a second, from 0.4 times `dive` in silence to 1.6
    times when loud, and up to 4 times on a Morph kick; negative goes round the loop
    outward. `bass_zoom` and Morph's punch still pulse the zoom on top.
  - **Editor and docs:** a Dive slider (−3 to 3), and `dive` in `looks/README.md`.
  - **Tests:** every point loops without a seam; a flight runs from one view to the
    other (zooming out when far); every kind, in and out, flies in smoothly then keeps
    going its way for 90 s, wrapping by exactly one turn (zoom, turn and shift) and
    never leaping between; a tour dives on to another point.
  - **Checked in the app** (debug build): the Mandelbrot set diving into a branching
    star over GPU stable fluids ink (124 fps), a wandering Julia set diving into its
    spirals, Newton's star, and the Burning Ship flying from its ship down to its
    spiked axis (240 fps without fluid). The Julia set first showed half a dark canvas
    (its point is a tip), hence the lean. The wraps themselves weren't caught on
    screen (stills 3 s apart); the seam test covers them. The Burning Ship's and
    Tricorn's axis loops are the plainest of the dives.
- **Phase C, the fluid bending the fractal (done):** `render/bend.rs` and `bend.wgsl`,
  and `bend` in `[fractal]`. The fractal releasing dye (§3.3's optional part) isn't
  done: that's still an open decision (§4).
  - **How:** a displacement texture at the scene's size (Rg16Float, uv like the flow),
    twice over (this frame's from last frame's). Each frame every pixel takes the
    displacement carried in from upstream (semi-Lagrangian, through the flow) plus the
    flow's push, eased back toward none; the fractal pass reads it at its pixel and
    draws the picture from where the fluid would have carried it. A plain offset by
    the velocity would have snapped back the moment the flow stopped; this drags the
    picture and lets it settle.
  - **The flow** is the feedback's (uv per second): the solver's velocity grid, or
    particles splatted onto one, in 2D; the raymarch's in 3D. With no feedback it's now
    drawn for the bend too, at most 30 times a second (`BEND_FLOW_HZ`): getting the
    velocity costs a GPU readback or a particle splat, and the bend eases anyway.
  - **Tuning, in the app:** at first the displacement remembered 1.5 s and reached a
    third of the screen, and the ink's turbulence kneaded the Mandelbrot set into
    marble, past recognising. Now it springs back over 0.6 s and reaches at most 0.12
    of the screen. Over FLIP the splatted flow is noisy and shredded the Julia set, with
    streaks in the air above the water: the bend now averages the flow over a small
    disc (9 taps, 0.015 of the screen), and the Julia set rides the waves whole.
  - **Cost** (debug build, full screen): the Mandelbrot set over GPU stable fluids ink
    116 fps unbent, 99 bent; at first, drawing the flow every frame, 81. Over FLIP no
    clear difference (the solver's own cost swamps it).
  - **Editor and docs:** a Bend slider (0 to 2), the `fractal_bend` frame output, and
    `bend` in `looks/README.md` (0.3 to 0.5 keeps it recognisable under a lively fluid;
    1 goes with the fluid).
  - **Tests:** on a GPU, a steady flow over half the scene drags the picture toward
    its pace times the spring's time, leaves the still half alone, springs back at the
    spring's rate when the flow stops, and saturates under a gale; both shaders
    validate (naga). The test readback now handles signs and two-channel textures.
- **Phase D, the randomizer, the editor and the load (done):**
  - **Random fractals** (`looks::random::random_fractal`): 45 % of looks without a fluid
    get one (the waves then stay only sometimes, and the feedback is optional: the
    fractal is enough to draw), 20 % over a 2D fluid, 12 % over a 3D one. Julia sets
    most often, then the Mandelbrot set, then the rest. It starts at one of the kind's
    tour spots (the whole set half the time alone, 80 % over a fluid, whose close-ups'
    fine filaments it hides), and moves one of three ways: diving (mostly in), touring,
    or still and turning; a Julia set's point wanders too, mostly. Over a 2D fluid it's
    often bent by the flow. Half get a Morph reaction on the kick or snare. Variations
    nudge its numbers.
  - **The tame rules, from what the app showed:**
    - Under brightest feedback a fractal's glow, which covers the canvas, piled up
      into a pale wash, even dimmed: a fractal now always takes smear feedback.
    - Even smeared, a long trail, a feedback zoom, turn, echo or drift blurred it to
      mush, since the new frame is under 1 % of each: with a fractal the trail is at
      most 0.5, the zoom 0.99 to 1.02, little turn, no echo or drift.
    - A kaleidoscope folds a fractal past reading: none with a fractal (mirrors stay).
    - Its opacity over a fluid is 0.7 to 0.95 (0.5 was lost in it).
  - **Over a flowing fluid, the fractal goes over the feedback** (`main.rs`, `render`):
    random fractals over fluids didn't show at all, and the cause was the feedback's
    flow. The fluid's dye is redrawn where the flow carried its history, so it stays
    sharp; a fractal standing still has its history carried off a different way every
    frame, and dissolved into the fluid in about a second (a look's shapes too). So
    with a fluid and the feedback's flow above 0, the fractal is drawn over the
    feedback's picture, under the walls; the bend is what moves it with the fluid.
    Without a fluid, or without flow, it's still drawn into the feedback, which swirls
    it.
  - **Load:** a second governor watches the frames while a fractal is on, in the
    visualizer: slower than 50 fps (or than the frame-rate cap, if that's lower; a
    capped rate isn't falling behind) for 3 s, and the fractal drops a step: one sample
    a pixel instead of four, then 70 % of the iterations a step, down to a third. Each
    look starts at full detail. Not seen in the app (nothing fell that slow); its parts
    are tested.
  - **Editor:** the fractal has its own group under the View tab: the kind (and the
    Julia point), then how it moves (Dive, Wander, Tour, Bass zoom, Bend, Spin), then
    where (Zoom, Across, Up, Turn: where a dive starts from, when diving), with the
    colours and detail folded away.
  - **The tests that hung** (twice before, and twice in this phase's runs): opening GPU
    devices from many threads at once deadlocks (wgpu 30 on Windows). It started in
    the GPU stable fluids phase, when the app's tests began building worlds with the
    GPU solvers, each of which opened a device, so many tests opened one at the same
    moment; 16 threads opening devices hung in the second round. `GpuContext::new` now
    opens one device per process and shares it (wgpu's handles are made to be shared:
    16 threads using one ran 480 maps clean in half a second). The app's tests have
    run in their usual minute every time since (three runs, and the workspace), and
    sim3d's GPU tests went from over a minute, waiting on devices, to one second. In
    the app, the GPU solvers now share one device rather than each opening their own.
  - **Tests:** random fractals stay in range and are tamed (smear, short trail, still
    zoom, no echo or kaleidoscope), start somewhere worth seeing, and come in every
    kind, alone and over a fluid, diving both ways, touring and bent, with Morph
    reactions; a capped frame rate doesn't count as falling behind.
  - **Checked in the app** (debug build): twelve random looks from the key, then three
    batches of eight random fractal looks saved from the randomizer, after each round
    of rules: the last batch shows the Mandelbrot set over ink, a Julia set bent over
    PBF, the Tricorn over dye and over 3D fire, and the Mandelbrot set over 3D fire,
    all readable; alone, the Burning Ship and the Tricorn read but soft (hence the
    stiller zoom, after that batch). The Fractal group in the editor.
- **Phase E, the looks (done):** four built-in looks, and a frame test on the GPU.
  - **The looks:**
    - *Julia tunnel*: no fluid; a Julia set wandering and diving in, kicked by Morph.
    - *Ink set*: the Mandelbrot set diving into a branch point over GPU stable fluids
      ink, bent by it.
    - *Ship of fire*: the Burning Ship touring over the campfire, mirrored.
    - *Newton's star*: Newton's six-armed star, travelling out for ever.
  - **Julia tunnel** first had a light feedback (a third left after a second): diving
    and wandering, its every frame new, it smeared into a green blur. Without feedback
    it's crisp (the dive is the tunnel). The randomizer now does the same: no feedback
    for a diving or wandering fractal without a fluid (a test). Newton's star keeps its
    light trail: its dive streams outward along its own arms, so the trail draws them
    out rather than blurring them.
  - **Frame test** (on a GPU): every kind, whole and deep in a dive, lights part of the
    canvas, none of it blown out or not a number (Newton's whole view is lit all over,
    as every point finds a root: checked for its edges' contrast instead).
  - **Checked in the app** (debug build, full screen): all four, stepping between
    them: Julia tunnel 240 fps, Ink set 108 to 115, Ship of fire 123 (where the
    governor lowered the fire's grid once, as it does for that scene), Newton's star
    234 to 240. The first run showed other looks: only the tests had been rebuilt, so
    the app was the old one.
  - The README's look count was behind (32, 12 without fluid): now 46 and 15, and
    fractals are in its list of what the visualizer does.
- **Phase F, 3D fractals (done):** the Mandelbulb and the Menger sponge, raymarched in
  the same shader (`fractal.wgsl`'s `march`).
  - **The kinds:** two more `FractalKind`s, `mandelbulb` and `menger`. A 3D kind's
    camera orbits it (`rotation` and `spin` its angle, `zoom` its closeness; `centre`
    isn't used), from 0.35 rad above its equator, with a 46° field of view. One sample a
    pixel (a raymarch is too much work for four), and `iterations` is the steps along a
    ray, so the load governor's cut applies.
  - **Raymarching:** the Mandelbulb's distance estimate (power n, 10 iterations, an
    orbit trap for its colour) and the Menger sponge's (Inigo Quilez's, measured from
    its corner so the finest holes keep their precision), marched from a bounding
    sphere (rays that miss it cost nothing), hit within half a pixel's width. Lit from
    above the camera, darkened in crevices (the steps it took), a rim in the highlight;
    near misses glow, far ones are clear, so a fluid shows round it.
  - **With the music:** a Mandelbulb's `wander` swings its power 3 either side of its
    own, the lobes growing and merging, kicked by Morph. The Mandelbulb doesn't dive
    (it doesn't repeat itself as it's zoomed); its Dive control is hidden.
  - **The sponge's dive:** the sponge shrunk to a third about its corner is its corner
    cube, so the camera goes round a loop from 3 to 1 from the corner, a level of holes
    deeper each time, the colours counting out from the corner less the dive's shift.
    The first version dived into the sponge itself: its corner cube matches only
    within itself, and the camera sees its neighbours, and through its holes the far
    side, which the wider end of the loop finds empty, so at the wrap they'd pop in and
    out (the first test only sampled inside the corner cube, so it passed). Now it dives
    into the sponge grown outward from its corner (3, 9, 27, 81 times the size, each with
    the last as its corner), which shrunk by a third is itself everywhere; its far parts
    fade out at 12 to 25 camera distances, the same at every turn. The camera looks in
    from within 0.5 rad of the diagonal, so never from inside, and aims a little into
    the cube, so the faces fill the canvas.
  - **In 3D looks** the fractal is now a backdrop: the scene is cleared to the 3D
    background, the fractal drawn, and the 3D renderer keeps it (`keep`), drawing the
    floor and the gas over it; a 3D kind turns with the look's camera. (Unless the
    feedback's flow would wash it out: then it goes over the picture, as in 2D.)
  - **Editor and the rest:** Power (the Mandelbulb), Wander (the Julia set and the
    Mandelbulb), Dive (all but the Mandelbulb), Zoom and Turn for the 3D kinds' camera;
    the `fractal_power` frame output; the randomizer picks the 3D kinds too (always
    turning, and with no feedback when alone: a turning solid smears like any moving
    fractal); `looks/README.md` and the README.
  - **Looks:** *Sponge dive* (no fluid: the sponge's endless dive, in Mercury) and *Bulb
    in smoke* (a Mandelbulb swinging its lobes behind the 3D smoke, the camera
    orbiting).
  - **Tests:** the grown sponge repeats itself at a third about its corner everywhere
    within six of it (inside it and out); the diving camera always looks at the corner
    from outside the cube; the GPU frame test covers the 3D kinds too.
  - **Checked in the app** (debug build): Bulb in smoke at 109 to 139 fps (the smoke's
    plume rising in front of the bulb), Sponge dive at 240. The glow round the solids
    was too wide at first (a white haze round the sponge): narrowed.
