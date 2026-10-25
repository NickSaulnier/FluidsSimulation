//! Live input: capture what the computer is playing (Spotify, a browser, anything).
//!
//! On Windows, opening the default *output* device as an input stream makes cpal's
//! WASAPI backend capture it in loopback mode: the mix as it goes to the speakers.
//!
//! The capture callback runs on cpal's audio thread, which must never wait: it converts
//! whatever sample format the device uses to `f32`, mixes the channels down to stereo,
//! and pushes the frames into a lock-free ring (`rtrb`) without locking or allocating.
//! The frame loop drains the ring into the analyzer every frame.

use rodio::cpal;
use rodio::cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rodio::cpal::{FromSample, SampleFormat, SizedSample};

pub use super::analyzer::Frame;

/// The ring holds this many seconds. The frame loop drains it every frame, so it only
/// fills while the window is stalled; once full, new frames are dropped (the audio
/// thread can't remove old ones) until the frame loop catches up.
const RING_S: f32 = 2.0;
/// After a stall only the newest this-many seconds are kept, so the analysis doesn't
/// lag behind what's playing.
const MAX_BACKLOG_S: f32 = 1.0;
/// Centre and surround channels mix into both sides at -3 dB, the LFE at -6 dB (it's
/// all bass, which the kick detector wants).
const SIDE_GAIN: f32 = std::f32::consts::FRAC_1_SQRT_2;
const LFE_GAIN: f32 = 0.5;

pub struct LiveInput {
    /// Capturing stops when this is dropped.
    _stream: cpal::Stream,
    pub device_name: String,
    pub sample_rate: u32,
    frames: rtrb::Consumer<Frame>,
    max_backlog: usize,
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
        let (producer, consumer) = rtrb::RingBuffer::new((RING_S * rate as f32) as usize);

        let stream = match supported.sample_format() {
            SampleFormat::I8 => build::<i8>(&device, &config, channels, producer),
            SampleFormat::I16 => build::<i16>(&device, &config, channels, producer),
            SampleFormat::I24 => build::<cpal::I24>(&device, &config, channels, producer),
            SampleFormat::I32 => build::<i32>(&device, &config, channels, producer),
            SampleFormat::I64 => build::<i64>(&device, &config, channels, producer),
            SampleFormat::U8 => build::<u8>(&device, &config, channels, producer),
            SampleFormat::U16 => build::<u16>(&device, &config, channels, producer),
            SampleFormat::U24 => build::<cpal::U24>(&device, &config, channels, producer),
            SampleFormat::U32 => build::<u32>(&device, &config, channels, producer),
            SampleFormat::U64 => build::<u64>(&device, &config, channels, producer),
            SampleFormat::F32 => build::<f32>(&device, &config, channels, producer),
            SampleFormat::F64 => build::<f64>(&device, &config, channels, producer),
            // DSD bitstreams aren't samples: there's nothing to analyse frame by frame.
            other => return Err(format!("{device_name}: unsupported sample format {other}")),
        }
        .map_err(|e| format!("{device_name}: {e}"))?;
        stream.play().map_err(|e| format!("{device_name}: {e}"))?;
        log::info!("capturing {device_name} ({rate} Hz, {channels} channels, {})", supported.sample_format());
        Ok(Self { _stream: stream, device_name, sample_rate: rate, frames: consumer, max_backlog: (MAX_BACKLOG_S * rate as f32) as usize })
    }

    /// Move the captured frames into `out`, at most the newest second of them. (WASAPI
    /// delivers nothing while the system is silent.)
    pub fn drain(&mut self, out: &mut Vec<Frame>) {
        take_newest(&mut self.frames, self.max_backlog, out);
    }
}

/// An input stream for samples of type `T`, pushing stereo frames into `frames`.
fn build<T>(device: &cpal::Device, config: &cpal::StreamConfig, channels: usize, mut frames: rtrb::Producer<Frame>) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| push_frames(data, channels, &mut frames),
        |e| log::warn!("live audio capture: {e}"),
        None,
    )
}

/// Convert interleaved `channels`-channel samples to stereo frames and push them,
/// dropping what doesn't fit rather than waiting (this runs on the audio thread).
fn push_frames<T>(data: &[T], channels: usize, frames: &mut rtrb::Producer<Frame>)
where
    T: SizedSample,
    f32: FromSample<T>,
{
    for frame in data.chunks_exact(channels.max(1)) {
        if frames.push(to_stereo(frame.iter().map(|s| s.to_sample::<f32>()))).is_err() {
            return; // full: the frame loop has stalled
        }
    }
}

/// Mix one frame down to stereo. Channels come in the usual WAVE order: front left,
/// front right, centre, LFE, then surround pairs (left, right). A single channel goes
/// to both sides.
pub fn to_stereo(samples: impl ExactSizeIterator<Item = f32>) -> Frame {
    if samples.len() == 1 {
        let mut samples = samples;
        let s = samples.next().unwrap_or(0.0);
        return [s, s];
    }
    let (mut left, mut right) = (0.0, 0.0);
    for (k, s) in samples.enumerate() {
        match k {
            0 => left += s,
            1 => right += s,
            2 => {
                left += SIDE_GAIN * s;
                right += SIDE_GAIN * s;
            }
            3 => {
                left += LFE_GAIN * s;
                right += LFE_GAIN * s;
            }
            k if k % 2 == 0 => left += SIDE_GAIN * s,
            _ => right += SIDE_GAIN * s,
        }
    }
    [left, right]
}

/// Move what's in `frames` into `out`, keeping only the newest `max` frames.
fn take_newest(frames: &mut rtrb::Consumer<Frame>, max: usize, out: &mut Vec<Frame>) {
    let available = frames.slots();
    if let Ok(chunk) = frames.read_chunk(available) {
        let (a, b) = chunk.as_slices();
        out.extend(a.iter().chain(b).skip(available.saturating_sub(max)).copied());
        chunk.commit_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture<T>(data: &[T], channels: usize) -> Vec<Frame>
    where
        T: SizedSample,
        f32: FromSample<T>,
    {
        let (mut producer, mut consumer) = rtrb::RingBuffer::new(64);
        push_frames(data, channels, &mut producer);
        let mut out = Vec::new();
        take_newest(&mut consumer, usize::MAX, &mut out);
        out
    }

    fn close(a: Frame, b: Frame) -> bool {
        (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3
    }

    /// Integer, unsigned and float formats all land on -1..1, unsigned centred on its
    /// midpoint.
    #[test]
    fn every_sample_format_converts() {
        assert!(close(capture(&[i16::MAX, i16::MIN], 2)[0], [1.0, -1.0]));
        // 8-bit unsigned: 128 is silence and 255 the top step, 127/128.
        assert!(close(capture(&[128u8, 255u8], 2)[0], [0.0, 127.0 / 128.0]));
        assert!(close(capture(&[u16::MAX, 0u16], 2)[0], [1.0, -1.0]));
        assert!(close(capture(&[i32::MAX / 2, 0], 2)[0], [0.5, 0.0]));
        assert!(close(capture(&[0.25f64, -0.5], 2)[0], [0.25, -0.5]));
        let i24 = cpal::I24::new(1 << 22).unwrap();
        assert!(close(capture(&[i24, i24], 2)[0], [0.5, 0.5]));
    }

    #[test]
    fn channels_mix_down_to_stereo() {
        assert_eq!(capture(&[0.3f32, -0.2], 1), vec![[0.3, 0.3], [-0.2, -0.2]], "mono goes to both sides");
        assert_eq!(capture(&[0.1f32, 0.2, 0.3, 0.4], 2), vec![[0.1, 0.2], [0.3, 0.4]]);
        // 5.1: front L, front R, centre, LFE, surround L, surround R.
        let six = capture(&[1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0], 6);
        assert!(close(six[0], [1.0, 0.0]), "front left: {:?}", six[0]);
        assert!(close(six[1], [SIDE_GAIN, SIDE_GAIN]), "centre: {:?}", six[1]);
        assert!(close(six[2], [LFE_GAIN, LFE_GAIN + SIDE_GAIN]), "LFE and surround right: {:?}", six[2]);
    }

    /// A full ring drops what doesn't fit instead of waiting, and draining after a stall
    /// keeps only the newest frames.
    #[test]
    fn stalls_drop_audio_instead_of_blocking() {
        let (mut producer, mut consumer) = rtrb::RingBuffer::new(4);
        let data: Vec<f32> = (0..12).map(|k| k as f32).collect();
        push_frames(&data, 2, &mut producer); // six frames into four slots
        let mut out = Vec::new();
        take_newest(&mut consumer, 2, &mut out);
        assert_eq!(out, vec![[4.0, 5.0], [6.0, 7.0]], "the newest two of the four that fitted");
        push_frames(&[8.0f32, 9.0], 2, &mut producer);
        out.clear();
        take_newest(&mut consumer, 2, &mut out);
        assert_eq!(out, vec![[8.0, 9.0]], "drained: room again");
    }
}
