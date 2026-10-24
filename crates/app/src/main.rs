//! Fluid toy: winit event loop, frame loop, egui panel, keyboard shortcuts.
//!
//! Keys: Space pause, S single step (while paused), R reset, P next preset,
//! N next solver, Left/Right tilt gravity, Esc quit. (Not Tab: egui consumes it for
//! keyboard focus navigation.)

mod gpu;
mod input;
mod render;
mod ui;
mod view;

use gpu::Gpu;
use input::Input;
use render::Renderer;
use sim::{PRESETS, SOLVERS, World};
use std::sync::Arc;
use std::time::{Duration, Instant};
use ui::{Perf, UiState};
use view::View;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

/// Longest simulated time per rendered frame. Below 60 fps the sim runs in slow motion
/// instead of asking for more simulated time per frame, which costs more substeps, which
/// makes the frame slower still. Also keeps a stall (window drag, breakpoint) harmless.
const MAX_FRAME_DT: f32 = 1.0 / 60.0;
const SINGLE_STEP_DT: f32 = 1.0 / 60.0;
const VIEW_MARGIN_PX: f32 = 16.0;
const TILT_STEP_DEG: f32 = 5.0;

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,fluid_toy=info")).init();
    let threads = sim::default_threads();
    rayon::ThreadPoolBuilder::new().num_threads(threads).build_global()?;
    log::info!("{threads} simulation threads");
    let event_loop = EventLoop::new()?;
    // Continuous animation: redraw as fast as vsync allows.
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::default();
    event_loop.run_app(&mut app)?;
    app.error.map_or(Ok(()), Err)
}

/// `state` is `None` until `resumed` creates the window: winit only allows window
/// creation once the event loop is running.
#[derive(Default)]
struct App {
    state: Option<State>,
    error: Option<anyhow::Error>,
}

struct State {
    window: Arc<Window>,
    gpu: Gpu,
    renderer: Renderer,
    egui_ctx: egui::Context,
    egui_winit: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
    /// Textures egui asked to free; released next frame, after this frame has drawn.
    egui_pending_free: Vec<egui::TextureId>,
    world: World,
    ui: UiState,
    input: Input,
    last_frame: Instant,
    perf: PerfCounter,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        match State::new(event_loop) {
            Ok(state) => self.state = Some(state),
            Err(e) => {
                self.error = Some(e);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = self.state.as_mut() else { return };
        // egui sees every event first; what it consumes (clicks on the panel, typing
        // in a field) doesn't reach the tools or shortcuts.
        let egui_took_it = state.egui_winit.on_window_event(&state.window, &event).consumed;
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => state.gpu.resize(size),
            WindowEvent::RedrawRequested => state.frame(),
            WindowEvent::KeyboardInput {
                event: KeyEvent { physical_key: PhysicalKey::Code(key), state: ElementState::Pressed, repeat, .. },
                ..
            } if !egui_took_it && !state.egui_ctx.egui_wants_keyboard_input() => {
                if key == KeyCode::Escape {
                    event_loop.exit();
                } else {
                    state.key(key, repeat);
                }
            }
            other => state.input.handle(&other, egui_took_it),
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(state) = &self.state {
            state.window.request_redraw();
        }
    }
}

impl State {
    fn new(event_loop: &ActiveEventLoop) -> anyhow::Result<Self> {
        let attrs = Window::default_attributes()
            .with_title("fluid toy")
            .with_inner_size(winit::dpi::LogicalSize::new(1440.0, 760.0));
        let window = Arc::new(event_loop.create_window(attrs)?);
        let gpu = pollster::block_on(Gpu::new(window.clone(), event_loop.owned_display_handle()))?;
        let renderer = Renderer::new(&gpu.device, gpu.config.format);

        let egui_ctx = egui::Context::default();
        let egui_winit = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(gpu.device.limits().max_texture_dimension_2d as usize),
        );
        let egui_renderer = egui_wgpu::Renderer::new(&gpu.device, gpu.ui_format, Default::default());

        let world = World::new(0, 0);
        log::info!("{} particles", world.initial_particles);
        Ok(Self {
            window,
            gpu,
            renderer,
            egui_ctx,
            egui_winit,
            egui_renderer,
            egui_pending_free: Vec::new(),
            world,
            ui: UiState::default(),
            input: Input::default(),
            last_frame: Instant::now(),
            perf: PerfCounter::new(),
        })
    }

    fn key(&mut self, key: KeyCode, repeat: bool) {
        match key {
            KeyCode::ArrowLeft => self.ui.gravity_deg = (self.ui.gravity_deg - TILT_STEP_DEG).max(-90.0),
            KeyCode::ArrowRight => self.ui.gravity_deg = (self.ui.gravity_deg + TILT_STEP_DEG).min(90.0),
            _ if repeat => {}
            KeyCode::Space => self.ui.paused = !self.ui.paused,
            KeyCode::KeyS => self.ui.step_once = true,
            KeyCode::KeyR => self.world.reset(),
            KeyCode::KeyP => self.world.load_preset((self.world.preset_index + 1) % PRESETS.len()),
            KeyCode::KeyN => {
                self.world.switch_solver((self.world.solver_index + 1) % SOLVERS.len(), self.ui.keep_particles);
            }
            _ => {}
        }
    }

    /// The domain is fitted into the window minus the panel on the right.
    fn view(&self) -> View {
        let size = self.window.inner_size();
        let panel_px = (ui::PANEL_WIDTH * self.egui_ctx.pixels_per_point()) as u32;
        let scene_area = winit::dpi::PhysicalSize::new(size.width.saturating_sub(panel_px).max(1), size.height);
        View::fit(self.world.scene.size, scene_area, VIEW_MARGIN_PX).with_window(size)
    }

    fn frame(&mut self) {
        let now = Instant::now();
        let real_dt = (now - self.last_frame).as_secs_f32();
        self.last_frame = now;
        let view = self.view();

        // UI first, so changes made in the panel apply to this frame's step.
        let raw_input = self.egui_winit.take_egui_input(&self.window);
        let cursor = self.input.cursor().map(|pos| ui::Cursor { pos, px_per_metre: view.px_per_metre });
        let perf = self.perf.latest();
        let (world, ui_state) = (&mut self.world, &mut self.ui);
        let mut egui_out = self.egui_ctx.run_ui(raw_input, |ui| ui::panel(ui, world, ui_state, &perf, cursor));
        self.egui_winit.handle_platform_output(&self.window, std::mem::take(&mut egui_out.platform_output));
        self.apply_egui_textures(std::mem::take(&mut egui_out.textures_delta));

        // Tools and gravity, then the simulation step.
        self.world.scene.gravity = self.ui.gravity();
        let step_input = self.input.apply(&view, &self.ui, &mut self.world, real_dt);
        let wanted = real_dt * self.ui.speed;
        let sim_dt = if !self.ui.paused {
            Some(wanted.min(MAX_FRAME_DT))
        } else if std::mem::take(&mut self.ui.step_once) {
            Some(SINGLE_STEP_DT)
        } else {
            None
        };
        let report = sim_dt.map(|dt| self.world.advance(dt, &step_input));
        self.perf.record(now, report, if self.ui.paused { 0.0 } else { wanted });
        self.update_title();

        let frame = match self.gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) => f,
            wgpu::CurrentSurfaceTexture::Suboptimal(f) => {
                // Draw this one; reconfigure once it has been presented and dropped.
                self.render(f, &view, egui_out);
                self.gpu.reconfigure();
                return;
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => return,
            wgpu::CurrentSurfaceTexture::Outdated => return self.gpu.reconfigure(),
            wgpu::CurrentSurfaceTexture::Lost => {
                if let Err(e) = self.gpu.recreate_surface() {
                    log::error!("failed to recreate surface: {e:#}");
                }
                return;
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                log::error!("validation error acquiring the surface texture");
                return;
            }
        };
        self.render(frame, &view, egui_out);
    }

    fn render(&mut self, frame: wgpu::SurfaceTexture, view: &View, egui_out: egui::FullOutput) {
        let (device, queue) = (&self.gpu.device, &self.gpu.queue);
        let target = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        self.renderer.render(device, queue, &mut encoder, &target, view, &self.world);

        // egui on top, in its own pass that keeps the scene underneath.
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.gpu.config.width, self.gpu.config.height],
            pixels_per_point: egui_out.pixels_per_point,
        };
        let paint_jobs = self.egui_ctx.tessellate(egui_out.shapes, egui_out.pixels_per_point);
        let ui_target = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.gpu.ui_format),
            ..Default::default()
        });
        let egui_commands = self.egui_renderer.update_buffers(device, queue, &mut encoder, &paint_jobs, &screen);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &ui_target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            // egui-wgpu wants a `RenderPass<'static>`; `forget_lifetime` trades the
            // compile-time borrow of the encoder for a runtime check.
            self.egui_renderer.render(&mut pass.forget_lifetime(), &paint_jobs, &screen);
        }
        queue.submit(egui_commands.into_iter().chain([encoder.finish()]));

        self.window.pre_present_notify();
        queue.present(frame);
    }

    /// egui's texture changes must be applied every frame, even ones we end up not
    /// drawing (epaint asserts on dropped deltas).
    fn apply_egui_textures(&mut self, mut delta: egui::TexturesDelta) {
        for id in self.egui_pending_free.drain(..) {
            self.egui_renderer.free_texture(&id);
        }
        for (id, images) in delta.set.drain() {
            for image in &images {
                self.egui_renderer.update_texture(&self.gpu.device, &self.gpu.queue, id, image);
            }
        }
        self.egui_pending_free.extend(delta.free.drain());
    }

    fn update_title(&self) {
        let title = format!(
            "fluid toy | {} | {} | {:.0} fps",
            SOLVERS[self.world.solver_index].name,
            PRESETS[self.world.preset_index].name,
            self.perf.latest().fps
        );
        if self.window.title() != title {
            self.window.set_title(&title);
        }
    }
}

/// Averages frame rate and sim cost over half-second windows.
struct PerfCounter {
    window_start: Instant,
    frames: u32,
    sim_time: Duration,
    substeps: u32,
    sim_frames: u32,
    wanted: f32,
    simulated: f32,
    latest: Perf,
}

impl PerfCounter {
    const PERIOD: Duration = Duration::from_millis(500);

    fn new() -> Self {
        Self {
            window_start: Instant::now(),
            frames: 0,
            sim_time: Duration::ZERO,
            substeps: 0,
            sim_frames: 0,
            wanted: 0.0,
            simulated: 0.0,
            latest: Perf::default(),
        }
    }

    /// `wanted` is the simulated time real time asked for this frame (0 when paused).
    fn record(&mut self, now: Instant, report: Option<sim::FrameReport>, wanted: f32) {
        self.frames += 1;
        self.wanted += wanted;
        if let Some(r) = report {
            self.sim_time += r.wall;
            self.substeps += r.substeps;
            self.sim_frames += 1;
            self.simulated += r.dt * r.substeps as f32;
        }
        let elapsed = now - self.window_start;
        if elapsed < Self::PERIOD {
            return;
        }
        let sims = self.sim_frames.max(1) as f32;
        let latest = Perf {
            fps: self.frames as f32 / elapsed.as_secs_f32(),
            sim_ms: self.sim_time.as_secs_f32() * 1e3 / sims,
            substeps: self.substeps as f32 / sims,
            slow_motion: if self.wanted > 0.0 { (1.0 - self.simulated / self.wanted).max(0.0) } else { 0.0 },
        };
        *self = Self::new();
        self.latest = latest;
    }

    fn latest(&self) -> Perf {
        self.latest
    }
}
