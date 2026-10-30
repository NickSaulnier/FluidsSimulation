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
//!
//! `fluid-toy --import <file or folder>` converts classic MilkDrop presets (`.milk`)
//! into looks in the user's looks folder and exits; dropping them on the window does the
//! same (see `looks::milk`).

mod audio;
mod director;
mod expr;
mod governor;
mod looks;
mod output;
mod react3d;
mod soak;
#[cfg(test)]
mod test_alloc;
mod stability;
mod gpu;
mod input;
mod render;
mod render3d;
mod ui;
mod view;

use audio::Music;
use gpu::Gpu;
use input::Input;
use render::Renderer;
use render3d::{OrbitCamera, Renderer3d};
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
/// A window shape this far from the scene's (as a ratio) rebuilds the scene, once it has
/// held for `REFIT_AFTER`; nearer, the canvas just crops a little more.
const REFIT_TOLERANCE: f32 = 0.08;
const REFIT_AFTER: Duration = Duration::from_millis(500);
/// In the full-screen visualizer a 3D look is framed this much closer.
const PRESENTING_ZOOM: f32 = 0.8;
const TILT_STEP_DEG: f32 = 5.0;
/// How much the up and down keys change gravity's strength, m/s².
const STRENGTH_STEP: f32 = 0.5;
/// A look's thumbnail is taken this long after it starts.
const THUMBNAIL_AFTER_S: f32 = 8.0;
/// How often the fluid's flow is drawn when only the fractal's bend wants it.
const BEND_FLOW_HZ: f32 = 30.0;
/// How often the director, mixing in random looks, makes one instead of its own pick,
/// and how many tries it takes at one that suits the music's energy.
const RANDOM_SHARE: f32 = 0.25;
const RANDOM_TRIES: usize = 8;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--import") {
        let path = args.get(i + 1).ok_or_else(|| anyhow::anyhow!("--import needs a .milk file or a folder of them"))?;
        return import_presets(std::path::Path::new(path));
    }
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
    /// The 3D world, while a 3D solver is picked; `world` waits underneath meanwhile.
    world3d: Option<sim3d::World3d>,
    camera: OrbitCamera,
    renderer3d: Renderer3d,
    /// The scene texture, bloom and composite, and the GPU time of each stage.
    post: render::post::PostProcess,
    timer: Option<render::timer::GpuTimer>,
    /// The feedback layer, and the 2D flow's velocity grid (reused).
    feedback: render::feedback::FeedbackLayer,
    velocity: Vec<sim::Vec2>,
    /// This frame's time for the feedback (0 while paused, which holds the picture).
    frame_dt: f32,
    /// Waves and shapes, and this frame's waveform (reused).
    waves: render::waves::WavesLayer,
    waveform: Vec<audio::analyzer::Frame>,
    /// The look's equations, running, and this frame's results (the feedback, waves and
    /// the rest as drawn); the look as applied, to see when its file changes it.
    look_runtime: looks::runtime::Runtime,
    look_frame: looks::runtime::Frame,
    look_pixel: String,
    applied_look: Option<looks::Look>,
    /// While a look without fluid shows: the 2D world's (solver, scene generation) then.
    /// The fluid comes back when either changes (or a 3D solver is picked).
    no_fluid: Option<(usize, u64)>,
    /// The composite's kaleidoscope turn, hue drift and grain seed.
    composite_clock: render::composite::CompositeClock,
    /// The pattern and seed of the blend into the current look (see `director`), chosen
    /// at random for each.
    blend_pattern: render::composite::BlendPattern,
    blend_seed: f32,
    rng: utils::rng::SplitMix64,
    /// Thumbnails being taken, and the look whose one to take this frame (its key).
    thumbnailer: render::thumbnail::Thumbnailer,
    thumbnail_key: Option<String>,
    ui: UiState,
    music: Music,
    /// What the computer reports playing (see `audio::media`); its cover, drawn into the
    /// scene, as a texture for the interface and as a palette; the song last announced
    /// (title, artist, album) and the track last seen.
    media: audio::media::Media,
    art: render::art::ArtLayer,
    fractal: render::fractal::FractalLayer,
    /// The fluid bending the fractal, and the time since its flow was last drawn (with
    /// no feedback wanting it every frame, the flow is drawn at most `BEND_FLOW_HZ`).
    bend: render::bend::BendLayer,
    bend_flow_age: f32,
    /// Watching the frames while a fractal is on, and how far its detail has been
    /// lowered to keep up (`render::fractal::quality`; back to full with each look).
    fractal_governor: governor::Governor,
    fractal_quality: u32,
    /// The fractal's own movement with the music, and the Morph reaction's last kick.
    fractal_motion: render::fractal::Motion,
    fractal_morph: f32,
    cover: Option<(std::sync::Arc<Vec<u8>>, egui::TextureHandle, render::Palette)>,
    announced: Option<(String, String, String)>,
    last_track: Option<String>,
    /// Frames read back for screenshots and recordings, a screenshot asked for, the
    /// recording under way, and word from the threads that save them.
    grabber: render::grab::Grabber<Grab>,
    screenshot_wanted: bool,
    /// A look just imported, to show once the looks are read again.
    pending_look: Option<String>,
    recorder: Option<output::Recorder>,
    saved: (std::sync::mpsc::Sender<Saved>, std::sync::mpsc::Receiver<Saved>),
    /// The colours drawn from: the look's, or the cover's, eased between.
    palette_ease: render::palette::PaletteEase,
    palette_base: render::Palette,
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
    /// How the window was last set up for the visualizer (presenting, and the screen
    /// settings), and the cursor shown.
    presented: (bool, ui::Screen),
    /// When the monitors were last listed.
    monitors_listed: Option<Instant>,
    /// Whether Shift is held (for Shift+ shortcuts).
    mods: ui::shortcuts::Mods,
    /// A new window shape waiting to rebuild the scene (the domain, since when), and
    /// whether the scene has been fitted to the window yet.
    refit: Option<(glam::Vec2, Instant)>,
    fitted: bool,
    /// A window just moved to another display: centre it again, at the size it settled
    /// at (a display with other scaling resizes it), after this many frames.
    recentre: Option<(winit::monitor::MonitorHandle, u32)>,
    cursor_visible: bool,
    governor: governor::Governor,
    /// The (solver, scene) the governor is judging (3D solvers count down from the
    /// top, so they never match a 2D one).
    governed: (usize, u64),
    /// The `World::scene_generation` whose gravity the UI last took on, and the 3D scene's
    /// (none: a new 3D world, whose gravity is still to take on).
    gravity_synced: u64,
    gravity_synced_3d: Option<usize>,
    last_frame: Instant,
    perf: PerfCounter,
}

impl ApplicationHandler for App {
    /// Called once on every way out (window closed, Esc): remember the UI settings.
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        let Some(state) = &mut self.state else { return };
        if let Err(e) = ui::settings::save(&state.ui) {
            log::warn!("couldn't save settings: {e:#}");
        }
        // Let a recording under way finish its file.
        if let Some(recorder) = state.recorder.take() {
            let (tx, rx) = std::sync::mpsc::channel();
            recorder.stop(tx);
            match rx.recv_timeout(Duration::from_secs(10)) {
                Ok(Ok(path)) => log::info!("saved {}", path.display()),
                Ok(Err(e)) => log::warn!("couldn't save the recording: {e}"),
                Err(_) => log::warn!("the recording didn't finish in time"),
            }
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
            WindowEvent::DroppedFile(path) => {
                // MilkDrop presets (or a folder of them) are imported; anything else is music.
                if path.is_dir() || path.extension().is_some_and(|x| x.eq_ignore_ascii_case("milk")) {
                    state.import_milk(&path);
                } else {
                    state.music.load(path);
                }
            }
            WindowEvent::ModifiersChanged(m) => {
                use ui::shortcuts::Mods;
                let m = m.state();
                state.mods = if m.control_key() { Mods::Ctrl } else if m.shift_key() { Mods::Shift } else { Mods::None };
            }
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

    /// Draw again at once, or, under a frame-rate cap, once the next frame is due.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(state) = &self.state else { return };
        let cap = state.ui.screen.fps_cap;
        let due = state.last_frame + Duration::from_secs_f64(if cap > 0 { 1.0 / f64::from(cap) } else { 0.0 });
        if cap > 0 && Instant::now() < due {
            event_loop.set_control_flow(ControlFlow::WaitUntil(due));
        } else {
            event_loop.set_control_flow(ControlFlow::Poll);
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
        // The scenes draw into the HDR scene texture; the composite writes the surface.
        let renderer = Renderer::new(&gpu.device, render::post::SCENE_FORMAT);
        let renderer3d = Renderer3d::new(&gpu.device, render::post::SCENE_FORMAT);
        let post = render::post::PostProcess::new(&gpu.device, gpu.config.format);
        let timer = render::timer::GpuTimer::new(&gpu.device, &gpu.queue);
        let feedback = render::feedback::FeedbackLayer::new(&gpu.device);
        let waves = render::waves::WavesLayer::new(&gpu.device);
        let art = render::art::ArtLayer::new(&gpu.device);
        let fractal = render::fractal::FractalLayer::new(&gpu.device);
        let bend = render::bend::BendLayer::new(&gpu.device);

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
            world3d: None,
            camera: OrbitCamera::framing(glam::Vec3::ONE),
            renderer3d,
            post,
            timer,
            feedback,
            velocity: Vec::new(),
            frame_dt: 0.0,
            waves,
            waveform: Vec::new(),
            look_runtime: looks::runtime::Runtime::default(),
            look_frame: looks::runtime::Frame::plain(&render::feedback::FeedbackSettings::OFF, &render::waves::WaveSettings::OFF, &render::composite::CompositeSettings::OFF),
            composite_clock: render::composite::CompositeClock::default(),
            blend_pattern: render::composite::BlendPattern::Dissolve,
            blend_seed: 0.0,
            rng: utils::rng::SplitMix64::new(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64)),
            thumbnailer: render::thumbnail::Thumbnailer::default(),
            thumbnail_key: None,
            look_pixel: String::new(),
            applied_look: None,
            no_fluid: None,
            ui: {
                let mut ui = UiState::default();
                if let Some(saved) = ui::settings::load() {
                    saved.apply(&mut ui);
                }
                ui.presenting = visualizer;
                if let Some(folder) = looks::Library::user_folder() {
                    let marks = std::mem::take(&mut ui.looks.marks);
                    ui.looks = looks::Library::with_folder(folder);
                    ui.looks.marks = marks;
                }
                ui.thumbnails = ui::thumbnails::Thumbnails::new(ui::thumbnails::Thumbnails::user_folder());
                ui
            },
            music,
            media: audio::media::Media::start(),
            art,
            fractal,
            bend,
            bend_flow_age: f32::INFINITY,
            fractal_governor: governor::Governor::default(),
            fractal_quality: 0,
            fractal_motion: render::fractal::Motion::default(),
            fractal_morph: 0.0,
            cover: None,
            announced: None,
            last_track: None,
            palette_ease: render::palette::PaletteEase::default(),
            grabber: render::grab::Grabber::default(),
            screenshot_wanted: false,
            pending_look: None,
            recorder: None,
            saved: std::sync::mpsc::channel(),
            palette_base: render::Palette::default(),
            field_values: Vec::new(),
            field_range: None,
            input: Input::default(),
            canvas: None,
            palette_now: render::Palette::default(),
            param_bases: (usize::MAX, Vec::new()),
            presented: (false, ui::Screen::default()),
            monitors_listed: None,
            mods: ui::shortcuts::Mods::None,
            refit: None,
            fitted: false,
            recentre: None,
            cursor_visible: true,
            governor: governor::Governor::default(),
            governed: (usize::MAX, u64::MAX),
            gravity_synced: u64::MAX, // take the first scene's gravity too
            gravity_synced_3d: None,
            last_frame: Instant::now(),
            perf: PerfCounter::new(),
        })
    }

    /// Presentation mode: full screen while presenting, the cursor hidden unless the
    /// overlay is showing, and the resolution governor watching for slow motion.
    fn sync_presentation(&mut self, real_dt: f32) {
        let presenting = self.ui.presenting;
        if self.monitors_listed.is_none_or(|t| t.elapsed() > Duration::from_secs(1)) {
            self.monitors_listed = Some(Instant::now());
            let primary = self.window.primary_monitor();
            let raw = self
                .window
                .available_monitors()
                .filter_map(|m| {
                    let (p, s) = (m.position(), m.size());
                    Some((m.name()?, [p.x, p.y, s.width as i32, s.height as i32], primary.as_ref() == Some(&m)))
                })
                .collect();
            self.ui.monitors = ui::describe_displays(raw);
            self.ui.window_display = self.window.current_monitor().and_then(|m| m.name());
        }
        let wanted = (presenting, self.ui.screen.clone());
        if wanted != self.presented {
            let screen = &self.ui.screen;
            let full = presenting && !screen.windowed;
            let moved = screen.monitor != self.presented.1.monitor;
            let monitor = screen.monitor.as_ref().and_then(|name| self.window.available_monitors().find(|m| m.name().as_ref() == Some(name)));
            if full {
                // Already full screen elsewhere: leave it first, or the window stays put.
                if moved && self.presented.0 && !self.presented.1.windowed {
                    self.window.set_fullscreen(None);
                }
                self.window.set_fullscreen(Some(winit::window::Fullscreen::Borderless(monitor)));
            } else {
                self.window.set_fullscreen(None);
                // A display chosen (or remembered): the window moves there at once.
                if let Some(m) = monitor.filter(|_| moved) {
                    self.centre_on(&m, true);
                    self.recentre = Some((m, 3));
                }
            }
            let on_top = presenting && screen.windowed && screen.on_top;
            self.window.set_window_level(if on_top { winit::window::WindowLevel::AlwaysOnTop } else { winit::window::WindowLevel::Normal });
            if presenting != self.presented.0 {
                self.governor.reset();
                (self.fractal_governor, self.fractal_quality) = (governor::Governor::default(), 0);
                if presenting {
                    self.ui.reveal();
                }
            }
            self.presented = wanted;
        }
        if let Some((m, frames)) = self.recentre.take() {
            if frames > 0 {
                self.recentre = Some((m, frames - 1));
            } else {
                self.centre_on(&m, false);
            }
        }
        let cursor = !presenting || self.ui.revealed() || self.egui_ctx.is_pointer_over_egui();
        if cursor != self.cursor_visible {
            self.cursor_visible = cursor;
            self.window.set_cursor_visible(cursor);
        }
        let (scene, slowness) = match &self.world3d {
            Some(w) => ((usize::MAX - w.solver_index, w.generation), governor::frame_slowness(real_dt)),
            None => ((self.world.solver_index, self.world.scene_generation), self.perf.latest().slow_motion),
        };
        if scene != self.governed {
            self.governed = scene;
            self.governor.reset();
        }
        if presenting && !self.ui.paused && self.governor.update(slowness, real_dt) {
            let lowered = match &mut self.world3d {
                Some(w) => governor::lower_resolution(w),
                None => governor::lower_resolution(&mut self.world),
            };
            if let Some(message) = lowered {
                log::info!("{message}");
                self.ui.show_notice(message);
            }
        }
        // A fractal costs frames, not simulated time: judged by the frames themselves.
        let fractal = self.look_frame.waves.fractal.enabled && self.fractal_quality < render::fractal::QUALITY_STEPS;
        if !fractal {
            self.fractal_governor.reset();
        } else if presenting && !self.ui.paused && self.fractal_governor.update(governor::frame_slowness_capped(real_dt, self.ui.screen.fps_cap), real_dt) {
            self.fractal_quality += 1;
            let message = "Lowered the fractal's detail to keep up".to_string();
            log::info!("{message} (step {})", self.fractal_quality);
            self.ui.show_notice(message);
        }
    }

    /// Import the MilkDrop presets at `path` (a file, or a folder) into the user's looks,
    /// and show the look if it was one.
    fn import_milk(&mut self, path: &std::path::Path) {
        let Some(folder) = self.ui.looks.folder.clone() else {
            return self.ui.show_notice("there's no looks folder to import into".into());
        };
        let presets = looks::milk::presets_in(path);
        let (mut imported, mut partly, mut failed) = (Vec::new(), 0, Vec::new());
        for preset in &presets {
            match looks::milk::import_file(preset, &folder) {
                Ok((file, look)) => {
                    log::info!("imported {} as {}{}", preset.display(), file.display(), if look.left_out.is_empty() { String::new() } else { format!(" (left out: {})", look.left_out.join("; ")) });
                    partly += usize::from(!look.left_out.is_empty());
                    imported.push(look);
                }
                Err(e) => failed.push(format!("{}: {e}", preset.display())),
            }
        }
        for f in &failed {
            log::warn!("couldn't import {f}");
        }
        let notice = match (imported.as_slice(), failed.len()) {
            ([], 0) => "no MilkDrop presets there".to_string(),
            ([], _) => format!("couldn't import: {}", failed[0]),
            ([one], 0) if one.left_out.is_empty() => format!("imported {}", one.name),
            ([one], 0) => format!("imported {} (left out: {})", one.name, one.left_out.join("; ")),
            (many, n) => {
                let failed = if n > 0 { format!(", {n} couldn't be") } else { String::new() };
                format!("imported {} presets ({partly} with parts left out: see the top of each file){failed}", many.len())
            }
        };
        self.ui.show_notice(notice);
        if let [one] = imported.as_slice() {
            self.pending_look = Some(one.name.clone());
        }
    }

    /// Move the window to the middle of `monitor` (shrunk to fit if `fit` and it's too
    /// big).
    fn centre_on(&self, monitor: &winit::monitor::MonitorHandle, fit: bool) {
        let (at, room) = (monitor.position(), monitor.size());
        self.window.set_maximized(false);
        let outer = self.window.outer_size();
        let size = if fit { winit::dpi::PhysicalSize::new(outer.width.min(room.width * 9 / 10), outer.height.min(room.height * 9 / 10)) } else { outer };
        if size != outer {
            let _ = self.window.request_inner_size(size);
        }
        let x = at.x + (room.width.saturating_sub(size.width) / 2) as i32;
        let y = at.y + (room.height.saturating_sub(size.height) / 2) as i32;
        self.window.set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
    }

    /// Read the user's looks again if their files changed. If the look on screen is one
    /// of them, show the change: at once if only its picture, colours, reactions or code
    /// changed, or by applying it afresh if its fluid did.
    fn reload_looks(&mut self, now: Instant) {
        let before: Vec<String> = self.ui.looks.looks.iter().map(|l| l.name.clone()).collect();
        if !self.ui.looks.poll(now) {
            return;
        }
        self.ui.director.reindex(&before, &self.ui.looks.looks);
        if let Some(i) = self.pending_look.take().and_then(|name| self.ui.looks.looks.iter().position(|l| l.name == name)) {
            self.ui.director.request(i);
            return;
        }
        if let Some(error) = self.ui.looks.errors.first() {
            let more = self.ui.looks.errors.len() - 1;
            let others = if more > 0 { format!(" (and {more} more: see the Music tab)") } else { String::new() };
            self.ui.show_notice(format!("look {error}{others}"));
        }
        let Some(index) = self.ui.director.current else { return };
        let look = self.ui.looks.looks[index].clone();
        if self.applied_look.as_ref() == Some(&look) {
            return;
        }
        if self.applied_look.as_ref().is_some_and(|old| old.same_scene(&look)) {
            looks::apply_picture(&look, &mut self.ui);
            self.start_code(&look);
            self.applied_look = Some(look);
        } else {
            self.apply_look(index, true);
        }
        if self.ui.looks.errors.is_empty() {
            self.ui.show_notice(format!("reloaded {}", self.ui.looks.looks[index].name));
        }
    }

    /// Save the settings as they stand as a look named `name`: the fluid (solver, scene as
    /// last loaded, every parameter at its own value, not as the music moves it), the
    /// colours, feedback, waves and reactions, and the current look's equations and
    /// energy. It becomes the look on screen.
    fn save_look(&mut self, name: String) {
        let current = self.ui.director.current.and_then(|i| self.ui.looks.looks.get(i)).cloned();
        let bases = self.param_bases.1.clone();
        let (solver, scene, params, liquid, orbit) = match &mut self.world3d {
            Some(w) => {
                let params = looks::current_params(w, &bases);
                (w.solver.name().to_string(), sim3d::PRESETS[w.preset_index].name.to_string(), params, false, self.ui.orbit)
            }
            None => {
                let w = &mut self.world;
                let params = looks::current_params(w, &bases);
                (w.solvers[w.solver_index].name.to_string(), sim::PRESETS[w.preset_index].name.to_string(), params, self.ui.liquid, 0.0)
            }
        };
        let look = looks::Look {
            name: name.clone(),
            author: current.as_ref().map_or_else(String::new, |l| l.author.clone()),
            tags: current.as_ref().map_or_else(Vec::new, |l| l.tags.clone()),
            energy: current.as_ref().map_or(looks::Energy::Any, |l| l.energy),
            solver,
            scene,
            palette: self.ui.palette,
            liquid,
            orbit,
            feedback: self.ui.feedback,
            waves: self.ui.waves,
            composite: self.ui.composite,
            params,
            reactions: self.ui.reactions.clone(),
            code: current.map(|l| l.code).unwrap_or_default(),
            source: looks::Source::Builtin("unsaved"),
        };
        let before: Vec<String> = self.ui.looks.looks.iter().map(|l| l.name.clone()).collect();
        match self.ui.looks.save(&look) {
            Ok(path) => {
                self.ui.director.reindex(&before, &self.ui.looks.looks);
                if let Some(i) = self.ui.looks.looks.iter().position(|l| l.name == name) {
                    self.ui.director.current = Some(i);
                    self.applied_look = Some(self.ui.looks.looks[i].clone());
                }
                log::info!("saved look {name} to {}", path.display());
                self.ui.show_notice(format!("saved {name} as {}", path.display()));
            }
            Err(e) => self.ui.show_notice(format!("can't save the look: {e}")),
        }
    }

    /// Run `code`, edited in the editor, in place of the look on screen's (and keep it
    /// with the look, for saving).
    fn edit_code(&mut self, code: looks::Code, rerun_init: bool) {
        if let Some(look) = self.ui.director.current.and_then(|i| self.ui.looks.looks.get_mut(i)) {
            look.code = code.clone();
        }
        if let Some(look) = &mut self.applied_look {
            look.code = code.clone();
        }
        self.look_runtime.replace(&code, rerun_init);
        self.look_pixel = code.pixel;
    }

    /// Keep up with the song: listening live, what the computer reports (its cover, and
    /// a new song announced once it plays); playing a track, the track. A new song gets
    /// a title card (if wanted) and a new look from the director.
    fn follow_song(&mut self) {
        let events = self.media.poll();
        if self.music.is_live() {
            let song = self.media.song.clone();
            let art = song.as_ref().and_then(|s| s.art.clone());
            if self.cover.as_ref().map(|c| &c.0) != art.as_ref().map(|a| &a.rgba) {
                self.set_cover(art.as_ref());
            }
            if events.is_empty() && self.ui.song.as_ref().map(|s| s.title.as_str()) == song.as_ref().map(|s| s.title.as_str()) {
                return;
            }
            self.ui.song = song.as_ref().map(|s| ui::title_card::Song {
                title: s.title.clone(),
                artist: s.artist.clone(),
                album: s.album.clone(),
                playing: s.playing,
                cover: self.cover.as_ref().map(|c| c.1.clone()),
            });
            if let Some(s) = song.filter(|s| s.playing) {
                let id = (s.title, s.artist, s.album);
                if self.announced.as_ref() != Some(&id) {
                    self.announced = Some(id);
                    self.announce();
                }
            }
        } else {
            if self.cover.is_some() {
                self.set_cover(None);
            }
            let track = self.music.track.as_ref().map(|t| t.name.clone());
            if self.ui.song.as_ref().map(|s| &s.title) != track.as_ref() || self.ui.song.as_ref().is_some_and(|s| !s.artist.is_empty()) {
                self.ui.song = track.clone().map(|title| ui::title_card::Song { title, artist: String::new(), album: String::new(), playing: true, cover: None });
            }
            if track.is_some() && track != self.last_track {
                self.last_track = track;
                self.announce();
            }
        }
    }

    /// Frames read back since last frame: screenshots saved, recordings fed; and word
    /// from the threads saving them.
    fn collect_grabs(&mut self) {
        for (tag, image) in self.grabber.poll(&self.gpu.device) {
            match tag {
                Grab::Screenshot => output::save_screenshot(image, &self.output_name(), self.saved.0.clone()),
                Grab::Video(t) => {
                    if let Some(r) = &mut self.recorder {
                        r.push(t, image);
                    }
                }
            }
        }
        while let Ok(result) = self.saved.1.try_recv() {
            match result {
                Ok(path) => {
                    log::info!("saved {}", path.display());
                    self.ui.show_notice(format!("saved {}", path.display()));
                }
                Err(e) => {
                    log::warn!("couldn't save: {e}");
                    self.ui.show_notice(format!("couldn't save: {e}"));
                }
            }
        }
    }

    /// Start recording a video, or stop the one under way.
    fn toggle_recording(&mut self) {
        if let Some(recorder) = self.recorder.take() {
            if recorder.dropped > 0 {
                log::info!("recording dropped {} frames", recorder.dropped);
            }
            self.ui.show_notice(format!("finishing {}", recorder.path.display()));
            recorder.stop(self.saved.0.clone());
            self.ui.recording = None;
            return;
        }
        let window = [self.gpu.config.width, self.gpu.config.height];
        // A track playing from a file goes in as the sound; live audio can't.
        let track = self.music.track.as_ref().filter(|_| !self.music.is_live() && self.music.is_playing()).map(|t| (t.path.clone(), self.music.position()));
        match output::Recorder::start(window, &self.output_name(), track.as_ref().map(|(p, at)| (p.as_path(), *at))) {
            Ok(recorder) => {
                log::info!("recording to {}", recorder.path.display());
                self.ui.recording = Some(Instant::now());
                self.recorder = Some(recorder);
            }
            Err(e) => self.ui.show_notice(e),
        }
    }

    /// What screenshots and recordings are named after: the look on screen.
    fn output_name(&self) -> String {
        self.ui.director.current.and_then(|i| self.ui.looks.looks.get(i)).map_or_else(|| "fluid-toy".to_string(), |l| l.name.clone())
    }

    /// Show `art` as the song's cover (none: no cover).
    fn set_cover(&mut self, art: Option<&audio::media::Art>) {
        self.art.set(&self.gpu.device, &self.gpu.queue, art);
        self.cover = art.map(|a| {
            let image = egui::ColorImage::from_rgba_unmultiplied([a.size[0] as usize, a.size[1] as usize], &a.rgba);
            let texture = self.egui_ctx.load_texture("cover", image, egui::TextureOptions::LINEAR);
            (a.rgba.clone(), texture, render::Palette::from_picture(&a.rgba))
        });
        if let Some(song) = &mut self.ui.song {
            song.cover = self.cover.as_ref().map(|c| c.1.clone());
        }
    }

    /// A new song: its title card, and a look for it.
    fn announce(&mut self) {
        self.ui.director.new_song();
        if self.ui.title_cards {
            self.ui.title_card = Some(Instant::now());
        }
    }

    /// Start `look`'s equations, and its composite's clock afresh (the last look's hue
    /// drift and kaleidoscope turn would carry over).
    fn start_code(&mut self, look: &looks::Look) {
        self.look_runtime = looks::runtime::Runtime::new(&look.code);
        self.look_pixel = look.code.pixel.clone();
        self.composite_clock = render::composite::CompositeClock::default();
    }

    /// Switch to look `index`, moving between the 2D and 3D worlds if it needs to.
    /// `fresh`: start the feedback afresh (for a cut, or a look reloaded from its file);
    /// otherwise the new look's warp starts from the old picture, as a blend should.
    fn apply_look(&mut self, index: usize, fresh: bool) {
        let Some(look) = self.ui.looks.looks.get(index).cloned() else { return };
        // A new look's fractal starts at full detail.
        (self.fractal_governor, self.fractal_quality) = (governor::Governor::default(), 0);
        // Put back any parameters the last look's reactions were modulating.
        self.modulate_params(&[], &[]);
        if fresh {
            self.feedback.pause();
        } else {
            let patterns = render::composite::BlendPattern::ALL;
            self.blend_pattern = patterns[(self.rng.next_u64() % patterns.len() as u64) as usize];
            self.blend_seed = 100.0 * self.rng.next_f32();
        }
        self.start_code(&look);
        if !look.has_fluid() {
            // Only the picture: the 2D world waits underneath, neither stepped nor drawn.
            if self.world3d.is_some() {
                self.choose_solver(ui::SolverChoice::TwoD(self.world.solver_index));
            }
            looks::apply_picture(&look, &mut self.ui);
            self.no_fluid = Some((self.world.solver_index, self.world.scene_generation));
            log::info!("look: {} (no fluid)", look.name);
            self.applied_look = Some(look);
            return;
        }
        self.no_fluid = None;
        let unknown = match look.solver_3d() {
            Some(solver) => {
                self.choose_solver(ui::SolverChoice::ThreeD(solver));
                let mut unknown = Vec::new();
                if let Some(world) = &mut self.world3d {
                    unknown = looks::apply_3d(&look, world, &mut self.ui);
                    // Frame the new scene, keeping the camera's heading; closer in full
                    // screen, where there are no controls around it.
                    let framing = OrbitCamera::framing(world.scene.size);
                    let distance = if self.ui.presenting { PRESENTING_ZOOM * framing.distance } else { framing.distance };
                    self.camera = OrbitCamera { yaw: self.camera.yaw, distance, ..framing };
                }
                unknown
            }
            None => {
                if self.world3d.is_some() {
                    self.choose_solver(ui::SolverChoice::TwoD(self.world.solver_index));
                }
                looks::apply(&look, &mut self.world, &mut self.ui)
            }
        };
        if !unknown.is_empty() {
            self.ui.show_notice(format!("{}: the solver has no {}", look.name, unknown.join(", ")));
        }
        log::info!("look: {}", look.name);
        self.applied_look = Some(look);
    }

    /// Parameter reactions: each key is set to its base value times the reaction's
    /// multiple; the base is remembered when modulation starts and restored when it
    /// stops (or the solver changes).
    /// `multiples` from the reactions, `values` from the look's equations (which replace
    /// the base value before the reactions' multiple applies).
    fn modulate_params(&mut self, multiples: &[(String, f32)], values: &[(String, f32)]) {
        // 3D solvers count down from the top, so they never match a 2D one.
        let (solver, world): (usize, &mut dyn ui::params::Params) = match &mut self.world3d {
            Some(w) => (usize::MAX - w.solver_index, w),
            None => (self.world.solver_index, &mut self.world),
        };
        if self.param_bases.0 != solver {
            self.param_bases = (solver, Vec::new());
        }
        let bases = &mut self.param_bases.1;
        bases.retain(|(key, base)| {
            let active = multiples.iter().chain(values).any(|(k, _)| k == key);
            if !active {
                ui::params::float_param(&mut *world, key, Some(*base));
            }
            active
        });
        let mut keys: Vec<&String> = multiples.iter().chain(values).map(|(k, _)| k).collect();
        keys.dedup();
        for key in keys {
            let base = match bases.iter().find(|(k, _)| k == key) {
                Some((_, base)) => *base,
                None => match ui::params::float_param(&mut *world, key, None) {
                    Some(base) => {
                        bases.push((key.clone(), base));
                        base
                    }
                    None => continue,
                },
            };
            let value = values.iter().find(|(k, _)| k == key).map_or(base, |(_, v)| *v);
            let multiple = multiples.iter().find(|(k, _)| k == key).map_or(1.0, |(_, m)| *m);
            ui::params::float_param(&mut *world, key, Some(value * multiple));
        }
    }

    /// Keyboard shortcuts, as listed in `ui::shortcuts::SHORTCUTS`.
    fn key(&mut self, key: KeyCode, repeat: bool) {
        use ui::shortcuts::Action;
        let Some(action) = ui::shortcuts::lookup(key, repeat, self.ui.presenting, self.mods) else { return };
        let ui = &mut self.ui;
        match action {
            Action::TogglePause => ui.paused = !ui.paused,
            Action::Step => ui.step_once = true,
            Action::Reset => match &mut self.world3d {
                Some(w) => w.reset(),
                None => self.world.reset(),
            },
            Action::NextPreset => match &mut self.world3d {
                Some(w) => w.load_preset((w.preset_index + 1) % sim3d::PRESETS.len()),
                None => self.world.load_preset((self.world.preset_index + 1) % PRESETS.len()),
            },
            Action::NextSolver => {
                // Through the 2D solvers, then the 3D ones, and round again.
                let n2 = self.world.solvers.len();
                let index = self.world3d.as_ref().map_or(self.world.solver_index, |w| n2 + w.solver_index);
                let next = (index + 1) % (n2 + sim3d::SOLVERS.len());
                let choice = if next < n2 { ui::SolverChoice::TwoD(next) } else { ui::SolverChoice::ThreeD(next - n2) };
                self.choose_solver(choice);
            }
            Action::SelectTool(tool) => ui.tool = tool,
            Action::UndoWall => {
                if self.world3d.is_none() && !self.input.undo(&mut self.world) {
                    ui.show_notice("nothing to undo".into());
                }
            }
            Action::BrushSmaller => ui.scale_brush(1.0 / ui::BRUSH_STEP),
            Action::BrushBigger => ui.scale_brush(ui::BRUSH_STEP),
            Action::TiltLeft => ui.turn_gravity(-TILT_STEP_DEG),
            Action::TiltRight => ui.turn_gravity(TILT_STEP_DEG),
            Action::Stronger => ui.gravity_mag = (ui.gravity_mag + STRENGTH_STEP).min(ui::compass::MAX_GRAVITY),
            Action::Weaker => ui.gravity_mag = (ui.gravity_mag - STRENGTH_STEP).max(0.0),
            Action::NextOverlay => ui.overlay = ui::next_overlay(ui.overlay, self.world.solver.fields()),
            Action::ToggleParticles => ui.show_particles = !ui.show_particles,
            Action::ToggleLiquid => ui.liquid = !ui.liquid,
            Action::ToggleMusic => self.music.toggle_play(),
            Action::TogglePanel => ui.panel_open = !ui.panel_open,
            Action::ToggleCheatSheet => ui.cheat_sheet = !ui.cheat_sheet,
            Action::TogglePresentation => ui.presenting = !ui.presenting,
            Action::PreviousLook => ui.director.request(ui.director.neighbour(-1, ui.looks.looks.len())),
            Action::NextLook => ui.director.request(ui.director.neighbour(1, ui.looks.looks.len())),
            Action::ToggleAutoLooks | Action::LockLook => {
                ui.director.auto = !ui.director.auto;
                ui.show_notice(if ui.director.auto { "looks change with the music".into() } else { "this look stays".into() });
            }
            Action::BackLook => {
                if !ui.director.back() {
                    ui.show_notice("no look before this one".into());
                }
            }
            Action::NextLookFade | Action::HardCut => {
                let next = ui.director.pick(&ui.looks.looks, &ui.look_weights());
                if action == Action::HardCut { ui.director.cut(next) } else { ui.director.request(next) }
            }
            Action::RateUp | Action::RateDown => {
                if let Some(name) = ui.director.current.and_then(|i| ui.looks.looks.get(i)).map(|l| l.name.clone()) {
                    let stars = ui.looks.rate(&name, if action == Action::RateUp { 1 } else { -1 });
                    let shown: String = (1..=5).map(|k| if k <= stars { '\u{2605}' } else { '\u{2606}' }).collect();
                    let note = if stars == 1 { " (not picked by itself)" } else { "" };
                    ui.show_notice(format!("{name}: {shown}{note}"));
                }
            }
            Action::ShowTitle => match &ui.song {
                Some(_) => ui.title_card = Some(Instant::now()),
                None if self.music.is_live() => ui.show_notice("nothing reports what's playing".into()),
                None => ui.show_notice("no track loaded".into()),
            },
            Action::ToggleBrowser => ui.browser_open = !ui.browser_open,
            Action::Screenshot => self.screenshot_wanted = true,
            Action::ToggleRecording => self.toggle_recording(),
            Action::NextDisplay => ui.next_display(),
            Action::ToggleEditor => ui.editor_open = !ui.editor_open,
            Action::RandomLook | Action::VaryLook => self.show_random(action == Action::VaryLook),
        }
    }

    /// Make a random look (or, `vary`, a variation on the one on screen) and blend to it.
    fn show_random(&mut self, vary: bool) {
        let index = self.make_random(vary, None);
        let name = self.ui.looks.looks[index].name.clone();
        self.ui.director.request(index);
        ui::anim::kick(&self.egui_ctx, ui::widgets::dice_id());
        self.ui.show_notice(format!("{name}: Save as keeps it, Backspace goes back"));
    }

    /// Make a random look (see `looks::random`), or `vary` the one on screen, and add it
    /// to the library; `energy`: one that suits it, if a few tries find one. Returns its
    /// index.
    fn make_random(&mut self, vary: bool, energy: Option<looks::Energy>) -> usize {
        let name = self.ui.looks.next_random_name();
        let current = self.ui.director.current.and_then(|i| self.ui.looks.looks.get(i)).cloned();
        let mut make = || match (vary, &current) {
            (true, Some(c)) => looks::random::vary(c, &mut self.rng, &name),
            _ => looks::random::random(&mut self.rng, &name),
        };
        let mut look = make();
        for _ in 0..RANDOM_TRIES {
            if energy.is_none_or(|e| e == looks::Energy::Any || look.energy == e || look.energy == looks::Energy::Any) {
                break;
            }
            look = make();
        }
        log::info!("random look {name}:
{}", looks::write(&look));
        let before: Vec<String> = self.ui.looks.looks.iter().map(|l| l.name.clone()).collect();
        let index = self.ui.looks.add_random(look);
        self.ui.director.reindex(&before, &self.ui.looks.looks);
        index
    }

    /// The canvas (what the UI's bars and panel leave over, last laid out; the whole
    /// window before the first layout), in physical pixels: its corner and size.
    fn canvas_px(&self) -> (glam::Vec2, glam::Vec2) {
        let size = self.window.inner_size();
        let ppp = self.egui_ctx.pixels_per_point();
        match self.canvas {
            Some(r) => (glam::Vec2::new(r.min.x, r.min.y) * ppp, glam::Vec2::new(r.width(), r.height()) * ppp),
            None => (glam::Vec2::ZERO, glam::Vec2::new(size.width as f32, size.height as f32)),
        }
    }

    /// The inside of the scene's walls covers the canvas edge to edge: the walls fall
    /// just outside it, unseen, and the fluid reaches the edges.
    fn view(&self) -> View {
        let (min, area) = self.canvas_px();
        let inside = self.world.scene.solid.interior();
        View::cover(inside.min, inside.size(), min, area, self.window.inner_size())
    }

    /// Fit the scene to the window's shape: the domain takes the shape of the canvas with
    /// the side panel closed (the panel crops the sides rather than reshaping it). A new
    /// shape that holds for `REFIT_AFTER` rebuilds the scene; the first one does at once.
    fn refit_domain(&mut self, now: Instant) {
        if self.world3d.is_some() || self.canvas.is_none() {
            return;
        }
        let (_, area) = self.canvas_px();
        let width = self.window.inner_size().width as f32;
        if area.y < 50.0 || width < 50.0 {
            return;
        }
        let target = sim::domain_for(width / area.y);
        let current = self.world.domain;
        let off = (target.x / target.y) / (current.x / current.y);
        if (off - 1.0).abs() < REFIT_TOLERANCE {
            self.refit = None;
            self.fitted = true;
            return;
        }
        let due = match self.refit {
            Some((t, since)) if (t - target).length() < 0.02 => now.duration_since(since) >= REFIT_AFTER,
            _ => {
                self.refit = Some((target, now));
                false
            }
        };
        if due || !self.fitted {
            self.fitted = true;
            self.refit = None;
            self.world.domain = target;
            self.world.load_preset(self.world.preset_index);
            // Keep the gravity as set, and a look without fluid without it.
            self.gravity_synced = self.world.scene_generation;
            if let Some(at) = &mut self.no_fluid {
                *at = (self.world.solver_index, self.world.scene_generation);
            }
            log::info!("scene fitted to the window: {:.2} x {:.2} m", target.x, target.y);
        }
    }

    fn frame(&mut self) {
        let now = Instant::now();
        let real_dt = (now - self.last_frame).as_secs_f32();
        self.last_frame = now;
        self.frame_dt = if self.ui.paused { 0.0 } else { real_dt.min(0.1) };
        if let Some((key, pixels)) = self.thumbnailer.poll(&self.gpu.device) {
            self.ui.thumbnails.store(&self.egui_ctx, &key, &pixels);
        }
        let view = self.view();

        // UI first, so changes made in the panel apply to this frame's step.
        let raw_input = self.egui_winit.take_egui_input(&self.window);
        let cursor = self.input.cursor().map(|pos| ui::Cursor { pos, px_per_metre: view.px_per_metre });
        let perf = self.perf.latest();
        let render_times = self.timer.as_ref().map(|t| t.latest().to_vec()).unwrap_or_default();
        self.music.latency_ms = self.ui.audio_latency_ms;
        self.music.update();
        self.music.waveform(&mut self.waveform);
        self.follow_song();
        self.collect_grabs();
        let cover = self.cover.as_ref().map(|c| c.2).filter(|_| self.ui.album_colours && self.music.is_live());
        self.palette_base = self.palette_ease.follow(cover.as_ref().unwrap_or(&self.ui.palette), real_dt);
        let mut frame = ui::Frame {
            world: &mut self.world,
            world3d: self.world3d.as_mut(),
            state: &mut self.ui,
            music: &mut self.music,
            perf: &perf,
            field_range: self.field_range,
            render_times: &render_times,
        };
        let mut canvas = None;
        let mut egui_out = self.egui_ctx.run_ui(raw_input, |ui| canvas = Some(ui::layout(ui, &mut frame, cursor)));
        // Fit this frame's input and drawing to the scene area just laid out (the panel
        // may have been shown or hidden).
        self.canvas = canvas;
        let view = self.view();
        self.egui_winit.handle_platform_output(&self.window, std::mem::take(&mut egui_out.platform_output));
        self.apply_egui_textures(std::mem::take(&mut egui_out.textures_delta));
        if let Some(choice) = self.ui.solver_request.take() {
            self.choose_solver(choice);
        }
        if let Some((code, rerun_init)) = self.ui.code_request.take() {
            self.edit_code(code, rerun_init);
        }
        if let Some(name) = self.ui.save_look_request.take() {
            self.save_look(name);
        }
        if let Some(vary) = self.ui.random_request.take() {
            self.show_random(vary);
        }

        self.sync_presentation(real_dt);
        self.refit_domain(now);
        self.reload_looks(now);
        // The director: change looks with the music (by itself only in full screen).
        let kicked = self.music.onsets().iter().any(|o| o.kind == audio::analyzer::OnsetKind::Kick);
        let weights = self.ui.look_weights();
        if let Some(change) = self.ui.director.update(&self.ui.looks.looks, &weights, self.ui.presenting, self.music.features(), kicked, real_dt) {
            let mut look = change.look;
            // Now and then, a new random look in place of the director's own pick.
            if self.ui.director.random_looks && self.ui.director.picked_itself() && self.rng.next_f32() < RANDOM_SHARE {
                let energy = self.ui.looks.looks.get(look).map(|l| l.energy);
                look = self.make_random(false, energy);
                self.ui.director.substitute(look);
            }
            self.apply_look(look, change.cut);
        }
        if self.no_fluid.is_some_and(|at| self.world3d.is_some() || at != (self.world.solver_index, self.world.scene_generation)) {
            self.no_fluid = None;
        }
        // The look's equations, from its settings as they stand and the music.
        let sound = looks::runtime::Sound { features: self.music.features(), onsets: self.music.onsets(), beats: self.music.beat_count() };
        let aspect = self.gpu.config.width as f32 / self.gpu.config.height.max(1) as f32;
        // The fractal's own movement first, so the equations can still override it.
        let mut waves = self.ui.waves;
        let music = self.music.features().map_or(render::fractal::Music::QUIET, |f| render::fractal::Music {
            bass: f.milk.bass_att,
            level: f.level,
            sections: f.sections.changes,
            morph: self.fractal_morph,
        });
        waves.fractal = self.fractal_motion.apply(&waves.fractal, &music, self.frame_dt);
        self.look_frame = self.look_runtime.step(&self.ui.feedback, &waves, &self.ui.composite, sound, self.frame_dt, aspect);
        self.composite_clock.advance(&self.look_frame.composite, self.frame_dt);
        self.waves.update(&self.waveform, &self.look_frame.waves, self.frame_dt);
        self.art.update(&self.look_frame.waves.art, self.frame_dt);
        self.fractal.update(&self.look_frame.waves.fractal, self.frame_dt);
        let (drag, wheel) = self.input.take_orbit();
        if self.world3d.is_some() {
            self.step_3d(drag, wheel, real_dt);
            self.perf.record(now, None, 0.0);
        } else {
            self.step_2d(now, real_dt, &view);
        }
        self.update_title();
        self.thumbnail_key = self.thumbnail_wanted();

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

    /// The 2D frame: tools, music reactions, then the simulation step.
    fn step_2d(&mut self, now: Instant, real_dt: f32, view: &View) {
        if self.no_fluid.is_some() {
            // No fluid: the reactions only colour the picture.
            let reaction = self.music.react(&self.ui.reactions, self.world.scene.size, self.ui.gravity(), self.ui.presenting, real_dt, self.look_frame.intensity);
            self.palette_now = self.palette_base.modulated(reaction.hue_shift, reaction.flash);
            self.fractal_morph = reaction.morph;
            self.look_frame.params.clear();
            self.perf.record(now, None, 0.0);
            return;
        }
        // Tools and gravity, then the simulation step. A freshly loaded preset brings
        // its own gravity (the wind tunnel has none); after that the sliders own it.
        if self.gravity_synced != self.world.scene_generation {
            self.gravity_synced = self.world.scene_generation;
            self.ui.set_gravity(self.world.scene.gravity);
        }
        let mut step_input = self.input.apply(view, &self.ui, &mut self.world, real_dt);
        // Music reactions: forces, gravity, parameters, and the colours drawn this frame.
        let gravity = tilt(self.ui.gravity(), self.look_frame.gravity_tilt) * self.look_frame.gravity_scale;
        let intensity = self.look_frame.intensity;
        let mut reaction = self.music.react(&self.ui.reactions, self.world.scene.size, gravity, self.ui.presenting, real_dt, intensity);
        if !stability::room_for_fluid(&self.world) {
            reaction.emitters.clear(); // jets pause while particles are over budget
        }
        self.world.scene.gravity = reaction.gravity;
        step_input.brushes.extend(reaction.brushes);
        render::waves::wave_brushes(&self.look_frame.waves, &self.waveform, self.waves.peak(), self.world.scene.size, &mut step_input.brushes);
        step_input.emitters.extend(reaction.emitters);
        step_input.drops.extend(reaction.drops);
        let values = std::mem::take(&mut self.look_frame.params);
        self.modulate_params(&reaction.params, &values);
        self.palette_now = self.palette_base.modulated(reaction.hue_shift, reaction.flash);
        self.fractal_morph = reaction.morph;
        let wanted = real_dt * self.ui.speed;
        let sim_dt = self.sim_dt(real_dt);
        let report = sim_dt.map(|dt| self.world.advance(dt, &step_input));
        self.perf.record(now, report, if self.ui.paused { 0.0 } else { wanted });
    }

    /// The 3D frame: the camera (the mouse, and a slow orbit in the visualizer), the
    /// mouse's and the music's pushes (see `react3d`), then the volume steps.
    fn step_3d(&mut self, drag: Option<glam::Vec2>, wheel: f32, real_dt: f32) {
        let sim_dt = self.sim_dt(real_dt);
        let viewport = self.viewport_3d();
        let Some((size, scene_gravity, preset)) = self.world3d.as_ref().map(|w| (w.scene.size, w.base_gravity, w.preset_index)) else { return };
        // A new 3D scene brings its own gravity; after that the dial owns it.
        if self.gravity_synced_3d != Some(preset) {
            self.gravity_synced_3d = Some(preset);
            self.ui.set_gravity_3d(scene_gravity);
        }
        let base_gravity = self.ui.gravity_3d() * self.look_frame.gravity_scale;
        if let Some(d) = drag {
            self.camera.orbit(d.x, d.y);
        }
        if wheel != 0.0 {
            self.camera.zoom(wheel, OrbitCamera::framing(size).distance);
        }
        if self.ui.presenting && !self.ui.paused {
            // Circle the scene, a little faster while the music is loud.
            let level = self.music.features().map_or(0.0, |f| f.level);
            self.camera.yaw += self.ui.orbit * (1.0 + level) * real_dt;
        }
        let mut input = self.input.apply_3d(&self.camera, viewport, size, &self.ui, real_dt);
        let mut reaction = self.music.react(
            &self.ui.reactions,
            react3d::front_view(size),
            tilt(react3d::front_gravity(base_gravity, self.camera.right()), self.look_frame.gravity_tilt),
            self.ui.presenting,
            real_dt,
            self.look_frame.intensity,
        );
        let front = react3d::front_view(size);
        render::waves::wave_brushes(&self.look_frame.waves, &self.waveform, self.waves.peak(), front, &mut reaction.brushes);
        let (music, gravity) = react3d::lift(&reaction, size, self.camera.right(), base_gravity);
        input.brushes.extend(music.brushes);
        input.burners.extend(music.burners);
        let values = std::mem::take(&mut self.look_frame.params);
        self.modulate_params(&reaction.params, &values);
        self.palette_now = self.palette_base.modulated(reaction.hue_shift, reaction.flash);
        self.fractal_morph = reaction.morph;
        let Some(world) = &mut self.world3d else { return };
        world.scene.gravity = gravity;
        if let Some(dt) = sim_dt {
            world.advance(dt, &input);
        }
    }

    /// Simulated time for this frame: real time at the chosen speed (capped), one fixed
    /// step if a single step was asked for while paused, or none.
    fn sim_dt(&mut self, real_dt: f32) -> Option<f32> {
        if !self.ui.paused {
            Some((real_dt * self.ui.speed).min(MAX_FRAME_DT))
        } else if std::mem::take(&mut self.ui.step_once) {
            Some(SINGLE_STEP_DT)
        } else {
            None
        }
    }

    /// Act on a solver picked in the UI. Into 3D creates the 3D world on the renderer's
    /// device; back to 2D drops it, and the 2D world carries on where it was.
    fn choose_solver(&mut self, choice: ui::SolverChoice) {
        match choice {
            ui::SolverChoice::TwoD(k) => {
                self.world3d = None;
                self.gravity_synced_3d = None;
                if k != self.world.solver_index {
                    self.world.switch_solver(k, self.ui.keep_particles);
                }
            }
            ui::SolverChoice::ThreeD(k) => match &mut self.world3d {
                Some(world) => world.switch_solver(k),
                None => {
                    let gpu = sim3d::GpuContext::shared(self.gpu.device.clone(), self.gpu.queue.clone(), self.gpu.adapter.clone());
                    let world = sim3d::World3d::new(gpu, k, 0);
                    self.camera = OrbitCamera::framing(world.scene.size);
                    self.world3d = Some(world);
                }
            },
        }
    }

    /// Where the 3D view draws: the canvas in physical pixels (x, y, width, height),
    /// kept inside the window.
    fn viewport_3d(&self) -> [f32; 4] {
        let (w, h) = (self.gpu.config.width as f32, self.gpu.config.height as f32);
        let ppp = self.egui_ctx.pixels_per_point();
        let Some(r) = self.canvas else { return [0.0, 0.0, w, h] };
        let (x, y) = ((r.min.x * ppp).clamp(0.0, w), (r.min.y * ppp).clamp(0.0, h));
        [x, y, (r.width() * ppp).min(w - x).max(0.0), (r.height() * ppp).min(h - y).max(0.0)]
    }

    /// The frame: the scene into the HDR scene texture (at the render scale), the
    /// feedback over it, then the glow and the composite onto the surface, then the
    /// interface.
    fn render(&mut self, frame: wgpu::SurfaceTexture, view: &View, egui_out: egui::FullOutput) {
        use render::timer::Stage;
        let target = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let surface = [self.gpu.config.width, self.gpu.config.height];
        let scale = render::post::render_scale(self.ui.render_scale, surface);
        let scene_size = render::post::scene_size(surface, scale);
        let scene = self.post.scene_target(&self.gpu.device, scene_size);
        let mark = |timer: &Option<render::timer::GpuTimer>, encoder: &mut wgpu::CommandEncoder, stage| {
            if let Some(t) = timer {
                t.mark(encoder, stage);
            }
        };
        mark(&self.timer, &mut encoder, Stage::Scene);
        let feedback = self.look_frame.feedback;
        let waves = self.look_frame.waves;
        // The fluid bends the fractal through the flow, which the feedback draws too.
        let bending = waves.fractal.enabled && waves.fractal.bend > 0.0;
        // Feedback that the fluid's flow carries washes anything standing still into the
        // fluid within a second (the new frame is under 1 % of each): over a fluid, then,
        // the fractal goes over the feedback's picture, crisp (the bend moves it instead).
        let fluid = self.no_fluid.is_none();
        let fractal_after = waves.fractal.enabled && feedback.enabled && feedback.flow > 0.0 && fluid;
        let flow = self.feedback.flow_target(&self.gpu.device, scene_size);
        if let Some(world) = &self.world3d {
            let viewport = self.viewport_3d().map(|v| v * scale);
            let (device, queue) = (&self.gpu.device, &self.gpu.queue);
            let volume = world.solver.volume();
            // Feedback would smear the box's lines into streaks: leave them out.
            let lines = !feedback.enabled;
            self.renderer3d.render(
                device,
                queue,
                &mut encoder,
                &scene,
                &flow,
                scene_size,
                viewport,
                &self.camera,
                &world.scene,
                volume,
                &self.palette_now,
                lines,
            );
            self.field_range = None;
        } else if self.no_fluid.is_some() {
            render::feedback::clear(&mut encoder, &scene);
            if feedback.enabled {
                self.feedback.draw_flow_2d(&self.gpu.device, &self.gpu.queue, &mut encoder, scene_size, view, None);
            }
            self.field_range = None;
        } else {
            self.render_2d(&mut encoder, &scene, view, scene_size, scale);
            // Getting the fluid's velocity costs (a readback, or a splat of the
            // particles): for the bend alone, which eases it anyway, not every frame.
            self.bend_flow_age += self.frame_dt;
            if feedback.enabled || (bending && self.bend_flow_age >= 1.0 / BEND_FLOW_HZ) {
                self.bend_flow_age = 0.0;
                self.draw_flow_2d(&mut encoder, view, scene_size);
            }
        }
        if waves.fractal.enabled {
            let (device, queue) = (&self.gpu.device, &self.gpu.queue);
            if bending && fluid {
                self.bend.update(device, queue, &mut encoder, &flow, scene_size, waves.fractal.bend, self.frame_dt);
            } else {
                self.bend.reset();
            }
            if !fractal_after {
                self.draw_fractal(&mut encoder, &scene, scale);
            }
        } else {
            self.bend.reset();
        }
        if waves.art.enabled && self.art.has_cover() {
            let viewport = self.viewport_3d().map(|v| v * scale);
            let bass = self.music.features().map_or(1.0, |f| f.milk.bass_att);
            self.art.draw(&self.gpu.queue, &mut encoder, &scene, viewport, &waves.art, bass);
        }
        if waves.any() {
            let viewport = self.viewport_3d().map(|v| v * scale);
            let features = self.music.features();
            let sound = render::waves::Sound {
                waveform: &self.waveform,
                spectrum: features.map_or(&[][..], |f| &f.spectrum_smooth[..]),
                bass: features.map_or(1.0, |f| f.milk.bass_att),
            };
            let (device, queue) = (&self.gpu.device, &self.gpu.queue);
            self.waves.draw(device, queue, &mut encoder, &scene, viewport, &waves, &sound, &self.palette_now);
        }
        mark(&self.timer, &mut encoder, Stage::Feedback);
        let picture = if feedback.enabled {
            let (device, queue) = (&self.gpu.device, &self.gpu.queue);
            let (shared, pixel) = (&self.look_frame.shared, &self.look_pixel);
            let picture = self.feedback.run(device, queue, &mut encoder, &scene, scene_size, &feedback, shared, pixel, self.frame_dt);
            let walls = self.world3d.is_none() && fluid;
            if walls || fractal_after {
                // The fractal and the walls over the feedback's picture, not into it, so
                // they stay sharp: on a copy (the scene texture is free again by now).
                encoder.copy_texture_to_texture(picture.texture().as_image_copy(), scene.texture().as_image_copy(), scene.texture().size());
                if fractal_after {
                    self.draw_fractal(&mut encoder, &scene, scale);
                }
                if walls {
                    self.renderer.draw_walls(&mut encoder, &scene);
                }
                scene.clone()
            } else {
                picture
            }
        } else {
            self.feedback.pause();
            scene.clone()
        };
        mark(&self.timer, &mut encoder, Stage::Glow);
        let blend = render::composite::Blend { amount: self.ui.director.blend(), pattern: self.blend_pattern, seed: self.blend_seed };
        let composite = render::composite::Composite { settings: self.look_frame.composite, clock: self.composite_clock, palette: self.palette_now, blend };
        let (device, queue) = (&self.gpu.device, &self.gpu.queue);
        self.post.run(device, queue, &mut encoder, &picture, &target, self.ui.glow, feedback.echo(), &composite, self.timer.as_ref());
        if self.ui.director.capturing() {
            // A change is on its way: keep this frame as the still to blend from.
            self.post.capture(device, &mut encoder, &picture, surface);
            self.ui.director.captured();
        }
        // Screenshots and recordings: the composite again, into a frame to read back.
        if std::mem::take(&mut self.screenshot_wanted) {
            match self.grabber.begin(device, surface, self.gpu.config.format, Grab::Screenshot) {
                Some(view) => {
                    self.post.composite_again(device, &mut encoder, &picture, &view);
                    self.grabber.end(&mut encoder);
                }
                None => self.ui.show_notice("busy: try the screenshot again".into()),
            }
        }
        if let Some(recorder) = self.recorder.as_ref().filter(|r| r.wants_frame()) {
            let tag = Grab::Video(recorder.time());
            if let Some(view) = self.grabber.begin(device, recorder.size, self.gpu.config.format, tag) {
                self.post.composite_again(device, &mut encoder, &picture, &view);
                self.grabber.end(&mut encoder);
            }
        }
        if let Some(key) = self.thumbnail_key.take() {
            let thumbnail = self.thumbnailer.target(device, surface, self.gpu.config.format);
            self.post.composite_again(device, &mut encoder, &picture, &thumbnail);
            let crop = self.canvas_fraction();
            self.thumbnailer.copy(device, &mut encoder, key, crop);
        }
        mark(&self.timer, &mut encoder, Stage::Interface);
        let (device, queue) = (&self.gpu.device, &self.gpu.queue);

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
        if let Some(timer) = &mut self.timer {
            timer.mark(&mut encoder, Stage::End);
            timer.finish(&mut encoder);
        }
        queue.submit(egui_commands.into_iter().chain([encoder.finish()]));
        if let Some(timer) = &mut self.timer {
            timer.submitted(device);
        }

        self.window.pre_present_notify();
        queue.present(frame);
    }

    /// The thumbnail to take this frame, if any: of the look on screen, once it has run a
    /// while (not paused, not fading), if it has none yet.
    fn thumbnail_wanted(&mut self) -> Option<String> {
        let d = &self.ui.director;
        // In silence most looks are still or dark: wait for the music.
        let silent = self.music.features().is_none_or(|f| f.silent);
        if self.thumbnailer.busy() || self.ui.paused || silent || d.changing() || d.elapsed() < THUMBNAIL_AFTER_S {
            return None;
        }
        let look = d.current.and_then(|i| self.ui.looks.looks.get(i)).filter(|l| self.applied_look.as_ref() == Some(*l))?.clone();
        self.ui.thumbnails.wants(&look)
    }

    /// The canvas as fractions of the window: x, y, width, height.
    fn canvas_fraction(&self) -> [f32; 4] {
        let [x, y, w, h] = self.viewport_3d();
        let (sw, sh) = (self.gpu.config.width.max(1) as f32, self.gpu.config.height.max(1) as f32);
        [x / sw, y / sh, w / sw, h / sh]
    }

    /// The 2D flow for the feedback: the solver's velocity grid, or its particles'
    /// velocities splatted onto one.
    fn draw_flow_2d(&mut self, encoder: &mut wgpu::CommandEncoder, view: &View, size: [u32; 2]) {
        let world = &self.world;
        let layout = match world.solver.velocity_field(&mut self.velocity) {
            Some(layout) => Some(layout),
            None => world.solver.particles().map(|p| render::feedback::splat_velocity(&p, world.scene.size, &mut self.velocity)),
        };
        let grid = layout.map(|l| (self.velocity.as_slice(), l));
        self.feedback.draw_flow_2d(&self.gpu.device, &self.gpu.queue, encoder, size, view, grid);
    }

    /// The look's fractal onto `target` (the scene, at `scale` of the window's size).
    fn draw_fractal(&self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView, scale: f32) {
        let viewport = self.viewport_3d().map(|v| v * scale);
        let fractal = &self.look_frame.waves.fractal;
        self.fractal.draw(&self.gpu.device, &self.gpu.queue, encoder, target, viewport, fractal, &self.palette_now, self.bend.displacement(), self.fractal_quality);
    }

    /// The 2D scene into `target`, `size` pixels, `scale` of the window's.
    fn render_2d(&mut self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView, view: &View, size: [u32; 2], scale: f32) {
        let (device, queue) = (&self.gpu.device, &self.gpu.queue);
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
            target_size: (size[0], size[1]),
            pixel_scale: scale,
            black_letterbox: self.look_frame.feedback.enabled,
            palette: self.palette_now,
            walls: !self.look_frame.feedback.enabled,
            brush: self.input.wall_brush(view, &self.ui).filter(|_| !self.egui_ctx.is_pointer_over_egui()),
        };
        self.field_range = self.renderer.render(device, queue, encoder, target, options);
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
        let (solver, scene) = match &self.world3d {
            Some(w) => (sim3d::SOLVERS[w.solver_index].name, sim3d::PRESETS[w.preset_index].name),
            None => (self.world.solvers[self.world.solver_index].name, PRESETS[self.world.preset_index].name),
        };
        let title = format!("fluid toy | {solver} | {scene} | {:.0} fps", self.perf.latest().fps);
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


/// `fluid-toy --import <path>`: convert the MilkDrop presets there into looks, and
/// say how it went.
fn import_presets(path: &std::path::Path) -> anyhow::Result<()> {
    let folder = looks::Library::user_folder().ok_or_else(|| anyhow::anyhow!("no folder for looks"))?;
    let presets = looks::milk::presets_in(path);
    if presets.is_empty() {
        anyhow::bail!("no .milk presets at {}", path.display());
    }
    let (mut ok, mut partly, mut failed) = (0, 0, 0);
    for preset in &presets {
        match looks::milk::import_file(preset, &folder) {
            Ok((file, look)) => {
                ok += 1;
                if look.left_out.is_empty() {
                    println!("{} -> {}", preset.display(), file.display());
                } else {
                    partly += 1;
                    println!("{} -> {} (left out: {})", preset.display(), file.display(), look.left_out.join("; "));
                }
            }
            Err(e) => {
                failed += 1;
                println!("{}: not imported: {e}", preset.display());
            }
        }
    }
    println!("{ok} imported into {} ({partly} with parts left out), {failed} not", folder.display());
    Ok(())
}

/// Word from a thread saving a screenshot or a recording: where it went, or why not.
type Saved = Result<std::path::PathBuf, String>;

/// What a frame was grabbed for.
#[derive(Clone, Copy, Debug)]
enum Grab {
    Screenshot,
    /// A recording's frame, grabbed this many seconds into it.
    Video(f64),
}

/// `v` turned `degrees` anticlockwise.
fn tilt(v: sim::Vec2, degrees: f32) -> sim::Vec2 {
    if degrees == 0.0 {
        return v;
    }
    let (s, c) = degrees.to_radians().sin_cos();
    sim::Vec2::new(c * v.x - s * v.y, s * v.x + c * v.y)
}
