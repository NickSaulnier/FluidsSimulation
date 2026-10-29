// Shared by the domain backdrop and the particle pipelines.
struct Globals {
    scale: vec2f,      // clip = world * scale + offset
    offset: vec2f,
    domain: vec2f,     // metres
    radius: f32,       // particle radius, metres
    max_speed: f32,    // speed that maps to the top of the colour ramp
    // Fluid palette, linear RGB (alpha unused).
    deep: vec4f,
    base: vec4f,
    highlight: vec4f,
};
@group(0) @binding(0) var<uniform> g: Globals;

fn to_clip(world: vec2f) -> vec4f {
    return vec4f(world * g.scale + g.offset, 0.0, 1.0);
}

// Triangle-strip quad corners in [0,1]^2: (0,0) (1,0) (0,1) (1,1).
fn corner01(vi: u32) -> vec2f {
    return vec2f(f32(vi & 1u), f32(vi >> 1u));
}

// ---- Domain backdrop: one quad covering [0, domain] ------------------------------

@vertex
fn vs_backdrop(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4f {
    return to_clip(corner01(vi) * g.domain);
}

@fragment
fn fs_backdrop() -> @location(0) vec4f {
    return vec4f(0.012, 0.014, 0.022, 1.0);
}

// ---- Particles: instanced discs ---------------------------------------------------

struct ParticleOut {
    @builtin(position) clip: vec4f,
    @location(0) local: vec2f,   // [-1,1]^2 across the quad
    @location(1) speed: f32,
};

@vertex
fn vs_particle(
    @builtin(vertex_index) vi: u32,
    @location(0) pos: vec2f,
    @location(1) vel: vec2f,
) -> ParticleOut {
    let corner = corner01(vi) * 2.0 - 1.0;
    var o: ParticleOut;
    o.clip = to_clip(pos + corner * g.radius);
    o.local = corner;
    o.speed = length(vel);
    return o;
}

@fragment
fn fs_particle(i: ParticleOut) -> @location(0) vec4f {
    let d = length(i.local);
    // One-pixel anti-aliased edge regardless of zoom.
    let alpha = 1.0 - smoothstep(1.0 - fwidth(d), 1.0, d);
    if alpha <= 0.0 {
        discard;
    }
    let t = clamp(i.speed / g.max_speed, 0.0, 1.0);
    // The palette's base colour at rest, its highlight when fast (linear; the surface is sRGB).
    return vec4f(mix(g.base.rgb, g.highlight.rgb, t), alpha);
}

// ---- Walls: a signed distance field over the domain, shaded ---------------------

@group(1) @binding(0) var solid_tex: texture_2d<f32>;
@group(1) @binding(1) var solid_sampler: sampler;

struct Walls {
    brush: vec4f,   // centre, radius (metres); mode: 0 none, 1 paint, 2 erase
    texel: vec2f,   // one sample of the field, in texture coordinates
    range: f32,     // metres at either end of the stored distance
    step: f32,      // one sample of the field, metres
};
@group(1) @binding(2) var<uniform> w: Walls;

// Light from the top left, a little above the picture.
const LIGHT_2D: vec2f = vec2f(-0.6, 0.8);
const BEVEL: f32 = 0.007;          // metres from the edge to the flat top
const SHADOW_SHIFT: f32 = 0.007;   // how far the shadow falls from the wall
const SHADOW_SOFT: f32 = 0.014;    // and how far it fades
const RIM: f32 = 0.0016;           // width of the lit edge

struct SolidOut {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
};

@vertex
fn vs_solid(@builtin(vertex_index) vi: u32) -> SolidOut {
    let uv = corner01(vi);
    var o: SolidOut;
    o.clip = to_clip(uv * g.domain);
    o.uv = uv;   // row 0 of the mask is the floor, so v = y / height directly
    return o;
}

// The walls' signed distance at `uv`, metres, negative inside.
fn wall_distance(uv: vec2f) -> f32 {
    return (textureSampleLevel(solid_tex, solid_sampler, uv, 0.0).r * 2.0 - 1.0) * w.range;
}

// The same with the brush's stroke applied: what the walls would be.
fn shaped_distance(uv: vec2f) -> f32 {
    let d = wall_distance(uv);
    let brush = length(uv * g.domain - w.brush.xy) - w.brush.z;
    if w.brush.w == 1.0 {
        return min(d, brush);
    } else if w.brush.w == 2.0 {
        return max(d, -brush);
    }
    return d;
}

// A wall's colour at distance `d` inside it, its edge facing `n` (outward).
fn wall_colour(d: f32, n: vec2f) -> vec3f {
    // The bevel: steep at the edge, flat on top.
    let t = clamp(-d / BEVEL, 0.0, 1.0);
    let slope = 1.6 * (1.0 - t) * (1.0 - t);
    let normal = normalize(vec3f(n * slope, 1.0));
    let light = normalize(vec3f(LIGHT_2D * 0.8, 0.75));
    let lit = max(dot(normal, light), 0.0);
    // The palette's deep colour, a touch of its base, kept dark.
    let fill = mix(g.deep.rgb, g.base.rgb, 0.2) * 0.4 + vec3f(0.01, 0.011, 0.014);
    var c = fill * (0.25 + 1.1 * lit);
    // A thin lit edge, brightest facing the light.
    let facing = 0.3 + 0.7 * max(dot(n, normalize(LIGHT_2D)), 0.0);
    c += g.highlight.rgb * exp(min(d, 0.0) / RIM) * facing * 0.8;
    return c;
}

@fragment
fn fs_solid(i: SolidOut) -> @location(0) vec4f {
    let p = i.uv * g.domain;
    let aa = 0.75 * length(fwidth(p));
    let d0 = wall_distance(i.uv);
    let d = shaped_distance(i.uv);
    // The edge's outward direction, from the field's slope.
    let ex = vec2f(w.texel.x, 0.0);
    let ey = vec2f(0.0, w.texel.y);
    let n = normalize(vec2f(
        shaped_distance(i.uv + ex) - shaped_distance(i.uv - ex),
        shaped_distance(i.uv + ey) - shaped_distance(i.uv - ey),
    ) + vec2f(1e-6, 0.0));
    let now = 1.0 - smoothstep(-aa, aa, d0);
    let then = 1.0 - smoothstep(-aa, aa, d);
    // The shadow: dark where a wall stands toward the light.
    let toward = normalize(LIGHT_2D) * SHADOW_SHIFT / g.domain;
    let shadow = 0.6 * (1.0 - smoothstep(-0.002, SHADOW_SOFT, shaped_distance(i.uv + toward)));
    var colour = wall_colour(d, n);
    var cover = then;
    if w.brush.w == 1.0 {
        // Painting: the new wall shows faintly until it's painted.
        cover = now + (then - now) * 0.55;
    } else if w.brush.w == 2.0 {
        // Erasing: what goes stays as a red ghost.
        let ghost = max(now - then, 0.0) * 0.45;
        colour = mix(colour, vec3f(0.9, 0.18, 0.25), ghost / max(then + ghost, 1e-4));
        cover = then + ghost;
    }
    let alpha = cover + shadow * (1.0 - cover);
    if alpha <= 0.002 {
        discard;
    }
    return vec4f(colour * cover / alpha, alpha);
}
