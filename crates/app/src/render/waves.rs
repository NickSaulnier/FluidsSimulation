//! Waves and shapes, MilkDrop's drawn elements: the sound itself (an oscilloscope, a
//! ring, spectrum bars, a left-against-right figure) and simple polygons, drawn onto the
//! scene before the feedback, so the feedback streaks them into tunnels and trails.
//!
//! Geometry is built on the CPU each frame as triangles (thick lines are a quad per
//! segment) in canvas units: the canvas is 2 high, centred on the origin, x across. It
//! draws additively at `brightness` (light beyond white), so the bloom makes it glow.
//!
//! The waveform is scaled by its own recent peak, so quiet and loud music both fill the
//! shapes; silence stays flat.

use super::Palette;
use super::post::SCENE_FORMAT;
use crate::audio::analyzer::Frame;
use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};
use sim::{ForceBrush, Vec2};
use std::f32::consts::TAU;

/// Points drawn from the waveform (the newest of what the analyzer keeps).
const WAVE_POINTS: usize = 512;
/// The waveform's peak follows loud passages at once and falls back over this time
/// constant, seconds; below `QUIETEST` it isn't scaled up (silence stays flat).
const PEAK_FALL_S: f32 = 2.0;
const QUIETEST: f32 = 0.02;
/// The circle's last stretch blends into its start, so the loop has no seam.
const SEAM: usize = 32;
/// Brushes placed along a pushing wave, their reach (a fraction of the domain's height),
/// push (m/s at full amplitude and full `push`), and blend rate (1/s).
const PUSH_BRUSHES: usize = 12;
const PUSH_REACH: f32 = 0.07;
const PUSH_SPEED: f32 = 3.0;
const PUSH_STRENGTH: f32 = 20.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WaveMode {
    #[default]
    Off,
    /// An oscilloscope across the screen.
    Line,
    /// The oscilloscope bent into a ring.
    Circle,
    /// Spectrum bars along the bottom.
    Spectrum,
    /// Spectrum bars up and down from a centre line.
    MirrorSpectrum,
    /// Left against right, turned 45° so mono is a vertical line and width spreads it.
    Xy,
}

impl WaveMode {
    pub const ALL: [WaveMode; 6] = [WaveMode::Off, WaveMode::Line, WaveMode::Circle, WaveMode::Spectrum, WaveMode::MirrorSpectrum, WaveMode::Xy];

    pub fn label(self) -> &'static str {
        match self {
            WaveMode::Off => "Off",
            WaveMode::Line => "Line",
            WaveMode::Circle => "Circle",
            WaveMode::Spectrum => "Spectrum",
            WaveMode::MirrorSpectrum => "Mirrored spectrum",
            WaveMode::Xy => "Left vs right",
        }
    }
}

/// A polygon: spinning, pulsing with the bass, outlined and optionally filled.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShapeSettings {
    pub enabled: bool,
    pub sides: u32,
    /// Radius, in canvas half-heights.
    pub radius: f32,
    /// Centre, in canvas half-heights from the middle (x right, y up).
    pub centre: [f32; 2],
    /// Turn, radians per second.
    pub spin: f32,
    /// Outline width, pixels (0: no outline).
    pub thickness: f32,
    /// How much the radius follows the bass (MilkDrop's `bass_att`), 0 to 1.
    pub bass_pulse: f32,
    /// Fill opacity, 0 (outline only) to 1.
    pub fill: f32,
}

impl ShapeSettings {
    pub const OFF: Self =
        Self { enabled: false, sides: 6, radius: 0.3, centre: [0.0, 0.0], spin: 0.3, thickness: 3.0, bass_pulse: 0.3, fill: 0.0 };
}

impl Default for ShapeSettings {
    fn default() -> Self {
        Self::OFF
    }
}

/// A look's waves and shapes.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WaveSettings {
    pub mode: WaveMode,
    /// How far the sound moves the wave, 0 to 2.
    pub scale: f32,
    /// The circle's radius, or the spectrum bars' height, in canvas half-heights.
    pub size: f32,
    /// Where the line or the mirrored spectrum sits, in half-heights from the middle.
    pub y: f32,
    /// Line width, pixels.
    pub thickness: f32,
    /// Light added, 0 to 4 (above 1 glows).
    pub brightness: f32,
    /// How hard a line or circle pushes the fluid with the sound, 0 to 1.
    pub push: f32,
    pub shapes: [ShapeSettings; 2],
}

impl WaveSettings {
    pub const OFF: Self =
        Self { mode: WaveMode::Off, scale: 1.0, size: 0.5, y: 0.0, thickness: 3.0, brightness: 1.5, push: 0.0, shapes: [ShapeSettings::OFF; 2] };

    /// Whether anything is drawn.
    pub fn any(&self) -> bool {
        self.mode != WaveMode::Off || self.shapes.iter().any(|s| s.enabled)
    }
}

impl Default for WaveSettings {
    fn default() -> Self {
        Self::OFF
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub struct Vertex {
    pos: [f32; 2],
    colour: [f32; 4],
}

/// What the waves are drawn from this frame.
pub struct Sound<'a> {
    /// The latest stereo frames, oldest first.
    pub waveform: &'a [Frame],
    /// The log-spaced spectrum, 0..1.
    pub spectrum: &'a [f32],
    /// MilkDrop's smoothed bass (1 is typical).
    pub bass: f32,
}

/// The newest `WAVE_POINTS` frames as (left, right, mono), scaled by `gain`.
fn points(waveform: &[Frame], gain: f32) -> impl Iterator<Item = (f32, f32, f32)> + '_ {
    let start = waveform.len().saturating_sub(WAVE_POINTS);
    waveform[start..].iter().map(move |&[l, r]| (l * gain, r * gain, 0.5 * (l + r) * gain))
}

/// The scale that brings a waveform with this `peak` to full size (none in silence).
fn gain(peak: f32) -> f32 {
    1.0 / peak.max(QUIETEST)
}

/// Build the frame's triangles into `out` (replacing it), for a canvas `aspect` wide
/// and `height_px` tall. `peak` is the waveform's recent peak, `time` the waves' clock.
#[allow(clippy::too_many_arguments)]
pub fn geometry(settings: &WaveSettings, sound: &Sound<'_>, peak: f32, time: f32, aspect: f32, height_px: f32, palette: &Palette, out: &mut Vec<Vertex>) {
    out.clear();
    let [_, base, highlight] = palette.linear();
    let tint = |c: [f32; 4], k: f32| [c[0] * k, c[1] * k, c[2] * k, 1.0];
    let colour = tint(highlight, settings.brightness);
    let px = 2.0 / height_px.max(1.0); // canvas units per pixel
    let half = 0.5 * settings.thickness * px;
    let g = gain(peak);
    let mut path: Vec<[f32; 2]> = Vec::with_capacity(WAVE_POINTS + 1);
    match settings.mode {
        WaveMode::Off => {}
        WaveMode::Line => {
            let n = sound.waveform.len().clamp(2, WAVE_POINTS);
            path.extend(points(sound.waveform, g).enumerate().map(|(k, (_, _, m))| {
                [aspect * (2.0 * k as f32 / (n - 1) as f32 - 1.0), settings.y + 0.5 * settings.scale * m]
            }));
            polyline(out, &path, half, colour, false);
        }
        WaveMode::Circle => {
            let values: Vec<f32> = points(sound.waveform, g).map(|(_, _, m)| m).collect();
            let n = values.len();
            path.extend((0..n).map(|k| {
                // Blend the last stretch toward the start, so the ends meet.
                let mut m = values[k];
                if n > SEAM && k >= n - SEAM {
                    let t = (k - (n - SEAM)) as f32 / SEAM as f32;
                    m = m * (1.0 - t) + values[0] * t;
                }
                let r = settings.size * (1.0 + 0.5 * settings.scale * m);
                let a = TAU * k as f32 / n.max(1) as f32;
                [r * a.cos(), r * a.sin()]
            }));
            polyline(out, &path, half, colour, true);
        }
        WaveMode::Xy => {
            let k = 0.5 * settings.scale * std::f32::consts::FRAC_1_SQRT_2;
            path.extend(points(sound.waveform, g).map(|(l, r, _)| [k * (l - r), k * (l + r)]));
            polyline(out, &path, half, colour, false);
        }
        WaveMode::Spectrum | WaveMode::MirrorSpectrum => {
            let bars = sound.spectrum.len().max(1);
            let width = 2.0 * aspect / bars as f32;
            let mirror = settings.mode == WaveMode::MirrorSpectrum;
            for (k, &level) in sound.spectrum.iter().enumerate() {
                let height = settings.size * settings.scale * level;
                let x = -aspect + k as f32 * width;
                let (x0, x1) = (x + 0.1 * width, x + 0.9 * width);
                let (y0, y1) = if mirror { (settings.y - height, settings.y + height) } else { (-1.0, -1.0 + 2.0 * height) };
                quad(out, [[x0, y0], [x1, y0], [x1, y1], [x0, y1]], colour);
            }
        }
    }
    for shape in settings.shapes.iter().filter(|s| s.enabled) {
        let sides = shape.sides.clamp(3, 64) as usize;
        let r = shape.radius * (1.0 + shape.bass_pulse * (sound.bass - 1.0)).max(0.0);
        let turn = shape.spin * time;
        let c = shape.centre;
        let corners: Vec<[f32; 2]> = (0..sides)
            .map(|k| {
                let a = turn + TAU * k as f32 / sides as f32;
                [c[0] + r * a.cos(), c[1] + r * a.sin()]
            })
            .collect();
        if shape.fill > 0.0 {
            let fill = tint(base, settings.brightness * shape.fill);
            for k in 0..sides {
                out.extend([c, corners[k], corners[(k + 1) % sides]].map(|pos| Vertex { pos, colour: fill }));
            }
        }
        if shape.thickness > 0.0 {
            polyline(out, &corners, 0.5 * shape.thickness * px, tint(base, settings.brightness), true);
        }
    }
    // Canvas units to NDC: x over the aspect.
    for v in out.iter_mut() {
        v.pos[0] /= aspect;
    }
}

/// A thick line through `points`: a quad per segment, `half` wide either side.
fn polyline(out: &mut Vec<Vertex>, points: &[[f32; 2]], half: f32, colour: [f32; 4], closed: bool) {
    let n = points.len();
    let segments = if closed { n } else { n.saturating_sub(1) };
    for k in 0..segments {
        let (a, b) = (Vec2::from(points[k]), Vec2::from(points[(k + 1) % n]));
        let normal = (b - a).perp().normalize_or_zero() * half;
        quad(out, [(a - normal).into(), (b - normal).into(), (b + normal).into(), (a + normal).into()], colour);
    }
}

fn quad(out: &mut Vec<Vertex>, [a, b, c, d]: [[f32; 2]; 4], colour: [f32; 4]) {
    out.extend([a, b, c, a, c, d].map(|pos| Vertex { pos, colour }));
}

/// Brushes along a pushing line or circle, for a domain of `size` (the 2D world, or the
/// 3D volume's front view): each pushes away from the wave's rest position (out of the
/// ring, up or down from the line) as far as the sound has moved it there.
pub fn wave_brushes(settings: &WaveSettings, waveform: &[Frame], peak: f32, size: Vec2, out: &mut Vec<ForceBrush>) {
    if settings.push <= 0.0 || !matches!(settings.mode, WaveMode::Line | WaveMode::Circle) {
        return;
    }
    let values: Vec<f32> = points(waveform, gain(peak)).map(|(_, _, m)| m).collect();
    if values.is_empty() {
        return;
    }
    let centre = 0.5 * size;
    let speed = PUSH_SPEED * settings.push * settings.scale;
    for k in 0..PUSH_BRUSHES {
        let m = values[k * values.len() / PUSH_BRUSHES];
        let (pos, direction) = if settings.mode == WaveMode::Circle {
            let a = TAU * k as f32 / PUSH_BRUSHES as f32;
            let out = Vec2::from_angle(a);
            (centre + out * settings.size * 0.5 * size.y, out)
        } else {
            let x = (k as f32 + 0.5) / PUSH_BRUSHES as f32 * size.x;
            (Vec2::new(x, centre.y + settings.y * 0.5 * size.y), Vec2::Y)
        };
        out.push(ForceBrush { pos, radius: PUSH_REACH * size.y, velocity: direction * speed * m, strength: PUSH_STRENGTH, spin: 0.0 });
    }
}

/// Draws the waves into the scene texture, keeping the waveform's peak and the clock.
pub struct WavesLayer {
    pipeline: wgpu::RenderPipeline,
    vertices: Option<wgpu::Buffer>,
    scratch: Vec<Vertex>,
    peak: f32,
    time: f32,
}

impl WavesLayer {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("waves.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("waves.wgsl").into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("waves"), bind_group_layouts: &[], immediate_size: 0 });
        let add = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("waves"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: SCENE_FORMAT,
                    blend: Some(wgpu::BlendState { color: add, alpha: add }),
                    write_mask: wgpu::ColorWrites::COLOR,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        Self { pipeline, vertices: None, scratch: Vec::new(), peak: 0.0, time: 0.0 }
    }

    /// The waveform's recent peak (for scaling pushes the same way as the drawing).
    pub fn peak(&self) -> f32 {
        self.peak
    }

    /// Follow the waveform's peak and advance the clock by `dt` (call every frame).
    pub fn update(&mut self, waveform: &[Frame], dt: f32) {
        let loudest = points(waveform, 1.0).map(|(l, r, _)| l.abs().max(r.abs())).fold(0.0, f32::max);
        self.peak = loudest.max(self.peak * (-dt / PEAK_FALL_S).exp());
        self.time += dt;
    }

    /// Add the waves onto `target` within `viewport` (pixels: x, y, width, height).
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        viewport: [f32; 4],
        settings: &WaveSettings,
        sound: &Sound<'_>,
        palette: &Palette,
    ) {
        let [x, y, w, h] = viewport;
        if !settings.any() || w < 1.0 || h < 1.0 {
            return;
        }
        let mut scratch = std::mem::take(&mut self.scratch);
        geometry(settings, sound, self.peak, self.time, w / h, h, palette, &mut scratch);
        let bytes: &[u8] = bytemuck::cast_slice(&scratch);
        if !bytes.is_empty() {
            if self.vertices.as_ref().is_none_or(|b| b.size() < bytes.len() as u64) {
                self.vertices = Some(device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("waves"),
                    size: (bytes.len() as u64).next_power_of_two(),
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
            }
            let buffer = self.vertices.as_ref().expect("just made");
            queue.write_buffer(buffer, 0, bytes);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("waves"),
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
            pass.set_vertex_buffer(0, buffer.slice(..bytes.len() as u64));
            pass.draw(0..scratch.len() as u32, 0..1);
        }
        self.scratch = scratch;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(n: usize, amplitude: f32, right: f32) -> Vec<Frame> {
        (0..n).map(|k| (TAU * k as f32 / 64.0).sin() * amplitude).map(|s| [s, s * right]).collect()
    }

    fn build(settings: &WaveSettings, waveform: &[Frame], spectrum: &[f32]) -> Vec<Vertex> {
        let mut out = Vec::new();
        let sound = Sound { waveform, spectrum, bass: 1.0 };
        geometry(settings, &sound, 0.5, 0.0, 1.5, 600.0, &Palette::default(), &mut out);
        out
    }

    #[test]
    fn nothing_when_off() {
        assert!(build(&WaveSettings::OFF, &sine(1024, 0.5, 1.0), &[0.5; 64]).is_empty());
        assert!(!WaveSettings::OFF.any());
    }

    /// The circle is a closed ring of quads around the centre, within its swing.
    #[test]
    fn the_circle_is_a_closed_ring() {
        let s = WaveSettings { mode: WaveMode::Circle, size: 0.5, scale: 1.0, ..WaveSettings::OFF };
        let v = build(&s, &sine(1024, 0.5, 1.0), &[]);
        assert_eq!(v.len(), WAVE_POINTS * 6, "a quad per point, closing the loop");
        for p in &v {
            let r = Vec2::new(p.pos[0] * 1.5, p.pos[1]).length();
            assert!((0.2..=0.8).contains(&r), "a point at radius {r}");
        }
    }

    /// Mono audio in the left-against-right view is a vertical line; opposite channels a
    /// horizontal one.
    #[test]
    fn the_xy_view_shows_width() {
        let s = WaveSettings { mode: WaveMode::Xy, thickness: 0.0, ..WaveSettings::OFF };
        let mono = build(&s, &sine(1024, 0.5, 1.0), &[]);
        assert!(mono.iter().all(|p| p.pos[0].abs() < 1e-4), "mono should be vertical");
        let wide = build(&s, &sine(1024, 0.5, -1.0), &[]);
        assert!(wide.iter().all(|p| p.pos[1].abs() < 1e-4), "opposite channels should be horizontal");
    }

    /// One bar per spectrum band, as tall as its level.
    #[test]
    fn spectrum_bars_follow_the_levels() {
        let s = WaveSettings { mode: WaveMode::Spectrum, size: 0.5, scale: 1.0, ..WaveSettings::OFF };
        let levels: Vec<f32> = (0..64).map(|k| k as f32 / 63.0).collect();
        let v = build(&s, &[], &levels);
        assert_eq!(v.len(), 64 * 6);
        let top = |bar: usize| v[bar * 6..bar * 6 + 6].iter().map(|p| p.pos[1]).fold(f32::MIN, f32::max);
        assert!((top(0) + 1.0).abs() < 1e-5 && (top(63) - 0.0).abs() < 1e-5, "bars from {} to {}", top(0), top(63));
    }

    /// A shape is a filled fan plus an outline, and grows with the bass.
    #[test]
    fn shapes_fill_outline_and_pulse() {
        let shape = ShapeSettings { enabled: true, sides: 5, radius: 0.4, fill: 0.5, bass_pulse: 1.0, ..ShapeSettings::OFF };
        let s = WaveSettings { shapes: [shape, ShapeSettings::OFF], ..WaveSettings::OFF };
        assert!(s.any());
        let radius = |bass: f32| {
            let mut out = Vec::new();
            geometry(&s, &Sound { waveform: &[], spectrum: &[], bass }, 0.5, 0.0, 1.0, 600.0, &Palette::default(), &mut out);
            assert_eq!(out.len(), 5 * 3 + 5 * 6);
            out[1].pos[0].hypot(out[1].pos[1])
        };
        assert!((radius(1.0) - 0.4).abs() < 1e-5);
        assert!(radius(1.5) > 0.55, "should pulse with the bass");
    }

    /// Silence stays flat (it isn't scaled up), and a quiet wave fills like a loud one.
    #[test]
    fn the_wave_scales_to_its_peak_but_not_in_silence() {
        let s = WaveSettings { mode: WaveMode::Line, scale: 1.0, thickness: 0.0, ..WaveSettings::OFF };
        let height = |amplitude: f32| {
            let wave = sine(1024, amplitude, 1.0);
            let mut out = Vec::new();
            let sound = Sound { waveform: &wave, spectrum: &[], bass: 1.0 };
            geometry(&s, &sound, amplitude, 0.0, 1.5, 600.0, &Palette::default(), &mut out);
            out.iter().map(|p| p.pos[1].abs()).fold(0.0, f32::max)
        };
        assert!((height(0.05) - height(0.8)).abs() < 1e-3, "quiet {} vs loud {}", height(0.05), height(0.8));
        assert!(height(0.001) < 0.03, "silence was scaled up: {}", height(0.001));
    }

    /// A pushing circle places brushes on its ring, pushing out where the wave swings out.
    #[test]
    fn a_pushing_circle_pushes_outward() {
        let s = WaveSettings { mode: WaveMode::Circle, push: 1.0, size: 0.5, ..WaveSettings::OFF };
        let wave: Vec<Frame> = vec![[0.5, 0.5]; 1024];
        let size = Vec2::new(2.0, 1.0);
        let mut brushes = Vec::new();
        wave_brushes(&s, &wave, 0.5, size, &mut brushes);
        assert_eq!(brushes.len(), PUSH_BRUSHES);
        for b in &brushes {
            let out = (b.pos - 0.5 * size).normalize();
            assert!(b.velocity.dot(out) > 1.0, "brush at {} pushes {}", b.pos, b.velocity);
            assert!(((b.pos - 0.5 * size).length() - 0.25).abs() < 1e-4);
        }
        let mut none = Vec::new();
        wave_brushes(&WaveSettings { push: 0.0, ..s }, &wave, 0.5, size, &mut none);
        assert!(none.is_empty());
    }
}
