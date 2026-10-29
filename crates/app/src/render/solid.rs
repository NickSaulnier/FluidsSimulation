//! Walls: the scene's solid mask drawn as a distance field, so its edges are smooth at
//! any zoom and can be shaded: a bevel lit from the top left with a rim on its edge, a
//! soft shadow on the fluid behind, the fill from the palette's deep colour.
//!
//! The field is built on the CPU when the mask changes (while painting): the mask is
//! sampled bilinearly at `FINE` times its resolution, and each sample's signed distance
//! to the half-full contour found exactly (a separable distance transform), sharpened
//! near the edge from the occupancy's slope. The domain's own border is left out: it
//! lies just outside the window, and drawn it would only shadow the fluid's edges.

use bytemuck::{Pod, Zeroable};
use glam::Vec2;
use rayon::prelude::*;
use sim::SolidMask;

/// Samples of the drawn field per mask cell, along each axis.
const FINE: usize = 3;
/// Distances kept, in fine samples either side of an edge (stored in 8 bits).
const RANGE: f32 = 16.0;

/// The wall a brush would leave, previewed under the cursor.
#[derive(Clone, Copy, Debug)]
pub struct WallBrush {
    pub pos: Vec2,
    pub radius: f32,
    /// Painting (true) or erasing.
    pub paint: bool,
}

/// Must match `Walls` in scene.wgsl (32 bytes).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    /// Centre and radius (metres), and the mode: 0 none, 1 paint, 2 erase.
    brush: [f32; 4],
    /// One fine sample, in texture coordinates.
    texel: [f32; 2],
    /// Metres at either end of the stored range.
    range: f32,
    /// One fine sample, metres.
    step: f32,
}

pub struct SolidLayer {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniforms: wgpu::Buffer,
    /// Texture and its bind group; rebuilt if the mask's resolution changes.
    texture: Option<(wgpu::Texture, wgpu::BindGroup)>,
    /// The scene and mask generations last uploaded.
    uploaded: Option<(u64, u64)>,
    /// Whether the mask has any walls inside the border (if not, nothing is drawn).
    any: bool,
    values: Vec<u8>,
}

impl SolidLayer {
    pub fn new(
        device: &wgpu::Device,
        globals_layout: &wgpu::BindGroupLayout,
        shader: &wgpu::ShaderModule,
        format: wgpu::TextureFormat,
    ) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("walls"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(size_of::<Uniforms>() as u64),
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("walls"),
            bind_group_layouts: &[Some(globals_layout), Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("walls"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vs_solid"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some("fs_solid"),
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
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("walls"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("walls"),
            size: size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { pipeline, layout, sampler, uniforms, texture: None, uploaded: None, any: false, values: Vec::new() }
    }

    /// Rebuild the field if the mask changed (`scene` is the world's scene generation,
    /// which tells a new scene's mask from an edited one), and set the brush preview.
    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, mask: &SolidMask, scene: u64, brush: Option<WallBrush>) {
        let (nx, ny, occ) = mask.occupancy();
        let (fx, fy) = (nx * FINE, ny * FINE);
        let size = wgpu::Extent3d { width: fx as u32, height: fy as u32, depth_or_array_layers: 1 };
        if self.texture.as_ref().is_none_or(|(t, _)| t.size() != size) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("walls"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("walls"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                    wgpu::BindGroupEntry { binding: 2, resource: self.uniforms.as_entire_binding() },
                ],
            });
            self.texture = Some((texture, bind_group));
            self.uploaded = None;
        }
        let step = mask.cell_size() / FINE as f32;
        let uniforms = Uniforms {
            brush: brush.map_or([0.0; 4], |b| [b.pos.x, b.pos.y, b.radius, if b.paint { 1.0 } else { 2.0 }]),
            texel: [1.0 / fx as f32, 1.0 / fy as f32],
            range: RANGE * step,
            step,
        };
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&uniforms));
        let key = (scene, mask.generation());
        if self.uploaded == Some(key) {
            return;
        }
        let (field, any) = wall_field(nx, ny, occ, sim::BORDER_CELLS);
        self.any = any;
        self.values.clear();
        self.values.extend(field.iter().map(|d| ((d / RANGE).clamp(-1.0, 1.0) * 127.5 + 127.5).round() as u8));
        let (texture, _) = self.texture.as_ref().expect("created above");
        // Row 0 of the mask is y = 0 (the floor); the shader samples with v = y / height,
        // so no flip is needed.
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &self.values,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(fx as u32), rows_per_image: Some(fy as u32) },
            size,
        );
        self.uploaded = Some(key);
    }

    /// Expects the globals bind group at index 0 to be set already. Draws nothing when
    /// there are no walls and no brush to preview.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, brush: bool) {
        if let Some((_, bind_group)) = self.texture.as_ref().filter(|_| self.any || brush) {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(1, bind_group, &[]);
            pass.draw(0..4, 0..1);
        }
    }
}

/// The signed distance (in fine samples, negative inside walls) at `FINE` times the
/// mask's resolution, row 0 at the bottom, with the `border` cells left out; and
/// whether there are any walls at all.
fn wall_field(nx: usize, ny: usize, occ: &[u8], border: usize) -> (Vec<f32>, bool) {
    let open = |i: usize, j: usize| i < border || j < border || i >= nx - border || j >= ny - border;
    let mut full: Vec<f32> = (0..nx * ny).map(|k| if open(k % nx, k / nx) { 0.0 } else { occ[k] as f32 / 255.0 }).collect();
    let any = full.iter().any(|&v| v > 0.0);
    // Softened by about a cell and a half, so the half-full contour rounds the cells'
    // steps off (the thinnest stroke, five cells across, keeps its width).
    blur(&mut full, nx, ny);
    blur(&mut full, nx, ny);
    let (fx, fy) = (nx * FINE, ny * FINE);
    // Each fine sample's occupancy, bilinear between cell centres, and its slope (per
    // fine sample).
    let sample = |x: usize, y: usize| {
        let u = ((x as f32 + 0.5) / FINE as f32 - 0.5).clamp(0.0, (nx - 1) as f32);
        let v = ((y as f32 + 0.5) / FINE as f32 - 0.5).clamp(0.0, (ny - 1) as f32);
        let (i, j) = ((u as usize).min(nx - 2), (v as usize).min(ny - 2));
        let (a, b) = (u - i as f32, v - j as f32);
        let at = |i: usize, j: usize| full[j * nx + i];
        let (c00, c10, c01, c11) = (at(i, j), at(i + 1, j), at(i, j + 1), at(i + 1, j + 1));
        let value = (c00 * (1.0 - a) + c10 * a) * (1.0 - b) + (c01 * (1.0 - a) + c11 * a) * b;
        let slope = Vec2::new((c10 - c00) * (1.0 - b) + (c11 - c01) * b, (c01 - c00) * (1.0 - a) + (c11 - c10) * a) / FINE as f32;
        (value, slope)
    };
    let samples: Vec<(f32, Vec2)> = (0..fx * fy).into_par_iter().map(|k| sample(k % fx, k / fx)).collect();
    let far = ((fx + fy) * (fx + fy)) as f32;
    let mut to_inside: Vec<f32> = samples.iter().map(|(v, _)| if *v >= 0.5 { 0.0 } else { far }).collect();
    let mut to_outside: Vec<f32> = samples.iter().map(|(v, _)| if *v < 0.5 { 0.0 } else { far }).collect();
    distance_transform(&mut to_inside, fx, fy);
    distance_transform(&mut to_outside, fx, fy);
    let field = samples
        .par_iter()
        .zip(to_inside.par_iter().zip(&to_outside))
        .map(|(&(value, slope), (&d_in, &d_out))| {
            // Samples meet their nearest one across the edge half a sample short of it.
            let coarse = if value >= 0.5 { 0.5 - d_out.sqrt() } else { d_in.sqrt() - 0.5 };
            // Near the edge, the occupancy's slope places it to a fraction of a sample.
            let gradient = slope.length();
            let fine = (0.5 - value) / gradient.max(1e-6);
            if gradient > 1e-4 && fine.abs() < 1.5 { fine } else { coarse }
        })
        .collect();
    (field, any)
}

/// A small Gaussian (1 4 6 4 1, along each axis) in place, edges held.
fn blur(f: &mut [f32], w: usize, h: usize) {
    const KERNEL: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];
    let pass = |f: &mut [f32], along: usize, across: usize, at: &dyn Fn(usize, usize) -> usize| {
        let mut line = vec![0.0; along];
        for b in 0..across {
            for (a, out) in line.iter_mut().enumerate() {
                *out = KERNEL.iter().enumerate().map(|(k, weight)| weight * f[at((a + k).saturating_sub(2).min(along - 1), b)]).sum();
            }
            for (a, v) in line.iter().enumerate() {
                f[at(a, b)] = *v;
            }
        }
    };
    pass(f, w, h, &|x, y| y * w + x);
    pass(f, h, w, &|y, x| y * w + x);
}

/// Squared Euclidean distance transform in place (Felzenszwalb and Huttenlocher): each
/// value becomes the least of (other's value + squared distance to it) over the grid.
fn distance_transform(f: &mut [f32], w: usize, h: usize) {
    let mut columns = vec![0.0; w * h];
    for (k, &v) in f.iter().enumerate() {
        columns[(k % w) * h + k / w] = v;
    }
    columns.par_chunks_mut(h).for_each(transform_line);
    for (k, v) in f.iter_mut().enumerate() {
        *v = columns[(k % w) * h + k / w];
    }
    f.par_chunks_mut(w).for_each(transform_line);
}

/// The one-dimensional pass: the lower envelope of parabolas rooted at each sample.
fn transform_line(f: &mut [f32]) {
    let n = f.len();
    let mut out = vec![0.0; n];
    let mut roots = vec![0usize; n];
    let mut bounds = vec![0.0f32; n + 1];
    let mut k = 0;
    bounds[0] = f32::NEG_INFINITY;
    bounds[1] = f32::INFINITY;
    let cross = |f: &[f32], q: usize, r: usize| ((f[q] + (q * q) as f32) - (f[r] + (r * r) as f32)) / (2.0 * q as f32 - 2.0 * r as f32);
    for q in 1..n {
        let mut s = cross(f, q, roots[k]);
        while s <= bounds[k] {
            k -= 1;
            s = cross(f, q, roots[k]);
        }
        k += 1;
        roots[k] = q;
        bounds[k] = s;
        bounds[k + 1] = f32::INFINITY;
    }
    k = 0;
    for (q, o) in out.iter_mut().enumerate() {
        while bounds[k + 1] < q as f32 {
            k += 1;
        }
        let r = roots[k];
        *o = (q as f32 - r as f32).powi(2) + f[r];
    }
    f.copy_from_slice(&out);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A disc of walls in the middle of a box: the field is about the true distance to
    /// the circle everywhere (inside negative), and the border is left out.
    #[test]
    fn the_field_measures_distance_to_the_walls() {
        let cell = 1.0 / 64.0;
        let mut mask = SolidMask::new_box(Vec2::new(1.0, 1.0), cell);
        let (centre, radius) = (Vec2::splat(0.5), 0.2);
        mask.paint_disc(centre, radius, true);
        let (nx, ny, occ) = mask.occupancy();
        let (field, any) = wall_field(nx, ny, occ, sim::BORDER_CELLS);
        assert!(any);
        let fine = cell / FINE as f32;
        let mut worst: f32 = 0.0;
        for y in (0..ny * FINE).step_by(5) {
            for x in (0..nx * FINE).step_by(5) {
                let p = (Vec2::new(x as f32, y as f32) + 0.5) * fine;
                let truth = (p.distance(centre) - radius) / fine;
                if truth.abs() < RANGE {
                    worst = worst.max((field[y * nx * FINE + x] - truth).abs());
                }
            }
        }
        // The painted disc is made of whole cells: within one cell of the true circle.
        assert!(worst < FINE as f32, "off by {worst} fine samples");
        // Next to the domain's border, far from the disc: open, not a wall's edge.
        let near_border = (2 * FINE * nx * FINE) + 2 * FINE;
        assert!(field[near_border] > RANGE, "{}", field[near_border]);
    }

    /// The thinnest stroke the brush makes survives the smoothing.
    #[test]
    fn thin_strokes_stay() {
        let cell = 1.0 / 128.0;
        let mut mask = SolidMask::new_box(Vec2::new(1.0, 1.0), cell);
        for k in 0..40 {
            mask.paint_disc(Vec2::new(0.3 + 0.01 * k as f32, 0.5), 0.02, true);
        }
        let (nx, ny, occ) = mask.occupancy();
        let (field, _) = wall_field(nx, ny, occ, sim::BORDER_CELLS);
        let at = |p: Vec2| field[(p.y / cell * FINE as f32) as usize * nx * FINE + (p.x / cell * FINE as f32) as usize];
        let middle = at(Vec2::new(0.5, 0.5));
        // Half the stroke's width inside, give or take a cell.
        assert!(middle < -(0.02 / cell - 1.0) * FINE as f32, "{middle}");
    }

    #[test]
    fn an_empty_box_has_no_walls() {
        let mask = SolidMask::new_box(Vec2::new(1.0, 0.5), 1.0 / 64.0);
        let (nx, ny, occ) = mask.occupancy();
        assert!(!wall_field(nx, ny, occ, sim::BORDER_CELLS).1);
    }

    #[test]
    fn the_transform_finds_squared_distances() {
        let far = 1e6;
        let mut line = vec![far, far, 0.0, far, far, far, 0.0];
        transform_line(&mut line);
        assert_eq!(line, vec![4.0, 1.0, 0.0, 1.0, 4.0, 1.0, 0.0]);
        let (w, h) = (5, 4);
        let mut grid = vec![far; w * h];
        grid[w + 1] = 0.0;
        distance_transform(&mut grid, w, h);
        for y in 0..h {
            for x in 0..w {
                let expected = (x as f32 - 1.0).powi(2) + (y as f32 - 1.0).powi(2);
                assert_eq!(grid[y * w + x], expected, "at {x},{y}");
            }
        }
    }
}
