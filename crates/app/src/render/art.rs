//! The song's cover art (see `audio::media`), drawn into the scene before the feedback,
//! so the warp carries it: spun into tunnels, smeared along the fluid's flow. A look
//! asks for it in `[art]` (part of its waves, `WaveSettings::art`); without a cover
//! nothing is drawn.

use super::post::SCENE_FORMAT;
use crate::audio::media::Art;
use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};
use std::f32::consts::TAU;

/// Where and how the cover is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtSettings {
    pub enabled: bool,
    /// Half the cover's longer side, in canvas half-heights.
    pub size: f32,
    /// 0 (unseen) to 1.
    pub opacity: f32,
    /// Centre, in canvas half-heights from the middle (x right, y up).
    pub centre: [f32; 2],
    /// Turn, radians per second.
    pub spin: f32,
    /// How much the size follows the bass (MilkDrop's `bass_att`), 0 to 1.
    pub bass_pulse: f32,
    /// Light, 0 to 4 (above 1 glows).
    pub brightness: f32,
}

impl ArtSettings {
    pub const OFF: Self = Self { enabled: false, size: 0.35, opacity: 1.0, centre: [0.0, 0.0], spin: 0.0, bass_pulse: 0.2, brightness: 1.0 };
}

impl Default for ArtSettings {
    fn default() -> Self {
        Self::OFF
    }
}

/// Must match `Art` in art.wgsl (48 bytes).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    centre: [f32; 2],
    half: [f32; 2],
    turn: [f32; 2],
    aspect: f32,
    opacity: f32,
    brightness: f32,
    _pad: [f32; 3],
}

pub struct ArtLayer {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniforms: wgpu::Buffer,
    /// The cover as a texture, bound, and its size; none without one.
    cover: Option<(wgpu::BindGroup, [u32; 2])>,
    /// The angle, radians: the spin added up.
    turn: f32,
}

impl ArtLayer {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("art.wgsl"), source: wgpu::ShaderSource::Wgsl(include_str!("art.wgsl").into()) });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("art"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: wgpu::BufferSize::new(size_of::<Uniforms>() as u64) },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("art"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        // Premultiplied alpha: the cover over the scene.
        let over = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("art"),
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
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("art"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("art uniforms"),
            size: size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { pipeline, layout, sampler, uniforms, cover: None, turn: 0.0 }
    }

    /// Show `art` from now on (none: draw nothing).
    pub fn set(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, art: Option<&Art>) {
        self.cover = art.filter(|a| a.size[0] > 0 && a.size[1] > 0).map(|art| {
            let extent = wgpu::Extent3d { width: art.size[0], height: art.size[1], depth_or_array_layers: 1 };
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("cover art"),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            queue.write_texture(
                texture.as_image_copy(),
                &art.rgba,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * art.size[0]), rows_per_image: None },
                extent,
            );
            let view = texture.create_view(&Default::default());
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("art"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.uniforms.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                ],
            });
            (group, art.size)
        });
    }

    pub fn has_cover(&self) -> bool {
        self.cover.is_some()
    }

    /// Advance the spin by `dt` seconds (call every frame).
    pub fn update(&mut self, settings: &ArtSettings, dt: f32) {
        self.turn = (self.turn + settings.spin * dt) % (TAU * 1000.0);
    }

    /// Draw the cover onto `target` within `viewport` (pixels: x, y, width, height), if
    /// there is one and `settings` ask for it. `bass`: MilkDrop's `bass_att`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&self, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView, viewport: [f32; 4], settings: &ArtSettings, bass: f32) {
        let [x, y, w, h] = viewport;
        let Some((group, size)) = &self.cover else { return };
        if !settings.enabled || settings.opacity <= 0.0 || w < 1.0 || h < 1.0 {
            return;
        }
        let scale = settings.size * (1.0 + settings.bass_pulse * (bass - 1.0)).max(0.0);
        let longer = size[0].max(size[1]) as f32;
        let uniforms = Uniforms {
            centre: settings.centre,
            half: [scale * size[0] as f32 / longer, scale * size[1] as f32 / longer],
            turn: [self.turn.cos(), self.turn.sin()],
            aspect: w / h,
            opacity: settings.opacity.clamp(0.0, 1.0),
            brightness: settings.brightness.max(0.0),
            _pad: [0.0; 3],
        };
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&uniforms));
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("art"),
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
        pass.set_bind_group(0, group, &[]);
        pass.draw(0..6, 0..1);
    }
}
