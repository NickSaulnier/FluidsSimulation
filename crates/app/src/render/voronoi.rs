//! Voronoi cells around the particles (`voronoi.wgsl`): each pixel takes the colour of
//! its nearest particle (or every Nth, for bigger cells), found on the GPU by the jump
//! flooding algorithm, so the cost goes with the target's size, not the particles'
//! count. A third way to draw particles, beside discs and the liquid surface.
//!
//! Each frame: the seeds (one pixel a particle, with its position and speed) into one of
//! two textures, then flood passes back and forth between them, the step halving from
//! the cells' reach (a cell is never drawn further than that from its particle, so
//! nothing beyond needs finding) down to 1; then, in the scene pass, the cells shaded.

use super::particles::ParticleLayer;
use bytemuck::{Pod, Zeroable};

/// The seeds' format: position (pixels), speed, whether there's one. Exact, unblended.
const SEED_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;
/// Uniform slots, one a flood pass (and slot 0 for the seeding and shading), each at the
/// alignment dynamic offsets need.
const SLOTS: usize = 16;
const SLOT_BYTES: u64 = 256;

/// How the cells look.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoronoiStyle {
    /// Every `stride`th particle seeds a cell (1: every one; more: bigger cells).
    pub stride: u32,
}

/// Must match `Voronoi` in voronoi.wgsl (32 bytes).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    stride: u32,
    step: i32,
    spacing: f32,
    reach: f32,
    edge: f32,
    _pad: [f32; 3],
}

/// The two seed textures, a bind group reading each, and their size.
struct Seeds {
    views: [wgpu::TextureView; 2],
    groups: [wgpu::BindGroup; 2],
    size: (u32, u32),
}

pub struct VoronoiLayer {
    seed_pipeline: wgpu::RenderPipeline,
    flood_pipeline: wgpu::RenderPipeline,
    shade_pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    textures: Option<Seeds>,
    /// The flood's steps this frame, and which texture holds the result.
    steps: Vec<i32>,
    result: usize,
}

impl VoronoiLayer {
    pub fn new(device: &wgpu::Device, globals_layout: &wgpu::BindGroupLayout, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("voronoi.wgsl"), source: wgpu::ShaderSource::Wgsl(include_str!("voronoi.wgsl").into()) });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("voronoi"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: true, min_binding_size: wgpu::BufferSize::new(size_of::<Params>() as u64) },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("voronoi"), bind_group_layouts: &[Some(globals_layout), Some(&layout)], immediate_size: 0 });
        let pipeline = |label, vs, fs, buffers: &[Option<wgpu::VertexBufferLayout<'_>>], topology, target: wgpu::ColorTargetState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState { module: &shader, entry_point: Some(vs), compilation_options: Default::default(), buffers },
                fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some(fs), compilation_options: Default::default(), targets: &[Some(target)] }),
                primitive: wgpu::PrimitiveState { topology, ..Default::default() },
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let seed_pipeline = pipeline("voronoi seed", "vs_seed", "fs_seed", &ParticleLayer::INSTANCE_LAYOUTS, wgpu::PrimitiveTopology::PointList, SEED_FORMAT.into());
        let flood_pipeline = pipeline("voronoi flood", "vs_full", "fs_flood", &[], wgpu::PrimitiveTopology::TriangleList, SEED_FORMAT.into());
        let shade_pipeline = pipeline(
            "voronoi shade",
            "vs_shade",
            "fs_shade",
            &[],
            wgpu::PrimitiveTopology::TriangleStrip,
            wgpu::ColorTargetState { format, blend: Some(wgpu::BlendState::ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL },
        );
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voronoi params"),
            size: SLOTS as u64 * SLOT_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { seed_pipeline, flood_pipeline, shade_pipeline, layout, params, textures: None, steps: Vec::new(), result: 0 }
    }

    /// Size the seed textures to the target and set this frame's cells up: `radius_px`
    /// is a particle's radius on the target, pixels.
    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, size: (u32, u32), style: VoronoiStyle, radius_px: f32) {
        let size = (size.0.max(1), size.1.max(1));
        if self.textures.as_ref().is_none_or(|t| t.size != size) {
            let texture = || {
                device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some("voronoi seeds"),
                        size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: SEED_FORMAT,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    })
                    .create_view(&Default::default())
            };
            let views = [texture(), texture()];
            let group = |view: &wgpu::TextureView| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("voronoi"),
                    layout: &self.layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &self.params, offset: 0, size: wgpu::BufferSize::new(size_of::<Params>() as u64) }),
                        },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(view) },
                    ],
                })
            };
            let groups = [group(&views[0]), group(&views[1])];
            self.textures = Some(Seeds { views, groups, size });
        }
        // A cell spans about two radii a seed (particles sit two radii apart), wider as
        // fewer particles seed: the stride's square root each way.
        let stride = style.stride.max(1);
        let spacing = (2.0 * radius_px * (stride as f32).sqrt()).max(2.0);
        // The chosen particles (every Nth by index) aren't evenly spread, so a cell may
        // run further than its typical width: with less, holes opened between them.
        let reach = 1.5 * spacing;
        let edge = (0.08 * spacing).clamp(1.0, 3.0);
        // Steps from the reach's power of two down to 1.
        self.steps.clear();
        let mut step = (reach.ceil() as u32).next_power_of_two() as i32;
        while step >= 1 && self.steps.len() + 1 < SLOTS {
            self.steps.push(step);
            step /= 2;
        }
        for (k, step) in std::iter::once(0).chain(self.steps.iter().copied()).enumerate() {
            let p = Params { stride, step, spacing, reach, edge, _pad: [0.0; 3] };
            queue.write_buffer(&self.params, k as u64 * SLOT_BYTES, bytemuck::bytes_of(&p));
        }
    }

    /// The seeds and the flood, before the scene pass.
    pub fn flood(&mut self, encoder: &mut wgpu::CommandEncoder, globals: &wgpu::BindGroup, particles: &ParticleLayer) {
        let Some(Seeds { views, groups, .. }) = &self.textures else { return };
        let pass = |encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView| {
            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("voronoi"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            })
            .forget_lifetime()
        };
        {
            // Into texture 0 (reading 1, unused: a texture can't be both).
            let mut p = pass(encoder, &views[0]);
            p.set_pipeline(&self.seed_pipeline);
            p.set_bind_group(0, globals, &[]);
            p.set_bind_group(1, &groups[1], &[0]);
            let count = particles.bind_instances(&mut p);
            if count > 0 {
                p.draw(0..1, 0..count);
            }
        }
        let mut from = 0;
        for k in 0..self.steps.len() {
            let mut p = pass(encoder, &views[1 - from]);
            p.set_pipeline(&self.flood_pipeline);
            p.set_bind_group(0, globals, &[]);
            p.set_bind_group(1, &groups[from], &[((k + 1) as u64 * SLOT_BYTES) as u32]);
            p.draw(0..3, 0..1);
            from = 1 - from;
        }
        self.result = from;
    }

    /// The cells, shaded, inside the scene pass (the globals bound at 0 already).
    pub fn shade(&self, pass: &mut wgpu::RenderPass<'_>) {
        if let Some(Seeds { groups, .. }) = &self.textures {
            pass.set_pipeline(&self.shade_pipeline);
            pass.set_bind_group(1, &groups[self.result], &[0]);
            pass.draw(0..4, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    /// The shader validates.
    #[test]
    fn the_shader_validates() {
        use wgpu::naga;
        let source = include_str!("voronoi.wgsl");
        let module = naga::front::wgsl::parse_str(source).unwrap_or_else(|e| panic!("{}", e.emit_to_string(source)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty()).validate(&module).unwrap_or_else(|e| panic!("{}", e.emit_to_string(source)));
    }
}
