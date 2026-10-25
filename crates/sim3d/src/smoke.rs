//! Smoke and fire in 3D, all on the GPU: stable fluids on a MAC grid in compute
//! shaders (`smoke.wgsl`), the 3D counterpart of the 2D `StableFluids` in its smoke and
//! fire modes. The two solvers differ only in parameters ([`SmokeParams::smoke`],
//! [`SmokeParams::fire`]): smoke burners release smoke; fire burners release lit fuel,
//! which burns into heat and soot.
//!
//! Each substep, in five timed passes:
//! 1. **Sources and forces:** cell types from the scene (solids, vents, burners);
//!    prescribed faces (zero at walls and solids, the exit velocity at burners); the
//!    burners' gas in their cells, flickering; Boussinesq buoyancy (heat rises, smoke
//!    is a little heavy); vorticity confinement (ε·h·N×ω, which spins the small swirls
//!    back up); brushes (the mouse and the music), as in 2D.
//! 2. **Advect velocity:** semi-Lagrangian with an RK2 backtrace.
//! 3. **Pressure:** divergence, then warm-started red-black SOR for ∇²φ = ∇·u. Where
//!    a vent touches the domain's wall the wall is open, with φ = 0 beyond it; other
//!    walls, solids and burners are Neumann.
//! 4. **Project:** u -= ∇φ on the free faces.
//! 5. **Advect smoke, heat and fuel:** MacCormack with the min/max limiter, then
//!    combustion as in 2D (fuel burns at `burn_rate` where the gas is at least
//!    `ignition` hot, releasing `heat` and `soot`), cooling (dT/dt = -k·T², implicit)
//!    and fading. Gas leaves through the open walls.
//!
//! Temperature is above ambient and dimensionless, as in 2D: ignition at 0.3, a
//! visible flame from about 1, burning gas up to about 5. There's only gas fuel here
//! (no solid piles), so the 2D model's heat diffusion, which lets a flame creep into
//! still fuel, isn't needed.
//!
//! Burners are inflow cells: every face of theirs carries the exit velocity, so a
//! burner off the floor pushes gas through (as much in as out) and one on the floor
//! adds it; the open walls let it out (and let air back in elsewhere). Vent cells are
//! ordinary fluid: only the wall beside them changes, so the flow carries straight on
//! out instead of stalling at the vent. A vent away from the walls has no effect.
//!
//! Fields live in storage buffers, sampled trilinearly in the shader: plain buffers
//! need no filterable float textures and fit wgpu's default limit of eight storage
//! buffers per stage. After each step the smoke and temperature are copied into a
//! 3D texture (`Solver3d::volume`) for the renderer to sample with hardware
//! filtering. Nothing is read back except the pass timings (timestamp queries, a
//! frame or two late, when the device supports them).

use crate::scene::{Scene3d, Shape};
use crate::solver::{Solver3d, StepInput3d, Volume};
use bytemuck::{Pod, Zeroable};
use glam::{UVec3, Vec3};
use sim::{FloatSpec, IntSpec, Metric, ParamVisitor, Stats};
use sim_gpu::GpuContext;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Longest substep. Semi-Lagrangian advection is stable at any step; this keeps the
/// buoyancy and burners accurate.
const MAX_SUBSTEP: f32 = 1.0 / 60.0;
/// Geometry capacity (vec4s), and the most of each kind it holds.
const MAX_GEOM: usize = 512;
const MAX_SHAPES: usize = 32;
const MAX_VENTS: usize = 16;
const MAX_BURNERS: usize = 64;
const MAX_BRUSHES: usize = 32;
const WORKGROUP: UVec3 = UVec3::new(8, 8, 4);
const PHASES: [&str; 6] = ["sources + forces", "advect velocity", "pressure", "project", "advect smoke", "volume copy"];
const VOLUME_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

#[derive(Clone, Debug)]
pub struct SmokeParams {
    pub cells_y: u32,
    pub pressure_iters: u32,
    pub over_relax: f32,
    /// Upward acceleration per unit temperature, as a fraction of g.
    pub rise: f32,
    /// Acceleration along gravity per unit smoke, as a fraction of g.
    pub heaviness: f32,
    /// dT/dt = -cooling · T² - heat_loss · T: the first keeps flames short, the second
    /// stops merely warm gas collecting in the box (which otherwise heats up, and turns
    /// over in big convection cells that blow the flames about).
    pub cooling: f32,
    pub heat_loss: f32,
    /// Air drag: velocity decays at this rate, 1/s. A box with only its top open turns
    /// over slowly as a whole (the plume's air comes back down the walls and across the
    /// floor), and that circulation grows until it blows a flame flat; a little drag
    /// keeps it weak while buoyancy keeps driving the plume.
    pub drag: f32,
    /// Fraction of the smoke lost per second.
    pub fade: f32,
    /// Vorticity confinement strength.
    pub vorticity: f32,
    /// Temperature of the gas leaving a burner.
    pub burner_temp: f32,
    /// Smoke and fuel in the gas leaving a burner.
    pub burner_smoke: f32,
    pub burner_fuel: f32,
    /// Burner output varies by up to this fraction, cell to cell and over time.
    pub flicker: f32,
    /// Temperature at which fuel burns.
    pub ignition: f32,
    /// Fraction of the fuel at a burning point consumed per second (exponential).
    pub burn_rate: f32,
    /// Temperature released per unit of fuel burned.
    pub heat: f32,
    /// Smoke released per unit of fuel burned.
    pub soot: f32,
}

impl Default for SmokeParams {
    fn default() -> Self {
        Self::smoke()
    }
}

impl SmokeParams {
    /// Burners release warm smoke.
    pub fn smoke() -> Self {
        Self {
            cells_y: 160,
            pressure_iters: 40,
            over_relax: 1.8,
            rise: 0.15,
            heaviness: 0.02,
            cooling: 0.6,
            heat_loss: 0.3,
            drag: 0.2,
            fade: 0.0,
            vorticity: 0.3,
            burner_temp: 1.0,
            burner_smoke: 1.0,
            burner_fuel: 0.0,
            flicker: 0.0,
            ignition: 0.3,
            burn_rate: 3.0,
            heat: 6.0,
            soot: 0.3,
        }
    }

    /// Burners release lit fuel: flames, then smoke from the soot. The 2D fire's
    /// model and values, with a flickering burner. The soot fades (about 25% a second):
    /// a fire burns for good, and without it the soot the vent doesn't catch fills the
    /// box with haze within half a minute.
    pub fn fire() -> Self {
        Self {
            rise: 0.12,
            cooling: 1.5,
            fade: 0.3,
            drag: 0.3,
            vorticity: 0.5,
            burner_smoke: 0.0,
            burner_fuel: 1.0,
            flicker: 0.4,
            ..Self::smoke()
        }
    }

    fn is_fire(&self) -> bool {
        self.burner_fuel > 0.0
    }
}

const CELLS_Y: IntSpec = IntSpec {
    key: "cells_y",
    label: "Grid cells (height)",
    range: 16..=256,
    restart: true,
    help: "Resolution of the volume. The cells are cubes, so the width and depth follow the domain's shape.",
};
const PRESSURE_ITERS: IntSpec = IntSpec {
    key: "pressure_iters",
    label: "Pressure iterations",
    range: 1..=200,
    restart: false,
    help: "Red-black Gauss-Seidel sweeps per substep (warm-started). More: less compressible gas, more GPU time.",
};
const OVER_RELAX: FloatSpec = FloatSpec {
    key: "over_relax",
    label: "Over-relaxation",
    range: 1.0..=1.95,
    log: false,
    restart: false,
    help: "SOR factor.",
};
const RISE: FloatSpec = FloatSpec {
    key: "rise",
    label: "Heat rise",
    range: 0.0..=1.0,
    log: false,
    restart: false,
    help: "Upward push per unit of temperature, as a fraction of g.",
};
const HEAVINESS: FloatSpec = FloatSpec {
    key: "buoyancy",
    label: "Smoke heaviness",
    range: -0.5..=0.5,
    log: false,
    restart: false,
    help: "Positive: cold smoke sinks. Negative: it keeps rising after the heat has gone.",
};
const COOLING: FloatSpec = FloatSpec {
    key: "cooling",
    label: "Cooling",
    range: 0.0..=5.0,
    log: false,
    restart: false,
    help: "How fast hot gas cools (dT/dt = -k·T²): the plume slows and spreads once cool.",
};
const HEAT_LOSS: FloatSpec = FloatSpec {
    key: "heat_loss",
    label: "Heat loss (1/s)",
    range: 0.0..=2.0,
    log: false,
    restart: false,
    help: "Warm gas losing heat to the surroundings. Without it a closed-in fire heats its whole box and the air starts churning.",
};
const DRAG: FloatSpec = FloatSpec {
    key: "drag",
    label: "Air drag (1/s)",
    range: 0.0..=2.0,
    log: false,
    restart: false,
    help: "Slows all motion. Keeps the box's slow overall churning from blowing flames about.",
};
const FADE: FloatSpec = FloatSpec {
    key: "dissipation",
    label: "Smoke fade (1/s)",
    range: 0.0..=1.0,
    log: false,
    restart: false,
    help: "Fraction of the smoke that fades away per second.",
};
const BURNER_TEMP: FloatSpec = FloatSpec {
    key: "burner",
    label: "Burner temperature",
    range: 0.0..=3.0,
    log: false,
    restart: false,
    help: "Temperature of the gas leaving the burners.",
};
const VORTICITY: FloatSpec = FloatSpec {
    key: "vorticity",
    label: "Vorticity confinement",
    range: 0.0..=2.0,
    log: false,
    restart: false,
    help: "Spins up the small swirls that advection smears out. Too much and the gas boils.",
};
const BURNER_FUEL: FloatSpec = FloatSpec {
    key: "burner_fuel",
    label: "Burner fuel",
    range: 0.0..=2.0,
    log: false,
    restart: false,
    help: "Fuel in the gas leaving the burners: 0 makes smoke only.",
};
const FLICKER: FloatSpec = FloatSpec {
    key: "flicker",
    label: "Burner flicker",
    range: 0.0..=1.0,
    log: false,
    restart: false,
    help: "How much the burners' output wavers, which sets flames flickering.",
};
const IGNITION: FloatSpec = FloatSpec {
    key: "ignition",
    label: "Ignition temperature",
    range: 0.05..=1.0,
    log: true,
    restart: false,
    help: "Fuel burns where the gas is at least this hot.",
};
const BURN_RATE: FloatSpec = FloatSpec {
    key: "burn_rate",
    label: "Burn rate (1/s)",
    range: 0.2..=20.0,
    log: true,
    restart: false,
    help: "How fast burning fuel is consumed. Faster: short, hot flames.",
};
const HEAT: FloatSpec = FloatSpec {
    key: "heat",
    label: "Heat per fuel",
    range: 0.5..=20.0,
    log: true,
    restart: false,
    help: "Temperature released by burning a unit of fuel.",
};
const SOOT: FloatSpec = FloatSpec {
    key: "soot",
    label: "Soot per fuel",
    range: 0.0..=2.0,
    log: false,
    restart: false,
    help: "Smoke released by burning a unit of fuel. 0 burns clean.",
};

impl SmokeParams {
    pub fn visit(&mut self, v: &mut dyn ParamVisitor) {
        v.section("Grid");
        v.int(&CELLS_Y, &mut self.cells_y);
        v.int(&PRESSURE_ITERS, &mut self.pressure_iters);
        v.float(&OVER_RELAX, &mut self.over_relax);
        v.section(if self.is_fire() { "Fire" } else { "Smoke" });
        if self.is_fire() {
            v.float(&BURN_RATE, &mut self.burn_rate);
            v.float(&HEAT, &mut self.heat);
            v.float(&SOOT, &mut self.soot);
        }
        v.float(&RISE, &mut self.rise);
        v.float(&HEAVINESS, &mut self.heaviness);
        v.float(&COOLING, &mut self.cooling);
        v.float(&HEAT_LOSS, &mut self.heat_loss);
        v.float(&DRAG, &mut self.drag);
        v.float(&FADE, &mut self.fade);
        v.float(&VORTICITY, &mut self.vorticity);
        v.advanced_section("Burners");
        v.float(&BURNER_TEMP, &mut self.burner_temp);
        v.float(&BURNER_FUEL, &mut self.burner_fuel);
        v.float(&FLICKER, &mut self.flicker);
        v.float(&IGNITION, &mut self.ignition);
    }
}

/// Cubic cells, `cells_y` of them up the domain: the cell counts and the cell size.
pub fn grid(size: Vec3, cells_y: u32) -> (UVec3, f32) {
    let h = size.y / cells_y as f32;
    ((size / h).round().as_uvec3().max(UVec3::splat(2)), h)
}

/// Must match `Params` in smoke.wgsl (128 bytes).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    n: [u32; 3],
    h: f32,
    gravity: [f32; 3],
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
    heat_loss: f32,
    drag: f32,
    _pad: u32,
}

struct Pipelines {
    classify: wgpu::ComputePipeline,
    boundaries: wgpu::ComputePipeline,
    forces: wgpu::ComputePipeline,
    advect_velocity: wgpu::ComputePipeline,
    divergence: wgpu::ComputePipeline,
    pressure_red: wgpu::ComputePipeline,
    pressure_black: wgpu::ComputePipeline,
    project: wgpu::ComputePipeline,
    advect_forward: wgpu::ComputePipeline,
    advect_correct: wgpu::ComputePipeline,
    export_volume: wgpu::ComputePipeline,
    curl: wgpu::ComputePipeline,
    confine: wgpu::ComputePipeline,
    brushes: wgpu::ComputePipeline,
}

/// The grid's GPU buffers.
struct Fields {
    n: UVec3,
    h: f32,
    nu: u32,
    nv: u32,
    vel: wgpu::Buffer,
    vel_tmp: wgpu::Buffer,
    scal: wgpu::Buffer,
    _scal_hat: wgpu::Buffer,
    scal_tmp: wgpu::Buffer,
    pd: wgpu::Buffer,
    flags: wgpu::Buffer,
    /// (smoke, temperature) as a filterable texture, for drawing.
    volume: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
}

impl Fields {
    fn cells(&self) -> u32 {
        self.n.element_product()
    }

    fn vel_len(&self) -> u32 {
        self.nu + self.nv + self.n.x * self.n.y * (self.n.z + 1)
    }

    fn cell_groups(&self) -> UVec3 {
        groups(self.n)
    }

    fn face_groups(&self) -> UVec3 {
        groups(self.n + 1)
    }
}

enum TimingState {
    Idle,
    /// The resolved timestamps are being copied, in the step just recorded.
    Copied,
    /// Waiting for the copy to map; the flag is set when it has.
    Mapping(Arc<AtomicBool>),
}

/// GPU timings per pass, from timestamp queries.
struct Timing {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    state: TimingState,
    period_ns: f32,
}

pub struct Smoke3d {
    pub params: SmokeParams,
    name: &'static str,
    /// Simulated seconds since reset (the burners' flicker follows it).
    time: f32,
    layout: wgpu::BindGroupLayout,
    pipelines: Pipelines,
    uniform: wgpu::Buffer,
    geom: wgpu::Buffer,
    fields: Option<Fields>,
    timing: Option<Timing>,
    stats: Stats,
}

impl Smoke3d {
    pub fn new(gpu: &GpuContext, name: &'static str, params: SmokeParams) -> Self {
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("smoke.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("smoke.wgsl").into()),
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
                min_binding_size: wgpu::BufferSize::new(size_of::<Uniforms>() as u64),
            },
            count: None,
        }];
        entries.extend((1..=7).map(|b| storage(b, false)));
        entries.push(storage(8, true));
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 9,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: VOLUME_FORMAT,
                view_dimension: wgpu::TextureViewDimension::D3,
            },
            count: None,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("smoke3d"), entries: &entries });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("smoke3d"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
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
            advect_velocity: pipeline("advect_velocity", &[]),
            divergence: pipeline("divergence", &[]),
            pressure_red: pipeline("pressure", &[("PARITY", 0.0)]),
            pressure_black: pipeline("pressure", &[("PARITY", 1.0)]),
            project: pipeline("project", &[]),
            advect_forward: pipeline("advect_forward", &[]),
            advect_correct: pipeline("advect_correct", &[]),
            export_volume: pipeline("export_volume", &[]),
            curl: pipeline("curl", &[]),
            confine: pipeline("confine", &[]),
            brushes: pipeline("brushes", &[]),
        };
        let buffer = |label: &str, size: u64, usage: wgpu::BufferUsages| {
            device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size, usage, mapped_at_creation: false })
        };
        let uniform = buffer("smoke3d params", size_of::<Uniforms>() as u64, wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST);
        let geom = buffer("smoke3d geometry", (MAX_GEOM * 16) as u64, wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST);
        let timing = device.features().contains(wgpu::Features::TIMESTAMP_QUERY).then(|| {
            let count = 2 * PHASES.len() as u32;
            let bytes = u64::from(count) * 8;
            Timing {
                queries: device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some("smoke3d timings"),
                    ty: wgpu::QueryType::Timestamp,
                    count,
                }),
                resolve: buffer("smoke3d timings", bytes, wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC),
                readback: buffer("smoke3d timings readback", bytes, wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST),
                state: TimingState::Idle,
                period_ns: gpu.queue.get_timestamp_period(),
            }
        });
        Self { params, name, time: 0.0, layout, pipelines, uniform, geom, fields: None, timing, stats: Stats::default() }
    }

    /// The grid's cell counts and cell size, once reset.
    pub fn dims(&self) -> Option<(UVec3, f32)> {
        self.fields.as_ref().map(|f| (f.n, f.h))
    }

    fn allocate(&mut self, gpu: &GpuContext, scene: &Scene3d) {
        let (n, h) = grid(scene.size, self.params.cells_y);
        let nu = (n.x + 1) * n.y * n.z;
        let nv = n.x * (n.y + 1) * n.z;
        let vel_len = u64::from(nu + nv + n.x * n.y * (n.z + 1));
        let cells = u64::from(n.element_product());
        let device = &gpu.device;
        let rw = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC;
        let buffer = |label: &str, size: u64| {
            device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size, usage: rw, mapped_at_creation: false })
        };
        let vel = buffer("velocity", vel_len * 4);
        let vel_tmp = buffer("velocity (advected)", vel_len * 4);
        let scal = buffer("smoke, temperature, fuel", cells * 16);
        let scal_hat = buffer("smoke, temperature, fuel (forward)", cells * 16);
        let scal_tmp = buffer("smoke, temperature, fuel (corrected)", cells * 16);
        let pd = buffer("pressure, divergence", cells * 8);
        let flags = buffer("cell types", cells * 4);
        let volume = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("smoke volume"),
                size: wgpu::Extent3d { width: n.x, height: n.y, depth_or_array_layers: n.z },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D3,
                format: VOLUME_FORMAT,
                usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("smoke3d"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: vel.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: vel_tmp.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: scal.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: scal_hat.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: scal_tmp.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 6, resource: pd.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 7, resource: flags.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 8, resource: self.geom.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 9, resource: wgpu::BindingResource::TextureView(&volume) },
            ],
        });
        self.fields = Some(Fields { n, h, nu, nv, vel, vel_tmp, scal, _scal_hat: scal_hat, scal_tmp, pd, flags, volume, bind_group });
    }

    /// The scene's shapes, vents and burners (plus the frame's extra burners and its
    /// brushes) for the shader, and the parameters for a substep of `dt`. Returns
    /// whether there are brushes.
    fn upload(&self, gpu: &GpuContext, scene: &Scene3d, input: &StepInput3d, dt: f32) -> bool {
        let f = self.fields.as_ref().expect("reset ran");
        let mut geom: Vec<[f32; 4]> = Vec::with_capacity(MAX_GEOM);
        let shapes = &scene.solids[..scene.solids.len().min(MAX_SHAPES)];
        for s in shapes {
            match *s {
                Shape::Sphere { centre: c, radius } => geom.extend([[c.x, c.y, c.z, radius], [0.0; 4]]),
                Shape::Cuboid(b) => geom.extend([[b.min.x, b.min.y, b.min.z, -1.0], [b.max.x, b.max.y, b.max.z, 0.0]]),
            }
        }
        let vents = &scene.vents[..scene.vents.len().min(MAX_VENTS)];
        for v in vents {
            geom.extend([v.min.extend(0.0).to_array(), v.max.extend(0.0).to_array()]);
        }
        let burners: Vec<_> = scene.burners.iter().chain(&input.burners).take(MAX_BURNERS).collect();
        for b in &burners {
            geom.extend([b.pos.extend(b.radius).to_array(), b.dir.normalize_or(Vec3::Y).extend(b.speed).to_array()]);
        }
        let brushes = &input.brushes[..input.brushes.len().min(MAX_BRUSHES)];
        for b in brushes {
            geom.extend([b.pos.extend(b.radius).to_array(), b.velocity.extend(b.strength).to_array(), b.spin.extend(0.0).to_array()]);
        }
        let p = &self.params;
        let uniforms = Uniforms {
            n: f.n.to_array(),
            h: f.h,
            gravity: scene.gravity.to_array(),
            dt,
            rise: p.rise,
            heaviness: p.heaviness,
            cooling: p.cooling,
            fade: p.fade,
            burner_temp: p.burner_temp,
            omega: p.over_relax,
            n_shapes: shapes.len() as u32,
            n_vents: vents.len() as u32,
            n_burners: burners.len() as u32,
            nu: f.nu,
            nv: f.nv,
            time: self.time,
            ignition: p.ignition,
            burn_rate: p.burn_rate,
            heat: p.heat,
            soot: p.soot,
            vorticity: p.vorticity,
            burner_fuel: p.burner_fuel,
            burner_smoke: p.burner_smoke,
            flicker: p.flicker,
            n_brushes: brushes.len() as u32,
            heat_loss: p.heat_loss,
            drag: p.drag,
            _pad: 0,
        };
        gpu.queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&uniforms));
        if !geom.is_empty() {
            gpu.queue.write_buffer(&self.geom, 0, bytemuck::cast_slice(&geom));
        }
        !brushes.is_empty()
    }

    /// Record one compute pass (timed as `phase` when timings are on).
    fn pass(&self, encoder: &mut wgpu::CommandEncoder, phase: usize, body: impl FnOnce(&mut wgpu::ComputePass<'_>, &Pipelines, &Fields)) {
        let f = self.fields.as_ref().expect("reset ran");
        let timestamp_writes = self.timing.as_ref().map(|t| wgpu::ComputePassTimestampWrites {
            query_set: &t.queries,
            beginning_of_pass_write_index: Some(2 * phase as u32),
            end_of_pass_write_index: Some(2 * phase as u32 + 1),
        });
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some(PHASES[phase]), timestamp_writes });
        pass.set_bind_group(0, &f.bind_group, &[]);
        body(&mut pass, &self.pipelines, f);
    }

    fn record_sources(&self, encoder: &mut wgpu::CommandEncoder, forces: bool, brushes: bool) {
        let confine = forces && self.params.vorticity > 0.0;
        self.pass(encoder, 0, |pass, p, f| {
            dispatch(pass, &p.classify, f.cell_groups());
            dispatch(pass, &p.boundaries, f.face_groups());
            if forces {
                dispatch(pass, &p.forces, f.face_groups());
            }
            if confine {
                dispatch(pass, &p.curl, f.cell_groups());
                dispatch(pass, &p.confine, f.face_groups());
            }
            if brushes {
                dispatch(pass, &p.brushes, f.face_groups());
            }
        });
    }

    fn record_projection(&self, encoder: &mut wgpu::CommandEncoder) {
        let iterations = self.params.pressure_iters;
        self.pass(encoder, 2, |pass, p, f| {
            dispatch(pass, &p.divergence, f.cell_groups());
            for _ in 0..iterations {
                dispatch(pass, &p.pressure_red, f.cell_groups());
                dispatch(pass, &p.pressure_black, f.cell_groups());
            }
        });
        self.pass(encoder, 3, |pass, p, f| dispatch(pass, &p.project, f.face_groups()));
    }

    fn record_substep(&self, encoder: &mut wgpu::CommandEncoder, brushes: bool) {
        let f = self.fields.as_ref().expect("reset ran");
        self.record_sources(encoder, true, brushes);
        self.pass(encoder, 1, |pass, p, f| dispatch(pass, &p.advect_velocity, f.face_groups()));
        encoder.copy_buffer_to_buffer(&f.vel_tmp, 0, &f.vel, 0, u64::from(f.vel_len()) * 4);
        self.record_projection(encoder);
        self.pass(encoder, 4, |pass, p, f| {
            dispatch(pass, &p.advect_forward, f.cell_groups());
            dispatch(pass, &p.advect_correct, f.cell_groups());
        });
        encoder.copy_buffer_to_buffer(&f.scal_tmp, 0, &f.scal, 0, u64::from(f.cells()) * 16);
    }

    /// Take the timings of an earlier step if they've arrived; queue the latest ones if
    /// the readback buffer is free.
    fn collect_timings(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some(t) = &mut self.timing else { return };
        if let TimingState::Mapping(ready) = &t.state
            && ready.load(Ordering::Acquire)
        {
            {
                let data = t.readback.slice(..).get_mapped_range().expect("mapped timings");
                let ticks: &[u64] = bytemuck::cast_slice(&data);
                self.stats.phases = PHASES
                    .iter()
                    .enumerate()
                    .map(|(k, &name)| {
                        let ns = ticks[2 * k + 1].saturating_sub(ticks[2 * k]) as f64 * f64::from(t.period_ns);
                        (name, Duration::from_nanos(ns as u64))
                    })
                    .collect();
            }
            t.readback.unmap();
            t.state = TimingState::Idle;
        }
        if matches!(t.state, TimingState::Idle) {
            let count = 2 * PHASES.len() as u32;
            encoder.resolve_query_set(&t.queries, 0..count, &t.resolve, 0);
            encoder.copy_buffer_to_buffer(&t.resolve, 0, &t.readback, 0, u64::from(count) * 8);
            t.state = TimingState::Copied;
        }
    }

    /// Classify the cells and set the prescribed faces, now (for tests and tools).
    pub fn prepare(&mut self, gpu: &GpuContext, scene: &Scene3d, input: &StepInput3d) {
        self.upload(gpu, scene, input, MAX_SUBSTEP);
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        self.record_sources(&mut encoder, false, false);
        gpu.queue.submit([encoder.finish()]);
    }

    /// Make the velocity divergence-free, now (for tests and tools; `prepare` first).
    pub fn project(&mut self, gpu: &GpuContext) {
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        self.record_projection(&mut encoder);
        gpu.queue.submit([encoder.finish()]);
    }

    /// The velocity's divergence per cell (zero outside the fluid), freshly computed.
    pub fn measure_divergence(&self, gpu: &GpuContext) -> Vec<f32> {
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        self.pass(&mut encoder, 2, |pass, p, f| dispatch(pass, &p.divergence, f.cell_groups()));
        gpu.queue.submit([encoder.finish()]);
        self.read_pressure_divergence(gpu).iter().map(|pd| pd[1]).collect()
    }

    /// The velocity buffer: u, then v, then w (see smoke.wgsl).
    pub fn read_velocity(&self, gpu: &GpuContext) -> Vec<f32> {
        bytemuck::cast_slice(&read(gpu, &self.fields.as_ref().expect("reset ran").vel)).to_vec()
    }

    /// (smoke, temperature, fuel, unused) per cell, x fastest, then y, then z.
    pub fn read_scalars(&self, gpu: &GpuContext) -> Vec<[f32; 4]> {
        bytemuck::cast_slice(&read(gpu, &self.fields.as_ref().expect("reset ran").scal)).to_vec()
    }

    /// (pressure φ, divergence) per cell.
    pub fn read_pressure_divergence(&self, gpu: &GpuContext) -> Vec<[f32; 2]> {
        bytemuck::cast_slice(&read(gpu, &self.fields.as_ref().expect("reset ran").pd)).to_vec()
    }

    /// Cell types (0 fluid, 1 solid, 3 burner, with the burner's index above bit 8;
    /// bits 4 to 6 mark fluid in a vent, open toward x, y or z).
    pub fn read_flags(&self, gpu: &GpuContext) -> Vec<u32> {
        bytemuck::cast_slice(&read(gpu, &self.fields.as_ref().expect("reset ran").flags)).to_vec()
    }

    pub fn write_velocity(&self, gpu: &GpuContext, vel: &[f32]) {
        gpu.queue.write_buffer(&self.fields.as_ref().expect("reset ran").vel, 0, bytemuck::cast_slice(vel));
    }

    pub fn write_scalars(&self, gpu: &GpuContext, scal: &[[f32; 4]]) {
        gpu.queue.write_buffer(&self.fields.as_ref().expect("reset ran").scal, 0, bytemuck::cast_slice(scal));
    }
}

fn groups(threads: UVec3) -> UVec3 {
    (threads + WORKGROUP - 1) / WORKGROUP
}

fn dispatch(pass: &mut wgpu::ComputePass<'_>, pipeline: &wgpu::ComputePipeline, groups: UVec3) {
    pass.set_pipeline(pipeline);
    pass.dispatch_workgroups(groups.x, groups.y, groups.z);
}

/// Copy a buffer back to the CPU, waiting for it.
fn read(gpu: &GpuContext, buffer: &wgpu::Buffer) -> Vec<u8> {
    let staging = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("smoke3d readback"),
        size: buffer.size(),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, buffer.size());
    gpu.queue.submit([encoder.finish()]);
    let slice = staging.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map readback buffer"));
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).expect("GPU poll");
    let data = slice.get_mapped_range().expect("mapped readback").to_vec();
    staging.unmap();
    data
}

impl Solver3d for Smoke3d {
    fn name(&self) -> &'static str {
        self.name
    }

    fn reset(&mut self, gpu: &GpuContext, scene: &Scene3d) {
        self.allocate(gpu, scene);
        self.time = 0.0;
        let cells = self.fields.as_ref().map_or(0, Fields::cells);
        self.stats = Stats {
            metrics: vec![Metric { name: "grid cells (thousands)", value: cells as f32 / 1000.0, tolerance: None }],
            ..Stats::default()
        };
    }

    fn step(&mut self, gpu: &GpuContext, encoder: &mut wgpu::CommandEncoder, dt: f32, scene: &Scene3d, input: &StepInput3d) {
        if dt <= 0.0 {
            return;
        }
        let substeps = (dt / MAX_SUBSTEP).ceil().max(1.0) as u32;
        let brushes = self.upload(gpu, scene, input, dt / substeps as f32);
        self.time += dt;
        for _ in 0..substeps {
            self.record_substep(encoder, brushes);
        }
        self.pass(encoder, 5, |pass, p, f| dispatch(pass, &p.export_volume, f.cell_groups()));
        self.collect_timings(encoder);
    }

    fn submitted(&mut self, gpu: &GpuContext) {
        let Some(t) = &mut self.timing else { return };
        if matches!(t.state, TimingState::Copied) {
            let ready = Arc::new(AtomicBool::new(false));
            let flag = ready.clone();
            t.readback.slice(..).map_async(wgpu::MapMode::Read, move |r| flag.store(r.is_ok(), Ordering::Release));
            t.state = TimingState::Mapping(ready);
        }
        let _ = gpu.device.poll(wgpu::PollType::Poll);
    }

    fn visit_params(&mut self, v: &mut dyn ParamVisitor) {
        self.params.visit(v);
    }

    /// Every value finite; smoke within 0..1, temperature and fuel not negative, and no
    /// gas faster than 50 m/s (a blow-up, not a flame).
    fn check(&self, gpu: &GpuContext) -> Vec<String> {
        if self.fields.is_none() {
            return Vec::new();
        }
        let mut problems = Vec::new();
        let scal = self.read_scalars(gpu);
        if let Some(bad) = scal.iter().find(|c| !c.iter().all(|v| v.is_finite())) {
            problems.push(format!("non-finite gas {bad:?}"));
        }
        let smoke = scal.iter().map(|c| c[0]).fold(0.0f32, f32::max);
        if smoke > 1.0 + 1e-3 {
            problems.push(format!("smoke {smoke} above 1"));
        }
        if scal.iter().any(|c| c[1] < -1e-4 || c[2] < -1e-4) {
            problems.push("negative temperature or fuel".into());
        }
        let vel = self.read_velocity(gpu);
        let fastest = vel.iter().map(|v| v.abs()).fold(0.0f32, |a, b| if b.is_finite() { a.max(b) } else { f32::INFINITY });
        if fastest > 50.0 {
            problems.push(format!("gas at {fastest} m/s"));
        }
        problems
    }

    fn volume(&self) -> Option<Volume<'_>> {
        self.fields.as_ref().map(|f| Volume { view: &f.volume, cells: f.n, fire: self.params.is_fire() })
    }

    fn stats(&self) -> &Stats {
        &self.stats
    }
}

#[cfg(test)]
#[path = "smoke_tests.rs"]
mod tests;
