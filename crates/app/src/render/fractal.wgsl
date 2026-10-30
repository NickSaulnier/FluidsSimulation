// Escape-time fractals over the scene (see fractal.rs): one triangle over the canvas,
// each pixel iterating the fractal at four supersamples. Coloured from the palette by
// the smooth iteration count; opaque near the set's edge, clear far outside and inside,
// so the fluid shows through. Output is premultiplied.

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
    _pad: vec2f,
};

@group(0) @binding(0) var<uniform> F: Fractal;
// How far the fluid has carried the picture at each pixel of the scene (uv), see bend.rs.
@group(0) @binding(1) var displacement: texture_2d<f32>;

const MANDELBROT: u32 = 0u;
const JULIA: u32 = 1u;
const BURNING_SHIP: u32 = 2u;
const TRICORN: u32 = 3u;
const NEWTON: u32 = 4u;

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
    let origin = i.ndc - textureLoad(displacement, texel, 0).xy * F.bend;
    // Four samples on a rotated grid, a pixel apart in all: smooth filaments, and less
    // shimmer for the feedback to smear.
    let offsets = array<vec2f, 4>(vec2f(0.125, 0.375), vec2f(-0.375, 0.125), vec2f(-0.125, -0.375), vec2f(0.375, -0.125));
    var sum = vec4f(0.0);
    for (var k = 0u; k < 4u; k++) {
        sum += sample(origin + offsets[k] * vec2f(1.0 / F.aspect, 1.0) * F.pixel);
    }
    return sum * 0.25;
}
