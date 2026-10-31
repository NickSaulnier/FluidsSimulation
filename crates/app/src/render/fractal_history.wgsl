// A 2D fractal's samples gathered across frames (see fractal.rs, `History`): this
// frame's (jittered) samples blended into the last frames', moved to where the fractal
// now shows them; then the result drawn over the scene.

struct Resolve {
    // Where a point of this frame's canvas was in the latest frame's: in units of the
    // canvas's height about its centre (x times the aspect), old = offset + turn * new,
    // `turn` the 2 × 2 matrix's first column (scale × (cos, sin)).
    turn: vec2f,
    offset: vec2f,
    aspect: f32,
    // How much of this frame goes in: 1 none of the old (nothing to reuse).
    weight: f32,
    _pad: vec2f,
};

@group(0) @binding(0) var<uniform> R: Resolve;
@group(0) @binding(1) var current: texture_2d<f32>;
@group(0) @binding(2) var history: texture_2d<f32>;
@group(0) @binding(3) var linear_clamp: sampler;

struct VOut {
    @builtin(position) pos: vec4f,
    // 0..1, y down.
    @location(0) uv: vec2f,
};

@vertex
fn vs(@builtin(vertex_index) i: u32) -> VOut {
    let p = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
    var o: VOut;
    o.pos = vec4f(p * 2.0 - 1.0, 0.0, 1.0);
    o.uv = vec2f(p.x, 1.0 - p.y);
    return o;
}

/// The history at `uv`, read with Catmull-Rom (nine bilinear taps), so a dive's
/// resampling every frame doesn't blur it (as for the feedback, see feedback.wgsl).
fn history_at(uv: vec2f) -> vec4f {
    let size = vec2f(textureDimensions(history));
    let pos = uv * size - 0.5;
    let base = floor(pos);
    let f = pos - base;
    let w0 = f * (-0.5 + f * (1.0 - 0.5 * f));
    let w1 = 1.0 + f * f * (-2.5 + 1.5 * f);
    let w2 = f * (0.5 + f * (2.0 - 1.5 * f));
    let w3 = f * f * (-0.5 + 0.5 * f);
    let w12 = w1 + w2;
    let t0 = (base - 0.5) / size;
    let t12 = (base + 0.5 + w2 / w12) / size;
    let t3 = (base + 2.5) / size;
    let xs = array<f32, 3>(t0.x, t12.x, t3.x);
    let ys = array<f32, 3>(t0.y, t12.y, t3.y);
    let wx = array<f32, 3>(w0.x, w12.x, w3.x);
    let wy = array<f32, 3>(w0.y, w12.y, w3.y);
    var sum = vec4f(0.0);
    for (var j = 0; j < 3; j++) {
        for (var i = 0; i < 3; i++) {
            sum += textureSampleLevel(history, linear_clamp, vec2f(xs[i], ys[j]), 0.0) * (wx[i] * wy[j]);
        }
    }
    return sum;
}

/// This frame's samples blended into the history: the history moved to where its
/// points now are, and clamped to the range of this frame's 3 × 3 neighbourhood, so
/// what changed (a Julia set's shape wandering, the colours cycling, a kick) doesn't
/// leave ghosts while still detail gathers samples.
@fragment
fn fs_resolve(in: VOut) -> @location(0) vec4f {
    let size = vec2i(textureDimensions(current));
    let at = vec2i(in.pos.xy);
    let now = textureLoad(current, at, 0);
    if R.weight >= 1.0 {
        return now;
    }
    var lo = now;
    var hi = now;
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let c = textureLoad(current, clamp(at + vec2i(dx, dy), vec2i(0), size - 1), 0);
            lo = min(lo, c);
            hi = max(hi, c);
        }
    }
    let local = vec2f((2.0 * in.uv.x - 1.0) * R.aspect, 1.0 - 2.0 * in.uv.y);
    let old_local = R.offset + vec2f(R.turn.x * local.x - R.turn.y * local.y, R.turn.y * local.x + R.turn.x * local.y);
    let old_uv = vec2f(0.5 + 0.5 * old_local.x / R.aspect, 0.5 - 0.5 * old_local.y);
    if any(old_uv < vec2f(0.0)) || any(old_uv > vec2f(1.0)) {
        return now;
    }
    let old = clamp(history_at(old_uv), lo, hi);
    return mix(old, now, R.weight);
}

/// The gathered fractal over the scene (premultiplied; the pipeline blends it over).
@fragment
fn fs_over(in: VOut) -> @location(0) vec4f {
    return textureSampleLevel(history, linear_clamp, in.uv, 0.0);
}
