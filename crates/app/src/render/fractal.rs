//! Escape-time fractals (Mandelbrot, Julia and others), drawn into the scene before the
//! feedback, as the cover art is (see `art`), so the warp carries them. Each pixel
//! iterates the fractal in `fractal.wgsl`; the colour comes from the look's palette, and
//! only the filaments near the set are opaque, so the fluid shows through elsewhere. A
//! look asks for one in `[fractal]` (part of its waves, `WaveSettings::fractal`).
//!
//! The shader works in f32: past about 10⁵× zoom the picture would break into blocks,
//! so zoom is capped there (`MAX_ZOOM`).

use super::palette::Palette;
use super::post::SCENE_FORMAT;
use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};
use std::f32::consts::TAU;

/// The deepest zoom f32 holds without blocks.
pub const MAX_ZOOM: f32 = 1e5;
pub const MAX_ITERATIONS: u32 = 1000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FractalKind {
    /// z² + c, c the pixel.
    #[default]
    #[serde(alias = "mandelbrot")]
    Mandelbrot,
    /// z² + c, z the pixel and c the look's `julia` point.
    #[serde(alias = "julia")]
    Julia,
    /// (|re z| + i |im z|)² + c: a ship, its sails along the negative real axis.
    #[serde(alias = "burning_ship", alias = "burningship")]
    BurningShip,
    /// conj(z)² + c: the Mandelbrot set's three-cornered cousin.
    #[serde(alias = "tricorn")]
    Tricorn,
    /// Newton's method on z³ − 1: three basins, coloured by the root each pixel finds.
    #[serde(alias = "newton")]
    Newton,
    /// 3D: the Mandelbrot set's cousin in spherical coordinates (raymarched), its lobes
    /// set by `FractalSettings::power`.
    #[serde(alias = "mandelbulb")]
    Mandelbulb,
    /// 3D: a cube with the middle of each face and the centre taken out, and the same
    /// again in each of the twenty cubes left, for ever (raymarched).
    #[serde(alias = "menger", alias = "menger_sponge")]
    Menger,
}

impl FractalKind {
    pub const ALL: [Self; 7] = [Self::Mandelbrot, Self::Julia, Self::BurningShip, Self::Tricorn, Self::Newton, Self::Mandelbulb, Self::Menger];

    pub fn label(self) -> &'static str {
        match self {
            Self::Mandelbrot => "Mandelbrot",
            Self::Julia => "Julia",
            Self::BurningShip => "Burning ship",
            Self::Tricorn => "Tricorn",
            Self::Newton => "Newton",
            Self::Mandelbulb => "Mandelbulb",
            Self::Menger => "Menger sponge",
        }
    }

    /// Whether it's a solid, raymarched in 3D (its camera orbits it).
    pub fn is_3d(self) -> bool {
        matches!(self, Self::Mandelbulb | Self::Menger)
    }

    /// Whether it can dive (the Mandelbulb doesn't repeat itself as it's zoomed).
    pub fn dives(self) -> bool {
        self != Self::Mandelbulb
    }

    /// The shader's number for it.
    fn code(self) -> u32 {
        self as u32
    }
}

/// Which fractal, where, and how it's coloured.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FractalSettings {
    pub enabled: bool,
    pub kind: FractalKind,
    /// The point in the fractal's plane at the canvas's centre.
    pub centre: [f32; 2],
    /// 1: the canvas is 3 units of the plane high (the whole Mandelbrot set fits).
    pub zoom: f32,
    /// Turn, radians.
    pub rotation: f32,
    /// Turn, radians per second.
    pub spin: f32,
    pub iterations: u32,
    /// Julia's constant `c`.
    pub julia: [f32; 2],
    /// 0 (unseen) to 1.
    pub opacity: f32,
    /// Light, 0 to 4 (above 1 glows).
    pub brightness: f32,
    /// How many times the palette repeats from the far outside to the set's edge.
    pub bands: f32,
    /// The palette's cycle, turns per second.
    pub colour_speed: f32,
    /// How fast a Julia set's point travels along the Mandelbrot set's edge (about laps
    /// a minute at full loudness; 0: it stays put). See `Motion`.
    pub wander: f32,
    /// How much the zoom follows the bass, 0 to 1.
    pub bass_zoom: f32,
    /// Move to another spot at each new section of the song.
    pub tour: bool,
    /// How far the fluid's flow carries the picture (1: with the fluid; 0: not at all),
    /// springing back when it stills. See `bend`.
    pub bend: f32,
    /// Travel endlessly into the fractal (negative: out of it), zoom doublings a second
    /// at middling loudness; 0: stay put. It flies to a point where the picture repeats
    /// itself and loops there. See `Motion`.
    pub dive: f32,
    /// The Mandelbulb's power: its lobes (8 is the classic one). A Mandelbulb's
    /// `wander` swings it.
    pub power: f32,
    /// Set by `Motion`, not a setting: iterations taken off the counts before colouring,
    /// so a dive's colours stay put as it goes deeper.
    #[serde(skip)]
    pub shift: f32,
}

impl FractalSettings {
    pub const OFF: Self = Self {
        enabled: false,
        kind: FractalKind::Mandelbrot,
        centre: [-0.6, 0.0],
        zoom: 1.0,
        rotation: 0.0,
        spin: 0.0,
        iterations: 160,
        julia: [-0.8, 0.156],
        opacity: 1.0,
        bands: 2.0,
        brightness: 1.0,
        colour_speed: 0.05,
        wander: 0.0,
        bass_zoom: 0.0,
        tour: false,
        bend: 0.0,
        dive: 0.0,
        power: 8.0,
        shift: 0.0,
    };
}

impl Default for FractalSettings {
    fn default() -> Self {
        Self::OFF
    }
}

/// Must match `Fractal` in fractal.wgsl (208 bytes).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    /// The palette in linear RGB: deep, base, highlight.
    palette: [[f32; 4]; 3],
    centre: [f32; 2],
    /// The turn as (cos, sin).
    turn: [f32; 2],
    julia: [f32; 2],
    /// Half the canvas's height, in units of the plane.
    half_height: f32,
    aspect: f32,
    kind: u32,
    iterations: u32,
    opacity: f32,
    brightness: f32,
    bands: f32,
    /// The palette's cycle so far, turns.
    phase: f32,
    /// One pixel, in the canvas's -1 to 1 up the canvas (for the supersamples' spacing).
    pixel: f32,
    /// Iterations taken off the counts before colouring (`FractalSettings::shift`).
    shift: f32,
    /// The displacement's uv (scene) to the canvas's -1 to 1 (and y up); 0 with no bend.
    bend: [f32; 2],
    /// Samples a pixel: 4, or 1 when the frames can't keep up (`Quality`).
    samples: u32,
    _pad: f32,
    /// The 3D kinds' camera: where it is, and its forward (w: the tangent of half the
    /// field of view), right and up.
    eye: [f32; 4],
    forward: [f32; 4],
    right: [f32; 4],
    up: [f32; 4],
    /// The 3D kinds' shape: x the Mandelbulb's power, y the point colours count out from
    /// (xyz of `aim`), w unused.
    aim: [f32; 4],
}

/// The 3D kinds' camera: its field of view (radians, up the canvas) and its height above
/// the fractal's equator (radians), and how far back it sits at zoom 1 (the whole thing
/// in view).
const FOV_3D: f32 = 0.8;
const PITCH_3D: f32 = 0.35;
const DISTANCE_3D: f32 = 3.0;
/// The Menger sponge's dive heads for its corner (1, 1, 1), looking in from within this
/// angle of the diagonal (radians), so the camera never goes inside the cube.
const MENGER_CONE: f32 = 0.5;
/// And looks this far into the cube past the corner (a share of its distance), so the
/// sponge fills the canvas rather than half of it staying the dark outside. It shrinks
/// with the distance, so the loop stays seamless.
const MENGER_LEAN: f32 = 0.6;

/// How much of its detail the fractal keeps, when the frames can't keep up (lowered a
/// step at a time by the app's governor; 0 is all of it).
pub const QUALITY_STEPS: u32 = 4;

/// The samples a pixel and the share of the iterations at quality step `step`: first
/// one sample in place of four (a quarter of the work), then 70 % of the iterations a
/// step, down to a third.
pub fn quality(step: u32) -> (u32, f32) {
    match step {
        0 => (4, 1.0),
        s => (1, 0.7f32.powi(s as i32 - 1).max(0.34)),
    }
}

pub struct FractalLayer {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    /// No displacement, for when the fluid doesn't bend it.
    still: wgpu::TextureView,
    /// The spin and the colour cycle, added up.
    turn: f32,
    phase: f32,
}

impl FractalLayer {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("fractal.wgsl"), source: wgpu::ShaderSource::Wgsl(include_str!("fractal.wgsl").into()) });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fractal"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: wgpu::BufferSize::new(size_of::<Uniforms>() as u64) },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("fractal"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        // Premultiplied alpha: the fractal over the scene.
        let over = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fractal"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: SCENE_FORMAT, blend: Some(wgpu::BlendState { color: over, alpha: over }), write_mask: wgpu::ColorWrites::COLOR })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fractal uniforms"),
            size: size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let still = super::feedback::texture(device, "fractal still", [1, 1], super::feedback::FLOW_FORMAT);
        Self { pipeline, layout, uniforms, still, turn: 0.0, phase: 0.0 }
    }

    /// Advance the spin and the colour cycle by `dt` seconds (call every frame).
    pub fn update(&mut self, settings: &FractalSettings, dt: f32) {
        self.turn = (self.turn + settings.spin * dt) % TAU;
        self.phase = (self.phase + settings.colour_speed * dt).rem_euclid(1.0);
    }

    /// Draw the fractal onto `target` within `viewport` (pixels: x, y, width, height), if
    /// `settings` ask for it, bent by `bend`'s displacement (scene uv, the target's size)
    /// if there is one.
    /// A 3D kind's camera turns with `camera`'s (yaw, pitch) too, when there is one (a
    /// 3D look's).
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        viewport: [f32; 4],
        settings: &FractalSettings,
        palette: &Palette,
        bend: Option<&wgpu::TextureView>,
        quality_step: u32,
        camera: Option<(f32, f32)>,
    ) {
        let [x, y, w, h] = viewport;
        if !settings.enabled || settings.opacity <= 0.0 || w < 1.0 || h < 1.0 {
            return;
        }
        let mut uniforms = self.uniforms(settings, palette, w / h, h, camera);
        let (samples, share) = quality(quality_step);
        // A raymarch is too much work to sample a pixel four times.
        uniforms.samples = if settings.kind.is_3d() { 1 } else { samples };
        uniforms.iterations = ((uniforms.iterations as f32 * share) as u32).max(16.min(uniforms.iterations));
        if bend.is_some() {
            // Scene uv (y down) to the canvas's -1 to 1 (y up).
            let size = target.texture().size();
            uniforms.bend = [2.0 * size.width as f32 / w, -2.0 * size.height as f32 / h];
        }
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&uniforms));
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fractal"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniforms.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(bend.unwrap_or(&self.still)) },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("fractal"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        pass.set_viewport(x, y, w, h, 0.0, 1.0);
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
    }

    fn uniforms(&self, s: &FractalSettings, palette: &Palette, aspect: f32, height_px: f32, camera: Option<(f32, f32)>) -> Uniforms {
        let half_height = 1.5 / view_zoom(s.zoom);
        let turn = s.rotation + self.turn;
        let (eye, forward, right, up, aim) = camera_3d(s.kind, s.dive != 0.0, turn, view_zoom(s.zoom), camera);
        let v4 = |v: glam::Vec3, w: f32| [v.x, v.y, v.z, w];
        Uniforms {
            palette: palette.linear(),
            centre: s.centre.map(|c| if c.is_finite() { c.clamp(-4.0, 4.0) } else { 0.0 }),
            turn: [turn.cos(), turn.sin()],
            julia: s.julia.map(|c| if c.is_finite() { c.clamp(-4.0, 4.0) } else { 0.0 }),
            half_height,
            aspect,
            // A diving sponge is the sponge grown from its corner (fractal.wgsl's 7).
            kind: if s.kind == FractalKind::Menger && s.dive != 0.0 { 7 } else { s.kind.code() },
            iterations: s.iterations.clamp(1, MAX_ITERATIONS),
            opacity: s.opacity.clamp(0.0, 1.0),
            brightness: s.brightness.max(0.0),
            bands: s.bands.max(0.0),
            phase: self.phase,
            pixel: 2.0 / height_px,
            shift: if s.shift.is_finite() { s.shift.max(0.0) } else { 0.0 },
            bend: [0.0; 2],
            samples: 4,
            _pad: 0.0,
            eye: v4(eye, 0.0),
            forward: v4(forward, (0.5 * FOV_3D).tan()),
            right: v4(right, 0.0),
            up: v4(up, 0.0),
            aim: [if s.power.is_finite() { s.power.clamp(2.0, 16.0) } else { 8.0 }, aim.x, aim.y, aim.z],
        }
    }
}

/// A 3D kind's camera at `turn` around it (radians, plus a 3D look's camera's yaw and
/// pitch), `zoom` times closer than the whole: eye, forward, right, up, and the point it
/// looks at. A diving Menger sponge's looks at its corner from within a cone about the
/// diagonal; otherwise at the middle, from a little above.
fn camera_3d(kind: FractalKind, diving: bool, turn: f32, zoom: f32, camera: Option<(f32, f32)>) -> (glam::Vec3, glam::Vec3, glam::Vec3, glam::Vec3, glam::Vec3) {
    use glam::Vec3;
    let (yaw, pitch) = camera.unwrap_or((0.0, 0.0));
    let distance = DISTANCE_3D / zoom.max(1e-3);
    let (aim, back, lean) = if kind == FractalKind::Menger && diving {
        let diagonal = Vec3::ONE.normalize();
        let across = Vec3::new(1.0, -1.0, 0.0).normalize();
        let other = diagonal.cross(across);
        let (s, c) = (turn + yaw).sin_cos();
        let off = MENGER_CONE.sin() * (c * across + s * other);
        (Vec3::ONE, (MENGER_CONE.cos() * diagonal + off).normalize(), -MENGER_LEAN * distance * diagonal)
    } else {
        let (sy, cy) = (turn + yaw).sin_cos();
        let (sp, cp) = (PITCH_3D + pitch).clamp(-1.4, 1.4).sin_cos();
        (Vec3::ZERO, Vec3::new(cp * sy, sp, cp * cy), Vec3::ZERO)
    };
    let eye = aim + distance * back;
    let forward = (aim + lean - eye).normalize();
    let right = forward.cross(Vec3::Y).normalize_or(Vec3::X);
    let up = right.cross(forward);
    (eye, forward, right, up, aim)
}

/// The places worth going on a tour of `kind`: centre and zoom (the first shows it whole).
pub fn spots(kind: FractalKind) -> &'static [([f32; 2], f32)] {
    SPOTS[kind.code() as usize]
}

/// Places worth going on a tour, per kind: centre and zoom (the first shows it whole).
/// Each shows the set's edge (a test checks).
const SPOTS: [&[([f32; 2], f32)]; 7] = [
    // Mandelbrot: the whole, seahorse and elephant valleys, an antenna, a mini set on the
    // spike, and two deeper spirals.
    &[([-0.6, 0.0], 1.0), ([-0.7453, 0.1127], 40.0), ([0.2925, 0.0149], 50.0), ([-0.1592, 1.0317], 40.0), ([-1.7549, 0.0], 40.0), ([-0.7435, 0.1314], 200.0), ([-0.235125, 0.827215], 300.0)],
    // Julia: the point moves instead (along the edge, to a random place).
    &[([0.0, 0.0], 1.0)],
    // Burning Ship: the whole, the ship, two of its masts, and the bow.
    &[([-0.45, -0.5], 0.9), ([-1.762, -0.028], 25.0), ([-1.6254, -0.0047], 60.0), ([-1.7795, -0.0255], 400.0), ([-0.45, -0.9], 6.0)],
    // Tricorn: the whole, its left point, an upper lobe, and further left.
    &[([-0.3, 0.0], 0.9), ([-1.1, 0.0], 6.0), ([0.2, 0.55], 6.0), ([-1.32, 0.0], 20.0)],
    // Newton: the whole, and three of the places where the basins meet.
    &[([0.0, 0.0], 0.8), ([-0.5, 0.0], 3.0), ([0.25, 0.433], 3.0), ([-0.25, 0.0], 12.0)],
    // The 3D kinds: whole, and two closer (the camera orbits; the centre isn't used).
    &[([0.0, 0.0], 1.0), ([0.0, 0.0], 1.8), ([0.0, 0.0], 2.6)],
    &[([0.0, 0.0], 1.0), ([0.0, 0.0], 1.6), ([0.0, 0.0], 2.4)],
];

/// A Julia set's point on the edge of the Mandelbrot set's main cardioid at `angle`,
/// pushed `out` along the edge's outward normal (outside: lacy, broken sets; inside:
/// solid ones).
pub fn cardioid_edge(angle: f32, out: f32) -> [f32; 2] {
    let (s1, c1) = angle.sin_cos();
    let (s2, c2) = (2.0 * angle).sin_cos();
    let point = [0.5 * c1 - 0.25 * c2, 0.5 * s1 - 0.25 * s2];
    // The tangent, and the outward normal a quarter turn clockwise from it (none at the
    // cusp, where the edge turns back: straight out along the real axis).
    let tangent = [0.5 * (s2 - s1), 0.5 * (c1 - c2)];
    let length = tangent[0].hypot(tangent[1]);
    let normal = if length > 1e-4 { [tangent[1] / length, -tangent[0] / length] } else { [1.0, 0.0] };
    [point[0] + out * normal[0], point[1] + out * normal[1]]
}

/// The angle of the cardioid's edge nearest `c`.
fn nearest_edge(c: [f32; 2]) -> f32 {
    (0..360)
        .map(|k| k as f32 * TAU / 360.0)
        .min_by(|&a, &b| {
            let d = |t: f32| {
                let p = cardioid_edge(t, 0.0);
                (p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2)
            };
            d(a).total_cmp(&d(b))
        })
        .unwrap_or(0.0)
}

/// A complex number, in f64 for the CPU's side of the dive.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Cx(f64, f64);

impl Cx {
    fn of(p: [f32; 2]) -> Self {
        Self(f64::from(p[0]), f64::from(p[1]))
    }
    fn add(self, o: Self) -> Self {
        Self(self.0 + o.0, self.1 + o.1)
    }
    fn sub(self, o: Self) -> Self {
        Self(self.0 - o.0, self.1 - o.1)
    }
    fn mul(self, o: Self) -> Self {
        Self(self.0 * o.0 - self.1 * o.1, self.0 * o.1 + self.1 * o.0)
    }
    fn scale(self, k: f64) -> Self {
        Self(self.0 * k, self.1 * k)
    }
    fn div(self, o: Self) -> Self {
        let d = (o.0 * o.0 + o.1 * o.1).max(1e-300);
        Self((self.0 * o.0 + self.1 * o.1) / d, (self.1 * o.0 - self.0 * o.1) / d)
    }
    fn abs(self) -> f64 {
        self.0.hypot(self.1)
    }
    fn arg(self) -> f64 {
        self.1.atan2(self.0)
    }
    fn sqrt(self) -> Self {
        let (r, a) = (self.abs().sqrt(), 0.5 * self.arg());
        Self(r * a.cos(), r * a.sin())
    }
    fn f32(self) -> [f32; 2] {
        [self.0 as f32, self.1 as f32]
    }
}

/// The smooth iteration count at `p` as fractal.wgsl finds it (in f64), or none inside.
fn smooth_count(kind: FractalKind, p: Cx, julia: Cx, limit: u32) -> Option<f64> {
    if kind.is_3d() {
        // A solid: no count across the plane (a dive's colours count from 0).
        return None;
    }
    if kind == FractalKind::Newton {
        let roots = [Cx(1.0, 0.0), Cx(-0.5, 0.866_025_403_784_438_6), Cx(-0.5, -0.866_025_403_784_438_6)];
        let mut z = p;
        for i in 0..limit {
            let z2 = z.mul(z);
            z = z.sub(z2.mul(z).sub(Cx(1.0, 0.0)).div(z2.scale(3.0)));
            for r in roots {
                let d = z.sub(r).abs().powi(2);
                if d < 1e-6 {
                    return Some(f64::from(i) + 1.0 + ((1e-6f64).ln() / d.max(1e-300).ln()).log2().clamp(-1.0, 0.0));
                }
            }
        }
        return None;
    }
    let c = if kind == FractalKind::Julia { julia } else { p };
    let mut z = p;
    for i in 0..limit {
        match kind {
            FractalKind::BurningShip => z = Cx(z.0.abs(), z.1.abs()),
            FractalKind::Tricorn => z.1 = -z.1,
            _ => {}
        }
        z = z.mul(z).add(c);
        let r2 = z.0 * z.0 + z.1 * z.1;
        if r2 > 256.0 {
            return Some((f64::from(i) + 1.0 - (0.5 * r2.ln()).log2()).max(0.0));
        }
    }
    None
}

/// A point where the picture repeats itself: around it, zooming in by |λ| and turning
/// by arg λ shows the same picture again, `period` iterations deeper. A dive loops
/// between `half` (the canvas's half-height) and `half / |λ|`.
#[derive(Clone, Copy, Debug)]
struct DivePoint {
    at: Cx,
    lambda: Cx,
    period: f64,
    half: f64,
}

/// Where the Mandelbrot set, the Burning Ship and the Tricorn loop: Misiurewicz points,
/// whose critical orbit falls into a repelling cycle (`pre` steps to it, `period` long);
/// the set near one repeats itself as the cycle's multiplier says (Tan Lei). With the
/// widest half-height that still loops seamlessly (found numerically; a test checks).
/// Found among the low-order ones for being branch points or tips worth seeing.
/// A Misiurewicz point: where, steps to its cycle, the cycle's length, widest half-height.
type Misiurewicz = ([f64; 2], u32, u32, f64);

const DIVES: [&[Misiurewicz]; 7] = [
    &[
        ([-0.564_097_854_9, 0.679_273_348_3], 2, 4, 0.01),
        ([0.470_426_028_3, 0.346_588_331_3], 4, 2, 0.02),
        ([0.445_783_624_4, 0.208_380_672_9], 5, 2, 0.01),
        ([0.419_643_377_607, 0.606_290_729_207], 3, 2, 0.02),
        ([0.0, 1.0], 2, 2, 0.02),
        ([-0.506_294_458_196, 0.683_991_968_089], 4, 2, 0.005),
        ([0.037_133_672_33, 0.806_378_316_492], 5, 2, 0.005),
        ([0.027_016_564_508, 0.846_915_826_863], 3, 4, 0.005),
        ([-0.670_524_811_969, 0.482_038_738_408], 4, 4, 0.005),
    ],
    // Julia: its β fixed point (`dive_point`).
    &[],
    // The Burning Ship and the Tricorn match the Mandelbrot set on the real axis, so its
    // real Misiurewicz points loop for them too (over two turns of the cycle, as λ is
    // negative and they aren't symmetric about the axis).
    &[([-1.543_689_012_692_076_4, 0.0], 3, 1, 0.005), ([-1.839_286_755_214_161, 0.0], 3, 2, 0.0025)],
    &[([-1.543_689_012_692_076_4, 0.0], 3, 1, 0.005), ([-1.839_286_755_214_161, 0.0], 3, 2, 0.005)],
    // Newton: 0, where the three basins meet in a six-armed star (`dive_point`).
    &[],
    // The Mandelbulb doesn't dive; the Menger sponge dives at its corner (`dive_point`).
    &[],
    &[],
];

/// The dive's `index`th point for `kind` (a Julia set's from its constant `julia`).
fn dive_point(kind: FractalKind, index: usize, julia: Cx) -> DivePoint {
    match kind {
        FractalKind::Julia => {
            // β, the repelling fixed point: z² + c = z, multiplier 2β.
            let beta = Cx(1.0, 0.0).add(Cx(1.0, 0.0).sub(julia.scale(4.0)).sqrt()).scale(0.5);
            DivePoint { at: beta, lambda: beta.scale(2.0), period: 1.0, half: 0.02 }
        }
        // Newton's map is 2z/3 + 1/(3z²): ∞ is a repelling fixed point (multiplier 3/2),
        // and 0 maps to it through 1/(3z²), so near 0 the picture repeats every √1.5.
        FractalKind::Newton => DivePoint { at: Cx(0.0, 0.0), lambda: Cx(1.5f64.sqrt(), 0.0), period: 1.0, half: 0.04 },
        // The sponge near its corner is itself shrunk to a third about the corner: the
        // camera (`camera_3d`) goes round a loop from the whole distance to a third of
        // it, a level of holes deeper each time. (The centre isn't used.) The Mandelbulb
        // doesn't dive: this is never asked for it.
        FractalKind::Menger | FractalKind::Mandelbulb => DivePoint { at: Cx(0.0, 0.0), lambda: Cx(3.0, 0.0), period: 1.0, half: 1.5 },
        _ => {
            let points = DIVES[kind.code() as usize];
            let (at, pre, period, half) = points[index % points.len()];
            let c = Cx(at[0], at[1]);
            let mut z = Cx(0.0, 0.0);
            let mut lambda = Cx(1.0, 0.0);
            for i in 0..pre + period {
                if i >= pre {
                    lambda = lambda.mul(z.scale(2.0));
                }
                z = z.mul(z).add(c);
            }
            if kind == FractalKind::Mandelbrot {
                DivePoint { at: c, lambda, period: f64::from(period), half }
            } else {
                DivePoint { at: c, lambda: lambda.mul(lambda), period: f64::from(2 * period), half }
            }
        }
    }
}

/// The number of dive points for `kind`.
fn dive_points(kind: FractalKind) -> usize {
    DIVES[kind.code() as usize].len().max(1)
}

/// The dive point nearest `centre`.
fn nearest_dive(kind: FractalKind, centre: [f32; 2]) -> usize {
    let c = Cx::of(centre);
    (0..dive_points(kind)).min_by(|&a, &b| dive_point(kind, a, Cx(0.0, 0.0)).at.sub(c).abs().total_cmp(&dive_point(kind, b, Cx(0.0, 0.0)).at.sub(c).abs())).unwrap_or(0)
}

/// Around a dive's loop at its widest: the typical count (what its colours count from),
/// and which way the set lies, as a share of a half-height (none around a branch point,
/// whose arms balance; toward its one arm, from a tip).
fn surroundings(kind: FractalKind, point: &DivePoint, julia: Cx, limit: u32) -> Option<(f64, Cx)> {
    let ring: Vec<(Cx, f64)> = (0..24)
        .map(|k| {
            let a = f64::from(k) * std::f64::consts::TAU / 24.0;
            let way = Cx(a.cos(), a.sin());
            (way, smooth_count(kind, point.at.add(way.scale(0.7 * point.half)), julia, limit).unwrap_or(f64::from(limit)))
        })
        .collect();
    let mut counts: Vec<f64> = ring.iter().map(|r| r.1).filter(|&n| n < f64::from(limit)).collect();
    counts.sort_by(f64::total_cmp);
    let median = *counts.get(counts.len() / 2)?;
    let (mut way, mut total) = (Cx(0.0, 0.0), 0.0);
    for (w, n) in ring {
        let weight = (n - median).max(0.0);
        way = way.add(w.scale(weight));
        total += weight;
    }
    Some((median, if total > 0.0 { way.scale(1.0 / total) } else { Cx(0.0, 0.0) }))
}

/// How far a dive's view sits from its point toward the set, in half-heights at full
/// lean (a tip's one arm then crosses the canvas, rather than half of it staying dark).
/// The offset scales and turns with the loop, so the loop stays seamless.
const LEAN: f64 = 0.8;

/// The widest half-height of a dive's loop when its view leans by `lean` (half-heights):
/// narrower, so the canvas reaches no further from the point than it would centred.
fn loop_half(point: &DivePoint, lean: Cx) -> f64 {
    point.half / (1.0 + lean.abs())
}

/// How a view flies from one place to another: zooming out and back in on the way, as
/// far as the distance asks, at a steady pace (van Wijk and Nuij's optimal path, with
/// their ρ).
struct Flight {
    from: Cx,
    to: Cx,
    w0: f64,
    w1: f64,
    /// The distance, and the path's parameters (none when it only zooms).
    u1: f64,
    r0: f64,
    length: f64,
}

const RHO: f64 = 1.42;

impl Flight {
    /// From `from` showing `w0` (half-height) to `to` showing `w1`.
    fn new(from: Cx, w0: f64, to: Cx, w1: f64) -> Self {
        let u1 = to.sub(from).abs();
        let rho2 = RHO * RHO;
        if u1 < 1e-9 * w0.min(w1) {
            return Self { from, to, w0, w1, u1: 0.0, r0: 0.0, length: (w1 / w0).ln().abs() / RHO };
        }
        let b = |w: f64, sign: f64| (w1 * w1 - w0 * w0 + sign * rho2 * rho2 * u1 * u1) / (2.0 * w * rho2 * u1);
        let r = |b: f64| (-b + (b * b + 1.0).sqrt()).ln();
        let (r0, r1) = (r(b(w0, 1.0)), r(b(w1, -1.0)));
        Self { from, to, w0, w1, u1, r0, length: (r1 - r0) / RHO }
    }

    /// Where it is `s` along (0 to `length`): the centre and the half-height.
    fn at(&self, s: f64) -> (Cx, f64) {
        if self.u1 == 0.0 {
            let k = if self.w1 < self.w0 { -1.0 } else { 1.0 };
            return (self.to, self.w0 * (k * RHO * s).exp());
        }
        let (r0, rho2) = (self.r0, RHO * RHO);
        let u = self.w0 / rho2 * (r0.cosh() * (RHO * s + r0).tanh() - r0.sinh());
        let w = self.w0 * r0.cosh() / (RHO * s + r0).cosh();
        (self.from.add(self.to.sub(self.from).scale(u / self.u1)), w)
    }
}

/// What's on the canvas: the centre, the half-height, the turn added (radians) and the
/// colours' shift (`FractalSettings::shift`).
#[derive(Clone, Copy, Debug)]
struct View {
    centre: Cx,
    half: f64,
    turn: f64,
    shift: f64,
}

/// A dive under way.
struct Dive {
    /// Which of the kind's dive points.
    point: usize,
    /// Flying to it: from where, and how far along.
    flight: Option<(View, f64)>,
    /// Around the loop, 0 to 1.
    phase: f64,
    /// The count its colours count from, and the way the view leans from the point
    /// (`surroundings`), eased.
    base: Option<(f64, Cx)>,
}

/// The dive's pace, zoom doublings a second: from a little under half the setting in
/// silence to a little over one and a half times it when loud, and up to four times it
/// on a Morph kick.
fn dive_rate(dive: f32, level: f32, morph: f32) -> f64 {
    f64::from(dive.abs()) * std::f64::consts::LN_2 * f64::from(0.4 + 1.2 * level) * f64::from(1.0 + 3.0 * morph)
}

/// The base count follows changes (a Julia set's point moving) with this time constant.
const BASE_S: f64 = 1.0;

/// What the music is doing, for `Motion`.
#[derive(Clone, Copy, Debug)]
pub struct Music {
    /// MilkDrop's `bass_att`: about 1 on average.
    pub bass: f32,
    /// Loudness, 0 to 1.
    pub level: f32,
    /// The song's section changes so far.
    pub sections: u32,
    /// The Morph reaction's kick, 0 to 1.
    pub morph: f32,
}

impl Music {
    pub const QUIET: Self = Self { bass: 1.0, level: 0.0, sections: 0, morph: 0.0 };
}

/// Laps a minute, as radians a second.
const LAP: f32 = TAU / 60.0;
/// How far the Morph reaction throws the Julia point along the edge, radians a second at
/// full kick, and how much it zooms in (a multiple).
const MORPH_LEAP: f32 = 0.5;
const MORPH_PUNCH: f32 = 0.25;
/// How far out from the edge the Julia point sits, and how far the bass moves it.
const EDGE_OUT: f32 = 0.01;
const EDGE_BASS: f32 = 0.02;
/// How far a Mandelbulb's power swings either side of its own as it wanders.
const POWER_SWING: f32 = 3.0;
/// Moving to the next spot on a tour: the glide's time constant, seconds.
const GLIDE_S: f32 = 0.6;

/// The fractal's own movement with the music, from its settings (`wander`, `bass_zoom`,
/// `tour`) and the Morph reaction: applied to a look's settings before its equations
/// run, so they can still override it.
pub struct Motion {
    kind: Option<FractalKind>,
    /// The Julia point's angle along the cardioid's edge, once it moves.
    angle: Option<f32>,
    /// On a tour: where it's going (centre, log zoom) and where it is, and which spot.
    target: Option<([f32; 2], f32)>,
    at: Option<([f32; 2], f32)>,
    spot: usize,
    /// The section changes seen, once the tour starts counting.
    sections: Option<u32>,
    /// Diving, and what it last showed (where the next flight starts).
    dive: Option<Dive>,
    view: Option<View>,
    rng: utils::rng::SplitMix64,
}

impl Default for Motion {
    fn default() -> Self {
        Self { kind: None, angle: None, target: None, at: None, spot: 0, sections: None, dive: None, view: None, rng: utils::rng::SplitMix64::new(0x00f4_ac7a) }
    }
}

impl Motion {
    /// `settings` moved by the music over `dt` seconds.
    pub fn apply(&mut self, settings: &FractalSettings, music: &Music, dt: f32) -> FractalSettings {
        let mut s = *settings;
        if !s.enabled || self.kind != Some(s.kind) {
            *self = Self { rng: self.rng.clone(), ..Self::default() };
            if !s.enabled {
                return s;
            }
            self.kind = Some(s.kind);
        }
        let morph = if music.morph.is_finite() { music.morph.clamp(0.0, 1.0) } else { 0.0 };
        let bass = if music.bass.is_finite() { music.bass.clamp(0.0, 3.0) } else { 1.0 };
        let level = if music.level.is_finite() { music.level.clamp(0.0, 1.0) } else { 0.0 };
        let new_section = s.tour && {
            let seen = *self.sections.get_or_insert(music.sections);
            self.sections = Some(music.sections);
            music.sections > seen
        };
        if !s.tour {
            self.sections = None;
        }

        if s.dive != 0.0 && s.dive.is_finite() && s.kind.dives() {
            // A dive moves the view itself (a tour moves it on to another point).
            (self.target, self.at) = (None, None);
            if new_section && s.kind == FractalKind::Julia {
                self.angle = Some(self.rng.next_f32() * TAU);
            }
            self.julia_point(&mut s, settings, bass, level, morph, dt);
            let rate = dive_rate(s.dive, level, morph);
            self.dive_step(&mut s, new_section, rate, dt);
        } else {
            (self.dive, self.view) = (None, None);
            self.tour_step(&mut s, new_section, dt);
            self.julia_point(&mut s, settings, bass, level, morph, dt);
        }

        let pulse = (1.0 + s.bass_zoom.clamp(0.0, 1.0) * (bass - 1.0)).clamp(0.5, 2.0);
        s.zoom = view_zoom(s.zoom * pulse * (1.0 + MORPH_PUNCH * morph));
        s
    }

    /// The tour without a dive: gliding to a new spot at each new section.
    fn tour_step(&mut self, s: &mut FractalSettings, new_section: bool, dt: f32) {
        if s.tour {
            if new_section {
                self.next_spot(s);
            }
            let target = *self.target.get_or_insert((s.centre, view_zoom(s.zoom).ln()));
            let at = self.at.get_or_insert(target);
            let k = 1.0 - (-dt.max(0.0) / GLIDE_S).exp();
            at.0 = [at.0[0] + k * (target.0[0] - at.0[0]), at.0[1] + k * (target.0[1] - at.0[1])];
            at.1 += k * (target.1 - at.1);
            s.centre = at.0;
            s.zoom = at.1.exp();
        } else {
            (self.target, self.at) = (None, None);
        }
    }

    /// A Julia set's point along the edge, or a Mandelbulb's power swinging, faster with
    /// the music and kicked by Morph.
    fn julia_point(&mut self, s: &mut FractalSettings, settings: &FractalSettings, bass: f32, level: f32, morph: f32, dt: f32) {
        let shapes = matches!(s.kind, FractalKind::Julia | FractalKind::Mandelbulb);
        if shapes && (s.wander != 0.0 || morph > 0.0 || self.angle.is_some()) {
            let start = if s.kind == FractalKind::Julia { nearest_edge(settings.julia) } else { 0.0 };
            let angle = self.angle.get_or_insert(start);
            *angle = (*angle + (s.wander * LAP * (0.3 + 0.7 * level) + MORPH_LEAP * morph) * dt.max(0.0)).rem_euclid(TAU);
            if s.kind == FractalKind::Julia {
                s.julia = cardioid_edge(*angle, EDGE_OUT + EDGE_BASS * (bass - 1.0).clamp(-0.5, 1.0));
            } else {
                s.power = settings.power + POWER_SWING * angle.sin();
            }
        }
    }

    /// The dive: fly to a point where the picture repeats itself, then go round its loop
    /// forever (in, or backward for out), `rate` the zoom's pace (ln a second).
    fn dive_step(&mut self, s: &mut FractalSettings, new_section: bool, rate: f64, dt: f32) {
        let (kind, julia, dt) = (s.kind, Cx::of(s.julia), f64::from(dt.max(0.0)));
        let start = View { centre: Cx::of(s.centre), half: 1.5 / f64::from(view_zoom(s.zoom)), turn: 0.0, shift: 0.0 };
        let dive = self.dive.get_or_insert_with(|| Dive { point: nearest_dive(kind, s.centre), flight: Some((start, 0.0)), phase: 0.0, base: None });
        if new_section {
            // On to another point, from wherever it is now.
            let n = dive_points(kind);
            if n > 1 {
                dive.point = (dive.point + 1 + (self.rng.next_u64() % (n as u64 - 1)) as usize) % n;
            }
            dive.flight = Some((self.view.unwrap_or(start), 0.0));
        }
        let point = dive_point(kind, dive.point, julia);
        if let Some((count, way)) = surroundings(kind, &point, julia, s.iterations.clamp(1, MAX_ITERATIONS)) {
            let k = 1.0 - (-dt / BASE_S).exp();
            dive.base = Some(dive.base.map_or((count, way), |(b, w)| (b + k * (count - b), w.add(way.sub(w).scale(k)))));
        }
        let (base, lean) = dive.base.unwrap_or((0.0, Cx(0.0, 0.0)));
        let lean = lean.scale(LEAN);
        let widest = loop_half(&point, lean);
        let (scale, turn) = (point.lambda.abs().max(1.01), point.lambda.arg());
        let mut view = None;
        if let Some((from, along)) = &mut dive.flight {
            let flight = Flight::new(from.centre, from.half, point.at.add(lean.scale(widest)), widest);
            *along += rate / RHO * dt;
            if *along < flight.length {
                let q = *along / flight.length;
                let (centre, half) = flight.at(*along);
                view = Some(View { centre, half, turn: from.turn * (1.0 - q), shift: from.shift + (base - from.shift) * q });
            } else {
                dive.phase = 0.0;
            }
        }
        let view = view.unwrap_or_else(|| {
            dive.flight = None;
            let sign = if s.dive < 0.0 { -1.0 } else { 1.0 };
            dive.phase = (dive.phase + sign * rate * dt / scale.ln()).rem_euclid(1.0);
            let (half, turn) = (widest * scale.powf(-dive.phase), -turn * dive.phase);
            View { centre: point.at.add(lean.mul(Cx(turn.cos(), turn.sin())).scale(half)), half, turn, shift: base + point.period * dive.phase }
        });
        self.view = Some(view);
        s.centre = view.centre.f32();
        s.zoom = (1.5 / view.half) as f32;
        s.rotation += view.turn as f32;
        s.shift = view.shift as f32;
    }

    fn next_spot(&mut self, s: &FractalSettings) {
        if s.kind == FractalKind::Julia {
            self.angle = Some(self.rng.next_f32() * TAU);
            return;
        }
        let spots = SPOTS[s.kind.code() as usize];
        if spots.len() > 1 {
            // Any spot but this one.
            let step = 1 + (self.rng.next_u64() % (spots.len() - 1) as u64) as usize;
            self.spot = (self.spot + step) % spots.len();
        }
        let (centre, zoom) = spots[self.spot];
        self.target = Some((centre, zoom.ln()));
    }
}

/// `zoom` as drawn: positive, finite, and no deeper than f32 holds.
pub fn view_zoom(zoom: f32) -> f32 {
    if zoom.is_finite() { zoom.clamp(1e-3, MAX_ZOOM) } else { 1.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_stays_where_f32_holds() {
        assert_eq!(view_zoom(1e9), MAX_ZOOM);
        assert_eq!(view_zoom(f32::NAN), 1.0);
        assert_eq!(view_zoom(0.0), 1e-3);
        assert_eq!(view_zoom(4.0), 4.0);
    }

    /// Both shaders parse and validate (the app would fail to start otherwise).
    #[test]
    fn the_shaders_validate() {
        use wgpu::naga;
        for (name, source) in [("fractal.wgsl", include_str!("fractal.wgsl")), ("bend.wgsl", include_str!("bend.wgsl"))] {
            let module = naga::front::wgsl::parse_str(source).unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(source)));
            naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
                .validate(&module)
                .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(source)));
        }
    }

    #[test]
    fn the_uniforms_match_the_shader() {
        assert_eq!(size_of::<Uniforms>(), 208);
    }

    /// What fractal.wgsl's `escape` and `newton` find at `p`: (escaped, or the root
    /// found; iterations taken). Inside (or no root): (None, `limit`).
    fn iterate(kind: FractalKind, p: [f32; 2], limit: u32) -> (Option<usize>, u32) {
        let mul = |a: [f32; 2], b: [f32; 2]| [a[0] * b[0] - a[1] * b[1], a[0] * b[1] + a[1] * b[0]];
        let mut z = p;
        if kind == FractalKind::Newton {
            let roots = [[1.0, 0.0], [-0.5, 0.866_025_4], [-0.5, -0.866_025_4]];
            for i in 0..limit {
                let z2 = mul(z, z);
                let (num, den) = (mul(z2, z), [3.0 * z2[0], 3.0 * z2[1]]);
                let d = (den[0] * den[0] + den[1] * den[1]).max(1e-20);
                let q = [((num[0] - 1.0) * den[0] + num[1] * den[1]) / d, (num[1] * den[0] - (num[0] - 1.0) * den[1]) / d];
                z = [z[0] - q[0], z[1] - q[1]];
                if let Some(k) = roots.iter().position(|r: &[f32; 2]| (z[0] - r[0]).powi(2) + (z[1] - r[1]).powi(2) < 1e-6) {
                    return (Some(k), i);
                }
            }
            return (None, limit);
        }
        for i in 0..limit {
            match kind {
                FractalKind::BurningShip => z = [z[0].abs(), z[1].abs()],
                FractalKind::Tricorn => z[1] = -z[1],
                _ => {}
            }
            z = mul(z, z);
            z = [z[0] + p[0], z[1] + p[1]];
            if z[0] * z[0] + z[1] * z[1] > 256.0 {
                return (Some(0), i);
            }
        }
        (None, limit)
    }

    /// Every spot on a tour shows the set's edge: some of the set or its basins'
    /// meetings, and not only the set.
    #[test]
    fn the_tour_goes_where_there_is_something_to_see() {
        for kind in FractalKind::ALL.into_iter().filter(|&k| k != FractalKind::Julia && !k.is_3d()) {
            for &(centre, zoom) in SPOTS[kind.code() as usize] {
                let half = 1.5 / zoom;
                let mut found = Vec::new();
                for j in 0..36 {
                    for i in 0..64 {
                        let (x, y) = ((i as f32 + 0.5) / 32.0 - 1.0, (j as f32 + 0.5) / 18.0 - 1.0);
                        found.push(iterate(kind, [centre[0] + x * 16.0 / 9.0 * half, centre[1] + y * half], 300));
                    }
                }
                let share = |f: fn(&(Option<usize>, u32)) -> bool| found.iter().filter(|r| f(r)).count() as f32 / found.len() as f32;
                let at = format!("{kind:?} at {centre:?} ×{zoom}");
                if kind == FractalKind::Newton {
                    let roots: std::collections::HashSet<_> = found.iter().filter_map(|r| r.0).collect();
                    assert!(roots.len() == 3 && share(|r| r.1 >= 8) > 0.1, "{at}");
                } else {
                    let inside = share(|r| r.0.is_none());
                    let slow = share(|r| r.0.is_some() && r.1 >= 15);
                    assert!(inside < 0.9 && (inside > 0.02 || slow > 0.1), "{at}: inside {inside}, slow {slow}");
                }
            }
        }
    }

    fn on(kind: FractalKind) -> FractalSettings {
        FractalSettings { enabled: true, kind, ..FractalSettings::OFF }
    }

    #[test]
    fn with_nothing_asked_the_settings_stand() {
        let mut motion = Motion::default();
        let loud = Music { bass: 2.0, level: 1.0, sections: 3, morph: 0.0 };
        for kind in FractalKind::ALL {
            assert_eq!(motion.apply(&on(kind), &loud, 0.1), on(kind));
        }
        assert_eq!(motion.apply(&FractalSettings::OFF, &loud, 0.1), FractalSettings::OFF);
    }

    #[test]
    fn the_julia_point_wanders_the_edge_faster_when_loud() {
        let s = FractalSettings { wander: 1.0, ..on(FractalKind::Julia) };
        let travelled = |level: f32| {
            let mut motion = Motion::default();
            let music = Music { level, ..Music::QUIET };
            let start = motion.apply(&s, &music, 0.0).julia;
            let end = motion.apply(&s, &music, 1.0).julia;
            // Near the edge throughout.
            let gap = (0..360).map(|k| {
                let p = cardioid_edge(k as f32 * TAU / 360.0, 0.0);
                (p[0] - end[0]).hypot(p[1] - end[1])
            });
            assert!(gap.fold(f32::INFINITY, f32::min) < 0.02, "{end:?}");
            (end[0] - start[0]).hypot(end[1] - start[1])
        };
        let (quiet, loud) = (travelled(0.0), travelled(1.0));
        assert!(quiet > 0.0 && loud > 2.0 * quiet, "quiet {quiet}, loud {loud}");
        // It starts from the look's own point (the edge nearest it).
        let mut motion = Motion::default();
        let first = motion.apply(&s, &Music::QUIET, 0.0).julia;
        assert!((first[0] - s.julia[0]).hypot(first[1] - s.julia[1]) < 0.1, "{first:?}");
    }

    #[test]
    fn the_bass_and_morph_zoom_in() {
        let s = FractalSettings { bass_zoom: 0.5, zoom: 2.0, ..on(FractalKind::Mandelbrot) };
        let mut motion = Motion::default();
        let zoom = |motion: &mut Motion, music: Music| motion.apply(&s, &music, 0.01).zoom;
        assert_eq!(zoom(&mut motion, Music::QUIET), 2.0);
        assert!(zoom(&mut motion, Music { bass: 1.6, ..Music::QUIET }) > 2.5);
        assert!(zoom(&mut motion, Music { morph: 1.0, ..Music::QUIET }) > 2.4);
    }

    #[test]
    fn a_tour_moves_on_at_each_section() {
        let s = FractalSettings { tour: true, ..on(FractalKind::Mandelbrot) };
        let mut motion = Motion::default();
        let music = |sections| Music { sections, ..Music::QUIET };
        // Sections already past when it starts don't count.
        assert_eq!(motion.apply(&s, &music(4), 0.1).centre, s.centre);
        let mut now = motion.apply(&s, &music(5), 0.1);
        for _ in 0..100 {
            now = motion.apply(&s, &music(5), 0.1);
        }
        let spot = SPOTS[0].iter().find(|(c, _)| (c[0] - now.centre[0]).hypot(c[1] - now.centre[1]) < 1e-3);
        let (_, zoom) = spot.unwrap_or_else(|| panic!("not at a spot: {now:?}"));
        assert!((now.zoom / zoom - 1.0).abs() < 1e-3 && now.centre != s.centre);
        // A Julia set's tour moves its point.
        let s = FractalSettings { tour: true, ..on(FractalKind::Julia) };
        let mut motion = Motion::default();
        let before = motion.apply(&s, &music(0), 0.1).julia;
        assert_ne!(motion.apply(&s, &music(1), 0.1).julia, before);
    }

    /// Every dive point loops: one turn of the loop in (zoomed by |λ|, turned by arg λ)
    /// shows the picture it started from, `period` iterations deeper.
    #[test]
    fn every_dive_loops_without_a_seam() {
        let mut points: Vec<(FractalKind, DivePoint, Cx)> = Vec::new();
        // (The 3D kinds' loop is checked on the sponge itself, below.)
        for kind in FractalKind::ALL.into_iter().filter(|k| !k.is_3d()) {
            let juliae = [Cx(-0.8, 0.156), Cx(-0.4, 0.6), Cx(0.285, 0.01)];
            for k in 0..dive_points(kind) {
                for &j in if kind == FractalKind::Julia { &juliae[..] } else { &juliae[..1] } {
                    points.push((kind, dive_point(kind, k, j), j));
                }
            }
        }
        for (kind, point, julia) in points {
            let (mut gaps, mut mismatched, mut total) = (Vec::new(), 0, 0);
            let turn = Cx(point.lambda.abs(), 0.0).div(point.lambda);
            // The canvas as a dive shows it, leaning toward the set.
            let lean = surroundings(kind, &point, julia, 1000).map_or(Cx(0.0, 0.0), |(_, way)| way.scale(LEAN));
            for j in 0..27 {
                for i in 0..48 {
                    let local = Cx(((i as f64 + 0.5) / 24.0 - 1.0) * 16.0 / 9.0, (j as f64 + 0.5) / 13.5 - 1.0).add(lean).scale(loop_half(&point, lean));
                    let start = smooth_count(kind, point.at.add(local), julia, 1000);
                    let end = smooth_count(kind, point.at.add(local.mul(turn).scale(1.0 / point.lambda.abs())), julia, 1000);
                    total += 1;
                    match (start, end) {
                        (Some(a), Some(b)) => gaps.push((b - point.period - a).abs()),
                        (None, None) => {}
                        _ => mismatched += 1,
                    }
                }
            }
            gaps.sort_by(f64::total_cmp);
            let median = gaps.get(gaps.len() / 2).copied().unwrap_or(f64::INFINITY);
            let at = format!("{kind:?} at {:?} (λ {:?}, c {julia:?})", point.at, point.lambda);
            // A quarter of an iteration moves the colours by well under 1 % of the palette
            // at a dive's depths: what would show is shapes that don't line up.
            assert!(median < 0.25 && mismatched * 100 < total, "{at}: median gap {median}, {mismatched} of {total} differ inside/out");
        }
    }

    #[test]
    fn a_flight_runs_from_one_view_to_the_other() {
        for (from, w0, to, w1) in [(Cx(-0.6, 0.0), 1.5, Cx(0.47, 0.35), 0.02), (Cx(0.0, 0.0), 0.04, Cx(0.0, 0.0), 1.0), (Cx(0.45, 0.2), 0.001, Cx(-0.56, 0.68), 0.01)] {
            let f = Flight::new(from, w0, to, w1);
            let (c0, h0) = f.at(0.0);
            let (c1, h1) = f.at(f.length);
            assert!(c0.sub(from).abs() < 1e-9 && (h0 / w0 - 1.0).abs() < 1e-9, "{c0:?} {h0}");
            assert!(c1.sub(to).abs() < 1e-6 * w1.max(1e-3) + 1e-9 && (h1 / w1 - 1.0).abs() < 1e-6, "{c1:?} {h1} vs {to:?} {w1}");
            // Far apart, it zooms out on the way.
            if to.sub(from).abs() > 10.0 * w0.max(w1) {
                assert!(f.at(0.5 * f.length).1 > w0.max(w1), "{}", f.at(0.5 * f.length).1);
            }
        }
    }

    /// A dive's views, a frame at a time: (log zoom, centre, half-height, turn, shift).
    fn dive_frames(s: &FractalSettings, seconds: f32) -> Vec<(f64, Cx, f64, f64, f64)> {
        let mut motion = Motion::default();
        (0..(seconds * 60.0) as usize)
            .map(|_| {
                let v = motion.apply(s, &Music::QUIET, 1.0 / 60.0);
                let half = 1.5 / f64::from(v.zoom);
                (half.recip().ln(), Cx::of(v.centre), half, f64::from(v.rotation), f64::from(v.shift))
            })
            .collect()
    }

    /// Diving in: the view flies in smoothly and then keeps zooming in, jumping back
    /// only where the picture repeats itself (by exactly its loop), forever.
    #[test]
    fn a_dive_travels_in_forever() {
        for kind in FractalKind::ALL.into_iter().filter(|k| k.dives()) {
            for dive in [1.0f32, -1.0] {
                let s = FractalSettings { dive, ..on(kind) };
                let frames = dive_frames(&s, 90.0);
                let point = dive_point(kind, nearest_dive(kind, s.centre), Cx::of(s.julia));
                let (loop_ln, sign) = (point.lambda.abs().ln(), f64::from(dive.signum()));
                let mut wraps = 0;
                for w in frames.windows(2) {
                    let ((z0, c0, h0, t0, s0), (z1, c1, _, t1, s1)) = (w[0], w[1]);
                    let step = z1 - z0;
                    if (step + sign * loop_ln).abs() < 0.05 {
                        // Round the loop: turned and shifted by exactly one turn of it.
                        wraps += 1;
                        let turned = (t1 - t0 - sign * point.lambda.arg()).rem_euclid(TAU as f64);
                        assert!(turned.min(TAU as f64 - turned) < 0.02, "{kind:?}: turned {turned}");
                        assert!((s1 - s0 + sign * point.period).abs() < 0.1, "{kind:?}: shift {s0} to {s1}");
                    } else {
                        // Between, the zoom and the centre move on smoothly.
                        assert!(step.abs() < 0.05, "{kind:?}: zoom leapt {step}");
                        assert!(c1.sub(c0).abs() / h0 < 0.2, "{kind:?}: the centre leapt");
                    }
                }
                // After the flight in (always in), it goes the dive's way, round and round.
                let late: Vec<f64> = frames[frames.len() - 600..].iter().map(|f| f.0).collect();
                let moving = late.windows(2).filter(|w| (w[1] - w[0]) * sign > 0.0).count();
                assert!(moving > 550 && wraps >= 3, "{kind:?} dive {dive}: {moving} frames the right way, {wraps} wraps");
                let (_, centre, half, ..) = *frames.last().unwrap();
                assert!(centre.sub(point.at).abs() <= LEAN * half + 1e-9, "{kind:?}: its point is off the canvas");
            }
        }
    }

    #[test]
    fn a_tour_dives_to_another_point() {
        let s = FractalSettings { dive: 2.0, tour: true, ..on(FractalKind::Mandelbrot) };
        let mut motion = Motion::default();
        let at = |motion: &mut Motion, sections: u32| {
            let mut v = s;
            for _ in 0..60 * 40 {
                v = motion.apply(&s, &Music { sections, ..Music::QUIET }, 1.0 / 60.0);
            }
            Cx::of(v.centre)
        };
        let first = at(&mut motion, 0);
        let second = at(&mut motion, 1);
        // Its point on the canvas (the canvas's half-height there is at most `half`).
        let point = |c: Cx| (0..dive_points(FractalKind::Mandelbrot)).find(|&k| {
            let p = dive_point(FractalKind::Mandelbrot, k, Cx(0.0, 0.0));
            p.at.sub(c).abs() <= LEAN * p.half + 1e-9
        });
        let (a, b) = (point(first), point(second));
        assert!(a.is_some() && b.is_some() && a != b, "{first:?} ({a:?}) then {second:?} ({b:?})");
    }

    /// Each kind as a look shows it (its whole set, and deep in a dive's loop, where the
    /// colours' shift has to keep the background clear) lights part of the canvas: not
    /// none of it, not all of it, nothing blown out or not a number. On a GPU.
    #[test]
    fn a_frame_is_neither_black_nor_blown_out() {
        use crate::render::post::tests::half_float;
        let gpu = match sim3d::GpuContext::new() {
            Ok(g) => g,
            Err(e) => return eprintln!("skipping: {e:#}"),
        };
        let (device, queue) = (&gpu.device, &gpu.queue);
        let size = [160u32, 90];
        let layer = FractalLayer::new(device);
        let palette = crate::render::palette::PALETTES[0].1;
        for kind in FractalKind::ALL {
            let whole = FractalSettings { centre: spots(kind)[0].0, zoom: spots(kind)[0].1, ..on(kind) };
            let mut motion = Motion::default();
            let diving = FractalSettings { dive: 1.0, ..on(kind) };
            let mut deep = diving;
            for _ in 0..60 * 30 {
                deep = motion.apply(&diving, &Music::QUIET, 1.0 / 60.0);
            }
            for (what, settings) in [("whole", whole), ("diving", deep)] {
                let target = super::super::feedback::texture(device, "scene", size, SCENE_FORMAT);
                let mut encoder = device.create_command_encoder(&Default::default());
                super::super::feedback::clear(&mut encoder, &target);
                layer.draw(device, queue, &mut encoder, &target, [0.0, 0.0, size[0] as f32, size[1] as f32], &settings, &palette, None, 0, None);
                queue.submit([encoder.finish()]);
                let pixels = half_float::read(device, queue, target.texture(), size);
                let light = |p: &[f32; 4]| p[0].max(p[1]).max(p[2]);
                assert!(pixels.iter().all(|p| p.iter().all(|c| c.is_finite())), "{kind:?} {what}: not a number");
                let lit = pixels.iter().filter(|p| light(p) > 0.03).count() as f32 / pixels.len() as f32;
                let mut lights: Vec<f32> = pixels.iter().map(light).collect();
                lights.sort_by(f32::total_cmp);
                let (dim, bright) = (lights[lights.len() / 10], lights[lights.len() * 9 / 10]);
                assert!(lights[lights.len() - 1] < 2.0, "{kind:?} {what}: blown out ({})", lights[lights.len() - 1]);
                if kind == FractalKind::Newton && what == "whole" {
                    // Every point finds a root, so the basins tile the plane: lit all
                    // over, but their edges stand out.
                    assert!(bright - dim > 0.2, "{kind:?} {what}: flat, {dim} to {bright}");
                } else {
                    assert!((0.05..0.97).contains(&lit) && bright > 0.1, "{kind:?} {what}: {lit} lit, the brightest tenth {bright}");
                }
            }
        }
    }

    /// fractal.wgsl's Menger sponge distance, `levels` of holes deep, at `q` back from
    /// its corner.
    fn menger(q: glam::Vec3, levels: u32) -> f32 {
        let b = (glam::Vec3::ONE - q).abs() - glam::Vec3::ONE;
        let mut d = b.max(glam::Vec3::ZERO).length() + b.max_element().min(0.0);
        let mut s = 1.0;
        for _ in 0..levels {
            let a = (1.0 - q * s).rem_euclid(glam::Vec3::splat(2.0)) - 1.0;
            s *= 3.0;
            let r = (1.0 - 3.0 * a.abs()).abs();
            let c = (r.x.max(r.y).min(r.y.max(r.z)).min(r.z.max(r.x)) - 1.0) / s;
            d = d.max(c);
        }
        d
    }

    /// fractal.wgsl's sponge grown from its corner, at `p`, grown `grown` times.
    fn grown(p: glam::Vec3, grown: u32) -> f32 {
        let k = 3f32.powi(grown as i32);
        k * menger((glam::Vec3::ONE - p) / k, 9 + grown)
    }

    /// The sponge grown from its corner is itself a third the size about the corner,
    /// everywhere (outside the corner's own cube too: what the dive's camera sees around
    /// it and through its holes), so a Menger dive loops: every point's distance, shrunk
    /// toward the corner by a third, is a third.
    #[test]
    fn the_sponge_repeats_itself_at_its_corner() {
        let corner = glam::Vec3::ONE;
        let mut rng = utils::rng::SplitMix64::new(5);
        let mut worst = 0.0f32;
        for _ in 0..4000 {
            // All round the corner, out to six (twice the dive's farthest camera).
            let p = corner + glam::Vec3::new(rng.signed(), rng.signed(), rng.signed()) * 6.0;
            let shrunk = corner + (p - corner) / 3.0;
            // One more growth on the larger, so both have the same levels at each place.
            worst = worst.max((3.0 * grown(shrunk, 4) - grown(p, 5)).abs());
        }
        assert!(worst < 1e-3, "{worst}");
        // A diving sponge's camera always looks at the corner from outside the cube.
        for k in 0..64 {
            let (eye, forward, ..) = camera_3d(FractalKind::Menger, true, k as f32 * TAU / 64.0, 2.0, None);
            assert!(eye.max_element() > 1.0 && (eye - corner).length() > 1.4 && forward.dot(corner - eye) > 0.0, "{eye} {forward}");
        }
        // The plain sponge's corner region is the grown one's.
        for _ in 0..500 {
            let p = corner - glam::Vec3::new(rng.next_f32(), rng.next_f32(), rng.next_f32()) * 2.0;
            assert!((grown(p, 4) - menger(corner - p, 13)).abs() < 1e-4 || menger(corner - p, 13) > 0.0);
        }
    }

    #[test]
    fn kinds_have_the_shaders_numbers() {
        // fractal.wgsl's MANDELBROT .. NEWTON.
        let codes: Vec<u32> = FractalKind::ALL.iter().map(|k| k.code()).collect();
        assert_eq!(codes, [0, 1, 2, 3, 4, 5, 6]);
    }
}
