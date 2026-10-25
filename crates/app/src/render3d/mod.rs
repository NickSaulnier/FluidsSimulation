//! The 3D view: an orbit camera and, for now, the scene in outline (domain box, floor
//! grid, burners, vents, solids). The raymarched volume joins it in a later phase.

mod camera;

pub use camera::OrbitCamera;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use sim3d::{Scene3d, Shape};
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 3],
    colour: [f32; 4],
}

/// Linear-space colours (the surface is sRGB).
const EDGE: [f32; 4] = [0.35, 0.38, 0.45, 1.0];
const GRID: [f32; 4] = [0.12, 0.13, 0.16, 1.0];
const BURNER: [f32; 4] = [1.0, 0.35, 0.05, 1.0];
const VENT: [f32; 4] = [0.15, 0.4, 0.8, 1.0];
const SOLID: [f32; 4] = [0.55, 0.57, 0.62, 1.0];
const GRID_STEP: f32 = 0.1;
const CIRCLE_SEGMENTS: usize = 40;
/// The colour around and behind the domain, matching the 2D view's letterbox.
const BACKGROUND: wgpu::Color = wgpu::Color { r: 0.012, g: 0.014, b: 0.022, a: 1.0 };

pub struct Renderer3d {
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    vertices: Option<(wgpu::Buffer, u32)>,
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
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        Self { pipeline, uniforms, bind_group, vertices: None }
    }

    /// Clear the target and draw `scene` from `camera` into the `viewport` (physical
    /// pixels: x, y, width, height), the canvas left by the UI.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        viewport: [f32; 4],
        camera: &OrbitCamera,
        scene: &Scene3d,
    ) {
        let [x, y, w, h] = viewport;
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&camera.view_proj(w / h.max(1.0))));
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

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("3d"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(BACKGROUND), store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        if w < 1.0 || h < 1.0 {
            return;
        }
        pass.set_viewport(x, y, w, h, 0.0, 1.0);
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        if let Some((buffer, count)) = &self.vertices {
            pass.set_vertex_buffer(0, buffer.slice(..));
            pass.draw(0..*count, 0..1);
        }
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
    // Floor grid, then the box edges over it.
    let (nx, nz) = ((s.x / GRID_STEP).round() as usize, (s.z / GRID_STEP).round() as usize);
    for i in 1..nx {
        let x = i as f32 * GRID_STEP;
        line(Vec3::new(x, 0.0, 0.0), Vec3::new(x, 0.0, s.z), GRID);
    }
    for k in 1..nz {
        let z = k as f32 * GRID_STEP;
        line(Vec3::new(0.0, 0.0, z), Vec3::new(s.x, 0.0, z), GRID);
    }
    box_edges(Vec3::ZERO, s, EDGE, &mut line);
    for vent in &scene.vents {
        box_edges(vent.min, vent.max, VENT, &mut line);
    }
    for b in &scene.burners {
        // A ring in the burner's plane.
        let (u, w) = b.dir.any_orthonormal_pair();
        circle(b.pos, u * b.radius, w * b.radius, BURNER, &mut line);
    }
    for solid in &scene.solids {
        match *solid {
            Shape::Sphere { centre, radius } => {
                for (a, b) in [(Vec3::X, Vec3::Y), (Vec3::Y, Vec3::Z), (Vec3::Z, Vec3::X)] {
                    circle(centre, a * radius, b * radius, SOLID, &mut line);
                }
            }
            Shape::Cuboid(b) => box_edges(b.min, b.max, SOLID, &mut line),
        }
    }
    v
}

fn box_edges(min: Vec3, max: Vec3, colour: [f32; 4], line: &mut impl FnMut(Vec3, Vec3, [f32; 4])) {
    let corner = |i: u32| Vec3::new(if i & 1 == 0 { min.x } else { max.x }, if i & 2 == 0 { min.y } else { max.y }, if i & 4 == 0 { min.z } else { max.z });
    for i in 0..8u32 {
        for bit in [1, 2, 4] {
            if i & bit == 0 {
                line(corner(i), corner(i | bit), colour);
            }
        }
    }
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
    fn outlines_have_whole_segments_and_twelve_box_edges() {
        let scene = Scene3d::empty_box(Vec3::new(1.0, 1.5, 1.0));
        let v = outline(&scene);
        assert_eq!(v.len() % 2, 0);
        let edges = v.iter().filter(|x| x.colour == EDGE).count() / 2;
        assert_eq!(edges, 12);
        for p in sim3d::PRESETS {
            assert_eq!(outline(&(p.build)()).len() % 2, 0, "{}", p.name);
        }
    }
}
