// 3D smoke and fire on a MAC grid (see smoke.rs for the algorithm and the pass order).
//
// Velocity: one buffer holding u ((nx+1)·ny·nz), then v (nx·(ny+1)·nz), then w
// (nx·ny·(nz+1)). Scalars: (smoke, temperature, fuel, unused) per cell. pd: (pressure,
// divergence) per cell; the pressure is φ = p·dt/ρ, so the projection is u -= ∇φ.

struct Params {
    n: vec3<u32>,
    h: f32,
    gravity: vec3<f32>,
    dt: f32,
    rise: f32,
    heaviness: f32,
    cooling: f32,
    fade: f32,
    burner_temp: f32,
    omega: f32,
    n_shapes: u32,
    n_vents: u32,
    n_burners: u32,
    nu: u32,
    nv: u32,
    /// Simulated seconds, for the burners' flicker.
    time: f32,
    ignition: f32,
    burn_rate: f32,
    heat: f32,
    soot: f32,
    vorticity: f32,
    burner_fuel: f32,
    burner_smoke: f32,
    flicker: f32,
    n_brushes: u32,
    /// Heat lost to the surroundings, 1/s: warm gas cools away instead of collecting.
    heat_loss: f32,
    /// Air drag, 1/s: velocity decays at this rate.
    drag: f32,
    _pad2: u32,
}

@group(0) @binding(0) var<uniform> P: Params;
@group(0) @binding(1) var<storage, read_write> vel: array<f32>;
@group(0) @binding(2) var<storage, read_write> vel_tmp: array<f32>;
@group(0) @binding(3) var<storage, read_write> scal: array<vec4<f32>>;
@group(0) @binding(4) var<storage, read_write> scal_hat: array<vec4<f32>>;
@group(0) @binding(5) var<storage, read_write> scal_tmp: array<vec4<f32>>;
@group(0) @binding(6) var<storage, read_write> pd: array<vec2<f32>>;
@group(0) @binding(7) var<storage, read_write> flags: array<u32>;
// Shapes (2 each: a sphere is (centre, radius), a box (min, -1) then (max, 0)), then
// vents (min, max), then burners ((pos, radius), (dir, speed)), then brushes (3 each:
// (pos, radius), (velocity, strength), (spin, unused)).
@group(0) @binding(8) var<storage, read> geom: array<vec4<f32>>;
// (smoke, temperature, solid, fuel) for drawing: a filterable copy of `scal`, with
// solid cells marked so the renderer can shadow with them.
@group(0) @binding(9) var volume: texture_storage_3d<rgba16float, write>;

// Red-black Gauss-Seidel colour, set per pipeline.
override PARITY: u32 = 0u;

const FLUID: u32 = 0u;
const SOLID: u32 = 1u;
const INFLOW: u32 = 3u;
/// A fluid cell inside a vent, open toward x (then y, z: shifted by the axis): where it
/// touches the domain's wall across that axis, the wall is open. A vent opens only the
/// wall it lies flat against (its thinnest direction), so a side vent reaching down to
/// the floor doesn't open the floor too.
const OPEN_X: u32 = 0x10u;
/// Fuel too cool to burn thins out at this rate, 1/s: stray fuel that drifts back down
/// would otherwise collect and flare up along the floor. (A burner's fuel leaves it
/// already alight, so the flames themselves don't lose any.)
const FUEL_DILUTION: f32 = 1.0;

fn dims() -> vec3<i32> {
    return vec3<i32>(P.n);
}

fn inside(c: vec3<i32>) -> bool {
    return all(c >= vec3<i32>(0)) && all(c < dims());
}

fn cell_index(c: vec3<i32>) -> u32 {
    let n = dims();
    return u32(c.x + n.x * (c.y + n.y * c.z));
}

/// Outside the domain counts as solid: the box walls (open only at vents).
fn cell_type(c: vec3<i32>) -> u32 {
    if !inside(c) {
        return SOLID;
    }
    return flags[cell_index(c)] & 0x0fu;
}

fn is_open(c: vec3<i32>, axis: u32) -> bool {
    return inside(c) && (flags[cell_index(c)] & (OPEN_X << axis)) != 0u;
}

fn axis_unit(axis: u32) -> vec3<i32> {
    return vec3<i32>(vec3<u32>(axis) == vec3<u32>(0u, 1u, 2u));
}

fn comp_dims(axis: u32) -> vec3<i32> {
    return dims() + axis_unit(axis);
}

fn comp_offset(axis: u32) -> u32 {
    return select(select(0u, P.nu, axis >= 1u), P.nu + P.nv, axis == 2u);
}

fn comp_index(axis: u32, c: vec3<i32>) -> u32 {
    let d = comp_dims(axis);
    return comp_offset(axis) + u32(c.x + d.x * (c.y + d.y * c.z));
}

/// Where component sample (0, 0, 0) sits, in cells: faces are on the cell walls.
fn comp_origin(axis: u32) -> vec3<f32> {
    return vec3<f32>(0.5) - 0.5 * vec3<f32>(axis_unit(axis));
}

fn burner_base(b: u32) -> u32 {
    return 2u * (P.n_shapes + P.n_vents + b);
}

/// A face with a prescribed velocity returns (1, that velocity); a free one (0, 0).
/// `c1` is the cell on the face's positive side, `c0` the one below it on `axis`.
/// The domain's wall is open where a vent meets it (the projection sets those faces).
fn fixed_face(axis: u32, c1: vec3<i32>) -> vec2<f32> {
    let c0 = c1 - axis_unit(axis);
    let t0 = cell_type(c0);
    let t1 = cell_type(c1);
    if (!inside(c0) && is_open(c1, axis)) || (!inside(c1) && is_open(c0, axis)) {
        return vec2<f32>(0.0, 0.0);
    }
    if t0 == SOLID || t1 == SOLID {
        return vec2<f32>(1.0, 0.0);
    }
    if t0 == INFLOW || t1 == INFLOW {
        let c = select(c1, c0, t0 == INFLOW);
        let d = geom[burner_base(flags[cell_index(c)] >> 8u) + 1u];
        return vec2<f32>(1.0, d[axis] * d.w);
    }
    return vec2<f32>(0.0, 0.0);
}

fn load_comp(axis: u32, c: vec3<i32>) -> f32 {
    return vel[comp_index(axis, c)];
}

fn sample_comp(axis: u32, p: vec3<f32>) -> f32 {
    let d = comp_dims(axis);
    let g = clamp(p / P.h - comp_origin(axis), vec3<f32>(0.0), vec3<f32>(d - 1));
    let c = min(vec3<i32>(floor(g)), d - 2);
    let f = g - vec3<f32>(c);
    let x00 = mix(load_comp(axis, c), load_comp(axis, c + vec3<i32>(1, 0, 0)), f.x);
    let x10 = mix(load_comp(axis, c + vec3<i32>(0, 1, 0)), load_comp(axis, c + vec3<i32>(1, 1, 0)), f.x);
    let x01 = mix(load_comp(axis, c + vec3<i32>(0, 0, 1)), load_comp(axis, c + vec3<i32>(1, 0, 1)), f.x);
    let x11 = mix(load_comp(axis, c + vec3<i32>(0, 1, 1)), load_comp(axis, c + vec3<i32>(1, 1, 1)), f.x);
    return mix(mix(x00, x10, f.y), mix(x01, x11, f.y), f.z);
}

fn velocity_at(p: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(sample_comp(0u, p), sample_comp(1u, p), sample_comp(2u, p));
}

/// Where the fluid at `p` was `dt` ago (RK2).
fn backtrace(p: vec3<f32>) -> vec3<f32> {
    let mid = p - 0.5 * P.dt * velocity_at(p);
    return p - P.dt * velocity_at(mid);
}

fn forwardtrace(p: vec3<f32>) -> vec3<f32> {
    let mid = p + 0.5 * P.dt * velocity_at(p);
    return p + P.dt * velocity_at(mid);
}

fn centre(c: vec3<i32>) -> vec3<f32> {
    return (vec3<f32>(c) + 0.5) * P.h;
}

struct Sampled {
    value: vec4<f32>,
    lo: vec4<f32>,
    hi: vec4<f32>,
}

/// Trilinear sample of the scalars, with the range of the eight cells it blends.
fn sample_scal(p: vec3<f32>) -> Sampled {
    let n = dims();
    let g = clamp(p / P.h - 0.5, vec3<f32>(0.0), vec3<f32>(n - 1));
    let c = min(vec3<i32>(floor(g)), max(n - 2, vec3<i32>(0)));
    let f = g - vec3<f32>(c);
    var v: array<vec4<f32>, 8>;
    for (var k = 0; k < 8; k++) {
        let o = vec3<i32>(k & 1, (k >> 1) & 1, (k >> 2) & 1);
        v[k] = scal[cell_index(min(c + o, n - 1))];
    }
    var s: Sampled;
    let y0 = mix(mix(v[0], v[1], f.x), mix(v[2], v[3], f.x), f.y);
    let y1 = mix(mix(v[4], v[5], f.x), mix(v[6], v[7], f.x), f.y);
    s.value = mix(y0, y1, f.z);
    s.lo = min(min(min(v[0], v[1]), min(v[2], v[3])), min(min(v[4], v[5]), min(v[6], v[7])));
    s.hi = max(max(max(v[0], v[1]), max(v[2], v[3])), max(max(v[4], v[5]), max(v[6], v[7])));
    return s;
}

fn sample_hat(p: vec3<f32>) -> vec4<f32> {
    let n = dims();
    let g = clamp(p / P.h - 0.5, vec3<f32>(0.0), vec3<f32>(n - 1));
    let c = min(vec3<i32>(floor(g)), max(n - 2, vec3<i32>(0)));
    let f = g - vec3<f32>(c);
    var v: array<vec4<f32>, 8>;
    for (var k = 0; k < 8; k++) {
        let o = vec3<i32>(k & 1, (k >> 1) & 1, (k >> 2) & 1);
        v[k] = scal_hat[cell_index(min(c + o, n - 1))];
    }
    let y0 = mix(mix(v[0], v[1], f.x), mix(v[2], v[3], f.x), f.y);
    let y1 = mix(mix(v[4], v[5], f.x), mix(v[6], v[7], f.x), f.y);
    return mix(y0, y1, f.z);
}

fn in_burner(p: vec3<f32>, b: u32) -> bool {
    let a = geom[burner_base(b)];
    let d = geom[burner_base(b) + 1u];
    let r = p - a.xyz;
    let along = dot(r, d.xyz);
    let radial = length(r - along * d.xyz);
    // At least one layer of cells, and at least one cell across.
    return abs(along) <= 0.75 * P.h && radial <= max(a.w, 0.75 * P.h);
}

fn in_solid(p: vec3<f32>) -> bool {
    for (var s = 0u; s < P.n_shapes; s++) {
        let a = geom[2u * s];
        let b = geom[2u * s + 1u];
        if a.w >= 0.0 {
            if distance(p, a.xyz) < a.w {
                return true;
            }
        } else if all(p >= a.xyz) && all(p <= b.xyz) {
            return true;
        }
    }
    return false;
}

/// The open bits for a cell at `p`: for each vent it's in, the vent's thinnest axis.
fn vent_open_bits(p: vec3<f32>) -> u32 {
    var bits = 0u;
    for (var v = 0u; v < P.n_vents; v++) {
        let lo = geom[2u * (P.n_shapes + v)].xyz;
        let hi = geom[2u * (P.n_shapes + v) + 1u].xyz;
        if all(p >= lo) && all(p <= hi) {
            let extent = hi - lo;
            var axis = 0u;
            if extent.y < extent[axis] {
                axis = 1u;
            }
            if extent.z < extent[axis] {
                axis = 2u;
            }
            bits |= OPEN_X << axis;
        }
    }
    return bits;
}

/// A burner cell's output relative to steady, varying by up to ±`flicker` from cell to
/// cell and every 50 ms: it seeds the buoyant instabilities that make flames flicker.
/// A hash, so runs are reproducible.
fn flicker(cell: u32) -> f32 {
    var x = cell * 0x9E3779B9u ^ u32(P.time * 20.0) * 0x85EBCA6Bu;
    x ^= x >> 16u;
    x *= 0x7FEB352Du;
    x ^= x >> 15u;
    x *= 0x846CA68Bu;
    x ^= x >> 16u;
    let unit = f32(x >> 8u) / 16777216.0;
    return 1.0 + P.flicker * (2.0 * unit - 1.0);
}

/// Cell types from the scene's shapes, and the burners' gas in their cells.
@compute @workgroup_size(8, 8, 4)
fn classify(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = vec3<i32>(id);
    if !inside(c) {
        return;
    }
    let i = cell_index(c);
    let p = centre(c);
    var t = FLUID;
    if in_solid(p) {
        t = SOLID;
    } else {
        for (var b = 0u; b < P.n_burners; b++) {
            if in_burner(p, b) {
                t = INFLOW | (b << 8u);
                break;
            }
        }
        if t == FLUID {
            t |= vent_open_bits(p);
        }
    }
    flags[i] = t;
    let kind = t & 0x0fu;
    if kind == INFLOW {
        scal[i] = vec4<f32>(P.burner_smoke, P.burner_temp, P.burner_fuel * flicker(i), 0.0);
    } else if kind == SOLID {
        scal[i] = vec4<f32>(0.0);
    }
    if kind != FLUID {
        pd[i].x = 0.0;
    }
}

/// Prescribed faces get their velocity: zero at walls and solids, the exit velocity
/// at burners.
@compute @workgroup_size(8, 8, 4)
fn boundaries(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = vec3<i32>(id);
    for (var axis = 0u; axis < 3u; axis++) {
        if all(c < comp_dims(axis)) {
            let fix = fixed_face(axis, c);
            if fix.x > 0.0 {
                vel[comp_index(axis, c)] = fix.y;
            }
        }
    }
}

/// Boussinesq buoyancy: heat rises against gravity, smoke is a little heavy.
@compute @workgroup_size(8, 8, 4)
fn forces(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = vec3<i32>(id);
    for (var axis = 0u; axis < 3u; axis++) {
        if all(c < comp_dims(axis)) && fixed_face(axis, c).x == 0.0 {
            // (One side may be outside, at a vent's open wall: take the inside cell.)
            let edge = dims() - 1;
            let s = 0.5 * (scal[cell_index(clamp(c, vec3<i32>(0), edge))] + scal[cell_index(clamp(c - axis_unit(axis), vec3<i32>(0), edge))]);
            let a = P.gravity[axis] * (P.heaviness * s.x - P.rise * s.y);
            let i = comp_index(axis, c);
            vel[i] = (vel[i] + P.dt * a) * exp(-P.drag * P.dt);
        }
    }
}

/// Cell-centred velocity, from the faces around the cell (clamped to the domain).
fn cell_velocity(c0: vec3<i32>) -> vec3<f32> {
    let c = clamp(c0, vec3<i32>(0), dims() - 1);
    var u: vec3<f32>;
    for (var axis = 0u; axis < 3u; axis++) {
        u[axis] = 0.5 * (load_comp(axis, c) + load_comp(axis, c + axis_unit(axis)));
    }
    return u;
}

/// Vorticity confinement, first half: the curl at each cell, into `vel_tmp` (free until
/// the velocity is advected), three floats per cell.
@compute @workgroup_size(8, 8, 4)
fn curl(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = vec3<i32>(id);
    if !inside(c) {
        return;
    }
    let dx = cell_velocity(c + vec3<i32>(1, 0, 0)) - cell_velocity(c - vec3<i32>(1, 0, 0));
    let dy = cell_velocity(c + vec3<i32>(0, 1, 0)) - cell_velocity(c - vec3<i32>(0, 1, 0));
    let dz = cell_velocity(c + vec3<i32>(0, 0, 1)) - cell_velocity(c - vec3<i32>(0, 0, 1));
    let w = vec3<f32>(dy.z - dz.y, dz.x - dx.z, dx.y - dy.x) / (2.0 * P.h);
    let i = 3u * cell_index(c);
    vel_tmp[i] = w.x;
    vel_tmp[i + 1u] = w.y;
    vel_tmp[i + 2u] = w.z;
}

fn curl_at(c0: vec3<i32>) -> vec3<f32> {
    let i = 3u * cell_index(clamp(c0, vec3<i32>(0), dims() - 1));
    return vec3<f32>(vel_tmp[i], vel_tmp[i + 1u], vel_tmp[i + 2u]);
}

/// The confinement force at a cell: ε·h·(N × ω), N pointing up the gradient of |ω|.
/// It spins up the small swirls that advection smears out.
fn confinement(c: vec3<i32>) -> vec3<f32> {
    let grad = vec3<f32>(
        length(curl_at(c + vec3<i32>(1, 0, 0))) - length(curl_at(c - vec3<i32>(1, 0, 0))),
        length(curl_at(c + vec3<i32>(0, 1, 0))) - length(curl_at(c - vec3<i32>(0, 1, 0))),
        length(curl_at(c + vec3<i32>(0, 0, 1))) - length(curl_at(c - vec3<i32>(0, 0, 1))),
    );
    let len = length(grad);
    if len < 1e-6 {
        return vec3<f32>(0.0);
    }
    return P.vorticity * P.h * cross(grad / len, curl_at(c));
}

/// Vorticity confinement, second half: the force, averaged onto the free faces.
@compute @workgroup_size(8, 8, 4)
fn confine(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = vec3<i32>(id);
    for (var axis = 0u; axis < 3u; axis++) {
        if all(c < comp_dims(axis)) && fixed_face(axis, c).x == 0.0 {
            let f = 0.5 * (confinement(c)[axis] + confinement(c - axis_unit(axis))[axis]);
            vel[comp_index(axis, c)] += P.dt * f;
        }
    }
}

fn brush_base(b: u32) -> u32 {
    return 2u * (P.n_shapes + P.n_vents + P.n_burners) + 3u * b;
}

/// Brushes (the mouse, music reactions): inside each sphere the velocity is pulled
/// toward the brush's push plus its spin (spin × offset), fading to the rim, at
/// `strength` per second, as the 2D brushes do.
@compute @workgroup_size(8, 8, 4)
fn brushes(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = vec3<i32>(id);
    for (var axis = 0u; axis < 3u; axis++) {
        if !all(c < comp_dims(axis)) || fixed_face(axis, c).x != 0.0 {
            continue;
        }
        let p = (vec3<f32>(c) + comp_origin(axis)) * P.h;
        let i = comp_index(axis, c);
        var u = vel[i];
        for (var b = 0u; b < P.n_brushes; b++) {
            let a = geom[brush_base(b)];
            let d = distance(p, a.xyz);
            if d < a.w {
                let push = geom[brush_base(b) + 1u];
                let spin = geom[brush_base(b) + 2u].xyz;
                let goal = push.xyz + cross(spin, p - a.xyz);
                let blend = min(push.w * (1.0 - d / a.w) * P.dt, 1.0);
                u += (goal[axis] - u) * blend;
            }
        }
        vel[i] = u;
    }
}

/// Semi-Lagrangian advection of the velocity into `vel_tmp`. A face in an open wall
/// copies the face just inside instead (zero gradient across the wall): where air flows
/// in, a backtrace would leave the box and bring the face's own value back, so nothing
/// would ever restore it, and on a fine grid with an unconverged pressure it crept up
/// step by step until the velocity blew up (a campfire died after 30 s at 4 ms steps).
@compute @workgroup_size(8, 8, 4)
fn advect_velocity(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = vec3<i32>(id);
    for (var axis = 0u; axis < 3u; axis++) {
        if all(c < comp_dims(axis)) {
            let fix = fixed_face(axis, c);
            var value = fix.y;
            let below = c - axis_unit(axis);
            if fix.x == 0.0 && !inside(below) {
                value = load_comp(axis, c + axis_unit(axis));
            } else if fix.x == 0.0 && !inside(c) {
                value = load_comp(axis, below);
            } else if fix.x == 0.0 {
                let p = (vec3<f32>(c) + comp_origin(axis)) * P.h;
                value = sample_comp(axis, backtrace(p));
            }
            vel_tmp[comp_index(axis, c)] = value;
        }
    }
}

@compute @workgroup_size(8, 8, 4)
fn divergence(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = vec3<i32>(id);
    if !inside(c) {
        return;
    }
    var div = 0.0;
    if cell_type(c) == FLUID {
        for (var axis = 0u; axis < 3u; axis++) {
            div += load_comp(axis, c + axis_unit(axis)) - load_comp(axis, c);
        }
        div /= P.h;
    }
    pd[cell_index(c)].y = div;
}

/// One colour of a red-black SOR sweep for ∇²φ = div: fluid neighbours couple, the
/// outside beyond an open wall holds φ = 0, walls, solids and burners are Neumann.
@compute @workgroup_size(8, 8, 4)
fn pressure(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = vec3<i32>(id);
    if !inside(c) || u32(c.x + c.y + c.z) % 2u != PARITY {
        return;
    }
    let i = cell_index(c);
    let f = flags[i];
    if (f & 0x0fu) != FLUID {
        return;
    }
    var sum = 0.0;
    var count = 0.0;
    for (var axis = 0u; axis < 3u; axis++) {
        for (var side = -1; side <= 1; side += 2) {
            let nb = c + side * axis_unit(axis);
            if cell_type(nb) == FLUID {
                sum += pd[cell_index(nb)].x;
                count += 1.0;
            } else if !inside(nb) && (f & (OPEN_X << axis)) != 0u {
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

fn phi(c: vec3<i32>) -> f32 {
    if cell_type(c) == FLUID {
        return pd[cell_index(c)].x;
    }
    return 0.0;
}

/// Subtract the pressure gradient on the free faces.
@compute @workgroup_size(8, 8, 4)
fn project(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = vec3<i32>(id);
    for (var axis = 0u; axis < 3u; axis++) {
        if all(c < comp_dims(axis)) && fixed_face(axis, c).x == 0.0 {
            vel[comp_index(axis, c)] -= (phi(c) - phi(c - axis_unit(axis))) / P.h;
        }
    }
}

/// MacCormack, first half: the plain semi-Lagrangian result into `scal_hat`.
@compute @workgroup_size(8, 8, 4)
fn advect_forward(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = vec3<i32>(id);
    if !inside(c) {
        return;
    }
    let i = cell_index(c);
    if cell_type(c) == FLUID {
        scal_hat[i] = sample_scal(backtrace(centre(c))).value;
    } else {
        scal_hat[i] = scal[i];
    }
}

/// MacCormack, second half: correct by half the round-trip error, clamped to the cells
/// the value came from; then combustion (fuel burns where the gas is hot enough,
/// releasing heat and soot), cooling and fading.
@compute @workgroup_size(8, 8, 4)
fn advect_correct(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = vec3<i32>(id);
    if !inside(c) {
        return;
    }
    let i = cell_index(c);
    if cell_type(c) != FLUID {
        scal_tmp[i] = scal[i];
        return;
    }
    let p = centre(c);
    let origin = sample_scal(backtrace(p));
    let back = sample_hat(forwardtrace(p));
    var s = clamp(scal_hat[i] + 0.5 * (scal[i] - back), origin.lo, origin.hi);
    s = max(s, vec4<f32>(0.0));
    var smoke = s.x;
    var temp = s.y;
    var fuel = s.z;
    if temp >= P.ignition && fuel > 0.0 {
        let burned = fuel * (1.0 - exp(-P.burn_rate * P.dt));
        fuel -= burned;
        temp += P.heat * burned;
        smoke = min(smoke + P.soot * burned, 1.0);
    } else {
        fuel *= exp(-FUEL_DILUTION * P.dt);
    }
    // dT/dt = -cooling·T² - heat_loss·T, implicitly: stable at any step.
    temp /= 1.0 + P.cooling * temp * P.dt;
    temp *= exp(-P.heat_loss * P.dt);
    scal_tmp[i] = vec4<f32>(smoke * exp(-P.fade * P.dt), temp, fuel, 0.0);
}

/// Copy the scalars into the volume texture the renderer samples.
@compute @workgroup_size(8, 8, 4)
fn export_volume(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = vec3<i32>(id);
    if !inside(c) {
        return;
    }
    let s = scal[cell_index(c)];
    textureStore(volume, c, vec4<f32>(s.xy, select(0.0, 1.0, cell_type(c) == SOLID), s.z));
}
