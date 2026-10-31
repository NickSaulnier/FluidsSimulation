//! Screen-space liquid look (after van der Laan et al., "Screen Space Fluid Rendering",
//! reduced to 2D): splat every particle as a Gaussian into an offscreen density field,
//! then threshold and shade that field into a continuous surface.

use super::particles::ParticleLayer;
use bytemuck::{Pod, Zeroable};
use std::f32::consts::PI;

/// Accumulation target: half floats, which are blendable (32-bit floats need an optional
/// feature to blend).
const ACCUM_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

#[derive(Clone, Copy, Debug)]
pub struct LiquidStyle {
    /// Blob radius in particle radii.
    pub splat_scale: f32,
    /// Surface level, as a fraction of the fluid's interior density.
    pub threshold: f32,
}

/// Must match `Liquid` in liquid.wgsl (16 bytes).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    splat_scale: f32,
    threshold: f32,
    expected: f32,
    splat_px: f32,
}

/// Summed splat weight inside the fluid. Each splat is exp(-4 (r/R)²) over its radius
/// R = k * particle radius, integrating to ≈ 0.98 π R² / 4; every solver spaces particles
/// at two radii, so the sum per particle area (2r)² is ≈ 0.98 π k² / 16. Independent of
/// resolution and zoom, which is what lets one threshold work for every solver.
pub fn expected_density(splat_scale: f32) -> f32 {
    let truncation = 1.0 - (-4.0f32).exp();
    truncation * PI * splat_scale * splat_scale / 16.0
}

pub struct LiquidLayer {
    splat_pipeline: wgpu::RenderPipeline,
    composite_pipeline: wgpu::RenderPipeline,
    params: wgpu::Buffer,
    splat_group: wgpu::BindGroup,
    composite_layout: wgpu::BindGroupLayout,
    /// The accumulation texture, its view, and the composite bind group reading it.
    target: Option<(wgpu::Texture, wgpu::TextureView, wgpu::BindGroup)>,
}

impl LiquidLayer {
    pub fn new(device: &wgpu::Device, globals_layout: &wgpu::BindGroupLayout, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("liquid.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("liquid.wgsl").into()),
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("liquid params"),
            size: size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let params_entry = wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(size_of::<Params>() as u64),
            },
            count: None,
        };
        // The splat pass renders into the accumulation texture, so it must not also have
        // it bound: separate layouts for the two passes.
        let splat_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("liquid splat"),
            entries: &[params_entry],
        });
        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("liquid composite"),
            entries: &[
                params_entry,
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let splat_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("liquid splat"),
            layout: &splat_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() }],
        });

        let pipeline = |label, layout: &wgpu::BindGroupLayout, vs, fs, buffers, target: wgpu::ColorTargetState| {
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(globals_layout), Some(layout)],
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(target)],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let additive = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let splat_pipeline = pipeline(
            "liquid splat",
            &splat_layout,
            "vs_splat",
            "fs_splat",
            &ParticleLayer::INSTANCE_LAYOUTS,
            wgpu::ColorTargetState {
                format: ACCUM_FORMAT,
                blend: Some(wgpu::BlendState { color: additive, alpha: additive }),
                write_mask: wgpu::ColorWrites::ALL,
            },
        );
        let composite_pipeline = pipeline(
            "liquid composite",
            &composite_layout,
            "vs_composite",
            "fs_composite",
            &[],
            wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            },
        );
        Self { splat_pipeline, composite_pipeline, params, splat_group, composite_layout, target: None }
    }

    /// Size the accumulation target to the surface and upload this frame's style.
    /// `splat_px` is the blob radius on screen, which sets the smoothing scale.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size: (u32, u32),
        style: LiquidStyle,
        splat_px: f32,
    ) {
        let extent = wgpu::Extent3d { width: size.0.max(1), height: size.1.max(1), depth_or_array_layers: 1 };
        if self.target.as_ref().is_none_or(|(t, _, _)| t.size() != extent) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("liquid accumulation"),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: ACCUM_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("liquid composite"),
                layout: &self.composite_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.params.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
                ],
            });
            self.target = Some((texture, view, group));
        }
        let params = Params {
            splat_scale: style.splat_scale,
            threshold: style.threshold,
            expected: expected_density(style.splat_scale),
            splat_px,
        };
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
    }

    /// Pass 1: clear the accumulation target and splat every particle into it.
    pub fn splat(&self, encoder: &mut wgpu::CommandEncoder, globals: &wgpu::BindGroup, particles: &ParticleLayer) {
        let Some((_, view, _)) = &self.target else { return };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("liquid splat"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.splat_pipeline);
        pass.set_bind_group(0, globals, &[]);
        pass.set_bind_group(1, &self.splat_group, &[]);
        let count = particles.bind_instances(&mut pass);
        if count > 0 {
            pass.draw(0..4, 0..count);
        }
    }

    /// Pass 2 (inside the scene pass): threshold and shade the field over the domain.
    /// Expects the globals bind group at index 0 to be set already.
    pub fn composite(&self, pass: &mut wgpu::RenderPass<'_>) {
        if let Some((_, _, group)) = &self.target {
            pass.set_pipeline(&self.composite_pipeline);
            pass.set_bind_group(1, group, &[]);
            pass.draw(0..4, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sum the splat weights at a point inside an infinite square lattice of particles
    /// spaced at two radii: it must match `expected_density` for any blob size.
    #[test]
    fn interior_density_matches_the_closed_form() {
        let radius = 1.0f32;
        let spacing = 2.0 * radius;
        for k in [2.0f32, 3.0, 4.5] {
            let reach = (k * radius / spacing).ceil() as i32 + 1;
            // Average over sample points within a lattice cell, since the sum ripples.
            let mut mean = 0.0;
            let samples = 8;
            for sy in 0..samples {
                for sx in 0..samples {
                    let p = glam::Vec2::new(sx as f32 + 0.5, sy as f32 + 0.5) / samples as f32 * spacing;
                    let mut sum = 0.0;
                    for j in -reach..=reach {
                        for i in -reach..=reach {
                            let d2 = (p - glam::Vec2::new(i as f32, j as f32) * spacing).length_squared() / (k * radius).powi(2);
                            if d2 < 1.0 {
                                sum += (-4.0 * d2).exp();
                            }
                        }
                    }
                    mean += sum;
                }
            }
            mean /= (samples * samples) as f32;
            let expected = expected_density(k);
            assert!((mean / expected - 1.0).abs() < 0.02, "k = {k}: lattice {mean}, closed form {expected}");
        }
    }
}
