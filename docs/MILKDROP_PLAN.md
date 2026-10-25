# MilkDrop-Style Visualizer Plan

Status: **approved; phases A to C done.** Decisions (§9) taken as recommended (2026-10-25):
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
