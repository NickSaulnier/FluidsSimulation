//! Frame orchestration: shared uniforms, the domain backdrop, and the particle layer.
//! The 2D scene draws into the post-processing's HDR scene texture (`post`), not the
//! screen.

pub mod art;
pub mod bend;
pub mod colormap;
pub mod composite;
mod liquid;
pub mod feedback;
pub mod fractal;
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
pub use solid::WallBrush;
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
    /// Draw the walls (with feedback on, they're drawn after it by `draw_walls`).
    pub walls: bool,
    /// The wall a brush would leave, previewed.
    pub brush: Option<WallBrush>,
}

pub struct Renderer {
    globals: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    backdrop: wgpu::RenderPipeline,
    particles: ParticleLayer,
    solid: SolidLayer,
    overlay: OverlayLayer,
    liquid: LiquidLayer,
    /// Whether the last frame previewed a brush.
    brush: bool,
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
        Self { globals, bind_group, backdrop, particles, solid, overlay, liquid, brush: false }
    }

    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        options: DrawOptions<'_>,
    ) -> Option<(f32, f32)> {
        let DrawOptions { view, world, field, show_particles, liquid, target_size, pixel_scale, black_letterbox, palette, walls, brush } = options;
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
        self.solid.upload(device, queue, &world.scene.solid, world.scene_generation, brush);
        self.brush = brush.is_some();
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
        if walls {
            self.solid.draw(&mut pass, self.brush);
        }
        field_range
    }

    /// The walls over `target`, as the last `render` set them up: after the feedback,
    /// so they don't smear into it.
    pub fn draw_walls(&self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("walls"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        pass.set_bind_group(0, &self.bind_group, &[]);
        self.solid.draw(&mut pass, self.brush);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::post::{SCENE_FORMAT, tests::half_float};
    use glam::Vec2;

    /// The walls on a headless GPU: the obstacle drawn solid and shaded, the shadow
    /// darkening the open space beside it, a painting brush showing a faint wall where
    /// there is none, an erasing brush tinting the wall it would take away. With
    /// `WALL_SHOTS` set to a folder, the pictures are saved there too, to look at.
    #[test]
    fn walls_are_drawn_shaded_and_previewed() {
        let gpu = match sim3d::GpuContext::new() {
            Ok(g) => g,
            Err(e) => return eprintln!("skipping: {e:#}"),
        };
        let (device, queue) = (&gpu.device, &gpu.queue);
        let mut renderer = Renderer::new(device, SCENE_FORMAT);
        let size = [640u32, 360];
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("walls test"),
            size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SCENE_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view_target = target.create_view(&Default::default());
        let tank = sim::PRESETS.iter().position(|p| p.name == "Tank with obstacle").expect("the obstacle scene");
        let mut world = sim::World::new(0, tank);
        world.domain = sim::domain_for(16.0 / 9.0);
        world.load_preset(tank);
        let inside = world.scene.solid.interior();
        let window = winit::dpi::PhysicalSize::new(size[0], size[1]);
        let view = View::cover(inside.min, inside.size(), Vec2::ZERO, Vec2::new(size[0] as f32, size[1] as f32), window);
        // The obstacle's middle: the most solid point of the mask, on screen.
        let (nx, ny, occ) = world.scene.solid.occupancy();
        let cell = world.scene.solid.cell_size();
        let border = sim::BORDER_CELLS;
        let (mut sum, mut count) = (Vec2::ZERO, 0.0);
        for j in border..ny - border {
            for i in border..nx - border {
                if occ[j * nx + i] != 0 {
                    sum += (Vec2::new(i as f32, j as f32) + 0.5) * cell;
                    count += 1.0;
                }
            }
        }
        assert!(count > 0.0, "the scene has an obstacle");
        let centre = sum / count;
        let pixel_of = |p: Vec2| {
            let clip = view.to_clip(p);
            let x = ((clip.x + 1.0) * 0.5 * size[0] as f32) as usize;
            let y = ((1.0 - clip.y) * 0.5 * size[1] as f32) as usize;
            y.min(size[1] as usize - 1) * size[0] as usize + x.min(size[0] as usize - 1)
        };
        let radius = world.scene.solid.distance(centre).abs();
        let open = centre + Vec2::new(radius + 0.25, 0.25);
        let mut draw = |brush: Option<WallBrush>, name: &str| {
            let mut encoder = device.create_command_encoder(&Default::default());
            let options = DrawOptions {
                view: &view,
                world: &world,
                field: None,
                show_particles: false,
                liquid: None,
                target_size: (size[0], size[1]),
                pixel_scale: 1.0,
                black_letterbox: true,
                palette: palette::PALETTES[0].1,
                walls: true,
                brush,
            };
            renderer.render(device, queue, &mut encoder, &view_target, options);
            queue.submit([encoder.finish()]);
            let pixels = half_float::read(device, queue, &target, size);
            if let Some(folder) = std::env::var_os("WALL_SHOTS") {
                save(&std::path::Path::new(&folder).join(format!("{name}.png")), &pixels, size);
            }
            pixels
        };
        let brightness = |p: [f32; 4]| p[0] + p[1] + p[2];
        let plain = draw(None, "walls");
        let backdrop = brightness(plain[pixel_of(open)]);
        let wall = plain[pixel_of(centre)];
        assert!(brightness(wall) > backdrop + 0.01, "the wall stands out from the backdrop: {wall:?}");
        // Along a line out of the obstacle's middle, past its edge (which the smoothing
        // puts a few millimetres from the mask's): the darkest point away from the
        // light is a shadow, the brightest toward it the lit rim.
        let along = |direction: Vec2| (0..40).map(move |k| centre + direction.normalize() * (radius - 0.01 + 0.001 * k as f32));
        let darkest = along(Vec2::new(0.6, -0.8)).map(|q| brightness(plain[pixel_of(q)])).fold(f32::MAX, f32::min);
        assert!(darkest < 0.5 * backdrop, "a shadow beside the wall: {darkest} against {backdrop}");
        let brightest = along(Vec2::new(-0.6, 0.8)).map(|q| brightness(plain[pixel_of(q)])).fold(0.0, f32::max);
        assert!(brightest > 1.5 * brightness(wall), "the rim catches the light: {brightest}");
        let painting = draw(Some(WallBrush { pos: open, radius: 0.05, paint: true }), "walls_paint");
        let ghost = brightness(painting[pixel_of(open)]);
        assert!(ghost > backdrop + 0.005 && ghost < brightness(wall) + 0.05, "a faint wall where the brush is: {ghost}");
        let erasing = draw(Some(WallBrush { pos: centre, radius: 0.05, paint: false }), "walls_erase");
        let red = erasing[pixel_of(centre)];
        assert!(red[0] > red[1] * 1.5 && red[0] > wall[0], "the wall to go shows red: {red:?}");
    }

    /// The scene texture as an 8-bit PNG (clamped, roughly sRGB), for looking at.
    fn save(path: &std::path::Path, pixels: &[[f32; 4]], size: [u32; 2]) {
        let bytes: Vec<u8> = pixels
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]].map(|c| (c.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0) as u8).into_iter().chain([255]))
            .collect();
        let file = std::fs::File::create(path).expect("create the picture");
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), size[0], size[1]);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header().and_then(|mut w| w.write_image_data(&bytes)).expect("write the picture");
    }
}
