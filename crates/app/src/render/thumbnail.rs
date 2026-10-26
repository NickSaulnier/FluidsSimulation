//! Look thumbnails, taken from the picture itself: once a look has been on screen a
//! while, the composite is drawn a second time into a small texture (`record`), copied
//! into a buffer and read back a few frames later without waiting on the GPU (`poll`).
//! The part of the window the scene shows in (the canvas) is cropped to 16:9 and scaled
//! down to `SIZE`.
//!
//! A fluid can't be simulated ahead in the background, so a look gets its thumbnail the
//! first time it's watched; the browser shows a stand-in until then.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

/// Thumbnails are this many pixels.
pub const SIZE: [usize; 2] = [192, 108];
/// The capture's width (its height follows the window's shape).
const CAPTURE_WIDTH: u32 = 512;

const WAITING: u8 = 0;
const MAPPED: u8 = 1;
const FAILED: u8 = 2;

/// A capture in flight.
struct Pending {
    key: String,
    buffer: wgpu::Buffer,
    size: [u32; 2],
    row: u32,
    /// The canvas, as fractions of the window: x, y, width, height.
    crop: [f32; 4],
    /// Blue first (the swapchain's byte order).
    bgra: bool,
    state: Arc<AtomicU8>,
    mapping: bool,
}

#[derive(Default)]
pub struct Thumbnailer {
    pending: Option<Pending>,
    /// Kept between captures (remade if the window's shape changes).
    texture: Option<(wgpu::Texture, wgpu::TextureView, [u32; 2])>,
}

impl Thumbnailer {
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }

    /// The texture to draw this frame's composite into (then `copy` it). `surface`: the
    /// window's size; `format`: the composite's output format.
    pub fn target(&mut self, device: &wgpu::Device, surface: [u32; 2], format: wgpu::TextureFormat) -> wgpu::TextureView {
        let height = ((CAPTURE_WIDTH as f32 * surface[1] as f32 / surface[0].max(1) as f32).round() as u32).max(1);
        let size = [CAPTURE_WIDTH, height];
        if self.texture.as_ref().is_none_or(|t| t.2 != size || t.0.format() != format) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("thumbnail"),
                size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            self.texture = Some((texture, view, size));
        }
        self.texture.as_ref().expect("just made").1.clone()
    }

    /// Copy what was drawn into `target` toward the CPU, as `key`'s thumbnail.
    pub fn copy(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, key: String, crop: [f32; 4]) {
        let Some((texture, _, size)) = &self.texture else { return };
        let row = (size[0] * 4).next_multiple_of(256);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("thumbnail readback"),
            size: u64::from(row * size[1]),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: None } },
            wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
        );
        let bgra = matches!(texture.format(), wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb);
        self.pending = Some(Pending { key, buffer, size: *size, row, crop, bgra, state: Arc::new(AtomicU8::new(WAITING)), mapping: false });
    }

    /// After the frame with the copy was submitted: ask for the buffer, and hand over the
    /// thumbnail once it's there (`SIZE`, RGBA).
    pub fn poll(&mut self, device: &wgpu::Device) -> Option<(String, Vec<u8>)> {
        let pending = self.pending.as_mut()?;
        if !pending.mapping {
            pending.mapping = true;
            let state = pending.state.clone();
            pending.buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| state.store(if r.is_ok() { MAPPED } else { FAILED }, Ordering::Release));
            return None;
        }
        let _ = device.poll(wgpu::PollType::Poll);
        match pending.state.load(Ordering::Acquire) {
            WAITING => None,
            FAILED => {
                self.pending = None;
                None
            }
            _ => {
                let pending = self.pending.take().expect("pending");
                let pixels = {
                    let data = pending.buffer.slice(..).get_mapped_range().ok()?;
                    shrink(&data, pending.size, pending.row, pending.crop, pending.bgra)
                };
                pending.buffer.unmap();
                Some((pending.key, pixels))
            }
        }
    }
}

/// The `crop` of an image (`size`, rows `row` bytes apart, 4 bytes a pixel), cut to 16:9
/// about its middle and averaged down to `SIZE`, as opaque RGBA.
fn shrink(data: &[u8], size: [u32; 2], row: u32, crop: [f32; 4], bgra: bool) -> Vec<u8> {
    let [w, h] = size.map(|v| v as f32);
    let (mut x0, mut y0, mut cw, mut ch) = (crop[0] * w, crop[1] * h, (crop[2] * w).max(1.0), (crop[3] * h).max(1.0));
    let aspect = SIZE[0] as f32 / SIZE[1] as f32;
    if cw / ch > aspect {
        x0 += 0.5 * (cw - ch * aspect);
        cw = ch * aspect;
    } else {
        y0 += 0.5 * (ch - cw / aspect);
        ch = cw / aspect;
    }
    let mut out = Vec::with_capacity(SIZE[0] * SIZE[1] * 4);
    for ty in 0..SIZE[1] {
        let (ya, yb) = (y0 + ch * ty as f32 / SIZE[1] as f32, y0 + ch * (ty + 1) as f32 / SIZE[1] as f32);
        for tx in 0..SIZE[0] {
            let (xa, xb) = (x0 + cw * tx as f32 / SIZE[0] as f32, x0 + cw * (tx + 1) as f32 / SIZE[0] as f32);
            let mut sum = [0u32; 3];
            let mut n = 0;
            for y in (ya as u32)..(yb.ceil() as u32).max(ya as u32 + 1).min(size[1]) {
                for x in (xa as u32)..(xb.ceil() as u32).max(xa as u32 + 1).min(size[0]) {
                    let i = (y * row + x * 4) as usize;
                    let p = &data[i..i + 3];
                    for c in 0..3 {
                        sum[c] += u32::from(p[c]);
                    }
                    n += 1;
                }
            }
            let n = n.max(1);
            let [a, g, b] = sum.map(|v| (v / n) as u8);
            let (r, b) = if bgra { (b, a) } else { (a, b) };
            out.extend_from_slice(&[r, g, b, 255]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The crop is cut to 16:9 about its middle; blue-first bytes come out red-first.
    #[test]
    fn shrinking_crops_and_swaps() {
        let size = [400u32, 400];
        let row = size[0] * 4;
        // Left half blue (BGRA bytes 255,0,0), right half red.
        let mut data = vec![0u8; (row * size[1]) as usize];
        for y in 0..size[1] {
            for x in 0..size[0] {
                let i = (y * row + x * 4) as usize;
                data[i..i + 4].copy_from_slice(if x < 200 { &[255, 0, 0, 255] } else { &[0, 0, 255, 255] });
            }
        }
        let out = shrink(&data, size, row, [0.0, 0.0, 1.0, 1.0], true);
        assert_eq!(out.len(), SIZE[0] * SIZE[1] * 4);
        assert_eq!(&out[..4], &[0, 0, 255, 255], "the left edge is blue");
        let last = (SIZE[0] - 1) * 4;
        assert_eq!(&out[last..last + 4], &[255, 0, 0, 255], "the right edge is red");
    }
}
