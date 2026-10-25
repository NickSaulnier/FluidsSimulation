//! Fluid toy: winit event loop, frame loop, egui panel, keyboard shortcuts.
//!
//! Keyboard shortcuts are listed in `ui::shortcuts` (F1 shows them in the app); Esc
//! closes the cheat sheet, or quits. (Not Tab: egui consumes it for keyboard focus
//! navigation.)
//!
//! Usage: `fluid-toy [TRACK] [--mute] [--live] [--visualizer]`. A track (mp3, ogg, flac,
//! wav) can also be dropped onto the window; the music then drives the fluid. `--live`
//! instead listens to whatever the computer is playing (Spotify, a browser...).
//! `--visualizer` starts full screen with the controls hidden (F11 toggles it), and
//! listens live unless a track is given.

mod audio;
mod director;
mod governor;
mod looks;
mod gpu;
mod input;
mod render;
mod ui;
mod view;

use audio::Music;
use gpu::Gpu;
use input::Input;
use render::Renderer;
use sim::{PRESETS, SolverEntry, World};
use std::sync::LazyLock;
use std::sync::Arc;
use std::time::{Duration, Instant};
use ui::{Perf, UiState};
use view::View;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

/// Every solver: the CPU ones from `sim`, plus the GPU ones from `sim-gpu`.
static SOLVERS: LazyLock<Vec<SolverEntry>> =
    LazyLock::new(|| sim::SOLVERS.iter().chain(sim_gpu::SOLVERS).copied().collect());

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
    music: Music,
    /// Reused buffer for the overlay field, and the range it was last drawn with.
    field_values: Vec<f32>,
    field_range: Option<(f32, f32)>,
    input: Input,
    /// Canvas rect (points) from the last UI layout.
    canvas: Option<egui::Rect>,
    /// The palette as drawn this frame: the chosen one, modulated by the music.
    palette_now: render::Palette,
    /// Base values of parameters the music is modulating, for the solver they belong to.
    param_bases: (usize, Vec<(String, f32)>),
    /// Whether the window is full screen (follows `ui.presenting`), and the cursor shown.
    fullscreen: bool,
    cursor_visible: bool,
    governor: governor::Governor,
    /// The (solver, scene) the governor is judging.
    governed: (usize, u64),
    /// The `World::scene_generation` whose gravity the UI last took on.
    gravity_synced: u64,
    last_frame: Instant,
    perf: PerfCounter,
}

impl ApplicationHandler for App {
    /// Called once on every way out (window closed, Esc): remember the UI settings.
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(state) = &self.state
            && let Err(e) = ui::settings::save(&state.ui)
        {
            log::warn!("couldn't save settings: {e:#}");
        }
    }

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
        if matches!(event, WindowEvent::CursorMoved { .. }) && state.ui.presenting {
            state.ui.reveal();
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => state.gpu.resize(size),
            WindowEvent::RedrawRequested => state.frame(),
            WindowEvent::DroppedFile(path) => state.music.load(path),
            WindowEvent::KeyboardInput {
                event: KeyEvent { physical_key: PhysicalKey::Code(key), state: ElementState::Pressed, repeat, .. },
                ..
            } if !egui_took_it && !state.egui_ctx.egui_wants_keyboard_input() => {
                if key == KeyCode::Escape {
                    // Esc closes the cheat sheet first, then leaves full screen, and quits
                    // only from the plain view.
                    if std::mem::take(&mut state.ui.cheat_sheet) || std::mem::take(&mut state.ui.presenting) {
                        return;
                    }
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
        ui::theme::apply(&egui_ctx);
        let egui_winit = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(gpu.device.limits().max_texture_dimension_2d as usize),
        );
        let egui_renderer = egui_wgpu::Renderer::new(&gpu.device, gpu.ui_format, Default::default());

        let world = World::with_solvers(&SOLVERS, 0, 0);
        log::info!("{} particles", world.initial_particles);
        let mut music = Music::default();
        let args: Vec<String> = std::env::args().skip(1).collect();
        music.muted = args.iter().any(|a| a == "--mute");
        if let Some(track) = args.iter().find(|a| !a.starts_with("--")) {
            music.load(track.into());
        }
        let track_given = args.iter().any(|a| !a.starts_with("--"));
        let visualizer = args.iter().any(|a| a == "--visualizer");
        if args.iter().any(|a| a == "--live") || (visualizer && !track_given) {
            music.set_live(true);
        }
        Ok(Self {
            window,
            gpu,
            renderer,
            egui_ctx,
            egui_winit,
            egui_renderer,
            egui_pending_free: Vec::new(),
            world,
            ui: {
                let mut ui = UiState::default();
                if let Some(saved) = ui::settings::load() {
                    saved.apply(&mut ui);
                }
                ui.presenting = visualizer;
                ui
            },
            music,
            field_values: Vec::new(),
            field_range: None,
            input: Input::default(),
            canvas: None,
            palette_now: render::Palette::default(),
            param_bases: (usize::MAX, Vec::new()),
            fullscreen: false,
            cursor_visible: true,
            governor: governor::Governor::default(),
            governed: (usize::MAX, u64::MAX),
            gravity_synced: u64::MAX, // take the first scene's gravity too
            last_frame: Instant::now(),
            perf: PerfCounter::new(),
        })
    }

    /// Presentation mode: full screen while presenting, the cursor hidden unless the
    /// overlay is showing, and the resolution governor watching for slow motion.
    fn sync_presentation(&mut self, real_dt: f32) {
        let presenting = self.ui.presenting;
        if presenting != self.fullscreen {
            self.fullscreen = presenting;
            self.window.set_fullscreen(presenting.then_some(winit::window::Fullscreen::Borderless(None)));
            self.governor.reset();
            if presenting {
                self.ui.reveal();
            }
        }
        let cursor = !presenting || self.ui.revealed() || self.egui_ctx.is_pointer_over_egui();
        if cursor != self.cursor_visible {
            self.cursor_visible = cursor;
            self.window.set_cursor_visible(cursor);
        }
        let scene = (self.world.solver_index, self.world.scene_generation);
        if scene != self.governed {
            self.governed = scene;
            self.governor.reset();
        }
        if presenting
            && !self.ui.paused
            && self.governor.update(self.perf.latest().slow_motion, real_dt)
            && let Some(message) = governor::lower_resolution(&mut self.world)
        {
            log::info!("{message}");
            self.ui.show_notice(message);
        }
    }

    /// Parameter reactions: each key is set to its base value times the reaction's
    /// multiple; the base is remembered when modulation starts and restored when it
    /// stops (or the solver changes).
    fn modulate_params(&mut self, params: &[(String, f32)]) {
        if self.param_bases.0 != self.world.solver_index {
            self.param_bases = (self.world.solver_index, Vec::new());
        }
        let bases = &mut self.param_bases.1;
        bases.retain(|(key, base)| {
            let active = params.iter().any(|(k, _)| k == key);
            if !active {
                ui::params::float_param(&mut self.world, key, Some(*base));
            }
            active
        });
        for (key, multiple) in params {
            let base = match bases.iter().find(|(k, _)| k == key) {
                Some((_, base)) => *base,
                None => match ui::params::float_param(&mut self.world, key, None) {
                    Some(base) => {
                        bases.push((key.clone(), base));
                        base
                    }
                    None => continue,
                },
            };
            ui::params::float_param(&mut self.world, key, Some(base * multiple));
        }
    }

    /// Keyboard shortcuts, as listed in `ui::shortcuts::SHORTCUTS`.
    fn key(&mut self, key: KeyCode, repeat: bool) {
        use ui::shortcuts::Action;
        let Some(action) = ui::shortcuts::lookup(key, repeat) else { return };
        let ui = &mut self.ui;
        match action {
            Action::TogglePause => ui.paused = !ui.paused,
            Action::Step => ui.step_once = true,
            Action::Reset => self.world.reset(),
            Action::NextPreset => self.world.load_preset((self.world.preset_index + 1) % PRESETS.len()),
            Action::NextSolver => {
                self.world.switch_solver((self.world.solver_index + 1) % self.world.solvers.len(), ui.keep_particles);
            }
            Action::SelectTool(tool) => ui.tool = tool,
            Action::BrushSmaller => ui.scale_brush(1.0 / ui::BRUSH_STEP),
            Action::BrushBigger => ui.scale_brush(ui::BRUSH_STEP),
            Action::TiltLeft => ui.gravity_deg = (ui.gravity_deg - TILT_STEP_DEG).max(-90.0),
            Action::TiltRight => ui.gravity_deg = (ui.gravity_deg + TILT_STEP_DEG).min(90.0),
            Action::NextOverlay => ui.overlay = ui::next_overlay(ui.overlay, self.world.solver.fields()),
            Action::ToggleParticles => ui.show_particles = !ui.show_particles,
            Action::ToggleLiquid => ui.liquid = !ui.liquid,
            Action::ToggleMusic => self.music.toggle_play(),
            Action::TogglePanel => ui.panel_open = !ui.panel_open,
            Action::ToggleCheatSheet => ui.cheat_sheet = !ui.cheat_sheet,
            Action::TogglePresentation => ui.presenting = !ui.presenting,
            Action::PreviousLook => ui.director.request(ui.director.neighbour(-1)),
            Action::NextLook => ui.director.request(ui.director.neighbour(1)),
            Action::ToggleAutoLooks => ui.director.auto = !ui.director.auto,
        }
    }

    /// The domain is fitted into the canvas: what the UI's bars and panel leave over
    /// (last laid out), or the whole window before the first layout.
    fn view(&self) -> View {
        let size = self.window.inner_size();
        let ppp = self.egui_ctx.pixels_per_point();
        let (min, area) = match self.canvas {
            Some(r) => (glam::Vec2::new(r.min.x, r.min.y) * ppp, glam::Vec2::new(r.width(), r.height()) * ppp),
            None => (glam::Vec2::ZERO, glam::Vec2::new(size.width as f32, size.height as f32)),
        };
        let margin = if self.ui.presenting { 0.0 } else { VIEW_MARGIN_PX };
        View::fit_in(self.world.scene.size, min, area, margin, size)
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
        self.music.update();
        let mut frame =
            ui::Frame { world: &mut self.world, state: &mut self.ui, music: &mut self.music, perf: &perf, field_range: self.field_range };
        let mut canvas = None;
        let mut egui_out = self.egui_ctx.run_ui(raw_input, |ui| canvas = Some(ui::layout(ui, &mut frame, cursor)));
        // Fit this frame's input and drawing to the scene area just laid out (the panel
        // may have been shown or hidden).
        self.canvas = canvas;
        let view = self.view();
        self.egui_winit.handle_platform_output(&self.window, std::mem::take(&mut egui_out.platform_output));
        self.apply_egui_textures(std::mem::take(&mut egui_out.textures_delta));

        self.sync_presentation(real_dt);
        // The director: change looks with the music (by itself only in full screen).
        let kicked = self.music.onsets().iter().any(|o| o.kind == audio::analyzer::OnsetKind::Kick);
        let active = self.ui.presenting && self.ui.director.auto;
        if let Some(look) = self.ui.director.update(active, self.music.features(), kicked, real_dt) {
            looks::apply(&looks::LOOKS[look], &mut self.world, &mut self.ui);
            log::info!("look: {}", looks::LOOKS[look].name);
        }

        // Tools and gravity, then the simulation step. A freshly loaded preset brings
        // its own gravity (the wind tunnel has none); after that the sliders own it.
        if self.gravity_synced != self.world.scene_generation {
            self.gravity_synced = self.world.scene_generation;
            self.ui.set_gravity(self.world.scene.gravity);
        }
        let mut step_input = self.input.apply(&view, &self.ui, &mut self.world, real_dt);
        // Music reactions: forces, gravity, parameters, and the colours drawn this frame.
        let reaction = self.music.react(&self.ui.reactions, self.world.scene.size, self.ui.gravity(), real_dt);
        self.world.scene.gravity = reaction.gravity;
        step_input.brushes.extend(reaction.brushes);
        step_input.emitters.extend(reaction.emitters);
        self.modulate_params(&reaction.params);
        self.palette_now = self.ui.palette.modulated(reaction.hue_shift, reaction.flash);
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
        // The selected overlay field, looked up by key in whatever the solver offers. A
        // solver without particles (pure grid) is drawn through its first field. A key
        // this solver doesn't have (picked under another solver) counts as no overlay;
        // otherwise a grid solver would draw nothing at all.
        let fields = self.world.solver.fields();
        let selected = self.ui.overlay.filter(|key| fields.iter().any(|f| f.key == *key));
        let (key, opacity) = match selected {
            Some(key) => (Some(key), self.ui.overlay_opacity),
            None if self.world.solver.particles().is_none() => (fields.first().map(|f| f.key), 1.0),
            None => (None, 0.0),
        };
        let field = key.and_then(|key| {
            let index = fields.iter().position(|f| f.key == key)?;
            let layout = self.world.solver.fill_field(index, &mut self.field_values)?;
            Some(render::FieldFrame { layout, values: &self.field_values, kind: fields[index].kind, opacity })
        });
        let options = render::DrawOptions {
            view,
            world: &self.world,
            field,
            show_particles: self.ui.show_particles,
            liquid: self.ui.liquid_style(),
            target_size: (self.gpu.config.width, self.gpu.config.height),
            palette: self.palette_now,
        };
        self.field_range = self.renderer.render(device, queue, &mut encoder, &target, options);

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
            self.world.solvers[self.world.solver_index].name,
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

