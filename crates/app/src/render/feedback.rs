//! The feedback layer, MilkDrop's core trick: each frame starts from the last one,
//! warped and faded, with the new scene combined over it (`feedback.wgsl`). Motion
//! leaves trails, and steady warps grow tunnels and spirals out of whatever is drawn.
//!
//! The warp is the classic one (zoom, rotation, drift, a wave wobble) plus the fluid's
//! own flow: a screen-space flow texture (uv per second) carries the picture along, so
//! trails follow the real motion. In 2D it's drawn from a coarse velocity grid
//! (`flow2d.wgsl`): the solver's own, or particle velocities splatted onto one. In 3D
//! the raymarch writes it, from the velocity of the gas it sees.
//!
//! All rates are per second and scaled by the frame time (in the shader), so a look
//! moves the same at 60 fps and 240.
//!
//! The motion arrives as the equations' shared values (`expr::SHARED`), so a look's
//! per-pixel code (`expr::wgsl`) can vary it across the picture: the shader is a
//! template, built once per per-pixel program and cached.

use super::post::{Echo, SCENE_FORMAT};
use crate::expr::{self, MOTION, SHARED};
use crate::view::View;
use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};
use sim::{FieldLayout, ParticleView, Vec2};
use std::collections::HashMap;

/// The flow texture's format: screen uv per second, x and y.
pub const FLOW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg16Float;
/// Cells up the domain when splatting particle velocities.
const SPLAT_CELLS_Y: f32 = 48.0;
/// The most feedback pipelines kept (one per per-pixel program).
const MAX_PIPELINES: usize = 32;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FeedbackMode {
    /// Blend toward the scene: still things stay put, moving ones leave trails.
    #[default]
    #[serde(alias = "smear")]
    Smear,
    /// The brighter of old and new: bright things streak over dark ones.
    #[serde(alias = "brightest")]
    Brightest,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum EchoFlip {
    #[default]
    #[serde(alias = "none")]
    None,
    #[serde(alias = "horizontal")]
    Horizontal,
    #[serde(alias = "vertical")]
    Vertical,
    #[serde(alias = "both")]
    Both,
}

/// A look's feedback: how the last frame is carried into this one. Rates are per
/// second.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FeedbackSettings {
    pub enabled: bool,
    pub mode: FeedbackMode,
    /// How much of the picture is left after a second, 0 to 0.98.
    pub trail: f32,
    /// Growth per second: 1 none, 1.3 a third bigger each second, below 1 shrinking.
    pub zoom: f32,
    /// Turn, radians per second (anticlockwise).
    pub rotation: f32,
    /// Shift, screen heights per second (x right, y up).
    pub drift: [f32; 2],
    /// Wave wobble: amount (screen heights) and speed (radians per second).
    pub warp: f32,
    pub warp_speed: f32,
    /// How strongly the fluid's flow carries the picture: 1 exactly with it.
    pub flow: f32,
    /// Extra zoom with the bass, 0 to 1.
    pub bass_zoom: f32,
    /// Echo: a zoomed copy over the picture, drawn but not fed back: how much (0 to 1),
    /// its zoom and flip.
    pub echo: f32,
    pub echo_zoom: f32,
    pub echo_flip: EchoFlip,
}

impl FeedbackSettings {
    /// No feedback: the scene as it is. The other values are a start for turning it on.
    pub const OFF: Self = Self {
        enabled: false,
        mode: FeedbackMode::Smear,
        trail: 0.6,
        zoom: 1.0,
        rotation: 0.0,
        drift: [0.0, 0.0],
        warp: 0.0,
        warp_speed: 1.0,
        flow: 1.0,
        bass_zoom: 0.0,
        echo: 0.0,
        echo_zoom: 1.5,
        echo_flip: EchoFlip::None,
    };

    /// The motion these settings make, per second, with the bass at `bass` (0..1): in
    /// `MOTION` order (zoom, rot, dx, dy, cx, cy, sx, sy, warp).
    pub fn motion(&self, bass: f32) -> [f32; MOTION.len()] {
        let zoom = (self.zoom * (1.0 + self.bass_zoom * bass)).max(0.05);
        [zoom, self.rotation, self.drift[0], self.drift[1], 0.5, 0.5, 1.0, 1.0, self.warp]
    }

    /// The shared values for a frame with no equations: the motion, the rest 0.
    pub fn shared(&self, bass: f32) -> [f32; SHARED] {
        let mut shared = [0.0; SHARED];
        let start = expr::slot(MOTION[0]);
        shared[start..start + MOTION.len()].copy_from_slice(&self.motion(bass));
        shared
    }

    /// The shader's values for a frame of `dt` seconds, the wave at `warp_phase`, on a
    /// target `aspect` wide, with the equations' `shared` values (the motion among them).
    fn uniforms(&self, dt: f32, warp_phase: f32, aspect: f32, shared: &[f32; SHARED]) -> Uniforms {
        let mut vars = [[0.0; 4]; 16];
        for (i, v) in shared.iter().enumerate() {
            vars[i / 4][i % 4] = *v;
        }
        Uniforms {
            dt,
            persistence: self.trail.clamp(0.0, 0.98).powf(dt),
            flow: self.flow * dt,
            warp_phase,
            mode: match self.mode {
                FeedbackMode::Smear => 0,
                FeedbackMode::Brightest => 1,
            },
            aspect,
            _pad: [0.0; 2],
            vars,
        }
    }

    /// The echo for the composite (none while feedback is off).
    pub fn echo(&self) -> Echo {
        if !self.enabled {
            return Echo::NONE;
        }
        let flip = match self.echo_flip {
            EchoFlip::None => [1.0, 1.0],
            EchoFlip::Horizontal => [-1.0, 1.0],
            EchoFlip::Vertical => [1.0, -1.0],
            EchoFlip::Both => [-1.0, -1.0],
        };
        Echo { amount: self.echo, zoom: self.echo_zoom.max(0.1), flip }
    }
}

impl Default for FeedbackSettings {
    fn default() -> Self {
        Self::OFF
    }
}

/// Must match `Feedback` in feedback.wgsl (288 bytes).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    dt: f32,
    persistence: f32,
    flow: f32,
    warp_phase: f32,
    mode: u32,
    aspect: f32,
    _pad: [f32; 2],
    vars: [[f32; 4]; 16],
}

const _: () = assert!(SHARED <= 64, "the shared values don't fit the feedback uniforms");

/// The feedback shader with `pixel` (a look's per-pixel code, from `expr::wgsl::pixel`;
/// empty for none) in it.
pub fn shader_source(pixel: &str) -> String {
    fill_template(include_str!("feedback.wgsl"), pixel)
}

/// `template` with its markers replaced. Only the markers: a Git checkout may have
/// turned the file's line endings into `\r\n`, and a marker matched with its `\n` would
/// then be left in, leaving the motion undeclared.
fn fill_template(template: &str, pixel: &str) -> String {
    let motion: String = MOTION.iter().map(|m| format!("    var {} = v({}u);\n", expr::wgsl::motion_var(m), expr::slot(m))).collect();
    let template = template.replace("//{MOTION}", &motion).replace("//{PIXEL}", pixel);
    format!("{}\n{template}", expr::wgsl::MATH)
}

/// Whether `pixel` makes a valid feedback shader (checked without a GPU), with naga's
/// report if not.
pub fn validate_pixel(pixel: &str) -> Result<(), String> {
    use wgpu::naga;
    let source = shader_source(pixel);
    let module = naga::front::wgsl::parse_str(&source).map_err(|e| e.emit_to_string(&source))?;
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
        .validate(&module)
        .map(|_| ())
        .map_err(|e| e.emit_to_string(&source))
}

/// Must match `View2d` in flow2d.wgsl (48 bytes).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FlowUniforms {
    scale: [f32; 2],
    offset: [f32; 2],
    cells: [u32; 2],
    origin: [f32; 2],
    cell: f32,
    _pad: [f32; 3],
}

/// Particle velocities averaged onto a grid over a domain of `size` (cells with no
/// particles are still). For the flow of particle solvers, which have no grid of it.
pub fn splat_velocity(particles: &ParticleView<'_>, size: Vec2, out: &mut Vec<Vec2>) -> FieldLayout {
    let cell = size.y / SPLAT_CELLS_Y;
    let (nx, ny) = (((size.x / cell).ceil() as usize).max(1), SPLAT_CELLS_Y as usize);
    let mut weight = vec![0.0f32; nx * ny];
    out.clear();
    out.resize(nx * ny, Vec2::ZERO);
    for (p, v) in particles.pos.iter().zip(particles.vel) {
        let i = ((p.x / cell) as usize).min(nx - 1);
        let j = ((p.y / cell).max(0.0) as usize).min(ny - 1);
        out[j * nx + i] += *v;
        weight[j * nx + i] += 1.0;
    }
    for (v, w) in out.iter_mut().zip(&weight) {
        if *w > 0.0 {
            *v /= *w;
        }
    }
    FieldLayout { nx, ny, origin: Vec2::ZERO, cell }
}

struct Targets {
    size: [u32; 2],
    /// Ping-pong: the last frame is read while the next is written.
    frames: [wgpu::TextureView; 2],
    flow: wgpu::TextureView,
}

pub struct FeedbackLayer {
    layout: wgpu::BindGroupLayout,
    /// One pipeline per per-pixel program (by its code; "" for none).
    pipelines: HashMap<String, wgpu::RenderPipeline>,
    uniforms: wgpu::Buffer,
    flow_layout: wgpu::BindGroupLayout,
    flow_pipeline: wgpu::RenderPipeline,
    flow_uniforms: wgpu::Buffer,
    sampler: wgpu::Sampler,
    targets: Option<Targets>,
    /// The frame holding the latest picture.
    current: usize,
    /// The last frame doesn't belong to this picture (just made, or feedback was off):
    /// start from the scene.
    stale: bool,
    warp_phase: f32,
    /// The 2D velocity grid and its size.
    velocity: Option<(wgpu::Texture, wgpu::TextureView, [u32; 2])>,
}

impl FeedbackLayer {
    pub fn new(device: &wgpu::Device) -> Self {
        let fragment = wgpu::ShaderStages::FRAGMENT;
        let uniform = |binding, size: usize| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: fragment,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(size as u64),
            },
            count: None,
        };
        let texture = |binding, filterable| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: fragment,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("feedback"),
            entries: &[
                uniform(0, size_of::<Uniforms>()),
                texture(1, true),
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: fragment, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                texture(3, true),
                texture(4, true),
            ],
        });
        let flow_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("flow 2d"),
            entries: &[uniform(0, size_of::<FlowUniforms>()), texture(1, false)],
        });
        let pipeline = fullscreen_pipeline(device, &layout, &shader_source(""), "fs_feedback", SCENE_FORMAT);
        let flow_pipeline = fullscreen_pipeline(device, &flow_layout, include_str!("flow2d.wgsl"), "fs_flow2d", FLOW_FORMAT);
        let buffer = |label, size: usize| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("feedback"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            layout,
            pipelines: HashMap::from([(String::new(), pipeline)]),
            uniforms: buffer("feedback", size_of::<Uniforms>()),
            flow_layout,
            flow_pipeline,
            flow_uniforms: buffer("flow 2d", size_of::<FlowUniforms>()),
            sampler,
            targets: None,
            current: 0,
            stale: true,
            warp_phase: 0.0,
            velocity: None,
        }
    }

    fn targets(&mut self, device: &wgpu::Device, size: [u32; 2]) -> &Targets {
        if self.targets.as_ref().is_none_or(|t| t.size != size) {
            let frame = || texture(device, "feedback frame", size, SCENE_FORMAT);
            self.targets = Some(Targets { size, frames: [frame(), frame()], flow: texture(device, "flow", size, FLOW_FORMAT) });
            self.stale = true;
        }
        self.targets.as_ref().expect("just made")
    }

    /// The flow texture for this frame, `size` pixels (the scene's): the 3D raymarch
    /// writes it, `draw_flow_2d` in 2D.
    pub fn flow_target(&mut self, device: &wgpu::Device, size: [u32; 2]) -> wgpu::TextureView {
        self.targets(device, size).flow.clone()
    }

    /// Draw the 2D flow from a velocity grid (`velocity` laid out as `layout`), seen
    /// through `view`; with no grid, a still flow.
    pub fn draw_flow_2d(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        size: [u32; 2],
        view: &View,
        grid: Option<(&[Vec2], FieldLayout)>,
    ) {
        let flow = self.targets(device, size).flow.clone();
        let Some((velocity, layout)) = grid.filter(|(v, l)| !v.is_empty() && v.len() == l.nx * l.ny) else {
            clear(encoder, &flow);
            return;
        };
        let cells = [layout.nx as u32, layout.ny as u32];
        if self.velocity.as_ref().is_none_or(|v| v.2 != cells) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("velocity grid"),
                size: wgpu::Extent3d { width: cells[0], height: cells[1], depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rg32Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            self.velocity = Some((texture, view, cells));
        }
        let (texture, grid_view, _) = self.velocity.as_ref().expect("just made");
        queue.write_texture(
            texture.as_image_copy(),
            bytemuck::cast_slice(velocity),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(cells[0] * 8), rows_per_image: None },
            wgpu::Extent3d { width: cells[0], height: cells[1], depth_or_array_layers: 1 },
        );
        let uniforms = FlowUniforms {
            scale: view.scale.into(),
            offset: view.offset.into(),
            cells,
            origin: layout.origin.into(),
            cell: layout.cell,
            _pad: [0.0; 3],
        };
        queue.write_buffer(&self.flow_uniforms, 0, bytemuck::bytes_of(&uniforms));
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("flow 2d"),
            layout: &self.flow_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.flow_uniforms.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(grid_view) },
            ],
        });
        fullscreen(encoder, "flow 2d", &flow, &self.flow_pipeline, &group);
    }

    /// Feedback is off: the next time it's on, start from the scene.
    pub fn pause(&mut self) {
        self.stale = true;
    }

    /// Carry the last frame into this one and combine `scene` over it; returns the new
    /// picture (for the glow and composite). `dt` is the frame time (0 holds the picture
    /// still), `shared` the equations' values (the motion among them: see
    /// `FeedbackSettings::shared` for none), `pixel` the look's per-pixel code (empty for
    /// none; already validated).
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        scene: &wgpu::TextureView,
        size: [u32; 2],
        settings: &FeedbackSettings,
        shared: &[f32; SHARED],
        pixel: &str,
        dt: f32,
    ) -> wgpu::TextureView {
        if !self.pipelines.contains_key(pixel) {
            // Editing per-pixel code live makes a version per keystroke: keep the cache to
            // the built-ins' worth (and the plain one).
            if self.pipelines.len() >= MAX_PIPELINES {
                self.pipelines.retain(|code, _| code.is_empty());
            }
            let pipeline = fullscreen_pipeline(device, &self.layout, &shader_source(pixel), "fs_feedback", SCENE_FORMAT);
            self.pipelines.insert(pixel.to_string(), pipeline);
        }
        let current = self.current;
        let t = self.targets(device, size);
        let (previous, next) = (t.frames[current].clone(), t.frames[1 - current].clone());
        let flow = t.flow.clone();
        if std::mem::take(&mut self.stale) {
            encoder.copy_texture_to_texture(scene.texture().as_image_copy(), previous.texture().as_image_copy(), previous.texture().size());
        }
        self.warp_phase = (self.warp_phase + settings.warp_speed * dt) % (std::f32::consts::TAU * 100.0);
        let aspect = size[0] as f32 / size[1].max(1) as f32;
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&settings.uniforms(dt, self.warp_phase, aspect, shared)));
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("feedback"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniforms.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&previous) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(scene) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&flow) },
            ],
        });
        fullscreen(encoder, "feedback", &next, &self.pipelines[pixel], &group);
        self.current = 1 - self.current;
        next
    }
}

pub(super) fn texture(device: &wgpu::Device, label: &str, size: [u32; 2], format: wgpu::TextureFormat) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: size[0].max(1), height: size[1].max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

pub(super) fn fullscreen_pipeline(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, source: &str, entry: &str, format: wgpu::TextureFormat) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(entry), source: wgpu::ShaderSource::Wgsl(source.into()) });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some(entry), bind_group_layouts: &[Some(layout)], immediate_size: 0 });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(entry),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
        fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some(entry), compilation_options: Default::default(), targets: &[Some(format.into())] }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}

pub(super) fn fullscreen(encoder: &mut wgpu::CommandEncoder, label: &str, target: &wgpu::TextureView, pipeline: &wgpu::RenderPipeline, group: &wgpu::BindGroup) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
        })],
        ..Default::default()
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, group, &[]);
    pass.draw(0..3, 0..1);
}

/// Clear `target` to transparent black.
pub fn clear(encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("clear"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
        })],
        ..Default::default()
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The template makes a valid shader whatever its line endings (a Windows checkout
    /// can turn them into `\r\n`), with per-pixel code or without.
    #[test]
    fn the_template_works_with_windows_line_endings() {
        let crlf = include_str!("feedback.wgsl").replace("\r\n", "\n").replace('\n', "\r\n");
        let program = expr::compile("zoom = zoom + 0.1 * rad", expr::Stage::Pixel).unwrap();
        let pixel = expr::wgsl::pixel(&program).replace('\n', "\r\n");
        for code in ["", pixel.as_str()] {
            let source = fill_template(&crlf, code);
            assert!(!source.contains("//{"), "a marker was left in");
            let module = wgpu::naga::front::wgsl::parse_str(&source).unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
            wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(), wgpu::naga::valid::Capabilities::empty())
                .validate(&module)
                .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
        }
    }

    /// The fade is per second: two half-frames fade as much as one whole one, and a
    /// paused frame holds the picture. (The motion is scaled in the shader.)
    #[test]
    fn the_fade_is_independent_of_the_frame_rate() {
        let s = FeedbackSettings { enabled: true, trail: 0.3, ..FeedbackSettings::OFF };
        let shared = s.shared(0.0);
        let (whole, half) = (s.uniforms(1.0 / 60.0, 0.0, 1.5, &shared), s.uniforms(1.0 / 120.0, 0.0, 1.5, &shared));
        assert!((whole.persistence - half.persistence * half.persistence).abs() < 1e-6);
        assert_eq!(s.uniforms(0.0, 0.0, 1.5, &shared).persistence, 1.0, "a paused frame should hold the picture");
    }

    /// The settings' motion lands in its slots; the bass adds zoom.
    #[test]
    fn the_motion_fills_its_slots() {
        let s = FeedbackSettings { enabled: true, zoom: 1.2, rotation: 0.3, drift: [0.1, -0.2], warp: 0.01, bass_zoom: 0.5, ..FeedbackSettings::OFF };
        let shared = s.shared(0.0);
        let at = |name| shared[expr::slot(name)];
        assert_eq!([at("zoom"), at("rot"), at("dx"), at("dy"), at("cx"), at("sx"), at("warp")], [1.2, 0.3, 0.1, -0.2, 0.5, 1.0, 0.01]);
        assert!(s.shared(1.0)[expr::slot("zoom")] > at("zoom"));
        let u = s.uniforms(0.1, 0.0, 1.0, &shared);
        assert_eq!(u.vars[expr::slot("rot") / 4][expr::slot("rot") % 4], 0.3);
    }

    /// The template builds valid shaders, with and without per-pixel code.
    #[test]
    fn the_shader_validates() {
        validate_pixel("").unwrap();
        let p = expr::compile("zoom = zoom + 0.2 * rad * sin(ang * 4 + time); rot = rot * (1 - rad) + q1", expr::Stage::Pixel).unwrap();
        if let Err(e) = validate_pixel(&expr::wgsl::pixel(&p)) {
            panic!("{e}");
        }
    }

    #[test]
    fn particle_velocities_average_per_cell() {
        let pos = [Vec2::new(0.005, 0.005), Vec2::new(0.012, 0.01), Vec2::new(1.99, 0.99)];
        let vel = [Vec2::new(1.0, 0.0), Vec2::new(3.0, 2.0), Vec2::new(0.0, -1.0)];
        let mut out = Vec::new();
        let layout = splat_velocity(&ParticleView { pos: &pos, vel: &vel, radius: 0.01 }, Vec2::new(2.0, 1.0), &mut out);
        assert_eq!((layout.ny, out.len()), (48, layout.nx * layout.ny));
        assert_eq!(out[0], Vec2::new(2.0, 1.0), "two particles in the first cell average");
        assert_eq!(out[out.len() - 1], Vec2::new(0.0, -1.0));
        assert_eq!(out.iter().filter(|v| **v != Vec2::ZERO).count(), 2);
    }

    /// On a headless GPU: with no motion and full persistence the picture holds; a
    /// zoom pushes a bright dot outward; and the 'brightest' mode keeps a dot that has
    /// left the scene while 'smear' fades it.
    #[test]
    fn the_feedback_pass_warps_and_fades() {
        let gpu = match sim3d::GpuContext::new() {
            Ok(g) => g,
            Err(e) => return eprintln!("skipping: {e:#}"),
        };
        let (device, queue) = (&gpu.device, &gpu.queue);
        let size = [64u32, 64];
        let mut layer = FeedbackLayer::new(device);
        let scene = texture(device, "scene", size, SCENE_FORMAT);
        let upload = |dot: Option<(u32, u32)>| {
            let pixels: Vec<[u16; 4]> = (0..size[1])
                .flat_map(|y| (0..size[0]).map(move |x| (x, y)))
                .map(|p| if Some(p) == dot { [0x3C00; 4] } else { [0, 0, 0, 0x3C00] })
                .collect();
            queue.write_texture(
                scene.texture().as_image_copy(),
                bytemuck::cast_slice(&pixels),
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size[0] * 8), rows_per_image: None },
                scene.texture().size(),
            );
        };
        let frame = |layer: &mut FeedbackLayer, s: &FeedbackSettings, dt: f32| {
            let mut encoder = device.create_command_encoder(&Default::default());
            let view = View::fit_in(Vec2::ONE, Vec2::ZERO, Vec2::splat(64.0), 0.0, winit::dpi::PhysicalSize::new(64, 64));
            layer.draw_flow_2d(device, queue, &mut encoder, size, &view, None);
            let out = layer.run(device, queue, &mut encoder, &scene, size, s, &s.shared(0.0), "", dt);
            queue.submit([encoder.finish()]);
            super::super::post::tests::read_red(device, queue, out.texture(), size)
        };
        let at = |img: &[f32], x: u32, y: u32| img[(y * size[0] + x) as usize];

        // A dot at (40, 32), then gone from the scene: the brightest mode keeps it.
        upload(Some((40, 32)));
        let keep = FeedbackSettings { enabled: true, mode: FeedbackMode::Brightest, trail: 0.98, ..FeedbackSettings::OFF };
        frame(&mut layer, &keep, 1.0 / 60.0);
        upload(None);
        let kept = frame(&mut layer, &keep, 1.0 / 60.0);
        assert!(at(&kept, 40, 32) > 0.9, "the dot faded at once: {}", at(&kept, 40, 32));
        let smear = FeedbackSettings { mode: FeedbackMode::Smear, trail: 0.1, ..keep };
        let mut faded = kept.clone();
        for _ in 0..30 {
            faded = frame(&mut layer, &smear, 1.0 / 60.0);
        }
        assert!(at(&faded, 40, 32) < 0.5, "the smear didn't fade the gone dot: {}", at(&faded, 40, 32));

        // Zoom: a dot right of centre drifts outward (right) over the frames.
        layer.pause();
        upload(Some((40, 32)));
        let zoom = FeedbackSettings { enabled: true, mode: FeedbackMode::Brightest, trail: 0.98, zoom: 3.0, ..FeedbackSettings::OFF };
        let mut img = Vec::new();
        for _ in 0..20 {
            img = frame(&mut layer, &zoom, 1.0 / 30.0);
        }
        let right_of_dot: f32 = (44..60).map(|x| at(&img, x, 32)).sum();
        let left_of_dot: f32 = (20..36).map(|x| at(&img, x, 32)).sum();
        assert!(right_of_dot > 2.0 && left_of_dot < 0.5, "zoom streaks: right {right_of_dot}, left {left_of_dot}");
    }
}
