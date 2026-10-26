//! Post-processing (`post.wgsl`): the scene is drawn into an offscreen HDR texture at a
//! render scale, then bloomed and composited onto the screen through a tone curve.
//!
//! The tone curve leaves colours up to `TONE_KNEE` (0.85, linear) alone, so the 2D
//! scenes look as they did when they drew straight to the screen; only highlights above
//! it (fire, glows, bloom) roll off toward white (Reinhard-style) instead of clipping.

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

/// Must match `Post` in post.wgsl (144 bytes).
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
    _pad: [f32; 3],
    deep: [f32; 4],
    base: [f32; 4],
    highlight: [f32; 4],
}

impl Uniforms {
    fn new(strength: f32, echo: Echo, aspect: f32, c: &Composite) -> Self {
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
    sampler: wgpu::Sampler,
    /// Stands in for the bloom texture in the passes that don't read it.
    blank: wgpu::TextureView,
    prefilter: wgpu::RenderPipeline,
    down: wgpu::RenderPipeline,
    up: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    targets: Option<Targets>,
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
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("post uniforms"),
            size: size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
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
            layout,
            uniforms,
            sampler,
            blank,
            targets: None,
        }
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
        let down = levels.windows(2).map(|w| self.group(device, &w[0], &self.blank)).collect();
        let up = levels.windows(2).map(|w| self.group(device, &w[1], &self.blank)).collect();
        Targets { size, scene, levels, down, up }
    }

    fn group(&self, device: &wgpu::Device, src: &wgpu::TextureView, bloom: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("post"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniforms.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(src) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(bloom) },
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
        let uniforms = Uniforms::new(strength, echo, aspect, composite);
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&uniforms));
        if strength > 0.0 {
            let prefilter = self.group(device, source, &self.blank);
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
        let composite = self.group(device, source, &t.levels[0]);
        fullscreen(encoder, "composite", output, false, &self.composite, &composite);
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
            let composite = Composite { settings, clock: CompositeClock::default(), palette };
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
        assert!(close(turned[0], 0.0, 0.01) && close(turned[1], 0.5, 0.01) && close(turned[2], 0.0, 0.01), "red turned 120° isn't green: {turned:?}");
        let inverted = render(&|_, _| [0.2; 3], CompositeSettings { invert: 1.0, ..off })[0];
        assert!(close(inverted[0], 0.8, 2e-3), "0.2 inverted: {inverted:?}");
        let vignetted = render(&|_, _| [0.5; 3], CompositeSettings { vignette: 1.0, ..off });
        assert!(close(at(&vignetted, 32, 32)[0], 0.5, 2e-3) && at(&vignetted, 0, 0)[0] < 0.2, "vignette: centre {:?}, corner {:?}", at(&vignetted, 32, 32), at(&vignetted, 0, 0));
        // (A bright white: 1.0 itself tone-maps to 0.925, a little short of the top.)
        let ramp = render(&|x, _| [if x < 32 { 0.0 } else { 50.0 }; 3], CompositeSettings { palette_map: 1.0, ..off });
        let highlight = crate::render::composite::linear(palette.highlight);
        let white = at(&ramp, 60, 10);
        assert!(at(&ramp, 4, 10)[0] < 1e-3 && (0..3).all(|c| close(white[c], highlight[c], 0.02)), "palette map: black {:?}, white {white:?} (highlight {highlight:?})", at(&ramp, 4, 10));
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
    mod half_float {
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
        fn to_f16(v: f32) -> u16 {
            let bits = v.to_bits();
            let exp = ((bits >> 23) & 0xff) as i32 - 127 + 15;
            if v == 0.0 || exp <= 0 {
                return 0;
            }
            ((exp as u16) << 10) | ((bits >> 13) & 0x3ff) as u16
        }

        fn from_f16(h: u16) -> f32 {
            let exp = ((h >> 10) & 0x1f) as i32;
            let mantissa = (h & 0x3ff) as f32 / 1024.0;
            if exp == 0 {
                return mantissa * 2f32.powi(-14);
            }
            (1.0 + mantissa) * 2f32.powi(exp - 15)
        }

        pub fn read(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture, size: [u32; 2]) -> Vec<[f32; 4]> {
            let row = (size[0] * 8).next_multiple_of(256);
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
                let halves: &[u16] = bytemuck::cast_slice(&data[start..start + (size[0] * 8) as usize]);
                out.extend(halves.chunks(4).map(|c| [from_f16(c[0]), from_f16(c[1]), from_f16(c[2]), from_f16(c[3])]));
            }
            out
        }
    }
}
