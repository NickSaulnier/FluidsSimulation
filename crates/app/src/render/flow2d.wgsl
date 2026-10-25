// The 2D flow for the feedback layer: the solver's velocity grid, seen through the
// view, as screen uv per second.

struct View2d {
    // World to NDC: ndc = world * scale + offset.
    scale: vec2f,
    offset: vec2f,
    // The grid: cells across and up, its origin and cell size (metres).
    cells: vec2u,
    origin: vec2f,
    cell: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};

@group(0) @binding(0) var<uniform> view2d: View2d;
@group(0) @binding(1) var velocity: texture_2d<f32>;

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


fn load_velocity(c: vec2i) -> vec2f {
    let clamped = clamp(c, vec2i(0), vec2i(view2d.cells) - 1);
    return textureLoad(velocity, clamped, 0).xy;
}

@fragment
fn fs_flow2d(in: VOut) -> @location(0) vec4f {
    let ndc = vec2f(in.uv.x * 2.0 - 1.0, 1.0 - in.uv.y * 2.0);
    let world = (ndc - view2d.offset) / view2d.scale;
    let g = (world - view2d.origin) / view2d.cell - 0.5;
    let size = vec2f(view2d.cells);
    if any(g < vec2f(-0.5)) || any(g > size - 0.5) {
        return vec4f(0.0);
    }
    let c = vec2i(floor(g));
    let f = g - vec2f(c);
    let v = mix(mix(load_velocity(c), load_velocity(c + vec2i(1, 0)), f.x), mix(load_velocity(c + vec2i(0, 1)), load_velocity(c + vec2i(1, 1)), f.x), f.y);
    // World m/s to uv/s: NDC spans 2 per uv unit, and uv's y runs down.
    return vec4f(0.5 * v.x * view2d.scale.x, -0.5 * v.y * view2d.scale.y, 0.0, 0.0);
}
