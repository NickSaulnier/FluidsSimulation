// Field overlay: a colourised texture placed at the solver's field layout.
// (Globals duplicated from scene.wgsl; a separate module keeps its bind group free.)
struct Globals {
    scale: vec2f,
    offset: vec2f,
    domain: vec2f,
    radius: f32,
    max_speed: f32,
};
@group(0) @binding(0) var<uniform> g: Globals;

struct Overlay {
    origin: vec2f,   // world position of the field's lower-left corner
    size: vec2f,     // world size covered by the field
    opacity: f32,
    _pad0: f32,
    _pad1: vec2f,
};
@group(1) @binding(0) var field_tex: texture_2d<f32>;
@group(1) @binding(1) var field_sampler: sampler;
@group(1) @binding(2) var<uniform> o: Overlay;

struct Out {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
};

@vertex
fn vs_overlay(@builtin(vertex_index) vi: u32) -> Out {
    let uv = vec2f(f32(vi & 1u), f32(vi >> 1u));
    let world = o.origin + uv * o.size;
    var out: Out;
    out.clip = vec4f(world * g.scale + g.offset, 0.0, 1.0);
    out.uv = uv;   // row 0 of the field is the bottom, so v = y directly
    return out;
}

@fragment
fn fs_overlay(i: Out) -> @location(0) vec4f {
    let c = textureSample(field_tex, field_sampler, i.uv);
    let a = c.a * o.opacity;
    if a <= 0.0 {
        discard;
    }
    return vec4f(c.rgb, a);
}
