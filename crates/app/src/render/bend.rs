//! The fluid bending the fractal (see `fractal`): a displacement texture that the
//! fluid's flow drags along, and that springs back home. Each frame, every pixel's
//! displacement is the one carried in from upstream plus the flow's push, eased back
//! toward none (`bend.wgsl`). The fractal is drawn where its picture would be had the
//! fluid carried it: swirls twist it, jets stretch it, and it settles when the fluid
//! does.
//!
//! The flow is the feedback's (`feedback::FLOW_FORMAT`: screen uv per second, y down);
//! the displacement is in the same uv.

use super::feedback::{FLOW_FORMAT, fullscreen, fullscreen_pipeline, texture};
use bytemuck::{Pod, Zeroable};

/// How long the displacement takes to spring back (time constant, seconds).
pub const RELAX_S: f32 = 0.6;
/// The farthest any pixel is carried, in uv (a long steady flow saturates here).
const MAX_SHIFT: f32 = 0.12;
/// How far around the flow is averaged, in uv up the scene: particle solvers' flow,
/// splatted from particles, is noisy, and would shred the picture.
const BLUR: f32 = 0.015;

/// Must match `Bend` in bend.wgsl (16 bytes).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    /// The flow's push this frame: its gain times the frame time.
    push: f32,
    /// What's kept of the displacement this frame (the spring back).
    keep: f32,
    max_shift: f32,
    blur: f32,
}

pub struct BendLayer {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniforms: wgpu::Buffer,
    /// The displacement, twice (this frame's from last frame's), at the scene's size.
    targets: Option<([wgpu::TextureView; 2], [u32; 2])>,
    current: usize,
}

impl BendLayer {
    pub fn new(device: &wgpu::Device) -> Self {
        let entry = |binding: u32, ty: wgpu::BindingType| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty, count: None };
        let texture_entry = |binding: u32| {
            entry(binding, wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false })
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bend"),
            entries: &[
                entry(0, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: wgpu::BufferSize::new(size_of::<Uniforms>() as u64) }),
                texture_entry(1),
                texture_entry(2),
                entry(3, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
            ],
        });
        let pipeline = fullscreen_pipeline(device, &layout, include_str!("bend.wgsl"), "fs_bend", FLOW_FORMAT);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("bend"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bend uniforms"),
            size: size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { pipeline, layout, sampler, uniforms, targets: None, current: 0 }
    }

    /// Move the displacement on by `dt` seconds of `flow` (`size` pixels: the scene's),
    /// carried at `gain` (1: with the fluid). A new size starts from none.
    #[allow(clippy::too_many_arguments)]
    pub fn update(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, flow: &wgpu::TextureView, size: [u32; 2], gain: f32, dt: f32) {
        if self.targets.as_ref().is_none_or(|t| t.1 != size) {
            let make = || texture(device, "bend", size, FLOW_FORMAT);
            let targets = [make(), make()];
            for t in &targets {
                super::feedback::clear(encoder, t);
            }
            self.targets = Some((targets, size));
        }
        let Some((targets, _)) = &self.targets else { return };
        let dt = dt.max(0.0);
        let uniforms = Uniforms { push: gain.max(0.0) * dt, keep: (-dt / RELAX_S).exp(), max_shift: MAX_SHIFT, blur: BLUR };
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&uniforms));
        let (from, to) = (&targets[self.current], &targets[1 - self.current]);
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bend"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniforms.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(from) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(flow) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        fullscreen(encoder, "bend", to, &self.pipeline, &group);
        self.current = 1 - self.current;
    }

    /// The displacement as it stands (scene uv), if it's running.
    pub fn displacement(&self) -> Option<&wgpu::TextureView> {
        self.targets.as_ref().map(|(t, _)| &t[self.current])
    }

    /// Stop, and start from none next time (it isn't wanted for now).
    pub fn reset(&mut self) {
        self.targets = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::post::tests::half_float;

    /// The displacement (x, y in uv) after `frames` frames of a steady `flow` (uv per
    /// second) over the left half of a 64 × 32 scene, then `still` frames of none.
    fn run(flow: f32, frames: usize, still: usize) -> Vec<[f32; 2]> {
        let gpu = sim3d::GpuContext::new().expect("gpu");
        let (device, queue) = (&gpu.device, &gpu.queue);
        let size = [64u32, 32];
        let flow_texture = texture(device, "flow", size, FLOW_FORMAT);
        let write = |v: f32| {
            let one = half_float::to_f16(v);
            let texels: Vec<[u16; 2]> = (0..size[1]).flat_map(|_| (0..size[0]).map(|x| if x < 32 { [one, 0] } else { [0, 0] })).collect();
            queue.write_texture(
                flow_texture.texture().as_image_copy(),
                bytemuck::cast_slice(&texels),
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size[0] * 4), rows_per_image: None },
                flow_texture.texture().size(),
            );
        };
        let mut layer = BendLayer::new(device);
        let mut step = |n: usize| {
            for _ in 0..n {
                let mut encoder = device.create_command_encoder(&Default::default());
                layer.update(device, queue, &mut encoder, &flow_texture, size, 1.0, 1.0 / 60.0);
                queue.submit([encoder.finish()]);
            }
        };
        write(flow);
        step(frames);
        write(0.0);
        step(still);
        let shown = layer.displacement().expect("running").texture().clone();
        half_float::read_channels(device, queue, &shown, size, 2).chunks(2).map(|c| [c[0], c[1]]).collect()
    }

    /// A steady flow carries the picture along (toward its pace times the spring's
    /// time), and still fluid leaves it be; when the flow stops, it springs back.
    #[test]
    fn the_flow_drags_the_picture_and_it_springs_back() {
        if let Err(e) = sim3d::GpuContext::new() {
            return eprintln!("skipping: {e:#}");
        }
        let at = |d: &[[f32; 2]], x: usize| d[16 * 64 + x];
        let blown = run(0.1, 120, 0);
        let steady = 0.1 * RELAX_S * (1.0 - (-2.0 / RELAX_S).exp());
        assert!((at(&blown, 10)[0] - steady).abs() < 0.2 * steady, "left: {:?}, expected about {steady}", at(&blown, 10));
        assert!(at(&blown, 10)[1].abs() < 1e-3, "no flow up or down: {:?}", at(&blown, 10));
        // Where the fluid is still, the picture stays put.
        assert!(at(&blown, 44)[0].abs() < 1e-3 && at(&blown, 60)[0].abs() < 1e-3, "still half: {:?}, {:?}", at(&blown, 44), at(&blown, 60));
        let settled = run(0.1, 120, 300);
        // Five seconds still: back by the spring's time constant.
        let left = at(&blown, 10)[0] * (-5.0 / RELAX_S).exp();
        assert!((at(&settled, 10)[0] - left).abs() < 0.3 * left, "it didn't spring back: {:?}, expected about {left}", at(&settled, 10));
        // A strong steady flow saturates.
        let gale = run(5.0, 600, 0);
        assert!(at(&gale, 10)[0] <= MAX_SHIFT + 1e-3, "{:?}", at(&gale, 10));
    }
}
