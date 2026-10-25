// Coloured lines in 3D: the domain box, floor grid, burners, vents and solid outlines.
struct Uniforms {
    view_proj: mat4x4f,
};
@group(0) @binding(0) var<uniform> u: Uniforms;

struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) colour: vec4f,
};

@vertex
fn vs(@location(0) pos: vec3f, @location(1) colour: vec4f) -> VOut {
    var o: VOut;
    o.clip = u.view_proj * vec4f(pos, 1.0);
    o.colour = colour;
    return o;
}

@fragment
fn fs(i: VOut) -> @location(0) vec4f {
    return i.colour;
}
