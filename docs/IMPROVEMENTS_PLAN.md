# Improvements Plan

Status: **approved** (2026-10-29), decisions taken (§4); **phase A done**. Nine ideas, grouped into phases.
Each phase is committed on its own.

## 1. Goal

Make the app easier and more striking to use, and free the fluids from their boxes:
- An interface that reads at a glance: larger, darker, higher contrast, with text that
  stands out.
- Fluids that fill the screen: no tank around the particles, no small box around the
  smoke, gravity in any direction, and walls that look good (or none).
- More to play with: a randomizer for new looks, more colour palettes, and new kinds
  of fluid.

## 2. Where things stand

| Idea | Today |
|---|---|
| GUI | Already a dark egui theme (panels #1C1E23, groups #262930, text #C8CDD7, one blue accent), but at egui's default sizes: about 12.5 pt text, 20 px rows, a 320 px side panel. No display font: headers are the body font in bold. |
| Randomizer | Nothing. Looks are data (TOML, `looks::Look`), so a random look is a matter of filling one in sensibly. |
| Display settings | The View tab's Screen group: the frame-rate cap and the windowed visualizer work (a 30 fps cap holds at 31 fps). Choosing a display seems to do nothing: it only applies the next time the visualizer goes full screen, under Windows' device names (§3.3). |
| Walls | Painted with the wall tool into the scene's solid mask, drawn as that mask thresholded into smooth edges, flat grey. The scenes' own tank walls are drawn the same way. |
| Solvers | 2D: FLIP, PBF, PBF (GPU), Stable fluids (dye), Smoke & fire, Lattice Boltzmann (dye), Ballistic (debug). 3D: Fire and Smoke (GPU). |
| Smoke | 3D smoke and fire live in a 1 × 1.5 × 1 m box, closed at the sides and floor and open at the top (`sim3d::scene`). The 2D Smoke & fire runs in the 2D tank. |
| Gravity | A tilt of ±90° from straight down (sliders, ← → keys, the compass) and a strength. 3D: always straight down. |
| Palettes | Five built in (Ocean, Lava, Slime, Ink, Mercury: `render::palette::PALETTES`), chosen by name in a menu of names, or edited colour by colour into a custom one. Looks name them in their files; the cover palette (phase H) makes its own. |
| Container | Every 2D scene is a 2:1 box (2 × 1 m) with solid walls, fitted inside the canvas with a 16 px margin and letterboxed. The particles bounce off its walls. |

## 3. The work

### 3.1 Interface: larger, darker, sharper

- **Darker, with more contrast** (as asked):
  - Near-black backgrounds: panels #0B0C10, groups #12141A, controls #1B1E26 (hovered #262A35).
  - Bright text: #F2F4F8 for values, #8A93A6 for labels and hints.
  - Thin borders (#2A2F3A) to separate things that now sit on near-black.
- **Text that pops:**
  - Fonts (§4, decision 6): Orbitron for titles, tab names and group headers, Inter for
    everything else. Both bundled with the app, under the Open Font License.
  - Headers in capitals with letter spacing, in a brighter accent.
  - One saturated accent pair, electric cyan #22D3EE into magenta #E040FB: for the selected tab, slider fills and the look's name in the visualizer bar, with a soft glow (a blurred copy under the text) on the main titles.
  - Warnings stay amber, errors red.
- **Larger:**
  - Body text 15 pt, values and buttons 16 pt, headers 18 pt, the title 24 pt.
  - 30 px rows, 36 px icon buttons.
  - A 380 px side panel.
  - The canvas tool strip and the full-screen bar scaled up to match.
  - An interface size setting (Compact, Normal, Large) through egui's zoom, saved, for small screens.
- **Easier to use:**
  - The side panel opens on the look rather than on solver internals.
  - Advanced groups (solver numerics, analysis meters, reaction tables) fold away by default, and their folding is remembered.
  - The top bar gets a large look picker and the randomize button (§3.2).
  - Fewer, clearer words on controls; the details move to hover text.
- **Animation:** quick and purposeful, never in the way. Most moves take 150 to 250 ms
  with an ease-out curve, starting fast and settling. The app redraws every frame
  anyway, so animation costs no extra repaints. egui has the pieces: animated panels
  (`show_animated`), and `animate_bool` / `animate_value_with_time` for any number.
  - **Panels slide:**
    - The side panel slides in and out from the right (\` and its button), the canvas
      and the fluid's view widening with it, frame by frame.
    - The full-screen bar rises from the bottom when the mouse moves, and sinks away.
    - Notices and the title card slide in from their edge and fade out.
    - The browser and the editor scale up slightly from their button as they fade in.
  - **Things that move with what you do:**
    - The selected tab's underline slides to the new tab.
    - The selected tool's highlight slides along the tool strip.
    - Toggles are switches whose knob slides across, the track filling with the accent.
    - Sliders' fills ease to a new value set from elsewhere (a look, an equation)
      instead of jumping.
    - A group's contents unfold and fold smoothly, with the chevron turning.
  - **Icons that move where it means something:**
    - The randomize dice tumbles while a new look is made.
    - The lock closes with a small snap when a look is kept.
    - The heart pops when a look becomes a favourite.
    - The stars fill one after another when rated.
    - The record dot pulses while recording.
    - Play and pause morph into each other.
    - The look's name in the full-screen bar slides out as the next one slides in,
      with the blend.
    - The gravity dial's arrow swings to a new angle rather than jumping.
  - **Hover and press:** buttons brighten and lift slightly on hover (a soft accent glow
    under them), and press in when clicked; cards in the browser rise a little under
    the pointer.
  - **Restraint:** nothing loops while idle except the recording dot. Nothing animates
    that moves what you're about to click: widths stay fixed, so motion is drawn, not
    laid out. A "Reduce motion" setting (also on when Windows' own setting asks for
    it) turns the movement into instant changes or short fades.
  - **Built once, used everywhere:** a small animation module (easing curves, a timed
    value, a spring for the dial), and the theme's widgets (switch, tab bar, icon
    button, card) built on it, so every tab gets the same feel.
- **Testing:**
  - The existing layout checks: nothing moves while music plays, and the side panel fits.
  - The animation module: easing curves start at 0 and end at 1, timed values arrive on
    time, and reduce-motion makes changes instant.
  - Screenshots mid-animation (the panel half open, a tab underline in flight) looked
    over by eye, and the frame time measured while the panel slides.
  - Screenshots of every tab at each interface size, looked over by eye.
  - A contrast check: every text colour against its background meets WCAG AA (4.5:1).

### 3.2 Randomizer: new looks at the press of a key

- **Make a random look** (a key, R being taken; a dice button in the top bar and the full-screen bar):
  - **The fluid:** any solver and scene, including 3D and no fluid. Weights favour the
    looks that work best (the tuning notes of phases I and H say which: for instance,
    brightest feedback with a strong zoom needs dim waves).
  - **The colours:** a palette from a random hue scheme (analogous, complementary or
    triadic), deep / base / highlight ordered by brightness, as the cover palettes are.
  - **The picture:**
    - Feedback mode, trail, zoom, turn, warp and echo.
    - A wave mode and up to two shapes.
    - The composite: symmetry, hue drift, vignette and the like.
    - All in ranges known to stay watchable, avoiding the combinations that blew out or
      went black in the phase I and H tours.
  - **The fluid's own settings** (§4, decision 4: everything that can change the
    picture): each solver's physical parameters within their controls' ranges
    (buoyancy, vorticity, viscosity, the fire's heat and fuel...), the liquid surface
    and its blob size, the glow, the gravity's angle and strength (§3.6), and
    sometimes an overlay field. Numerics that only trade speed for accuracy (pressure
    iterations, grid resolution, CFL) keep their defaults, so a random look runs as
    fast and as stably as a built-in.
  - **The motion:** reactions drawn from sets that suit the solver, and sometimes
    equations built from a library of snippets with random constants (a wandering
    centre, beat pulses, a spiral twist, ripples).
- **Vary the current look** (Shift + the same key): nudge its numbers by ±20 %, sometimes
  swapping one choice (the wave mode, the symmetry). Good for exploring around a
  favourite.
- **Keep it:**
  - A random look shows as "Random 1", "Random 2" and so on, until kept.
  - Save as (it exists) keeps it, with the author "random".
  - Backspace returns to what was on screen before (the director's history).
- **Optional:** the director could mix a random look into its rotation now and then
  (a setting, off by default).
- **Testing:**
  - A thousand random looks all load, round-trip through their files, and compile their
    equations.
  - Every value stays in range.
  - A tour of twenty random looks in the app, screenshotted, to tune the ranges by eye
    (as was done for the built-ins).

### 3.3 Display settings: fix

It's the Screen group's display choice ("Full screen on"): changing it seems to do
nothing. Checked on this machine's three displays:
- The choice itself works. Starting the visualizer, or pressing F11 after choosing,
  fills the chosen display (DISPLAY1 at (−2160, −225), DISPLAY2 at (5760, 183)).
- But picking a display does nothing you can see:
  - **It waits.** The choice only applies the next time the visualizer goes full screen,
    and nothing says so.
  - **The names are cryptic.** They're Windows' device names (`\\.\DISPLAY1`), with
    nothing to tell you which screen is which.
  - **There's no panel in full screen,** so you can't change it from where you'd use it.

The fix:
- **Act at once.** Picking a display moves the app there straight away: the window is
  centred on that display (keeping its size, shrunk to fit), and if the visualizer is
  full screen it moves to fill that display.
- **Readable names:** "Display 2, 1080 × 1920, right of the main one", numbered as
  Windows numbers them, with "(main)" on the primary one.
- **Identify:** a button that briefly shows each display's number on that display, as
  Windows' own display settings do.
- **From full screen:** a display picker in the full-screen bar, and keys to move the
  visualizer to the next display (Shift+F11).
- **Remembered:** the app starts on the display it was last on.

Also in this group:
- **The Display group** (colours, liquid surface, particles): its controls disappear for
  grid and 3D solvers (by design, with a note), which can read as broken. Keep the colours
  always shown, and grey out what doesn't apply instead of hiding it.
- **The render scale:** "Automatic" only differs at 4K and above; say what it picked.

### 3.4 Walls: make them look good

Keep them, and draw them properly (§4, decision 2).
- **Shape:** the solid mask's signed distance (from the mask, by a jump-flood pass when
  it changes) gives smooth, resolution-independent edges instead of a thresholded blob.
- **Shading:**
  - A bevelled look from the distance's slope: a rim light on the edges facing the
    light, a soft shadow cast onto the fluid behind.
  - The fill from the look's palette (its deep colour, darkened), so walls belong to
    the picture rather than sitting on it in grey.
- **In the visualizer:** walls are drawn after the feedback, so they don't smear into
  streaks; or, a look's choice, before it, so they do.
- **The tool:** a brush preview of the finished wall shape while painting, and
  undo (Ctrl+Z) for the last stroke.
- If after a first pass they still don't look good, remove the wall and erase tools and
  keep walls only as part of scenes (the obstacle, the chimney). A small change: the
  tool strip and two shortcuts.

### 3.5 No container: particles free on the canvas

- **The domain fits the canvas:**
  - A scene's domain takes the canvas's shape (wider on a wide window), at the same
    height, instead of a fixed 2:1 box.
  - Scenes are described relative to the domain, so they still make sense at any width.
  - Resizing the window enough to change the shape refits the scene at the next reset,
    not mid-flight: grids are sized when a scene loads.
- **No walls drawn, no letterbox:** the domain's edges are invisible, the fluid reaching
  the window's edges. In the windowed view, a faint frame stays at the canvas edge, so
  you can tell where the fluid ends.
- **At the edges, the fluid bounces** (§4, decision 3): the edges stay solid but
  invisible, as the floor and walls of a canvas-sized tank. All solvers already
  support it. (Considered and set aside: wrapping round, which needs periodic boundaries
  in every solver, and open edges, which suit fountains and smoke but not dam breaks.)
- **Scenes:** the dam breaks and tank keep a floor, which gravity defines, not a drawn tank. The
  obstacle stays an obstacle.

### 3.6 Gravity in any direction

- **2D:**
  - A full 360°, and a strength from 0 (weightless) up.
  - The compass becomes a dial you drag: the angle, and the distance from the centre
    as the strength.
  - The ← → keys keep turning it, now all the way round; ↑ ↓ change the strength.
- **Equations:** `gravity` is already an angle; nothing limits it to ±90° any more.
  `gravity_strength` is added.
- **Reactions:** the gust and slosh effects work relative to the current gravity, so they still look right with it sideways.
- **3D:**
  - Gravity as two angles (a heading and a tilt from straight down), so the smoke can
    rise sideways.
  - Burners keep blowing their way, so a fire leans as the gravity turns.
- **Goes with §3.5:** with no tank drawn, gravity pointing anywhere is what lets the
  fluid fall to any edge of the screen.

### 3.7 Smoke without the box

- **3D:**
  - A wider, lower domain (3 × 2 × 3 m, the fire in the middle), open on every side and
    the top, with no box lines.
  - The smoke fades out as it nears the open sides, so the edge of the volume never
    shows.
  - The same number of grid cells, spread wider (the fire can be finer than the space
    around it if needed: a stretched grid is possible later).
  - The camera frames the fire and its plume, not the box.
- **2D Smoke & fire:** with §3.5, the fire fills the canvas, and the open edges (or the
  top at least) let the smoke out instead of pooling under the ceiling.
- **Cost:** the open sides cost nothing; the wider domain costs resolution near the fire
  unless the grid stretches. Measured with the 3D tests and the soak test.

### 3.8 More solvers

Candidates, each picked for what it adds to the picture:

| Solver | Adds | Effort |
|---|---|---|
| **MLS-MPM** (Moving Least Squares Material Point Method) | New materials: snow, sand, jelly, elastic blobs; they splash, crumble and wobble. Particles, so it fits the particle renderer and the liquid surface. | Medium. Well documented (Hu et al. 2018, "88 lines" MLS-MPM); one grid transfer like FLIP. A GPU version later. |
| **SPH** (smoothed particle hydrodynamics, divergence-free DFSPH) | A second liquid with a different character: livelier splashes than PBF, viscous flows (honey, lava). Shares PBF's neighbour search. | Medium. |
| **Shallow water** (height field) | Ripples, waves and wakes on a water surface seen from above; very fast, full screen at high resolution, a strong match for music (drops as ripples). | Small. |
| **Stable fluids on the GPU** | The dye solver at 4 to 8 times the resolution, for crisp ink at full screen. | Medium (the 3D GPU solver's pieces, in 2D). |
| **Reaction-diffusion** (Gray-Scott) | Not a fluid: living patterns (spots, stripes, coral) that the music can feed. Very MilkDrop. | Small. |

In this order (§4, decision 5): shallow water (quick, very visual), then MLS-MPM (the
biggest new look), then GPU stable fluids. SPH and reaction-diffusion may follow. Each comes with scenes, built-in looks, and the usual
tests: invariants, the benchmark, and a soak in the visualizer.

### 3.9 More colour palettes

- **About fifteen more built-in palettes**, each a deep, base and highlight colour
  ordered by brightness, as the five are. Chosen to cover what's missing:
  - Warm: Sunset (violet, orange, gold), Ember (deep red, orange, yellow-white), Gold.
  - Cool: Ice (navy, glacier blue, white), Aurora (teal, green, violet highlight),
    Deep sea (black-blue, teal, aqua).
  - Bright: Neon (indigo, cyan, magenta-white), Vapor (purple, pink, cyan),
    Candy (berry, pink, mint), Toxic (black-green, acid green, yellow).
  - Earthy and muted: Forest, Rust, Sand, Slate.
  - Plain: Mono (black, grey, white) and Blood (near-black red, crimson, pink).
- **Checked, not just picked:**
  - Each is tried on a particle solver, a dye solver, the 3D smoke and a look without
    fluid, screenshotted, and kept only if it reads well on all four.
  - The deep colour needs to stay visible on near-black; the highlight needs enough
    contrast with the base to show foam and dense dye.
- **A better menu:** the palette menu shows each palette's three colours as swatches
  beside its name, grouped as above (with the interface work in phase A).
- **Everywhere they're used:**
  - Look files name them as now. A test makes sure every name is unique and none
    changes the meaning of an existing look file.
  - The randomizer (§3.2) picks among them, as well as making its own colour schemes.
  - Some built-in looks may move to a palette that suits them better, if the
    screenshots say so.
- **Testing:** names unique, every palette ordered by brightness, a look naming each one
  round-trips, and the screenshots above.

## 4. Decisions (2026-10-29)

1. **Display choice:** it's the Screen group's display; fixed as in §3.3, as part of
   phase A.
2. **Walls:** make them look good (§3.4). If that fails, remove the wall tools.
3. **Edges without a container:** the fluid bounces off invisible edges (§3.5).
4. **Randomizer scope:** every setting that can affect the picture, the fluid's own
   physical parameters included; numerics keep their defaults (§3.2).
5. **Solvers:** shallow water, then MLS-MPM, then GPU stable fluids (§3.8).
6. **Fonts:** Orbitron for titles and headers, a wide geometric face that suits a
   visualizer and stands out on near-black; Inter for everything else, for clear
   text at small sizes and in numbers. Both under the Open Font License.

## 5. Phases

| Phase | Scope | Visible result |
|---|---|---|
| **A. Interface** | Darker theme with more contrast, Orbitron and Inter, the accent, larger sizes, interface size setting, advanced groups folded; animation (sliding panels, moving icons, hover and press, reduce motion); the display choice fixed (§3.3); more palettes, with swatches in their menu (§3.9). | An app that reads at a glance and feels alive, in more colours. |
| **B. Free canvas and gravity** | Domain fits the canvas, no tank or letterbox, invisible edges; gravity at any angle and strength, the dial; 3D gravity direction. | Fluid edge to edge, falling any way you point it. |
| **C. Walls** | Distance-field walls with bevel, shadow and palette colour; brush preview and undo. (Their removal, if they still don't look good.) | Obstacles that look designed. |
| **D. Open smoke** | 3D smoke in a wide open domain with faded edges and no box; 2D smoke venting through open edges. | Smoke rising into open air. |
| **E. Randomizer** | Random and varied looks, the fluid's own settings included; keep and back; the snippet library; optional random looks in the director. | Endless new looks from one key. |
| **F. New solvers** | One phase per solver: shallow water, MLS-MPM, GPU stable fluids. | New materials and kinds of motion. |

Suggested order: A first (everything after it is seen through the new interface), then B
(it changes what every scene looks like), then C and D, then E, then F. A and E are
independent of the rest and could be done in either order.

## 6. Progress

- **Phase A (done):** the interface (`app/src/ui/theme.rs`, `anim.rs`, `widgets.rs`,
  and every tab and bar).
  - **Darker, sharper:**
    - Panels #0B0C10, groups #12141A with thin borders, controls #1B1E26; text #F2F4F8
      and #8A93A6; one accent, cyan #22D3EE, with magenta #E040FB in title glows.
    - Every text colour meets WCAG AA (4.5:1) against every background it's drawn on
      (a test).
    - Pressed controls brighten with an accent edge rather than filling with the
      accent: egui draws its "strong" text in the pressed colour, and dark text there
      made bold labels vanish (the first in-app check showed the song title gone).
  - **Fonts:**
    - Inter (Regular, SemiBold) for text and Orbitron Bold for titles, tabs and headers,
      from Fontsource with their OFL licences in `assets/fonts`, compiled in.
    - egui's own fonts follow as fallbacks for symbols, and the icons in every family.
    - Headers are capitals with letter spacing in the accent; titles have a soft
      cyan-and-magenta glow.
  - **Larger:**
    - Body text 15 pt, headings 20 pt, 30 px rows, 36 px icon buttons, a 380 px side
      panel, a larger tool strip.
    - An interface size setting (Compact 85 %, Normal, Large 120 %, through egui's zoom),
      saved.
  - **Easier to use:**
    - The side panel opens on the Music tab (the looks are there); tabs in the display
      face, Music, View, Fluid (was Simulate), Info.
    - Every group folds from its header; the advanced ones start folded (Analysis, Beat,
      Reactions, Overlay, the performance breakdowns, Shortcuts), and folds are saved.
    - The top bar has the look on screen, large, with previous and next.
  - **Motion** (a small module, `ui::anim`: eased transitions, timed values, one-off
    kicks):
    - The side panel slides (egui's collapsible panel); the full-screen bar rises and
      sinks; notices drop in and rise away; the title card slides in from the left.
    - The tab underline and the tool strip's highlight slide to the chosen one; switches
      replace checkboxes, their knobs sliding; folds turn their chevron; sliders ease to
      values a look sets.
    - The lock pops, the heart pops (overshooting), stars fill one after another, the
      record dot pulses, play and pause morph, the look's name slides out as the next
      slides in, the gravity dial's needle swings.
    - Buttons lift and brighten on hover with an accent edge and a soft glow; browser
      cards rise with a deepening shadow.
    - "Reduce motion" (a setting, and on when Windows' animation effects are off) makes
      every change instant, egui's own included.
    - The dice tumble came with the randomizer (phase E).
  - **The display choice** (§3.3):
    - Picking a display moves the app there at once (the window centred, or the
      visualizer filling it), and the app starts on the display chosen.
    - Readable names ("Display 2 · 1620 × 2880 · right"), and in place of an Identify
      button (which needs windows on the other displays), a map of the displays as
      Windows arranges them: numbered, the chosen one lit, the app's marked, clickable.
    - Shift+F11 and a button in the full-screen bar move to the next display (the
      shortcut table learned Shift).
    - A window moved to a display with other scaling is resized by Windows after it
      lands, so it's centred again a few frames later.
  - **Palettes** (§3.9): 16 more (Sunset, Ember, Gold, Blood; Ice, Aurora, Deep sea;
    Neon, Vapor, Candy, Toxic; Forest, Rust, Sand, Slate; Mono), grouped in the menu
    with swatches. The test caught two whose highlight was darker than their base
    (Aurora, Neon), now paler.
  - Tests: contrast; the fonts installed; easing and reduced motion; folds remembered;
    displays described, numbered, placed and cycled; Shift shortcuts; every palette
    unique, grouped, ordered by brightness and visible on near-black.
  - Checked in the app (debug build, scratch settings; your own copy of the app was left
    running):
    - Every tab at Normal, Compact and Large. Two fixes came from it: the
      chevrons crowding their headers, and a tool caption wrapping.
    - The side panel's slide (210 fps before, 199 during).
    - The window opening centred on the remembered display, on each of three displays.
    - All 16 new palettes on particles, dye, 3D smoke and a look without fluid (64 looks
      screenshotted): all readable; smoke only tints, as before.
  - Not checked in the app: Shift+F11 and clicks on the display map (my scripts can't
    send Shift or reliable clicks), and the motion frame by frame (screenshots can't
    catch a 0.2 s slide reliably).

- **Phase B (done):** the free canvas and gravity in any direction (§3.5, §3.6).
  - **Scenes of any shape** (`sim/src/presets.rs`):
    - A scene is built for a domain: `domain_for(aspect)` gives one whose short side is
      1 m, shaped like the window (aspect held to 0.5 to 3.6). Positions are given
      relative to the domain, so every scene makes sense from a tall window to an
      ultrawide one (a test builds them all at five shapes).
    - The world keeps its domain; loading a scene builds it at that shape.
  - **No tank on screen** (`app/src/view.rs`, `View::cover`): the inside of the
    scene's walls covers the canvas edge to edge, so the walls fall just outside the
    window and the fluid reaches the edges, bouncing off them unseen. No letterbox, and
    the tool strip floats over the fluid instead of taking a column.
  - **Refitting:** the scene takes the window's shape when the app starts, and again
    when the window's shape stays more than 8 % off for half a second (a resize, full
    screen). The side panel crops the sides rather than reshaping the scene, so opening
    it doesn't restart anything. Gravity as set survives the rebuild.
  - The faint frame at the canvas edge (§3.5) was left out: with the edges invisible,
    the window's own edge already shows where the fluid ends.
  - **Gravity any way** (`ui/mod.rs`, `ui/compass.rs`):
    - The dial goes all the way round (±180°, with a tick at straight down), and its
      radius is the strength from 0. ← → turn it 5° at a time with no stop, ↑ ↓ change
      the strength by 0.5 m/s²; the Fluid tab's World group has the angle, the strength
      and a Down button.
    - Equations: `gravity` is no longer held to ±90°, and `gravity_strength` (a
      multiple, 1 as set, up to 5) scales it.
  - **3D:** gravity is the same tilt plus a heading (a slider in the World group), so
    smoke can rise sideways. A 3D scene's own gravity shows on the dial when it loads.
    The music's reactions see gravity from the camera and give back the part along the
    view untouched, so sideways gravity survives them (a test).
  - Tests: domains follow the window, every scene fits any shape, cover fills the area,
    gravity points any way and round-trips through the panel, sideways gravity through
    the 3D reactions.
  - Checked in the app (debug build, scratch settings):
    - The scene fitted to 1.78 × 1 m at start; the fluid edge to edge with the panel
      open and closed, and in full screen after the refit, with no tank anywhere.
    - Gravity turned with the arrow keys to straight up (the water fell to the top) and
      to 90° (it piled on the right); 3D smoke with gravity to the right drifted left.
    - A first try looked broken: my script posted the arrows without the extended-key
      flag, so they arrived as the numpad's 4 and 6. Posted as real arrows, they work.

- **Phase C (done):** walls that look designed (§3.4); they stay.
  - **Shape** (`app/src/render/solid.rs`):
    - The walls are drawn from a distance field: the mask, softened by a small
      Gaussian (twice, about a cell and a half), is sampled at three times its
      resolution, and each sample's distance to the half-full contour is found
      exactly (Felzenszwalb's separable transform), sharpened near the edge from the
      occupancy's slope. Stored in 8 bits (±16 samples) and filtered, it gives smooth,
      round edges with no cell steps at any size.
    - Built on the CPU (in parallel) when the mask changes, not by a GPU jump flood:
      the mask is small, and it changes only while painting.
    - The domain's own border is left out: it lies just outside the window, and drawn
      it would only shadow the fluid's edges.
  - **Shading** (`scene.wgsl`): a bevel lit from the top left, a thin rim along the
    edge in the palette's highlight (brightest facing the light, and caught by the
    glow), a soft shadow cast down and right onto whatever is behind, and the fill from
    the palette's deep colour, darkened. Walls change colour with the palette.
  - **In the visualizer:** with feedback on, the walls are drawn after it, over a copy
    of its picture, so they stay sharp while the fluid around them smears. The choice
    to let them smear (a look setting) was left out: nothing asked for it yet.
  - **The tools:**
    - A preview under the cursor with the wall and erase tools: the wall the stroke
      would leave, shaded, faint; or, erasing, the part that would go in red.
    - Ctrl+Z undoes the last wall stroke (32 kept, per scene). Shortcuts learned Ctrl.
  - Tests: the field measures distance to a painted disc (within a cell), the border
    left out, thin strokes kept, the transform exact; undo stroke by stroke and per
    scene; Ctrl+Z; a headless render of the obstacle (solid, shadowed, lit rim) and of
    both previews.
  - Checked in the app (debug build): the obstacle round and smooth, lit and
    shadowed; under Firestorm's feedback it stays crisp, in the look's red and gold.
    My scripts can't paint (posted mouse moves read as the cursor leaving the
    window), so painting, the preview and Ctrl+Z were checked by the tests and the
    headless render, not by hand.

- **Phase D (done):** smoke in the open (§3.7).
  - **3D** (`sim3d/src/scene.rs`):
    - Every scene stands on a floor in the open: 2 × 2 × 2 m, vents along all four
      sides and across the top (the solver already opened a wall wherever a vent lay
      against it), the fire in the middle. The chimney became a hood with a flue pipe,
      the smoke gathering under it, spilling round its edges and going up the pipe.
    - 2 × 2 × 2 m rather than the plan's 3 × 2 × 3: measured on this GPU, the old box
      took 3.6 ms a step (1.8 M cells); 3 × 2 × 3 at the same cost would need cells
      twice as big. The default resolution went from 160 to 144 cells up (3 M cells,
      about 5.5 ms a step), cells 1.5 times as big as before; the governor still
      lowers it in full screen if frames fall behind.
    - The view (`render3d`): no box or vent lines; the floor grid fades out round the
      fire; the gas thins to nothing over the last 35 cm before the open sides and top,
      so the volume's edge never shows; the camera frames the fire and its plume (1 ×
      1.4 × 1 m), not the domain, so the plume rises out of the picture.
  - **2D Smoke & fire:** the campfire's top and sides (above a low sill) are open:
    its drains now lie in the border just outside the window (a drain there opens the
    domain's always-solid border; painted walls stay solid), so smoke rises out of the
    picture with no band where it vanishes. Stable fluids and LBM both honour it.
  - Tests: every 3D scene open but for the floor; the hood and its flue; with a gust
    blowing, the volume keeps about a third of the smoke it keeps with only its top
    open (421 against 1135); the 2D campfire has far less smoke under its top than the
    same scene closed; the floor grid fades and nothing else is drawn; the camera
    frames the fire's region.
  - Checked in the app (debug build): Fire on all four scenes and Smoke on the campfire
    and chimney, 150 to 170 fps; the 2D campfire's smoke leaving through the top.
  - A note on the test run: once, the sim3d GPU tests hung (a process waiting on the
    GPU for 20 minutes, using no CPU) while another copy of the app was running; run
    again, they passed in 8 s. Not reproduced since.

- **Phase E (done):** the randomizer (§3.2), `app/src/looks/random.rs`.
  - **D** (or the dice in the top bar and the full-screen bar) makes a new look and
    blends to it; **Shift+D** (or Shift-clicking the dice) varies the look on screen.
    The dice tumbles, turning over twice as its face changes. A notice says how to keep
    it or go back.
  - **What a random look is:**
    - The fluid: none (the most likely, weight 3), FLIP, PBF, Stable fluids, Smoke &
      fire, Lattice Boltzmann, Fire 3D or Smoke 3D, each in the scenes its built-ins
      use (the dye solvers need something to stir; fire needs its burner). The debug
      solver and the GPU PBF (not always there) are left out.
    - Its physics: every physical parameter, from a table of watchable ranges inside the
      controls' (`PHYSICS`; a test checks each against the solver, the 3D ones on the
      GPU). Numerics (iterations, resolution, CFL) keep their defaults. Every key is
      set, so nothing carries over from the look before.
    - Colours: a built-in palette (40 %) or one from a hue scheme (analogous,
      complementary or triadic), ordered by brightness, the base no brighter than
      luma 0.55, the highlight well above it.
    - The picture: feedback mode, trail, zoom, turn, drift, warp, bass zoom, echo; a
      wave mode (and push, for 2D fluids) and up to two shapes; symmetry, hue drift,
      grading, palette map, aberration, vignette, grain. Liquid surface for particles;
      an orbit speed in 3D.
    - Reactions: two to four from a set that suits the fluid (particles, dye, 2D fire,
      3D fire, 3D smoke, none), the first always its signature (kick splash, snare
      vortex, kick jet, kick flash), sometimes the bass on a physical parameter.
    - Equations, half the time: one to three per-frame snippets (a wandering centre, a
      beat pulse, a swinging turn, a hue swing, a pulsing vignette) and with feedback
      on, sometimes one per-pixel (a spiral twist, ripples, petals, a faster rim). The
      2D fluids' gravity leans or swings now and then (gravity is set through the
      equations, so it's kept with the look), and sometimes the bass moves a physical
      parameter.
    - Energy (calm, energetic, either) from how hard it moves.
  - **Watchable:** after drawing, `tame` clamps every value, and in brightest mode keeps
    out what blew out: a strong zoom with bright waves or long trails, long trails
    with much zoom or echo, bright waves under any zoom, filled shapes, exposure above
    1.05, long trails on particles and dye, and a strong palette map over a bright
    palette.
  - **Keeping it:** random looks are named "Random 1", "Random 2" and so on (skipping
    names in use), with the author "random" and the tag "random". They sit in the
    library after the rest (the browser, the menu, the director's history), the last
    20 kept, until Save as writes one to the folder, where the file takes over.
    Backspace goes back as with any look. The director doesn't pick them by itself
    unless they're favourites.
  - **In the director's rotation** (Music tab, "Mix in random looks", off by default):
    a quarter of the director's own picks become a new random look of the same energy
    (eight tries), not requests or a locked look's refresh. The passed-over pick then
    counts as never shown, so it comes soon.
  - **Left out:** glow, blob size and the overlay. They're the user's display
    settings, not part of a look, so a random look that set them would change them for
    every look after.
  - **Tests:** a thousand random looks round-trip through their files with their
    equations compiled; every value in range and none of the bright combinations;
    every fluid, wave mode and energy turns up; variations keep their fluid and code,
    stay within ±20 % and load; the physics ranges fit the controls (2D and 3D); random
    2D looks apply with every parameter known; the library keeps, names, saves and
    drops random looks; the director's own picks are marked and can be replaced; the
    D keys and the setting round-trip.
  - **Checked in the app** (debug build, the long test beat, muted, full screen): three
    tours of twenty random looks, screenshotted.
    - The first showed two blown out to white: brightest feedback over a filled shape
      and bright spectrum, and bright particles under a strong palette map.
    - The second, after taming those, showed four nearly black: no-fluid looks with
      smear feedback fading toward the black between thin waves. Without fluid, a
      random look now uses brightest 90 % of the time, as every built-in does. Its
      equations also repeated a snippet; each now appears once.
    - The third: twenty varied looks at 52 to 240 fps, none black or blown out. One
      liquid look kept a large flat white area, and one particle look was sparse.
      Backspace went back. No errors.
    - Not checked in the app: Shift+D (a posted Shift doesn't reach the app; the tests
      cover varying), clicking the dice and the director mixing in random looks (both
      covered by tests).

- **Phase F, shallow water (done):** a water surface seen from above (§3.8),
  `sim/src/solvers/shallow.rs`.
  - **The solver:** the shallow water equations on a staggered grid (Müller 2008): the
    depth at cell centres, the velocity on faces. Per substep: brushes and the tilt;
    semi-Lagrangian advection of the velocity (one step back along the face's own
    velocity: first order is plenty when waves carry the motion, and five times
    cheaper than sampling the field); the depth moved by upwind fluxes, so water is
    neither made nor lost; acceleration down the surface slope; damping; and a little
    smoothing of the velocity (4/s toward the neighbours' average) that wipes out
    grid-scale chop.
  - **Scenes from above:** water everywhere at the resting depth. Fluid blocks stand
    half as high again (a dam break is a bore spreading out). Walls are islands.
    Emitters lift the water at their nozzles and push a current (the faucet too).
    Drains absorb waves. The surface eases back to the water it started with, so
    emitters don't flood it. The scene's gravity, turned from straight down, tilts the
    water (slosh rocks it); waves themselves always run at sqrt(g × depth).
  - **Parameters:** depth (1 to 50 cm, default 3: waves at 0.54 m/s, slow enough to
    watch cross the picture), damping (default 0.3/s), resolution (192 cells up), CFL.
  - **Two new scenes**, at the end of the list: **Pond** (four rocks and a stone
    dropped among them) and **Breakwater** (a wall with two gaps and a band of raised
    water below it: one wave rolls up and each gap spreads it in rings). Other solvers
    get a falling blob and a pool under a holed shelf. Every solver keeps its
    invariants on both (the all-solvers test).
  - **Drops:** a new reaction effect, **Drop**: a drop lands at a random spot. It
    travels in the step input (`sim::Droplet`, a rate held for 0.09 s, so it lands the
    same at any frame rate); shallow water dents its surface (3 cm), which rings out;
    other solvers ignore it. Rendered headlessly, a drop makes a crisp ring with fine
    ripples inside, still clear a second later.
  - **Drawn as lit water** (`FieldKind::Water`, shaded on the CPU like fire): three
    values per cell (height and slope). The palette's deep colour in the troughs to its
    base on the crests, lit from the upper left with slopes drawn four times steeper, a
    glint in the highlight where the slope mirrors the light, and foam only where the
    water is steep enough to break.
  - **Speed:** at first 43 ms a frame (the velocity's RK2 backtrace sampled the field
    five times per face; brushes and wall faces were walked serially every substep).
    With a first-order backtrace, brushes touching only the faces under them and the
    face masks computed once per substep: about 4.6 ms a frame on the dam break and
    3.8 on the pond (the bench, five substeps a frame). In the app, 90 to 190 fps.
  - **Brushes** push the water no faster than 1.5 times the wave speed: the music's
    splashes (made for particles, up to 5 m/s) broke it into shocks with stair-stepped
    fronts.
  - **Built-in looks:** *Moonlit pond* (calm: drops on kick and snare, night colours),
    *Harbour wall* (waves from the kick rolling through the breakwater, drops on the
    snare, Deep sea) and *Rain dance* (energetic: hard drops, mirrored, Neon).
  - **The randomizer** knows shallow water: its five scenes, depth and damping, and
    reactions with drops. Feedback stays off it: water's picture is its thin lit wave
    fronts, which any feedback smeared to mush (the in-app tuning found this: the
    first pond looks, with feedback, showed soft blobs where the headless frames had
    crisp rings).
  - **Tests:** still water stays still and all there; a raised block spreads at the
    wave speed and keeps its water; a brush pushes a wave; tilting sloshes; walls stay
    dry; emitters make currents without flooding; a drop dents and rings out; the water
    field has height and slope; water is lit (crests lighter, slopes toward the light
    brighter, a glint, walls transparent); random looks with shallow water keep its
    ranges and no feedback. The soak runs the three new looks with the rest.
  - **Checked in the app** (debug build, the long test beat, muted): shallow water on
    the breakwater, pond, dam break, tank with obstacle and wind tunnel (a wake behind
    the cylinder), and the three looks in full screen, tuned over five rounds of
    screenshots (above).
  - A compiler crash (an access violation inside rustc, not this code) left the sim
    crate's incremental cache broken once; clearing it fixed the build.

- **Phase F, MLS-MPM (done):** new materials (§3.8), `sim/src/solvers/mpm.rs`.
  - **Three solvers, one implementation:** *MPM jelly* (elastic, fixed corotated),
    *MPM snow* (Stomakhin et al. 2013: it packs, hardens as it does, and breaks into
    clumps past its critical compression and stretch) and *MPM sand* (Drucker-Prager on
    the log strain, Klár et al. 2016: it flows and piles at its friction angle, and
    lets go when pulled apart). MLS-MPM (Hu et al. 2018) with APIC: each particle
    carries a velocity, an affine velocity C, a deformation gradient F and (snow) its
    plastic volume ratio; quadratic B-spline weights over 3 × 3 nodes.
  - Per substep: drains and emitters (draining every per-particle array, since the
    shared drain only knows positions and velocities) -> particles to grid (in
    parallel, each thread into its own grid, summed) -> the grid: gravity, brushes, and
    walls (slip; sand with Coulomb friction) -> grid to particles, plasticity, moved and
    kept out of walls.
  - **Explicit and elastic:** the substep follows the material's sound speed, so MPM
    takes many small, cheap substeps. The stepper's cap of 8 a frame would have run it
    in slow motion: solvers can now raise it (`Solver::max_substeps`, 40 here).
  - **Parameters:** stiffness (as a sound speed squared, m²/s²: jelly 30, snow 25, sand
    60), Poisson ratio; snow's hardening, critical compression and stretch; sand's
    friction angle; resolution (64 cells up, 4 particles a cell: about 8,000 on the dam
    break); CFL. Packed snow gets at most 4 times stiffer: at first it went up to 20
    times, which took the substeps to the cap and dropped time.
  - **Speed** (the bench, dam break): jelly 6.9 ms a frame (16 substeps), sand 12.6
    (22), snow 12.1 (25). In the app, 200 to 240 fps.
  - **A new scene, Falling blocks:** three blocks in mid-air over a shallow layer. Jelly
    lands, bounces and wobbles; liquids splash; shallow water rings.
  - **Built-in looks:** *Jelly drop* (blocks flung by the kick, tumbling and wobbling,
    the floor layer breaking into jelly strips; Candy), *Snow globe* (snow columns
    thrown up into powder; Ice) and *Shifting dunes* (sand sliding and heaping as
    gravity rocks; Sand). Tuned from the first in-app round: jelly under feedback and a
    strong splash smeared into mush, and snow dropped from 75 cm packed flat into the
    floor layer.
  - **The randomizer** knows all three: their scenes, physics ranges and the particle
    reactions.
  - **Tests:** the SVD rebuilds its matrix; jelly stands (settling a little, not
    spreading); sand slumps into a pile (lower, wider, not flat); snow packs and doesn't
    bounce; every material stays finite, in the domain and calm on two scenes; and the
    all-solvers tests run all three on every scene and switching between every pair.
  - **Checked in the app** (debug build): each material on the dam break, tank with
    obstacle and fountain (jelly slides as a block, snow holds as a slab, sand slopes),
    and the three looks in full screen with the test beat.

- **Phase F, GPU stable fluids (done):** the dye solver on the GPU (§3.8),
  `sim-gpu/src/stable.rs` and `stable.wgsl`. Phase F is complete.
  - **The algorithm** is the CPU solver's, dye mode: a 2D port of the 3D smoke solver's
    kernels with the CPU solver's boundaries. Per substep, one compute pass: classify
    the cells (the scene's walls and drains, worked out on the CPU when the mask or the
    drains change; the emitters' nozzles on the GPU, releasing dye, striped for wide
    ones) -> prescribed faces -> buoyancy -> vorticity confinement -> brushes ->
    semi-Lagrangian velocity advection -> divergence -> red-black SOR pressure sweeps
    (warm-started) -> projection -> MacCormack dye advection, faded.
  - **Nothing comes back while stepping.** The dye (with the fastest speed in the fluid,
    an atomic max, for the next substep's size) and the velocity are read back only
    when the app asks (`fill_field`, `velocity_field`: once a frame, for drawing).
  - **8 storage buffers** is all a stage may bind by default: the dye's three copies
    share one buffer (vec4 per cell), the cell kinds and the scene's base kinds another,
    and the last kernel leaves a compact copy of the dye in the spare velocity buffer
    to read back.
  - **Parameters:** the CPU solver's (buoyancy, vorticity, dissipation, pressure
    iterations, over-relaxation, CFL) under the same keys, so looks and the randomizer
    carry over; 384 cells up by default (768 × 384: nine times the CPU solver's cells).
    The pressure is plain SOR, not the CPU's multigrid: at this size it doesn't fully
    converge, which leaves the flow slightly compressible but stable.
  - **Without a GPU** the registry falls back to the CPU solver, as GPU PBF does.
  - **The wind tunnel's inlet now reaches into the walls** (which win). It used to stop
    5 mm short of them: the inlet draws from the pocket behind it, and at 384 cells those
    gaps were two cells wide and the pocket's only way in, so the inflow went through
    them at 96 m/s and the substeps shrank to nothing (6 % slow motion). At the CPU's
    resolution the gaps were under a cell, so the pocket was sealed. More pressure
    sweeps didn't help, which is how it was found not to be the solver.
  - **Looks:** *Ink nebula* (heavy ink in the double dam break, stirred by the snare;
    Ink) and *Silk streams* (the wind tunnel's streaklines curling round the cylinder,
    mirrored; Aurora). The randomizer picks it like the CPU dye solver.
  - **Tests** (on a GPU, skipped without): heavy dye slumps along the floor and keeps
    its amount, between 0 and 1, finite; light dye rises; the wind tunnel flows through
    without piling dye up; a spinning brush stirs a vortex its way. The app's tests of
    looks, random looks and the soak now build their worlds with the GPU solvers too,
    as the app does (before, a GPU look was quietly run on the CPU's first solver).
  - **Checked in the app** (debug build): beside the CPU solver on the double dam break
    (far finer swirls), wind tunnel and fountain (88 fps where the CPU solver falls to
    slow motion), and the two looks in full screen (75 to 107 fps).
  - Twice in this phase a compiler crash left an incremental cache broken (sim, then
    the app); clearing that crate's cache fixed the build each time.
