# Fidelity Plan

Status: **phases A to E done** (2026-10-31); decisions 1 and 5 made, 2 to 4 open (§4). A sharper, cleaner
picture: fewer artefacts (banding, blur, shimmer), truer colour, and more detail in the
fluids, at the frame rates we have now. Grouped into phases; each phase is committed on
its own.

## 1. Goal

What's on screen in full screen should hold up close on a large display:
- Smooth gradients, with no banding in dark looks, glows or vignettes.
- Bright light that fades toward white the way film does, keeping its hue.
- Trails that keep their detail instead of softening a little every frame.
- Fractals that stay still while they move, without shimmer in the filaments.
- Fluids with more detail and some body: lit dye, liquid that looks wet, smoke that
  shadows itself.
- A way to measure all this, so each change is checked on the contact sheet, as the
  visuals phases were.

## 2. Where things stand

| Piece | Today |
|---|---|
| Output | The scene, the feedback frames and the bloom levels are `Rgba16Float` (`render/post.rs`, `SCENE_FORMAT`); the window is an 8-bit sRGB surface (`gpu.rs`). Nothing dithers on the way down, so smooth gradients band. |
| Tone curve | Per channel: unchanged up to a knee at 0.85, then Reinhard-style toward 1 (`post.wgsl`, `tone`). Per channel means a bright saturated colour shifts hue as its channels flatten one by one (bright orange fire drifts toward yellow) and never quite reaches white. |
| Colour maths | Palettes are three colours blended in RGB (`post.wgsl`, `gradient`; `render/palette.rs`). The hue turns round RGB's grey axis (`hue_turn`), which changes how bright a colour looks as it turns. Harmony (visuals phase F) now turns the hue all the time. |
| Feedback | Each frame samples the last one bilinearly at the warped place (`feedback.wgsl`). A bilinear sample between pixels is a small blur; repeated every frame, fine detail in a trail is gone within a second or two, which is part of why long trails look soft. |
| Fractals | Four supersamples on a rotated grid, dropping to one when frames fall behind (`fractal.wgsl`, `samples`). At one sample, and in the fine filaments of a dive even at four, they shimmer. |
| Resolution | The picture renders at a render scale and is stretched to the window; the governor lowers the resolution in full screen when the simulation falls behind (`governor.rs`). The stretch is a plain filter. |
| 2D fluids | Stable fluids (GPU) runs at three times the CPU resolution; its dye, and the CPU stable, fire and LBM solvers' dye, use MacCormack advection already. Smoke & fire, Lattice Boltzmann and shallow water run on the CPU, at grids sized for it. Dye is drawn flat, through the palette. |
| Particle liquid | A 2D screen-space surface: particles splatted as Gaussians into a density field, thresholded and shaded (`render/liquid.rs`). No refraction of what's behind, no depth. |
| 3D volumes | Raymarched (`render3d/march.wgsl`) with a jittered start per pixel (so no step banding, but grain instead), and a short shadow probe as a stand-in for real self-shadowing. |
| Measures | `render/score.rs` scores dark, blown out, detail, colour, coverage, contrast, motion and following. Nothing measures banding, shimmer or softening. |

## 3. The work

### 3.1 Measuring fidelity

- **New measures** in `render/score.rs`, from the audit's grabs (full-resolution crops,
  not only the small frames: banding and shimmer don't survive downscaling):
  - **Banding:** in smooth regions (low gradient), the share of pixels on long runs of
    one value with a step of one level at their ends.
  - **Shimmer:** in still regions of a look whose motion is known (a fractal, the
    feedback's warp), the change from frame to frame that the motion doesn't explain.
  - **Softening:** the detail measure (already there) along a trail, as the trail ages:
    how fast high frequencies fall.
- **On the contact sheet** as columns, and in the score's quality only once the
  thresholds are tuned against looks judged by eye.
- **A fidelity reel:** a handful of looks chosen to stress each artefact (a dark smoke
  look, a long-trail look, a deep fractal dive, a fire look, a liquid look), audited
  before and after each phase.

### 3.2 Output: dithering, the tone curve, perceptual colour

- **Dithering:** blue noise (a small tiling texture, its offset changing each frame)
  added before the colour is written to the 8-bit surface, about one level in size.
  Also when grabbing screenshots and recordings.
- **The tone curve on brightness, not per channel:** map the luminance through the curve
  and scale the colour to match, then let colour above white bleed toward white in a
  controlled way (as AgX and similar curves do), so bright colours keep their hue and
  the brightest go white. Looks are tuned to today's curve, so it changes every look a
  little (decision 1).
- **OKLab and OKLCH:** blend palette colours in OKLab, and turn the hue in OKLCH (with the
  lightness kept), in `post.wgsl` and wherever palettes are blended on the CPU (`palette.rs`,
  `modulated`). Gradients stay even in brightness; harmony's hue turns stop pulsing in
  brightness.

### 3.3 Sharper feedback

- **A sharper sample of the last frame:** Catmull-Rom (bicubic, nine bilinear taps with
  the usual trick) in place of one bilinear tap, in the warp. Catmull-Rom overshoots a
  little at edges; clamp to the neighbourhood's range so trails don't ring.
- **Optional, per look:** a `sharpness` setting in `[feedback]` (0 bilinear, as now, to
  1 Catmull-Rom), so looks that rely on the softness (smoky smears) keep it. Default to
  be decided from the contact sheet.
- **Cost:** the warp pass runs at the render resolution once a frame; nine taps instead
  of one is affordable, and measured before it's made the default.

### 3.4 Stable fractals

- **Samples across frames:** each frame takes its samples at new sub-pixel offsets, and
  blends with the last frame reprojected by the fractal's own motion (its zoom, centre
  and rotation are known each frame, so the reprojection is exact for a 2D fractal).
  Rejected where the reprojected history differs too much from this frame's (a Julia
  set's shape changing, a morph kick), as temporal antialiasing does.
- **The effect:** many samples' worth of smoothing at the cost of one or four, so the
  one-sample fallback no longer shimmers.
- **3D fractals** (Mandelbulb, Menger) move with an orbiting camera; reprojection there
  needs depth. Left for later unless it comes cheaply with §3.7.

### 3.5 Fluid detail

- **Lit dye:** treat the dye's density as a height field, take its gradient as a normal,
  and light it (a soft key light and a rim), so ink and smoke have body instead of being
  a flat colour map. A per-look amount (`[sim] relief`?), 0 for today's flat look.
- **Smoke & fire on the GPU:** the stable solver's GPU port shows what a grid at three
  times the resolution does at full screen; fire would gain the most. Ported on the GPU
  stable solver's pattern (`sim-gpu`), with fuel, temperature and burning as the CPU
  solver has them, validated against it at the CPU's resolution (decision 3).
- **Then, if worth it:** shallow water and Lattice Boltzmann on the GPU, in that order
  (shallow water is the simpler, and its ripples suffer most at low resolution).

### 3.6 Liquid that looks wet

- **From the density field the liquid already has:** smooth it (a bilateral or Gaussian
  blur at its resolution), take normals from its gradient, and shade with:
  - refraction of the picture behind (offset by the normal; the scene is there to
    sample),
  - a Fresnel-weighted reflection of a simple environment (a gradient, or the palette),
  - a thickness tint (Beer's law, deeper is darker and more saturated).
- **For PBF, PBF (GPU) and FLIP's fountain**, the particle fluids the visualizer shows.

### 3.7 3D volumes

- **Real self-shadowing:** march toward the light into a low-resolution light volume
  once a frame (or every other), instead of the short probe per sample; smoke then
  shadows itself and the flame lights the smoke around it.
- **Temporal accumulation of the jitter:** the jittered start already removes banding
  but leaves grain; blend with the last frame, reprojected by the camera's orbit and
  each pixel's depth along the ray, to average the grain away.
- **Fire's colour:** check the flame's emission against a blackbody ramp in linear
  light, with the tone curve of §3.2 doing the rest.

### 3.8 Extras

- **Temporal upscaling** when the governor or the render scale lowers the resolution:
  render with sub-pixel jitter and accumulate into a full-resolution history (as §3.4,
  for the whole picture), instead of a plain stretch.
- **Motion blur along the flow:** a few taps along the fluid's screen-space flow (the
  feedback already has it) at the end, so fast motion looks continuous.
- **HDR output:** an HDR surface (`Rgba16Float`, scRGB) where Windows reports an HDR
  display, with the tone curve mapping to the display's peak instead of to white
  (decision 2).

## 4. Decisions (1 and 5 made)

1. **The tone curve (§3.2):** the same compression, on the brightest channel instead of
   each, with a gradual turn toward white (Khronos' PBR Neutral, without its toe), so
   colours under the knee are as they were; compared on the reel's fire looks before
   switching (see phase B in §6). No look needed retuning.
2. **HDR output (§3.8):** in scope? It needs an HDR display to see and to test.
   Recommended: last, if the user has one.
3. **Which fluids move to the GPU (§3.5):** Smoke & fire is the largest single piece of
   work in the plan. Recommended: Smoke & fire only, then decide on the others.
4. **The frame budget:** the target (60 fps at 1080p or 1440p? 4K?) and the GPU it's
   measured on decide how far §3.3, §3.4 and §3.7 can go by default. Recommended: the
   user's own machine at its display's resolution, with the governor as the fallback.
5. **Per-look settings or global (§3.3, §3.5):** look settings, with defaults from the
   contact sheet. Sharpness (phase C): per look, default 1 (sharp), MilkDrop's
   converted presets 0. Relief (phase E): per look, default 0 (flat, as before).

## 5. Phases

| Phase | Scope | Visible result |
|---|---|---|
| **A. Measures** | Banding, shimmer and softening scores, full-resolution crops, the fidelity reel (§3.1). | A sheet that shows each artefact, worst first. |
| **B. Output** | Dithering, the tone curve, OKLab palettes and OKLCH hue (§3.2). | No banding in dark looks; bright colours keep their hue. |
| **C. Sharp feedback** | Catmull-Rom warp sampling, clamped, and the sharpness setting (§3.3). | Long trails that keep their detail. |
| **D. Stable fractals** | Samples across frames with reprojection (§3.4). | Dives without shimmer. |
| **E. Lit dye** | Relief lighting from the dye's density (§3.5). | Ink and smoke with body. |
| **F. Wet liquid** | Smoothed surface, refraction, Fresnel, thickness (§3.6). | Particle fluids that look like liquid. |
| **G. 3D volumes** | A light volume for self-shadowing, temporal accumulation (§3.7). | Smoke that shadows itself, without grain. |
| **H. GPU fire** (optional) | Smoke & fire ported to the GPU (§3.5). | Fire at three times the resolution. |
| **I. Extras** (optional) | Temporal upscaling, flow motion blur, HDR output (§3.8). | Sharp at a lowered resolution; HDR on HDR screens. |

Suggested order: A first (it measures everything after), then B and C (small shader
changes every look shows), then D to G; H and I if chosen. Each phase ends with the
fidelity reel audited, against the run before, and its frame time measured.

## 6. Progress

- **Phase A, measures (done):** `render/score.rs` (`fidelity`, `shimmer`), `audit.rs`.
  - **Full-size frames:** every 3 s from a second after the warm-up, the audit grabs two
    frames in a row at the window's size (1280 × 720), measured as they come back and
    not kept (about 22 frames a look).
  - **Banding:** the share of the frame in terraces, across or down: runs of one colour
    at least 6 pixels long next to another a step of 1 or 2 levels away. A dark
    gradient on 8 bits measures 90 % and more; the same dithered under 20 %; flat
    black 0.
  - **Sharpness** (in place of the plan's softening along a trail, which needs to know
    where a trail is): the fine detail (luma less its 3 × 3 mean) over the same at a
    quarter of the size, times 4. Sharp squares measure about 1, the same blurred well
    under half that, noise over 1.5. It's for comparing a look with itself, before and
    after a change: its level depends on what's drawn.
  - **Shimmer:** between the two frames of a pair, the change in fine detail where the
    coarse picture moved less than 2 levels, over the fine detail there (counted only
    where there's at least 2 levels of it, and only if that's 1 % of the frame).
    Random detail redrawn each frame measures over 0.8; the same frame twice 0; a
    smooth pattern moving a tenth of a pixel under 0.3. Smooth looks (fire on black,
    smoke) often have too little still detail to tell, and show none.
  - **The sheet** has three more columns (banding, sharpness, shimmer, with what each
    means on hovering the heading); none counts in the quality yet.
  - **The reel:** `--audit-reel` audits eight looks: Smoke dance, Ink nebula, Aurora,
    Spiral galaxy, Julia tunnel, Firestorm, Bonfire 3D, Quicksilver. Looks with film
    grain are left out: grain reads as noise (high sharpness, shimmer over 1) and hides
    banding as a dither does (Ghost smoke and Ember drift, tried first, measured 0 %
    banding and shimmer 1.2).
  - **`--audit-looks`** takes several names, separated by commas.
  - **The baseline** (the anthem, 36 s each; the measures to beat in phases B to G):

    | Look | Quality | Banding | Sharpness | Shimmer |
    |---|---|---|---|---|
    | Smoke dance | 87 | 37 % | 1.40 | – |
    | Ink nebula | 100 | 9 % | 0.88 | 0.05 |
    | Aurora | 94 | 64 % | 1.11 | 0.04 |
    | Spiral galaxy | 97 | 48 % | 2.20 | 0.08 |
    | Julia tunnel | 100 | 19 % | 3.19 | 0.90 |
    | Firestorm | 93 | 21 % | 1.13 | – |
    | Bonfire 3D | 100 | 45 % | 1.39 | – |
    | Quicksilver | 100 | 10 % | 1.39 | 0.33 |

    What it says: banding is widespread in the dark and trailing looks (Aurora 64 %,
    Spiral galaxy 48 %, Bonfire 3D 45 %), which phase B's dithering should take to near
    0; the Julia dive shimmers (0.90), phase D's target; the liquid flickers a little
    (0.33).
  - **Checked:** 245 app tests (new: banding finds terraces and not dithered or flat
    frames, sharpness tells sharp from soft and noise, shimmer finds flickering detail
    and not still or smoothly moving pictures) and clippy pass; the reel audited
    (above, no errors).
- **Phase B, output (done):** `render/post.wgsl`, `render/post.rs`, `render/palette.rs`.
  - **Found on the way:** today's tone curve was already Khronos PBR Neutral's
    compression (knee at 0.85, the same Reinhard-style roll-off), applied to each channel
    on its own; that's what turned bright orange toward yellow.
  - **The tone curve:** applied to the brightest channel, the others scaled with it (hue
    and saturation kept), then turned toward white by how much it was compressed
    (`1 - 1 / (0.4 × compressed + 1)`), the turn made in OKLab so the hue holds as it
    pales (mixed in RGB, an orange went pink). Colours whose brightest channel is under
    the knee are untouched, as before. Checked: an orange at 1.5 keeps its OKLab hue
    within 4°; one at 12 comes out a warm white. On the reel, Bonfire 3D's cores are
    peach-white with orange edges instead of yellow-white; Firestorm, less bright,
    looks the same.
  - **OKLab and OKLCH:** the composite's hue turn (harmony's, the hue drift, the Hue
    setting) turns in OKLCH, keeping lightness and chroma (a red turned 120° is a green
    as light, not the brighter green of before); the palette map blends deep, base and
    highlight in OKLab; the reactions' palette hue shift (`Palette::modulated`) turns in
    OKLCH on the CPU instead of HSV. The CPU colour maps for fields and the dye ramp
    (`render/colormap.rs`, blended in sRGB, close to perceptual already) are as they
    were.
  - **Dithering:** the last step of the composite: in the target's sRGB encoding,
    triangular noise of about a level each way (two interleaved-gradient noises at the
    frame's offsets, so it moves and averages out), then back to linear for the target
    to encode. Black stays black. Only into an 8-bit target (the screen, screenshots,
    recordings, thumbnails); float targets (the GPU tests) aren't. Interleaved gradient
    noise rather than a blue-noise texture: no texture to make or bind, and as even.
  - **The measures** now ignore detail, and changes, under a level and a half: the
    dither's noise had read as detail (sharpness up to 7) and flicker. So the baseline
    below replaces phase A's for sharpness and shimmer; banding is measured as before.
  - **The reel after** (banding against phase A's):

    | Look | Quality | Banding (was) | Sharpness | Shimmer |
    |---|---|---|---|---|
    | Smoke dance | 100 | 0.3 % (37 %) | 0.07 | 0.00 |
    | Ink nebula | 100 | 0.1 % (9 %) | 0.61 | 0.00 |
    | Aurora | 94 | 0.1 % (64 %) | 0.85 | 0.00 |
    | Spiral galaxy | 97 | 0.5 % (48 %) | 0.70 | 0.02 |
    | Julia tunnel | 100 | 0.2 % (19 %) | 3.25 | 0.81 |
    | Firestorm | 95 | 0.1 % (21 %) | 0.16 | – |
    | Bonfire 3D | 100 | 0.1 % (45 %) | 0.04 | – |
    | Quicksilver | 100 | 0.0 % (10 %) | 0.64 | 0.16 |

    Banding is gone; the Julia dive still shimmers (phase D); sharpness is low for the
    smooth fluids, as they are (phase C will show whether the feedback softens them).
  - **Checked:** 246 app tests (new: the tone curve keeps an orange's hue and whitens a
    blazing one; into an 8-bit target a dark gradient comes out without terraces and
    black stays black; changed: a hue turn keeps the lightness, in the composite and on
    the CPU) and clippy pass; the reel audited (above, no errors).
- **Phase C, sharp feedback (done):** `render/feedback.wgsl`, `render/feedback.rs`.
  - **The read:** the warp reads the last frame with Catmull-Rom in nine bilinear taps
    (each row's two middle weights folded into one tap between them), clamped to the
    range of the four nearest texels so edges don't ring and overshoot can't build up
    frame on frame; mixed with the plain bilinear read by `sharpness`.
  - **`sharpness`** in `[feedback]` (0 to 1, default 1) and a slider in the View tab's
    feedback group, under Trail. Converted MilkDrop presets take 0: MilkDrop reads its
    last frame bilinearly, and its presets are made for that blur. Random looks take the
    default.
  - **Why it matters, measured:** stripes 8 pixels apart carried a third of a pixel a
    frame for a second keep 0.43 of their contrast read sharply, 0.02 bilinearly (a
    bilinear read at that shift keeps 93 % of that detail a frame, Catmull-Rom 99 %).
    Detail finer than that fades either way.
  - **The reel** (sharpness 1 against phase B's bilinear run; Ink nebula and Julia
    tunnel have no feedback, so theirs is the run's own variation):

    | Look | Sharpness | Shimmer | Detail (small frames) |
    |---|---|---|---|
    | Smoke dance | 0.07 → 0.12 | 0.00 → 0.00 | 0.012 → 0.016 |
    | Ink nebula | 0.61 → 0.57 | 0.00 → 0.00 | 0.034 → 0.034 |
    | Aurora | 0.85 → 1.11 | 0.00 → 0.01 | 0.017 → 0.018 |
    | Spiral galaxy | 0.70 → 0.96 | 0.02 → 0.01 | 0.010 → 0.013 |
    | Julia tunnel | 3.25 → 2.69 | 0.81 → 0.72 | 0.050 → 0.050 |
    | Firestorm | 0.16 → 0.26 | – | 0.012 → 0.014 |
    | Bonfire 3D | 0.04 → 0.08 | – | 0.013 → 0.016 |
    | Quicksilver | 0.64 → 0.93 | 0.16 → 0.16 | 0.038 → 0.048 |

    Every look with feedback is sharper (by a third to a half), with no more shimmer
    and no banding back. By eye, Spiral galaxy's zoom trails keep their rings instead
    of melting into an even glow. Frame rates are as before (Quicksilver 112 → 142,
    the others within 10, which is run-to-run variation): nine taps at the render
    resolution cost nothing measurable here. So the default is 1.
  - **Checked:** 246 app tests (the feedback pass's GPU test now also carries stripes:
    sharp keeps them, bilinear blurs them, and nothing overshoots) and clippy pass; the
    reel audited (above, no errors).
- **Phase D, stable fractals (done):** `render/fractal.rs` (`History`), `render/fractal_history.wgsl`.
  - **Three passes** for a 2D fractal not bent by the fluid: its samples (4, or 1 when
    the governor has lowered it) at this frame's sub-pixel offset (Halton, bases 2 and
    3, eight long) into a texture of the canvas's size; the resolve, blending 15 % of
    that into the gathered picture before it, moved to where its points now are and
    clamped to the range of this frame's 3 × 3 neighbourhood, into the other of two
    history textures; and the gathered picture drawn over the scene, as the fractal
    was before.
  - **The move** is exact: the canvas's centre, turn and half-height are known each
    frame, so a point of this canvas is found in the last one's as an offset plus a
    turn and scale (worked out in canvas heights, so a deep zoom's tiny centre steps
    keep their precision). History that moves outside the canvas isn't used; nor is it
    after a change of kind, a new look or variant, the fractal turned off, a bent or 3D
    fractal (drawn straight into the scene, as before), or a change of size.
  - **What the clamp does:** what changed this frame (a Julia set wandering, the
    colours cycling, a dive's colour shift, a morph kick) is pulled to this frame's
    colours, so it can't leave ghosts; still detail keeps gathering. The history is read
    with Catmull-Rom, as the feedback is (phase C), so a dive's resampling doesn't blur
    it.
  - **Measured on the GPU** (one sample a pixel, a Julia set): single frames at their
    own offsets change 0.086 a pixel from frame to frame, gathered frames 0.0095 (9×
    less); a Julia set panned a sixth of the canvas over 20 frames differs from the same
    place gathered still by 0.022, a quarter of one frame's flicker (no ghost).
  - **In the app** (20 s each, gathering on against off, with Julia tunnel taken apart):

    | Look | Sharpness | Shimmer |
    |---|---|---|
    | Julia, still | 2.79 → 1.70 | 0.00 → 0.07 |
    | Julia, diving (no wander) | 2.79 → 1.40 | 0.87 → 0.53 |
    | Julia tunnel (diving, wandering, kicked) | 2.45 → 1.66 | – → 0.93 |

    The fall in sharpness is the aliasing going: a still set's jagged, pixel-scale
    edges become smooth (by eye too); a still set without gathering doesn't flicker at
    all (every frame samples the same points), and gathered it moves a little with the
    offsets. A dive shimmers 40 % less. Julia tunnel itself changes for real every frame
    (its Julia point wanders, its colours shift with the dive's depth), which the clamp
    rightly keeps and the shimmer measure can't tell from flicker: its shimmer stays
    about where it was (the off run found too little still detail to measure).
  - **The reel** (against phase C's): Julia tunnel's sharpness 2.69 → 1.72, its shimmer
    0.72 → 0.73; the looks without a fractal as they were (within the runs' variation).
    Frame rates as before (Julia tunnel at 239, the cap).
  - **Checked:** 248 app tests (new: a view finds its points in the last one after a
    pan, zoom and turn, and not across kinds; on the GPU, gathered frames stop
    flickering and a panned fractal leaves no ghost) and clippy pass; the reel and the
    Julia experiments audited (above, no errors).
- **Phase E, lit dye (done):** `render/overlay.wgsl`, `render/overlay.rs`.
  - **Where:** every grid solver's dye (the CPU and GPU stable fluids, Lattice Boltzmann)
    is drawn the same way, colourised on the CPU into an 8-bit texture whose alpha rises
    with the dye's thickness (`colormap`). The lighting is in that overlay's shader, for
    concentrations only (fire and water have their own colouring; water is lit already).
  - **The light:** the alpha's slope, a texel and a half each way (wider than a texel,
    so the grid's facets don't show when the field is magnified), taken 6 times steeper
    as a normal; shaded from the upper left (flat dye as it was: 1 + 1.5 × (n·l − l_z)),
    with a highlight off the slopes facing the light (Blinn, exponent 40) in the
    palette's highlight colour; mixed in by the look's relief.
  - **`relief`** in `[sim]` (0 to 1, default 0, an error on a look without a fluid),
    a slider in the View tab for grid solvers, saved by Save as, set with the look's
    picture (so a reloaded file shows it at once). Random dye looks get 0.4 to 0.9 two
    times in five (drawn last, so a seed's look is otherwise as it was). Ink nebula
    (0.6) and Silk streams (0.5) have it.
  - **By eye** (a full-size frame of Ink nebula at 0.8): glossy, embossed ink, its
    filaments standing up from the dark between them, with no facets or speckle from
    the 8-bit alpha.
  - **The audit** keeps one full-size frame of each look (`full_NNN.png`, the strip on
    the sheet links to it), for looking at closely.
  - **The reel** (against phase D's): Ink nebula's sharpness 0.44 → 0.58 and shimmer
    0.00 → 0.09 (the lit detail moving with the flow, which is real); the others as
    they were. Frame rates within the runs' variation (Ink nebula 124 → 117).
  - **Found on the way:** the linker's unresolved-symbol error seen once during the
    visuals plan came back (a corrupt incremental build cache); `cargo clean -p app`
    fixed it again.
  - **Checked:** 249 app tests (new: relief is read, kept to 0..1, written back, and
    needs a fluid) and clippy pass; the reel and the relief looks audited (no errors).
