// Screen-space liquid: particles splatted as Gaussians into an offscreen density field,
// then thresholded and shaded into a continuous surface: wet, with a reflection that
// grows toward its edges (Fresnel) and a body that deepens with depth (Beer's law).
// (Globals duplicated from scene.wgsl; a separate module keeps its bind groups free.)
struct Globals {
    scale: vec2f,
    offset: vec2f,
    domain: vec2f,
    radius: f32,
    max_speed: f32,
    // Fluid palette, linear RGB (alpha unused).
    deep: vec4f,
    base: vec4f,
    highlight: vec4f,
};
@group(0) @binding(0) var<uniform> g: Globals;

struct Liquid {
    splat_scale: f32,   // blob radius in particle radii
    threshold: f32,     // surface level, as a fraction of the interior density
    expected: f32,      // interior density of the field, so the threshold is resolution-free
    splat_px: f32,      // blob radius on screen, pixels (sets the smoothing scale)
};
@group(1) @binding(0) var<uniform> l: Liquid;
@group(1) @binding(1) var accum: texture_2d<f32>;   // composite only

fn to_clip(world: vec2f) -> vec4f {
    return vec4f(world * g.scale + g.offset, 0.0, 1.0);
}

fn corner01(vi: u32) -> vec2f {
    return vec2f(f32(vi & 1u), f32(vi >> 1u));
}

// ---- Pass 1: splat (additive into an Rgba16Float target) --------------------------

struct SplatOut {
    @builtin(position) clip: vec4f,
    @location(0) local: vec2f,
    @location(1) speed: f32,
};

@vertex
fn vs_splat(@builtin(vertex_index) vi: u32, @location(0) pos: vec2f, @location(1) vel: vec2f) -> SplatOut {
    let corner = corner01(vi) * 2.0 - 1.0;
    var o: SplatOut;
    o.clip = to_clip(pos + corner * g.radius * l.splat_scale);
    o.local = corner;
    o.speed = length(vel);
    return o;
}

@fragment
fn fs_splat(i: SplatOut) -> @location(0) vec4f {
    let d2 = dot(i.local, i.local);
    if d2 >= 1.0 {
        discard;
    }
    let w = exp(-4.0 * d2);
    return vec4f(w, w * i.speed, 0.0, 0.0);
}

// ---- Pass 2: composite over the scene ----------------------------------------------

@vertex
fn vs_composite(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4f {
    return to_clip(corner01(vi) * g.domain);
}

fn density_at(p: vec2i) -> f32 {
    let dims = vec2i(textureDimensions(accum));
    return textureLoad(accum, clamp(p, vec2i(0), dims - 1), 0).r / l.expected;
}

// 3x3 tent filter at a step proportional to the blob size. The raw splat sum ripples at
// particle scale; thresholding or differentiating it directly gives specks, fizzy edges
// and sparkling highlights.
fn smoothed(p: vec2i, step: i32) -> f32 {
    let x = vec2i(step, 0);
    let y = vec2i(0, step);
    var s = 4.0 * density_at(p);
    s += 2.0 * (density_at(p + x) + density_at(p - x) + density_at(p + y) + density_at(p - y));
    s += density_at(p + x + y) + density_at(p - x + y) + density_at(p + x - y) + density_at(p - x - y);
    return s / 16.0;
}

// How strongly the body's colour deepens toward the middle of the fluid (Beer's law:
// each channel's share kept falls as the colour's own power, so deep reads darker and
// more saturated), and the reflectance head on (water's, about 4 %).
const BEER: f32 = 2.0;
const F0: f32 = 0.04;

// What the surface reflects: a sky above (the palette's highlight, dimmed) fading to
// the deep colour below, by the reflected ray's height.
fn environment(r: vec3f) -> vec3f {
    return mix(0.3 * g.deep.rgb, 0.8 * g.highlight.rgb, smoothstep(-0.3, 0.9, r.y));
}

@fragment
fn fs_composite(@builtin(position) frag: vec4f) -> @location(0) vec4f {
    let p = vec2i(frag.xy);
    let step = max(1, i32(0.5 * l.splat_px));
    let d = smoothed(p, step);
    let thr = l.threshold;
    let alpha = smoothstep(thr - 0.05, thr + 0.05, d);
    if alpha <= 0.0 {
        discard;
    }

    // Normal from the density's gradient smoothed at twice the blob scale (screen y
    // points down, world y up): a surface that bulges smoothly over the particles
    // instead of following each one, flattened deep inside so highlights stay at the
    // surface. The depth below the surface from wider still (eight times): about 0 at
    // the edge, 1 several blobs in.
    let wide = 2 * step;
    let gx = smoothed(p + vec2i(wide, 0), wide) - smoothed(p - vec2i(wide, 0), wide);
    let gy = smoothed(p - vec2i(0, wide), wide) - smoothed(p + vec2i(0, wide), wide);
    let depth = clamp((smoothed(p, 8 * step) - thr) / max(1.0 - thr, 0.05), 0.0, 1.0);
    let interior = clamp((d - thr) * 2.0, 0.0, 1.0);
    let n = normalize(vec3f(-2.0 * gx, -2.0 * gy, 0.35 + 3.0 * interior));
    let light = normalize(vec3f(-0.45, 0.65, 0.6));
    let diffuse = 0.75 + 0.25 * dot(n, light);
    let halfway = normalize(light + vec3f(0.0, 0.0, 1.0));
    let spec = 0.6 * pow(max(dot(n, halfway), 0.0), 80.0);

    // The fluid palette (linear; the surface is sRGB).
    let shallow = g.base.rgb;
    let deep = g.deep.rgb;
    let foam = g.highlight.rgb;
    let sample = textureLoad(accum, p, 0);
    let speed = sample.g / max(sample.r, 1e-4);
    // The body: the base colour, deepening with depth, and toward the deep colour.
    var colour = mix(pow(max(shallow, vec3f(1e-4)), vec3f(1.0 + BEER * depth)), deep, 0.5 * depth);
    colour = mix(colour, foam, 0.6 * clamp(speed / g.max_speed, 0.0, 1.0));     // fast water whitens
    colour = mix(colour, foam, 0.25 * (1.0 - smoothstep(thr, thr + 0.25, d)));  // light rim at the surface
    // Fresnel (Schlick): the reflection grows as the surface turns away from the view,
    // so the rounded edges shine like a liquid's.
    let fresnel = F0 + (1.0 - F0) * pow(1.0 - n.z, 5.0);
    let reflected = vec3f(2.0 * n.z * n.x, 2.0 * n.z * n.y, 2.0 * n.z * n.z - 1.0);
    colour = mix(colour * diffuse, environment(reflected), fresnel);
    // Thin at the edge, a little see-through.
    return vec4f(colour + spec, alpha * mix(0.8, 1.0, depth));
}
