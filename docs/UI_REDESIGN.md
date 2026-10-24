# UI Redesign Plan

Status: **approved. Phases A–C done.** A: top bar, status bar, tabbed panel, canvas rect from egui, panel toggle. B: theme, aligned form rows and groups, restart markers; the panel widened to 320 pt so rows fit. C: one shortcut table (`ui/shortcuts.rs`) behind the key handler, tooltips, Info tab and an F1 cheat sheet; tools on 1–4, brush size on [ ]; Esc closes the sheet before quitting; particle-only settings hidden for grid solvers. The §8 decisions were taken as recommended
(2026-10-24):

| # | Decision | Resolution |
|---|---|---|
| 1 | Tools | Vertical strip on the canvas. In Phase A they sit in the top bar; Phase D moves them to the strip. |
| 2 | Panel side | Right. |
| 3 | Panel structure | Tabs. |
| 4 | Icons | Embed an icon font (Phase D); text labels until then. |
| 5 | Gravity compass | Deferred to Phase F; sliders until then. |
| 6 | Stats | Status bar plus an Info tab. |
| 7 | Persistence | Deferred to Phase F. |
| 8 | Panel toggle key | `` ` `` (backquote). |

## 1. What's wrong today

The whole UI is one 340-pt scrolling column (`crates/app/src/ui.rs`) with nine sections
in this order: stats, playback, scene, tools, music, look, overlay, solver, help.

| # | Problem | Where it shows |
|---|---|---|
| P1 | **No hierarchy.** Every control has the same weight; sections are separated by thin rules, and most have no heading (scene, tools, look, overlay). | Whole panel |
| P2 | **Debug info takes the best spot.** The top 6–10 rows are numbers: fps, sim cost, time, raw metric names such as `max \|ρ - 1\|`. | Stats |
| P3 | **What you change most is at the bottom.** Solver parameters sit below the fold; at 1080p you scroll to reach them. | Solver |
| P4 | **Unlabelled dropdowns.** The preset, overlay and solver combos have no label, so their meaning comes only from the current value. | Scene, overlay, solver |
| P5 | **Irrelevant controls stay visible.** "Liquid surface", "Show particles" and "Keep particles" show for grid solvers, which have no particles. "Music" takes space with no track loaded. | Look, overlay, music |
| P6 | **Shortcuts are hidden.** They live in a collapsed "Controls" block at the very bottom; only Music and Liquid mention theirs. | Help |
| P7 | **Ragged layout.** egui's default slider puts the label on the right, so labels of different lengths make every row a different width. The four tool radio buttons are crammed into one row. | Everywhere |
| P8 | **The overlay legend is text** (`transparent: none white: full`) instead of a colour bar, and it sits far from the canvas it describes. | Overlay |
| P9 | **Metric health is invisible.** Metrics with tolerances (the invariant checks) are plain numbers; nothing tells you one has gone red. | Stats |
| P10 | **Fixed panel.** The panel can't be hidden. The canvas is fitted around a hard-coded `PANEL_WIDTH`, so any other layout (a toolbar or a status bar) would overlap the scene. | `main.rs`, `view.rs` |

## 2. Goals

1. **Play first.** The controls you touch every few seconds (pause, reset, tool, brush
   size, scene, solver) are one click away and always visible.
2. **Tune second.** Solver and look parameters are one level down, grouped, with the
   common ones first.
3. **Diagnose on demand.** Performance, metrics and the step breakdown are out of the
   way until asked for, but a problem (a metric over tolerance, slow motion) is still
   signalled at a glance.
4. **Discoverable.** Every action shows its shortcut, and a single key brings up a
   cheat sheet.
5. **Keep the architecture.** `ui.rs` still never names a solver; parameters still
   come from `Solver::visit_params`.
6. **Web-ready.** The layout should survive a narrow browser window (the deferred WASM
   build): a collapsible panel, and nothing wider than ~300 pt.

## 3. Proposed layout

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ ▶ ❚❚  ⏭  ↺   Speed ━━●━━ 1.0×  │ Scene [Dam break ▾]  Solver [FLIP ▾]  │  ☰  │ ← top bar
├────┬──────────────────────────────────────────────────────┬──────────────────┤
│ ✋ │                                                      │ Simulate View ♪ ⓘ│ ← tabs
│ ▦  │                                                      │──────────────────│
│ ⌫  │                                                      │ ▾ World          │
│ 💧 │                 simulation canvas                    │   Gravity  ◔ 9.8 │
│────│                                                      │   Tilt     ━●━ 0°│
│ ◯  │                                                      │ ▾ FLIP           │
│size│                                    ┌───────────┐     │   PIC/FLIP ━━●━  │
│ ━● │                                    │ Speed m/s │     │   CFL      ━●━━  │
│    │                                    │ ▁▂▃▅▇ 0–3 │     │ ▸ Pressure       │
│    │                                    └───────────┘     │ ▸ Advanced  ↻    │
├────┴──────────────────────────────────────────────────────┴──────────────────┤
│ 60 fps · 3.6 ms · 20 636 particles · t = 4.2 s            ● all checks OK    │ ← status bar
└──────────────────────────────────────────────────────────────────────────────┘
  tool strip (left)                          overlay legend (on canvas)
```

The glyphs are placeholders; see §6 on font coverage.

### 3.1 Top bar: run and choose

- **Transport:** Play/Pause (Space), Step (S, enabled while paused), Reset (R).
- **Speed:** a compact slider showing `1.0×`. Double-click resets it to 1×.
- **Scene and Solver:** labelled dropdowns. The solver dropdown shows a small
  "particles" or "grid" tag per entry, so the CPU/GPU and particle/grid families are
  obvious.
- **"Keep particles when switching"** moves into the solver dropdown as a checkbox at
  the bottom of the list, where it's relevant, instead of a permanent row.
- **☰** shows/hides the side panel (shortcut **(?)**: `` ` `` or F2; Tab is
  consumed by egui).

### 3.2 Tool strip: interact with the canvas

A narrow vertical strip on the canvas's left edge, as in paint programs.

- **Four tool buttons:** Push, Paint wall, Erase, Faucet. Shortcuts **1–4**, shown in
  the tooltip and as a small superscript.
- **Below the tools:** brush size (a vertical slider, also `[` / `]`). It's the one
  setting shared by every tool.
- **Tool-specific settings** (push strength, faucet speed) appear in a small popover
  next to the strip while that tool is selected, not in the panel.
- **Alternative (?):** put the tools in the top bar as a segmented control, and keep
  no strip.

### 3.3 Side panel: tabs instead of one long column

The side panel narrows from 340 to 300 pt. Its tabs are a row of selectable labels
(egui has no tab widget).

**Simulate** (default tab)
- **World:** gravity magnitude and tilt, with "Level". Optionally a gravity compass
  widget: drag a needle to set direction and strength in one gesture **(?)**.
- **One collapsible group per solver section,** generated from `visit_params`
  sections, all in an aligned two-column grid (label left, widget right).
  - Ordinary sections are open by default.
  - Sections marked advanced (§5.2) are collapsed.
- **Restart marker:** parameters that restart the simulation get one, and their
  tooltip says so.

**View**
- **Display:** liquid surface (L) with blob size and surface level; show particles (H).
  - **Particle solvers only:** hidden for grid solvers, replaced by one muted line
    ("Grid solver: showing the dye field").
- **Overlay:** field dropdown (O cycles), opacity.
  - The legend moves onto the canvas (bottom-right): a real colour bar with min/max
    and units, drawn with the same colour map as the overlay.
  - Categorical fields get swatches.

**Music** (♪)
- No track loaded: a single drop-zone hint ("Drop an audio file on the window").
- Track loaded: name and time, a seek bar, Play/Pause (M), Restart, Mute, a kick
  indicator, Splash/Jet, selectivity, strength.
- The tab label shows a dot while music is playing.

**Info** (ⓘ)
- **Performance:** fps, sim cost, substeps, slow-motion percentage.
- **Solver metrics:** each with a health dot: green within tolerance, amber above 80%
  of it, red over it.
- **Step breakdown:** a horizontal bar per phase, not a list of milliseconds.
- **Shortcuts reference:** the same list as the cheat sheet.

### 3.4 Status bar: always-on summary

One line under the canvas: fps · sim cost · particles (or "grid") · simulated time.

- **Right end:** a health summary.
  - "● all checks OK", or "● max lattice speed over tolerance" in amber/red.
  - Clicking it opens the Info tab.
- **Slow motion:** when the sim can't keep up, "slow motion 62%" appears in amber.

### 3.5 Cheat sheet

**F1** (or `?`) toggles a translucent overlay on the canvas, listing every shortcut and
mouse action in two columns. It replaces the "Controls" collapsible.

## 4. Visual style

- **Theme:** a dark theme tuned to the scene's palette.
  - Accent = the liquid blue (selected tool, active tab, slider fill).
  - Muted greys for inactive controls.
  - Amber and red reserved for warnings.
- **Typography:** slightly larger section headers (a 15-pt strong font); everything
  else stays at 13 pt. One heading style; no ad-hoc `strong()` calls.
- **Spacing:** a consistent 6-pt item spacing, 10 pt between groups, and rounded
  group frames instead of separator lines.
- **Forms:** every slider row is a two-column `Grid` with label on the left and a
  fixed-width slider with its value on the right. This fixes the ragged right edge
  (P7).
- **Contextual hiding:** controls that don't apply are hidden. If hiding would be
  confusing, they're disabled with a one-line reason.

## 5. Architecture changes

### 5.1 Canvas rect comes from egui (fixes P10)

Today `main.rs` fits the view to `window width − PANEL_WIDTH`. Instead:
- After egui lays out the top bar, status bar, tool strip and side panel, the
  remaining central rect (`ctx.available_rect()`, or a `CentralPanel`'s rect) becomes
  the canvas.
- `View::fit` and the renderer's viewport/scissor use that rect, so hiding the panel
  or resizing the window just works.
- The brush-outline clip (the last commit) uses the same rect.

### 5.2 Advanced parameter sections (small `sim` API addition)

To collapse rarely-used groups:
- Add `ParamVisitor::advanced_section(&mut self, title)`, whose default calls
  `section`.
- Solvers move expert knobs under it: lattice speed, max steps per frame, and
  separation iterations, for example. Each solver opts in.
- It stays UI-agnostic: "advanced" is a property of the parameter, not of egui.

### 5.3 Module split

`ui.rs` (420 lines) becomes `ui/`:
- `mod.rs`: `UiState`, the top-level layout;
- `top_bar.rs`, `tool_strip.rs`, `status_bar.rs`;
- `panel/`: `simulate.rs`, `view.rs`, `music.rs`, `info.rs`;
- `widgets.rs`: param grid rows, health dot, colour-bar legend, and possibly the
  gravity compass;
- `theme.rs`: the egui `Style`/`Visuals` setup.

### 5.4 Keyboard

All shortcuts move into one table (`ui/shortcuts.rs`: key, action, label). Both the
key handler in `main.rs` and the cheat sheet read it, so they can't drift apart.

New shortcuts: 1–4 for tools, `[` / `]` for brush size, F1 for the cheat sheet, and
the panel toggle.

## 6. Risks and constraints

- **Glyphs.** egui's default fonts lack many symbols (the "●" kick dot already failed).
  Either embed a small icon font (e.g. a subset of Phosphor or Material Symbols as
  TTF, ~50 KB, loaded via `FontDefinitions`), or use text labels. Recommendation:
  embed icons; text-only tool buttons won't fit a narrow strip.
- **egui 0.36 specifics.** It uses `Panel::top/bottom/left/right` and `run_ui`, as
  found in Phase 2. Overlays on the canvas (tool strip, legend, cheat sheet) are
  `egui::Area`s anchored to the canvas rect.
- **Input routing.** Tool strokes must not start on the new on-canvas widgets. The
  existing `ui_owns_it` check covers egui areas, but it needs a test with the strip
  and the legend.
- **Testing.** UI layout isn't unit-tested today. Plan: keep logic (shortcut table,
  health classification, legend ticks) in plain functions with unit tests, and verify
  the layout by screenshots at 1080p and 1440×900, and with the panel hidden.

## 7. Implementation phases

Each phase leaves the app fully working and is committed separately.

| Phase | Scope | Fixes |
|---|---|---|
| **A. Skeleton** (done) | Split `ui.rs` into modules. Canvas rect from egui (§5.1). Top bar, status bar, tabbed side panel. Move the existing widgets into place, otherwise unchanged. | P1, P2, P3, P4, P10 |
| **B. Forms and theme** (done) | Theme, two-column param grids, group frames, labelled dropdowns, restart markers. | P7 |
| **C. Context and discoverability** (done) | Hide controls that don't apply. Music drop-zone state. Shortcut table, cheat sheet, shortcut hints in tooltips, new shortcuts. | P5, P6 |
| **D. Canvas widgets** | Tool strip with icon font, brush size, tool popover. Overlay colour-bar legend on the canvas. | P8 |
| **E. Health and diagnostics** | Metric health dots, status-bar summary, step-breakdown bars. Advanced param sections (§5.2). | P9 |
| **F. Optional extras** | Gravity compass, panel hide/show, persisting UI settings between runs **(?)**. | — |

A and B are the bulk of the visible improvement. C–E are each a focused session.

## 8. Decisions for you

1. **Tools:** a vertical strip on the canvas (recommended), or a segmented control in
   the top bar?
2. **Panel side:** keep it on the right (recommended; it matches today), or move it
   left with the tool strip on the right?
3. **Panel structure:** tabs (recommended; short panel, no scrolling), or one column
   of collapsible sections (everything visible at once, but long)?
4. **Icons:** embed an icon font (recommended), or stay text-only?
5. **Gravity compass:** worth building, or keep two sliders?
6. **Stats visibility:** status bar plus Info tab (recommended), or keep a compact stats
   block visible in the panel?
7. **Persistence:** remember UI settings (tool, brush, look, panel state) between runs?
   It adds a small config file and serde; the WASM build would use local storage.
8. **Panel toggle key:** `` ` ``, F2, or another?
