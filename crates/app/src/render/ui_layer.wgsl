// The interface over an HDR surface (see ui_layer.rs): egui draws into an 8-bit layer
// in gamma space, as it expects; this puts that layer over the surface, decoded to
// linear light and scaled so the interface's white is the paper white.

// x: the scale from linear 0..1 to the surface's units (scRGB: paper white / 80 nits).
@group(0) @binding(0) var<uniform> L: vec4f;
@group(0) @binding(1) var layer: texture_2d<f32>;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    let p = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4f(p * 2.0 - 1.0, 0.0, 1.0);
}

fn decode(s: vec3f) -> vec3f {
    return select(pow((s + 0.055) / 1.055, vec3f(2.4)), s / 12.92, s <= vec3f(0.04045));
}

/// The layer's pixel (premultiplied, gamma encoded) as premultiplied linear light.
@fragment
fn fs(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let c = textureLoad(layer, vec2i(pos.xy), 0);
    if c.a <= 0.0 {
        return vec4f(0.0);
    }
    let linear = decode(clamp(c.rgb / c.a, vec3f(0.0), vec3f(1.0))) * L.x;
    return vec4f(linear * c.a, c.a);
}
