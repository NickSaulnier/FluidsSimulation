// The gas volume, raymarched front to back over the lines already drawn, with the
// scene's solids shaded where the rays hit them. Smoke absorbs and scatters light; hot
// gas glows (fire). Output is premultiplied: colour added over the target, alpha the
// light that still comes through from behind. A second output is the screen-space flow
// for the feedback layer: the visible gas's velocity, as uv per second.

struct March {
    inv_view_proj: mat4x4f,
    view_proj: mat4x4f,
    // xyz: the camera; w: glow per metre of full flame (0: no fire).
    eye: vec4f,
    // xyz: the domain; w: the march step (m).
    size: vec4f,
    // The canvas in target pixels: x, y, width, height.
    viewport: vec4f,
    // rgb: smoke facing the light (linear); w: extinction per unit smoke (1/m).
    lit: vec4f,
    // rgb: smoke in its own shadow; w: how far the shadow probe looks (m).
    shade: vec4f,
    // xyz: toward the light; w: extinction per unit smoke for the shadow probe (1/m).
    light: vec4f,
    // x: solid shapes in `shapes`; y: 1 if there's a volume to march.
    counts: vec4u,
    // Two per shape: a sphere is (centre, radius) then unused, a box (min, -1) then
    // (max, 0).
    shapes: array<vec4f, 64>,
};

@group(0) @binding(0) var<uniform> m: March;
@group(0) @binding(1) var volume: texture_3d<f32>;
@group(0) @binding(2) var linear: sampler;
@group(0) @binding(3) var depth: texture_depth_2d;
// The gas velocity (m/s), on the volume's grid.
@group(0) @binding(4) var velocity: texture_3d<f32>;

struct Out {
    @location(0) colour: vec4f,
    @location(1) flow: vec4f,
};

const SOLID: vec3f = vec3f(0.42, 0.44, 0.48);
/// Temperatures drawn as the faintest and the fullest flame, as in 2D: warm gas below
/// the first is invisible, so a flame ends where its gas cools.
const FLAME_FAINT: f32 = 1.0;
const FLAME_FULL: f32 = 3.0;

/// Gas thins out over this far from the open sides and top, so the volume's edge
/// never shows (m).
const FADE: f32 = 0.35;

/// How much of the gas at `p` shows: none at the open sides and the top, all of it
/// this far in. (The floor is closed.)
fn edge_fade(p: vec3f) -> f32 {
    let s = m.size.xyz;
    let e = min(min(min(p.x, s.x - p.x), min(p.z, s.z - p.z)), s.y - p.y);
    return smoothstep(0.0, FADE, e);
}

/// The 2D fire's ramp (deep red, orange, yellow, near white), in linear light.
fn flame_colour(t: f32) -> vec3f {
    let stops = array<vec3f, 4>(
        vec3f(0.188, 0.0060, 0.0006),
        vec3f(0.730, 0.0513, 0.0024),
        vec3f(1.0, 0.352, 0.0110),
        vec3f(1.0, 0.888, 0.552),
    );
    let x = clamp(t, 0.0, 1.0) * 3.0;
    let k = min(u32(x), 2u);
    return mix(stops[k], stops[k + 1u], x - f32(k));
}

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    // One triangle covering the viewport.
    let p = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4f(p * 2.0 - 1.0, 0.0, 1.0);
}

fn unproject(ndc: vec2f, z: f32) -> vec3f {
    let p = m.inv_view_proj * vec4f(ndc, z, 1.0);
    return p.xyz / p.w;
}

/// Where a ray enters and leaves the box (enter > leave: it misses), and the entry
/// face's normal.
fn slab(o: vec3f, d: vec3f, lo: vec3f, hi: vec3f) -> vec3f {
    let inv = 1.0 / select(d, vec3f(1e-8), abs(d) < vec3f(1e-8));
    let a = (lo - o) * inv;
    let b = (hi - o) * inv;
    let near = min(a, b);
    let far = max(a, b);
    return vec3f(max(max(near.x, near.y), near.z), min(min(far.x, far.y), far.z), 0.0);
}

fn box_normal(o: vec3f, d: vec3f, lo: vec3f, hi: vec3f, t: f32) -> vec3f {
    let p = o + d * t;
    let c = 0.5 * (lo + hi);
    let q = (p - c) / (0.5 * (hi - lo));
    let a = abs(q);
    if a.x >= a.y && a.x >= a.z {
        return vec3f(sign(q.x), 0.0, 0.0);
    }
    if a.y >= a.z {
        return vec3f(0.0, sign(q.y), 0.0);
    }
    return vec3f(0.0, 0.0, sign(q.z));
}

/// A hash of the pixel, so neighbouring rays start at different depths (no banding).
fn jitter(px: vec2f) -> f32 {
    return fract(52.9829189 * fract(dot(px, vec2f(0.06711056, 0.00583715))));
}

fn smoke_at(p: vec3f) -> f32 {
    return textureSampleLevel(volume, linear, p / m.size.xyz, 0.0).r;
}

/// What blocks the light at `p`: smoke, and solids (as very thick smoke).
fn occluder_at(p: vec3f) -> f32 {
    let v = textureSampleLevel(volume, linear, p / m.size.xyz, 0.0);
    return v.r + 20.0 * v.b;
}

@fragment
fn fs(@builtin(position) pos: vec4f) -> Out {
    let uv = (pos.xy - m.viewport.xy) / m.viewport.zw;
    let ndc = vec2f(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let eye = m.eye.xyz;
    let dir = normalize(unproject(ndc, 1.0) - eye);

    // Stop at the lines in front (floor grid, edges) and at the nearest solid.
    let z = textureLoad(depth, vec2i(pos.xy), 0);
    var end = 1e9;
    if z < 1.0 {
        end = dot(unproject(ndc, z) - eye, dir);
    }
    var normal = vec3f(0.0);
    for (var s = 0u; s < m.counts.x; s++) {
        let a = m.shapes[2u * s];
        let b = m.shapes[2u * s + 1u];
        if a.w >= 0.0 {
            let oc = eye - a.xyz;
            let half_b = dot(oc, dir);
            let disc = half_b * half_b - (dot(oc, oc) - a.w * a.w);
            if disc >= 0.0 {
                let t = -half_b - sqrt(disc);
                if t > 0.0 && t < end {
                    end = t;
                    normal = normalize(eye + dir * t - a.xyz);
                }
            }
        } else {
            let hit = slab(eye, dir, a.xyz, b.xyz);
            if hit.x <= hit.y && hit.x > 0.0 && hit.x < end {
                end = hit.x;
                normal = box_normal(eye, dir, a.xyz, b.xyz, hit.x);
            }
        }
    }

    var colour = vec3f(0.0);
    var glow = vec3f(0.0);
    var through = 1.0;
    let glow_k = m.eye.w;
    // For the flow: velocity and position, weighted by how much each sample shows.
    var seen = 0.0;
    var seen_velocity = vec3f(0.0);
    var seen_at = vec3f(0.0);
    let span = slab(eye, dir, vec3f(0.0), m.size.xyz);
    if m.counts.y == 1u && span.x < span.y {
        let step = m.size.w;
        let sigma_k = m.lit.w;
        let probe = m.shade.w;
        let stop = min(span.y, end);
        var t = max(span.x, 0.0) + step * jitter(pos.xy);
        loop {
            if t >= stop || through < 0.01 {
                break;
            }
            let p = eye + dir * t;
            let v = textureSampleLevel(volume, linear, p / m.size.xyz, 0.0);
            let fade = edge_fade(p);
            let s = v.r * fade;
            var shows = 0.0;
            if glow_k > 0.0 {
                let flame = fade * clamp((v.g - FLAME_FAINT) / (FLAME_FULL - FLAME_FAINT), 0.0, 1.0);
                glow += through * flame_colour(flame) * (glow_k * flame * step);
                // A full flame counts about as much as opaque smoke over 25 steps.
                shows = through * 0.04 * flame;
            }
            if s > 1e-4 {
                let alpha = 1.0 - exp(-s * sigma_k * step);
                // Smoke and solids between here and the light, from two taps: a cheap
                // stand-in for real shadowing.
                let depth_to_light = probe * (occluder_at(p + m.light.xyz * probe) + 2.0 * occluder_at(p + m.light.xyz * 3.0 * probe));
                let lit = exp(-m.light.w * depth_to_light);
                colour += through * alpha * mix(m.shade.rgb, m.lit.rgb, lit);
                shows += through * alpha;
                through *= 1.0 - alpha;
            }
            if shows > 0.0 {
                seen += shows;
                seen_velocity += shows * textureSampleLevel(velocity, linear, p / m.size.xyz, 0.0).xyz;
                seen_at += shows * p;
            }
            t += step;
        }
    }
    if any(normal != vec3f(0.0)) {
        let diffuse = max(dot(normal, m.light.xyz), 0.0);
        colour += through * SOLID * (0.25 + 0.75 * diffuse);
        through = 0.0;
    }
    var out: Out;
    // Flames are brighter than white: the composite blooms them and rolls them off.
    out.colour = vec4f(colour + glow, through);
    out.flow = vec4f(0.0);
    if seen > 1e-4 {
        // Where the seen gas moves on screen: project a short step along its velocity.
        let at = seen_at / seen;
        let moved = at + 0.01 * seen_velocity / seen;
        let a = m.view_proj * vec4f(at, 1.0);
        let b = m.view_proj * vec4f(moved, 1.0);
        let d_ndc = (b.xy / b.w - a.xy / a.w) / 0.01;
        out.flow = vec4f(0.5 * d_ndc.x, -0.5 * d_ndc.y, 0.0, 0.0) * min(seen, 1.0);
    }
    return out;
}
