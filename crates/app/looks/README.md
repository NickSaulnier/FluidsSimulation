# Looks

A look is one TOML file: the fluid, how the picture carries over from frame to frame,
what's drawn from the sound, how the music pushes the fluid, and equations that drive it
all. The files here are the built-in looks, compiled into the app.

Your own looks go in the `looks` folder beside the settings file (on Windows,
`%APPDATA%\fluid-toy\looks`). The Music tab's Looks group opens that folder, and can copy
the built-ins there to start from. Its "Save as" row saves the settings as they stand as
a look there: the fluid (solver, the scene as last loaded, every parameter at its own
value, not as the music moves it), the colours, feedback, waves and reactions, and the
equations of the look on screen. Saving under one of your looks' names writes over it;
under a built-in's name, it becomes your own version in its place. The app checks the
folder every second: a changed
file is read again, and if it's the look on screen, the change shows at once. Only a
change to the fluid (solver, scene, parameters) restarts the scene. A look named like a
built-in replaces it. A file with a mistake is skipped, and the Music tab lists it with
the line and what's wrong.

```toml
[meta]
name = "Spiral ink"        # what the look is called (unique)
author = "you"
tags = ["ink", "spiral"]
energy = "energetic"       # calm, energetic or any: when the director picks it

[sim]
solver = "Stable fluids (dye)"
scene = "Double dam break"
palette = "Ink"
liquid = false             # the liquid surface, for particle solvers
orbit = 0.0                # 3D looks: the camera's circling speed, radians per second
params = { buoyancy = 0.2, vorticity = 0.3 }
reactions = [
    { trigger = "snare", effect = "vortex", gain = 1.4 },
    { trigger = "bass", effect = "param", param = "buoyancy", gain = 0.8 },
]

[feedback]                 # present: feedback on
mode = "brightest"
trail = 0.5
rotation = 0.15

[waves]
mode = "circle"
size = 0.45

[[shape]]                  # up to two
sides = 6
radius = 0.3

[art]                      # the song's cover
size = 0.25

[fractal]                  # an escape-time fractal
kind = "julia"
wander = 1.0
dive = 0.5

[composite]
symmetry = "kaleidoscope"
segments = 6
vignette = 0.4

[code]
init = "phase = 0"
frame = """
phase = phase + dt * bass_att
rot = 0.15 + 0.1 * sin(phase)
"""
pixel = """
rot = rot * (0.5 + rad)
"""
```

The app's look browser (B, or the grid button in the Looks group) finds looks by name and
by `tags`, and keeps your stars and favourites (in the settings, by the look's name).
When looks change by themselves, better-rated looks come round more often, a one-star
look is never picked, and the browser can keep them to your favourites or to the tags
picked there. Its equation editor (E, or the function button) edits
the `[code]` of the look on screen while it runs; Save there writes the look as Save as
does, under its own name.

## Classic MilkDrop presets

Drop `.milk` files (or a folder of them) on the window, or run `fluid-toy --import
<file or folder>`, to convert them into looks in your folder (`milk_<name>.toml`, over
an earlier import of the same preset). Their warp, decay, echo, waveform (the nearest
of our modes, coloured by the wave's colour), first two shapes and equations come
across; the equations run between lines that convert our values to MilkDrop's units
(per frame at 60 a second, y down) and back. MilkDrop 2 shaders, custom waves, shape
equations, borders and motion vectors have no equivalent and are left out, and listed
at the top of the file, as are equations that don't compile.

## `[sim]`

- `solver`, `scene`: names as the app shows them. A 3D solver (`Smoke (3D GPU)`,
  `Fire (3D GPU)`) needs a 3D scene. Leave both out for a look without fluid: the
  picture is then only the waves and shapes, carried by the feedback (`params`,
  `liquid` and `orbit` need a fluid; reactions can still flash and turn the colours).
- `palette`: a palette's name (`Ocean`, `Lava`, `Slime`, `Ink`, `Mercury`), or its
  colours: `{ deep = [r, g, b], base = [r, g, b], highlight = [r, g, b] }` (0 to 255).
- `params`: solver parameters by key: the keys the Simulate tab shows for the solver.
  A toggle is on unless 0.
- `reactions`: each one `trigger`, `effect`, `gain` (1 if left out) and, for
  `effect = "param"`, the parameter's key. `enabled = false` keeps one off.
  - Triggers: `kick`, `snare`, `hat`, `beat` (every beat of a steady beat, even a quiet
    one), `sub`, `bass`, `lowmid`, `mid`, `high`, `level`, `brightness`.
  - Effects: `splash`, `jet`, `vortex`, `flash`, `gust`, `slosh`, `breathe`, `hue`,
    `param`, `drop` (a drop at a random spot: ripples on shallow water), `morph`
    (kicks the fractal: a Julia set's shape leaps on, and the zoom punches in).

## `[feedback]`

Each frame starts from the last one, warped and faded, with the new scene over it.
Rates are per second.

| Setting | Meaning |
|---|---|
| `mode` | `smear` (a blend toward the scene) or `brightest` (bright things streak) |
| `trail` | how much of the picture is left after a second, 0 to 0.98 |
| `zoom` | growth per second: 1 none, above 1 grows from the centre |
| `rotation` | turn, radians per second |
| `drift` | `[x, y]` shift, screen heights per second |
| `warp`, `warp_speed` | a slow wobble: amount (screen heights) and speed |
| `flow` | how strongly the fluid's own motion carries the picture (1 exactly with it) |
| `bass_zoom` | extra zoom with the bass, 0 to 1 |
| `echo`, `echo_zoom`, `echo_flip` | a zoomed copy over the picture: amount, zoom, and `none`, `horizontal`, `vertical` or `both` |
| `enabled` | `false` to keep feedback off with the section present |

## `[waves]` and `[[shape]]`

- `[waves]`: `mode` (`off`, `line`, `circle`, `spectrum`, `mirror_spectrum`, `xy`),
  `scale` (how far the sound moves it), `size` (circle radius or bar height), `y` (where
  a line sits), `thickness` (pixels), `brightness` (above 1 glows), `push` (how hard a
  line or circle pushes the fluid, 0 to 1).
- `[[shape]]`: `sides`, `radius`, `centre` (`[x, y]`), `spin` (radians per second),
  `thickness`, `bass_pulse` (how much the radius follows the bass), `fill` (0 to 1).
  Sizes are in half the picture's height; `[0, 0]` is the middle.

## `[art]`

The song's cover art, drawn into the picture before the feedback, so the warp carries it
(present: on). There's a cover while listening live to a player that reports one
(Spotify, a browser, a media player); otherwise nothing is drawn. `size` (half the
cover's longer side, in half the picture's height), `opacity` (0 to 1), `centre`
(`[x, y]`), `spin` (radians per second), `bass_pulse` (how much the size follows the
bass), `brightness` (above 1 glows).

## `[fractal]`

An escape-time fractal in the palette's colours, drawn into the picture before the
feedback, so the warp carries it (present: on). It's opaque near the set's edge and clear
far outside and inside, so the fluid shows through.

- `kind`: `mandelbrot`, `julia`, `burning_ship`, `tricorn` or `newton` (Newton's method on
  z³ − 1, coloured by the root each point finds).
- Where: `centre` (`[x, y]`, the point of the plane in the middle), `zoom` (1: the
  picture is 3 units high; up to 100000, as far as the shader's precision goes),
  `rotation` (radians), `spin` (radians per second).
- `julia`: the Julia set's point `c` (`[re, im]`).
- `iterations` (up to 1000: finer detail when zoomed in, and more work), `bands` (how
  many times the palette repeats toward the edge), `colour_speed` (palette cycles per
  second), `opacity`, `brightness` (above 1 glows).
- With the music: `wander` (a Julia set's point travels along the Mandelbrot set's edge,
  about laps a minute at full loudness, starting from the edge nearest `julia`),
  `bass_zoom` (0 to 1: the zoom follows the bass), `tour = true` (at each new section,
  move to another place worth seeing; a Julia set changes shape).
- `dive`: travel endlessly into the fractal (negative: out of it), in zoom doublings a
  second (from 0.4 times that in silence to 1.6 times when loud, and up to 4 times on a
  `morph` kick). It flies from `centre` and `zoom` to the nearest point where the
  picture repeats itself (a Julia set's tip, Newton's star at 0, one of nine for the
  Mandelbrot set and two each for the Burning Ship and Tricorn), then loops there
  forever, the colours keeping their place. With `tour`, each new section flies on to
  another point.

## `[composite]`

The picture's last touches, drawn on the screen and never fed back. All off by default.

| Setting | Meaning |
|---|---|
| `symmetry` | `none`, `mirror_x` (the left half mirrored onto the right), `mirror_y` (the top onto the bottom), `quad` (the top-left quarter into all four), `kaleidoscope` |
| `segments`, `symmetry_spin` | the kaleidoscope's mirrored wedges (2 to 16) and its turn (radians per second) |
| `hue`, `hue_speed` | turn every colour round the colour wheel (degrees), and keep turning (degrees per second) |
| `saturation`, `contrast`, `exposure` | 1 as is: saturation 0 is grey, contrast is around mid-grey, exposure a multiple |
| `palette_map` | recolour the picture by its brightness with the look's palette (dark to deep, base, highlight), 0 to 1 |
| `invert` | 0 to 1 |
| `aberration` | colour fringes toward the edges, 0 to 0.05 |
| `vignette`, `grain` | darker corners (0 to 1), film grain (0 to 0.3) |

## `[code]`: equations

Three programs, all optional: `init` runs once when the look starts, `frame` every
frame, and `pixel` for every pixel of the feedback's warp (on the GPU). A program is
assignments, one per line or separated by `;`, with `//` comments:

```
zoom = 1 + 0.05 * bass_att
rot = if(beat_phase < 0.5, 0.2, -0.2)
```

Values are numbers. There are `+ - * / %`, comparisons (`< <= > >= == !=`), `&&`, `||`
and `!`, which give 1 or 0 (anything but 0 counts as true), and parentheses. For powers
use `pow(a, b)`. Dividing by 0 gives 0. Names don't care about case.

Functions: `sin cos tan asin acos atan atan2(y, x) abs sqrt pow(a, b) exp log log10
min(a, b) max(a, b) floor ceil round fract sign clamp(x, lo, hi) sigmoid(x, k)
if(c, a, b) above(a, b) below(a, b) equal(a, b)`, and `rand()` (0 to 1, per frame only).
MilkDrop's are there too, for its presets: `sqr(x)`, `int(x)` (toward zero), `band(a, b)`,
`bor(a, b)`, `bnot(a)` (1 or 0), and `rand(n)` (a whole number from 0 up to n, per frame
only).
`sqrt`, `log` and `pow` keep to numbers they can take (`sqrt(-4)` is 2).

### What every program reads

| Name | Meaning |
|---|---|
| `time`, `frame` | seconds and frames since the look started |
| `dt`, `fps` | this frame's length, and frames per second |
| `aspect` | the picture's width over its height |
| `bass`, `mid`, `treb` | MilkDrop's levels: each band over its own running average (1 is typical for the song) |
| `bass_att`, `mid_att`, `treb_att` | the same, smoothed |
| `band_sub`, `band_bass`, `band_lowmid`, `band_mid`, `band_high` | the five bands, 0 to 1 |
| `level`, `brightness` | loudness (relative, 0 to 1) and how bright the sound is (0 to 1) |
| `loudness` | EBU R128 loudness, LUFS |
| `kick`, `snare`, `hat`, `beat` | 1 when one happens, falling to 0 over about a third of a second |
| `beat_phase`, `bar_phase` | 0 to 1 from one beat (or bar) to the next |
| `bpm`, `beating` | the tempo, and 1 while there's a steady beat |
| `silent` | 1 in silence |
| `novelty` | how different the last two seconds are from the eight before: about 1 for music carrying on, 3 or more at a change of section |
| `buildup` | how much the music is building up (rising loudness and busier hits), 0 to 1 |
| `section`, `drop`, `breakdown` | 1 at a change of section (any; a drop; a breakdown), falling to 0 over about a second and a half |

### What they set

The warp, per frame and per pixel (each starts from the `[feedback]` settings):

| Name | Meaning |
|---|---|
| `zoom` | growth per second (1 none) |
| `rot` | turn, radians per second |
| `dx`, `dy` | shift, screen heights per second |
| `cx`, `cy` | the centre of zoom and turn, 0 to 1 (0.5 the middle; y up) |
| `sx`, `sy` | stretch per second (1 none) |
| `warp` | the wobble's amount |

Per frame only:
- `trail`, `warp_speed`, `flow`, `echo`, `echo_zoom` (as in `[feedback]`);
- `wave_scale`, `wave_size`, `wave_y`, `wave_thickness`, `wave_brightness`, `wave_push`;
- `shape1_radius`, `shape1_x`, `shape1_y`, `shape1_spin`, `shape1_fill` (and `shape2_`);
- `art_size`, `art_opacity`, `art_x`, `art_y`, `art_spin` (the cover);
- `fractal_x`, `fractal_y`, `fractal_zoom`, `fractal_rot`, `fractal_iter`,
  `fractal_opacity`, `julia_re`, `julia_im` (the fractal, after its own movement with
  the music: `fractal_zoom = fractal_zoom * (1 + 0.5 * kick)`);
- `gravity` (degrees anticlockwise from straight down, any angle), `gravity_strength`
  (a multiple: 1 as set, up to 5), `intensity` (the reactions' strength, a multiple: 1
  as set);
- the composite, by its settings' names: `segments`, `symmetry_spin`, `hue`,
  `hue_speed`, `saturation`, `contrast`, `exposure`, `palette_map`, `invert`,
  `aberration`, `vignette`, `grain` (`hue = 40 * sin(time * 0.1)`, `vignette = 0.3 + 0.4
  * beat`);
- `param_<key>`: a solver parameter (`param_buoyancy = 0.2 + 0.2 * bass_att`);
- `q1` to `q8`: passed on to the per-pixel program;
- any other name: the program's own variable, kept from frame to frame (`init` and
  `frame` share them).

Per pixel, also read `x` and `y` (0 to 1, y up), `rad` (the distance from the centre, 1
at the top and bottom edges) and `ang` (the angle, radians, anticlockwise from the
right), plus `q1` to `q8`. Any other name assigned there is a temporary for that pixel.

A name that's read but never assigned, and isn't one of these, is reported as a mistake
(with a suggestion if it's close to one).
