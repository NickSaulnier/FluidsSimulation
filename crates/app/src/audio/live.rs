//! Live input: capture what the computer is playing (Spotify, a browser, anything).
//!
//! On Windows, opening the default *output* device as an input stream makes cpal's
//! WASAPI backend capture it in loopback mode: the mix as it goes to the speakers. The
//! capture callback runs on cpal's audio thread; it downmixes to mono and appends to a
//! shared buffer, which the frame loop drains into the analyzer.

use rodio::cpal;
use rodio::cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{Arc, Mutex};

/// Captured audio waiting for the analyzer is capped at this many seconds (the frame
/// loop drains it every frame, so it only fills while the window is stalled).
const MAX_BACKLOG_S: f32 = 1.0;

pub struct LiveInput {
    /// Capturing stops when this is dropped.
    _stream: cpal::Stream,
    pub device_name: String,
    pub sample_rate: u32,
    buffer: Arc<Mutex<Vec<f32>>>,
}

impl LiveInput {
    /// Start capturing the default output device.
    pub fn open() -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or("no audio output device")?;
        let device_name = device.description().map(|d| d.name().to_string()).unwrap_or_else(|_| "speakers".into());
        let supported = device.default_output_config().map_err(|e| format!("{device_name}: {e}"))?;
        let channels = usize::from(supported.channels());
        let rate = supported.sample_rate();
        let config: cpal::StreamConfig = supported.config();
        let max_backlog = (MAX_BACKLOG_S * rate as f32) as usize;
        let buffer = Arc::new(Mutex::new(Vec::new()));

        let sink = buffer.clone();
        let push = move |mono: &mut dyn Iterator<Item = f32>| {
            if let Ok(mut b) = sink.lock() {
                b.extend(mono);
                let excess = b.len().saturating_sub(max_backlog);
                b.drain(..excess);
            }
        };
        let on_error = |e| log::warn!("live audio capture: {e}");
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    push(&mut data.chunks(channels).map(|f| f.iter().sum::<f32>() / channels as f32));
                },
                on_error,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_input_stream(
                &config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    let to_f32 = |s: &i16| f32::from(*s) / f32::from(i16::MAX);
                    push(&mut data.chunks(channels).map(|f| f.iter().map(to_f32).sum::<f32>() / channels as f32));
                },
                on_error,
                None,
            ),
            other => return Err(format!("{device_name}: unsupported sample format {other}")),
        }
        .map_err(|e| format!("{device_name}: {e}"))?;
        stream.play().map_err(|e| format!("{device_name}: {e}"))?;
        log::info!("capturing {device_name} ({rate} Hz, {channels} channels)");
        Ok(Self { _stream: stream, device_name, sample_rate: rate, buffer })
    }

    /// Move the captured mono samples into `out`. (WASAPI delivers nothing while the
    /// system is silent.)
    pub fn drain(&mut self, out: &mut Vec<f32>) {
        if let Ok(mut b) = self.buffer.lock() {
            out.append(&mut b);
        }
    }
}
