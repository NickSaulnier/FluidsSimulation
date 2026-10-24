// Position Based Fluids on the GPU: the same algorithm, kernels and constants as the CPU
// solver in sim::solvers::pbf, one thread per particle (or per cell for `clear`).
//
// Neighbours come from fixed-capacity cell lists built with atomic counters, so no GPU
// prefix sum is needed. Eight storage buffers: WebGPU's default per-stage limit, so this
// runs on any adapter (λ and density share one vec2 array to stay within it).

struct Params {
    n: u32,
    grid_nx: u32,
    grid_ny: u32,
    max_per_cell: u32,
    dt: f32,
    inv_dt: f32,
    h: f32,               // kernel radius
    cell: f32,            // neighbour-grid cell size (kernel radius plus a margin)
    gravity: vec2f,
    interior_min: vec2f,
    interior_max: vec2f,
    radius: f32,          // particle radius, for collisions
    rho0: f32,
    poly6: f32,
    spiky_grad: f32,
    relaxation: f32,      // ε, already scaled by the lattice stiffness
    tensile_k: f32,       // s_corr k, already in λ units
    w_dq: f32,
    viscosity: f32,
    sdf_nx: u32,
    sdf_ny: u32,
    sdf_cell: f32,
    _pad: f32,
};

@group(0) @binding(0) var<uniform> P: Params;
@group(0) @binding(1) var<storage, read_write> pos: array<vec2f>;
@group(0) @binding(2) var<storage, read_write> vel: array<vec2f>;
@group(0) @binding(3) var<storage, read_write> pred: array<vec2f>;
@group(0) @binding(4) var<storage, read_write> delta: array<vec2f>;
@group(0) @binding(5) var<storage, read_write> lambda_rho: array<vec2f>;   // (λ, density)
@group(0) @binding(6) var<storage, read_write> cell_count: array<atomic<u32>>;
@group(0) @binding(7) var<storage, read_write> cell_slots: array<u32>;
@group(0) @binding(8) var<storage, read> sdf: array<f32>;

const WG: u32 = 256u;

// ---- helpers ------------------------------------------------------------------------

fn poly6(r2: f32) -> f32 {
    let h2 = P.h * P.h;
    if r2 >= h2 {
        return 0.0;
    }
    let d = h2 - r2;
    return P.poly6 * d * d * d;
}

// Gradient wrt particle i of the spiky kernel at d = x_i - x_j.
fn spiky_grad(d: vec2f) -> vec2f {
    let r = length(d);
    if r >= P.h || r < 1e-9 * P.h {
        return vec2f(0.0);
    }
    let f = P.h - r;
    return d * (P.spiky_grad * f * f / r);
}

fn cell_of(p: vec2f) -> vec2i {
    let g = vec2i(floor(p / P.cell));
    return clamp(g, vec2i(0), vec2i(i32(P.grid_nx) - 1, i32(P.grid_ny) - 1));
}

fn sdf_at(i: i32, j: i32) -> f32 {
    return sdf[u32(j) * P.sdf_nx + u32(i)];
}

// Bilinear signed distance (as SolidMask::distance).
fn distance(p: vec2f) -> f32 {
    let n = vec2f(f32(P.sdf_nx), f32(P.sdf_ny));
    let g = clamp(p / P.sdf_cell - 0.5, vec2f(0.0), n - 1.0);
    let i = min(i32(g.x), i32(P.sdf_nx) - 2);
    let j = min(i32(g.y), i32(P.sdf_ny) - 2);
    let f = g - vec2f(f32(i), f32(j));
    let bottom = mix(sdf_at(i, j), sdf_at(i + 1, j), f.x);
    let top = mix(sdf_at(i, j + 1), sdf_at(i + 1, j + 1), f.x);
    return mix(bottom, top, f.y);
}

// Push out of solids along the SDF normal, then clamp into the domain interior
// (as sim::solvers::collide_with_solids; velocities are rebuilt from positions later).
fn collide(p_in: vec2f) -> vec2f {
    var p = p_in;
    let d = distance(p);
    if d < P.radius {
        let e = P.sdf_cell;
        let grad = vec2f(distance(p + vec2f(e, 0.0)) - distance(p - vec2f(e, 0.0)),
                         distance(p + vec2f(0.0, e)) - distance(p - vec2f(0.0, e)));
        let len = length(grad);
        if len > 0.0 {
            p += grad / len * (P.radius - d);
        }
    }
    return clamp(p, P.interior_min + P.radius, P.interior_max - P.radius);
}

// ---- passes -------------------------------------------------------------------------

@compute @workgroup_size(WG)
fn clear(@builtin(global_invocation_id) id: vec3u) {
    if id.x < P.grid_nx * P.grid_ny {
        atomicStore(&cell_count[id.x], 0u);
    }
}

@compute @workgroup_size(WG)
fn predict(@builtin(global_invocation_id) id: vec3u) {
    let i = id.x;
    if i >= P.n {
        return;
    }
    let v = vel[i] + P.gravity * P.dt;
    vel[i] = v;
    pred[i] = collide(pos[i] + v * P.dt);
}

@compute @workgroup_size(WG)
fn bin(@builtin(global_invocation_id) id: vec3u) {
    let i = id.x;
    if i >= P.n {
        return;
    }
    let c = cell_of(pred[i]);
    let cell = u32(c.y) * P.grid_nx + u32(c.x);
    let k = atomicAdd(&cell_count[cell], 1u);
    if k < P.max_per_cell {
        cell_slots[cell * P.max_per_cell + k] = i;
    }
}

// λ_i = -C_i / (Σ_j |∇_j C_i|² + ε), C_i = max(ρ_i/ρ₀ - 1, 0).
@compute @workgroup_size(WG)
fn lambda(@builtin(global_invocation_id) id: vec3u) {
    let i = id.x;
    if i >= P.n {
        return;
    }
    let pi = pred[i];
    let c = cell_of(pi);
    var rho = 0.0;
    var grad_i = vec2f(0.0);
    var sum_grad2 = 0.0;
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let cc = c + vec2i(dx, dy);
            if cc.x < 0 || cc.y < 0 || cc.x >= i32(P.grid_nx) || cc.y >= i32(P.grid_ny) {
                continue;
            }
            let cell = u32(cc.y) * P.grid_nx + u32(cc.x);
            let count = min(atomicLoad(&cell_count[cell]), P.max_per_cell);
            for (var k = 0u; k < count; k++) {
                let j = cell_slots[cell * P.max_per_cell + k];
                let d = pi - pred[j];
                rho += poly6(dot(d, d));
                if j != i {
                    let g = spiky_grad(d) / P.rho0;
                    grad_i += g;
                    sum_grad2 += dot(g, g);
                }
            }
        }
    }
    let constraint = max(rho / P.rho0 - 1.0, 0.0);
    lambda_rho[i] = vec2f(-constraint / (sum_grad2 + dot(grad_i, grad_i) + P.relaxation), rho);
}

// Δp_i = (1/ρ₀) Σ_j (λ_i + λ_j + s_corr) ∇W(p_i - p_j)
@compute @workgroup_size(WG)
fn displacement(@builtin(global_invocation_id) id: vec3u) {
    let i = id.x;
    if i >= P.n {
        return;
    }
    let pi = pred[i];
    let li = lambda_rho[i].x;
    let c = cell_of(pi);
    var dp = vec2f(0.0);
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let cc = c + vec2i(dx, dy);
            if cc.x < 0 || cc.y < 0 || cc.x >= i32(P.grid_nx) || cc.y >= i32(P.grid_ny) {
                continue;
            }
            let cell = u32(cc.y) * P.grid_nx + u32(cc.x);
            let count = min(atomicLoad(&cell_count[cell]), P.max_per_cell);
            for (var k = 0u; k < count; k++) {
                let j = cell_slots[cell * P.max_per_cell + k];
                if j == i {
                    continue;
                }
                let d = pi - pred[j];
                let r2 = dot(d, d);
                if r2 >= P.h * P.h {
                    continue;
                }
                let ratio = poly6(r2) / P.w_dq;
                let s_corr = -P.tensile_k * ratio * ratio * ratio * ratio;   // n = 4
                dp += (li + lambda_rho[j].x + s_corr) * spiky_grad(d);
            }
        }
    }
    delta[i] = dp / P.rho0;
}

@compute @workgroup_size(WG)
fn apply(@builtin(global_invocation_id) id: vec3u) {
    let i = id.x;
    if i < P.n {
        pred[i] = collide(pred[i] + delta[i]);
    }
}

@compute @workgroup_size(WG)
fn velocity(@builtin(global_invocation_id) id: vec3u) {
    let i = id.x;
    if i < P.n {
        vel[i] = (pred[i] - pos[i]) * P.inv_dt;
    }
}

// XSPH: blend toward neighbours' velocities, written to `delta` (scratch), then `commit`.
@compute @workgroup_size(WG)
fn xsph(@builtin(global_invocation_id) id: vec3u) {
    let i = id.x;
    if i >= P.n {
        return;
    }
    let pi = pred[i];
    let vi = vel[i];
    let c = cell_of(pi);
    var blend = vec2f(0.0);
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let cc = c + vec2i(dx, dy);
            if cc.x < 0 || cc.y < 0 || cc.x >= i32(P.grid_nx) || cc.y >= i32(P.grid_ny) {
                continue;
            }
            let cell = u32(cc.y) * P.grid_nx + u32(cc.x);
            let count = min(atomicLoad(&cell_count[cell]), P.max_per_cell);
            for (var k = 0u; k < count; k++) {
                let j = cell_slots[cell * P.max_per_cell + k];
                if j != i {
                    let d = pi - pred[j];
                    blend += (vel[j] - vi) * poly6(dot(d, d));
                }
            }
        }
    }
    delta[i] = vi + P.viscosity * blend / P.rho0;
}

@compute @workgroup_size(WG)
fn commit(@builtin(global_invocation_id) id: vec3u) {
    let i = id.x;
    if i < P.n {
        if P.viscosity > 0.0 {
            vel[i] = delta[i];
        }
        pos[i] = pred[i];
    }
}
