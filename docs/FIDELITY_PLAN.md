# Fidelity Plan

Status: **planned** (2026-10-31); nothing started, decisions open (§4). A sharper, cleaner
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

## 4. Decisions (open)

1. **The tone curve (§3.2):** it changes every look's colours a little. Recommended: on
   brightness with a controlled bleed to white, compared side by side on the contact
   sheet against today's before switching; looks retuned only where they suffer.
2. **HDR output (§3.8):** in scope? It needs an HDR display to see and to test.
   Recommended: last, if the user has one.
3. **Which fluids move to the GPU (§3.5):** Smoke & fire is the largest single piece of
   work in the plan. Recommended: Smoke & fire only, then decide on the others.
4. **The frame budget:** the target (60 fps at 1080p or 1440p? 4K?) and the GPU it's
   measured on decide how far §3.3, §3.4 and §3.7 can go by default. Recommended: the
   user's own machine at its display's resolution, with the governor as the fallback.
5. **Per-look settings or global (§3.3, §3.5):** sharpness and relief as look settings
   (more control, more to tune) or one global setting each. Recommended: look settings,
   with defaults from the contact sheet.

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

Nothing yet.
