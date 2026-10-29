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
    - The dice tumble waits for the randomizer (phase E).
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

