//! GPU-accelerated solvers (wgpu compute shaders).
//!
//! A separate crate so that `sim` stays free of wgpu: these solvers implement the same
//! `sim::Solver` trait and reuse `sim`'s solver toolkit. Add [`SOLVERS`] to `sim::SOLVERS`
//! to make them selectable (see `World::with_solvers`).

pub mod pbf;
pub mod stable;

use anyhow::Context;
use sim::SolverEntry;
use std::sync::OnceLock;

pub use pbf::GpuPbf;
pub use stable::GpuStable;

pub static SOLVERS: &[SolverEntry] = &[
    SolverEntry { name: "PBF (GPU)", make: pbf::make },
    SolverEntry { name: "Stable fluids (GPU)", make: stable::make },
    SolverEntry { name: "Smoke & fire (GPU)", make: stable::make_fire },
];

/// A wgpu device for compute work: either headless (`new`, for tests and the bench, and
/// the 2D GPU solver, which reads its particles back for drawing), or the renderer's own
/// (`shared`), so a solver's GPU data can be drawn without leaving the GPU (the 3D
/// solvers).
#[derive(Clone)]
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

    /// The process's headless device: opened the first time, and shared after (the
    /// handles are clones). Opening devices from several threads at once deadlocks (wgpu
    /// 30 on Windows: the app's tests hung that way, many opening one each in parallel),
    /// and one device serves every solver and test as well as many would.
    pub fn new() -> anyhow::Result<Self> {
        static SHARED: OnceLock<Result<GpuContext, String>> = OnceLock::new();
        SHARED.get_or_init(|| Self::open().map_err(|e| format!("{e:#}"))).clone().map_err(anyhow::Error::msg)
    }

    fn open() -> anyhow::Result<Self> {
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
