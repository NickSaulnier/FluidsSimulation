// Voronoi cells around the particles (see voronoi.rs): every pixel takes the colour of
// the particle nearest it, so the fluid becomes a moving mosaic of cells, small where
// it's packed and large where it spreads, with edges between them.
//
// Pass 1 (seed): each particle (or every Nth) as one pixel of a seed texture holding
// its own position on the target (pixels) and its speed. Passes 2.. (flood): the jump
// flooding algorithm (Rong and Tan, 2006): each pixel looks at nine pixels `step` away
// and keeps the nearest seed any of them knows, the step halving from half the target
// down to 1. Last (shade, in the scene pass): each pixel's cell, filled from the palette
// by its particle's speed, edged where the next cell begins, and ended a few cells'
// spacing out from its particle so the air stays empty.
// (Globals duplicated from scene.wgsl; a separate module keeps its bind groups free.)
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

struct Voronoi {
    // Every `stride`th particle seeds a cell.
    stride: u32,
    // This flood pass's step, pixels.
    step: i32,
    // The cells' spacing (a typical cell's width) and how far from its particle a cell
    // reaches at most, pixels; the edges' width, pixels.
    spacing: f32,
    reach: f32,
    edge: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};
@group(1) @binding(0) var<uniform> V: Voronoi;
// The seeds known so far: (x, y) on the target in pixels, the particle's speed, 1 if
// there's one (0: none yet).
@group(1) @binding(1) var seeds: texture_2d<f32>;

fn to_clip(world: vec2f) -> vec4f {
    return vec4f(world * g.scale + g.offset, 0.0, 1.0);
}

// ---- Pass 1: seed --------------------------------------------------------------------

struct SeedOut {
    @builtin(position) clip: vec4f,
    @location(0) speed: f32,
};

@vertex
fn vs_seed(@builtin(instance_index) k: u32, @location(0) pos: vec2f, @location(1) vel: vec2f) -> SeedOut {
    var o: SeedOut;
    // Not a seed: outside the target, so nothing is drawn.
    o.clip = select(vec4f(2.0, 2.0, 0.0, 1.0), to_clip(pos), k % max(V.stride, 1u) == 0u);
    o.speed = length(vel);
    return o;
}

@fragment
fn fs_seed(i: SeedOut) -> @location(0) vec4f {
    return vec4f(i.clip.xy, i.speed, 1.0);
}

// ---- Passes 2..: flood -----------------------------------------------------------------

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    let p = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4f(p * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_flood(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let size = vec2i(textureDimensions(seeds));
    let at = vec2i(pos.xy);
    var best = vec4f(0.0);
    var nearest = 1e30;
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let q = at + vec2i(dx, dy) * V.step;
            if any(q < vec2i(0)) || any(q >= size) {
                continue;
            }
            let s = textureLoad(seeds, q, 0);
            if s.w > 0.0 {
                let d = dot(pos.xy - s.xy, pos.xy - s.xy);
                if d < nearest {
                    nearest = d;
                    best = s;
                }
            }
        }
    }
    return best;
}

// ---- Last: shade, over the domain in the scene pass -------------------------------------

@vertex
fn vs_shade(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4f {
    let corner = vec2f(f32(vi & 1u), f32(vi >> 1u));
    return to_clip(corner * g.domain);
}

/// How far `p` is inside its cell (seeded at `own`) from the edge with the cell seeded
/// at `other`: its distance to their bisector.
fn inside(p: vec2f, own: vec2f, other: vec2f) -> f32 {
    let dir = normalize(other - own);
    return dot(0.5 * (own + other) - p, dir);
}

@fragment
fn fs_shade(@builtin(position) frag: vec4f) -> @location(0) vec4f {
    let size = vec2i(textureDimensions(seeds));
    let at = vec2i(frag.xy);
    let s = textureLoad(seeds, at, 0);
    let d = distance(frag.xy, s.xy);
    if s.w <= 0.0 || d > V.reach {
        discard;
    }
    // The nearest edge: the bisectors with the cells around (any edge close enough to
    // draw passes between this pixel and a neighbour a few pixels off).
    var edge = 1e9;
    let ring = i32(ceil(V.edge)) + 2;
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let q = clamp(at + vec2i(dx, dy) * ring, vec2i(0), size - 1);
            let n = textureLoad(seeds, q, 0);
            if n.w > 0.0 && any(n.xy != s.xy) {
                edge = min(edge, inside(frag.xy, s.xy, n.xy));
            }
        }
    }
    // The fill: the base colour, toward the highlight as the particle runs faster,
    // brightest at the cell's heart.
    let fast = clamp(s.z / g.max_speed, 0.0, 1.0);
    let heart = clamp(d / V.spacing, 0.0, 1.0);
    var colour = mix(g.base.rgb, g.highlight.rgb, 0.7 * fast) * (1.0 - 0.45 * heart * heart);
    // A thin bright rim just inside the edge, then the edge itself in the deep colour
    // (leaded glass), anti-aliased.
    let line = 1.0 - smoothstep(0.5 * V.edge, 0.5 * V.edge + 1.0, edge);
    let rim = (1.0 - smoothstep(V.edge + 1.0, V.edge + 3.0, edge)) * (1.0 - line);
    colour += 0.25 * g.highlight.rgb * rim;
    colour = mix(colour, 0.25 * g.deep.rgb, line);
    // The cell fades out at its reach, so the fluid's outline is soft.
    let alpha = 1.0 - smoothstep(V.reach - 2.0, V.reach, d);
    return vec4f(colour, alpha);
}
