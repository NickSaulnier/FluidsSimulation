// The light volume (see mod.rs): for each cell of a coarse grid over the domain, how
// much of the light gets there, marched toward it through the smoke and the solids.
// The raymarch reads it for the smoke's shadowing, so smoke shadows itself and the
// solids shadow the smoke, however far apart.

struct Light {
    // xyz: the domain (m); w: the march step (m).
    size: vec4f,
    // xyz: toward the light; w: extinction per unit smoke (1/m).
    light: vec4f,
    // xyz: the light grid's cells.
    cells: vec4u,
};

@group(0) @binding(0) var<uniform> L: Light;
@group(0) @binding(1) var volume: texture_3d<f32>;
@group(0) @binding(2) var linear: sampler;
@group(0) @binding(3) var out: texture_storage_3d<rgba16float, write>;

/// The march's own fade at the open sides and top (as march.wgsl's `edge_fade`), so
/// smoke that isn't drawn doesn't cast shadows.
const FADE: f32 = 0.35;
const MAX_STEPS: i32 = 160;

fn edge_fade(p: vec3f) -> f32 {
    let s = L.size.xyz;
    let e = min(min(min(p.x, s.x - p.x), min(p.z, s.z - p.z)), s.y - p.y);
    return smoothstep(0.0, FADE, e);
}

@compute @workgroup_size(4, 4, 4)
fn light(@builtin(global_invocation_id) id: vec3u) {
    if any(id >= L.cells.xyz) {
        return;
    }
    let size = L.size.xyz;
    let p = (vec3f(id) + 0.5) / vec3f(L.cells.xyz) * size;
    let step = L.size.w;
    var depth = 0.0;
    var t = 0.5 * step;
    for (var k = 0; k < MAX_STEPS; k++) {
        let q = p + L.light.xyz * t;
        if any(q < vec3f(0.0)) || any(q > size) {
            break;
        }
        let v = textureSampleLevel(volume, linear, q / size, 0.0);
        // Solids as very thick smoke.
        depth += (v.r * edge_fade(q) + 20.0 * v.b) * step;
        t += step;
    }
    textureStore(out, id, vec4f(exp(-L.light.w * depth), 0.0, 0.0, 1.0));
}
