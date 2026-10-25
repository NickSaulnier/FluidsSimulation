// The feedback layer (see feedback.rs): each frame starts from the last one, warped
// (zoomed, turned, shifted, waved, and carried along by the fluid's flow) and faded,
// with the new scene combined over it.

struct Feedback {
    // This frame's motion: zoom factor, turn (radians), shift (uv).
    zoom: f32,
    rotation: f32,
    shift: vec2f,
    // Wave warp: amount (uv) and phase.
    warp: f32,
    warp_phase: f32,
    // The flow's push: uv per (uv/s) of flow, i.e. the frame time times the gain.
    flow: f32,
    // How much of the old image survives this frame, and how the scene combines.
    persistence: f32,
    mode: u32,
    aspect: f32,
    _pad: vec2f,
};

@group(0) @binding(0) var<uniform> fb: Feedback;
@group(0) @binding(1) var previous: texture_2d<f32>;
@group(0) @binding(2) var linear_clamp: sampler;
@group(0) @binding(3) var scene: texture_2d<f32>;
// Screen-space flow, uv per second.
@group(0) @binding(4) var flow: texture_2d<f32>;

const SMEAR: u32 = 0u;

struct VOut {
    @builtin(position) pos: vec4f,
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

/// Where this pixel's colour comes from in the last frame. Zoom > 1 grows the image
/// from the centre (so it samples nearer the centre), rotation turns it, and the flow
/// carries it along: a semi-Lagrangian step of the whole picture.
fn source(uv: vec2f) -> vec2f {
    let square = vec2f(fb.aspect, 1.0);
    var p = (uv - 0.5) * square;
    let s = sin(-fb.rotation);
    let c = cos(-fb.rotation);
    p = vec2f(c * p.x - s * p.y, s * p.x + c * p.y) / fb.zoom;
    // MilkDrop-style wobble: slow waves that bend the motion.
    p += fb.warp * vec2f(sin(fb.warp_phase + 6.0 * p.y + 1.7 * p.x), cos(1.3 * fb.warp_phase + 5.0 * p.x - 2.1 * p.y));
    var src = p / square + 0.5 - fb.shift;
    src -= textureSampleLevel(flow, linear_clamp, uv, 0.0).xy * fb.flow;
    return src;
}

@fragment
fn fs_feedback(in: VOut) -> @location(0) vec4f {
    // From outside the frame (a turn or a shrink brings the edges in) comes black, not
    // the edge pixels smeared into streaks.
    let src_uv = source(in.uv);
    let outside = any(src_uv < vec2f(0.0)) || any(src_uv > vec2f(1.0));
    let old = select(textureSampleLevel(previous, linear_clamp, src_uv, 0.0).rgb * fb.persistence, vec3f(0.0), outside);
    let now = textureSampleLevel(scene, linear_clamp, in.uv, 0.0).rgb;
    var c: vec3f;
    if fb.mode == SMEAR {
        // A blend toward the scene: still things stay put, moving ones leave trails.
        c = now * (1.0 - fb.persistence) + old;
    } else {
        // The brighter of the two: bright things leave streaks over dark ones.
        c = max(old, now);
    }
    return vec4f(c, 1.0);
}
