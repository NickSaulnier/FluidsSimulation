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

// ---- Walls: the solid mask texture over the domain --------------------------------

@group(1) @binding(0) var solid_tex: texture_2d<f32>;
@group(1) @binding(1) var solid_sampler: sampler;

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

@fragment
fn fs_solid(i: SolidOut) -> @location(0) vec4f {
    let occupancy = textureSample(solid_tex, solid_sampler, i.uv).r;
    // Bilinear filtering plus a narrow threshold turns blocky cells into smooth edges.
    let alpha = smoothstep(0.4, 0.6, occupancy);
    if alpha <= 0.0 {
        discard;
    }
    return vec4f(0.10, 0.11, 0.13, alpha);
}
