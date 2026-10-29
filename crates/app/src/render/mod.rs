//! Frame orchestration: shared uniforms, the domain backdrop, and the particle layer.
//! The 2D scene draws into the post-processing's HDR scene texture (`post`), not the
//! screen.

pub mod art;
pub mod colormap;
pub mod composite;
mod liquid;
pub mod feedback;
pub mod grab;
pub mod palette;
mod overlay;
mod particles;
pub mod post;
mod solid;
pub mod thumbnail;
pub mod timer;
pub mod waves;

use crate::view::View;
use bytemuck::{Pod, Zeroable};
pub use liquid::LiquidStyle;
pub use overlay::FieldFrame;
pub use palette::Palette;
use liquid::LiquidLayer;
use overlay::OverlayLayer;
use particles::ParticleLayer;
use solid::SolidLayer;

/// Must match `Globals` in scene.wgsl (and its copies) byte for byte (80 bytes, a
/// multiple of 16 as uniform buffers require).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    scale: [f32; 2],
    offset: [f32; 2],
    domain: [f32; 2],
    radius: f32,
    max_speed: f32,
    /// The fluid palette in linear RGB: deep, base, highlight.
    fluid: [[f32; 4]; 3],
}

/// Everything one frame draws.
pub struct DrawOptions<'a> {
    pub view: &'a View,
    pub world: &'a sim::World,
    pub field: Option<FieldFrame<'a>>,
    pub show_particles: bool,
    /// Draw particles as a continuous liquid surface instead of discs.
    pub liquid: Option<LiquidStyle>,
    /// Target size in pixels (the liquid pass renders offscreen at this size).
    pub target_size: (u32, u32),
    /// Target pixels per window pixel (the render scale), for sizes set in pixels.
    pub pixel_scale: f32,
    /// Black around the domain instead of the walls' grey (under feedback, which would
    /// carry the grey into the picture).
    pub black_letterbox: bool,
    /// The fluid's colours (particles, liquid surface, dye).
    pub palette: Palette,
}

pub struct Renderer {
    globals: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    backdrop: wgpu::RenderPipeline,
    particles: ParticleLayer,
    solid: SolidLayer,
    overlay: OverlayLayer,
    liquid: LiquidLayer,
}

/// Colour of the letterbox area around the domain; reads as the walls' outer edge.
const LETTERBOX: wgpu::Color = wgpu::Color { r: 0.06, g: 0.065, b: 0.075, a: 1.0 };
const MAX_SPEED_COLOUR: f32 = 3.0;

impl Renderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("scene.wgsl").into()),
        });

        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(size_of::<Globals>() as u64),
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let backdrop = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("backdrop"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_backdrop"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_backdrop"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });

        let particles = ParticleLayer::new(device, &layout, &shader, format);
        let solid = SolidLayer::new(device, &bind_group_layout, &shader, format);
        let overlay = OverlayLayer::new(device, &bind_group_layout, format);
        let liquid = LiquidLayer::new(device, &bind_group_layout, format);
        Self { globals, bind_group, backdrop, particles, solid, overlay, liquid }
    }

    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        options: DrawOptions<'_>,
    ) -> Option<(f32, f32)> {
        let DrawOptions { view, world, field, show_particles, liquid, target_size, pixel_scale, black_letterbox, palette } = options;
        let particles = world.solver.particles();
        let globals = Globals {
            scale: view.scale.into(),
            offset: view.offset.into(),
            domain: world.scene.size.into(),
            radius: particles.map_or(0.0, |p| p.radius),
            max_speed: MAX_SPEED_COLOUR,
            fluid: palette.linear(),
        };
        queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));
        if let Some(p) = particles {
            self.particles.upload(device, queue, p);
        }
        self.solid.upload(device, queue, &world.scene.solid);
        let field_range = self.overlay.upload(device, queue, field, &palette);
        let liquid = liquid.filter(|_| show_particles && particles.is_some());
        if let Some(style) = liquid {
            let splat_px = globals.radius * style.splat_scale * view.px_per_metre * pixel_scale;
            self.liquid.prepare(device, queue, target_size, style, splat_px);
            self.liquid.splat(encoder, &self.bind_group, &self.particles);
        }

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scene"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(if black_letterbox { wgpu::Color::BLACK } else { LETTERBOX }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_pipeline(&self.backdrop);
        pass.draw(0..4, 0..1);
        self.overlay.draw(&mut pass);
        if liquid.is_some() {
            self.liquid.composite(&mut pass);
        } else if particles.is_some() && show_particles {
            self.particles.draw(&mut pass);
        }
        // Walls over particles: hides the sub-cell overlap grid solvers allow.
        self.solid.draw(&mut pass);
        field_range
    }
}
