# 3D Simulation Plan

Status: **approved; Phase A done.** Decisions (§8), taken as recommended
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
