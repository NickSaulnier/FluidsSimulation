// Post-processing: bloom and the composite to the screen.
//
// Bloom: the scene, soft-thresholded, is downsampled through a chain of half-size
// textures (13-tap filter), then blurred back up level by level (3×3 tent), each level
// added onto the one above. The composite adds it to the scene and tone-maps.

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
    _pad: f32,
    echo_flip: vec2f,
};

@group(0) @binding(0) var<uniform> post: Post;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var linear_clamp: sampler;
// The composite's bloom; a stand-in elsewhere.
@group(0) @binding(3) var bloom: texture_2d<f32>;

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

/// The picture (with its echo) plus bloom, tone-mapped per channel (bright fire goes
/// white, as it should).
@fragment
fn fs_composite(in: VOut) -> @location(0) vec4f {
    var scene = textureSampleLevel(src, linear_clamp, in.uv, 0.0).rgb;
    if post.echo > 0.0 {
        let echo_uv = 0.5 + (in.uv - 0.5) * post.echo_flip / post.echo_zoom;
        scene = mix(scene, textureSampleLevel(src, linear_clamp, echo_uv, 0.0).rgb, post.echo);
    }
    let glow = textureSampleLevel(bloom, linear_clamp, in.uv, 0.0).rgb;
    let c = max(scene + post.strength * glow, vec3f(0.0));
    return vec4f(tone(c.r), tone(c.g), tone(c.b), 1.0);
}
