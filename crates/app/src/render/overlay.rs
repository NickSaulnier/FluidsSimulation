//! Field overlays: a solver's grid field, colourised on the CPU, drawn as a textured
//! quad at the field's world layout.

use super::colormap;
use bytemuck::{Pod, Zeroable};
use sim::{FieldKind, FieldLayout};

/// Must match `Overlay` in overlay.wgsl (32 bytes).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    origin: [f32; 2],
    size: [f32; 2],
    opacity: f32,
    _pad: [f32; 3],
}

/// A field to draw this frame.
pub struct FieldFrame<'a> {
    pub layout: FieldLayout,
    pub values: &'a [f32],
    pub kind: FieldKind,
    pub opacity: f32,
}

pub struct OverlayLayer {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    nearest: wgpu::Sampler,
    linear: wgpu::Sampler,
    params: wgpu::Buffer,
    /// The field texture and its bind group, plus whether that bind group samples linearly.
    texture: Option<(wgpu::Texture, wgpu::BindGroup, bool)>,
    rgba: Vec<u8>,
    visible: bool,
}

impl OverlayLayer {
    pub fn new(device: &wgpu::Device, globals_layout: &wgpu::BindGroupLayout, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("overlay.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("overlay.wgsl").into()),
        });
        let entry = |binding, visibility, ty| wgpu::BindGroupLayoutEntry { binding, visibility, ty, count: None };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay"),
            entries: &[
                entry(
                    0,
                    wgpu::ShaderStages::FRAGMENT,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                entry(1, wgpu::ShaderStages::FRAGMENT, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
                entry(
                    2,
                    wgpu::ShaderStages::VERTEX_FRAGMENT,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(size_of::<Params>() as u64),
                    },
                ),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("overlay"),
            bind_group_layouts: &[Some(globals_layout), Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("overlay"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_overlay"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_overlay"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        // Nearest for diagnostic fields (one texel per cell: what you see is what the
        // solver computed); linear for concentrations such as dye, which are the picture.
        let nearest = device.create_sampler(&wgpu::SamplerDescriptor { label: Some("overlay nearest"), ..Default::default() });
        let linear = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("overlay linear"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("overlay params"),
            size: size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { pipeline, layout, nearest, linear, params, texture: None, rgba: Vec::new(), visible: false }
    }

    /// Upload this frame's field, or hide the overlay with `None`. Returns the value
    /// range the colours were normalised over, for the legend.
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        field: Option<FieldFrame<'_>>,
        palette: &super::Palette,
    ) -> Option<(f32, f32)> {
        let Some(f) = field else {
            self.visible = false;
            return None;
        };
        let range = colormap::range(f.values, f.kind);
        colormap::colourise(f.values, f.kind, range, &palette.dye_ramp(), &mut self.rgba);

        let size = wgpu::Extent3d { width: f.layout.nx as u32, height: f.layout.ny as u32, depth_or_array_layers: 1 };
        let smooth = f.kind == FieldKind::Concentration;
        if self.texture.as_ref().is_none_or(|(t, _, linear)| t.size() != size || *linear != smooth) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("overlay"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                // sRGB: the colour maps are authored in sRGB, and sampling decodes to linear.
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("overlay"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(if smooth { &self.linear } else { &self.nearest }),
                    },
                    wgpu::BindGroupEntry { binding: 2, resource: self.params.as_entire_binding() },
                ],
            });
            self.texture = Some((texture, bind_group, smooth));
        }
        let (texture, _, _) = self.texture.as_ref().expect("created above");
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &self.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * f.layout.nx as u32),
                rows_per_image: Some(f.layout.ny as u32),
            },
            size,
        );
        let extent = glam::Vec2::new(f.layout.nx as f32, f.layout.ny as f32) * f.layout.cell;
        let params = Params { origin: f.layout.origin.into(), size: extent.into(), opacity: f.opacity, _pad: [0.0; 3] };
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
        self.visible = true;
        Some(range)
    }

    /// Expects the globals bind group at index 0 to be set already.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if let (true, Some((_, bind_group, _))) = (self.visible, &self.texture) {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(1, bind_group, &[]);
            pass.draw(0..4, 0..1);
        }
    }
}
