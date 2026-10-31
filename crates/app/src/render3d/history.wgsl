// The raymarch's frames gathered (see mod.rs): this frame's march (its rays started at
// new depths) blended into the frames before, moved to where the camera now sees them
// (through each pixel's visible gas or solid), clamped to this frame's neighbourhood;
// then drawn over the scene. Colour premultiplied, alpha the light let through.

struct Resolve {
    // The last frame's view and projection.
    prev_view_proj: mat4x4f,
    // The canvas in target pixels: x, y, width, height.
    viewport: vec4f,
    // How much of this frame goes in: 1 none of the old (nothing to reuse).
    weight: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};

@group(0) @binding(0) var<uniform> R: Resolve;
// This frame's march: colour and the light let through; where its visible gas is (w:
// how much shows, 0 for none).
@group(0) @binding(1) var layer: texture_2d<f32>;
@group(0) @binding(2) var position: texture_2d<f32>;
@group(0) @binding(3) var history: texture_2d<f32>;
@group(0) @binding(4) var linear: sampler;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    let p = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4f(p * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_resolve(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let size = vec2i(textureDimensions(layer));
    let at = vec2i(pos.xy);
    let now = textureLoad(layer, at, 0);
    if R.weight >= 1.0 {
        return now;
    }
    var lo = now;
    var hi = now;
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let c = textureLoad(layer, clamp(at + vec2i(dx, dy), vec2i(0), size - 1), 0);
            lo = min(lo, c);
            hi = max(hi, c);
        }
    }
    // Where this pixel's gas was on screen last frame (nothing seen: the same place,
    // the empty background doesn't move).
    var was = pos.xy;
    let world = textureLoad(position, at, 0);
    if world.w > 0.0 {
        let clip = R.prev_view_proj * vec4f(world.xyz, 1.0);
        let ndc = clip.xy / clip.w;
        was = R.viewport.xy + vec2f(0.5 + 0.5 * ndc.x, 0.5 - 0.5 * ndc.y) * R.viewport.zw;
    }
    if any(was < R.viewport.xy) || any(was > R.viewport.xy + R.viewport.zw) || (world.w > 0.0 && clip_behind(world.xyz)) {
        return now;
    }
    let old = clamp(textureSampleLevel(history, linear, was / vec2f(size), 0.0), lo, hi);
    return mix(old, now, R.weight);
}

/// Whether `p` was behind the last frame's camera (its projection meaningless).
fn clip_behind(p: vec3f) -> bool {
    return (R.prev_view_proj * vec4f(p, 1.0)).w <= 0.0;
}

/// The gathered march over the scene (the pipeline blends: colour plus what's behind
/// times the light let through).
@fragment
fn fs_over(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    return textureLoad(history, vec2i(pos.xy), 0);
}
