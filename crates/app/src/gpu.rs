//! Instance, adapter, device, queue and surface: everything tied to the window's lifetime.

use anyhow::Context;
use std::sync::Arc;
use winit::dpi::PhysicalSize;
use winit::event_loop::OwnedDisplayHandle;
use winit::window::Window;

pub struct Gpu {
    instance: wgpu::Instance,
    window: Arc<Window>,
    /// `'static` because the surface holds its own `Arc<Window>` clone, so it can never
    /// outlive the window it draws into.
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
    /// Non-sRGB view format of the swapchain, for egui (which blends in gamma space).
    pub ui_format: wgpu::TextureFormat,
    /// The adapter's name, for logs and the 3D solvers' context.
    pub adapter: String,
}

impl Gpu {
    pub async fn new(window: Arc<Window>, display: OwnedDisplayHandle) -> anyhow::Result<Self> {
        // `_from_env` lets WGPU_BACKEND=vulkan|dx12 etc. override the backend for debugging.
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(display)));
        let surface = instance.create_surface(window.clone())?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await?;
        let info = adapter.get_info();
        log::info!("adapter: {} ({:?})", info.name, info.backend);

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("device"),
                // Pass timings, where available (the 3D solvers report them).
                required_features: adapter.features() & wgpu::Features::TIMESTAMP_QUERY,
                ..Default::default()
            })
            .await?;

        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .context("surface is not supported by the adapter")?;
        // Prefer an sRGB format so shader colours are linear and blending is correct.
        let caps = surface.get_capabilities(&adapter);
        config.format = caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(config.format);
        config.present_mode = wgpu::PresentMode::AutoVsync;
        let ui_format = config.format.remove_srgb_suffix();
        if ui_format != config.format {
            config.view_formats.push(ui_format);
        }
        surface.configure(&device, &config);

        Ok(Self { instance, window, surface, device, queue, config, ui_format, adapter: info.name })
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        // Minimising reports 0x0, which is an invalid surface size; keep the old config.
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
    }

    pub fn reconfigure(&self) {
        self.surface.configure(&self.device, &self.config);
    }

    /// The surface itself is gone (rare: GPU driver reset, display change). Rebuild it.
    pub fn recreate_surface(&mut self) -> anyhow::Result<()> {
        self.surface = self.instance.create_surface(self.window.clone())?;
        self.reconfigure();
        Ok(())
    }
}
