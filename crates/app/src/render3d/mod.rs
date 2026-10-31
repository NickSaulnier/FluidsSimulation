//! The 3D view: an orbit camera, the scene's lines (a floor grid fading out round the
//! fire, and the burners; no box: the domain is open air,
//! vents) and, over them, the gas raymarched from the solver's volume texture, with the
//! solids shaded where the rays hit them (`march.wgsl`).
//!
//! The lines write depth, and the raymarch stops at it: a line in front of the smoke
//! stays sharp, one behind is seen through it.
//!
//! The smoke's shadowing comes from a light volume (`light.wgsl`): a compute pass each
//! frame marches from every cell of a grid at half the volume's resolution toward the
//! light, through the smoke and solids, so smoke shadows itself and the solids shadow
//! it, however far apart. The march's frames are gathered (`history.wgsl`): each frame's
//! rays start at new depths, and the frame is blended into the ones before, moved with
//! the camera (through each pixel's visible gas or solid) and clamped to its
//! neighbourhood, so the grain of the starts averages out.

mod camera;

pub use camera::OrbitCamera;

use bytemuck::{Pod, Zeroable};
use crate::render::Palette;
use crate::render::feedback::FLOW_FORMAT;
use glam::{Mat4, Vec3};
use sim3d::{Scene3d, Shape, Volume};
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 3],
    colour: [f32; 4],
}

/// Linear-space colours (the surface is sRGB).
const GRID: [f32; 4] = [0.12, 0.13, 0.16, 1.0];
const BURNER: [f32; 4] = [1.0, 0.35, 0.05, 1.0];
const GRID_STEP: f32 = 0.1;
/// The floor grid fades into the background between these distances from the middle.
const GRID_FADE: std::ops::Range<f32> = 0.35..0.9;
const CIRCLE_SEGMENTS: usize = 40;
/// The colour around and behind the domain, matching the 2D view's letterbox.
const BACKGROUND: wgpu::Color = wgpu::Color { r: 0.012, g: 0.014, b: 0.022, a: 1.0 };

/// Clear `target` to the 3D scene's background (for a backdrop drawn before `render`).
pub fn clear(encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("3d background"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(BACKGROUND), store: wgpu::StoreOp::Store },
        })],
        ..Default::default()
    });
}
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Smoke extinction per unit of smoke (the burners' output), 1/m.
const EXTINCTION: f32 = 20.0;
/// The light volume's: stronger than the view's, so thin smoke still shows its form.
const SHADOW_EXTINCTION: f32 = 120.0;
/// March step, in cells.
const STEP_CELLS: f32 = 0.75;
/// The light volume's cells: this many of the volume's each way.
const LIGHT_CELLS: u32 = 2;
/// How much of each frame goes into the gathered march.
const HISTORY_WEIGHT: f32 = 0.2;
/// The gathered march's textures' format.
const LAYER_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Toward the light: above, to the side and a little to the front.
const LIGHT: Vec3 = Vec3::new(0.6, 0.8, 0.3);
const MAX_SHAPES: usize = 32;
/// Glow per metre of full flame, before the roll-off.
const FLAME_GLOW: f32 = 40.0;

/// Must match `March` in march.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MarchUniforms {
    inv_view_proj: [[f32; 4]; 4],
    view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
    size: [f32; 4],
    viewport: [f32; 4],
    lit: [f32; 4],
    shade: [f32; 4],
    light: [f32; 4],
    counts: [u32; 4],
    shapes: [[f32; 4]; 2 * MAX_SHAPES],
}

/// Must match `Light` in light.wgsl (48 bytes).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct LightUniforms {
    size: [f32; 4],
    light: [f32; 4],
    cells: [u32; 4],
}

/// Must match `Resolve` in history.wgsl (96 bytes).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ResolveUniforms {
    prev_view_proj: [[f32; 4]; 4],
    viewport: [f32; 4],
    weight: f32,
    _pad: [f32; 3],
}

/// The march gathered across frames, at the target's size: this frame's march and
/// where its visible gas is, and the gathered picture (the latest of two).
struct Layers {
    size: [u32; 2],
    layer: wgpu::TextureView,
    position: wgpu::TextureView,
    frames: [wgpu::TextureView; 2],
    latest: usize,
}

fn layer_texture(device: &wgpu::Device, label: &str, size: [u32; 2]) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: size[0].max(1), height: size[1].max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: LAYER_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

pub struct Renderer3d {
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    vertices: Option<(wgpu::Buffer, u32)>,
    march: wgpu::RenderPipeline,
    march_layout: wgpu::BindGroupLayout,
    march_uniforms: wgpu::Buffer,
    sampler: wgpu::Sampler,
    /// The lines' depth, at the target's size.
    depth: Option<(wgpu::TextureView, [u32; 2])>,
    /// Stands in for the volume when the solver has none.
    empty_volume: wgpu::TextureView,
    light_pipeline: wgpu::ComputePipeline,
    light_layout: wgpu::BindGroupLayout,
    light_uniforms: wgpu::Buffer,
    /// The light volume and its cells.
    light_volume: Option<(wgpu::TextureView, glam::UVec3)>,
    resolve_pipeline: wgpu::RenderPipeline,
    over_pipeline: wgpu::RenderPipeline,
    history_layout: wgpu::BindGroupLayout,
    resolve_uniforms: wgpu::Buffer,
    layers: Option<Layers>,
    /// The last frame's view and projection and viewport, if there's a gathered march
    /// to carry on; and the frame count, for the rays' starts.
    last_view: Option<(Mat4, [f32; 4])>,
    frame: u32,
}

impl Renderer3d {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("wire.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("wire.wgsl").into()),
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("3d uniforms"),
            size: size_of::<Mat4>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("3d uniforms"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(size_of::<Mat4>() as u64),
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("3d uniforms"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("wire"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("wire"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::LineList, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });

        let march_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("march.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("march.wgsl").into()),
        });
        let march_uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("march uniforms"),
            size: size_of::<MarchUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let fragment = wgpu::ShaderStages::FRAGMENT;
        let march_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("march"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: fragment,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(size_of::<MarchUniforms>() as u64),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: fragment,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: fragment,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: fragment,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: fragment,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: fragment,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let march_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("march"),
            bind_group_layouts: &[Some(&march_layout)],
            immediate_size: 0,
        });
        // Premultiplied: the march's colour plus what's behind, times the light let through.
        let over = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::SrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let march = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("march"),
            layout: Some(&march_pipeline_layout),
            vertex: wgpu::VertexState { module: &march_shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState {
                module: &march_shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                // This frame's march, as it is (gathered and drawn over the target after).
                targets: &[Some(LAYER_FORMAT.into()), Some(FLOW_FORMAT.into()), Some(LAYER_FORMAT.into())],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("volume"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let empty_volume = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("empty volume"),
                size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D3,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        // The light volume.
        let light_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("light.wgsl"), source: wgpu::ShaderSource::Wgsl(include_str!("light.wgsl").into()) });
        let compute = wgpu::ShaderStages::COMPUTE;
        let light_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("light volume"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: compute,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: wgpu::BufferSize::new(size_of::<LightUniforms>() as u64) },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: compute,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D3, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: compute, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: compute,
                    ty: wgpu::BindingType::StorageTexture { access: wgpu::StorageTextureAccess::WriteOnly, format: wgpu::TextureFormat::Rgba16Float, view_dimension: wgpu::TextureViewDimension::D3 },
                    count: None,
                },
            ],
        });
        let light_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("light volume"),
            layout: Some(&device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("light volume"), bind_group_layouts: &[Some(&light_layout)], immediate_size: 0 })),
            module: &light_shader,
            entry_point: Some("light"),
            compilation_options: Default::default(),
            cache: None,
        });
        let buffer = |label, size: usize| device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: size as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let light_uniforms = buffer("light volume uniforms", size_of::<LightUniforms>());

        // Gathering the march's frames.
        let history_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("history.wgsl"), source: wgpu::ShaderSource::Wgsl(include_str!("history.wgsl").into()) });
        let texture_2d = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: fragment,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
            count: None,
        };
        let history_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("march history"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: fragment,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: wgpu::BufferSize::new(size_of::<ResolveUniforms>() as u64) },
                    count: None,
                },
                texture_2d(1),
                texture_2d(2),
                texture_2d(3),
                wgpu::BindGroupLayoutEntry { binding: 4, visibility: fragment, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let history_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("march history"), bind_group_layouts: &[Some(&history_layout)], immediate_size: 0 });
        let fullscreen = |label, entry, target: wgpu::ColorTargetState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&history_pipeline_layout),
                vertex: wgpu::VertexState { module: &history_shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                fragment: Some(wgpu::FragmentState { module: &history_shader, entry_point: Some(entry), compilation_options: Default::default(), targets: &[Some(target)] }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let resolve_pipeline = fullscreen("march resolve", "fs_resolve", LAYER_FORMAT.into());
        let over_pipeline = fullscreen(
            "march over",
            "fs_over",
            wgpu::ColorTargetState { format, blend: Some(wgpu::BlendState { color: over, alpha: wgpu::BlendComponent::OVER }), write_mask: wgpu::ColorWrites::ALL },
        );
        let resolve_uniforms = buffer("march resolve uniforms", size_of::<ResolveUniforms>());
        Self {
            pipeline,
            uniforms,
            bind_group,
            vertices: None,
            march,
            march_layout,
            march_uniforms,
            sampler,
            depth: None,
            empty_volume,
            light_pipeline,
            light_layout,
            light_uniforms,
            light_volume: None,
            resolve_pipeline,
            over_pipeline,
            history_layout,
            resolve_uniforms,
            layers: None,
            last_view: None,
            frame: 0,
        }
    }

    /// The light volume for `volume` (its cells), made at half its resolution if it
    /// isn't already.
    fn light_view(&mut self, device: &wgpu::Device, cells: glam::UVec3) -> (wgpu::TextureView, glam::UVec3) {
        let light_cells = (cells / LIGHT_CELLS).max(glam::UVec3::ONE);
        if self.light_volume.as_ref().is_none_or(|(_, c)| *c != light_cells) {
            let view = device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("light volume"),
                    size: wgpu::Extent3d { width: light_cells.x, height: light_cells.y, depth_or_array_layers: light_cells.z },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D3,
                    format: wgpu::TextureFormat::Rgba16Float,
                    usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default());
            self.light_volume = Some((view, light_cells));
        }
        self.light_volume.clone().expect("just made")
    }

    fn depth_view(&mut self, device: &wgpu::Device, size: [u32; 2]) -> wgpu::TextureView {
        if self.depth.as_ref().is_none_or(|(_, s)| *s != size) {
            let view = device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("3d depth"),
                    size: wgpu::Extent3d { width: size[0].max(1), height: size[1].max(1), depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: DEPTH_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default());
            self.depth = Some((view, size));
        }
        self.depth.as_ref().expect("just made").0.clone()
    }

    /// Clear the target (`target_size` pixels) and draw `scene` and the solver's
    /// `volume` from `camera` into the `viewport` (physical pixels: x, y, width,
    /// height), the canvas left by the UI. The smoke takes its colours from `palette`.
    /// The gas's screen-space flow goes into `flow` (same size). `show_lines`: draw the
    /// floor grid, box and markers. `keep`: draw over what the target holds (a backdrop,
    /// over `clear`'s background) rather than clearing it.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        flow: &wgpu::TextureView,
        target_size: [u32; 2],
        viewport: [f32; 4],
        camera: &OrbitCamera,
        scene: &Scene3d,
        volume: Option<Volume<'_>>,
        palette: &Palette,
        show_lines: bool,
        keep: bool,
    ) {
        let [x, y, w, h] = viewport;
        let view_proj = camera.view_proj(w / h.max(1.0));
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&view_proj));
        self.frame = self.frame.wrapping_add(1);
        let mut march = march_uniforms(view_proj, camera.eye(), viewport, scene, volume.as_ref().map(|v| (v.cells, v.fire)), palette);
        march.counts[2] = self.frame % 64;
        queue.write_buffer(&self.march_uniforms, 0, bytemuck::bytes_of(&march));
        let depth = self.depth_view(device, target_size);
        // The light volume: how much light reaches each place, through the smoke.
        let light_volume = volume.as_ref().map(|v| {
            let (view, cells) = self.light_view(device, v.cells);
            let uniforms = LightUniforms {
                size: scene.size.extend(scene.size.y / cells.y as f32).to_array(),
                light: LIGHT.normalize().extend(SHADOW_EXTINCTION).to_array(),
                cells: [cells.x, cells.y, cells.z, 0],
            };
            queue.write_buffer(&self.light_uniforms, 0, bytemuck::bytes_of(&uniforms));
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("light volume"),
                layout: &self.light_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.light_uniforms.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(v.view) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&view) },
                ],
            });
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("light volume"), timestamp_writes: None });
            pass.set_pipeline(&self.light_pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(cells.x.div_ceil(4), cells.y.div_ceil(4), cells.z.div_ceil(4));
            view
        });
        // The march's own textures, and whether last frame's gathered march carries on.
        if self.layers.as_ref().is_none_or(|l| l.size != target_size) {
            let texture = |label| layer_texture(device, label, target_size);
            self.layers = Some(Layers { size: target_size, layer: texture("march"), position: texture("march position"), frames: [texture("march history"), texture("march history")], latest: 0 });
            self.last_view = None;
        }
        let carry_on = self.last_view.filter(|(_, v)| *v == viewport && volume.is_some()).map(|(m, _)| m);
        let march_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("march"),
            layout: &self.march_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.march_uniforms.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(volume.as_ref().map_or(&self.empty_volume, |v| v.view)),
                },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&depth) },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(volume.as_ref().map_or(&self.empty_volume, |v| v.velocity)),
                },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(light_volume.as_ref().unwrap_or(&self.empty_volume)) },
            ],
        });
        let lines = outline(scene);
        let bytes: &[u8] = bytemuck::cast_slice(&lines);
        match &self.vertices {
            Some((buffer, _)) if buffer.size() >= bytes.len() as u64 => queue.write_buffer(buffer, 0, bytes),
            _ => {
                let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("3d lines"),
                    contents: bytes,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                });
                self.vertices = Some((buffer, 0));
            }
        }
        if let Some((_, count)) = &mut self.vertices {
            *count = lines.len() as u32;
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("3d lines"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: if keep { wgpu::LoadOp::Load } else { wgpu::LoadOp::Clear(BACKGROUND) }, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            if w < 1.0 || h < 1.0 {
                return;
            }
            pass.set_viewport(x, y, w, h, 0.0, 1.0);
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            if let Some((buffer, count)) = &self.vertices
                && show_lines
            {
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..*count, 0..1);
            }
        }
        let layers = self.layers.as_mut().expect("made above");
        let attachment = |view, clear| {
            Some(wgpu::RenderPassColorAttachment { view, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(clear), store: wgpu::StoreOp::Store } })
        };
        {
            // Nothing marched lets all the light through.
            let clear_layer = wgpu::Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("3d volume"),
                color_attachments: &[attachment(&layers.layer, clear_layer), attachment(flow, wgpu::Color::TRANSPARENT), attachment(&layers.position, wgpu::Color::TRANSPARENT)],
                ..Default::default()
            });
            pass.set_viewport(x, y, w, h, 0.0, 1.0);
            pass.set_pipeline(&self.march);
            pass.set_bind_group(0, &march_group, &[]);
            pass.draw(0..3, 0..1);
        }
        let resolve = ResolveUniforms {
            prev_view_proj: carry_on.unwrap_or(view_proj).to_cols_array_2d(),
            viewport,
            weight: if carry_on.is_some() { HISTORY_WEIGHT } else { 1.0 },
            _pad: [0.0; 3],
        };
        queue.write_buffer(&self.resolve_uniforms, 0, bytemuck::bytes_of(&resolve));
        let (old, new) = (layers.latest, 1 - layers.latest);
        let bind = |history: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("march history"),
                layout: &self.history_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.resolve_uniforms.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&layers.layer) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&layers.position) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(history) },
                    wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                ],
            })
        };
        {
            let group = bind(&layers.frames[old]);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("march resolve"),
                color_attachments: &[attachment(&layers.frames[new], wgpu::Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 })],
                ..Default::default()
            });
            pass.set_viewport(x, y, w, h, 0.0, 1.0);
            pass.set_pipeline(&self.resolve_pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
        {
            let group = bind(&layers.frames[new]);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("march over"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            pass.set_viewport(x, y, w, h, 0.0, 1.0);
            pass.set_pipeline(&self.over_pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
        layers.latest = new;
        self.last_view = Some((view_proj, viewport));
    }
}

/// The raymarch's parameters: camera, domain, step, smoke colours and the solids.
fn march_uniforms(view_proj: Mat4, eye: Vec3, viewport: [f32; 4], scene: &Scene3d, volume: Option<(glam::UVec3, bool)>, palette: &Palette) -> MarchUniforms {
    let cells = volume.map(|v| v.0);
    let volume_fire = volume.is_some_and(|v| v.1);
    let cell = cells.map_or(1.0, |n| scene.size.y / n.y as f32);
    let fire = volume_fire;
    let [deep, _, highlight] = palette.linear();
    let rgb = |c: [f32; 4]| Vec3::new(c[0], c[1], c[2]);
    // Lit smoke: the highlight paled toward white; shadowed: the deep colour, dimmed.
    // A fire's smoke is soot, grey whatever the palette (as in 2D, fire keeps its own
    // colours).
    let (lit, shade) = if fire {
        (Vec3::splat(0.32), Vec3::splat(0.02))
    } else {
        (rgb(highlight).lerp(Vec3::splat(0.7), 0.5), 0.25 * rgb(deep) + Vec3::splat(0.015))
    };
    let mut shapes = [[0.0; 4]; 2 * MAX_SHAPES];
    let solids = &scene.solids[..scene.solids.len().min(MAX_SHAPES)];
    for (k, s) in solids.iter().enumerate() {
        (shapes[2 * k], shapes[2 * k + 1]) = match *s {
            Shape::Sphere { centre: c, radius } => ([c.x, c.y, c.z, radius], [0.0; 4]),
            Shape::Cuboid(b) => ([b.min.x, b.min.y, b.min.z, -1.0], [b.max.x, b.max.y, b.max.z, 0.0]),
        };
    }
    MarchUniforms {
        inv_view_proj: view_proj.inverse().to_cols_array_2d(),
        view_proj: view_proj.to_cols_array_2d(),
        eye: eye.extend(if fire { FLAME_GLOW } else { 0.0 }).to_array(),
        size: scene.size.extend(STEP_CELLS * cell).to_array(),
        viewport,
        lit: lit.extend(EXTINCTION).to_array(),
        shade: shade.extend(0.0).to_array(),
        light: LIGHT.normalize().extend(0.0).to_array(),
        counts: [solids.len() as u32, u32::from(cells.is_some()), 0, 0],
        shapes,
    }
}

/// The scene as coloured line segments (pairs of vertices).
fn outline(scene: &Scene3d) -> Vec<Vertex> {
    let mut v = Vec::new();
    let mut line = |a: Vec3, b: Vec3, colour: [f32; 4]| {
        v.push(Vertex { pos: a.into(), colour });
        v.push(Vertex { pos: b.into(), colour });
    };
    let s = scene.size;
    // The floor grid, in short pieces, each fading toward the background with its
    // distance from the middle.
    let middle = Vec3::new(0.5 * s.x, 0.0, 0.5 * s.z);
    let faded = |p: Vec3| {
        let d = (p.distance(middle) - GRID_FADE.start) / (GRID_FADE.end - GRID_FADE.start);
        let k = 1.0 - d.clamp(0.0, 1.0);
        let bg = [BACKGROUND.r as f32, BACKGROUND.g as f32, BACKGROUND.b as f32];
        [0, 1, 2].map(|c| bg[c] + (GRID[c] - bg[c]) * k * k).into_iter().chain([1.0]).collect::<Vec<_>>().try_into().unwrap_or(GRID)
    };
    let (nx, nz) = ((s.x / GRID_STEP).round() as usize, (s.z / GRID_STEP).round() as usize);
    let mut piece = |a: Vec3, b: Vec3| {
        let mid = 0.5 * (a + b);
        if mid.distance(middle) < GRID_FADE.end {
            line(a, b, faded(mid));
        }
    };
    for i in 1..nx {
        for k in 0..nz {
            let x = i as f32 * GRID_STEP;
            piece(Vec3::new(x, 0.0, k as f32 * GRID_STEP), Vec3::new(x, 0.0, (k + 1) as f32 * GRID_STEP));
        }
    }
    for k in 1..nz {
        for i in 0..nx {
            let z = k as f32 * GRID_STEP;
            piece(Vec3::new(i as f32 * GRID_STEP, 0.0, z), Vec3::new((i + 1) as f32 * GRID_STEP, 0.0, z));
        }
    }
    for b in &scene.burners {
        // A ring in the burner's plane.
        let (u, w) = b.dir.any_orthonormal_pair();
        circle(b.pos, u * b.radius, w * b.radius, BURNER, &mut line);
    }
    // Solids aren't lines: the raymarch shades them.
    v
}

fn circle(centre: Vec3, u: Vec3, w: Vec3, colour: [f32; 4], line: &mut impl FnMut(Vec3, Vec3, [f32; 4])) {
    let point = |k: usize| {
        let a = std::f32::consts::TAU * k as f32 / CIRCLE_SEGMENTS as f32;
        centre + u * a.cos() + w * a.sin()
    };
    for k in 0..CIRCLE_SEGMENTS {
        line(point(k), point(k + 1), colour);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_uniforms_match_the_shaders() {
        assert_eq!((size_of::<LightUniforms>(), size_of::<ResolveUniforms>()), (48, 96));
    }

    /// The raymarch's, the light volume's and the gathering's shaders validate.
    #[test]
    fn the_shaders_validate() {
        use wgpu::naga;
        for (name, source) in [("march.wgsl", include_str!("march.wgsl")), ("light.wgsl", include_str!("light.wgsl")), ("history.wgsl", include_str!("history.wgsl"))] {
            let module = naga::front::wgsl::parse_str(source).unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(source)));
            naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
                .validate(&module)
                .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(source)));
        }
    }

    /// No box: only the floor grid, round the middle and fading out, and the burners.
    #[test]
    fn outlines_are_a_fading_floor_and_the_burners() {
        let scene = sim3d::scene::campfire();
        let v = outline(&scene);
        assert_eq!(v.len() % 2, 0);
        let middle = Vec3::new(0.5 * scene.size.x, 0.0, 0.5 * scene.size.z);
        let brightness = |c: [f32; 4]| c[0] + c[1] + c[2];
        for pair in v.chunks(2).filter(|p| p[0].colour != BURNER) {
            let (a, b) = (Vec3::from(pair[0].pos), Vec3::from(pair[1].pos));
            assert!(a.y == 0.0 && b.y == 0.0, "a line off the floor");
            let far = (0.5 * (a + b)).distance(middle);
            assert!(far < GRID_FADE.end, "a line out at {far}");
            if far > GRID_FADE.start + 0.1 {
                assert!(brightness(pair[0].colour) < brightness(GRID), "no fade at {far}");
            }
        }
        assert!(v.iter().any(|x| x.colour == BURNER));
        for p in sim3d::PRESETS {
            assert_eq!(outline(&(p.build)()).len() % 2, 0, "{}", p.name);
        }
    }

    /// The march gets every solid in the shader's encoding, a step in cells, and
    /// marches only when there's a volume.
    #[test]
    fn march_parameters_carry_the_solids_and_the_grid() {
        let scene = sim3d::scene::chimney();
        let cells = glam::UVec3::new(107, 160, 107);
        let m = march_uniforms(Mat4::IDENTITY, Vec3::ONE, [0.0, 0.0, 800.0, 600.0], &scene, Some((cells, false)), &Palette::default());
        assert_eq!(m.eye[3], 0.0, "smoke glows");
        let lit = march_uniforms(Mat4::IDENTITY, Vec3::ONE, [0.0; 4], &scene, Some((cells, true)), &Palette::default());
        assert!(lit.eye[3] > 0.0, "fire doesn't glow");
        assert_eq!(m.counts, [scene.solids.len() as u32, 1, 0, 0]);
        for k in 0..scene.solids.len() {
            assert_eq!(m.shapes[2 * k][3], -1.0, "the chimney's solids are boxes");
            assert!(Vec3::from_slice(&m.shapes[2 * k + 1][..3]).cmpge(Vec3::from_slice(&m.shapes[2 * k][..3])).all(), "max below min");
        }
        assert!((m.size[3] - STEP_CELLS * scene.size.y / 160.0).abs() < 1e-6);
        let twin = sim3d::scene::twin_burners();
        let none = march_uniforms(Mat4::IDENTITY, Vec3::ONE, [0.0; 4], &twin, None, &Palette::default());
        assert_eq!(none.counts[1], 0, "marched with no volume");
        assert!(none.shapes[0][3] > 0.0, "a sphere keeps its radius");
    }
}
