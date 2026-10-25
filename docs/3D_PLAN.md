# 3D Simulation Plan

Status: **approved; phases A to E done.** Decisions (§8), taken as recommended
(2026-10-24):
- 128³ with automatic step-down;
- a separate `World3d` with an app mode switch;
- left drag orbits, right drag pushes;
- emission and absorption first, self-shadowing later;
- analytic obstacles only;
- 3D now, before the visualizer's phases F and G.

Whether 3D liquid follows (decision 6) is open for later.

## 1. Goal

Add real-time 3D fluids alongside the existing 2D ones, starting with **smoke and
fire**:
- a 3D grid solver running entirely on the GPU;
- drawn by raymarching;
- viewed through an orbit camera;
- driven by the same music reactions and director as the 2D looks.

3D liquid comes later, as a separate step (§7, phase G).

Targets:
- **Speed:** 128³ cells at 60 fps on the RTX 4090. Weaker GPUs drop resolution
  automatically (96³, 64³) through the existing governor.
- **Fit with what exists:** the 2D solvers, UI, visualizer and tests keep working
  unchanged. 3D is a mode, not a rewrite.

## 2. Why this is a big step

| | 2D today | 3D smoke and fire |
|---|---|---|
| Cells | 256×128 ≈ 33k | 128³ ≈ 2.1M (about 60×) |
| Where it runs | CPU (rayon); one GPU solver | GPU only: CPU would take seconds per frame |
| Data path | CPU arrays, uploaded for drawing | Stays on the GPU: the solver and renderer share textures, nothing is read back |
| Drawing | Flat view, field textures, liquid splats | Raymarching through a volume; camera |
| Interaction | Brushes, painted walls, tool strip | 3D brushes on a plane in front of the camera; walls as simple shapes |

The existing GPU solver (`sim-gpu`, PBF) creates its own headless device and reads
particles back to the CPU every substep. That's fine for 2D particles, but a 3D volume
must stay on the GPU and share the renderer's device. That's the main architectural
change.

## 3. Architecture

### 3.1 Crates and interfaces

- **New crate `sim3d`** (wgpu compute, no winit or egui, like `sim-gpu`):
  - `Scene3d`:
    - a box domain (e.g. 1 × 1.5 × 1 m, y up) and gravity;
    - burners (3D emitters with position, direction, radius and speed);
    - vents (open boundaries);
    - solids as simple analytic shapes (spheres, boxes, cylinders), voxelised on the
      GPU.
  - `Solver3d` trait, deliberately parallel to `sim::Solver`:
    - `reset`;
    - `step(encoder, dt, input)`, recording GPU passes rather than running on the CPU;
    - `visit_params(&mut dyn sim::ParamVisitor)`, reusing the 2D parameter specs, so
      the Simulate tab works unchanged;
    - `stats()` (same `Stats`, so metrics and health dots work);
    - `volumes() -> VolumeViews`: texture views the renderer samples directly.
  - `World3d`: scene, solver, stepper and time; the 3D counterpart of `sim::World`.
- **Shared GPU context:** solvers take the app's `wgpu::Device` and `Queue` instead
  of creating their own. `sim-gpu`'s `GpuContext` gets a constructor from an existing
  device, and its headless constructor stays for tests and the bench.
- **App:** a `SimMode` switch between the 2D `World` and `World3d`.
  - The Solver dropdown lists 3D solvers with a "3D" tag; picking one switches mode.
    This is decision 2: separate worlds **(?)**.
  - The renderer gets a 3D path, and the 2D path stays as it is.

### 3.2 Data on the GPU

- **Velocity:** a staggered (MAC) grid: three 3D textures (u: (n+1)·n·n, v and w
  likewise). The staggering avoids the checkerboard pressure modes of a collocated
  grid, as in 2D.
- **Scalars:** pressure, divergence, smoke, temperature, fuel; `r16float` or `r32float`
  3D textures, ping-ponged.
- **Formats:**
  - *Reading:* passes read through sampled textures, so advection and raymarching get
    hardware trilinear filtering.
  - *Writing:* passes write write-only storage textures.

  This sidesteps uneven read-write storage-texture support across GPUs (and later
  WebGPU).
- **Memory at 128³:** about 2.1M cells × ~60 bytes (fields and ping-pong copies)
  ≈ 250 MB. Fine on the 4090; the governor's lower resolutions cut it 2–8×.

### 3.3 The solver's passes (per substep)

The same algorithm as the 2D stable-fluids and fire code, in compute shaders:

1. **Sources:** burners write fuel, temperature and velocity into their cells. Music
   reactions add brushes (spheres with push, spin and puff).
2. **Forces:**
   - Boussinesq buoyancy: hot gas rises against gravity, smoke is slightly heavy;
   - 3D vorticity confinement (curl, its gradient, cross product), which matters even
     more in 3D, where swirls are the whole look.
3. **Advect velocity:** semi-Lagrangian, RK2 backtrace, trilinear sampling.
4. **Project:**
   - divergence;
   - warm-started red-black Gauss–Seidel pressure, about 40 iterations;
   - subtract the gradient;
   - walls (Neumann) and vents (Dirichlet, p = 0).

   2D found warm-started Gauss–Seidel fast for smoke. If it's too slow in 3D, a GPU
   multigrid V-cycle comes next (phase F).
5. **Advect scalars:** MacCormack with the min/max limiter, as in 2D.
6. **Combustion:** the 2D model ported:
   - fuel burns above ignition and releases heat and soot;
   - cooling as dT/dt = −k·T², implicit;
   - a little heat diffusion;
   - flickering burners.

**Cost estimate:** each pass is bandwidth-bound, about 2.1M cells × ~32 bytes ≈ 70 MB,
or roughly 0.07 ms on a ~1 TB/s card. With about 90 passes per step (mostly pressure
iterations), that's 6–8 ms at 128³: inside a 16 ms frame. To be measured in phase B
with GPU timestamp queries.

## 4. Rendering

- **Raymarching:**
  - A full-screen pass intersects each camera ray with the domain box and marches
    front to back, about half a voxel per step.
  - Smoke absorbs, using a light grey with the palette's tint. Fire emits, through the
    same blackbody ramp as 2D's `FieldKind::Fire`, done in the shader.
  - A per-pixel jitter on the start position hides banding.
  - Rays stop early once nearly opaque.
- **Lighting**, in two stages:
  1. emission, absorption and ambient light (phase C);
  2. self-shadowing smoke: a light-transmittance volume computed once per frame from a
     directional light (one extra march per voxel column), which gives smoke its
     sculpted look (phase F) **(?)**.
- **Scene dressing:**
  - a floor plane and the box edges, faint;
  - solids drawn as shaded analytic shapes by the same raymarcher;
  - the background stays the dark canvas colour.
- **Camera:**
  - orbit: yaw and pitch around the domain centre, wheel to zoom;
  - in the visualizer, a slow automatic orbit, optionally nudged by the music.
- **Visualizer polish:** the bloom planned for visualizer phase F applies here too:
  fire benefits most.

## 5. Interaction and the visualizer

- **Mouse in 3D** **(?)**: left drag orbits, right drag pushes the smoke on a plane
  through the domain centre facing the camera, wheel zooms. The tool strip shows the
  3D tools only: push, puff (a burst of smoke), torch (a burner at the cursor).
  Painting walls isn't supported in 3D.
- **Reactions:** the existing effects map onto 3D, so reactions lists and looks keep
  their meaning:

  | 2D effect | In 3D |
  |---|---|
  | Splash | Upward push from a floor patch |
  | Jet | A burner burst (a fuel puff, for fire) |
  | Vortex | A swirl about a random vertical axis, or a smoke ring |
  | Flash, Hue | Brighten or shift the smoke tint (fire stays blackbody) |
  | Gust | Horizontal push across the volume, slowly turning |
  | Slosh | Gravity tilting, about the camera's view axis |
  | Breathe | Gravity strength |
  | Param | Any 3D solver parameter by key |

- **Looks:** new 3D looks for the director (e.g. "Bonfire 3D", "Smoke rings",
  "Inferno"), each with a scene, palette, camera orbit speed and reactions. The
  director mixes 2D and 3D looks, and a look switch can cross modes.
- **Scenes:**
  - *Campfire 3D:* a burner on the floor, an open top.
  - *Torch:* a narrow, fast burner.
  - *Twin burners around a sphere.*
  - *Chimney:* a narrow vent above a broad burner.

## 6. Testing

GPU tests run on a headless device and skip with a note when there's no adapter, as
`sim-gpu`'s do:
- **Projection:** after one step from random velocities, the largest divergence
  residual is below tolerance, with walls and vents.
- **Advection:** with no sources, smoke mass is conserved within a small error, since
  MacCormack's limiter isn't exactly conservative.
- **Plumes:**
  - a hot blob's centroid rises;
  - with zero gravity it doesn't;
  - with tilted gravity it leans.
- **Symmetry:** a centred burner gives a plume symmetric about its axis, to tolerance.
  This catches staggered-grid indexing bugs, the classic 3D mistake.
- **Fire:** the burner keeps burning, and with no fuel nothing burns (as the 2D tests).
- **CPU reference:** a small CPU version of the projection on a 16³ grid, compared
  value by value with the GPU result, to verify the shader's stencils.
- **Performance:** the bench runs 3D solvers with per-pass GPU timings (timestamp
  queries), and the soak test gains a 3D look.

## 7. Phases

Each phase leaves the app working and is committed on its own.

| Phase | Scope | Visible result |
|---|---|---|
| **A. Foundation** | Shared-device GPU context; `sim3d` crate with `Scene3d`, `Solver3d`, `World3d`; app mode switch; empty 3D view with the orbit camera and box outline. | Pick "3D" and orbit an empty box. |
| **B. GPU grid core** | MAC textures; advection; divergence; red-black Gauss–Seidel pressure; walls and vents; burner smoke with buoyancy; projection and plume tests; GPU timing. | (Seen once C lands.) Tests show plumes rising. |
| **C. Raymarcher** | Emission and absorption raymarch, jitter, early exit, floor and edges; smoke visible. | A rising smoke column you can orbit. |
| **D. Fire** | Combustion port (fuel, T² cooling, flicker), blackbody emission, 3D vorticity confinement, Campfire 3D and Torch scenes. | Real-time 3D fire. |
| **E. Interaction and visualizer** | 3D brushes and tools, reactions mapping, 3D looks, auto-orbit, director mixing 2D and 3D, governor resolution steps. | A full-screen 3D fire that dances to Spotify. |
| **F. Polish** | Self-shadowing light volume; obstacles; multigrid pressure if needed; bloom (shared with the visualizer). | It looks finished. |
| **G. 3D liquid (later)** | GPU PBF in 3D (extending the existing GPU PBF: neighbour grids in 3D, no CPU readback), then a screen-space liquid surface (depth and thickness splats, smoothing, shading). FLIP 3D would need fixed-point atomics for particle-to-grid, so PBF comes first. | 3D water. |

A–D get 3D fire on screen; E makes it part of the visualizer.

### Progress

- **Phase A (done):**
  - `GpuContext::shared` wraps the renderer's device and queue, so 3D solvers run on
    the GPU that draws (no second device, no readback).
  - The `sim3d` crate: `Scene3d` (domain, gravity, burners, vents, analytic solids)
    with four presets (Campfire 3D, Torch, Twin burners, Chimney); the `Solver3d`
    trait, which records compute passes into the caller's encoder; `World3d`; and a
    placeholder "3D preview (empty box)" solver.
  - The app: the solver menu lists the 3D solvers (with a cube icon) under the 2D
    ones. Picking one swaps in a `World3d`, and the 2D world waits underneath until a
    2D solver is picked again. N cycles through both families, and P goes through the
    3D scenes.
  - The 3D view (`app/src/render3d`): an orbit camera (left drag orbits, the wheel
    zooms, pitch is clamped short of the poles) and the scene in outline: domain box,
    floor grid, vents, burner rings, solids. The tool strip, legend and brush hide in
    3D, and the status bar, Simulate, View and Info tabs follow the 3D world.
  - Tests: the camera centres its target, keeps up as up and fits the whole box;
    outlines are whole segments; left drags orbit while right drags and drags begun on
    the panel don't; wheel steps (lines or touchpad pixels). A GPU test steps a world
    and switches scenes.
  - Checked in the app: all four scenes in outline, the wheel zoom, and switching to
    3D and back. Orbiting couldn't be driven from the test harness (synthetic mouse
    moves get a mouse-leave when the real cursor is elsewhere), so the unit test
    covers it.
- **Phase B (done):** `sim3d::Smoke3d` ("Smoke (3D GPU)", `smoke.rs` and
  `smoke.wgsl`) replaces the empty preview solver.
  - Each substep runs five timed passes: sources and forces (cell types, prescribed
    faces, burner gas, Boussinesq buoyancy); velocity advection (RK2
    semi-Lagrangian); pressure (divergence, then 40 warm-started red-black SOR
    sweeps); projection; smoke and heat advection (MacCormack with the limiter, T²
    cooling, fade). Substeps are at most 1/60 s.
  - Burners are inflow cells whose faces carry the exit velocity (as the 2D
    nozzles). Vents hold φ = 0, so gas reaching them leaves. Solids are classified
    from the analytic shapes on the GPU each substep.
  - Changed from §3.2: the fields are storage buffers sampled trilinearly in the
    shader, not 3D textures. That needs no filterable float textures, and with
    velocity in one buffer and (pressure, divergence) and (smoke, temperature) packed
    as pairs it fits wgpu's default eight storage buffers per stage. The raymarcher
    can copy the smoke into a 3D texture.
  - Pass timings come from timestamp queries (both devices now request
    `TIMESTAMP_QUERY` where supported), read back a frame or two late without
    stalling, and show in the Info tab's step breakdown.
  - Measured on the RTX 4090, GPU time per substep, with pressure about 85% of it:

    | Grid | Cells | GPU per substep |
    |---|---|---|
    | 85×128×85 | 0.92M | 1.2 ms |
    | 107×160×107 (default) | 1.83M | 2.3 ms |
    | 128×192×128 | 3.15M | 4.4 ms |

    That's well under the 6–8 ms estimated for 128³, so the default is 160 cells
    high (close to the 128³ budget). In the app it runs at the display's 240 fps.
  - Tests (headless GPU, skipped without an adapter): a hot blob rises, stays put
    without gravity and leans under tilted gravity; smoke is conserved within 3% in
    a closed box; projecting random velocities (walls, vent, solid ball, burners)
    leaves under 0.1% of the divergence; the classification, prescribed faces,
    pressure and projected velocities match a CPU version value by value (16×24×16,
    chimney scene); a centred plume is symmetric under swapping x and z (exactly)
    and mirroring x (closely). An ignored test prints the pass timings.
  - Not yet: brushes (`StepInput3d::brushes`) wait for phase E; the extra burners in
    `StepInput3d::burners` already work. Nothing draws the smoke until phase C.
- **Phase C (done):** the smoke is drawn (`app/src/render3d/march.wgsl`).
  - After each step the solver copies (smoke, temperature, solid) into an
    `Rgba16Float` 3D texture (`Solver3d::volume`), which the renderer samples with
    hardware filtering.
  - The lines (floor grid, box, burners, vents) are drawn first and write depth. A
    full-screen pass over the canvas then marches each pixel's ray front to back
    through the box, three quarters of a cell per step, from a per-pixel jittered
    start. It stops at the lines' depth, at the nearest solid, or once 99% opaque,
    and blends over the lines premultiplied.
  - Solids are shaded where the rays hit them (spheres and boxes, intersected
    analytically), so they're no longer outlines.
  - Lighting: a directional light from above and to the side. Each sample's light
    comes from a two-tap probe toward it, through smoke and solids (solid cells are
    marked in the texture), with stronger extinction than the view's so that even
    thin smoke shows its form. It's a stand-in for the self-shadowing light volume of
    phase F. Lit smoke is the palette's highlight paled toward white; shadowed smoke
    its deep colour, dimmed.
  - Found on the way: the vents stalled the flow. Vent cells held φ = 0 but their
    own velocity stayed at rest (semi-Lagrangian advection can't carry motion into a
    region at rest), so the plume turned and spread in a thin sheet under the vent,
    which looked like a solid lid at a grazing angle. Vent cells are now ordinary
    fluid, and the domain's wall beside them is open with φ = 0 beyond it: the flow
    carries on out of the box, and air comes back in elsewhere. The CPU reference
    test follows the new rule.
  - Checked in the app at 240 fps: Campfire 3D's column leaving through the top,
    Twin burners' plumes curling round the shaded ball, Chimney's roof shadowing the
    smoke below it and the flue carrying smoke out. Orbiting is still only
    unit-tested (synthetic drags can't be driven here); the wheel zoom works.
  - Test: the march gets every solid in the shader's encoding, a step in cells, and
    marches only with a volume.
- **Phase D (done):** 3D fire. "Fire (3D GPU)" is now the first 3D solver; "Smoke
  (3D GPU)" stays. They're the same solver with different parameters
  (`SmokeParams::fire` and `::smoke`).
  - The scalars are now (smoke, temperature, fuel) per cell. Fire burners release
    lit fuel (temperature 1, so it's already above ignition), smoke burners release
    smoke.
  - Combustion is the 2D model's, run after the scalar advection: fuel burns at
    `burn_rate` where the gas is at least `ignition` hot, releasing `heat` and
    `soot`; T² cooling as before. There's only gas fuel in 3D, so the 2D heat
    diffusion (for flames creeping into still solid fuel) isn't ported.
  - Flicker: each burner cell's fuel varies by up to ±40%, cell to cell and every
    50 ms, from a hash (reproducible), which seeds the flickering.
  - Vorticity confinement, two passes in the forces phase: the curl at each cell
    (stored in the advection's scratch buffer, free at that point, so no ninth
    storage buffer), then ε·h·(N×ω) averaged onto the free faces. It costs about
    0.25 ms per substep at the default grid (3.2 ms in total).
  - Drawing: hot gas glows through the 2D fire's ramp (deep red, orange, yellow, near
    white) between temperatures 1 and 3, as in 2D, rolled off toward white rather
    than clipped. A fire's smoke is grey soot whatever the palette, as in 2D.
  - Tuning: the 2D fire's cooling (2.5) gave short, dim flames in 3D, so the 3D
    fire uses 1.5; vorticity 0.5 for fire, 0.3 for smoke.
  - Checked in the app at about 230 to 240 fps: Campfire 3D burns with a
    yellow-white core going orange and red into a flickering tongue, under a
    turbulent soot plume; Torch is a long, twisting jet of flame.
  - Tests: a fire burner keeps burning (gas hotter than 2, soot made, fuel mostly
    burned within a metre); without fuel nothing burns (no gas hotter than the
    burner, no smoke); confinement adds swirl (more kinetic energy in a rising blob).
    The smoke-conservation test now runs without confinement, since it checks the
    advection and the extra fine swirls cost the limiter a little more (3.1% instead
    of under 3%).
  - Once, a full test run stalled (the test process spinning while it waited on the
    GPU). It didn't happen again in seven runs, and every test passes alone in under
    half a second, so it looks like a transient GPU hang; worth watching.
- **Phase E (done):** 3D in the visualizer.
  - Brushes in the solver: a pass after confinement pulls the velocity inside each
    sphere toward its push plus spin × offset, fading to the rim, at its strength per
    second (the 2D brushes' rule). They ride in the geometry buffer.
  - Mouse in 3D: left drag orbits, the wheel zooms, and right drag acts where the
    cursor meets the plane through the volume's centre facing the camera. The tool
    strip shows two 3D tools: Push, and Torch (the faucet), which holds a burner at
    the cursor pointing along the drag, up while still. (Changed from §5: the right
    button carries the tool, since the left one orbits; "puff" isn't a separate tool,
    a quick torch does it.)
  - Reactions (`app/src/react3d.rs`): the 2D engine runs unchanged on a front view of
    the volume, and its output is lifted along the camera's horizontal right.
    Splashes and gusts push the same way on screen; vortices and the idle drift spin
    about the axis toward the camera, so they read as swirls; jets become burner
    bursts on the floor (flame puffs, smoke puffs); slosh tilts gravity across the
    screen and breathe scales it; parameter reactions work by key on the 3D solver;
    hue and flash tint the smoke. As the camera orbits, the lifted pushes turn with
    it. (Changed from §5's table: vortices face the camera rather than a random
    vertical axis, and gusts turn with the orbit rather than on their own.)
  - Looks: Bonfire 3D (fire on Campfire 3D; kicks puff flames, the bass feeds the
    burner, any music), Inferno (the Torch; kick jets, snare swirls, bass leans the
    flame, loud music) and Smoke dance (smoke round the twin burners' ball, palette
    tinted, quiet music). A look naming a 3D solver is a 3D look; it carries an orbit
    speed.
  - Director: it now runs before either world steps, and `apply_look` moves between
    the 2D and 3D worlds as a look needs, so the rotation mixes them. In full screen
    the camera circles at the look's speed, faster when the music is loud, and a 3D
    look is framed 20% closer.
  - Governor: a 3D step never falls behind in simulated time (it's queued on the
    GPU), so in 3D it judges the frames' slowness instead (anything below 50 fps
    counts) and lowers `cells_y` in the same 0.8 steps. The parameter helpers now take
    either world.
  - Soak: 3D looks run on a headless GPU world with the reactions lifted as in the
    app, checked every 10 s (finite values, smoke within 0..1, no negative heat or
    fuel, nothing faster than 50 m/s; `World3d::check`). Reading 1.8M cells back every
    half second would dominate the run. The 30-minute soak passes with the 3D looks
    in: 83 look changes, all nine looks 9 or 10 times, 55 healthy 3D checks, peak
    29,040 particles.
  - Tests: brushes push and spin the gas; the health check passes a running fire and
    catches a NaN; screen points land on the camera's target plane; a right drag
    pushes along the drag on screen, or with the torch holds an upright burner; the
    lift (pushes follow the screen, swirls face the camera, jets become floor burners,
    slosh tilts across the screen); 3D looks name real scenes and parameters (with a
    GPU); 3D frame slowness.
  - Checked in the app, full screen at 4K and over 200 fps (with the soak sharing the
    GPU): stepping through the looks crosses from the 2D Mercury rain to Bonfire 3D,
    then Inferno and Smoke dance, with the camera circling. Not checked here: music
    driving the 3D looks (no audio source in this session), and the right-drag tools
    (synthetic drags are cut short), which unit tests cover.
- **Long-run fixes (found during the MilkDrop plan's phase A):** left running, the
  campfire filled with haze, its flame was blown flat along the floor, and at the app's
  4 ms steps it went out after about 30 s with the velocity blowing up. Four causes:
  - **Open walls fed back on themselves:** where air flows in through an open wall, a
    backtrace leaves the box and brings the wall face's own velocity back, so nothing
    restored it; on the fine grid it crept up step by step. Open-wall faces now copy
    the face just inside (zero gradient). Vents also open only the wall they lie flat
    against (their thinnest axis), so a vent reaching a corner doesn't open the floor.
  - **Heat collected:** the T² cooling barely touches merely warm gas, so the box
    warmed up and turned over in big convection cells. New `heat_loss` (0.3/s).
  - **Stray fuel collected** and flared up along the floor: fuel too cool to burn now
    thins out at 1/s (a burner's fuel leaves it alight, so flames lose nothing).
  - **Soot never left** where the vent didn't catch it: fire soot fades at 0.3/s.
  - A light `drag` (0.3/s for fire, 0.2 for smoke) keeps the box's slow churning weak.
  - Tried and dropped: opening the side walls (so a fire draws air from all round)
    made inflow through open walls unstable ("backflow"); a proper open-boundary
    treatment would be its own job.
  - Test (on demand, about a minute): a fire at the app's grid and step burns for 60 s
    on its burner (within 10 cm), with calm air (under 1 m/s rms) and a clean health
    check every 15 s.

## 8. Decisions for you

1. **Resolution target:** 128³ by default, with automatic step-down (recommended), or
   aim higher (160³–192³) at the cost of headroom?
2. **Separate 3D world:** a `World3d` beside the 2D `World` with an app mode switch
   (recommended; leaves 2D untouched), or one generalised world type (bigger
   refactor)?
3. **Mouse in 3D:** left drag orbits and right drag pushes (recommended), or keep left
   for tools and orbit with the middle button?
4. **Lighting:** emission and absorption first, self-shadowing later (recommended), or
   self-shadowing from the start (slower to first picture)?
5. **Obstacles:** analytic shapes only (recommended), or voxel painting in 3D?
6. **After smoke and fire:** go on to 3D liquid (phase G), or stop at smoke and fire?
7. **Order:** start 3D now, or finish the visualizer's remaining phases (F polish, G
   beat grid and extras) first?
