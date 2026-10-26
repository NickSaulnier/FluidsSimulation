// The feedback layer (see feedback.rs): each frame starts from the last one, warped
// (zoomed, turned, stretched, shifted, waved, and carried along by the fluid's flow)
// and faded, with the new scene combined over it.
//
// A template: feedback.rs puts the equation maths before it, declares the motion where
// `MOTION` is, and puts a look's per-pixel code (see `expr::wgsl`) where `PIXEL` is.

struct Feedback {
    dt: f32,
    // How much of the old image survives this frame, and how the scene combines.
    persistence: f32,
    // The flow's push: uv per (uv/s) of flow, the gain times the frame time.
    flow: f32,
    warp_phase: f32,
    mode: u32,
    aspect: f32,
    _pad: vec2f,
    // The equations' shared values (`expr::SHARED`): inputs, motion (per second), q's.
    vars: array<vec4f, 16>,
};

@group(0) @binding(0) var<uniform> fb: Feedback;
@group(0) @binding(1) var previous: texture_2d<f32>;
@group(0) @binding(2) var linear_clamp: sampler;
@group(0) @binding(3) var scene: texture_2d<f32>;
// Screen-space flow, uv per second.
@group(0) @binding(4) var flow: texture_2d<f32>;

const SMEAR: u32 = 0u;

fn v(i: u32) -> f32 {
    return fb.vars[i / 4u][i % 4u];
}

// `value` within lo..hi, or `fallback` if it isn't a number (a NaN would stay in the
// picture for good).
fn clean(value: f32, fallback: f32, lo: f32, hi: f32) -> f32 {
    let finite = (bitcast<u32>(value) & 0x7f800000u) != 0x7f800000u;
    return select(fallback, clamp(value, lo, hi), finite);
}

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
    // The pixel, as the equations see it: y up, the distance from the centre (1 at the
    // top and bottom edges) and the angle.
    let p_x = uv.x;
    let p_y = 1.0 - uv.y;
    let from_centre = vec2f((p_x - 0.5) * fb.aspect, p_y - 0.5);
    let p_rad = 2.0 * length(from_centre);
    let p_ang = atan2(from_centre.y, from_centre.x);
//{MOTION}
//{PIXEL}
    let dt = fb.dt;
    let zoom = pow(clean(m_zoom, 1.0, 0.05, 20.0), dt);
    let rot = clean(m_rot, 0.0, -100.0, 100.0) * dt;
    let stretch = pow(vec2f(clean(m_sx, 1.0, 0.05, 20.0), clean(m_sy, 1.0, 0.05, 20.0)), vec2f(dt));
    let shift = vec2f(clean(m_dx, 0.0, -10.0, 10.0) * dt / fb.aspect, -clean(m_dy, 0.0, -10.0, 10.0) * dt);
    let centre = vec2f(clean(m_cx, 0.5, -1.0, 2.0), 1.0 - clean(m_cy, 0.5, -1.0, 2.0));
    let warp = clean(m_warp, 0.0, 0.0, 0.2);

    let square = vec2f(fb.aspect, 1.0);
    var p = (uv - centre) * square;
    let s = sin(-rot);
    let c = cos(-rot);
    p = vec2f(c * p.x - s * p.y, s * p.x + c * p.y) / (zoom * stretch);
    // MilkDrop-style wobble: slow waves that bend the motion.
    p += warp * vec2f(sin(fb.warp_phase + 6.0 * p.y + 1.7 * p.x), cos(1.3 * fb.warp_phase + 5.0 * p.x - 2.1 * p.y));
    var src = p / square + centre - shift;
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
