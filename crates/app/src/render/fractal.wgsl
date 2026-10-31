// Escape-time fractals over the scene (see fractal.rs): one triangle over the canvas,
// each pixel iterating the fractal at four supersamples. Coloured from the palette by
// the smooth iteration count; opaque near the set's edge, clear far outside and inside,
// so the fluid shows through. The 3D kinds are raymarched instead, a sample a pixel: lit
// solids, a glow near them, clear beyond. Output is premultiplied.

struct Fractal {
    // The palette in linear RGB: deep, base, highlight.
    palette: array<vec4f, 3>,
    centre: vec2f,
    // The turn as (cos, sin).
    turn: vec2f,
    julia: vec2f,
    // Half the canvas's height, in units of the plane.
    half_height: f32,
    aspect: f32,
    kind: u32,
    iterations: u32,
    opacity: f32,
    brightness: f32,
    bands: f32,
    phase: f32,
    // One pixel, in the canvas's -1 to 1 (up the canvas).
    pixel: f32,
    // Iterations taken off the counts before colouring (a dive's depth).
    shift: f32,
    // The displacement's uv (scene, y down) to the canvas's -1 to 1 (y up); 0: unbent.
    bend: vec2f,
    // Samples a pixel: 4, or 1 when the frames can't keep up.
    samples: u32,
    _pad: f32,
    // This frame's offset of the samples, in pixels (so frames gathered together sample
    // between each other's points).
    jitter: vec2f,
    _pad2: vec2f,
    // The 3D kinds' camera: where it is, and its forward (w: the tangent of half the
    // field of view), right and up.
    eye: vec4f,
    forward: vec4f,
    right: vec4f,
    up: vec4f,
    // x: the Mandelbulb's power; yzw: the point the colours count out from.
    aim: vec4f,
};

@group(0) @binding(0) var<uniform> F: Fractal;
// How far the fluid has carried the picture at each pixel of the scene (uv), see bend.rs.
@group(0) @binding(1) var displacement: texture_2d<f32>;

const MANDELBROT: u32 = 0u;
const JULIA: u32 = 1u;
const BURNING_SHIP: u32 = 2u;
const TRICORN: u32 = 3u;
const NEWTON: u32 = 4u;
const MANDELBULB: u32 = 5u;
const MENGER: u32 = 6u;
// The Menger sponge diving: grown outward from its corner (see sponge_grown).
const MENGER_GROWN: u32 = 7u;

// The 3D kinds' bounds (a sphere about the origin), and the sponge's levels of holes:
// the finest smaller than a pixel wherever its dive goes, so its loop can't be seen.
const BULB_RADIUS: f32 = 1.3;
const SPONGE_RADIUS: f32 = 1.75;
const SPONGE_LEVELS: i32 = 9;
// How many times the diving sponge is grown (3⁴ its size), and how far off, in
// distances from the camera to the corner, its far parts fade out.
const GROWN: i32 = 4;
const GROWN_FADE: vec2f = vec2f(12.0, 25.0);

// |z|² past which a point has escaped (large, so the smooth count is smooth).
const BAILOUT: f32 = 256.0;

struct VOut {
    @builtin(position) pos: vec4f,
    // The canvas, -1 to 1 each way, y up.
    @location(0) ndc: vec2f,
};

@vertex
fn vs(@builtin(vertex_index) i: u32) -> VOut {
    // One triangle over the viewport: (-1, -1), (3, -1), (-1, 3).
    let p = vec2f(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    var o: VOut;
    o.pos = vec4f(p, 0.0, 1.0);
    o.ndc = p;
    return o;
}

fn mul(a: vec2f, b: vec2f) -> vec2f {
    return vec2f(a.x * b.x - a.y * b.y, a.x * b.y + a.y * b.x);
}

fn div(a: vec2f, b: vec2f) -> vec2f {
    return vec2f(a.x * b.x + a.y * b.y, a.y * b.x - a.x * b.y) / max(dot(b, b), 1e-20);
}

// The palette as a loop: deep, base, highlight, base, deep.
fn ramp(s: f32) -> vec3f {
    let t = fract(s) * 4.0;
    let deep = F.palette[0].rgb;
    let base = F.palette[1].rgb;
    let high = F.palette[2].rgb;
    if t < 1.0 {
        return mix(deep, base, t);
    } else if t < 2.0 {
        return mix(base, high, t - 1.0);
    } else if t < 3.0 {
        return mix(high, base, t - 2.0);
    }
    return mix(base, deep, t - 3.0);
}

// How far along to the set's edge a count is, 0 to 1 (logarithmic: the bands stay even
// as the counts grow toward the edge).
fn depth(n: f32) -> f32 {
    return clamp(log2(n + 1.0) / log2(f32(F.iterations) + 1.0), 0.0, 1.0);
}

// Premultiplied colour, alpha.
fn shade(s: f32, n: f32) -> vec4f {
    let d = depth(max(n - F.shift, 0.0));
    let a = pow(clamp(d * 1.6, 0.0, 1.0), 1.5) * F.opacity;
    return vec4f(ramp(s + F.bands * d + F.phase) * F.brightness * a, a);
}

fn escape(p: vec2f) -> vec4f {
    var z = p;
    var c = p;
    if F.kind == JULIA {
        c = F.julia;
    }
    var i = 0u;
    for (; i < F.iterations; i++) {
        if F.kind == BURNING_SHIP {
            z = abs(z);
        } else if F.kind == TRICORN {
            z.y = -z.y;
        }
        z = mul(z, z) + c;
        if dot(z, z) > BAILOUT {
            break;
        }
    }
    if i >= F.iterations {
        // Inside: clear.
        return vec4f(0.0);
    }
    // The smooth count: whole iterations, less how far past the bailout it landed.
    let n = max(f32(i) + 1.0 - log2(0.5 * log(dot(z, z))), 0.0);
    return shade(0.0, n);
}

fn newton(p: vec2f) -> vec4f {
    let roots = array<vec2f, 3>(vec2f(1.0, 0.0), vec2f(-0.5, 0.8660254), vec2f(-0.5, -0.8660254));
    var z = p;
    for (var i = 0u; i < F.iterations; i++) {
        // z − (z³ − 1) / 3z²
        let z2 = mul(z, z);
        z = z - div(mul(z2, z) - vec2f(1.0, 0.0), 3.0 * z2);
        for (var k = 0u; k < 3u; k++) {
            let d = dot(z - roots[k], z - roots[k]);
            if d < 1e-6 {
                // Smooth: converging quadratically, a point that only just made it
                // counts nearly a whole step more than one that landed deep inside.
                let n = f32(i) + 1.0 + clamp(log2(log(1e-6) / log(max(d, 1e-30))), -1.0, 0.0);
                return shade(f32(k) / 3.0, n);
            }
        }
    }
    return vec4f(0.0);
}

// The Mandelbulb's distance estimate at p, and how near the origin its orbit came (for
// the colour).
fn bulb(p: vec3f) -> vec2f {
    let n = F.aim.x;
    var z = p;
    var dr = 1.0;
    var r = length(z);
    var trap = r;
    for (var i = 0; i < 10; i++) {
        r = length(z);
        if r > 2.0 {
            break;
        }
        let theta = acos(clamp(z.z / max(r, 1e-9), -1.0, 1.0)) * n;
        let phi = atan2(z.y, z.x) * n;
        dr = pow(r, n - 1.0) * n * dr + 1.0;
        z = pow(r, n) * vec3f(sin(theta) * cos(phi), sin(phi) * sin(theta), cos(theta)) + p;
        trap = min(trap, length(z));
    }
    r = length(z);
    return vec2f(0.5 * log(max(r, 1e-9)) * r / dr, trap);
}

// The Menger sponge's distance (the cube -1..1, holes cut `levels` deep), at the point
// q back from its corner (1, 1, 1): measured from the corner, the finest holes near it
// keep their precision.
fn sponge(q: vec3f, levels: i32) -> f32 {
    let b = abs(vec3f(1.0) - q) - vec3f(1.0);
    var d = length(max(b, vec3f(0.0))) + min(max(b.x, max(b.y, b.z)), 0.0);
    var s = 1.0;
    for (var i = 0; i < levels; i++) {
        // The cell's -1..1 coordinate: mod(p s, 2) − 1, and p s = s − q s with s odd.
        let u = 1.0 - q * s;
        let a = u - 2.0 * floor(0.5 * u) - 1.0;
        s *= 3.0;
        let r = abs(1.0 - 3.0 * abs(a));
        let c = (min(max(r.x, r.y), min(max(r.y, r.z), max(r.z, r.x))) - 1.0) / s;
        d = max(d, c);
    }
    return d;
}

// The sponge grown outward from its corner: three times the size with the sponge as its
// corner, and again (`GROWN` times). Shrunk to a third about the corner it's itself
// everywhere, not only within the corner's own cube, so a dive into the corner loops
// without anything popping in or out at the wrap.
fn sponge_grown(p: vec3f) -> f32 {
    let k = pow(3.0, f32(GROWN));
    return k * sponge((vec3f(1.0) - p) / k, SPONGE_LEVELS + GROWN);
}

fn solid(p: vec3f) -> f32 {
    if F.kind == MANDELBULB {
        return bulb(p).x;
    }
    if F.kind == MENGER_GROWN {
        return sponge_grown(p);
    }
    return sponge(vec3f(1.0) - p, SPONGE_LEVELS);
}

// A ray from the camera through the canvas at ndc, marched to the solid: lit where it
// hits (coloured by the bulb's orbit, or by how many levels of holes in from the aim the
// sponge's surface is, less the dive's shift), a glow where it passes close.
fn march(ndc: vec2f) -> vec4f {
    let tan_half = F.forward.w;
    let dir = normalize(F.forward.xyz + (ndc.x * F.aspect * F.right.xyz + ndc.y * F.up.xyz) * tan_half);
    let eye = F.eye.xyz;
    var t = 0.0;
    var exit: f32;
    // The diving sponge has no bounds: it fades with distance (in camera distances
    // from its corner, the same at every turn of the loop).
    let camera = length(eye - F.aim.yzw);
    if F.kind == MENGER_GROWN {
        exit = GROWN_FADE.y * camera;
    } else {
        // Only inside the bounding sphere: rays that miss it cost nothing.
        let radius = select(SPONGE_RADIUS, BULB_RADIUS, F.kind == MANDELBULB);
        let b = dot(eye, dir);
        let disc = b * b - (dot(eye, eye) - radius * radius);
        if disc < 0.0 {
            return vec4f(0.0);
        }
        t = max(-b - sqrt(disc), 0.0);
        exit = -b + sqrt(disc);
    }
    // A pixel's width at a distance of 1.
    let pixel = F.pixel * tan_half;
    var closest = 1e9;
    var hit = false;
    var steps = 0u;
    for (; steps < F.iterations; steps++) {
        let d = solid(eye + dir * t);
        closest = min(closest, d / max(t, 1e-4));
        if d < 0.5 * pixel * t {
            hit = true;
            break;
        }
        // A little short of the estimate, which the bulb's overshoots now and then.
        t += 0.9 * d;
        if t > exit {
            break;
        }
    }
    if !hit {
        // Near misses glow (in the highlight), far ones are clear.
        let g = clamp(1.0 - closest * 60.0, 0.0, 1.0);
        let a = 0.5 * g * g * F.opacity;
        return vec4f(F.palette[2].rgb * F.brightness * a, a);
    }
    let p = eye + dir * t;
    let e = max(0.5 * pixel * t, 1e-5);
    let k = vec2f(1.0, -1.0);
    let normal = normalize(k.xyy * solid(p + k.xyy * e) + k.yyx * solid(p + k.yyx * e) + k.yxy * solid(p + k.yxy * e) + k.xxx * solid(p + k.xxx * e));
    let light = normalize(-F.forward.xyz + 0.6 * F.up.xyz + 0.4 * F.right.xyz);
    let diffuse = max(dot(normal, light), 0.0);
    let rim = pow(1.0 - max(dot(normal, -dir), 0.0), 3.0);
    // Many steps to get here: a crevice, in shadow.
    let open = 1.0 - f32(steps) / f32(F.iterations);
    var key: f32;
    if F.kind == MANDELBULB {
        key = 1.5 * bulb(p).y;
    } else {
        key = -log2(max(length(p - F.aim.yzw), 1e-6)) / log2(3.0) - F.shift;
    }
    let colour = ramp(0.5 * F.bands * key + F.phase);
    let lit = colour * (0.25 + 0.75 * diffuse) * (0.35 + 0.65 * open) + F.palette[2].rgb * rim * 0.5;
    var a = F.opacity;
    if F.kind == MENGER_GROWN {
        a *= 1.0 - smoothstep(GROWN_FADE.x, GROWN_FADE.y, t / camera);
    }
    return vec4f(lit * F.brightness * a, a);
}

fn sample(ndc: vec2f) -> vec4f {
    let local = vec2f(ndc.x * F.aspect, ndc.y) * F.half_height;
    let p = F.centre + vec2f(local.x * F.turn.x - local.y * F.turn.y, local.x * F.turn.y + local.y * F.turn.x);
    if F.kind == NEWTON {
        return newton(p);
    }
    return escape(p);
}

@fragment
fn fs(i: VOut) -> @location(0) vec4f {
    // Where the picture here came from, had the fluid carried it.
    let texel = min(vec2u(i.pos.xy), textureDimensions(displacement) - 1u);
    let origin = i.ndc - textureLoad(displacement, texel, 0).xy * F.bend + F.jitter * vec2f(1.0 / F.aspect, 1.0) * F.pixel;
    if F.kind >= MANDELBULB {
        return march(origin);
    }
    // Four samples on a rotated grid, a pixel apart in all: smooth filaments, and less
    // shimmer for the feedback to smear.
    let offsets = array<vec2f, 4>(vec2f(0.125, 0.375), vec2f(-0.375, 0.125), vec2f(-0.125, -0.375), vec2f(0.375, -0.125));
    if F.samples < 4u {
        return sample(origin);
    }
    var sum = vec4f(0.0);
    for (var k = 0u; k < 4u; k++) {
        sum += sample(origin + offsets[k] * vec2f(1.0 / F.aspect, 1.0) * F.pixel);
    }
    return sum * 0.25;
}
