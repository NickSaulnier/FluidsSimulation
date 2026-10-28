// The song's cover art: one quad, turned and placed in canvas half-heights (see
// art.rs), its edges feathered, drawn over the scene with premultiplied alpha.

struct Art {
    // Centre and half-size, in canvas half-heights; the turn as (cos, sin).
    centre: vec2f,
    half: vec2f,
    turn: vec2f,
    aspect: f32,
    opacity: f32,
    brightness: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};

@group(0) @binding(0) var<uniform> art: Art;
@group(0) @binding(1) var cover: texture_2d<f32>;
@group(0) @binding(2) var linear_clamp: sampler;

struct VOut {
    @builtin(position) pos: vec4f,
    @location(0) uv: vec2f,
};

@vertex
fn vs(@builtin(vertex_index) i: u32) -> VOut {
    // Two triangles: corners (-1, -1), (1, -1), (1, 1), (-1, 1).
    var corners = array<vec2f, 6>(vec2f(-1.0, -1.0), vec2f(1.0, -1.0), vec2f(1.0, 1.0), vec2f(-1.0, -1.0), vec2f(1.0, 1.0), vec2f(-1.0, 1.0));
    let c = corners[i];
    let local = c * art.half;
    let turned = vec2f(local.x * art.turn.x - local.y * art.turn.y, local.x * art.turn.y + local.y * art.turn.x);
    let p = art.centre + turned;
    var o: VOut;
    o.pos = vec4f(p.x / art.aspect, p.y, 0.0, 1.0);
    o.uv = vec2f(0.5 + 0.5 * c.x, 0.5 - 0.5 * c.y);
    return o;
}

@fragment
fn fs(i: VOut) -> @location(0) vec4f {
    let c = textureSample(cover, linear_clamp, i.uv);
    // Soft edges, so the feedback doesn't streak a hard square.
    let edge = min(min(i.uv.x, 1.0 - i.uv.x), min(i.uv.y, 1.0 - i.uv.y));
    let a = c.a * art.opacity * smoothstep(0.0, 0.04, edge);
    return vec4f(c.rgb * art.brightness * a, a);
}
