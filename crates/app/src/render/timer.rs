//! GPU time per stage of a frame (the scene, the glow, the composite, the interface),
//! from timestamp queries. A marker (an empty compute pass that writes a timestamp) goes
//! between stages, so the renderers themselves need no timing code. The results are
//! read back a frame or two late, without stalling; without `TIMESTAMP_QUERY` there are
//! none.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The frame's stages, in order; a marker goes at the start of each, and one at the end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Scene,
    Glow,
    Composite,
    Interface,
    End,
}

const STAGES: [(Stage, &str); 4] = [(Stage::Scene, "scene"), (Stage::Glow, "glow"), (Stage::Composite, "composite"), (Stage::Interface, "interface")];
const MARKERS: u32 = STAGES.len() as u32 + 1;

enum State {
    Idle,
    /// The resolved timestamps are being copied, in the frame just recorded.
    Copied,
    /// Waiting for the copy to map; the flag is set when it has.
    Mapping(Arc<AtomicBool>),
}

pub struct GpuTimer {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    state: State,
    period_ns: f32,
    latest: Vec<(&'static str, Duration)>,
}

impl GpuTimer {
    /// A timer, if the device can write timestamps.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Self> {
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return None;
        }
        let bytes = u64::from(MARKERS) * 8;
        let buffer = |label: &str, usage| device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: bytes, usage, mapped_at_creation: false });
        Some(Self {
            queries: device.create_query_set(&wgpu::QuerySetDescriptor { label: Some("frame timings"), ty: wgpu::QueryType::Timestamp, count: MARKERS }),
            resolve: buffer("frame timings", wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC),
            readback: buffer("frame timings readback", wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST),
            state: State::Idle,
            period_ns: queue.get_timestamp_period(),
            latest: Vec::new(),
        })
    }

    /// Mark the start of `stage` (or, with `Stage::End`, the end of the last one).
    pub fn mark(&self, encoder: &mut wgpu::CommandEncoder, stage: Stage) {
        let index = STAGES.iter().position(|(s, _)| *s == stage).unwrap_or(STAGES.len()) as u32;
        encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("timing marker"),
            timestamp_writes: Some(wgpu::ComputePassTimestampWrites {
                query_set: &self.queries,
                beginning_of_pass_write_index: Some(index),
                end_of_pass_write_index: None,
            }),
        });
    }

    /// After the frame's last marker: take an earlier frame's timings if they've
    /// arrived, and queue this frame's if the readback buffer is free.
    pub fn finish(&mut self, encoder: &mut wgpu::CommandEncoder) {
        if let State::Mapping(ready) = &self.state
            && ready.load(Ordering::Acquire)
        {
            {
                let data = self.readback.slice(..).get_mapped_range().expect("mapped timings");
                let ticks: &[u64] = bytemuck::cast_slice(&data);
                self.latest = STAGES
                    .iter()
                    .enumerate()
                    .map(|(k, (_, name))| {
                        let ns = ticks[k + 1].saturating_sub(ticks[k]) as f64 * f64::from(self.period_ns);
                        (*name, Duration::from_nanos(ns as u64))
                    })
                    .collect();
            }
            self.readback.unmap();
            self.state = State::Idle;
        }
        if matches!(self.state, State::Idle) {
            encoder.resolve_query_set(&self.queries, 0..MARKERS, &self.resolve, 0);
            encoder.copy_buffer_to_buffer(&self.resolve, 0, &self.readback, 0, u64::from(MARKERS) * 8);
            self.state = State::Copied;
        }
    }

    /// The frame's commands were submitted: start reading back what they copied.
    pub fn submitted(&mut self, device: &wgpu::Device) {
        if matches!(self.state, State::Copied) {
            let ready = Arc::new(AtomicBool::new(false));
            let flag = ready.clone();
            self.readback.slice(..).map_async(wgpu::MapMode::Read, move |r| flag.store(r.is_ok(), Ordering::Release));
            self.state = State::Mapping(ready);
        }
        let _ = device.poll(wgpu::PollType::Poll);
    }

    /// The latest frame timings that arrived: (stage, GPU time).
    pub fn latest(&self) -> &[(&'static str, Duration)] {
        &self.latest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On a headless GPU, timings arrive within a few frames, one per stage.
    #[test]
    fn timings_arrive_for_every_stage() {
        let gpu = match sim3d::GpuContext::new() {
            Ok(g) => g,
            Err(e) => return eprintln!("skipping: {e:#}"),
        };
        let Some(mut timer) = GpuTimer::new(&gpu.device, &gpu.queue) else {
            return eprintln!("skipping: no timestamp queries");
        };
        for _ in 0..10 {
            let mut encoder = gpu.device.create_command_encoder(&Default::default());
            for (stage, _) in STAGES {
                timer.mark(&mut encoder, stage);
            }
            timer.mark(&mut encoder, Stage::End);
            timer.finish(&mut encoder);
            gpu.queue.submit([encoder.finish()]);
            timer.submitted(&gpu.device);
            gpu.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        }
        let names: Vec<_> = timer.latest().iter().map(|(n, _)| *n).collect();
        assert_eq!(names, ["scene", "glow", "composite", "interface"]);
    }
}
