//! Escape-time fractals (Mandelbrot, Julia and others), drawn into the scene before the
//! feedback, as the cover art is (see `art`), so the warp carries them. Each pixel
//! iterates the fractal in `fractal.wgsl`; the colour comes from the look's palette, and
//! only the filaments near the set are opaque, so the fluid shows through elsewhere. A
//! look asks for one in `[fractal]` (part of its waves, `WaveSettings::fractal`).
//!
//! The shader works in f32: past about 10⁵× zoom the picture would break into blocks,
//! so zoom is capped there (`MAX_ZOOM`).

use super::palette::Palette;
use super::post::SCENE_FORMAT;
use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};
use std::f32::consts::TAU;

/// The deepest zoom f32 holds without blocks.
pub const MAX_ZOOM: f32 = 1e5;
pub const MAX_ITERATIONS: u32 = 1000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FractalKind {
    /// z² + c, c the pixel.
    #[default]
    #[serde(alias = "mandelbrot")]
    Mandelbrot,
    /// z² + c, z the pixel and c the look's `julia` point.
    #[serde(alias = "julia")]
    Julia,
    /// (|re z| + i |im z|)² + c: a ship, its sails along the negative real axis.
    #[serde(alias = "burning_ship", alias = "burningship")]
    BurningShip,
    /// conj(z)² + c: the Mandelbrot set's three-cornered cousin.
    #[serde(alias = "tricorn")]
    Tricorn,
    /// Newton's method on z³ − 1: three basins, coloured by the root each pixel finds.
    #[serde(alias = "newton")]
    Newton,
}

impl FractalKind {
    pub const ALL: [Self; 5] = [Self::Mandelbrot, Self::Julia, Self::BurningShip, Self::Tricorn, Self::Newton];

    pub fn label(self) -> &'static str {
        match self {
            Self::Mandelbrot => "Mandelbrot",
            Self::Julia => "Julia",
            Self::BurningShip => "Burning ship",
            Self::Tricorn => "Tricorn",
            Self::Newton => "Newton",
        }
    }

    /// The shader's number for it.
    fn code(self) -> u32 {
        self as u32
    }
}

/// Which fractal, where, and how it's coloured.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FractalSettings {
    pub enabled: bool,
    pub kind: FractalKind,
    /// The point in the fractal's plane at the canvas's centre.
    pub centre: [f32; 2],
    /// 1: the canvas is 3 units of the plane high (the whole Mandelbrot set fits).
    pub zoom: f32,
    /// Turn, radians.
    pub rotation: f32,
    /// Turn, radians per second.
    pub spin: f32,
    pub iterations: u32,
    /// Julia's constant `c`.
    pub julia: [f32; 2],
    /// 0 (unseen) to 1.
    pub opacity: f32,
    /// Light, 0 to 4 (above 1 glows).
    pub brightness: f32,
    /// How many times the palette repeats from the far outside to the set's edge.
    pub bands: f32,
    /// The palette's cycle, turns per second.
    pub colour_speed: f32,
}

impl FractalSettings {
    pub const OFF: Self = Self {
        enabled: false,
        kind: FractalKind::Mandelbrot,
        centre: [-0.6, 0.0],
        zoom: 1.0,
        rotation: 0.0,
        spin: 0.0,
        iterations: 160,
        julia: [-0.8, 0.156],
        opacity: 1.0,
        bands: 2.0,
        brightness: 1.0,
        colour_speed: 0.05,
    };
}

impl Default for FractalSettings {
    fn default() -> Self {
        Self::OFF
    }
}

/// Must match `Fractal` in fractal.wgsl (112 bytes).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    /// The palette in linear RGB: deep, base, highlight.
    palette: [[f32; 4]; 3],
    centre: [f32; 2],
    /// The turn as (cos, sin).
    turn: [f32; 2],
    julia: [f32; 2],
    /// Half the canvas's height, in units of the plane.
    half_height: f32,
    aspect: f32,
    kind: u32,
    iterations: u32,
    opacity: f32,
    brightness: f32,
    bands: f32,
    /// The palette's cycle so far, turns.
    phase: f32,
    /// One pixel, in the canvas's -1 to 1 up the canvas (for the supersamples' spacing).
    pixel: f32,
    _pad: f32,
}

pub struct FractalLayer {
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    group: wgpu::BindGroup,
    /// The spin and the colour cycle, added up.
    turn: f32,
    phase: f32,
}

impl FractalLayer {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("fractal.wgsl"), source: wgpu::ShaderSource::Wgsl(include_str!("fractal.wgsl").into()) });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fractal"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: wgpu::BufferSize::new(size_of::<Uniforms>() as u64) },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("fractal"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        // Premultiplied alpha: the fractal over the scene.
        let over = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fractal"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: SCENE_FORMAT, blend: Some(wgpu::BlendState { color: over, alpha: over }), write_mask: wgpu::ColorWrites::COLOR })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fractal uniforms"),
            size: size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fractal"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() }],
        });
        Self { pipeline, uniforms, group, turn: 0.0, phase: 0.0 }
    }

    /// Advance the spin and the colour cycle by `dt` seconds (call every frame).
    pub fn update(&mut self, settings: &FractalSettings, dt: f32) {
        self.turn = (self.turn + settings.spin * dt) % TAU;
        self.phase = (self.phase + settings.colour_speed * dt).rem_euclid(1.0);
    }

    /// Draw the fractal onto `target` within `viewport` (pixels: x, y, width, height), if
    /// `settings` ask for it.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&self, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView, viewport: [f32; 4], settings: &FractalSettings, palette: &Palette) {
        let [x, y, w, h] = viewport;
        if !settings.enabled || settings.opacity <= 0.0 || w < 1.0 || h < 1.0 {
            return;
        }
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&self.uniforms(settings, palette, w / h, h)));
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("fractal"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        pass.set_viewport(x, y, w, h, 0.0, 1.0);
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.group, &[]);
        pass.draw(0..3, 0..1);
    }

    fn uniforms(&self, s: &FractalSettings, palette: &Palette, aspect: f32, height_px: f32) -> Uniforms {
        let half_height = 1.5 / view_zoom(s.zoom);
        let turn = s.rotation + self.turn;
        Uniforms {
            palette: palette.linear(),
            centre: s.centre.map(|c| if c.is_finite() { c.clamp(-4.0, 4.0) } else { 0.0 }),
            turn: [turn.cos(), turn.sin()],
            julia: s.julia.map(|c| if c.is_finite() { c.clamp(-4.0, 4.0) } else { 0.0 }),
            half_height,
            aspect,
            kind: s.kind.code(),
            iterations: s.iterations.clamp(1, MAX_ITERATIONS),
            opacity: s.opacity.clamp(0.0, 1.0),
            brightness: s.brightness.max(0.0),
            bands: s.bands.max(0.0),
            phase: self.phase,
            pixel: 2.0 / height_px,
            _pad: 0.0,
        }
    }
}

/// `zoom` as drawn: positive, finite, and no deeper than f32 holds.
pub fn view_zoom(zoom: f32) -> f32 {
    if zoom.is_finite() { zoom.clamp(1e-3, MAX_ZOOM) } else { 1.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_stays_where_f32_holds() {
        assert_eq!(view_zoom(1e9), MAX_ZOOM);
        assert_eq!(view_zoom(f32::NAN), 1.0);
        assert_eq!(view_zoom(0.0), 1e-3);
        assert_eq!(view_zoom(4.0), 4.0);
    }

    #[test]
    fn the_uniforms_match_the_shader() {
        assert_eq!(size_of::<Uniforms>(), 112);
    }

    #[test]
    fn kinds_have_the_shaders_numbers() {
        // fractal.wgsl's MANDELBROT .. NEWTON.
        let codes: Vec<u32> = FractalKind::ALL.iter().map(|k| k.code()).collect();
        assert_eq!(codes, [0, 1, 2, 3, 4]);
    }
}
