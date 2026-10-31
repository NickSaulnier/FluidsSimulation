//! Stable fluids with dye in wgpu compute shaders: the CPU solver's algorithm
//! (`sim::solvers::StableFluids`, dye mode) at several times its resolution, for crisp
//! ink at full screen.
//!
//! Per substep, one compute pass: classify the cells (the scene's walls and drains,
//! built on the CPU when the mask changes, plus the emitters' nozzles) -> prescribed
//! faces -> buoyancy -> vorticity confinement -> brushes -> semi-Lagrangian velocity
//! advection -> divergence -> red-black SOR pressure sweeps -> projection -> MacCormack
//! dye advection, faded. Nothing is read back while stepping: the dye (with the fastest
//! speed, for the next substep's size) and the velocity come back only when asked for
//! (`fill_field`, `velocity_field`: once a frame, for drawing).
//!
//! The pressure solve is plain red-black SOR, warm-started from the last substep: at
//! this resolution it doesn't fully converge, which leaves the flow slightly
//! compressible but never unstable (the CPU solver's multigrid has no GPU port yet).

use crate::GpuContext;
use bytemuck::{Pod, Zeroable};
use sim::grid::Grid2;
use sim::solvers::{opens_border, resample_solids};
use sim::{FieldInfo, FieldKind, FieldLayout, FloatSpec, Init, IntSpec, Metric, ParamVisitor, ParticleView, Scene, Solver, Stats, StepInput, Vec2};
use std::cell::Cell;
use std::time::Instant;

const WORKGROUP: u32 = 16;
/// Room for emitters (2 entries each) and brushes (3 each); any beyond are dropped.
const GEOM_CAPACITY: usize = 1024;

#[derive(Clone, Debug)]
pub struct GpuStableParams {
    pub cells_y: u32,
    pub pressure_iters: u32,
    pub over_relax: f32,
    pub buoyancy: f32,
    pub vorticity: f32,
    pub dissipation: f32,
    pub cfl: f32,
}

impl Default for GpuStableParams {
    /// The CPU solver's, at three times its resolution (nine times the cells).
    fn default() -> Self {
        Self { cells_y: 384, pressure_iters: 80, over_relax: 1.9, buoyancy: 0.5, vorticity: 0.15, dissipation: 0.0, cfl: 3.0 }
    }
}

const CELLS_Y: IntSpec = IntSpec {
    key: "cells_y",
    label: "Grid cells (height)",
    range: 64..=1024,
    restart: true,
    help: "Resolution of the velocity, pressure and dye grids.",
};
const PRESSURE_ITERS: IntSpec = IntSpec {
    key: "pressure_iters",
    label: "Pressure iterations",
    range: 4..=400,
    restart: false,
    help: "Red-black SOR sweeps per substep (warm-started). More: less compressible, more GPU time.",
};
const OVER_RELAX: FloatSpec = FloatSpec {
    key: "over_relax",
    label: "Over-relaxation",
    range: 1.0..=1.98,
    log: false,
    restart: false,
    help: "SOR factor.",
};
const BUOYANCY: FloatSpec = FloatSpec {
    key: "buoyancy",
    label: "Dye heaviness",
    range: -1.0..=1.0,
    log: false,
    restart: false,
    help: "Positive: dye is heavy and sinks (ink). Negative: dye is light and rises (smoke).",
};
const VORTICITY: FloatSpec = FloatSpec {
    key: "vorticity",
    label: "Vorticity confinement",
    range: 0.0..=1.0,
    log: false,
    restart: false,
    help: "Re-injects the small swirls that semi-Lagrangian advection smears out.",
};
const DISSIPATION: FloatSpec = FloatSpec {
    key: "dissipation",
    label: "Dye fade (1/s)",
    range: 0.0..=1.0,
    log: false,
    restart: false,
    help: "Fraction of the dye that fades away per second.",
};
const CFL: FloatSpec = FloatSpec {
    key: "cfl",
    label: "CFL (cells per substep)",
    range: 0.5..=8.0,
    log: true,
    restart: false,
    help: "Max travel per substep. Advection is stable at any CFL; lower is more accurate.",
};

const FIELDS: &[FieldInfo] = &[
    FieldInfo { key: "dye", label: "Dye", unit: "", kind: FieldKind::Concentration },
    FieldInfo { key: "speed", label: "Speed", unit: "m/s", kind: FieldKind::NonNegative },
];

/// Cell kinds, as stable.wgsl has them.
const FLUID: u32 = 0;
const SOLID: u32 = 1;
const DRAIN: u32 = 2;

/// Must match `Params` in stable.wgsl (64 bytes).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    n: [u32; 2],
    nu: u32,
    n_emitters: u32,
    h: f32,
    dt: f32,
    buoyancy: f32,
    vorticity: f32,
    gravity: [f32; 2],
    dissipation: f32,
    omega: f32,
    n_brushes: u32,
    _pad: [u32; 3],
}

struct Pipelines {
    classify: wgpu::ComputePipeline,
    boundaries: wgpu::ComputePipeline,
    forces: wgpu::ComputePipeline,
    curl: wgpu::ComputePipeline,
    confine: wgpu::ComputePipeline,
    brushes: wgpu::ComputePipeline,
    advect_velocity: wgpu::ComputePipeline,
    commit_velocity: wgpu::ComputePipeline,
    divergence: wgpu::ComputePipeline,
    pressure_red: wgpu::ComputePipeline,
    pressure_black: wgpu::ComputePipeline,
    project: wgpu::ComputePipeline,
    advect_forward: wgpu::ComputePipeline,
    advect_correct: wgpu::ComputePipeline,
    commit_dye: wgpu::ComputePipeline,
}

/// The grids on the GPU, sized for `nx` × `ny` cells.
struct Buffers {
    vel: wgpu::Buffer,
    /// After a substep, holds a compact copy of the dye (for reading back).
    vel_tmp: wgpu::Buffer,
    dyes: wgpu::Buffer,
    flags: wgpu::Buffer,
    geom: wgpu::Buffer,
    max_speed: wgpu::Buffer,
    /// Mappable: the dye then the fastest speed; the velocity.
    dye_readback: wgpu::Buffer,
    vel_readback: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

pub struct GpuStable {
    pub params: GpuStableParams,
    gpu: GpuContext,
    layout: wgpu::BindGroupLayout,
    pipelines: Pipelines,
    uniform: wgpu::Buffer,
    buffers: Option<Buffers>,
    nx: usize,
    ny: usize,
    h: f32,
    /// The scene's walls and drains, by cell (the CPU's half of the classification).
    base_flags: Vec<u32>,
    base_key: Option<(u64, usize)>,
    /// The fastest speed last read back, for the substep size.
    speed: Cell<f32>,
    steps: u64,
    stats: Stats,
}

/// Registry constructor: GPU stable fluids if an adapter is available, otherwise the
/// CPU solver.
pub fn make() -> Box<dyn Solver> {
    match GpuContext::new() {
        Ok(gpu) => Box::new(GpuStable::new(gpu, GpuStableParams::default())),
        Err(e) => {
            log::warn!("GPU stable fluids unavailable ({e:#}); falling back to the CPU solver");
            Box::new(sim::solvers::StableFluids::default())
        }
    }
}

impl GpuStable {
    pub fn new(gpu: GpuContext, params: GpuStableParams) -> Self {
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("stable.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("stable.wgsl").into()),
        });
        let storage = |binding, read_only| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only }, has_dynamic_offset: false, min_binding_size: None },
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
        entries.extend((1..=5).map(|b| storage(b, false)));
        entries.push(storage(6, true));
        entries.push(storage(7, false));
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("stable"), entries: &entries });
        let pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("stable"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let pipeline = |entry: &str, constants: &[(&str, f64)]| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some(entry),
                compilation_options: wgpu::PipelineCompilationOptions { constants, ..Default::default() },
                cache: None,
            })
        };
        let pipelines = Pipelines {
            classify: pipeline("classify", &[]),
            boundaries: pipeline("boundaries", &[]),
            forces: pipeline("forces", &[]),
            curl: pipeline("curl", &[]),
            confine: pipeline("confine", &[]),
            brushes: pipeline("brushes", &[]),
            advect_velocity: pipeline("advect_velocity", &[]),
            commit_velocity: pipeline("commit_velocity", &[]),
            divergence: pipeline("divergence", &[]),
            pressure_red: pipeline("pressure", &[("PARITY", 0.0)]),
            pressure_black: pipeline("pressure", &[("PARITY", 1.0)]),
            project: pipeline("project", &[]),
            advect_forward: pipeline("advect_forward", &[]),
            advect_correct: pipeline("advect_correct", &[]),
            commit_dye: pipeline("commit_dye", &[]),
        };
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("stable params"),
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
            nx: 0,
            ny: 0,
            h: 1.0,
            base_flags: Vec::new(),
            base_key: None,
            speed: Cell::new(0.0),
            steps: 0,
            stats: Stats::default(),
        }
    }

    /// Velocity floats: u then v.
    fn vel_len(&self) -> usize {
        (self.nx + 1) * self.ny + self.nx * (self.ny + 1)
    }

    fn create_buffers(&mut self) {
        let device = &self.gpu.device;
        let cells = self.nx * self.ny;
        let buffer = |label: &str, size: usize, usage: wgpu::BufferUsages| {
            device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: size.max(4) as u64, usage, mapped_at_creation: false })
        };
        let rw = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC;
        let vel_bytes = self.vel_len() * 4;
        let vel = buffer("vel", vel_bytes, rw);
        let vel_tmp = buffer("vel_tmp", vel_bytes.max(cells * 4), rw);
        let dyes = buffer("dyes", cells * 16, rw);
        let pd = buffer("pd", cells * 8, rw);
        let flags = buffer("flags", cells * 8, rw);
        let geom = buffer("geom", GEOM_CAPACITY * 16, rw);
        let max_speed = buffer("max_speed", 4, rw);
        let readable = wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST;
        let dye_readback = buffer("dye readback", cells * 4 + 4, readable);
        let vel_readback = buffer("vel readback", vel_bytes, readable);
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("stable"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: vel.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: vel_tmp.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: dyes.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: pd.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: flags.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 6, resource: geom.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 7, resource: max_speed.as_entire_binding() },
            ],
        });
        self.buffers = Some(Buffers { vel, vel_tmp, dyes, flags, geom, max_speed, dye_readback, vel_readback, bind_group });
    }

    /// The scene's walls (the border always, but where a drain opens it) and drains, by
    /// cell, as the CPU solver classifies them; uploaded when the mask or drains change.
    fn update_base_flags(&mut self, scene: &Scene) {
        let key = (scene.solid.generation(), scene.drains.len());
        if self.base_key == Some(key) {
            return;
        }
        let (nx, ny, h) = (self.nx, self.ny, self.h);
        let mut solid = Grid2::new(nx, ny);
        resample_solids(scene, h, &mut solid);
        self.base_flags = (0..nx * ny)
            .map(|k| {
                let (i, j) = (k % nx, k / nx);
                let c = (Vec2::new(i as f32, j as f32) + 0.5) * h;
                if solid[(i, j)] && !opens_border(scene, c) {
                    SOLID
                } else if scene.drains.iter().any(|d| d.contains(c)) {
                    DRAIN
                } else {
                    FLUID
                }
            })
            .collect();
        // (current, base): classify rewrites the current kind every substep.
        let both: Vec<[u32; 2]> = self.base_flags.iter().map(|&f| [f, f]).collect();
        let b = self.buffers.as_ref().expect("buffers");
        self.gpu.queue.write_buffer(&b.flags, 0, bytemuck::cast_slice(&both));
        self.base_key = Some(key);
    }

    /// Emitters then brushes, as stable.wgsl reads them (as many as fit). Returns how
    /// many of each.
    fn upload_geometry(&self, scene: &Scene, input: &StepInput) -> (u32, u32) {
        let mut geom: Vec<[f32; 4]> = Vec::new();
        let emitters: Vec<_> = scene.emitters.iter().chain(&input.emitters).take(GEOM_CAPACITY / 4).collect();
        for e in &emitters {
            let dir = e.dir.normalize_or(Vec2::NEG_Y);
            geom.push([e.pos.x, e.pos.y, dir.x, dir.y]);
            geom.push([e.speed, e.width, e.dye, 0.0]);
        }
        let brushes: Vec<_> = input.brushes.iter().take((GEOM_CAPACITY - geom.len()) / 3).collect();
        for b in &brushes {
            geom.push([b.pos.x, b.pos.y, b.radius, 0.0]);
            geom.push([b.velocity.x, b.velocity.y, b.strength, 0.0]);
            geom.push([b.spin, 0.0, 0.0, 0.0]);
        }
        if !geom.is_empty() {
            let b = self.buffers.as_ref().expect("buffers");
            self.gpu.queue.write_buffer(&b.geom, 0, bytemuck::cast_slice(&geom));
        }
        (emitters.len() as u32, brushes.len() as u32)
    }

    /// Read `bytes` of `source` back to the CPU (blocking).
    fn read_back(&self, source: &wgpu::Buffer, staging: &wgpu::Buffer, bytes: u64, extra: Option<(&wgpu::Buffer, u64)>) -> Vec<u8> {
        let mut encoder = self.gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("stable readback") });
        encoder.copy_buffer_to_buffer(source, 0, staging, 0, bytes);
        let mut total = bytes;
        if let Some((buffer, len)) = extra {
            encoder.copy_buffer_to_buffer(buffer, 0, staging, bytes, len);
            total += len;
        }
        self.gpu.queue.submit([encoder.finish()]);
        let slice = staging.slice(..total);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map readback buffer"));
        self.gpu.device.poll(wgpu::PollType::wait_indefinitely()).expect("GPU poll");
        let data = slice.get_mapped_range().expect("mapped readback").to_vec();
        staging.unmap();
        data
    }

    /// The dye, per cell (and the fastest speed of the last substep, kept for its size).
    fn read_dye(&self) -> Vec<f32> {
        let b = self.buffers.as_ref().expect("buffers");
        let cells = self.nx * self.ny;
        let data = self.read_back(&b.vel_tmp, &b.dye_readback, (cells * 4) as u64, Some((&b.max_speed, 4)));
        let floats: &[f32] = bytemuck::cast_slice(&data);
        let speed = floats[cells];
        if speed.is_finite() {
            self.speed.set(speed);
        }
        floats[..cells].to_vec()
    }

    /// The velocity at cell centres (zero in walls).
    fn read_velocity(&self) -> Vec<Vec2> {
        let b = self.buffers.as_ref().expect("buffers");
        let data = self.read_back(&b.vel, &b.vel_readback, (self.vel_len() * 4) as u64, None);
        let v: &[f32] = bytemuck::cast_slice(&data);
        let (nx, ny) = (self.nx, self.ny);
        let nu = (nx + 1) * ny;
        let mut out = Vec::with_capacity(nx * ny);
        let mut fastest = 0.0f32;
        for j in 0..ny {
            for i in 0..nx {
                if self.base_flags[j * nx + i] == SOLID {
                    out.push(Vec2::ZERO);
                    continue;
                }
                let u = 0.5 * (v[j * (nx + 1) + i] + v[j * (nx + 1) + i + 1]);
                let w = 0.5 * (v[nu + j * nx + i] + v[nu + (j + 1) * nx + i]);
                let c = Vec2::new(u, w);
                fastest = fastest.max(c.length());
                out.push(c);
            }
        }
        if fastest.is_finite() {
            self.speed.set(self.speed.get().max(fastest));
        }
        out
    }
}

impl Solver for GpuStable {
    fn name(&self) -> &'static str {
        "Stable fluids (GPU)"
    }

    fn reset(&mut self, scene: &Scene, init: Init<'_>) {
        let ny = self.params.cells_y.max(8) as usize;
        self.h = scene.size.y / ny as f32;
        (self.nx, self.ny) = (((scene.size.x / self.h).round() as usize).max(8), ny);
        self.create_buffers();
        self.base_key = None;
        self.update_base_flags(scene);
        let (nx, h) = (self.nx, self.h);
        let mut dye = vec![0.0f32; nx * ny];
        match init {
            Init::FromScene => {
                for (k, d) in dye.iter_mut().enumerate() {
                    let c = (Vec2::new((k % nx) as f32, (k / nx) as f32) + 0.5) * h;
                    if self.base_flags[k] == FLUID && scene.fluid_blocks.iter().any(|b| b.contains(c)) {
                        *d = 1.0;
                    }
                }
            }
            // Another solver's particles become dye: their density per cell, relative to
            // a densely filled cell, capped at 1.
            Init::FromParticles(view) => {
                let per_cell = (h / (2.0 * view.radius)).powi(2);
                for &p in view.pos {
                    let (i, j) = ((p.x / h) as usize, (p.y / h) as usize);
                    if i < nx && j < ny {
                        dye[j * nx + i] = (dye[j * nx + i] + 1.0 / per_cell).min(1.0);
                    }
                }
            }
        }
        // The dye into place, and its compact copy, so it reads back before any step.
        let packed: Vec<[f32; 4]> = dye.iter().map(|&d| [d, d, d, 0.0]).collect();
        let b = self.buffers.as_ref().expect("buffers");
        self.gpu.queue.write_buffer(&b.dyes, 0, bytemuck::cast_slice(&packed));
        self.gpu.queue.write_buffer(&b.vel_tmp, 0, bytemuck::cast_slice(&dye));
        self.speed.set(0.0);
        self.steps = 0;
        self.stats = Stats::default();
    }

    fn max_dt(&self, _scene: &Scene) -> f32 {
        // Semi-Lagrangian advection is stable at any step; the CFL keeps it accurate.
        (self.params.cfl * self.h / self.speed.get().max(0.25)).min(1.0 / 60.0)
    }

    fn step(&mut self, dt: f32, scene: &Scene, input: &StepInput) {
        let t0 = Instant::now();
        self.update_base_flags(scene);
        let (n_emitters, n_brushes) = self.upload_geometry(scene, input);
        let params = Params {
            n: [self.nx as u32, self.ny as u32],
            nu: ((self.nx + 1) * self.ny) as u32,
            n_emitters,
            h: self.h,
            dt,
            buoyancy: self.params.buoyancy,
            vorticity: self.params.vorticity,
            gravity: scene.gravity.into(),
            dissipation: self.params.dissipation,
            omega: self.params.over_relax,
            n_brushes,
            _pad: [0; 3],
        };
        let queue = &self.gpu.queue;
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&params));
        let b = self.buffers.as_ref().expect("buffers");
        queue.write_buffer(&b.max_speed, 0, bytemuck::bytes_of(&0u32));

        let groups = |w: usize, h: usize| ((w as u32).div_ceil(WORKGROUP), (h as u32).div_ceil(WORKGROUP));
        // Cells and faces both fit in (nx + 1) × (ny + 1).
        let (gx, gy) = groups(self.nx + 1, self.ny + 1);
        let mut encoder = self.gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("stable") });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("stable"), timestamp_writes: None });
            pass.set_bind_group(0, &b.bind_group, &[]);
            let p = &self.pipelines;
            let mut run = |pipeline: &wgpu::ComputePipeline| {
                pass.set_pipeline(pipeline);
                pass.dispatch_workgroups(gx, gy, 1);
            };
            run(&p.classify);
            run(&p.boundaries);
            run(&p.forces);
            if self.params.vorticity > 0.0 {
                run(&p.curl);
                run(&p.confine);
            }
            if n_brushes > 0 {
                run(&p.brushes);
            }
            run(&p.boundaries);
            run(&p.advect_velocity);
            run(&p.commit_velocity);
            run(&p.divergence);
            for _ in 0..self.params.pressure_iters {
                run(&p.pressure_red);
                run(&p.pressure_black);
            }
            run(&p.project);
            run(&p.advect_forward);
            run(&p.advect_correct);
            run(&p.commit_dye);
        }
        queue.submit([encoder.finish()]);
        self.steps += 1;
        self.stats.phases = vec![("gpu submit", t0.elapsed())];
        self.stats.metrics = vec![Metric { name: "fastest (m/s, last read)", value: self.speed.get(), tolerance: None }];
    }

    fn visit_params(&mut self, v: &mut dyn ParamVisitor) {
        let p = &mut self.params;
        v.section("Dye");
        v.float(&BUOYANCY, &mut p.buoyancy);
        v.float(&DISSIPATION, &mut p.dissipation);
        v.float(&VORTICITY, &mut p.vorticity);
        v.section("Resolution");
        v.int(&CELLS_Y, &mut p.cells_y);
        v.advanced_section("Solver");
        v.int(&PRESSURE_ITERS, &mut p.pressure_iters);
        v.float(&OVER_RELAX, &mut p.over_relax);
        v.float(&CFL, &mut p.cfl);
    }

    fn particles(&self) -> Option<ParticleView<'_>> {
        None
    }

    fn fields(&self) -> &'static [FieldInfo] {
        FIELDS
    }

    fn fill_field(&self, index: usize, out: &mut Vec<f32>) -> Option<FieldLayout> {
        let info = FIELDS.get(index)?;
        self.buffers.as_ref()?;
        let layout = FieldLayout { nx: self.nx, ny: self.ny, origin: Vec2::ZERO, cell: self.h };
        out.clear();
        match info.key {
            "dye" => out.extend(self.read_dye()),
            _ => out.extend(self.read_velocity().iter().map(|v| v.length())),
        }
        for (v, &flag) in out.iter_mut().zip(&self.base_flags) {
            if flag == SOLID {
                *v = f32::NAN;
            }
        }
        Some(layout)
    }

    fn velocity_field(&self, out: &mut Vec<Vec2>) -> Option<FieldLayout> {
        self.buffers.as_ref()?;
        out.clear();
        out.extend(self.read_velocity());
        Some(FieldLayout { nx: self.nx, ny: self.ny, origin: Vec2::ZERO, cell: self.h })
    }

    fn stats(&self) -> &Stats {
        &self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim::presets;
    use sim::stepper::Stepper;

    fn solver(cells_y: u32, params: impl FnOnce(&mut GpuStableParams)) -> Option<GpuStable> {
        let gpu = match GpuContext::new() {
            Ok(g) => g,
            Err(e) => {
                eprintln!("skipping: {e:#}");
                return None;
            }
        };
        let mut p = GpuStableParams { cells_y, ..GpuStableParams::default() };
        params(&mut p);
        Some(GpuStable::new(gpu, p))
    }

    fn run(s: &mut GpuStable, scene: &Scene, input: &StepInput, seconds: f32) {
        let stepper = Stepper::default();
        for frame in 0..(seconds * 60.0) as usize {
            stepper.advance(s, scene, 1.0 / 60.0, input);
            if frame % 10 == 0 {
                // As the app does each frame: reading the dye keeps the substep size current.
                s.read_dye();
            }
        }
    }

    fn centroid(s: &GpuStable) -> (Vec2, f32) {
        let dye = s.read_dye();
        let (mut sum, mut total) = (Vec2::ZERO, 0.0);
        for (k, d) in dye.iter().enumerate() {
            let c = (Vec2::new((k % s.nx) as f32, (k / s.nx) as f32) + 0.5) * s.h;
            sum += c * *d;
            total += d;
        }
        (sum / total.max(1e-6), total)
    }

    /// Heavy dye slumps along the floor (as the CPU solver's), staying between 0 and 1,
    /// and the velocity stays finite.
    #[test]
    fn heavy_dye_slumps() {
        let Some(mut s) = solver(128, |_| {}) else { return };
        let scene = presets::dam_break();
        s.reset(&scene, Init::FromScene);
        let (start, amount) = centroid(&s);
        run(&mut s, &scene, &StepInput::default(), 2.0);
        let (end, after) = centroid(&s);
        assert!(end.y < start.y - 0.05 && end.x > start.x + 0.1, "{start} -> {end}");
        assert!((after / amount - 1.0).abs() < 0.15, "dye {amount} -> {after}");
        let dye = s.read_dye();
        assert!(dye.iter().all(|d| d.is_finite() && (-1e-4..=1.0 + 1e-4).contains(d)));
        assert!(s.read_velocity().iter().all(|v| v.is_finite() && v.length() < 20.0));
    }

    /// Light dye rises.
    #[test]
    fn light_dye_rises() {
        let Some(mut s) = solver(96, |p| p.buoyancy = -0.5) else { return };
        let scene = presets::dam_break();
        s.reset(&scene, Init::FromScene);
        let (start, _) = centroid(&s);
        run(&mut s, &scene, &StepInput::default(), 1.5);
        assert!(centroid(&s).0.y > start.y + 0.03, "{start} -> {}", centroid(&s).0);
    }

    /// The wind tunnel flows through: dye comes in at the nozzle, streams past the
    /// cylinder and leaves through the drain, without piling up.
    #[test]
    fn the_wind_tunnel_flows_through() {
        let Some(mut s) = solver(96, |_| {}) else { return };
        let scene = presets::wind_tunnel();
        s.reset(&scene, Init::FromScene);
        run(&mut s, &scene, &StepInput::default(), 6.0);
        let v = s.read_velocity();
        let mid = v[(s.ny / 4) * s.nx + 3 * s.nx / 4];
        assert!(mid.x > 0.2, "no flow downstream: {mid}");
        let (_, after) = centroid(&s);
        run(&mut s, &scene, &StepInput::default(), 3.0);
        let (_, later) = centroid(&s);
        assert!(later < 1.2 * after, "dye piles up: {after} -> {later}");
    }

    /// A spinning brush stirs a vortex in its direction (anticlockwise: up on the right).
    #[test]
    fn a_spinning_brush_stirs_a_vortex() {
        let Some(mut s) = solver(96, |_| {}) else { return };
        let scene = presets::dam_break();
        s.reset(&scene, Init::FromScene);
        let centre = Vec2::new(1.4, 0.5);
        let brush = sim::ForceBrush { pos: centre, radius: 0.2, velocity: Vec2::ZERO, strength: 25.0, spin: 10.0 };
        run(&mut s, &scene, &StepInput { brushes: vec![brush], ..StepInput::default() }, 0.3);
        let v = s.read_velocity();
        let at = |p: Vec2| v[(p.y / s.h) as usize * s.nx + (p.x / s.h) as usize];
        assert!(at(centre + Vec2::new(0.1, 0.0)).y > 0.2 && at(centre - Vec2::new(0.1, 0.0)).y < -0.2);
    }
}

