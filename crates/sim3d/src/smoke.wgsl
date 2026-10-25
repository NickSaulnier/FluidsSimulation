// 3D smoke on a MAC grid (see smoke.rs for the algorithm and the pass order).
//
// Velocity: one buffer holding u ((nx+1)·ny·nz), then v (nx·(ny+1)·nz), then w
// (nx·ny·(nz+1)). Scalars: (smoke, temperature) per cell. pd: (pressure, divergence)
// per cell; the pressure is φ = p·dt/ρ, so the projection is u -= ∇φ.

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
    _pad: u32,
}

@group(0) @binding(0) var<uniform> P: Params;
@group(0) @binding(1) var<storage, read_write> vel: array<f32>;
@group(0) @binding(2) var<storage, read_write> vel_tmp: array<f32>;
@group(0) @binding(3) var<storage, read_write> scal: array<vec2<f32>>;
@group(0) @binding(4) var<storage, read_write> scal_hat: array<vec2<f32>>;
@group(0) @binding(5) var<storage, read_write> scal_tmp: array<vec2<f32>>;
@group(0) @binding(6) var<storage, read_write> pd: array<vec2<f32>>;
@group(0) @binding(7) var<storage, read_write> flags: array<u32>;
// Shapes (2 each: a sphere is (centre, radius), a box (min, -1) then (max, 0)), then
// vents (min, max), then burners ((pos, radius), (dir, speed)).
@group(0) @binding(8) var<storage, read> geom: array<vec4<f32>>;
// (smoke, temperature, solid) for drawing: a filterable copy of `scal`, with solid cells
// marked so the renderer can shadow with them.
@group(0) @binding(9) var volume: texture_storage_3d<rgba16float, write>;

// Red-black Gauss-Seidel colour, set per pipeline.
override PARITY: u32 = 0u;

const FLUID: u32 = 0u;
const SOLID: u32 = 1u;
const INFLOW: u32 = 3u;
/// A fluid cell inside a vent: where it touches the domain's wall, the wall is open.
const OPEN: u32 = 0x10u;

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

fn is_open(c: vec3<i32>) -> bool {
    return inside(c) && (flags[cell_index(c)] & OPEN) != 0u;
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
    if (!inside(c0) && is_open(c1)) || (!inside(c1) && is_open(c0)) {
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
    value: vec2<f32>,
    lo: vec2<f32>,
    hi: vec2<f32>,
}

/// Trilinear sample of the scalars, with the range of the eight cells it blends.
fn sample_scal(p: vec3<f32>) -> Sampled {
    let n = dims();
    let g = clamp(p / P.h - 0.5, vec3<f32>(0.0), vec3<f32>(n - 1));
    let c = min(vec3<i32>(floor(g)), max(n - 2, vec3<i32>(0)));
    let f = g - vec3<f32>(c);
    var v: array<vec2<f32>, 8>;
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

fn sample_hat(p: vec3<f32>) -> vec2<f32> {
    let n = dims();
    let g = clamp(p / P.h - 0.5, vec3<f32>(0.0), vec3<f32>(n - 1));
    let c = min(vec3<i32>(floor(g)), max(n - 2, vec3<i32>(0)));
    let f = g - vec3<f32>(c);
    var v: array<vec2<f32>, 8>;
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

fn in_vent(p: vec3<f32>) -> bool {
    for (var v = 0u; v < P.n_vents; v++) {
        let lo = geom[2u * (P.n_shapes + v)];
        let hi = geom[2u * (P.n_shapes + v) + 1u];
        if all(p >= lo.xyz) && all(p <= hi.xyz) {
            return true;
        }
    }
    return false;
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
        if t == FLUID && in_vent(p) {
            t = FLUID | OPEN;
        }
    }
    flags[i] = t;
    let kind = t & 0x0fu;
    if kind == INFLOW {
        scal[i] = vec2<f32>(1.0, P.burner_temp);
    } else if kind == SOLID {
        scal[i] = vec2<f32>(0.0);
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
            vel[comp_index(axis, c)] += P.dt * a;
        }
    }
}

/// Semi-Lagrangian advection of the velocity into `vel_tmp`.
@compute @workgroup_size(8, 8, 4)
fn advect_velocity(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = vec3<i32>(id);
    for (var axis = 0u; axis < 3u; axis++) {
        if all(c < comp_dims(axis)) {
            let fix = fixed_face(axis, c);
            var value = fix.y;
            if fix.x == 0.0 {
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
    let open = (f & OPEN) != 0u;
    var sum = 0.0;
    var count = 0.0;
    for (var axis = 0u; axis < 3u; axis++) {
        for (var side = -1; side <= 1; side += 2) {
            let nb = c + side * axis_unit(axis);
            if cell_type(nb) == FLUID {
                sum += pd[cell_index(nb)].x;
                count += 1.0;
            } else if open && !inside(nb) {
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
/// the value came from; then cooling and fading.
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
    s = max(s, vec2<f32>(0.0));
    let temp = s.y / (1.0 + P.cooling * s.y * P.dt);
    scal_tmp[i] = vec2<f32>(s.x * exp(-P.fade * P.dt), temp);
}

/// Copy the scalars into the volume texture the renderer samples.
@compute @workgroup_size(8, 8, 4)
fn export_volume(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = vec3<i32>(id);
    if !inside(c) {
        return;
    }
    let s = scal[cell_index(c)];
    textureStore(volume, c, vec4<f32>(s, select(0.0, 1.0, cell_type(c) == SOLID), 0.0));
}
