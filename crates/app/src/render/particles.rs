//! Instanced anti-aliased discs, one instance per particle.
//!
//! Positions and velocities are separate instance buffers, mirroring the solver's
//! struct-of-arrays layout, so each upload is a single `cast_slice` with no repacking.

use sim::{ParticleView, Vec2};

pub struct ParticleLayer {
    pipeline: wgpu::RenderPipeline,
    pos: wgpu::Buffer,
    vel: wgpu::Buffer,
    /// In particles, not bytes.
    capacity: usize,
    count: u32,
}

const VEC2_BYTES: u64 = size_of::<Vec2>() as u64;

impl ParticleLayer {
    pub fn new(
        device: &wgpu::Device,
        layout: &wgpu::PipelineLayout,
        shader: &wgpu::ShaderModule,
        format: wgpu::TextureFormat,
    ) -> Self {
        const POS_ATTRS: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Float32x2];
        const VEL_ATTRS: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![1 => Float32x2];
        let instance_layout = |attributes: &'static [wgpu::VertexAttribute]| wgpu::VertexBufferLayout {
            array_stride: VEC2_BYTES,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes,
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("particles"),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vs_particle"),
                compilation_options: Default::default(),
                buffers: &[Some(instance_layout(&POS_ATTRS)), Some(instance_layout(&VEL_ATTRS))],
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some("fs_particle"),
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

        let capacity = 1024;
        Self {
            pipeline,
            pos: instance_buffer(device, "particle pos", capacity),
            vel: instance_buffer(device, "particle vel", capacity),
            capacity,
            count: 0,
        }
    }

    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, p: ParticleView<'_>) {
        let n = p.pos.len();
        if n > self.capacity {
            // Grow geometrically so a running emitter doesn't reallocate every frame.
            self.capacity = n.next_power_of_two();
            self.pos = instance_buffer(device, "particle pos", self.capacity);
            self.vel = instance_buffer(device, "particle vel", self.capacity);
        }
        if n > 0 {
            queue.write_buffer(&self.pos, 0, bytemuck::cast_slice(p.pos));
            queue.write_buffer(&self.vel, 0, bytemuck::cast_slice(p.vel));
        }
        self.count = n as u32;
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.count == 0 {
            return;
        }
        let bytes = u64::from(self.count) * VEC2_BYTES;
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.pos.slice(..bytes));
        pass.set_vertex_buffer(1, self.vel.slice(..bytes));
        pass.draw(0..4, 0..self.count);
    }
}

fn instance_buffer(device: &wgpu::Device, label: &str, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: capacity as u64 * VEC2_BYTES,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}
