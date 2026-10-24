//! Audio-reactive mode: play a track and fire fluid pulses on its kick drums.
//!
//! Loading (decode + kick analysis) runs on a background thread so a long track doesn't
//! freeze the window. Kicks are detected offline (see `kicks`), so during playback the
//! only work is comparing their times against the player's position.

pub mod kicks;

use kicks::{Kick, Onsets};
use rodio::Source;
use sim::{Emitter, ForceBrush, Vec2};
use std::num::NonZero;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

/// How long one kick keeps pushing, and where successive kicks land across the floor.
const PULSE: Duration = Duration::from_millis(90);
const LANES: [f32; 3] = [0.3, 0.7, 0.5];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KickAction {
    /// Shove the water upward (conserves the fluid).
    Splash,
    /// Fire an emitter jet from the floor (adds fluid; pair with drains).
    Jet,
}

pub struct Track {
    pub name: String,
    pub duration: f32,
    pub kicks: Vec<Kick>,
    onsets: Onsets,
}

/// Decoded audio and its analysis, produced by the loader thread.
struct Loaded {
    name: String,
    channels: NonZero<u16>,
    rate: NonZero<u32>,
    samples: Vec<f32>,
    onsets: Onsets,
}

struct Pulse {
    started: Instant,
    strength: f32,
    lane: f32,
}

pub struct Music {
    pub track: Option<Track>,
    pub error: Option<String>,
    pub sensitivity: f32,
    pub gain: f32,
    pub action: KickAction,
    pub muted: bool,
    pub last_kick: Option<Instant>,
    /// The output device, opened on first load (playback stops if it's dropped).
    device: Option<rodio::MixerDeviceSink>,
    player: Option<rodio::Player>,
    /// Interleaved samples, kept so the track can be restarted after it ends.
    samples: Option<(NonZero<u16>, NonZero<u32>, Vec<f32>)>,
    loading: Option<(String, Receiver<Result<Loaded, String>>)>,
    picked_with: f32,
    next_kick: usize,
    last_pos: f32,
    pulses: Vec<Pulse>,
    lane: usize,
}

impl Default for Music {
    fn default() -> Self {
        Self {
            track: None,
            error: None,
            sensitivity: 1.5,
            gain: 1.0,
            action: KickAction::Splash,
            muted: false,
            last_kick: None,
            device: None,
            player: None,
            samples: None,
            loading: None,
            picked_with: f32::NAN,
            next_kick: 0,
            last_pos: 0.0,
            pulses: Vec::new(),
            lane: 0,
        }
    }
}

impl Music {
    /// Start loading a track in the background.
    pub fn load(&mut self, path: PathBuf) {
        let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        let (tx, rx) = mpsc::channel();
        let thread_name = name.clone();
        std::thread::spawn(move || {
            // The receiver may be gone if another file was dropped meanwhile; that's fine.
            let _ = tx.send(decode(&path, thread_name));
        });
        self.loading = Some((name, rx));
        self.error = None;
    }

    pub fn loading_name(&self) -> Option<&str> {
        self.loading.as_ref().map(|(n, _)| n.as_str())
    }

    pub fn is_playing(&self) -> bool {
        self.player.as_ref().is_some_and(|p| !p.is_paused() && !p.empty())
    }

    pub fn position(&self) -> f32 {
        self.player.as_ref().map_or(0.0, |p| p.get_pos().as_secs_f32())
    }

    pub fn toggle_play(&mut self) {
        match &self.player {
            Some(p) if p.empty() => self.restart(),
            Some(p) if p.is_paused() => p.play(),
            Some(p) => p.pause(),
            None => {}
        }
    }

    pub fn restart(&mut self) {
        if let (Some(player), Some((channels, rate, samples))) = (&self.player, &self.samples) {
            player.clear();
            player.append(rodio::buffer::SamplesBuffer::new(*channels, *rate, samples.clone()));
            player.play();
            self.next_kick = 0;
            self.last_pos = 0.0;
        }
    }

    pub fn apply_volume(&self) {
        if let Some(p) = &self.player {
            p.set_volume(if self.muted { 0.0 } else { 1.0 });
        }
    }

    /// Poll the loader, keep the kick list in sync with the sensitivity, and fire any
    /// kicks the playback position has passed since last frame.
    pub fn update(&mut self) {
        if let Some((_, rx)) = &self.loading {
            match rx.try_recv() {
                Ok(Ok(loaded)) => {
                    self.loading = None;
                    self.start(loaded);
                }
                Ok(Err(e)) => {
                    self.loading = None;
                    self.error = Some(e);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.loading = None;
                    self.error = Some("loader thread stopped".into());
                }
            }
        }
        // Read the player before borrowing the track: `position()` borrows all of `self`.
        let playing = self.player.as_ref().is_some_and(|p| !p.is_paused());
        let pos = self.position();
        let Some(track) = &mut self.track else { return };
        if self.sensitivity != self.picked_with {
            track.kicks = kicks::pick_kicks(&track.onsets, self.sensitivity);
            self.picked_with = self.sensitivity;
            self.next_kick = track.kicks.partition_point(|k| k.time <= self.last_pos);
        }
        if !playing {
            return;
        }
        if pos + 0.05 < self.last_pos {
            // Jumped backwards (restart): resynchronise.
            self.next_kick = track.kicks.partition_point(|k| k.time <= pos);
        }
        while let Some(kick) = track.kicks.get(self.next_kick).filter(|k| k.time <= pos) {
            let now = Instant::now();
            self.pulses.push(Pulse { started: now, strength: kick.strength, lane: LANES[self.lane % LANES.len()] });
            self.lane += 1;
            self.last_kick = Some(now);
            self.next_kick += 1;
        }
        self.last_pos = pos;
    }

    /// This frame's forces from recent kicks, for a domain of the given size.
    pub fn pulse_input(&mut self, domain: Vec2) -> (Vec<ForceBrush>, Vec<Emitter>) {
        let now = Instant::now();
        self.pulses.retain(|p| now - p.started < PULSE);
        let (mut brushes, mut emitters) = (Vec::new(), Vec::new());
        for p in &self.pulses {
            let x = domain.x * p.lane;
            match self.action {
                KickAction::Splash => brushes.push(ForceBrush {
                    pos: Vec2::new(x, 0.15 * domain.y),
                    radius: 0.22 * domain.y,
                    velocity: Vec2::new(0.0, (1.5 + 3.5 * p.strength) * self.gain),
                    strength: 45.0,
                }),
                KickAction::Jet => emitters.push(Emitter {
                    pos: Vec2::new(x, 0.07 * domain.y),
                    dir: Vec2::Y,
                    speed: (2.0 + 3.0 * p.strength) * self.gain,
                    width: 0.04,
                }),
            }
        }
        (brushes, emitters)
    }

    fn start(&mut self, loaded: Loaded) {
        let Loaded { name, channels, rate, samples, onsets } = loaded;
        let duration = samples.len() as f32 / (f32::from(channels.get()) * rate.get() as f32);
        if self.device.is_none() {
            match rodio::DeviceSinkBuilder::open_default_sink() {
                Ok(device) => self.device = Some(device),
                Err(e) => self.error = Some(format!("no audio output: {e}")),
            }
        }
        if let Some(device) = &self.device {
            let player = rodio::Player::connect_new(device.mixer());
            player.append(rodio::buffer::SamplesBuffer::new(channels, rate, samples.clone()));
            self.player = Some(player);
            self.apply_volume();
        }
        log::info!("loaded {name}: {duration:.1} s");
        self.samples = Some((channels, rate, samples));
        self.track = Some(Track { name, duration, kicks: Vec::new(), onsets });
        self.picked_with = f32::NAN;
        self.next_kick = 0;
        self.last_pos = 0.0;
    }
}

/// Decode a whole file and analyse it (runs on the loader thread).
fn decode(path: &PathBuf, name: String) -> Result<Loaded, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{name}: {e}"))?;
    let decoder = rodio::Decoder::try_from(file).map_err(|e| format!("{name}: {e}"))?;
    let (channels, rate) = (decoder.channels(), decoder.sample_rate());
    let samples: Vec<f32> = decoder.collect();
    let c = usize::from(channels.get());
    let mono: Vec<f32> = samples.chunks(c).map(|frame| frame.iter().sum::<f32>() / c as f32).collect();
    let onsets = kicks::onset_function(&mono, rate.get());
    Ok(Loaded { name, channels, rate, samples, onsets })
}
