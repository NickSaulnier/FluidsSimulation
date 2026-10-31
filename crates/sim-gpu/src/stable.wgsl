// Stable fluids with dye on a 2D MAC grid (see stable.rs for the algorithm and the pass
// order). A 2D port of sim3d's smoke shader, with the CPU solver's boundaries: walls
// (solid), drains (outlets: φ = 0, dye removed) and emitter nozzles (cells whose faces
// carry the jet, releasing dye).
//
// Velocity: one buffer holding u ((nx+1)·ny), then v (nx·(ny+1)). pd: (pressure,
// divergence) per cell; the pressure is φ = p·dt/ρ, so the projection is u -= ∇φ.
//
// Fire mode (smoke and fire, as sim's fire.rs): the dye is the smoke; each cell also has
// gas fuel and temperature (carried with the flow, MacCormack like the dye) and solid
// fuel (which stays put, and smoulders once lit). Nozzles are burners, releasing lit,
// flickering fuel; hot gas rises; after advection, fuel burns into heat and soot, hot
// gas cools, and heat diffuses a little.

struct Params {
    n: vec2<u32>,
    nu: u32,
    n_emitters: u32,
    h: f32,
    dt: f32,
    buoyancy: f32,
    vorticity: f32,
    gravity: vec2<f32>,
    dissipation: f32,
    omega: f32,
    n_brushes: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    // Fire mode: 1 on; the substep (for the burners' flicker); then fire.rs's parameters
    // and constants.
    fire: u32,
    step: u32,
    _pad3: u32,
    _pad4: u32,
    ignition: f32,
    burn_rate: f32,
    heat: f32,
    cooling: f32,
    soot: f32,
    rise: f32,
    diffusion: f32,
    burner: f32,
    solid_burn: f32,
    embers_out: f32,
    flicker: f32,
    _pad5: f32,
}

@group(0) @binding(0) var<uniform> P: Params;
@group(0) @binding(1) var<storage, read_write> vel: array<f32>;
@group(0) @binding(2) var<storage, read_write> vel_tmp: array<f32>;
// Per cell: (the dye, MacCormack's forward step, its corrected result, unused). Packed,
// to keep within the 8 storage buffers a stage may bind.
@group(0) @binding(3) var<storage, read_write> dyes: array<vec4<f32>>;
@group(0) @binding(4) var<storage, read_write> pd: array<vec2<f32>>;
// Per cell: (its kind this substep, its kind before emitters: fluid, wall or drain,
// from the scene on the CPU).
@group(0) @binding(5) var<storage, read_write> flags: array<vec2<u32>>;
// Emitters (2 each: (pos, dir), (speed, width, 0, 0)), then brushes (3 each: (pos,
// radius, 0), (velocity, strength, 0), (spin, 0, 0, 0)).
@group(0) @binding(6) var<storage, read> geom: array<vec4<f32>>;
// The fastest speed in the fluid this substep (an f32's bits: positive floats order
// as their bits do), for the next substep's size.
@group(0) @binding(7) var<storage, read_write> max_speed: atomic<u32>;
// Fire mode, two per cell: (gas fuel, temperature, solid fuel, 1 if it's smouldering),
// then (fuel and temperature after MacCormack's forward step, and corrected).
@group(0) @binding(8) var<storage, read_write> fires: array<vec4<f32>>;

// Red-black Gauss-Seidel colour, set per pipeline.
override PARITY: u32 = 0u;

const FLUID: u32 = 0u;
const SOLID: u32 = 1u;
const DRAIN: u32 = 2u;
const INFLOW: u32 = 3u;
/// Wide nozzles release dye in stripes this far apart (wind-tunnel streaklines); those
/// narrower than `STRIPED_WIDTH` release solid dye (as the CPU solver's `nozzle_dye`).
const STRIPE_PITCH: f32 = 0.08;
const STRIPED_WIDTH: f32 = 0.25;
/// Fire: a burner's output holds for this many substeps between flickers, and the
/// temperatures drawn as the faintest and the fullest flame (as fire.rs).
const FLICKER_HOLD: u32 = 3u;
const FLAME_FAINT: f32 = 1.0;
const FLAME_FULL: f32 = 3.0;

fn dims() -> vec2<i32> {
    return vec2<i32>(P.n);
}

fn inside(c: vec2<i32>) -> bool {
    return all(c >= vec2<i32>(0)) && all(c < dims());
}

fn cell_index(c: vec2<i32>) -> u32 {
    return u32(c.x + dims().x * c.y);
}

/// Outside the domain counts as solid.
fn cell_type(c: vec2<i32>) -> u32 {
    if !inside(c) {
        return SOLID;
    }
    return flags[cell_index(c)].x & 0x0fu;
}

fn axis_unit(axis: u32) -> vec2<i32> {
    return vec2<i32>(vec2<u32>(axis) == vec2<u32>(0u, 1u));
}

fn comp_dims(axis: u32) -> vec2<i32> {
    return dims() + axis_unit(axis);
}

fn comp_index(axis: u32, c: vec2<i32>) -> u32 {
    let d = comp_dims(axis);
    return select(0u, P.nu, axis == 1u) + u32(c.x + d.x * c.y);
}

/// Where component sample (0, 0) sits, in cells: faces are on the cell walls.
fn comp_origin(axis: u32) -> vec2<f32> {
    return vec2<f32>(0.5) - 0.5 * vec2<f32>(axis_unit(axis));
}

/// A face with a prescribed velocity returns (1, that velocity); a free one (0, 0).
/// `c1` is the cell on the face's positive side.
fn fixed_face(axis: u32, c1: vec2<i32>) -> vec2<f32> {
    let c0 = c1 - axis_unit(axis);
    let t0 = cell_type(c0);
    let t1 = cell_type(c1);
    if t0 == SOLID || t1 == SOLID {
        return vec2<f32>(1.0, 0.0);
    }
    if t0 == INFLOW || t1 == INFLOW {
        let c = select(c1, c0, t0 == INFLOW);
        let e = flags[cell_index(c)].x >> 8u;
        let jet = geom[2u * e].zw * geom[2u * e + 1u].x;
        return vec2<f32>(1.0, jet[axis]);
    }
    return vec2<f32>(0.0, 0.0);
}

fn load_comp(axis: u32, c: vec2<i32>) -> f32 {
    return vel[comp_index(axis, c)];
}

fn sample_comp(axis: u32, p: vec2<f32>) -> f32 {
    let d = comp_dims(axis);
    let g = clamp(p / P.h - comp_origin(axis), vec2<f32>(0.0), vec2<f32>(d - 1));
    let c = min(vec2<i32>(floor(g)), d - 2);
    let f = g - vec2<f32>(c);
    let x0 = mix(load_comp(axis, c), load_comp(axis, c + vec2<i32>(1, 0)), f.x);
    let x1 = mix(load_comp(axis, c + vec2<i32>(0, 1)), load_comp(axis, c + vec2<i32>(1, 1)), f.x);
    return mix(x0, x1, f.y);
}

fn velocity_at(p: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(sample_comp(0u, p), sample_comp(1u, p));
}

/// Where the fluid at `p` was `dt` ago (RK2).
fn backtrace(p: vec2<f32>) -> vec2<f32> {
    let mid = p - 0.5 * P.dt * velocity_at(p);
    return p - P.dt * velocity_at(mid);
}

fn forwardtrace(p: vec2<f32>) -> vec2<f32> {
    let mid = p + 0.5 * P.dt * velocity_at(p);
    return p + P.dt * velocity_at(mid);
}

fn centre(c: vec2<i32>) -> vec2<f32> {
    return (vec2<f32>(c) + 0.5) * P.h;
}

struct Sampled {
    value: f32,
    lo: f32,
    hi: f32,
}

/// Bilinear sample of `field` (0: the dye, 1: the forward step's), with the range of
/// the four cells it blends.
fn sample_dye(p: vec2<f32>, field: u32) -> Sampled {
    let n = dims();
    let g = clamp(p / P.h - 0.5, vec2<f32>(0.0), vec2<f32>(n - 1));
    let c = min(vec2<i32>(floor(g)), max(n - 2, vec2<i32>(0)));
    let f = g - vec2<f32>(c);
    var v: array<f32, 4>;
    for (var k = 0; k < 4; k++) {
        let i = cell_index(min(c + vec2<i32>(k & 1, (k >> 1) & 1), n - 1));
        v[k] = select(dyes[i].x, dyes[i].y, field == 1u);
    }
    var s: Sampled;
    s.value = mix(mix(v[0], v[1], f.x), mix(v[2], v[3], f.x), f.y);
    s.lo = min(min(v[0], v[1]), min(v[2], v[3]));
    s.hi = max(max(v[0], v[1]), max(v[2], v[3]));
    return s;
}

/// How much fuel a burner releases at column `i` on substep `step`, relative to its
/// steady output: ±`flicker`, held for a few substeps, which seeds the flames' flicker.
/// A hash (32-bit, so not fire.rs's numbers, but the same spread), so runs repeat.
fn flicker(i: u32, step: u32) -> f32 {
    var x = (i * 0x9E3779B9u) ^ ((step / FLICKER_HOLD) * 0x85EBCA6Bu);
    x ^= x >> 16u;
    x *= 0x7FEB352Du;
    x ^= x >> 15u;
    x *= 0x846CA68Bu;
    x ^= x >> 16u;
    let unit = f32(x >> 8u) / 16777216.0;
    return 1.0 + P.flicker * (2.0 * unit - 1.0);
}

/// Bilinear sample of a fire field at `p` (0: fuel, 1: temperature; 2 and 3: the same
/// after the forward step), with the range of the four cells it blends.
fn sample_fire(p: vec2<f32>, field: u32) -> Sampled {
    let n = dims();
    let g = clamp(p / P.h - 0.5, vec2<f32>(0.0), vec2<f32>(n - 1));
    let c = min(vec2<i32>(floor(g)), max(n - 2, vec2<i32>(0)));
    let f = g - vec2<f32>(c);
    var v: array<f32, 4>;
    for (var k = 0; k < 4; k++) {
        let i = cell_index(min(c + vec2<i32>(k & 1, (k >> 1) & 1), n - 1));
        let w = fires[2u * i + field / 2u];
        v[k] = select(w.x, w.y, field % 2u == 1u);
    }
    var s: Sampled;
    s.value = mix(mix(v[0], v[1], f.x), mix(v[2], v[3], f.x), f.y);
    s.lo = min(min(v[0], v[1]), min(v[2], v[3]));
    s.hi = max(max(v[0], v[1]), max(v[2], v[3]));
    return s;
}

/// Cell kinds this substep: the scene's (walls, drains), plus the emitters' nozzles,
/// which hold their dye (in fire mode, lit fuel: burners); walls and drains hold none.
@compute @workgroup_size(16, 16)
fn classify(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = vec2<i32>(id.xy);
    if !inside(c) {
        return;
    }
    let i = cell_index(c);
    var t = flags[i].y;
    if t != SOLID {
        let p = centre(c);
        for (var e = 0u; e < P.n_emitters; e++) {
            let a = geom[2u * e];
            let d = p - a.xy;
            let across = dot(d, vec2<f32>(-a.w, a.z));
            let width = geom[2u * e + 1u].y;
            let dye = geom[2u * e + 1u].z;
            if abs(across) <= 0.5 * max(width, P.h) && abs(dot(d, a.zw)) <= P.h {
                t = INFLOW | (e << 8u);
                let stripe = (i32(floor(across / (0.5 * STRIPE_PITCH))) % 2 + 2) % 2 == 0;
                let amount = select(0.0, dye, width < STRIPED_WIDTH || stripe);
                if P.fire == 1u {
                    dyes[i].x = 0.0;
                    fires[2u * i] = vec4<f32>(amount * flicker(u32(c.x), P.step), P.burner, 0.0, 0.0);
                } else {
                    dyes[i].x = amount;
                }
                break;
            }
        }
    }
    flags[i].x = t;
    if t == SOLID || t == DRAIN {
        dyes[i].x = 0.0;
        if P.fire == 1u {
            fires[2u * i] = vec4<f32>(0.0);
        }
    }
    if t != FLUID {
        pd[i].x = 0.0;
    }
}

/// Prescribed faces get their velocity: zero at walls, the jet at nozzles.
@compute @workgroup_size(16, 16)
fn boundaries(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = vec2<i32>(id.xy);
    for (var axis = 0u; axis < 2u; axis++) {
        if all(c < comp_dims(axis)) {
            let fix = fixed_face(axis, c);
            if fix.x > 0.0 {
                vel[comp_index(axis, c)] = fix.y;
            }
        }
    }
}

/// How heavy a cell's gas is, as a share of gravity: its dye (negative buoyancy:
/// light dye rises), less, in fire mode, its heat (hot gas rises).
fn heaviness(c: vec2<i32>) -> f32 {
    let i = cell_index(clamp(c, vec2<i32>(0), dims() - 1));
    var h = P.buoyancy * dyes[i].x;
    if P.fire == 1u {
        h -= P.rise * fires[2u * i].y;
    }
    return h;
}

/// Boussinesq buoyancy: each face accelerates along gravity in proportion to the
/// heaviness on it.
@compute @workgroup_size(16, 16)
fn forces(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = vec2<i32>(id.xy);
    for (var axis = 0u; axis < 2u; axis++) {
        if all(c < comp_dims(axis)) && fixed_face(axis, c).x == 0.0 {
            let d = 0.5 * (heaviness(c) + heaviness(c - axis_unit(axis)));
            vel[comp_index(axis, c)] += P.dt * P.gravity[axis] * d;
        }
    }
}

/// Cell-centred velocity, from the faces around the cell (clamped to the domain).
fn cell_velocity(c0: vec2<i32>) -> vec2<f32> {
    let c = clamp(c0, vec2<i32>(0), dims() - 1);
    return vec2<f32>(0.5 * (load_comp(0u, c) + load_comp(0u, c + vec2<i32>(1, 0))), 0.5 * (load_comp(1u, c) + load_comp(1u, c + vec2<i32>(0, 1))));
}

/// Vorticity confinement, first half: the curl at each cell, into `vel_tmp` (free until
/// the velocity is advected).
@compute @workgroup_size(16, 16)
fn curl(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = vec2<i32>(id.xy);
    if !inside(c) {
        return;
    }
    let dx = cell_velocity(c + vec2<i32>(1, 0)) - cell_velocity(c - vec2<i32>(1, 0));
    let dy = cell_velocity(c + vec2<i32>(0, 1)) - cell_velocity(c - vec2<i32>(0, 1));
    vel_tmp[cell_index(c)] = (dx.y - dy.x) / (2.0 * P.h);
}

fn curl_at(c0: vec2<i32>) -> f32 {
    return vel_tmp[cell_index(clamp(c0, vec2<i32>(0), dims() - 1))];
}

/// The confinement force at a cell: ε·h·(N × ω), N pointing up the gradient of |ω|.
fn confinement(c: vec2<i32>) -> vec2<f32> {
    let grad = vec2<f32>(
        abs(curl_at(c + vec2<i32>(1, 0))) - abs(curl_at(c - vec2<i32>(1, 0))),
        abs(curl_at(c + vec2<i32>(0, 1))) - abs(curl_at(c - vec2<i32>(0, 1))),
    );
    let len = length(grad);
    if len < 1e-6 {
        return vec2<f32>(0.0);
    }
    let n = grad / len;
    let w = curl_at(c);
    return P.vorticity * P.h * vec2<f32>(n.y * w, -n.x * w);
}

/// Vorticity confinement, second half: the force, averaged onto the free faces.
@compute @workgroup_size(16, 16)
fn confine(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = vec2<i32>(id.xy);
    for (var axis = 0u; axis < 2u; axis++) {
        if all(c < comp_dims(axis)) && fixed_face(axis, c).x == 0.0 {
            let f = 0.5 * (confinement(c)[axis] + confinement(c - axis_unit(axis))[axis]);
            vel[comp_index(axis, c)] += P.dt * f;
        }
    }
}

/// Brushes (the mouse, music reactions): inside each disc the velocity is pulled
/// toward the brush's push plus its spin, fading to the rim, at `strength` per second.
@compute @workgroup_size(16, 16)
fn brushes(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = vec2<i32>(id.xy);
    for (var axis = 0u; axis < 2u; axis++) {
        if !all(c < comp_dims(axis)) || fixed_face(axis, c).x != 0.0 {
            continue;
        }
        let p = (vec2<f32>(c) + comp_origin(axis)) * P.h;
        let i = comp_index(axis, c);
        var u = vel[i];
        for (var b = 0u; b < P.n_brushes; b++) {
            let base = 2u * P.n_emitters + 3u * b;
            let a = geom[base];
            let d = distance(p, a.xy);
            if d < a.z {
                let push = geom[base + 1u];
                let r = p - a.xy;
                let goal = push.xy + geom[base + 2u].x * vec2<f32>(-r.y, r.x);
                u += (goal[axis] - u) * min(push.z * (1.0 - d / a.z) * P.dt, 1.0);
            }
        }
        vel[i] = u;
    }
}

/// Semi-Lagrangian advection of the velocity into `vel_tmp`.
@compute @workgroup_size(16, 16)
fn advect_velocity(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = vec2<i32>(id.xy);
    for (var axis = 0u; axis < 2u; axis++) {
        if all(c < comp_dims(axis)) {
            let fix = fixed_face(axis, c);
            var value = fix.y;
            if fix.x == 0.0 {
                value = sample_comp(axis, backtrace((vec2<f32>(c) + comp_origin(axis)) * P.h));
            }
            vel_tmp[comp_index(axis, c)] = value;
        }
    }
}

/// The advected velocity back into `vel`.
@compute @workgroup_size(16, 16)
fn commit_velocity(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = vec2<i32>(id.xy);
    for (var axis = 0u; axis < 2u; axis++) {
        if all(c < comp_dims(axis)) {
            let i = comp_index(axis, c);
            vel[i] = vel_tmp[i];
        }
    }
}

/// The divergence of each fluid cell, and the fastest speed in the fluid.
@compute @workgroup_size(16, 16)
fn divergence(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = vec2<i32>(id.xy);
    if !inside(c) {
        return;
    }
    var div = 0.0;
    if cell_type(c) == FLUID {
        div = (load_comp(0u, c + vec2<i32>(1, 0)) - load_comp(0u, c) + load_comp(1u, c + vec2<i32>(0, 1)) - load_comp(1u, c)) / P.h;
        atomicMax(&max_speed, bitcast<u32>(length(cell_velocity(c))));
    }
    pd[cell_index(c)].y = div;
}

/// One colour of a red-black SOR sweep for ∇²φ = div: fluid neighbours couple, drains
/// hold φ = 0, walls and nozzles are Neumann.
@compute @workgroup_size(16, 16)
fn pressure(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = vec2<i32>(id.xy);
    if !inside(c) || u32(c.x + c.y) % 2u != PARITY {
        return;
    }
    let i = cell_index(c);
    if (flags[i].x & 0x0fu) != FLUID {
        return;
    }
    var sum = 0.0;
    var count = 0.0;
    for (var axis = 0u; axis < 2u; axis++) {
        for (var side = -1; side <= 1; side += 2) {
            let nb = c + side * axis_unit(axis);
            let t = cell_type(nb);
            if t == FLUID {
                sum += pd[cell_index(nb)].x;
                count += 1.0;
            } else if t == DRAIN {
                count += 1.0;
            }
        }
    }
    if count == 0.0 {
        return;
    }
    let gs = (sum - P.h * P.h * pd[i].y) / count;
    pd[i].x = mix(pd[i].x, gs, P.omega);
}

fn phi(c: vec2<i32>) -> f32 {
    if cell_type(c) == FLUID {
        return pd[cell_index(c)].x;
    }
    return 0.0;
}

/// Subtract the pressure gradient on the free faces.
@compute @workgroup_size(16, 16)
fn project(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = vec2<i32>(id.xy);
    for (var axis = 0u; axis < 2u; axis++) {
        if all(c < comp_dims(axis)) && fixed_face(axis, c).x == 0.0 {
            vel[comp_index(axis, c)] -= (phi(c) - phi(c - axis_unit(axis))) / P.h;
        }
    }
}

/// MacCormack, first half: the plain semi-Lagrangian result into `dye_hat`.
@compute @workgroup_size(16, 16)
fn advect_forward(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = vec2<i32>(id.xy);
    if !inside(c) {
        return;
    }
    let i = cell_index(c);
    if cell_type(c) == FLUID {
        let src = backtrace(centre(c));
        dyes[i].y = sample_dye(src, 0u).value;
        if P.fire == 1u {
            fires[2u * i + 1u] = vec4<f32>(sample_fire(src, 0u).value, sample_fire(src, 1u).value, 0.0, 0.0);
        }
    } else {
        dyes[i].y = dyes[i].x;
        if P.fire == 1u {
            fires[2u * i + 1u] = vec4<f32>(fires[2u * i].xy, 0.0, 0.0);
        }
    }
}

/// MacCormack, second half: correct by half the round-trip error, clamped to the cells
/// the value came from; then fade.
@compute @workgroup_size(16, 16)
fn advect_correct(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = vec2<i32>(id.xy);
    if !inside(c) {
        return;
    }
    let i = cell_index(c);
    if cell_type(c) != FLUID {
        dyes[i].z = dyes[i].x;
        if P.fire == 1u {
            fires[2u * i + 1u] = vec4<f32>(fires[2u * i + 1u].xy, fires[2u * i].xy);
        }
        return;
    }
    let p = centre(c);
    let src = backtrace(p);
    let to = forwardtrace(p);
    let origin = sample_dye(src, 0u);
    let back = sample_dye(to, 1u);
    let d = clamp(dyes[i].y + 0.5 * (dyes[i].x - back.value), origin.lo, origin.hi);
    dyes[i].z = max(d, 0.0) * exp(-P.dissipation * P.dt);
    if P.fire == 1u {
        let now = fires[2u * i];
        let hat = fires[2u * i + 1u];
        var out = vec2<f32>(0.0);
        for (var k = 0u; k < 2u; k++) {
            let o = sample_fire(src, k);
            let b = sample_fire(to, k + 2u);
            out[k] = max(clamp(hat[k] + 0.5 * (now[k] - b.value), o.lo, o.hi), 0.0);
        }
        fires[2u * i + 1u] = vec4<f32>(hat.xy, out);
    }
}

/// The advected dye into place, and a compact copy of it in `vel_tmp` (free until the
/// next substep) for reading back.
@compute @workgroup_size(16, 16)
fn commit_dye(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = vec2<i32>(id.xy);
    if inside(c) {
        let i = cell_index(c);
        dyes[i].x = dyes[i].z;
        vel_tmp[i] = dyes[i].z;
        if P.fire == 1u {
            fires[2u * i] = vec4<f32>(fires[2u * i + 1u].zw, fires[2u * i].zw);
        }
    }
}

/// Fire: burn, release heat and soot (into the smoke), and cool (as fire.rs's
/// `react`). Gas fuel burns where the gas is hot enough; solid fuel catches there and
/// smoulders until it's spent. Hot gas cools as dT/dt = -cooling T², integrated
/// implicitly. Walls and burners are left alone.
@compute @workgroup_size(16, 16)
fn react(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = vec2<i32>(id.xy);
    if !inside(c) {
        return;
    }
    let t = cell_type(c);
    if t == SOLID || t == INFLOW {
        return;
    }
    let i = cell_index(c);
    let f = fires[2u * i];
    var fuel = f.x;
    var temp = f.y;
    var solid = f.z;
    let hot = temp >= P.ignition;
    var burning = f.w > 0.5 || (hot && solid > 0.0);
    let gas_fraction = 1.0 - exp(-P.burn_rate * P.dt);
    let solid_fraction = 1.0 - exp(-P.solid_burn * P.burn_rate * P.dt);
    var burned = 0.0;
    if hot {
        burned += fuel * gas_fraction;
        fuel *= 1.0 - gas_fraction;
    }
    if burning {
        burned += solid * solid_fraction;
        solid *= 1.0 - solid_fraction;
        if solid < P.embers_out {
            solid = 0.0;
            burning = false;
        }
    }
    temp += P.heat * burned;
    dyes[i].x = min(dyes[i].x + P.soot * burned, 1.0);
    temp /= 1.0 + P.cooling * max(temp, 0.0) * P.dt;
    fires[2u * i] = vec4<f32>(fuel, temp, solid, select(0.0, 1.0, burning));
}

fn open_cell(c: vec2<i32>) -> bool {
    let t = cell_type(c);
    return t != SOLID && t != INFLOW;
}

/// Fire: heat diffusion, explicit and capped for stability (it's a small effect), into
/// the forward step's slot (free now); then back.
@compute @workgroup_size(16, 16)
fn diffuse(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = vec2<i32>(id.xy);
    if !inside(c) {
        return;
    }
    let i = cell_index(c);
    var temp = fires[2u * i].y;
    if open_cell(c) {
        let k = min(P.diffusion * P.dt / (P.h * P.h), 0.2);
        var flux = 0.0;
        for (var axis = 0u; axis < 2u; axis++) {
            for (var side = -1; side <= 1; side += 2) {
                let nb = c + side * axis_unit(axis);
                if inside(nb) && open_cell(nb) {
                    flux += fires[2u * cell_index(nb)].y - temp;
                }
            }
        }
        temp += k * flux;
    }
    fires[2u * i + 1u].y = temp;
}

@compute @workgroup_size(16, 16)
fn commit_heat(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = vec2<i32>(id.xy);
    if inside(c) {
        let i = cell_index(c);
        fires[2u * i].y = fires[2u * i + 1u].y;
    }
}

/// Fire: compact copies for reading back, in `vel_tmp` (free until the next substep):
/// the smoke, then the flame's brightness and all the fuel, packed as two halves.
@compute @workgroup_size(16, 16)
fn compact_fire(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = vec2<i32>(id.xy);
    if inside(c) {
        let i = cell_index(c);
        let f = fires[2u * i];
        let flame = clamp((f.y - FLAME_FAINT) / (FLAME_FULL - FLAME_FAINT), 0.0, 1.0);
        let n = u32(dims().x * dims().y);
        vel_tmp[i] = dyes[i].x;
        vel_tmp[n + i] = bitcast<f32>(pack2x16float(vec2<f32>(flame, min(f.x + f.z, 1.0))));
    }
}
