//! Position Based Fluids with the constraint solve in wgpu compute shaders.
//!
//! Same algorithm, parameters and derived constants as `sim::solvers::Pbf` (the
//! constants come from `PbfParams::constants`). Each substep: emitters, drains and
//! brushes on the CPU, exactly as the CPU solver does them; upload the particles; one
//! compute pass of dispatches (predict, bin, iterations x (λ, Δp, apply), velocity,
//! XSPH, commit); read positions, velocities and densities back. The readback keeps
//! `particles()` and everything built on it (renderer, bench, invariants) unchanged.

use crate::GpuContext;
use bytemuck::{Pod, Zeroable};
use sim::solvers::{Emission, PbfParams, apply_brushes, resample, seed_particles};
use sim::{Init, Metric, ParamVisitor, ParticleView, Scene, Solver, Stats, StepInput, Vec2};
use std::time::Instant;

/// Cell-list capacity. A cell of 1.1 kernel radii holds ~5 particles at rest; extra
/// particles beyond this are dropped from the lists (only under extreme compression).
const MAX_PER_CELL: u32 = 32;
/// Neighbour-grid cell size relative to the kernel radius: lists are built once per
/// substep, so leave room for the constraint iterations moving particles.
const CELL_MARGIN: f32 = 1.1;
const WORKGROUP: u32 = 256;

/// Must match `Params` in pbf.wgsl (112 bytes: uniform structs round up to 16).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    n: u32,
    grid_nx: u32,
    grid_ny: u32,
    max_per_cell: u32,
    dt: f32,
    inv_dt: f32,
    h: f32,
    cell: f32,
    gravity: [f32; 2],
    interior_min: [f32; 2],
    interior_max: [f32; 2],
    radius: f32,
    rho0: f32,
    poly6: f32,
    spiky_grad: f32,
    relaxation: f32,
    tensile_k: f32,
    w_dq: f32,
    viscosity: f32,
    sdf_nx: u32,
    sdf_ny: u32,
    sdf_cell: f32,
    _pad: [f32; 3],
}

struct Pipelines {
    clear: wgpu::ComputePipeline,
    predict: wgpu::ComputePipeline,
    bin: wgpu::ComputePipeline,
    lambda: wgpu::ComputePipeline,
    displacement: wgpu::ComputePipeline,
    apply: wgpu::ComputePipeline,
    velocity: wgpu::ComputePipeline,
    xsph: wgpu::ComputePipeline,
    commit: wgpu::ComputePipeline,
}

/// GPU buffers sized for `capacity` particles and a `cells`-cell neighbour grid.
struct Buffers {
    capacity: usize,
    cells: usize,
    sdf_len: usize,
    pos: wgpu::Buffer,
    vel: wgpu::Buffer,
    lambda_rho: wgpu::Buffer,
    sdf: wgpu::Buffer,
    /// Mappable copies of pos, vel and lambda_rho, back to back.
    staging: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

pub struct GpuPbf {
    pub params: PbfParams,
    gpu: GpuContext,
    layout: wgpu::BindGroupLayout,
    pipelines: Pipelines,
    uniform: wgpu::Buffer,
    buffers: Option<Buffers>,
    sdf_generation: Option<u64>,
    pos: Vec<Vec2>,
    vel: Vec<Vec2>,
    density: Vec<f32>,
    emission: Emission,
    stats: Stats,
}

/// Registry constructor: GPU PBF if an adapter is available, otherwise CPU PBF.
pub fn make() -> Box<dyn Solver> {
    match GpuContext::new() {
        Ok(gpu) => Box::new(GpuPbf::new(gpu, GpuPbf::default_params())),
        Err(e) => {
            log::warn!("GPU PBF unavailable ({e:#}); falling back to CPU PBF");
            Box::new(sim::solvers::Pbf::default())
        }
    }
}

impl GpuPbf {
    /// The CPU solver's defaults, but at FLIP's resolution: the GPU makes the extra
    /// constraint iterations that deep fluid needs cheap (the CPU solver has to run at
    /// half this resolution to stay stable and fast).
    pub fn default_params() -> PbfParams {
        PbfParams { spacing: 0.005, iterations: 20, ..PbfParams::default() }
    }

    pub fn new(gpu: GpuContext, params: PbfParams) -> Self {
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("pbf.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("pbf.wgsl").into()),
        });
        let storage = |binding, read_only| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let mut entries = vec![wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(size_of::<Params>() as u64),
            },
            count: None,
        }];
        entries.extend((1..=7).map(|b| storage(b, false)));
        entries.push(storage(8, true));
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("pbf"), entries: &entries });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pbf"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let pipelines = Pipelines {
            clear: pipeline("clear"),
            predict: pipeline("predict"),
            bin: pipeline("bin"),
            lambda: pipeline("lambda"),
            displacement: pipeline("displacement"),
            apply: pipeline("apply"),
            velocity: pipeline("velocity"),
            xsph: pipeline("xsph"),
            commit: pipeline("commit"),
        };
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pbf params"),
            size: size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            params,
            gpu,
            layout,
            pipelines,
            uniform,
            buffers: None,
            sdf_generation: None,
            pos: Vec::new(),
            vel: Vec::new(),
            density: Vec::new(),
            emission: Emission::default(),
            stats: Stats::default(),
        }
    }

    fn radius(&self) -> f32 {
        0.5 * self.params.spacing
    }

    fn grid_dims(&self, scene: &Scene) -> (u32, u32, f32) {
        let cell = CELL_MARGIN * self.params.constants().kernel_radius;
        ((scene.size.x / cell).ceil() as u32, (scene.size.y / cell).ceil() as u32, cell)
    }

    /// (Re)create the GPU buffers if the particle count, grid or SDF outgrew them.
    fn ensure_buffers(&mut self, scene: &Scene) {
        let n = self.pos.len().max(1);
        let (gx, gy, _) = self.grid_dims(scene);
        let cells = (gx * gy) as usize;
        let sdf_len = scene.solid.sdf_grid().3.len();
        if self.buffers.as_ref().is_some_and(|b| b.capacity >= n && b.cells == cells && b.sdf_len == sdf_len) {
            return;
        }
        let capacity = n.next_power_of_two().max(1024);
        let device = &self.gpu.device;
        let buffer = |label: &str, size: usize, usage: wgpu::BufferUsages| {
            device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: size as u64, usage, mapped_at_creation: false })
        };
        let rw = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC;
        let vec2_bytes = size_of::<Vec2>();
        let pos = buffer("pos", capacity * vec2_bytes, rw);
        let vel = buffer("vel", capacity * vec2_bytes, rw);
        let pred = buffer("pred", capacity * vec2_bytes, rw);
        let delta = buffer("delta", capacity * vec2_bytes, rw);
        let lambda_rho = buffer("lambda_rho", capacity * vec2_bytes, rw);
        let cell_count = buffer("cell_count", cells * 4, rw);
        let cell_slots = buffer("cell_slots", cells * MAX_PER_CELL as usize * 4, rw);
        let sdf = buffer("sdf", sdf_len * 4, rw);
        let staging = buffer(
            "pbf readback",
            3 * capacity * vec2_bytes,
            wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        );
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pbf"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: pos.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: vel.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: pred.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: delta.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: lambda_rho.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 6, resource: cell_count.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 7, resource: cell_slots.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 8, resource: sdf.as_entire_binding() },
            ],
        });
        self.buffers = Some(Buffers { capacity, cells, sdf_len, pos, vel, lambda_rho, sdf, staging, bind_group });
        self.sdf_generation = None;
    }

    fn write_params(&self, scene: &Scene, dt: f32) {
        let c = self.params.constants();
        let (grid_nx, grid_ny, cell) = self.grid_dims(scene);
        let interior = scene.solid.interior();
        let (sdf_nx, sdf_ny, sdf_cell, _) = scene.solid.sdf_grid();
        let params = Params {
            n: self.pos.len() as u32,
            grid_nx,
            grid_ny,
            max_per_cell: MAX_PER_CELL,
            dt,
            inv_dt: 1.0 / dt,
            h: c.kernel_radius,
            cell,
            gravity: scene.gravity.into(),
            interior_min: interior.min.into(),
            interior_max: interior.max.into(),
            radius: self.radius(),
            rho0: c.rest_density,
            poly6: c.poly6,
            spiky_grad: c.spiky_grad,
            relaxation: self.params.relaxation * c.lattice_stiffness,
            tensile_k: self.params.tensile_k / c.lattice_stiffness,
            w_dq: c.w_dq,
            viscosity: self.params.viscosity,
            sdf_nx: sdf_nx as u32,
            sdf_ny: sdf_ny as u32,
            sdf_cell,
            _pad: [0.0; 3],
        };
        self.gpu.queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&params));
    }

    /// Run one substep on the GPU and read the particles back.
    fn run_gpu(&mut self, scene: &Scene) {
        let n = self.pos.len();
        let b = self.buffers.as_ref().expect("ensure_buffers ran");
        let queue = &self.gpu.queue;
        if self.sdf_generation != Some(scene.solid.generation()) {
            queue.write_buffer(&b.sdf, 0, bytemuck::cast_slice(scene.solid.sdf_grid().3));
            self.sdf_generation = Some(scene.solid.generation());
        }
        queue.write_buffer(&b.pos, 0, bytemuck::cast_slice(&self.pos));
        queue.write_buffer(&b.vel, 0, bytemuck::cast_slice(&self.vel));

        let groups = |count: usize| (count as u32).div_ceil(WORKGROUP);
        let (particles, cells) = (groups(n), groups(b.cells));
        let mut encoder = self.gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("pbf") });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("pbf"), timestamp_writes: None });
            pass.set_bind_group(0, &b.bind_group, &[]);
            let p = &self.pipelines;
            let mut run = |pipeline: &wgpu::ComputePipeline, groups: u32| {
                pass.set_pipeline(pipeline);
                pass.dispatch_workgroups(groups, 1, 1);
            };
            run(&p.clear, cells);
            run(&p.predict, particles);
            run(&p.bin, particles);
            for _ in 0..self.params.iterations {
                run(&p.lambda, particles);
                run(&p.displacement, particles);
                run(&p.apply, particles);
            }
            run(&p.velocity, particles);
            if self.params.viscosity > 0.0 {
                run(&p.xsph, particles);
            }
            run(&p.commit, particles);
        }
        let bytes = (n * size_of::<Vec2>()) as u64;
        encoder.copy_buffer_to_buffer(&b.pos, 0, &b.staging, 0, bytes);
        encoder.copy_buffer_to_buffer(&b.vel, 0, &b.staging, bytes, bytes);
        encoder.copy_buffer_to_buffer(&b.lambda_rho, 0, &b.staging, 2 * bytes, bytes);
        queue.submit([encoder.finish()]);

        let slice = b.staging.slice(..3 * bytes);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map readback buffer"));
        self.gpu.device.poll(wgpu::PollType::wait_indefinitely()).expect("GPU poll");
        {
            let data = slice.get_mapped_range().expect("mapped readback");
            let floats: &[Vec2] = bytemuck::cast_slice(&data);
            self.pos.copy_from_slice(&floats[..n]);
            self.vel.copy_from_slice(&floats[n..2 * n]);
            self.density.clear();
            self.density.extend(floats[2 * n..3 * n].iter().map(|lr| lr.y));
        }
        b.staging.unmap();
    }
}

impl Solver for GpuPbf {
    fn name(&self) -> &'static str {
        "PBF (GPU)"
    }

    fn reset(&mut self, scene: &Scene, init: Init<'_>) {
        match init {
            Init::FromScene => {
                seed_particles(scene, self.params.spacing, 0.0, &mut self.pos);
                self.vel = vec![Vec2::ZERO; self.pos.len()];
            }
            Init::FromParticles(view) => resample(view, self.params.spacing, scene.size, &mut self.pos, &mut self.vel),
        }
        self.density = vec![0.0; self.pos.len()];
        self.emission.reset();
        self.buffers = None;
        self.stats = Stats { particles: self.pos.len(), ..Stats::default() };
    }

    fn max_dt(&self, _scene: &Scene) -> f32 {
        let max_speed = self.vel.iter().map(|v| v.length_squared()).fold(0.0, f32::max).sqrt();
        self.params.max_dt(max_speed)
    }

    fn step(&mut self, dt: f32, scene: &Scene, input: &StepInput) {
        let t0 = Instant::now();
        let (emitted, drained) = self.emission.apply(scene, input, dt, self.params.spacing, &mut self.pos, &mut self.vel);
        self.stats.emitted += emitted;
        self.stats.drained += drained;
        self.stats.particles = self.pos.len();
        apply_brushes(&input.brushes, dt, &self.pos, &mut self.vel);
        let t1 = Instant::now();
        if !self.pos.is_empty() {
            self.ensure_buffers(scene);
            self.write_params(scene, dt);
            self.run_gpu(scene);
        }
        let t2 = Instant::now();
        let rho0 = self.params.constants().rest_density;
        let compression = self.density.iter().map(|&rho| (rho / rho0 - 1.0).max(0.0)).sum::<f32>()
            / self.density.len().max(1) as f32;
        self.stats.phases = vec![("cpu prep", t1 - t0), ("gpu + readback", t2 - t1)];
        self.stats.metrics = vec![Metric { name: "mean compression", value: compression, tolerance: None }];
    }

    fn visit_params(&mut self, v: &mut dyn ParamVisitor) {
        self.params.visit(v);
    }

    fn particles(&self) -> Option<ParticleView<'_>> {
        Some(ParticleView { pos: &self.pos, vel: &self.vel, radius: self.radius() })
    }

    fn stats(&self) -> &Stats {
        &self.stats
    }
}
