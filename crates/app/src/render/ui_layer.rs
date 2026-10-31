//! The interface over an HDR surface: egui blends in gamma space and writes gamma
//! values to any target that isn't sRGB, so on a linear (scRGB) surface its colours
//! would come out washed out. Instead it draws into an 8-bit layer, as on SDR, and
//! `ui_layer.wgsl` puts the layer over the surface, decoded and at paper white.

pub struct UiLayer {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    /// The layer, its size and format.
    layer: Option<(wgpu::TextureView, [u32; 2])>,
    format: wgpu::TextureFormat,
}

impl UiLayer {
    /// egui draws into `format` (8 bits, not sRGB); the layer goes over `surface`.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, surface: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("ui_layer.wgsl"), source: wgpu::ShaderSource::Wgsl(include_str!("ui_layer.wgsl").into()) });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ui layer"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: wgpu::BufferSize::new(16) },
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
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("ui layer"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let over = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ui layer"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: surface, blend: Some(wgpu::BlendState { color: over, alpha: over }), write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor { label: Some("ui layer"), size: 16, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        Self { pipeline, layout, uniforms, layer: None, format }
    }

    /// The layer for egui to draw into, `size` pixels, cleared by the caller.
    pub fn target(&mut self, device: &wgpu::Device, size: [u32; 2]) -> wgpu::TextureView {
        if self.layer.as_ref().is_none_or(|(_, s)| *s != size) {
            let view = device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("ui layer"),
                    size: wgpu::Extent3d { width: size[0].max(1), height: size[1].max(1), depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: self.format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default());
            self.layer = Some((view, size));
        }
        self.layer.as_ref().expect("just made").0.clone()
    }

    /// The layer over `surface`, its white at `scale` (the surface's units per linear 1).
    pub fn draw(&self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, surface: &wgpu::TextureView, scale: f32) {
        let Some((layer, _)) = &self.layer else { return };
        queue.write_buffer(&self.uniforms, 0, bytemuck::cast_slice(&[scale, 0.0, 0.0, 0.0]));
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ui layer"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniforms.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(layer) },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ui layer"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: surface,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    /// The layer's shader validates.
    #[test]
    fn the_shader_validates() {
        use wgpu::naga;
        let source = include_str!("ui_layer.wgsl");
        let module = naga::front::wgsl::parse_str(source).unwrap_or_else(|e| panic!("{}", e.emit_to_string(source)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty()).validate(&module).unwrap_or_else(|e| panic!("{}", e.emit_to_string(source)));
    }
}
