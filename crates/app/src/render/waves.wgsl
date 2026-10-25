// Waves and shapes: coloured triangles built on the CPU (see waves.rs), added onto the
// scene in light brighter than white, so the composite blooms them.

struct VOut {
    @builtin(position) pos: vec4f,
    @location(0) colour: vec4f,
};

@vertex
fn vs(@location(0) pos: vec2f, @location(1) colour: vec4f) -> VOut {
    var o: VOut;
    o.pos = vec4f(pos, 0.0, 1.0);
    o.colour = colour;
    return o;
}

@fragment
fn fs(i: VOut) -> @location(0) vec4f {
    return i.colour;
}
