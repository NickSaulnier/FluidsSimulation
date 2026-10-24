//! Fluid toy: winit event loop, frame loop, keyboard controls.
//!
//! Keys: Space pause, S single step (while paused), R reset, P next preset,
//! Tab next solver, Esc quit.

mod gpu;
mod render;
mod view;

use gpu::Gpu;
use render::Renderer;
use sim::{PRESETS, SOLVERS, StepInput, World};
use std::sync::Arc;
use std::time::{Duration, Instant};
use view::View;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

/// Longest simulated time per rendered frame. After a stall (window drag, breakpoint)
/// the sim resumes calmly instead of trying to catch up in one giant step.
const MAX_FRAME_DT: f32 = 1.0 / 30.0;
const SINGLE_STEP_DT: f32 = 1.0 / 60.0;
const VIEW_MARGIN_PX: f32 = 16.0;

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,fluid_toy=info")).init();
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
    world: World,
    paused: bool,
    step_once: bool,
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
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => state.gpu.resize(size),
            WindowEvent::RedrawRequested => state.frame(),
            WindowEvent::KeyboardInput {
                event: KeyEvent { physical_key: PhysicalKey::Code(key), state: ElementState::Pressed, repeat: false, .. },
                ..
            } => {
                if key == KeyCode::Escape {
                    event_loop.exit();
                } else {
                    state.key(key);
                }
            }
            _ => {}
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
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0));
        let window = Arc::new(event_loop.create_window(attrs)?);
        let gpu = pollster::block_on(Gpu::new(window.clone(), event_loop.owned_display_handle()))?;
        let renderer = Renderer::new(&gpu.device, gpu.config.format);
        let world = World::new(0, 0);
        log::info!("{} particles", world.initial_particles);
        Ok(Self {
            window,
            gpu,
            renderer,
            world,
            paused: false,
            step_once: false,
            last_frame: Instant::now(),
            perf: PerfCounter::new(),
        })
    }

    fn key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Space => self.paused = !self.paused,
            KeyCode::KeyS => self.step_once = true,
            KeyCode::KeyR => self.world.reset(),
            KeyCode::KeyP => self.world.load_preset((self.world.preset_index + 1) % PRESETS.len()),
            KeyCode::Tab => self.world.switch_solver((self.world.solver_index + 1) % SOLVERS.len(), true),
            _ => {}
        }
    }

    fn frame(&mut self) {
        let now = Instant::now();
        let real_dt = (now - self.last_frame).as_secs_f32();
        self.last_frame = now;

        let input = StepInput::default();
        let sim_dt = if !self.paused {
            Some(real_dt.min(MAX_FRAME_DT))
        } else if std::mem::take(&mut self.step_once) {
            Some(SINGLE_STEP_DT)
        } else {
            None
        };
        let report = sim_dt.map(|dt| self.world.advance(dt, &input));
        if let Some(summary) = self.perf.record(now, report) {
            let title = format!(
                "fluid toy | {} | {} | {} particles | {summary}{}",
                SOLVERS[self.world.solver_index].name,
                PRESETS[self.world.preset_index].name,
                self.world.solver.stats().particles,
                if self.paused { " | PAUSED" } else { "" },
            );
            self.window.set_title(&title);
            log::info!("{title}");
        }

        let frame = match self.gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) => f,
            wgpu::CurrentSurfaceTexture::Suboptimal(f) => {
                // Draw this one; reconfigure once it has been presented and dropped.
                self.render(f);
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
        self.render(frame);
    }

    fn render(&mut self, frame: wgpu::SurfaceTexture) {
        let target = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let view = View::fit(self.world.scene.size, self.window.inner_size(), VIEW_MARGIN_PX);
        let mut encoder = self.gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        self.renderer.render(&self.gpu.device, &self.gpu.queue, &mut encoder, &target, &view, &self.world);
        self.gpu.queue.submit([encoder.finish()]);
        self.window.pre_present_notify();
        self.gpu.queue.present(frame);
    }
}

/// Averages frame rate and sim cost over half-second windows.
struct PerfCounter {
    window_start: Instant,
    frames: u32,
    sim_time: Duration,
    substeps: u32,
    sim_frames: u32,
}

impl PerfCounter {
    const PERIOD: Duration = Duration::from_millis(500);

    fn new() -> Self {
        Self { window_start: Instant::now(), frames: 0, sim_time: Duration::ZERO, substeps: 0, sim_frames: 0 }
    }

    /// Returns a summary line once per period.
    fn record(&mut self, now: Instant, report: Option<sim::FrameReport>) -> Option<String> {
        self.frames += 1;
        if let Some(r) = report {
            self.sim_time += r.wall;
            self.substeps += r.substeps;
            self.sim_frames += 1;
        }
        let elapsed = now - self.window_start;
        if elapsed < Self::PERIOD {
            return None;
        }
        let fps = f64::from(self.frames) / elapsed.as_secs_f64();
        let sims = self.sim_frames.max(1);
        let summary = format!(
            "{fps:.0} fps | sim {:.2} ms/frame, {:.1} substeps",
            self.sim_time.as_secs_f64() * 1e3 / f64::from(sims),
            f64::from(self.substeps) / f64::from(sims),
        );
        *self = Self::new();
        Some(summary)
    }
}
