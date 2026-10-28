// Post-processing: bloom and the composite to the screen.
//
// Bloom: the scene, soft-thresholded, is downsampled through a chain of half-size
// textures (13-tap filter), then blurred back up level by level (3×3 tent), each level
// added onto the one above. The composite folds the picture into its symmetry, adds the
// echo and the bloom, tone-maps, then grades the colours (see composite.rs). During a
// blend between looks it mixes in a still of the old picture.

struct Post {
    // Brightness (linear) where glow starts, and the width of the soft start.
    threshold: f32,
    knee: f32,
    // Bloom added in the composite.
    strength: f32,
    // Echo: how much of a zoomed copy shows over the picture, its zoom, and its flip
    // (a sign per axis).
    echo: f32,
    echo_zoom: f32,
    aspect: f32,
    echo_flip: vec2f,
    // The composite's look (see composite.rs): symmetry (`Symmetry::index`), the
    // kaleidoscope's wedges and turn (radians), the hue's turn (radians), the grading,
    // the lens, the grain's seed, and the palette (linear) for mapping onto.
    symmetry: u32,
    segments: f32,
    turn: f32,
    hue: f32,
    saturation: f32,
    contrast: f32,
    exposure: f32,
    palette_map: f32,
    invert: f32,
    aberration: f32,
    vignette: f32,
    grain: f32,
    seed: f32,
    // A blend: how far the new look has come (1: no blend), its pattern
    // (`BlendPattern::index`) and seed.
    blend: f32,
    blend_pattern: u32,
    blend_seed: f32,
    deep: vec4f,
    base: vec4f,
    highlight: vec4f,
};

@group(0) @binding(0) var<uniform> post: Post;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var linear_clamp: sampler;
// The composite's bloom; a stand-in elsewhere.
@group(0) @binding(3) var bloom: texture_2d<f32>;
// The old picture, for a blend; a stand-in elsewhere.
@group(0) @binding(4) var still: texture_2d<f32>;

/// Brightness up to here passes the tone curve unchanged, so ordinary colours look as
/// they did before HDR; above it, highlights roll off smoothly toward 1.
const TONE_KNEE: f32 = 0.85;

struct VOut {
    @builtin(position) pos: vec4f,
    @location(0) uv: vec2f,
};

@vertex
fn vs(@builtin(vertex_index) i: u32) -> VOut {
    // One triangle covering the target.
    let p = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
    var o: VOut;
    o.pos = vec4f(p * 2.0 - 1.0, 0.0, 1.0);
    o.uv = vec2f(p.x, 1.0 - p.y);
    return o;
}

fn tap(uv: vec2f, offset: vec2f) -> vec3f {
    let texel = 1.0 / vec2f(textureDimensions(src));
    return textureSampleLevel(src, linear_clamp, uv + offset * texel, 0.0).rgb;
}

/// The 13-tap downsample from Jimenez's "Next generation post processing in Call of
/// Duty: Advanced Warfare" (2014): wide enough not to shimmer as things move.
fn downsample(uv: vec2f) -> vec3f {
    let a = tap(uv, vec2f(-2.0, 2.0));
    let b = tap(uv, vec2f(0.0, 2.0));
    let c = tap(uv, vec2f(2.0, 2.0));
    let d = tap(uv, vec2f(-2.0, 0.0));
    let e = tap(uv, vec2f(0.0, 0.0));
    let f = tap(uv, vec2f(2.0, 0.0));
    let g = tap(uv, vec2f(-2.0, -2.0));
    let h = tap(uv, vec2f(0.0, -2.0));
    let i = tap(uv, vec2f(2.0, -2.0));
    let j = tap(uv, vec2f(-1.0, 1.0));
    let k = tap(uv, vec2f(1.0, 1.0));
    let l = tap(uv, vec2f(-1.0, -1.0));
    let m = tap(uv, vec2f(1.0, -1.0));
    return e * 0.125 + (a + c + g + i) * 0.03125 + (b + d + f + h) * 0.0625 + (j + k + l + m) * 0.125;
}

/// The first level: downsample the scene and keep only what's above the threshold,
/// with a soft start so glow doesn't pop in.
@fragment
fn fs_prefilter(in: VOut) -> @location(0) vec4f {
    let c = downsample(in.uv);
    let brightness = max(max(c.r, c.g), c.b);
    var soft = clamp(brightness - post.threshold + post.knee, 0.0, 2.0 * post.knee);
    soft = soft * soft / (4.0 * post.knee + 1e-4);
    let keep = max(soft, brightness - post.threshold) / max(brightness, 1e-4);
    return vec4f(c * keep, 1.0);
}

@fragment
fn fs_down(in: VOut) -> @location(0) vec4f {
    return vec4f(downsample(in.uv), 1.0);
}

/// Blur the level below back up (3×3 tent), added onto this level by the blend state.
@fragment
fn fs_up(in: VOut) -> @location(0) vec4f {
    let c = tap(in.uv, vec2f(0.0, 0.0)) * 4.0
        + (tap(in.uv, vec2f(-1.0, 0.0)) + tap(in.uv, vec2f(1.0, 0.0)) + tap(in.uv, vec2f(0.0, -1.0)) + tap(in.uv, vec2f(0.0, 1.0))) * 2.0
        + tap(in.uv, vec2f(-1.0, -1.0)) + tap(in.uv, vec2f(1.0, -1.0)) + tap(in.uv, vec2f(-1.0, 1.0)) + tap(in.uv, vec2f(1.0, 1.0));
    return vec4f(c / 16.0, 1.0);
}

/// Unchanged up to the knee, then Reinhard-style toward 1 (slower than an exponential,
/// so bright fire keeps some gradation instead of going flat white).
fn tone(x: f32) -> f32 {
    if x <= TONE_KNEE {
        return x;
    }
    let room = 1.0 - TONE_KNEE;
    let over = x - TONE_KNEE;
    return TONE_KNEE + over / (1.0 + over / room);
}

/// `uv` reflected back into the picture (beyond an edge, the picture mirrored), so a
/// fold or a kaleidoscope reaching past the edges sees more picture, not smeared edges.
fn reflect_in(uv: vec2f) -> vec2f {
    let m = uv - 2.0 * floor(uv * 0.5);
    return 1.0 - abs(1.0 - m);
}

/// Where this pixel shows the picture from, under the symmetry.
fn fold(uv: vec2f) -> vec2f {
    switch post.symmetry {
        case 1u: {
            return vec2f(0.5 - abs(uv.x - 0.5), uv.y);
        }
        case 2u: {
            return vec2f(uv.x, 0.5 - abs(uv.y - 0.5));
        }
        case 3u: {
            return 0.5 - abs(uv - 0.5);
        }
        case 4u: {
            // Each wedge round the centre shows the same half-wedge, mirrored, turning.
            let square = vec2f(post.aspect, 1.0);
            let p = (uv - 0.5) * square;
            let wedge = 6.2831853 / max(post.segments, 1.0);
            var a = atan2(p.y, p.x) - post.turn;
            a = a - wedge * floor(a / wedge);
            a = min(a, wedge - a) + post.turn;
            let q = length(p) * vec2f(cos(a), sin(a));
            return reflect_in(q / square + 0.5);
        }
        default: {
            return uv;
        }
    }
}

fn luma(c: vec3f) -> f32 {
    return dot(c, vec3f(0.2126, 0.7152, 0.0722));
}

/// `c` turned `angle` radians round the grey axis (Rodrigues' rotation).
fn hue_turn(c: vec3f, angle: f32) -> vec3f {
    let k = vec3f(0.57735027);
    let s = sin(angle);
    let co = cos(angle);
    return c * co + cross(k, c) * s + k * dot(k, c) * (1.0 - co);
}

/// Brightness (0..1) onto the palette: black, deep, base, highlight.
fn gradient(l: f32) -> vec3f {
    let t = clamp(l, 0.0, 1.0) * 3.0;
    if t < 1.0 {
        return mix(vec3f(0.0), post.deep.rgb, t);
    }
    if t < 2.0 {
        return mix(post.deep.rgb, post.base.rgb, t - 1.0);
    }
    return mix(post.base.rgb, post.highlight.rgb, t - 2.0);
}

/// A pseudo-random number in 0..1 for a pixel and a seed.
fn hash(p: vec2f, seed: f32) -> f32 {
    let q = fract(vec3f(p.x, p.y, seed) * vec3f(0.1031, 0.1030, 0.0973));
    let r = q + dot(q, q.yzx + 33.33);
    return fract((r.x + r.y) * r.z);
}

/// Smooth noise in 0..1: hashes at the corners of a unit grid, blended.
fn value_noise(p: vec2f, seed: f32) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash(i, seed);
    let b = hash(i + vec2f(1.0, 0.0), seed);
    let c = hash(i + vec2f(0.0, 1.0), seed);
    let d = hash(i + vec2f(1.0, 1.0), seed);
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

/// How much of the new look shows at `uv` this far (`amount`) through a blend.
fn blend_mask(uv: vec2f, amount: f32) -> f32 {
    let soft = 0.25;
    switch post.blend_pattern {
        case 1u: {
            // From the middle out: 0 at the centre, 1 in the corners.
            let d = length((uv - 0.5) * vec2f(post.aspect, 1.0)) / length(vec2f(post.aspect, 1.0) * 0.5);
            return smoothstep(d, d + soft, amount * (1.0 + soft));
        }
        case 2u: {
            // In patches, two octaves of noise.
            let p = uv * vec2f(post.aspect, 1.0) * 5.0;
            let n = 0.65 * value_noise(p, post.blend_seed) + 0.35 * value_noise(p * 2.7, post.blend_seed + 7.0);
            return smoothstep(n, n + soft, amount * (1.0 + soft));
        }
        default: {
            return amount;
        }
    }
}

/// The old picture at `uv`, drifting toward the viewer as the blend goes on (with the
/// zoom blur, rushing and smeared along its rays).
fn old_picture(uv: vec2f, amount: f32) -> vec3f {
    if post.blend_pattern == 3u {
        var sum = vec3f(0.0);
        for (var k = 0; k < 8; k++) {
            let zoom = 1.0 + amount * (0.3 + 0.05 * f32(k));
            sum += textureSampleLevel(still, linear_clamp, 0.5 + (uv - 0.5) / zoom, 0.0).rgb;
        }
        return sum / 8.0;
    }
    let zoom = 1.0 + 0.12 * amount;
    return textureSampleLevel(still, linear_clamp, 0.5 + (uv - 0.5) / zoom, 0.0).rgb;
}

/// The picture folded into its symmetry, with chromatic aberration and its echo, plus
/// bloom, tone-mapped per channel (bright fire goes white, as it should), then graded.
@fragment
fn fs_composite(in: VOut) -> @location(0) vec4f {
    let uv = fold(in.uv);
    var scene: vec3f;
    if post.aberration > 0.0 {
        // Red pushed out from the centre, blue pulled in.
        let shift = (uv - 0.5) * post.aberration;
        scene = vec3f(
            textureSampleLevel(src, linear_clamp, uv + shift, 0.0).r,
            textureSampleLevel(src, linear_clamp, uv, 0.0).g,
            textureSampleLevel(src, linear_clamp, uv - shift, 0.0).b,
        );
    } else {
        scene = textureSampleLevel(src, linear_clamp, uv, 0.0).rgb;
    }
    if post.echo > 0.0 {
        let echo_uv = 0.5 + (uv - 0.5) * post.echo_flip / post.echo_zoom;
        scene = mix(scene, textureSampleLevel(src, linear_clamp, echo_uv, 0.0).rgb, post.echo);
    }
    let glow = textureSampleLevel(bloom, linear_clamp, uv, 0.0).rgb;
    let lit = max(scene + post.strength * glow, vec3f(0.0));
    var c = vec3f(tone(lit.r), tone(lit.g), tone(lit.b));

    // Grading, on the tone-mapped colours (each step leaves them alone at its default).
    if post.hue != 0.0 {
        c = max(hue_turn(c, post.hue), vec3f(0.0));
    }
    c = max(mix(vec3f(luma(c)), c, post.saturation), vec3f(0.0));
    if post.contrast != 1.0 {
        // About mid-grey as it looks (contrast in a roughly perceptual space).
        c = sqrt(c);
        c = max((c - 0.5) * post.contrast + 0.5, vec3f(0.0));
        c = c * c;
    }
    c = c * post.exposure;
    if post.palette_map > 0.0 {
        c = mix(c, gradient(sqrt(luma(c))), post.palette_map);
    }
    c = mix(c, 1.0 - min(c, vec3f(1.0)), post.invert);
    if post.vignette > 0.0 {
        let from_centre = (in.uv - 0.5) * vec2f(post.aspect, 1.0);
        let d = length(from_centre) / length(vec2f(post.aspect, 1.0) * 0.5);
        c = c * (1.0 - post.vignette * smoothstep(0.35, 1.0, d));
    }
    if post.grain > 0.0 {
        c = c + post.grain * (hash(in.pos.xy, post.seed) - 0.5);
    }
    c = clamp(c, vec3f(0.0), vec3f(1.0));
    if post.blend < 1.0 {
        c = mix(old_picture(in.uv, post.blend), c, blend_mask(in.uv, post.blend));
    }
    return vec4f(c, 1.0);
}
