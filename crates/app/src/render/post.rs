//! Post-processing (`post.wgsl`): the scene is drawn into an offscreen HDR texture at a
//! render scale, then bloomed and composited onto the screen through a tone curve.
//!
//! The tone curve leaves colours up to `TONE_KNEE` (0.85, linear) alone, so the 2D
//! scenes look as they did when they drew straight to the screen; only highlights above
//! it (fire, glows, bloom) roll off toward white (Reinhard-style) instead of clipping.
//!
//! For blends between looks it keeps a still of the picture (`capture`): the composite
//! drawn a second time into one of two textures, so the still being blended from is
//! never the one being written.

use super::composite::{self, Composite};
use super::timer::{GpuTimer, Stage};
use bytemuck::{Pod, Zeroable};

/// The scene texture's format: linear, and able to hold light brighter than white.
pub const SCENE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Bloom starts at this linear brightness, fading in over `KNEE` either side.
const THRESHOLD: f32 = 0.8;
const KNEE: f32 = 0.3;
/// At most this many half-size levels, and none smaller than `MIN_LEVEL` pixels.
const MAX_LEVELS: usize = 7;
const MIN_LEVEL: u32 = 8;
/// Above this many pixels (1440p) the automatic render scale draws the scene smaller.
const AUTO_FULL_PIXELS: u32 = 2560 * 1440;
const AUTO_SCALE: f32 = 0.75;

/// HDR output's levels: the paper white (what the picture's white is shown at) and the
/// display's peak, nits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HdrLevels {
    pub white: f32,
    pub peak: f32,
}

/// An HDR surface's format (scRGB: linear, 1 is 80 nits).
pub const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const SCRGB_NITS: f32 = 80.0;

/// How the composite writes its target: dithered (8 bits), the brightest it goes in
/// paper whites, the scale to the target's units, the motion blur's shutter (seconds),
/// whether it enlarges the scene.
#[derive(Clone, Copy, Debug)]
struct Output {
    dither: bool,
    top: f32,
    scale: f32,
    blur: f32,
    upscale: bool,
}

/// Must match `Post` in post.wgsl (176 bytes).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    threshold: f32,
    knee: f32,
    strength: f32,
    echo: f32,
    echo_zoom: f32,
    aspect: f32,
    echo_flip: [f32; 2],
    symmetry: u32,
    segments: f32,
    turn: f32,
    hue: f32,
    saturation: f32,
    contrast: f32,
    exposure: f32,
    palette_map: f32,
    invert: f32,
    aberration: f32,
    vignette: f32,
    grain: f32,
    seed: f32,
    blend: f32,
    blend_pattern: u32,
    blend_seed: f32,
    /// 1 to dither (for an 8-bit target), 0 not.
    dither: f32,
    top: f32,
    scale: f32,
    blur: f32,
    upscale: f32,
    _pad: [f32; 3],
    deep: [f32; 4],
    base: [f32; 4],
    highlight: [f32; 4],
}

impl Uniforms {
    fn new(strength: f32, echo: Echo, aspect: f32, c: &Composite, out: Output) -> Self {
        let s = &c.settings;
        Self {
            threshold: THRESHOLD,
            knee: KNEE,
            strength,
            echo: echo.amount,
            echo_zoom: echo.zoom,
            aspect,
            echo_flip: echo.flip,
            symmetry: s.symmetry.index(),
            segments: s.segments.clamp(2, 16) as f32,
            turn: c.clock.turn,
            hue: (s.hue + c.clock.hue_drift).to_radians(),
            saturation: s.saturation.max(0.0),
            contrast: s.contrast.max(0.0),
            exposure: s.exposure.max(0.0),
            palette_map: s.palette_map.clamp(0.0, 1.0),
            invert: s.invert.clamp(0.0, 1.0),
            aberration: s.aberration.clamp(0.0, 0.1),
            vignette: s.vignette.clamp(0.0, 1.0),
            grain: s.grain.clamp(0.0, 1.0),
            seed: c.clock.seed,
            blend: c.blend.amount.clamp(0.0, 1.0),
            blend_pattern: c.blend.pattern.index(),
            blend_seed: c.blend.seed,
            dither: f32::from(u8::from(out.dither)),
            top: out.top,
            scale: out.scale,
            blur: out.blur,
            upscale: f32::from(u8::from(out.upscale)),
            _pad: [0.0; 3],
            deep: composite::linear(c.palette.deep),
            base: composite::linear(c.palette.base),
            highlight: composite::linear(c.palette.highlight),
        }
    }
}

/// The echo the composite draws: how much (0 for none), its zoom, its flip as signs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Echo {
    pub amount: f32,
    pub zoom: f32,
    pub flip: [f32; 2],
}

impl Echo {
    pub const NONE: Self = Self { amount: 0.0, zoom: 1.0, flip: [1.0, 1.0] };
}

/// The render scale to draw the scene at for a `size` surface: `setting` if it's
/// positive, otherwise automatic (full size up to 1440p, three quarters above).
pub fn render_scale(setting: f32, size: [u32; 2]) -> f32 {
    if setting > 0.0 {
        setting
    } else if size[0] * size[1] > AUTO_FULL_PIXELS {
        AUTO_SCALE
    } else {
        1.0
    }
}

/// The scene texture's size for a `size` surface at `scale`.
pub fn scene_size(size: [u32; 2], scale: f32) -> [u32; 2] {
    size.map(|v| ((v as f32 * scale).round() as u32).max(1))
}

struct Targets {
    size: [u32; 2],
    scene: wgpu::TextureView,
    /// Half size, quarter size... The first holds the finished bloom.
    levels: Vec<wgpu::TextureView>,
    /// Bind groups: `down[i]` reads level i (writing i + 1); `up[i]` reads level i + 1
    /// (adding onto i). The prefilter's and composite's are made each frame, since
    /// their source changes (the scene, or the feedback's latest frame).
    down: Vec<wgpu::BindGroup>,
    up: Vec<wgpu::BindGroup>,
}

pub struct PostProcess {
    layout: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    /// The composite's settings for an SDR target while the output is HDR (for grabs).
    uniforms_sdr: wgpu::Buffer,
    shader: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    sampler: wgpu::Sampler,
    /// Stands in for the bloom texture in the passes that don't read it.
    blank: wgpu::TextureView,
    prefilter: wgpu::RenderPipeline,
    down: wgpu::RenderPipeline,
    up: wgpu::RenderPipeline,
    /// The composite into the SDR format (the surface's without HDR, and grabs').
    composite: wgpu::RenderPipeline,
    targets: Option<Targets>,
    /// The SDR format.
    output: wgpu::TextureFormat,
    /// HDR output: its levels, and the composite into `HDR_FORMAT`.
    hdr: Option<(HdrLevels, wgpu::RenderPipeline)>,
    /// This frame's flow (for the motion blur), and its time.
    flow: Option<wgpu::TextureView>,
    frame_dt: f32,
    /// Stills of the picture for blends (made when first captured), and which one shows.
    stills: [Option<(wgpu::TextureView, [u32; 2])>; 2],
    still: usize,
}

impl PostProcess {
    /// `output`: the format the composite writes (the surface's).
    pub fn new(device: &wgpu::Device, output: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("post.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("post.wgsl").into()),
        });
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(size_of::<Uniforms>() as u64),
                    },
                    count: None,
                },
                texture(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                texture(3),
                texture(4),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("post"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry: &str, format: wgpu::TextureFormat, blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })],
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let add = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add };
        let buffer = |label| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size_of::<Uniforms>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let (uniforms, uniforms_sdr) = (buffer("post uniforms"), buffer("post uniforms (sdr)"));
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("post"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let blank = texture_2d(device, "blank", [1, 1]);
        Self {
            prefilter: pipeline("fs_prefilter", SCENE_FORMAT, None),
            down: pipeline("fs_down", SCENE_FORMAT, None),
            up: pipeline("fs_up", SCENE_FORMAT, Some(wgpu::BlendState { color: add, alpha: add })),
            composite: pipeline("fs_composite", output, None),
            output,
            hdr: None,
            flow: None,
            frame_dt: 0.0,
            stills: [None, None],
            still: 0,
            layout,
            uniforms,
            uniforms_sdr,
            shader,
            pipeline_layout,
            sampler,
            blank,
            targets: None,
        }
    }

    /// Write HDR output at `levels` (to an `HDR_FORMAT` target), or SDR (`None`). A change
    /// drops the stills (they're in the other's units).
    pub fn set_hdr(&mut self, device: &wgpu::Device, levels: Option<HdrLevels>) {
        match (levels, &mut self.hdr) {
            (Some(l), Some((now, _))) => *now = l,
            (Some(l), None) => {
                let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("fs_composite (hdr)"),
                    layout: Some(&self.pipeline_layout),
                    vertex: wgpu::VertexState { module: &self.shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                    fragment: Some(wgpu::FragmentState {
                        module: &self.shader,
                        entry_point: Some("fs_composite"),
                        compilation_options: Default::default(),
                        targets: &[Some(HDR_FORMAT.into())],
                    }),
                    primitive: Default::default(),
                    depth_stencil: None,
                    multisample: Default::default(),
                    multiview_mask: None,
                    cache: None,
                });
                self.hdr = Some((l, pipeline));
                self.stills = [None, None];
            }
            (None, Some(_)) => {
                self.hdr = None;
                self.stills = [None, None];
            }
            (None, None) => {}
        }
    }

    /// This frame's flow (screen uv per second, the scene's size), for the motion blur
    /// (none: no blur), and the frame's time.
    pub fn set_flow(&mut self, flow: Option<&wgpu::TextureView>, frame_dt: f32) {
        self.flow = flow.cloned();
        self.frame_dt = frame_dt;
    }

    /// The format the composite writes the surface in.
    fn surface_format(&self) -> wgpu::TextureFormat {
        if self.hdr.is_some() { HDR_FORMAT } else { self.output }
    }

    /// The texture to draw the scene into, `size` pixels (made or remade as needed).
    pub fn scene_target(&mut self, device: &wgpu::Device, size: [u32; 2]) -> wgpu::TextureView {
        if self.targets.as_ref().is_none_or(|t| t.size != size) {
            self.targets = Some(self.make_targets(device, size));
        }
        self.targets.as_ref().expect("just made").scene.clone()
    }

    fn make_targets(&self, device: &wgpu::Device, size: [u32; 2]) -> Targets {
        let scene = texture_2d(device, "scene", size);
        let mut levels = Vec::new();
        let mut level = size.map(|v| (v / 2).max(1));
        while levels.len() < MAX_LEVELS && level[0].min(level[1]) >= MIN_LEVEL {
            levels.push(texture_2d(device, "bloom level", level));
            level = level.map(|v| (v / 2).max(1));
        }
        if levels.is_empty() {
            levels.push(texture_2d(device, "bloom level", size.map(|v| (v / 2).max(1))));
        }
        let down = levels.windows(2).map(|w| self.group(device, &w[0], &self.blank, &self.blank)).collect();
        let up = levels.windows(2).map(|w| self.group(device, &w[1], &self.blank, &self.blank)).collect();
        Targets { size, scene, levels, down, up }
    }

    fn group(&self, device: &wgpu::Device, src: &wgpu::TextureView, bloom: &wgpu::TextureView, still: &wgpu::TextureView) -> wgpu::BindGroup {
        self.group_with(device, &self.uniforms, src, bloom, still)
    }

    fn group_with(&self, device: &wgpu::Device, uniforms: &wgpu::Buffer, src: &wgpu::TextureView, bloom: &wgpu::TextureView, still: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("post"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(src) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(bloom) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(still) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(self.flow.as_ref().unwrap_or(&self.blank)) },
            ],
        })
    }

    /// Record the bloom of `source` (skipped at `strength` 0) and the composite of it,
    /// with `echo` and `composite`'s look, into `output`. `source` is the scene texture
    /// (after drawing into `scene_target`), or the feedback's latest frame, at the same
    /// size.
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::TextureView,
        output: &wgpu::TextureView,
        strength: f32,
        echo: Echo,
        composite: &Composite,
        timer: Option<&GpuTimer>,
    ) {
        let t = self.targets.as_ref().expect("scene_target first");
        let aspect = t.size[0] as f32 / t.size[1].max(1) as f32;
        // Only a target of 8 bits a channel bands (a float one, in tests, doesn't).
        let dither = matches!(self.output.remove_srgb_suffix(), wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm);
        let blur = if self.flow.is_some() { composite.settings.motion_blur.clamp(0.0, 1.0) * self.frame_dt } else { 0.0 };
        let target = output.texture().size();
        let upscale = t.size[0] < target.width || t.size[1] < target.height;
        let sdr = Output { dither, top: 1.0, scale: 1.0, blur, upscale };
        let out = match &self.hdr {
            Some((l, _)) => Output { dither: false, top: (l.peak / l.white).max(1.0), scale: l.white / SCRGB_NITS, ..sdr },
            None => sdr,
        };
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&Uniforms::new(strength, echo, aspect, composite, out)));
        queue.write_buffer(&self.uniforms_sdr, 0, bytemuck::bytes_of(&Uniforms::new(strength, echo, aspect, composite, sdr)));
        if strength > 0.0 {
            let prefilter = self.group(device, source, &self.blank, &self.blank);
            fullscreen(encoder, "bloom prefilter", &t.levels[0], false, &self.prefilter, &prefilter);
            for (k, group) in t.down.iter().enumerate() {
                fullscreen(encoder, "bloom down", &t.levels[k + 1], false, &self.down, group);
            }
            for (k, group) in t.up.iter().enumerate().rev() {
                fullscreen(encoder, "bloom up", &t.levels[k], true, &self.up, group);
            }
        }
        if let Some(timer) = timer {
            timer.mark(encoder, Stage::Composite);
        }
        let composite = self.group(device, source, &t.levels[0], self.still_view());
        let pipeline = self.hdr.as_ref().map_or(&self.composite, |(_, p)| p);
        fullscreen(encoder, "composite", output, false, pipeline, &composite);
    }

    /// The still a blend shows the old picture from (a stand-in before any capture).
    fn still_view(&self) -> &wgpu::TextureView {
        self.stills[self.still].as_ref().map_or(&self.blank, |(view, _)| view)
    }

    /// Keep this frame's picture, `size` pixels (the window's), as the still to blend
    /// from: the composite of `source` drawn again, with what the last `run` set up.
    pub fn capture(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, source: &wgpu::TextureView, size: [u32; 2]) {
        let next = 1 - self.still;
        if self.stills[next].as_ref().is_none_or(|(_, s)| *s != size) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("still"),
                size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.surface_format(),
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            self.stills[next] = Some((texture.create_view(&Default::default()), size));
        }
        let Some(t) = &self.targets else { return };
        let (target, _) = self.stills[next].as_ref().expect("just made");
        let composite = self.group(device, source, &t.levels[0], self.still_view());
        let pipeline = self.hdr.as_ref().map_or(&self.composite, |(_, p)| p);
        fullscreen(encoder, "capture", target, false, pipeline, &composite);
        self.still = next;
    }

    /// The composite of `source` again, into `output` (the surface's format, or the SDR
    /// one while the output is HDR: grabs are SDR), with what the last `run` set up (its
    /// glow and settings): for thumbnails, screenshots, recordings.
    pub fn composite_again(&self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, source: &wgpu::TextureView, output: &wgpu::TextureView) {
        let Some(t) = &self.targets else { return };
        match &self.hdr {
            Some((_, pipeline)) if output.texture().format() == HDR_FORMAT => {
                let composite = self.group(device, source, &t.levels[0], self.still_view());
                fullscreen(encoder, "composite again", output, false, pipeline, &composite);
            }
            Some(_) => {
                // (The blend's still is in HDR units: an SDR grab shows the new look.)
                let composite = self.group_with(device, &self.uniforms_sdr, source, &t.levels[0], &self.blank);
                fullscreen(encoder, "composite again (sdr)", output, false, &self.composite, &composite);
            }
            None => {
                let composite = self.group(device, source, &t.levels[0], self.still_view());
                fullscreen(encoder, "composite again", output, false, &self.composite, &composite);
            }
        }
    }
}

fn texture_2d(device: &wgpu::Device, label: &str, size: [u32; 2]) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SCENE_FORMAT,
            // (Copies: for tests and, later, screenshots.)
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

/// One full-target triangle into `target`, keeping its contents when `keep` (for the
/// additive upsample) or replacing them.
fn fullscreen(encoder: &mut wgpu::CommandEncoder, label: &str, target: &wgpu::TextureView, keep: bool, pipeline: &wgpu::RenderPipeline, group: &wgpu::BindGroup) {
    let load = if keep { wgpu::LoadOp::Load } else { wgpu::LoadOp::Clear(wgpu::Color::BLACK) };
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
        })],
        ..Default::default()
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, group, &[]);
    pass.draw(0..3, 0..1);
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The red channel of an `Rgba16Float` texture, read back (for render tests).
    pub fn read_red(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture, size: [u32; 2]) -> Vec<f32> {
        half_float::read(device, queue, texture, size).iter().map(|p| p[0]).collect()
    }

    /// The composite's look on a headless GPU: off it leaves colours alone; the mirror
    /// and the kaleidoscope fold the picture; the grading, palette map, inversion and
    /// vignette do what they say.
    #[test]
    fn the_composite_folds_and_grades() {
        use crate::render::composite::{CompositeClock, CompositeSettings, Symmetry};
        let gpu = match sim3d::GpuContext::new() {
            Ok(g) => g,
            Err(e) => return eprintln!("skipping: {e:#}"),
        };
        let (device, queue) = (&gpu.device, &gpu.queue);
        let mut post = PostProcess::new(device, SCENE_FORMAT);
        let size = [64u32, 64];
        let scene = post.scene_target(device, size);
        let palette = crate::render::palette::PALETTES[0].1;
        let render = |pixels: &dyn Fn(u32, u32) -> [f32; 3], settings: CompositeSettings| -> Vec<[f32; 4]> {
            let data: Vec<half_float::F16x4> = (0..size[1]).flat_map(|y| (0..size[0]).map(move |x| (x, y))).map(|(x, y)| half_float::F16x4::rgb(pixels(x, y))).collect();
            queue.write_texture(
                scene.texture().as_image_copy(),
                bytemuck::cast_slice(&data),
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size[0] * 8), rows_per_image: None },
                wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
            );
            let output = texture_2d(device, "output", size);
            let mut encoder = device.create_command_encoder(&Default::default());
            let composite = Composite { settings, clock: CompositeClock::default(), palette, blend: super::composite::Blend::NONE };
            post.run(device, queue, &mut encoder, &scene, &output, 0.0, Echo::NONE, &composite, None);
            queue.submit([encoder.finish()]);
            half_float::read(device, queue, output.texture(), size)
        };
        let at = |img: &[[f32; 4]], x: u32, y: u32| img[(y * size[0] + x) as usize];
        let close = |a: f32, b: f32, tolerance: f32| (a - b).abs() <= tolerance;
        let off = CompositeSettings::OFF;

        let colour = render(&|_, _| [0.6, 0.2, 0.1], off);
        assert!(colour.iter().all(|p| close(p[0], 0.6, 2e-3) && close(p[1], 0.2, 2e-3) && close(p[2], 0.1, 2e-3)), "off changed {:?}", colour[0]);

        let dot = |x, y| if (x, y) == (10, 20) { [0.8; 3] } else { [0.1; 3] };
        let mirrored = render(&dot, CompositeSettings { symmetry: Symmetry::MirrorX, ..off });
        assert!(close(at(&mirrored, 10, 20)[0], 0.8, 0.01) && close(at(&mirrored, 53, 20)[0], 0.8, 0.01), "no mirror: {:?} {:?}", at(&mirrored, 10, 20), at(&mirrored, 53, 20));

        // An image that changes with the angle round the centre: the kaleidoscope repeats
        // it every 60°, and mirrors it within each wedge.
        let swirl = |x: u32, y: u32| {
            let (dx, dy) = (x as f32 + 0.5 - 32.0, y as f32 + 0.5 - 32.0);
            [0.1 + 0.4 * (0.5 + 0.5 * (dy.atan2(dx) * 1.3).sin()), 0.2, 0.2]
        };
        let kaleido = render(&swirl, CompositeSettings { symmetry: Symmetry::Kaleidoscope, segments: 6, ..off });
        let polar = |deg: f32| {
            let a = deg.to_radians();
            at(&kaleido, (32.0 + 20.0 * a.cos()).round() as u32, (32.0 + 20.0 * a.sin()).round() as u32)[0]
        };
        for deg in [10.0, 25.0, 40.0] {
            for k in 1..6 {
                assert!(close(polar(deg), polar(deg + 60.0 * k as f32), 0.03), "{deg}° and {}° differ: {} {}", deg + 60.0 * k as f32, polar(deg), polar(deg + 60.0 * k as f32));
            }
            assert!(close(polar(deg), polar(60.0 - deg), 0.03), "not mirrored within the wedge at {deg}°");
        }

        let grey = render(&|_, _| [0.6, 0.2, 0.1], CompositeSettings { saturation: 0.0, ..off });
        let g = grey[0];
        assert!(close(g[0], g[1], 2e-3) && close(g[1], g[2], 2e-3), "saturation 0 isn't grey: {g:?}");
        let turned = render(&|_, _| [0.5, 0.0, 0.0], CompositeSettings { hue: 120.0, ..off })[0];
        // In OKLCH: green, as light as the red was.
        let lightness = |c: [f32; 4]| crate::render::palette::oklab([c[0], c[1], c[2]])[0];
        assert!(turned[1] > 4.0 * turned[0].max(turned[2]) && close(lightness(turned), lightness([0.5, 0.0, 0.0, 1.0]), 0.03), "red turned 120° isn't green: {turned:?}");
        let inverted = render(&|_, _| [0.2; 3], CompositeSettings { invert: 1.0, ..off })[0];
        assert!(close(inverted[0], 0.8, 2e-3), "0.2 inverted: {inverted:?}");
        let vignetted = render(&|_, _| [0.5; 3], CompositeSettings { vignette: 1.0, ..off });
        assert!(close(at(&vignetted, 32, 32)[0], 0.5, 2e-3) && at(&vignetted, 0, 0)[0] < 0.2, "vignette: centre {:?}, corner {:?}", at(&vignetted, 32, 32), at(&vignetted, 0, 0));
        // (A bright white: 1.0 itself tone-maps to 0.925, a little short of the top.)
        let ramp = render(&|x, _| [if x < 32 { 0.0 } else { 50.0 }; 3], CompositeSettings { palette_map: 1.0, ..off });
        let highlight = crate::render::composite::linear(palette.highlight);
        let white = at(&ramp, 60, 10);
        assert!(at(&ramp, 4, 10)[0] < 1e-3 && (0..3).all(|c| close(white[c], highlight[c], 0.02)), "palette map: black {:?}, white {white:?} (highlight {highlight:?})", at(&ramp, 4, 10));

        // The tone curve keeps a bright colour's hue (an orange stays orange, not
        // yellow), and turns a very bright one toward white.
        let hue = |c: [f32; 4]| {
            let [_, a, b] = crate::render::palette::oklab([c[0], c[1], c[2]]);
            b.atan2(a).to_degrees()
        };
        let orange = [1.5, 0.5, 0.1];
        let toned = render(&|_, _| orange, off)[0];
        assert!(toned[0] < 1.0 && (hue(toned) - hue([orange[0], orange[1], orange[2], 1.0])).abs() < 4.0, "orange toned to {toned:?}");
        let blazing = render(&|_, _| [12.0, 4.0, 0.8], off)[0];
        assert!(blazing[2] > 0.5 && blazing[0] <= 1.0, "a blazing orange toned to {blazing:?}");
    }

    /// Into an 8-bit target the composite dithers: a dark gradient comes out without the
    /// terraces 8 bits would give it, and black stays black.
    #[test]
    fn dithering_smooths_a_dark_gradient() {
        let gpu = match sim3d::GpuContext::new() {
            Ok(g) => g,
            Err(e) => return eprintln!("skipping: {e:#}"),
        };
        let (device, queue) = (&gpu.device, &gpu.queue);
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let mut post = PostProcess::new(device, format);
        let size = [256u32, 64];
        let scene = post.scene_target(device, size);
        let data: Vec<half_float::F16x4> = (0..size[1])
            .flat_map(|y| (0..size[0]).map(move |x| (x, y)))
            .map(|(x, _)| half_float::F16x4::grey(if x < 16 { 0.0 } else { 0.002 + 0.01 * x as f32 / 256.0 }))
            .collect();
        queue.write_texture(
            scene.texture().as_image_copy(),
            bytemuck::cast_slice(&data),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size[0] * 8), rows_per_image: None },
            wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
        );
        let mut grabber = crate::render::grab::Grabber::default();
        let view = grabber.begin(device, size, format, ()).expect("a free slot");
        let mut encoder = device.create_command_encoder(&Default::default());
        post.run(device, queue, &mut encoder, &scene, &view, 0.0, Echo::NONE, &Composite::NONE, None);
        grabber.end(&mut encoder);
        queue.submit([encoder.finish()]);
        let mut image = None;
        for _ in 0..100 {
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
            if let Some((_, i)) = grabber.poll(device).pop() {
                image = Some(i);
                break;
            }
        }
        let image = image.expect("the frame came back");
        let banding = crate::render::score::fidelity(&image).banding;
        let black = (0..size[1] as usize).all(|y| (0..12).all(|x| image.rgba[(y * size[0] as usize + x) * 4..][..3] == [0, 0, 0]));
        assert!(banding < 0.25 && black, "banding {banding}, black kept {black}");
    }

    /// Blends on a headless GPU: a captured still shows at the start and the new picture
    /// at the end, half and half dissolved midway; the radial pattern brings the new look
    /// in from the middle; and a still captured mid-blend holds the blend as it was.
    #[test]
    fn blends_mix_in_the_captured_still() {
        use crate::render::composite::{Blend, BlendPattern};
        let gpu = match sim3d::GpuContext::new() {
            Ok(g) => g,
            Err(e) => return eprintln!("skipping: {e:#}"),
        };
        let (device, queue) = (&gpu.device, &gpu.queue);
        let mut post = PostProcess::new(device, SCENE_FORMAT);
        let size = [64u32, 64];
        let scene = post.scene_target(device, size);
        let fill = |rgb: [f32; 3]| {
            let data = vec![half_float::F16x4::rgb(rgb); (size[0] * size[1]) as usize];
            queue.write_texture(
                scene.texture().as_image_copy(),
                bytemuck::cast_slice(&data),
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size[0] * 8), rows_per_image: None },
                wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
            );
        };
        let draw = |post: &mut PostProcess, blend: Blend, capture: bool| {
            let output = texture_2d(device, "output", size);
            let mut encoder = device.create_command_encoder(&Default::default());
            post.run(device, queue, &mut encoder, &scene, &output, 0.0, Echo::NONE, &Composite { blend, ..Composite::NONE }, None);
            if capture {
                post.capture(device, &mut encoder, &scene, size);
            }
            queue.submit([encoder.finish()]);
            half_float::read(device, queue, output.texture(), size)
        };
        let at = |img: &[[f32; 4]], x: u32, y: u32| img[(y * size[0] + x) as usize];
        let close = |p: [f32; 4], rgb: [f32; 3]| (0..3).all(|c| (p[c] - rgb[c]).abs() < 0.02);
        let blend = |amount, pattern| Blend { amount, pattern, seed: 0.0 };

        fill([0.5, 0.0, 0.0]);
        draw(&mut post, Blend::NONE, true);
        fill([0.0, 0.5, 0.0]);
        let start = draw(&mut post, blend(0.0, BlendPattern::Dissolve), false);
        assert!(close(at(&start, 32, 32), [0.5, 0.0, 0.0]), "the start isn't the still: {:?}", at(&start, 32, 32));
        let end = draw(&mut post, blend(1.0, BlendPattern::Dissolve), false);
        assert!(close(at(&end, 32, 32), [0.0, 0.5, 0.0]), "the end isn't the new look: {:?}", at(&end, 32, 32));
        let half = draw(&mut post, blend(0.5, BlendPattern::Dissolve), false);
        assert!(close(at(&half, 5, 50), [0.25, 0.25, 0.0]), "midway: {:?}", at(&half, 5, 50));
        let radial = draw(&mut post, blend(0.5, BlendPattern::Radial), false);
        assert!(close(at(&radial, 32, 32), [0.0, 0.5, 0.0]) && close(at(&radial, 0, 0), [0.5, 0.0, 0.0]), "radial: centre {:?}, corner {:?}", at(&radial, 32, 32), at(&radial, 0, 0));

        // Captured halfway through a dissolve, then blended from at the start.
        draw(&mut post, blend(0.5, BlendPattern::Dissolve), true);
        fill([0.0, 0.0, 0.5]);
        let held = draw(&mut post, blend(0.0, BlendPattern::Dissolve), false);
        assert!(close(at(&held, 32, 32), [0.25, 0.25, 0.0]), "the mid-blend still: {:?}", at(&held, 32, 32));
    }

    #[test]
    fn the_automatic_scale_draws_4k_smaller() {
        assert_eq!(render_scale(0.0, [1920, 1080]), 1.0);
        assert_eq!(render_scale(0.0, [3840, 2160]), AUTO_SCALE);
        assert_eq!(render_scale(0.5, [3840, 2160]), 0.5);
        assert_eq!(scene_size([3840, 2160], 0.75), [2880, 1620]);
    }

    /// The composite and bloom on a headless GPU: ordinary colours pass unchanged,
    /// bright ones roll off below white, and a bright dot glows onto its surroundings
    /// while a dim scene doesn't glow at all.
    #[test]
    fn composite_keeps_colours_and_blooms_highlights() {
        let gpu = match sim3d::GpuContext::new() {
            Ok(g) => g,
            Err(e) => return eprintln!("skipping: {e:#}"),
        };
        let (device, queue) = (&gpu.device, &gpu.queue);
        let mut post = PostProcess::new(device, SCENE_FORMAT);
        let size = [64u32, 64];
        let scene = post.scene_target(device, size);
        let render = |pixels: &dyn Fn(u32, u32) -> f32, strength: f32| -> Vec<[f32; 4]> {
            let data: Vec<half_float::F16x4> = (0..size[1]).flat_map(|y| (0..size[0]).map(move |x| (x, y))).map(|(x, y)| half_float::F16x4::grey(pixels(x, y))).collect();
            let texture = scene.texture();
            queue.write_texture(
                texture.as_image_copy(),
                bytemuck::cast_slice(&data),
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size[0] * 8), rows_per_image: None },
                wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
            );
            let output = texture_2d(device, "output", size);
            let mut encoder = device.create_command_encoder(&Default::default());
            post.run(device, queue, &mut encoder, &scene, &output, strength, Echo::NONE, &Composite::NONE, None);
            queue.submit([encoder.finish()]);
            half_float::read(device, queue, output.texture(), size)
        };
        let flat = render(&|_, _| 0.5, 0.4);
        assert!(flat.iter().all(|p| (p[0] - 0.5).abs() < 2e-3), "0.5 changed: {:?}", flat[0]);
        let bright = render(&|_, _| 4.0, 0.0);
        assert!(bright.iter().all(|p| p[0] > 0.95 && p[0] < 1.0), "4.0 became {:?}", bright[0]);
        let dot = |x, y| if (x, y) == (32, 32) { 50.0 } else { 0.1 };
        let glowing = render(&dot, 0.4);
        let near = glowing[(32 * size[0] + 36) as usize][0];
        assert!(near > 0.12, "no glow 4 px from a bright dot: {near}");
        let dim = render(&|_, _| 0.1, 0.4);
        assert!(dim.iter().all(|p| (p[0] - 0.1).abs() < 2e-3), "a dim scene glowed: {:?}", dim[0]);
    }

    /// Half floats for the test's uploads and readbacks.
    /// The composite of a grey `scene` (`size`, a value per pixel) into a float target
    /// `out` pixels, with `post` set up as the caller likes.
    fn composite_of(device: &wgpu::Device, queue: &wgpu::Queue, post: &mut PostProcess, size: [u32; 2], out: [u32; 2], scene: &dyn Fn(u32, u32) -> f32) -> Vec<[f32; 4]> {
        let target = post.scene_target(device, size);
        let data: Vec<half_float::F16x4> = (0..size[1]).flat_map(|y| (0..size[0]).map(move |x| (x, y))).map(|(x, y)| half_float::F16x4::grey(scene(x, y))).collect();
        queue.write_texture(
            target.texture().as_image_copy(),
            bytemuck::cast_slice(&data),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size[0] * 8), rows_per_image: None },
            wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
        );
        let output = texture_2d(device, "output", out);
        let mut encoder = device.create_command_encoder(&Default::default());
        post.run(device, queue, &mut encoder, &target, &output, 0.0, Echo::NONE, &Composite::NONE, None);
        queue.submit([encoder.finish()]);
        half_float::read(device, queue, output.texture(), out)
    }

    /// On an HDR target: an ordinary grey is at paper white's scale (scRGB: 1 is 80
    /// nits), and a bright highlight goes above paper white, up to the peak; on SDR the
    /// same highlight stays under white.
    #[test]
    fn hdr_output_reaches_above_paper_white() {
        let gpu = match sim3d::GpuContext::new() {
            Ok(g) => g,
            Err(e) => return eprintln!("skipping: {e:#}"),
        };
        let (device, queue) = (&gpu.device, &gpu.queue);
        let mut post = PostProcess::new(device, SCENE_FORMAT);
        let size = [32u32, 32];
        let sdr_bright = composite_of(device, queue, &mut post, size, size, &|_, _| 6.0)[0][0];
        post.set_hdr(device, Some(HdrLevels { white: 200.0, peak: 1000.0 }));
        let grey = composite_of(device, queue, &mut post, size, size, &|_, _| 0.5)[0][0];
        let bright = composite_of(device, queue, &mut post, size, size, &|_, _| 6.0)[0][0];
        let scale = 200.0 / SCRGB_NITS;
        assert!((grey - 0.5 * scale).abs() < 0.01, "grey at {grey}, expected {}", 0.5 * scale);
        assert!(bright > 2.0 * scale && bright <= 5.0 * scale + 0.01, "a highlight at {bright} (paper white {scale}, peak {})", 5.0 * scale);
        assert!(sdr_bright < 1.0, "on SDR the highlight is {sdr_bright}");
    }

    /// Stripes moving fast along the flow come out blurred; with no flow, sharp.
    #[test]
    fn motion_blur_follows_the_flow() {
        let gpu = match sim3d::GpuContext::new() {
            Ok(g) => g,
            Err(e) => return eprintln!("skipping: {e:#}"),
        };
        let (device, queue) = (&gpu.device, &gpu.queue);
        let mut post = PostProcess::new(device, SCENE_FORMAT);
        let size = [64u32, 32];
        let stripes = |x: u32, _| if x % 8 < 4 { 0.6 } else { 0.05 };
        let contrast = |img: &[[f32; 4]]| {
            let row: Vec<f32> = (16..48).map(|x| img[(16 * size[0] + x) as usize][0]).collect();
            row.iter().fold(0.0f32, |m, &v| m.max(v)) - row.iter().fold(1.0f32, |m, &v| m.min(v))
        };
        let still = contrast(&composite_of(device, queue, &mut post, size, size, &stripes));
        // Ten uv a second sideways: over half a 60th of a second, a twelfth of the width.
        let flow = super::super::feedback::texture(device, "flow", size, super::super::feedback::FLOW_FORMAT);
        // (Halves: 10 is 0x4900.)
        let moving = vec![[0x4900u16, 0]; (size[0] * size[1]) as usize];
        queue.write_texture(
            flow.texture().as_image_copy(),
            bytemuck::cast_slice(&moving),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size[0] * 4), rows_per_image: None },
            wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
        );
        post.set_flow(Some(&flow), 1.0 / 60.0);
        let blurred = contrast(&composite_of(device, queue, &mut post, size, size, &stripes));
        assert!(still > 0.5 && blurred < 0.5 * still, "stripes' contrast {still} still, {blurred} moving");
    }

    /// A scene drawn at half size and enlarged keeps a sharper step than bilinear would
    /// (Catmull-Rom: the pixels either side of the edge differ by 0.475 of the step
    /// instead of 0.4), without overshooting.
    #[test]
    fn an_enlarged_scene_stays_sharp() {
        let gpu = match sim3d::GpuContext::new() {
            Ok(g) => g,
            Err(e) => return eprintln!("skipping: {e:#}"),
        };
        let (device, queue) = (&gpu.device, &gpu.queue);
        let mut post = PostProcess::new(device, SCENE_FORMAT);
        let img = composite_of(device, queue, &mut post, [32, 32], [64, 64], &|x, _| if x < 16 { 0.0 } else { 0.8 });
        let at = |x: u32| img[(32 * 64 + x) as usize][0];
        assert!(at(32) - at(31) > 0.44, "across the edge: {} to {}", at(31), at(32));
        assert!((0..64).all(|x| (-1e-3..=0.8 + 1e-3).contains(&at(x))), "overshoot");
    }

    pub(crate) mod half_float {
        use bytemuck::{Pod, Zeroable};

        #[repr(C)]
        #[derive(Clone, Copy, Pod, Zeroable)]
        pub struct F16x4([u16; 4]);

        impl F16x4 {
            pub fn grey(v: f32) -> Self {
                Self::rgb([v; 3])
            }

            pub fn rgb(c: [f32; 3]) -> Self {
                Self([to_f16(c[0]), to_f16(c[1]), to_f16(c[2]), to_f16(1.0)])
            }
        }

        /// f32 to f16 for the normal positive values the test uses.
        pub fn to_f16(v: f32) -> u16 {
            let bits = v.to_bits();
            let exp = ((bits >> 23) & 0xff) as i32 - 127 + 15;
            if v == 0.0 || exp <= 0 {
                return 0;
            }
            ((exp as u16) << 10) | ((bits >> 13) & 0x3ff) as u16
        }

        fn from_f16(h: u16) -> f32 {
            let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
            let exp = ((h >> 10) & 0x1f) as i32;
            let mantissa = (h & 0x3ff) as f32 / 1024.0;
            if exp == 0 {
                return sign * mantissa * 2f32.powi(-14);
            }
            sign * (1.0 + mantissa) * 2f32.powi(exp - 15)
        }

        pub fn read(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture, size: [u32; 2]) -> Vec<[f32; 4]> {
            read_channels(device, queue, texture, size, 4).chunks(4).map(|c| [c[0], c[1], c[2], c[3]]).collect()
        }

        /// A half-float texture of `channels` channels, read back row by row.
        pub fn read_channels(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture, size: [u32; 2], channels: u32) -> Vec<f32> {
            let texel = 2 * channels;
            let row = (size[0] * texel).next_multiple_of(256);
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("readback"),
                size: u64::from(row * size[1]),
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            encoder.copy_texture_to_buffer(
                texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: None },
                },
                wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
            );
            queue.submit([encoder.finish()]);
            buffer.slice(..).map_async(wgpu::MapMode::Read, |r| r.expect("map"));
            device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
            let data = buffer.slice(..).get_mapped_range().expect("mapped");
            let mut out = Vec::new();
            for y in 0..size[1] {
                let start = (y * row) as usize;
                let halves: &[u16] = bytemuck::cast_slice(&data[start..start + (size[0] * texel) as usize]);
                out.extend(halves.iter().map(|&h| from_f16(h)));
            }
            out
        }
    }
}
