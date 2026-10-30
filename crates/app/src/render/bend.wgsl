// The fluid bending the fractal (see bend.rs): each pixel's displacement (uv, y down)
// is the one carried in from upstream plus the flow's push, eased back toward none.

struct Bend {
    // The flow's push this frame: its gain times the frame time.
    push: f32,
    // What's kept of the displacement this frame (the spring back).
    keep: f32,
    max_shift: f32,
    // How far around the flow is averaged, uv (particle solvers' flow is noisy).
    blur: f32,
};

@group(0) @binding(0) var<uniform> B: Bend;
@group(0) @binding(1) var previous: texture_2d<f32>;
// Screen-space flow, uv per second.
@group(0) @binding(2) var flow: texture_2d<f32>;
@group(0) @binding(3) var linear_clamp: sampler;

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

// The flow averaged over a small disc: the centre and eight around it.
fn smooth_flow(uv: vec2f) -> vec2f {
    var sum = textureSampleLevel(flow, linear_clamp, uv, 0.0).xy;
    for (var k = 0; k < 8; k++) {
        let a = f32(k) * 0.7853982;
        sum += textureSampleLevel(flow, linear_clamp, uv + vec2f(cos(a), sin(a)) * B.blur, 0.0).xy;
    }
    return sum / 9.0;
}

@fragment
fn fs_bend(in: VOut) -> @location(0) vec4f {
    let step = smooth_flow(in.uv) * B.push;
    // Semi-Lagrangian: what's here now came from upstream, and moved by this step.
    let carried = textureSampleLevel(previous, linear_clamp, in.uv - step, 0.0).xy;
    var d = (carried + step) * B.keep;
    let reach = length(d);
    if reach > B.max_shift {
        d *= B.max_shift / reach;
    }
    return vec4f(d, 0.0, 0.0);
}
