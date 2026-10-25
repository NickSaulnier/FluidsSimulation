//! GPU-accelerated solvers (wgpu compute shaders).
//!
//! A separate crate so that `sim` stays free of wgpu: these solvers implement the same
//! `sim::Solver` trait and reuse `sim`'s solver toolkit. Add [`SOLVERS`] to `sim::SOLVERS`
//! to make them selectable (see `World::with_solvers`).

pub mod pbf;

use anyhow::Context;
use sim::SolverEntry;

pub use pbf::GpuPbf;

pub static SOLVERS: &[SolverEntry] = &[SolverEntry { name: "PBF (GPU)", make: pbf::make }];

/// A wgpu device for compute work: either headless (`new`, for tests and the bench, and
/// the 2D GPU solver, which reads its particles back for drawing), or the renderer's own
/// (`shared`), so a solver's GPU data can be drawn without leaving the GPU (the 3D
/// solvers).
pub struct GpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter: String,
}

impl GpuContext {
    /// Use an existing device (e.g. the renderer's). wgpu handles are cheap to clone.
    pub fn shared(device: wgpu::Device, queue: wgpu::Queue, adapter: String) -> Self {
        Self { device, queue, adapter }
    }

    pub fn new() -> anyhow::Result<Self> {
        pollster::block_on(async {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    ..Default::default()
                })
                .await
                .context("no GPU adapter")?;
            let info = adapter.get_info();
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("sim-gpu"),
                    // Pass timings, where available (the 3D solvers report them).
                    required_features: adapter.features() & wgpu::Features::TIMESTAMP_QUERY,
                    ..Default::default()
                })
                .await
                .context("GPU device")?;
            Ok(Self { device, queue, adapter: format!("{} ({:?})", info.name, info.backend) })
        })
    }
}
