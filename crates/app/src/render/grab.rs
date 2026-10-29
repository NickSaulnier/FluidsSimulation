//! Frames read back from the GPU for screenshots and recordings, without waiting: the
//! composite is drawn a second time into a slot's texture (`begin`, then the drawing,
//! then `end`), copied to a buffer, and handed over a few frames later (`poll`). Several
//! slots can be in flight, so a recording can grab every frame; when all are busy a grab
//! is refused (the frame dropped) rather than stalling the app.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

/// The most grabs in flight at once.
const SLOTS: usize = 4;

const IDLE: u8 = 0;
const WAITING: u8 = 1;
const MAPPED: u8 = 2;
const FAILED: u8 = 3;

/// A grabbed frame: RGBA, 8 bits a channel, `size` pixels, rows packed.
pub struct Image {
    pub size: [u32; 2],
    pub rgba: Vec<u8>,
}

struct Slot<T> {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    buffer: wgpu::Buffer,
    size: [u32; 2],
    row: u32,
    state: Arc<AtomicU8>,
    /// What it holds (none: free), and whether its copy has been recorded and its
    /// mapping asked for.
    tag: Option<T>,
    copied: bool,
    mapping: bool,
}

pub struct Grabber<T> {
    slots: Vec<Slot<T>>,
    /// The slot `begin` handed out, for `end`.
    open: Option<usize>,
}

impl<T> Default for Grabber<T> {
    fn default() -> Self {
        Self { slots: Vec::new(), open: None }
    }
}

impl<T> Grabber<T> {
    /// A texture to draw the frame `tag` into, `size` pixels in `format` (then `end`);
    /// none if every slot is busy.
    pub fn begin(&mut self, device: &wgpu::Device, size: [u32; 2], format: wgpu::TextureFormat, tag: T) -> Option<wgpu::TextureView> {
        let free = self.slots.iter().position(|s| s.tag.is_none() && s.size == size && s.texture.format() == format);
        let index = match free {
            Some(i) => i,
            None if self.slots.len() < SLOTS => {
                self.slots.push(slot(device, size, format));
                self.slots.len() - 1
            }
            None => {
                // Remake a free slot of another size, if there is one.
                let i = self.slots.iter().position(|s| s.tag.is_none())?;
                self.slots[i] = slot(device, size, format);
                i
            }
        };
        let s = &mut self.slots[index];
        (s.tag, s.copied, s.mapping) = (Some(tag), false, false);
        s.state.store(IDLE, Ordering::Release);
        self.open = Some(index);
        Some(s.view.clone())
    }

    /// Copy what was drawn into the texture `begin` gave toward the CPU.
    pub fn end(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some(i) = self.open.take() else { return };
        let s = &mut self.slots[i];
        encoder.copy_texture_to_buffer(
            s.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo { buffer: &s.buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(s.row), rows_per_image: None } },
            wgpu::Extent3d { width: s.size[0], height: s.size[1], depth_or_array_layers: 1 },
        );
        s.copied = true;
    }

    /// After the frame with the copies was submitted: ask for the buffers, and hand
    /// over the frames that have arrived, oldest grab first.
    pub fn poll(&mut self, device: &wgpu::Device) -> Vec<(T, Image)> {
        for s in self.slots.iter_mut().filter(|s| s.copied && !s.mapping) {
            s.mapping = true;
            s.state.store(WAITING, Ordering::Release);
            let state = s.state.clone();
            s.buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| state.store(if r.is_ok() { MAPPED } else { FAILED }, Ordering::Release));
        }
        if !self.slots.iter().any(|s| s.mapping) {
            return Vec::new();
        }
        let _ = device.poll(wgpu::PollType::Poll);
        let mut out = Vec::new();
        for s in self.slots.iter_mut().filter(|s| s.mapping) {
            match s.state.load(Ordering::Acquire) {
                MAPPED => {
                    let rgba = s.buffer.slice(..).get_mapped_range().ok().map(|data| pack(&data, s.size, s.row, s.texture.format()));
                    s.buffer.unmap();
                    let tag = s.tag.take();
                    (s.copied, s.mapping) = (false, false);
                    if let (Some(tag), Some(rgba)) = (tag, rgba) {
                        out.push((tag, Image { size: s.size, rgba }));
                    }
                }
                FAILED => {
                    s.tag = None;
                    (s.copied, s.mapping) = (false, false);
                }
                _ => {}
            }
        }
        out
    }

    /// Whether every slot is busy (a grab now would be refused).
    #[cfg(test)]
    pub fn full(&self) -> bool {
        self.slots.len() >= SLOTS && self.slots.iter().all(|s| s.tag.is_some())
    }
}

fn slot<T>(device: &wgpu::Device, size: [u32; 2], format: wgpu::TextureFormat) -> Slot<T> {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("grab"),
        size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let row = (size[0] * 4).next_multiple_of(256);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("grab readback"),
        size: u64::from(row * size[1]),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    Slot { view: texture.create_view(&Default::default()), texture, buffer, size, row, state: Arc::new(AtomicU8::new(IDLE)), tag: None, copied: false, mapping: false }
}

/// Rows `row` bytes apart, packed tight and made RGBA with full alpha (a blue-first
/// format swapped).
fn pack(data: &[u8], size: [u32; 2], row: u32, format: wgpu::TextureFormat) -> Vec<u8> {
    let bgra = matches!(format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb);
    let width = (size[0] * 4) as usize;
    let mut out = Vec::with_capacity(width * size[1] as usize);
    for y in 0..size[1] as usize {
        let line = &data[y * row as usize..y * row as usize + width];
        out.extend(line.as_chunks::<4>().0.iter().flat_map(|p| if bgra { [p[2], p[1], p[0], 255] } else { [p[0], p[1], p[2], 255] }));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Grabs arrive in order, as drawn, and four can be in flight.
    #[test]
    fn grabs_come_back_as_drawn() {
        let gpu = match sim3d::GpuContext::new() {
            Ok(g) => g,
            Err(e) => return eprintln!("skipping: {e:#}"),
        };
        let (device, queue) = (&gpu.device, &gpu.queue);
        let mut grabber = Grabber::default();
        let size = [70u32, 30];
        for k in 0..SLOTS {
            let view = grabber.begin(device, size, wgpu::TextureFormat::Bgra8Unorm, k).expect("a free slot");
            let mut encoder = device.create_command_encoder(&Default::default());
            let shade = f64::from(k as u32 + 1) / 8.0;
            drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: shade, g: 0.0, b: 1.0, a: 1.0 }), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            }));
            grabber.end(&mut encoder);
            queue.submit([encoder.finish()]);
        }
        assert!(grabber.full() && grabber.begin(device, size, wgpu::TextureFormat::Bgra8Unorm, 99).is_none(), "a fifth grab was taken");
        let mut got = Vec::new();
        for _ in 0..100 {
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
            got.extend(grabber.poll(device));
            if got.len() == SLOTS {
                break;
            }
        }
        assert_eq!(got.iter().map(|(k, _)| *k).collect::<Vec<_>>(), (0..SLOTS).collect::<Vec<_>>());
        for (k, image) in &got {
            assert_eq!((image.size, image.rgba.len()), (size, (70 * 30 * 4) as usize));
            let expected = ((*k as f32 + 1.0) / 8.0 * 255.0).round() as u8;
            assert!(image.rgba[0].abs_diff(expected) <= 1 && image.rgba[2] == 255 && image.rgba[3] == 255, "frame {k}: {:?}", &image.rgba[..4]);
        }
    }
}
