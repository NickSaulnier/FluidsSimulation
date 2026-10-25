// Screen-space liquid: particles splatted as Gaussians into an offscreen density field,
// then thresholded and shaded into a continuous surface.
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

    // Normal from the smoothed density gradient (screen y points down, world y up),
    // flattened deep inside the fluid so highlights stay at the surface.
    let gx = smoothed(p + vec2i(step, 0), step) - smoothed(p - vec2i(step, 0), step);
    let gy = smoothed(p - vec2i(0, step), step) - smoothed(p + vec2i(0, step), step);
    let interior = clamp((d - thr) * 2.0, 0.0, 1.0);
    let n = normalize(vec3f(-gx, -gy, 0.4 + 3.0 * interior));
    let light = normalize(vec3f(-0.45, 0.65, 0.6));
    let diffuse = 0.8 + 0.2 * dot(n, light);
    let halfway = normalize(light + vec3f(0.0, 0.0, 1.0));
    let spec = 0.45 * pow(max(dot(n, halfway), 0.0), 40.0);

    // The fluid palette (linear; the surface is sRGB).
    let shallow = g.base.rgb;
    let deep = g.deep.rgb;
    let foam = g.highlight.rgb;
    let sample = textureLoad(accum, p, 0);
    let speed = sample.g / max(sample.r, 1e-4);
    var colour = mix(shallow, deep, interior);                                  // thicker reads darker
    colour = mix(colour, foam, 0.6 * clamp(speed / g.max_speed, 0.0, 1.0));     // fast water whitens
    colour = mix(colour, foam, 0.3 * (1.0 - smoothstep(thr, thr + 0.25, d)));   // light rim at the surface
    return vec4f(colour * diffuse + spec, alpha);
}
