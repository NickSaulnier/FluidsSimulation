// Field overlay: a colourised texture placed at the solver's field layout.
// (Globals duplicated from scene.wgsl; a separate module keeps its bind group free.)
struct Globals {
    scale: vec2f,
    offset: vec2f,
    domain: vec2f,
    radius: f32,
    max_speed: f32,
    // Fluid palette, linear RGB (alpha unused).
    deep: vec4f,
    base: vec4f,
    highlight: vec4f,
};
@group(0) @binding(0) var<uniform> g: Globals;

struct Overlay {
    origin: vec2f,   // world position of the field's lower-left corner
    size: vec2f,     // world size covered by the field
    opacity: f32,
    // Dye lit as a surface (see fs_overlay): 0 flat, 1 fully.
    relief: f32,
    _pad1: vec2f,
};

// Relief: the dye's thickness (its alpha) taken this many times steeper as a height,
// lit from the upper left (x right, y up, z toward the viewer), with a highlight off the
// slopes facing the light.
const RELIEF_SLOPE: f32 = 6.0;
const LIGHT: vec3f = vec3f(-0.45, 0.55, 0.7);
const SHINE: f32 = 40.0;
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

/// The dye's thickness (the alpha, which rises with it) at `uv`.
fn thickness(uv: vec2f) -> f32 {
    return textureSampleLevel(field_tex, field_sampler, uv, 0.0).a;
}

@fragment
fn fs_overlay(i: Out) -> @location(0) vec4f {
    let c = textureSample(field_tex, field_sampler, i.uv);
    let a = c.a * o.opacity;
    if a <= 0.0 {
        discard;
    }
    var rgb = c.rgb;
    if o.relief > 0.0 {
        // The slope a texel and a half each way (wider than a texel, so the light
        // doesn't show the grid's facets), as a normal; flat dye lit as it was.
        let step = 1.5 / vec2f(textureDimensions(field_tex));
        let dx = thickness(i.uv + vec2f(step.x, 0.0)) - thickness(i.uv - vec2f(step.x, 0.0));
        let dy = thickness(i.uv + vec2f(0.0, step.y)) - thickness(i.uv - vec2f(0.0, step.y));
        let n = normalize(vec3f(-RELIEF_SLOPE * dx, -RELIEF_SLOPE * dy, 1.0));
        let light = normalize(LIGHT);
        let shade = max(1.0 + 1.5 * (dot(n, light) - light.z), 0.0);
        let shine = pow(max(dot(n, normalize(light + vec3f(0.0, 0.0, 1.0))), 0.0), SHINE) * (1.0 - n.z) * 8.0;
        rgb = mix(rgb, rgb * shade + g.highlight.rgb * shine, o.relief);
    }
    return vec4f(rgb, a);
}
