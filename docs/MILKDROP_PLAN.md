# MilkDrop-Style Visualizer Plan

Status: **approved; phases A to E, G and I done** (E with the analyzer plan, which did most of its audio work; I with part of F: ratings, favourites, tags and hard cuts; F's blended transitions not yet). Decisions (§9) taken as recommended (2026-10-25):
- our own MilkDrop-inspired preset format, with a classic `.milk` importer later;
- presets in TOML;
- our own equation language, interpreted on the CPU and compiled to WGSL per pixel;
- raw WGSL composite snippets allowed, in a late phase;
- presets without fluid allowed;
- song metadata from the Windows media session;
- extras: classic import, screenshots and recording first; Spout, MIDI and OSC later
  if wanted;
- start with phases A and B.

## 1. Goal

Turn the fluid toy into a full music visualizer in the spirit of MilkDrop: something
you put on full screen with Spotify playing and leave for the evening, which never
looks the same twice and always looks like it's listening.

What "akin to MilkDrop" means here:
- **The feedback look:** each frame is built on the last one, warped (zoomed, turned,
  bent) and faded, so motion leaves trails and patterns grow out of themselves.
- **Presets as data:** hundreds of looks, written as small text files with equations
  driven by the music, which anyone can edit and share.
- **Smooth hands-off running:** blended transitions between presets on the music's
  phrases, hard cuts on drops, shuffle with ratings and a lock.
- **Waves and shapes:** oscilloscope and spectrum drawings, and simple shapes, that
  the feedback turns into tunnels, spirals and smears.

What stays ours:
- **The fluid simulations are the content.** MilkDrop's images come from waveforms fed
  back through a warp. Ours can also come from real fluids (FLIP water, ink, fire, 3D
  smoke), and the fluid's own velocity can drive the warp. That's the distinctive part,
  and the plan keeps it central.
- Everything that works today keeps working: the 2D and 3D solvers, the toy UI, the
  analyzer, reactions, director, soak tests.

## 2. Where we are

The visualizer plan's phases A–E (and the 3D plan's A–E) are done:

| Area | Today |
|---|---|
| Audio | Live system audio (WASAPI loopback) or a track file. Streaming analyzer: 5 bands with automatic gain, level, brightness, kick/snare/hat onsets, silence. |
| Reactions | 10 triggers × 9 effects (splash, jet, vortex, flash, gust, slosh, breathe, hue, parameter), lifted into 3D for the volume solvers. |
| Looks | 9 hand-tuned looks coded in Rust (`looks.rs`): 6 in 2D, 3 in 3D. |
| Director | Changes look on new songs, drops and a timer (on a kick), picking calm or energetic looks by loudness, least recently shown first. Fades through black. |
| Presentation | Full screen (F11), hidden UI with a reveal overlay, resolution governor, idle drift in silence, 30-minute soak test. |
| Rendering | One pass per frame straight to the screen: particles, liquid surface, dye fields, raymarched 3D gas. No offscreen buffers, no post-processing. |

The gaps, against MilkDrop:

| MilkDrop has | We have | Gap |
|---|---|---|
| Feedback buffer with per-pixel warp, decay, echo | Nothing between frames | **Large:** the core of the look |
| Presets as text with per-frame and per-pixel equations | 9 looks coded in Rust | **Large:** content and variety |
| Thousands of community presets | 9 | Content |
| Blended transitions (2–5 s, with patterns), hard cuts on beats | Fade to black | Medium |
| Waveform and spectrum drawing, custom waves and shapes | None | Medium |
| bass/mid/treb relative to their average, plus smoothed `_att` versions | AGC-normalised bands, onsets | Small: different, should add MilkDrop's style |
| Song title display, ratings, preset browser, many hotkeys | Look name, prev/next, lock | Medium: UX |
| Composite shaders (colour, symmetry, glow) | None | Medium |

What we have that MilkDrop doesn't: real fluid simulation, 3D volumes, music
reactions that push on physics, and onset detection per drum.

## 3. How MilkDrop works (briefly)

For reference, since the design borrows from it:
- Each frame: run the preset's **per-frame equations** once (set variables like `zoom`,
  `rot`, `warp`, `decay`, wave colour from `time`, `bass`, `mid`, `treb`...), then the
  **per-vertex equations** on a coarse mesh (e.g. 48×36), which can vary `zoom`, `rot`,
  `dx`, `dy` by position (`x`, `y`, `rad`, `ang`).
- **Warp:** the previous frame is drawn through that mesh (each vertex samples the old
  image at a moved position), then darkened by `decay`. That's the feedback.
- **Draw:** waveforms, custom waves and custom shapes are drawn on top, into the
  feedback image, so the next frame warps them too.
- **Composite:** the feedback image goes to the screen through effects (gamma,
  brightening, darkening at the centre, echo, colour inversion; in MilkDrop 2, a
  preset's own HLSL shader). The composite isn't fed back.
- **Transitions:** the old and new presets run together for a few seconds, blended
  through a mask pattern.
- Audio: `bass`, `mid`, `treb` are each band's level relative to its recent average
  (1.0 is average, 2.0 twice as loud); `bass_att` etc. are smoothed versions.

projectM is the open-source reimplementation (C++, OpenGL). It loads `.milk` files,
translating MilkDrop 2's HLSL to GLSL.

## 4. Architecture

### 4.1 The render graph

Today the scene draws straight to the screen. The new pipeline draws into offscreen
HDR textures (`rgba16float`) and builds the picture in layers:

```
  simulation ──► scene texture ──┐
                                 ▼
  previous frame ──► warp + decay ──► + scene + waves/shapes ──► feedback texture ─┐
        ▲                                                                          │
        └──────────────────────────── next frame ◄─────────────────────────────────┘
                                                                                   ▼
                                     composite (colour, symmetry, bloom, tone map) ──► screen ──► UI
```

- **Scene:** the existing 2D and 3D renderers, unchanged except for their target: they
  draw into a texture at a chosen render scale (e.g. 0.75 of 4K), not the screen.
- **Feedback:** the previous feedback texture, sampled through the warp (MilkDrop-style
  motion and/or the fluid's velocity, §4.3), times `decay`, plus the new scene composited
  over it (add, screen, max or replace, per preset), plus the waves and shapes. Two
  textures, ping-ponged.
- **Composite:** never fed back. Kaleidoscope and mirror symmetry, colour grading (hue,
  saturation, contrast, palette mapping), bloom, chromatic aberration, vignette, echo
  (a zoomed second copy), then tone mapping into the sRGB surface.
- **UI:** egui on top, as now.

With the feedback set to "decay 0, replace", the picture is today's: presets without
feedback lose nothing.

### 4.2 Presets as data

The Rust `LOOKS` table becomes a preset format, with the current looks as the first
built-in presets.

- **Files:** TOML **(?)**, one preset per file. Built-ins compiled into the binary; user
  presets in a folder (`%APPDATA%/fluid-toy/presets`), loaded at start and **hot
  reloaded** when a file changes, so editing a preset shows the result at once.
- **Sections:**
  - `[meta]`: name, author, tags, energy (calm, energetic, any), rating.
  - `[sim]`: solver, scene, palette, parameters, reactions (today's look contents). May
    be `none`: a pure MilkDrop-style preset with no fluid **(?)**.
  - `[feedback]`: decay, warp mode and strength, blend mode, echo.
  - `[frame]`: per-frame equations.
  - `[pixel]`: per-pixel warp equations.
  - `[[wave]]`, `[[shape]]`: drawn elements, each with its own equations.
  - `[composite]`: symmetry, colour grading, bloom, and optionally a WGSL snippet **(?)**.
- **Validation:** loading reports errors by line (unknown solver, bad equation) in the
  UI and skips the preset instead of crashing; a test loads every built-in.

### 4.3 The warp, and the fluid's part in it

The warp moves each pixel of the previous frame before decaying it. Two sources,
mixed per preset:
- **MilkDrop motion:** zoom, rotation, translation (`dx`, `dy`), centre (`cx`, `cy`),
  stretch and the wavy `warp`, set per frame and varied per pixel by equations. This
  alone recreates the classic tunnels and spirals.
- **Fluid motion:** sample the simulation's velocity field and move the image along it
  (semi-Lagrangian advection of the whole picture). The trails then follow the real
  flow: ink curls leave glowing wakes, FLIP splashes smear like long exposures, and a
  vortex reaction twists the whole screen. 2D grid solvers already have velocity
  fields; particle solvers splat theirs to a grid; 3D projects the volume's velocity
  onto the screen plane.

Per-pixel equations run on the GPU. Rather than MilkDrop's coarse mesh, the plan
**compiles them to WGSL**: the expression language is small enough to translate
directly, so each preset gets its own warp shader, built at load and validated by
naga before use.

### 4.4 The expression language

MilkDrop's equations are a tiny C-like language. Ours is close to it, so presets are
easy to write and classic ones easy to convert:
- `zoom = 1.0 + 0.05*bass_att; rot = 0.02*sin(time*0.3);` and so on: assignments,
  arithmetic, comparisons, `if(cond, a, b)`, maths functions (`sin`, `abs`, `pow`,
  `sqrt`, `rand`, `sigmoid`...).
- **Inputs:** `time`, `frame`, `fps`, `bass`, `mid`, `treb` (and `_att`), the five bands,
  `level`, `brightness`, onset pulses (`kick`, `snare`, `hat`), beat and bar phase,
  `song_time`, `progress`; per pixel, also `x`, `y`, `rad`, `ang`; shared `q1`–`q8` from
  the frame equations to the pixel, wave and shape ones.
- **Outputs:** the warp's controls, wave and shape properties, composite controls, and
  some fluid controls (gravity tilt, a reaction's gain, solver parameters by key), so
  equations can drive the physics too.
- **Implementation:** our own parser (about 400 lines: tokenizer, Pratt parser, an AST)
  with two back ends, a CPU interpreter for per-frame and wave equations and a WGSL
  generator for per-pixel ones **(?)**. Errors carry positions for the editor.

### 4.5 Audio, MilkDrop-style and better

- **MilkDrop's variables:** `bass`, `mid`, `treb` as each band's energy over its own
  running average (so 1 is typical for this song), with `_att` smoothed versions. Easy
  to derive from the analyzer's raw band energies.
- **Tempo and beat phase** (the old phase G): an onset-strength signal, tempo by
  autocorrelation (60–200 BPM, with octave correction), and a phase-locked beat clock.
  Gives `beat` (a pulse on each beat, even quiet ones), `beat_phase` (0..1 between
  beats) and `bar_phase` (four beats), so motion can lock to the tempo and transitions
  can land on bar lines.
- **Sections:** spectral novelty over several seconds marks build-ups, drops and
  breakdowns, which is better than today's loudness jump for the director.
- **Latency:** loopback audio arrives ahead of or behind what's heard, depending on the
  system's output buffering. A sync offset (with a simple calibration screen that
  flashes on detected beats) shifts the analysis to match.

### 4.6 Song awareness

On Windows, the system media session (`GlobalSystemMediaTransportControlsSessionManager`,
via the `windows` crate) reports what Spotify is playing: title, artist, album art,
position, play/pause **(?)**. That gives:
- exact song changes (instead of guessing from silence);
- a MilkDrop-style title card when a song starts;
- the album art as an image the fluid can carry and the feedback can warp, and a
  palette drawn from it (dominant colours), so each song gets its own colours.

## 5. Features in detail

### 5.1 Waves and shapes

- **Waves:** the classic modes: circular oscilloscope, line scope, spectrum bars,
  mirrored spectrum, XY (left vs right channel) dots. Drawn into the feedback image
  with additive glow, so they streak into tunnels.
- **Custom waves:** a preset defines up to four, each point placed by its own equations
  (MilkDrop's per-point code), from the waveform or spectrum value at that point.
- **Shapes:** up to four polygons per preset (sides, radius, position, rotation, colour,
  textured with the previous frame or the album art), each with equations.
- **Into the fluid:** optionally, a wave also pushes the fluid (a ring of brushes
  following the scope), so the sound visibly stirs the liquid.

### 5.2 Transitions and the director

- **Blends:** the outgoing preset's feedback image is kept and crossfaded into the new
  one over 2–5 s through a pattern (plain dissolve, radial wipe, noise mask, zoom blur).
  The new preset's warp starts from the old picture, so looks grow out of each other
  instead of fading through black. Changing the fluid solver still needs a reset, so
  the blend covers it.
- **Timing:** changes land on bar lines (from the beat clock); drops get **hard cuts**
  on the downbeat; breakdowns get slow blends.
- **Choice:** energy and section as now, plus preset ratings (rated presets come up
  more), favourites, tags (e.g. only "water" presets tonight) and playlists; still least
  recently shown, with the lock.

### 5.3 Composite and colour

- Symmetry: kaleidoscope (N-fold), mirror X/Y, quad mirror.
- Colour: hue rotation, saturation, contrast, palette remapping (luminance to a palette
  gradient), invert, beat flashes (today's flash and hue reactions move here).
- Glow and lens: bloom (the old phase F), chromatic aberration, vignette, film grain,
  echo.
- Tone mapping from HDR, so fire, glows and bloom roll off instead of clipping.

### 5.4 Interface

- **MilkDrop-like hotkeys** (alongside today's): Space for a blended next preset, H for
  a hard cut, Backspace for previous, L to lock, + and − to rate, T for the song title,
  F1 for help. Today's `,` and `.` stay.
- **Preset browser:** search, tags, ratings, favourites, thumbnails (rendered once per
  preset and cached).
- **Preset editor:** a side panel with the preset's equations, live: edits apply on each
  keystroke that parses, and errors are underlined.
- **Display:** choose the monitor for full screen, a windowed always-on-top mode, a
  render scale setting, a frame-rate cap.

### 5.5 Optional extras

- **Classic MilkDrop import:** convert `.milk` presets that don't use MilkDrop 2 shaders
  (most older ones): their per-frame, per-vertex, wave and shape equations and warp
  settings map onto our format. MilkDrop 2's HLSL shaders don't translate cleanly; those
  presets would load without their shaders, or not at all **(?)**.
- **Outputs:** screenshots, recording (frames piped to `ffmpeg`), Spout (Windows texture
  sharing, for OBS, Resolume and other VJ software).
- **Control:** MIDI controller mapping (knobs to warp, blend, intensity), OSC.

## 6. Performance

- **Budget:** 60 fps at 4K on the RTX 4090 with the fluid, feedback and composite
  together; weaker GPUs scale down the render resolution first, then the fluid's.
- **Render scale:** the scene and feedback render at a fraction of the screen (default
  0.75 at 4K) and the composite upscales. The governor learns to lower it.
- **Costs to expect:** each full-screen pass at 4K (8.3M pixels, 16-bit HDR) is roughly
  0.1–0.3 ms on the 4090. Feedback plus composite is about 6–10 passes with bloom, so
  2–3 ms. The 3D raymarch is the most expensive scene and gains most from render scale.
- **Measured, not guessed:** GPU timestamps per render pass, shown in the Info tab like
  the 3D solver's, and a bench mode that renders presets offscreen and reports times.

## 7. Testing

- **Expressions:** parser and interpreter unit tests (precedence, functions, errors with
  positions); every generated WGSL warp shader validated by naga in a test; the CPU
  interpreter and the GPU code agree on sample inputs.
- **Presets:** every built-in loads, refers to real solvers, scenes and parameters, and
  compiles; a malformed preset is reported, not fatal.
- **Render passes:** headless renders of small test scenes compared with stored images
  within a tolerance (golden images): warp identity leaves the image unchanged, decay
  halves it, zoom moves a dot outward, symmetry mirrors.
- **Beat tracking:** synthetic beats at 70–180 BPM, with swing, dropped beats and tempo
  changes; the tempo within 2% (octave errors counted), the phase within 30 ms.
- **Transitions and director:** blends finish, cuts land on downbeats, ratings shift
  the choice.
- **Soak:** the 30-minute soak runs the full pipeline (presets, feedback, composite)
  headless and checks for NaN in the feedback textures and steady frame cost.

## 8. Phases

Each phase leaves the app working and is committed on its own.

| Phase | Scope | Visible result |
|---|---|---|
| **A. Render pipeline** | Offscreen HDR scene target at a render scale; composite pass with tone mapping; bloom; GPU timings per pass. | Same pictures, now with glow; fire and bright liquid roll off nicely. |
| **B. Feedback and warp** | Ping-pong feedback textures; decay, zoom, rotation, translation, warp, blend modes, echo; warp by the fluid's velocity; feedback settings per look. | The MilkDrop feel: trails, tunnels and flow-following smears over the fluids. |
| **C. Waves and shapes** | Scope, spectrum and XY waves; custom shapes; optional wave-driven brushes. | Sound drawn on screen, streaking through the feedback. |
| **D. Presets as data** | Expression language (parser, interpreter, WGSL generator); TOML presets; built-ins moved out of Rust; hot reload; error reporting. | Looks are files: edit one and watch it change. |
| **E. Audio upgrades** | MilkDrop's bass/mid/treb and `_att`; tempo, beat and bar phase; section novelty; latency offset and calibration. | Motion locks to the beat; changes land on bar lines. |
| **F. Transitions and director** | Blended transitions with patterns; bar-aligned changes, hard cuts on drops; ratings, favourites, tags. | Presets flow into each other on the music's phrases. |
| **G. Composite and colour** | Symmetry, colour grading and palette mapping, chromatic aberration, vignette. | Kaleidoscopes and colour moods. |
| **H. Song awareness** | Media session metadata; title cards; album art texture and palette; exact song changes. | Each song announced and coloured by its cover. |
| **I. Library and interface** | 30+ built-in presets (fluid and pure-feedback); preset browser and live editor; MilkDrop-like hotkeys; monitor choice, render scale, windowed mode. | A visualizer you can browse, tweak and leave on. |
| **J. Extras (optional)** | Classic `.milk` import; recording, Spout; MIDI and OSC. | Bring old presets; stream or VJ with it. |

A and B bring most of the change in look; D is the biggest engineering step and the one
that makes variety cheap. A suggested order is A, B, C, D, E, F, then the rest by taste.

## 9. Decisions for you

1. **MilkDrop compatibility:** our own preset format, inspired by MilkDrop, with an
   optional importer for classic presets later (recommended); full `.milk`
   compatibility including MilkDrop 2 shaders (a very large job: an HLSL translator);
   or embedding projectM (OpenGL, which doesn't share textures with our wgpu renderer
   without a lot of interop work).
2. **Preset file format:** TOML (recommended: readable, comments, already common in
   Rust), JSON, or a MilkDrop-like `key=value` text.
3. **Equations:** our own small language close to MilkDrop's, interpreted on the CPU and
   compiled to WGSL for per-pixel work (recommended); or an embedded scripting language
   (Rhai, Lua), which is more capable but can't run per pixel on the GPU.
4. **Raw WGSL in presets:** allow presets to carry their own composite shader snippet
   (validated by naga before use) for advanced effects (recommended, but in a late
   phase); or keep presets to equations only.
5. **Presets without fluid:** allow pure feedback presets with no simulation, cheaper
   and closer to MilkDrop (recommended), or keep the fluid in every preset.
6. **Song metadata:** read Spotify's title, artist and album art from the Windows media
   session (recommended; Windows only, other platforms simply skip it), or stay with
   audio only.
7. **Extras (phase J):** which, if any: classic import, recording, Spout, MIDI/OSC.
   Recommended: classic import, screenshots and recording first (they serve everyone);
   Spout, MIDI and OSC later, if you want to use it for VJing or streaming.
8. **Order:** start with A and B (recommended: the biggest visible change first), or D
   first (presets as data before more features).

## 10. Progress

- **Phase A (done):** the render pipeline (`app/src/render/post.rs`, `post.wgsl`,
  `timer.rs`).
  - The 2D and 3D scenes draw into an offscreen `Rgba16Float` scene texture instead
    of the screen, at a render scale: automatic (full size up to 1440p, 75% above) or
    100, 75 or 50% (View tab, Picture). Pixel-sized things (the liquid splats, the 3D
    viewport and depth) follow the scale.
  - Bloom: the scene, soft-thresholded at 0.8, is downsampled through up to seven
    half-size levels (Jimenez's 13-tap filter) and blurred back up with a 3×3 tent,
    each level added onto the one above. Glow strength is a View tab setting (0.35 by
    default; 0 skips the passes).
  - Composite: scene plus bloom, tone-mapped per channel: unchanged up to 0.85, then
    Reinhard-style toward 1 (a first exponential roll-off made everything above about
    1.2 flat white, which the test caught). Ordinary 2D colours look as before. The
    3D fire now leaves its glow unclamped, so the composite blooms and rolls it off.
  - GPU timings: timestamp markers (empty compute passes) between the frame's stages
    give the scene, glow, composite and interface times without touching the
    renderers; the Info tab shows them. At 2160×1140, full scale, with the 3D fire:
    scene 0.22 ms, glow 0.06 ms, composite and interface 0.01 ms each.
  - Tests (headless GPU): the composite leaves 0.5 unchanged and rolls 4.0 off below
    white; a bright dot glows onto pixels 4 px away while a dim scene doesn't glow;
    timings arrive for every stage; the automatic render scale.
  - Checked in the app: 2D scenes as before; the 3D fire with a warm halo and a bright
    core that keeps its shading.
  - Found while checking: the 3D fire went wrong after a minute or so, which the
    shorter tests never saw. Fixed in the solver (see the 3D plan's Phase E notes): open
    walls, heat and stray fuel.
- **Phase B (done):** feedback and warp (`app/src/render/feedback.rs`,
  `feedback.wgsl`, `flow2d.wgsl`).
  - Each frame samples the last one through the warp, fades it, and combines the new
    scene over it, in ping-pong `Rgba16Float` textures at the scene's size. Two modes:
    smear (a blend toward the scene: still things stay put, moving ones trail) and
    brightest (the brighter of old and new: bright things streak over dark ones). The
    glow and composite then work on the result.
  - The warp: zoom from the centre, rotation, drift, a wave wobble, extra zoom with
    the bass, and the fluid's flow. Samples from outside the frame are black (clamping
    smeared the edge pixels into streaks).
  - Rates are per second, scaled by the frame time (zoom and persistence as powers of
    it), so a look moves the same at 60 and 240 fps; paused, the picture holds.
  - The flow: a screen-space texture of uv per second. 2D grid solvers provide their
    velocity (new `Solver::velocity_field`, stable fluids and LBM); particle solvers'
    velocities are averaged onto a 48-cell-high grid; a pass maps either through the
    view. In 3D the raymarch writes it as a second output: the visible gas's velocity
    (smoke by opacity, flames by brightness), averaged along the ray, projected onto the
    screen and scaled by the pixel's coverage. The solver now exports its velocity to a
    3D texture alongside the smoke.
  - Echo: the composite mixes in a zoomed, optionally flipped copy of the picture
    (not fed back).
  - Settings: a Feedback group in the View tab (mode, trail, flow, zoom, turn, drift,
    wobble, bass zoom, echo), saved with the others; every look carries its own (Ink
    storm swirls and smears with the flow, Firestorm and Bonfire 3D trail upward,
    Inferno turns its torch into a spiral tunnel, Mercury rain and Smoke dance echo,
    Lava lamp wobbles). With feedback on, the 2D letterbox is black and the 3D box's
    lines are hidden, since feedback would carry them into the picture.
  - Measured at 2160×1140 with Inferno: feedback 0.03 ms of GPU time per frame.
  - Tests: rates are frame-rate independent and a paused frame holds; the bass adds
    zoom; particle velocities average per cell; on a headless GPU, brightest mode keeps
    a dot that left the scene while smear fades it, and zoom streaks a dot outward.
  - Checked in the app, full screen: the looks above. The 2D looks still carry a little
    of their grey walls into the picture where the flow or drift crosses them; tuning
    per look belongs with the presets (phase D).
  - Not yet: the trails' character depends on the scene's brightness against its
    background; brightest mode with long trails lets dim smoke pile up (the looks use
    0.5–0.6 left per second, close to MilkDrop's usual fade).
- **Phase C (done):** waves and shapes (`app/src/render/waves.rs`, `waves.wgsl`), drawn
  from analyzer phase B's outputs.
  - Wave modes: a line scope, a circular scope (its seam blended shut), spectrum bars
    along the bottom, mirrored bars about a centre line, and left against right (a
    goniometer, turned 45° so mono is vertical and width spreads it). Up to two shapes
    per look: polygons that spin, pulse with MilkDrop's `bass_att`, and are outlined
    and optionally filled.
  - Built on the CPU each frame as triangles (a quad per line segment) in canvas units,
    and added onto the scene texture at a brightness above white, before the feedback:
    the feedback streaks them, and the bloom makes them glow. With feedback off they
    simply sit on the scene.
  - The waveform is scaled by its own recent peak (rising at once, falling over 2 s),
    so quiet and loud music fill the shapes alike; below 0.02 it isn't scaled up, so
    silence stays flat.
  - Pushes: a line or circle can push the fluid along its path, through 12 brushes
    pushing away from the wave's rest position as far as the sound has moved it. In 2D
    they go into the step's brushes; in 3D through the reactions' front-view lift.
  - Settings: a Waves group in the View tab (mode, swing, size, height, line width,
    push, two shapes, brightness), saved; each look carries its own. Ocean pulse has a
    line, Ink storm a pushing circle, Lava lamp a slow hexagon, Mercury rain the
    left-against-right figure, Inferno a ring that the feedback stacks into a tunnel,
    Smoke dance spectrum bars and a spinning triangle.
  - Found while checking: a look's feedback lingered into the next (Inferno's swirls
    were still visible 7 s into Smoke dance), so a look change now restarts the
    feedback from the scene, under the director's fade.
  - Tests: nothing drawn when off; the circle is a closed ring within its swing; mono
    is vertical and opposite channels horizontal in the left-against-right view; bars
    follow the spectrum; shapes fill, outline and pulse with the bass; quiet waves fill
    like loud ones but silence stays flat; a pushing circle pushes outward from its
    ring.
  - Checked in the app, full screen, with a generated stereo test track (muted): Ink
    storm's ring tangles into glowing filaments in the ink, Inferno's ring becomes a
    tunnel of rings, Smoke dance's bars run along the bottom (its echo mirrors them at
    the top), Mercury rain's figure shows the track's slight width.
- **Phase D (done):** looks as data (`app/src/expr/`, `app/src/looks/`, `app/looks/`).
  - Naming: the files are called looks, as the UI calls them, in a `looks` folder:
    "presets" already means scenes in the simulation crates.
  - The equation language (`expr`): MilkDrop-like assignments with `+ - * / %`,
    comparisons, `&&`, `||`, `!`, `if(c, a, b)`, 26 maths functions, `rand()` per frame
    only, and `//` comments. The names, tokenizer, Pratt parser and name resolution take
    about 570 lines.
    - Maths is safe: dividing by 0 gives 0, and `sqrt`, `log`, `pow`, `asin` and `acos`
      keep to their domains. Each function has a WGSL twin in `math.wgsl`.
    - Names: the inputs (clock, MilkDrop's levels, bands, loudness, hit and beat
      pulses, beat and bar phase, tempo) are read-only. The motion (`zoom`, `rot`, `dx`,
      `dy`, `cx`, `cy`, `sx`, `sy`, `warp`) is shared by both stages. Frame-only outputs
      cover the rest of the feedback, the waves and shapes, gravity tilt and reaction
      intensity, plus `param_<key>` for solver parameters. `q1` to `q8` go from frame
      to pixel.
    - A name that's read but never assigned is an error, with the closest known name
      suggested. Errors carry line and column.
  - Two back ends:
    - A stack-machine interpreter for per-frame code, allocation-free per run.
    - A WGSL generator for per-pixel code, spliced into the feedback shader (now a
      template) and validated by naga when the look loads.
    - A headless-GPU test checks that the generated code computes what the interpreter
      does, on 64 random inputs for each of five programs that use every function.
  - Changed from §4.3: the warp stays per second (zoom per second, radians and screen
    heights per second), as the View tab's settings already were. The shader applies
    the frame time, so an equation behaves the same at any frame rate. The shader also
    replaces any non-finite motion with a safe value, since a NaN would stay in the
    feedback picture for good.
  - Changed from §4.2: one per-frame program drives everything, waves and shapes
    included, rather than separate equation sections per wave and shape. `init` runs
    once and shares its variables with `frame`.
  - Files (TOML): `[meta]`, `[sim]`, `[feedback]`, `[waves]`, `[[shape]]`, `[code]`.
    - A missing setting takes its default. An unknown key, a solver, scene, palette,
      trigger or effect that doesn't exist, or an equation that doesn't compile is an
      error with its line. A typo that's quietly ignored is worse than one that's
      reported.
    - The format is documented in `crates/app/looks/README.md`.
  - The nine built-in looks are now files, compiled in. A test (since removed, with the
    old table) checked each loads exactly as the Rust table had it. Three gained
    equations:
    - Inferno twists its tunnel into spirals (per pixel: the outer rings turn faster)
      and zooms on each beat.
    - Ink storm has a slow three-petalled wobble (per pixel) and a turn that swings with
      the music.
    - Lava lamp's wobble breathes, and its hexagon spins up on beats.
  - User looks: `%APPDATA%/fluid-toy/looks`, read at start and checked every second.
    - A look named like a built-in replaces it, and a file that doesn't load is listed
      and skipped.
    - When the look on screen changes on disk, it takes the change at once. Only a
      change to its solver, scene, parameters or liquid restarts the scene.
    - The director follows its looks by name across reloads.
    - The Music tab's Looks group has buttons to open the folder and to copy the
      built-ins into it, and lists files that didn't load with their errors. A user look
      shows a pencil in the list. Load errors also show as a notice.
  - In the app: equation-set parameters combine with the reactions' multiples on the
    same remembered base value; gravity tilt turns the reactions' base gravity; a shape's
    spin now adds up per frame (so an equation can vary it smoothly).
  - Deferred:
    - Looks with no fluid at all (decision 5) need a scene-less render path; they fit
      with the pure-feedback looks in phase I.
    - The `[composite]` section belongs to phase G.
    - MilkDrop's per-point custom waves are left for later.
  - Tests:
    - Language: precedence, statement splitting, names by kind, errors with positions
      and suggestions, number forms, arithmetic and guarded functions, state kept
      between frames.
    - WGSL: the generated code validates, and the GPU agrees with the interpreter.
    - Feedback: the template validates with and without per-pixel code. The settings'
      motion fills its slots. The fade is frame-rate independent, and the GPU test still
      streaks a zoomed dot outward.
    - Files: defaults, sections turning things on, friendly reaction names, and mistakes
      reported with their lines.
    - Runtime: settings pass through without code; equations drive the motion, outputs
      and parameters; `init` runs once; pulses fall; bad values fall back.
    - Library: built-ins load and refer to real things (2D and 3D). A folder adds,
      replaces and reloads, and its broken file is reported at its line.
    - Director: it re-indexes by name.
  - Checked in the app, full screen, with a generated beat, muted, and `APPDATA` pointed
    at a scratch folder:
    - Inferno's spirals.
    - Ink storm as before, with its wobble.
    - Editing Inferno's per-pixel line in the scratch folder showed within six seconds,
      without restarting the fire.
    - The Music tab's Looks group with a broken file listed (`broken.toml, line 10:
      unknown name 'bas', did you mean 'bass'?`). Its first layout was too wide, so the
      buttons became icons.
  - Seen in passing, and the same before this phase: Lava lamp starts dim, and its
    hexagon (a thin outline in the palette's dark red) hardly shows. For tuning in
    phase I.
- **Added after phase D:** saving the settings as a look (the Music tab's "Save as").
  - What's saved: the fluid (solver, the scene as last loaded, every parameter at its
    base value even while a reaction moves it, toggles as 0 or 1), the colours, feedback,
    waves and reactions, and the equations and energy of the look on screen.
  - A palette can be its colours in the file, so one edited in the View tab saves too.
  - Files leave out settings at their defaults. A test writes every built-in out and reads
    it back unchanged.
  - Saving over one of the user's looks writes its file; otherwise a new file is named
    after the look, never over another's. The saved look becomes the one on screen.
  - Not checked in the app: posted clicks and typing don't reach egui's text field, so
    gathering the app's state for a save is checked only by reading the code. The
    library's saving and loading are tested.
- **Phase E (done):** audio upgrades.
  - Already done by the analyzer plan: MilkDrop's `bass`, `mid`, `treb` and `_att`
    (its phase B), and tempo, beats, bars, the latency offset and the calibration
    flash (its phase E). The equations read them all since phase D.
  - Sections (`app/src/audio/sections.rs`), from the analyzer's filter bank in quarter-
    second blocks: timbre (16 band groups, relative to their mean), loudness, low end
    and onset density.
    - Novelty: the last 2 s against the 8 s before. Each group's and the loudness's change
      is measured against how much it varies anyway over the long window (a
      t-statistic): steady pads make small changes count, busy drums make fills count
      less.
    - Changes: novelty peaks of 3 or more, 4 s apart. A fall in loudness or low end makes
      it a breakdown, anything else a plain change. Plain changes aren't reported while
      the music builds up.
    - Drops: loudness and low end jumping together, with the low end a large share of the
      mix. They're reported as soon as they show, rather than at a novelty peak (novelty
      is already high through a build-up).
    - Build-up: loudness and onset density rising along a line over the last 8 s, each
      scaled to a full rate and weighted by how well the line fits.
    - Nothing is compared across silence, so a new song isn't a section change. The first
      second (the analyzer warming up) is left out.
  - Measured on generated songs only: there's no openly licensed audio with section
    annotations.
    - A new 72 s song, "anthem 128", has an intro, a build-up (a snare roll speeding up
      to 32nds, a riser), a drop, a breakdown, another build-up and a drop. All five of
      its changes are found, of the right kind, 0.15 to 0.6 s after them (reported up
      to 2 s later).
    - The dynamics song's breakdown is found. Nothing is found in the five loops or the
      six drum grooves, which never change.
    - Build-up reads 0.47 through the anthem's build-ups and 0.02 elsewhere.
    - The first version found a change about 8 s into almost every clip (the warm-up)
      and fills in the drum grooves; self-normalised novelty and the warm-up rule fixed
      both.
    - Real music will be the test: the thresholds are set on generated songs.
  - The director (changing looks with the music) now follows sections instead of a jump
    in loudness:
    - A drop changes at once to an energetic look, and a breakdown to a calm one.
    - A plain change makes a change due, once half the interval has passed. Sections need
      15 s of a look first.
  - Changes land on bar lines: with a steady beat, a change the director decides on
    (timer, breakdown, plain change, refreshing a locked look) starts its fade-out over
    what's left of the bar, so the new look arrives on the downbeat. The fade shortens to
    the time left, down to 0.15 s. Without a beat it waits for a kick, as before. Drops
    don't wait. Hard cuts belong to phase F.
  - Motion locked to the beat:
    - A Beat trigger for reactions fires on every beat of a steady beat, even a quiet one.
    - Equations read `novelty`, `buildup`, and `section`, `drop` and `breakdown` as pulses
      (1 at a change, falling over about 1.5 s).
    - Smoke dance sways one way and back over each bar while there's a beat. Wind tunnel
      gets a vortex on each beat.
  - Tests:
    - Section scoring in the analyzer's baseline (12 clips), and the build-up test.
    - Director: a drop changes at once to an energetic look, a breakdown to a calm one, and
      a plain change only past half the interval. Its changes switch within 1.5 frames of
      a bar line at 90, 128 and 174 BPM.
    - The Beat trigger fires and pulses. Equations get section pulses.
  - Checked in the app, full screen, with the anthem (muted, in a scratch settings
    folder): looks changed 2.3 s after the first drop, 3.3 s after the breakdown (reported
    2 s late, then waiting for the bar line) and 2.5 s after the second drop, and nowhere
    else. The log's whole seconds can't show the bar line itself: the tests check that.
- **Phase G (done):** composite and colour (`app/src/render/composite.rs`,
  `post.wgsl`).
  - The composite (drawn on the screen, never fed back) gained:
    - Symmetry: mirror left-right, top-bottom or four ways, or an N-fold kaleidoscope
      (2 to 16 mirrored wedges) that can turn. It folds where the picture is read from,
      so the bloom and echo fold with it. A kaleidoscope reaching past the edges reads
      the picture reflected, not its edge pixels smeared.
    - Grading, after tone mapping (where the numbers mean what they look like): a hue turn
      (round the grey axis) and drift, saturation, contrast (about mid-grey in a roughly
      perceptual space), exposure.
    - Palette mapping: the picture's brightness onto the look's palette (black, deep,
      base, highlight), mixed in by an amount.
    - Invert, chromatic aberration (red pushed out, blue in), vignette, film grain.
    - Each step leaves the picture exactly as it was at its default, so looks without a
      composite are unchanged (the existing composite test still passes).
  - Settings: a Composite group in the View tab (saved), a `[composite]` section in look
    files (checked for unknown keys, written by Save as), and equation outputs named as
    the settings (`hue`, `vignette`...). The kaleidoscope's turn and the hue's drift add
    up per frame, and start afresh with each look: the first in-app check showed a
    look's red palette gone green, from the previous look's drift carried over.
  - Changed from §5.3: the Flash and Hue reactions stay on the fluid's palette. They
    colour the fluid itself, which the feedback then carries, and moving them would
    change every look; the composite has its own hue and exposure for equations.
    Changed from §4.2: no raw WGSL in `[composite]` yet (decision 4 placed it late).
  - Looks:
    - Inferno is a turning six-wedge kaleidoscope of its spirals.
    - Mercury rain is silver film (low saturation, contrast, vignette, grain).
    - Lava lamp is brighter (exposure 2, partly mapped onto its palette) with a vignette
      and fringes: it was dim.
    - Ink storm drifts in hue.
    - Smoke dance is mirrored four ways and partly palette-mapped.
  - Tests:
    - GPU: off leaves colours unchanged. The mirror puts a dot at its mirror position.
      The kaleidoscope repeats every 60° and mirrors within a wedge. Saturation 0 is
      grey, a 120° hue turn takes red to green, invert takes 0.2 to 0.8. The vignette
      darkens corners, not the centre. The palette map takes black to black and white
      to the highlight.
    - Equations drive the composite, in range. Every built-in survives the round trip
      with its composite.
  - Checked in the app, full screen, with the long test beat: the five looks above
    (Inferno's mandala in fire colours, Mercury's silver, Lava lamp's red blobs).
- **Phase I (done):** library and interface.
  - Looks without fluid: a look file may leave out `solver` and `scene`. The 2D world
    then waits underneath, neither stepped nor drawn: each frame starts from black and
    the picture is only the waves and shapes, carried by the feedback. Reactions still
    flash and turn the palette. Picking a solver or a scene brings the fluid back.
  - 32 built-in looks (23 new), with tags:
    - 12 without fluid: Tunnel vision, Spiral galaxy, Starfield, Neon scope, Bass bloom,
      Ripple pool, Prism, Heartbeat, Mandala, Vortex drain, Aurora, Strobe lattice. Most
      have equations (a wandering tunnel centre, spiral arms, rings spreading from the
      middle, a monitor trace, a strobe that inverts on the beat).
    - 11 more with fluid: Deep current, Fountain of light, Quicksilver, Slime pit, Dye
      tornado, Ember drift, Karman street, Ink garden (2D); Ghost smoke, Twin flames,
      Blue torch (3D). 3D fire takes its colours from its heat, not the palette, so Blue
      torch is turned blue by the composite's hue.
    - The list alternates fluid and no-fluid looks, so the director's rotation does too.
  - Ratings, favourites, history, hard cuts (moved here from phase F):
    - Stars (1 to 5; unrated counts as 3) and favourites per look, saved in the
      settings by name.
    - When the director changes looks by itself it skips one-star looks, and with
      "favourites only" keeps to favourites (if any suit the music; otherwise any look).
    - It remembers the looks shown (50), for going back, and can cut without a fade.
  - Hotkeys, in the shortcut table (cheat sheet and Info tab):
    - In full screen MilkDrop's take over the keys they share: Space is the next look
      (the director's pick, faded), H the same as a hard cut, L locks the look. Outside
      full screen Space, H and L keep pausing and hiding particles and the liquid.
    - Everywhere: Backspace goes back a look, + and - rate it (shown as stars), T shows
      the song's title (the track's name, or the device listened to live), B the
      browser, E the editor. `,` and `.` stay.
  - The look browser (B, the grid button in the Looks group and the full-screen bar):
    - A card per look: thumbnail, name (a pencil for the user's own), clickable stars
      and heart. Search over names and tags, tag chips (all must match), a favourites
      filter, and the director's favourites-only switch.
    - Thumbnails come from the picture itself, since a fluid can't be run ahead in the
      background: 8 s into a look, while music plays, the composite is drawn a second
      time into a small texture, read back over the next frames without waiting, the
      canvas cropped to 16:9 and averaged down to 192x108. They're cached as PNGs beside
      the settings, named after the look and a fingerprint of its file, so an edited
      look gets a new one (the old is removed). Until then a card shows the palette and
      what the look is.
  - The equation editor (E, or the function button):
    - The look on screen's init, per-frame and per-pixel programs in code boxes. Each
      keystroke that leaves them compiling takes effect at once: the clock, the beat
      pulses, the q's and the variables both versions share carry on (`init` runs again
      only if it was edited).
    - A mistake is underlined (the word at its place, or the first line for a shader
      error) with the message below, while the look keeps running its last good code.
    - Save writes the look as Save as does, under its name (a built-in becomes the
      user's copy); Revert goes back to the code as loaded or last saved.
    - Each per-pixel edit builds a shader: the feedback keeps at most 32.
  - Display, in a Screen group of the View tab (saved):
    - The monitor for full screen; the visualizer in a window instead, optionally kept
      on top; a frame-rate cap (30, 60, 120, 144 fps), waiting between frames rather
      than drawing as fast as possible. The render scale was already there.
  - Tests:
    - Looks without fluid load, round-trip, and need both or neither of solver and
      scene. Every built-in loads and refers to things that exist.
    - Code swapped while running keeps its clock and variables, and reruns `init` only
      when asked. Editor errors underline the right word.
    - The director keeps to eligible looks while any suit, goes back through the looks
      shown and cuts without a fade. Ratings and favourites survive the settings.
    - In full screen the visualizer's keys win; every key is unique per context.
    - The browser's filter searches names and tags. Thumbnails are stored, replace the
      old one and load back; the shrink crops to 16:9 and swaps blue-first bytes.
  - Checked in the app (release, a scratch settings folder, a 7.5-minute mono test beat,
    muted), full screen:
    - All 32 looks, 10 s each: no errors, and 32 thumbnails written. A first run
      outlasted its 90 s track, and the looks after that were dark: in silence the
      waves are flat. Thumbnails now wait for sound.
    - Toned down from the first run: Starfield, Prism and Neon scope (blown out to
      white), Tunnel vision and Quicksilver (washed out). Brightened: Aurora and Ripple
      pool (brightest instead of smear). Ink garden moved to the double dam break (its
      dye barely showed in the tank). Blue torch was orange before its hue turn.
    - The browser: cards with thumbnails in rows (a wrapping layout first widened the
      window to the screen), tags and filters. The editor shows the look's code.
    - + showed the stars; T the track; Space faded to the director's pick; Backspace
      returned.
    - Not checked in the app: typing in the editor (posted keys don't reach egui text
      fields; the tests cover compiling and swapping), the monitor choice (one monitor),
      the windowed visualizer, and the frame-rate cap.

