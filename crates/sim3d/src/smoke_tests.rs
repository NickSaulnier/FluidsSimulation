//! GPU tests for the 3D smoke solver. Each skips (with a note) without a GPU adapter.

use super::*;
use crate::scene::{self, Aabb};
use glam::IVec3;

fn gpu() -> Option<GpuContext> {
    match GpuContext::new() {
        Ok(g) => Some(g),
        Err(e) => {
            eprintln!("skipping: {e:#}");
            None
        }
    }
}

fn solver(gpu: &GpuContext, scene: &Scene3d, params: SmokeParams) -> Smoke3d {
    let mut s = Smoke3d::new(gpu, "test", params);
    s.reset(gpu, scene);
    s
}

fn at(cells_y: u32) -> SmokeParams {
    SmokeParams { cells_y, ..SmokeParams::default() }
}

fn run(s: &mut Smoke3d, gpu: &GpuContext, scene: &Scene3d, seconds: f32) {
    for _ in 0..(seconds * 60.0).round() as usize {
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        s.step(gpu, &mut encoder, 1.0 / 60.0, scene, &StepInput3d::default());
        gpu.queue.submit([encoder.finish()]);
        s.submitted(gpu);
    }
}

/// Uniform random numbers in [-1, 1) (xorshift).
fn noise(len: usize, mut seed: u32) -> Vec<f32> {
    (0..len)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as f32 / u32::MAX as f32 * 2.0 - 1.0
        })
        .collect()
}

fn index(n: UVec3, i: u32, j: u32, k: u32) -> usize {
    (i + n.x * (j + n.y * k)) as usize
}

fn centre(h: f32, i: u32, j: u32, k: u32) -> Vec3 {
    (Vec3::new(i as f32, j as f32, k as f32) + 0.5) * h
}

/// A box with a vent along the top and no burners.
fn vented_box() -> Scene3d {
    let mut s = Scene3d::empty_box(Vec3::new(1.0, 1.5, 1.0));
    s.vents.push(Aabb::new(Vec3::new(0.0, 1.44, 0.0), Vec3::new(1.0, 1.5, 1.0)));
    s
}

/// A hot, smoky ball low in the domain.
fn blob(s: &Smoke3d, gpu: &GpuContext) {
    let (n, h) = s.dims().unwrap();
    let mut scal = vec![[0.0f32; 4]; n.element_product() as usize];
    for k in 0..n.z {
        for j in 0..n.y {
            for i in 0..n.x {
                if centre(h, i, j, k).distance(Vec3::new(0.5, 0.35, 0.5)) < 0.15 {
                    scal[index(n, i, j, k)] = [1.0, 1.0, 0.0, 0.0];
                }
            }
        }
    }
    s.write_scalars(gpu, &scal);
}

/// Total smoke (in cell units) and its centroid.
fn smoke_mass_and_centroid(s: &Smoke3d, gpu: &GpuContext) -> (f32, Vec3) {
    let (n, h) = s.dims().unwrap();
    let scal = s.read_scalars(gpu);
    let (mut mass, mut moment) = (0.0, Vec3::ZERO);
    for k in 0..n.z {
        for j in 0..n.y {
            for i in 0..n.x {
                let m = scal[index(n, i, j, k)][0];
                mass += m;
                moment += m * centre(h, i, j, k);
            }
        }
    }
    (mass, moment / mass.max(1e-9))
}

fn blob_drift(gravity: Vec3) -> Option<Vec3> {
    let gpu = gpu()?;
    let mut scene = vented_box();
    scene.gravity = gravity;
    let mut s = solver(&gpu, &scene, at(32));
    blob(&s, &gpu);
    let (_, before) = smoke_mass_and_centroid(&s, &gpu);
    run(&mut s, &gpu, &scene, 1.0);
    let (_, after) = smoke_mass_and_centroid(&s, &gpu);
    Some(after - before)
}

#[test]
fn a_hot_blob_rises() {
    let Some(d) = blob_drift(Vec3::new(0.0, -9.81, 0.0)) else { return };
    assert!(d.y > 0.1, "rose only {d}");
    assert!(d.x.abs() < 0.01 && d.z.abs() < 0.01, "drifted sideways: {d}");
}

#[test]
fn without_gravity_a_hot_blob_stays() {
    let Some(d) = blob_drift(Vec3::ZERO) else { return };
    assert!(d.length() < 0.01, "moved {d}");
}

#[test]
fn under_tilted_gravity_a_hot_blob_leans() {
    let tilt = 20f32.to_radians();
    let Some(d) = blob_drift(9.81 * Vec3::new(tilt.sin(), -tilt.cos(), 0.0)) else { return };
    assert!(d.y > 0.1, "rose only {d}");
    assert!(d.x < -0.03, "didn't lean away from gravity: {d}");
}

/// MacCormack's limiter isn't exactly conservative, but close. (Without confinement:
/// this checks the advection, and the finer swirls confinement adds cost a little more.)
#[test]
fn smoke_is_conserved_in_a_closed_box() {
    let Some(gpu) = gpu() else { return };
    let scene = Scene3d::empty_box(Vec3::new(1.0, 1.5, 1.0));
    let mut s = solver(&gpu, &scene, SmokeParams { vorticity: 0.0, ..at(32) });
    blob(&s, &gpu);
    let (before, _) = smoke_mass_and_centroid(&s, &gpu);
    run(&mut s, &gpu, &scene, 1.0);
    let (after, _) = smoke_mass_and_centroid(&s, &gpu);
    assert!((after / before - 1.0).abs() < 0.03, "smoke went from {before} to {after}");
}

/// Vorticity confinement feeds the swirls: a rising blob ends up with more of them.
#[test]
fn confinement_adds_swirl() {
    let Some(gpu) = gpu() else { return };
    let scene = vented_box();
    let swirl = |vorticity| {
        let mut s = solver(&gpu, &scene, SmokeParams { vorticity, ..at(32) });
        blob(&s, &gpu);
        run(&mut s, &gpu, &scene, 1.0);
        s.read_velocity(&gpu).iter().map(|v| v * v).sum::<f32>()
    };
    let (plain, confined) = (swirl(0.0), swirl(1.0));
    assert!(confined > 1.05 * plain, "kinetic energy {plain} without confinement, {confined} with");
}

/// A fire burner keeps a flame going: hot gas above the visible threshold, soot from
/// the burning, and the fuel mostly used up within a metre (the burner's is 1).
#[test]
fn a_fire_burner_keeps_burning() {
    let Some(gpu) = gpu() else { return };
    let scene = scene::campfire();
    let mut s = solver(&gpu, &scene, SmokeParams { cells_y: 48, ..SmokeParams::fire() });
    run(&mut s, &gpu, &scene, 3.0);
    let scal = s.read_scalars(&gpu);
    let hottest = scal.iter().map(|c| c[1]).fold(0.0, f32::max);
    let soot: f32 = scal.iter().map(|c| c[0]).sum();
    let (n, h) = s.dims().unwrap();
    // Fuel left high up: mostly burned before it gets there.
    let high_fuel = (0..n.z).flat_map(|k| (0..n.x).map(move |i| (i, k))).map(|(i, k)| scal[index(n, i, (1.0 / h) as u32, k)][2]).fold(0.0, f32::max);
    assert!(hottest > 2.0, "hottest gas {hottest}: no visible flame");
    assert!(soot > 1.0, "no soot ({soot})");
    assert!(high_fuel < 0.15, "fuel reached 1 m up unburned ({high_fuel})");
}

/// With no fuel nothing burns: the gas never gets hotter than the burner, and makes
/// no soot.
#[test]
fn without_fuel_nothing_burns() {
    let Some(gpu) = gpu() else { return };
    let scene = scene::campfire();
    let params = SmokeParams { cells_y: 48, burner_fuel: 0.0, ..SmokeParams::fire() };
    let burner = params.burner_temp;
    let mut s = solver(&gpu, &scene, params);
    run(&mut s, &gpu, &scene, 2.0);
    let scal = s.read_scalars(&gpu);
    let hottest = scal.iter().map(|c| c[1]).fold(0.0, f32::max);
    let smoke: f32 = scal.iter().map(|c| c[0]).sum();
    assert!(hottest <= burner + 1e-4, "gas reached {hottest}, hotter than the burner's {burner}");
    assert_eq!(smoke, 0.0, "smoke without burning");
}

/// Random velocities, walls, a vent, a solid ball and burners: after the projection
/// almost nothing of the divergence is left.
#[test]
fn projection_leaves_no_divergence() {
    let Some(gpu) = gpu() else { return };
    let scene = scene::twin_burners();
    let mut s = solver(&gpu, &scene, SmokeParams { pressure_iters: 1500, ..at(24) });
    s.write_velocity(&gpu, &noise(s.read_velocity(&gpu).len(), 7));
    s.prepare(&gpu, &scene, &StepInput3d::default());
    let largest = |d: Vec<f32>| d.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    let before = largest(s.measure_divergence(&gpu));
    s.project(&gpu);
    let after = largest(s.measure_divergence(&gpu));
    assert!(before > 1.0, "the test velocities aren't divergent: {before}");
    assert!(after < 1e-3 * before, "divergence {before} -> {after}");
}

/// The shader's classification, stencils and red-black order, checked value by value
/// against the same algorithm on the CPU.
#[test]
fn matches_a_cpu_reference() {
    let Some(gpu) = gpu() else { return };
    let scene = scene::chimney();
    let params = SmokeParams { pressure_iters: 10, ..at(24) };
    let mut s = solver(&gpu, &scene, params.clone());
    let (n, h) = s.dims().unwrap();
    s.write_velocity(&gpu, &noise(s.read_velocity(&gpu).len(), 11));
    s.prepare(&gpu, &scene, &StepInput3d::default());
    let flags = s.read_flags(&gpu);
    let mut vel = s.read_velocity(&gpu);

    let reference = cpu::Grid::new(n, h, &scene);
    assert_eq!(flags, reference.flags, "cell types differ");
    for (axis, face) in reference.faces() {
        if let Some(v) = reference.fixed(axis, face) {
            assert_eq!(vel[reference.comp_index(axis, face)], v, "prescribed face {axis} {face}");
        }
    }
    let phi = reference.project(&mut vel, params.pressure_iters, params.over_relax);

    s.project(&gpu);
    let gpu_phi: Vec<f32> = s.read_pressure_divergence(&gpu).iter().map(|pd| pd[0]).collect();
    let gpu_vel = s.read_velocity(&gpu);
    let close = |a: &[f32], b: &[f32], what: &str| {
        let scale = a.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let worst = a.iter().zip(b).fold(0.0f32, |m, (x, y)| m.max((x - y).abs()));
        assert!(worst <= 1e-4 * scale + 1e-7, "{what}: differs by {worst} (scale {scale})");
    };
    close(&phi, &gpu_phi, "pressure");
    close(&vel, &gpu_vel, "velocity");
}

/// A centred burner's plume is symmetric: exactly so under swapping x and z (which
/// the red-black colouring respects), and closely under mirroring x.
#[test]
fn a_centred_plume_is_symmetric() {
    let Some(gpu) = gpu() else { return };
    let scene = scene::campfire();
    let mut s = solver(&gpu, &scene, at(48));
    run(&mut s, &gpu, &scene, 1.5);
    let (n, _) = s.dims().unwrap();
    assert_eq!(n.x, n.z);
    let scal = s.read_scalars(&gpu);
    let smoke = |i, j, k| scal[index(n, i, j, k)][0];
    let (mut peak, mut swapped, mut mirrored) = (0.0f32, 0.0f32, 0.0f32);
    for k in 0..n.z {
        for j in 0..n.y {
            for i in 0..n.x {
                peak = peak.max(smoke(i, j, k));
                swapped = swapped.max((smoke(i, j, k) - smoke(k, j, i)).abs());
                mirrored = mirrored.max((smoke(i, j, k) - smoke(n.x - 1 - i, j, k)).abs());
            }
        }
    }
    let top: f32 = (0..n.z).flat_map(|k| (0..n.x).map(move |i| (i, k))).map(|(i, k)| smoke(i, n.y * 2 / 3, k)).sum();
    assert!(top > 0.0, "the plume didn't reach two thirds of the way up");
    assert!(swapped <= 0.01 * peak, "x/z swap differs by {swapped} (peak {peak})");
    assert!(mirrored <= 0.05 * peak, "x mirror differs by {mirrored} (peak {peak})");
}

/// GPU time per pass at the default resolution (run on demand with --nocapture).
#[test]
#[ignore = "timing: run on demand"]
fn timings_at_the_default_resolution() {
    let Some(gpu) = gpu() else { return };
    let scene = scene::campfire();
    let mut s = solver(&gpu, &scene, SmokeParams::default());
    let start = std::time::Instant::now();
    run(&mut s, &gpu, &scene, 3.0);
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let per_step = start.elapsed() / 180;
    run(&mut s, &gpu, &scene, 0.05);
    let (n, _) = s.dims().unwrap();
    let total: Duration = s.stats().phases.iter().map(|p| p.1).sum();
    eprintln!("{} at {n} ({} cells), wall {per_step:?} per step, GPU {total:?} per substep:", gpu.adapter, n.element_product());
    for (name, d) in &s.stats().phases {
        eprintln!("  {name}: {d:?}");
    }
}

/// The solver's algorithm on the CPU, for `matches_a_cpu_reference`.
mod cpu {
    use super::*;

    const FLUID: u32 = 0;
    const SOLID: u32 = 1;
    const INFLOW: u32 = 3;
    const OPEN: u32 = 0x10;

    pub struct Grid {
        n: UVec3,
        h: f32,
        pub flags: Vec<u32>,
        burner_vel: Vec<Vec3>,
    }

    impl Grid {
        /// The shader's classification rules: spheres are open sets, boxes and vents closed,
        /// burners are discs at least a cell thick and a cell across.
        pub fn new(n: UVec3, h: f32, scene: &Scene3d) -> Self {
            let mut flags = vec![FLUID; n.element_product() as usize];
            for k in 0..n.z {
                for j in 0..n.y {
                    for i in 0..n.x {
                        let p = centre(h, i, j, k);
                        let solid = scene.solids.iter().any(|s| match *s {
                            Shape::Sphere { centre, radius } => p.distance(centre) < radius,
                            Shape::Cuboid(b) => p.cmpge(b.min).all() && p.cmple(b.max).all(),
                        });
                        let burner = scene.burners.iter().position(|b| {
                            let r = p - b.pos;
                            let along = r.dot(b.dir);
                            along.abs() <= 0.75 * h && (r - along * b.dir).length() <= b.radius.max(0.75 * h)
                        });
                        let vent = scene.vents.iter().any(|v| p.cmpge(v.min).all() && p.cmple(v.max).all());
                        flags[index(n, i, j, k)] = match (solid, burner, vent) {
                            (true, _, _) => SOLID,
                            (false, Some(b), _) => INFLOW | ((b as u32) << 8),
                            (false, None, true) => FLUID | OPEN,
                            _ => FLUID,
                        };
                    }
                }
            }
            let burner_vel = scene.burners.iter().map(|b| b.dir * b.speed).collect();
            Self { n, h, flags, burner_vel }
        }

        fn kind(&self, c: IVec3) -> u32 {
            if c.cmplt(IVec3::ZERO).any() || c.cmpge(self.n.as_ivec3()).any() {
                return SOLID;
            }
            self.flags[index(self.n, c.x as u32, c.y as u32, c.z as u32)] & 0x0f
        }

        fn open(&self, c: IVec3) -> bool {
            self.inside(c) && self.flags[index(self.n, c.x as u32, c.y as u32, c.z as u32)] & OPEN != 0
        }

        fn comp_dims(&self, axis: usize) -> UVec3 {
            self.n + UVec3::AXES[axis]
        }

        pub fn comp_index(&self, axis: usize, c: UVec3) -> usize {
            let offset: u32 = (0..axis).map(|a| self.comp_dims(a).element_product()).sum();
            let d = self.comp_dims(axis);
            (offset + c.x + d.x * (c.y + d.y * c.z)) as usize
        }

        pub fn faces(&self) -> impl Iterator<Item = (usize, UVec3)> + '_ {
            (0..3).flat_map(move |axis| {
                let d = self.comp_dims(axis);
                (0..d.z).flat_map(move |k| (0..d.y).flat_map(move |j| (0..d.x).map(move |i| (axis, UVec3::new(i, j, k)))))
            })
        }

        fn inside(&self, c: IVec3) -> bool {
            c.cmpge(IVec3::ZERO).all() && c.cmplt(self.n.as_ivec3()).all()
        }

        /// The prescribed velocity of a face, if it has one. Walls are open at vents.
        pub fn fixed(&self, axis: usize, face: UVec3) -> Option<f32> {
            let c1 = face.as_ivec3();
            let c0 = c1 - IVec3::AXES[axis];
            let (t0, t1) = (self.kind(c0), self.kind(c1));
            if (!self.inside(c0) && self.open(c1)) || (!self.inside(c1) && self.open(c0)) {
                return None;
            }
            if t0 == SOLID || t1 == SOLID {
                return Some(0.0);
            }
            for (t, c) in [(t0, c0), (t1, c1)] {
                if t == INFLOW {
                    let b = self.flags[index(self.n, c.x as u32, c.y as u32, c.z as u32)] >> 8;
                    return Some(self.burner_vel[b as usize][axis]);
                }
            }
            None
        }

        /// Divergence, `iterations` red-black SOR sweeps from φ = 0, and the gradient
        /// subtracted from `vel`. Returns φ.
        pub fn project(&self, vel: &mut [f32], iterations: u32, omega: f32) -> Vec<f32> {
            let (n, h) = (self.n, self.h);
            let cells = n.element_product() as usize;
            let mut div = vec![0.0f32; cells];
            let mut phi = vec![0.0f32; cells];
            let each_cell = || (0..n.z).flat_map(move |k| (0..n.y).flat_map(move |j| (0..n.x).map(move |i| UVec3::new(i, j, k))));
            for c in each_cell() {
                if self.kind(c.as_ivec3()) == FLUID {
                    let d: f32 = (0..3).map(|a| vel[self.comp_index(a, c + UVec3::AXES[a])] - vel[self.comp_index(a, c)]).sum();
                    div[index(n, c.x, c.y, c.z)] = d / h;
                }
            }
            for _ in 0..iterations {
                for parity in 0..2 {
                    for c in each_cell() {
                        if (c.x + c.y + c.z) % 2 != parity || self.kind(c.as_ivec3()) != FLUID {
                            continue;
                        }
                        let (mut sum, mut count) = (0.0f32, 0.0f32);
                        for a in 0..3 {
                            for side in [-1, 1] {
                                let nb = c.as_ivec3() + side * IVec3::AXES[a];
                                if self.kind(nb) == FLUID {
                                    sum += phi[index(n, nb.x as u32, nb.y as u32, nb.z as u32)];
                                    count += 1.0;
                                } else if self.open(c.as_ivec3()) && !self.inside(nb) {
                                    count += 1.0;
                                }
                            }
                        }
                        if count > 0.0 {
                            let i = index(n, c.x, c.y, c.z);
                            let gs = (sum - h * h * div[i]) / count;
                            phi[i] += (gs - phi[i]) * omega;
                        }
                    }
                }
            }
            let phi_at = |c: IVec3| {
                if self.kind(c) == FLUID { phi[index(n, c.x as u32, c.y as u32, c.z as u32)] } else { 0.0 }
            };
            let faces: Vec<_> = self.faces().collect();
            for (axis, face) in faces {
                if self.fixed(axis, face).is_none() {
                    let c = face.as_ivec3();
                    vel[self.comp_index(axis, face)] -= (phi_at(c) - phi_at(c - IVec3::AXES[axis])) / h;
                }
            }
            phi
        }
    }
}
