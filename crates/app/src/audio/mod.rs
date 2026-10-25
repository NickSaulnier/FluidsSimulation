//! Audio-reactive mode: analyse the music (see `analyzer`) from either a track file
//! played by the app or whatever the computer is playing (live, see `live`), and drive
//! the fluid with it (see `reactions`).
//!
//! Both sources go through the same streaming analyzer: live audio as it's captured,
//! a track's decoded samples as playback passes them. Decoding runs on a background
//! thread so a long track doesn't freeze the window.

pub mod analyzer;
pub mod live;
pub mod reactions;

use analyzer::{Analyzer, AudioFeatures, Onset, OnsetKind};
use reactions::{Reaction, ReactionEngine, ReactionOutput};
use rodio::Source;
use sim::Vec2;
use std::num::NonZero;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Instant;

/// Track playback that jumps further than this (seeking) restarts the feed there
/// instead of analysing everything skipped.
const MAX_FEED_S: f32 = 0.5;

pub struct Track {
    pub name: String,
    pub duration: f32,
}

/// Decoded audio, produced by the loader thread.
struct Loaded {
    name: String,
    channels: NonZero<u16>,
    rate: NonZero<u32>,
    samples: Vec<f32>,
}

pub struct Music {
    pub track: Option<Track>,
    /// Capturing the computer's audio output instead of playing the track.
    pub live: Option<live::LiveInput>,
    pub error: Option<String>,
    /// Onset threshold, in standard deviations above the recent onset activity.
    pub sensitivity: f32,
    /// Scales every reaction.
    pub intensity: f32,
    pub muted: bool,
    /// Hits counted since the source started, and when each kind last hit
    /// (`OnsetKind::ALL` order).
    pub hits: [usize; 3],
    pub last_hit: [Option<Instant>; 3],
    /// The analyzer for the current source (its sample rate is the source's).
    analyzer: Option<Analyzer>,
    /// The output device, opened on first load (playback stops if it's dropped).
    device: Option<rodio::MixerDeviceSink>,
    player: Option<rodio::Player>,
    /// Interleaved samples, kept so the track can be restarted and analysed as it plays.
    samples: Option<(NonZero<u16>, NonZero<u32>, Vec<f32>)>,
    loading: Option<(String, Receiver<Result<Loaded, String>>)>,
    /// Track frames already fed to the analyzer.
    fed_frames: usize,
    /// Scratch: mono samples for the analyzer.
    mono: Vec<f32>,
    /// This frame's hits.
    frame_onsets: Vec<Onset>,
    engine: ReactionEngine,
}

impl Default for Music {
    fn default() -> Self {
        Self {
            track: None,
            live: None,
            error: None,
            sensitivity: 1.5,
            intensity: 1.0,
            muted: false,
            hits: [0; 3],
            last_hit: [None; 3],
            analyzer: None,
            device: None,
            player: None,
            samples: None,
            loading: None,
            fed_frames: 0,
            mono: Vec::new(),
            frame_onsets: Vec::new(),
            engine: ReactionEngine::default(),
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

    /// Switch between the track and live capture of the computer's audio output. The
    /// track pauses while live, or the capture would hear it too.
    pub fn set_live(&mut self, on: bool) {
        if on == self.live.is_some() {
            return;
        }
        if !on {
            self.live = None;
            self.restart_analysis(self.samples.as_ref().map(|(_, rate, _)| rate.get()));
            return;
        }
        if let Some(p) = &self.player {
            p.pause();
        }
        match live::LiveInput::open() {
            Ok(input) => {
                self.restart_analysis(Some(input.sample_rate));
                self.live = Some(input);
                self.error = None;
            }
            Err(e) => self.error = Some(format!("can't capture system audio: {e}")),
        }
    }

    pub fn is_live(&self) -> bool {
        self.live.is_some()
    }

    /// The latest analysis of the current source, if there is one.
    pub fn features(&self) -> Option<&AudioFeatures> {
        self.analyzer.as_ref().map(Analyzer::features)
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
            self.fed_frames = 0;
        }
    }

    pub fn apply_volume(&self) {
        if let Some(p) = &self.player {
            p.set_volume(if self.muted { 0.0 } else { 1.0 });
        }
    }

    /// Poll the loader, and feed the analyzer whatever the source produced since last
    /// frame.
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

        self.mono.clear();
        if let Some(input) = &mut self.live {
            input.drain(&mut self.mono);
        } else if let (Some((channels, rate, samples)), true) = (&self.samples, self.is_playing()) {
            // The frames playback has passed since last frame, downmixed.
            let c = usize::from(channels.get());
            let rate = rate.get() as f32;
            let now = ((self.position() * rate) as usize).min(samples.len() / c);
            if now < self.fed_frames || now - self.fed_frames > (MAX_FEED_S * rate) as usize {
                self.fed_frames = now; // restarted or skipped: pick up from here
            }
            let frames = &samples[self.fed_frames * c..now * c];
            self.mono.extend(frames.chunks(c).map(|f| f.iter().sum::<f32>() / c as f32));
            self.fed_frames = now;
        }
        self.frame_onsets.clear();
        let Some(analyzer) = &mut self.analyzer else { return };
        analyzer.process(&self.mono, self.sensitivity);
        self.frame_onsets = analyzer.take_onsets();
        for onset in &self.frame_onsets {
            let k = OnsetKind::ALL.iter().position(|&kind| kind == onset.kind).unwrap_or(0);
            self.hits[k] += 1;
            self.last_hit[k] = Some(Instant::now());
        }
    }

    /// This frame's reactions to the music (see `reactions`).
    pub fn react(&mut self, reactions: &[Reaction], domain: Vec2, base_gravity: Vec2, dt: f32) -> ReactionOutput {
        let features = self.analyzer.as_ref().map(Analyzer::features);
        self.engine.evaluate(reactions, features, &self.frame_onsets, domain, base_gravity, self.intensity, dt)
    }

    /// A fresh analyzer at `rate` (or none), with the hit counters reset.
    fn restart_analysis(&mut self, rate: Option<u32>) {
        self.analyzer = rate.map(Analyzer::new);
        self.hits = [0; 3];
        self.last_hit = [None; 3];
        self.fed_frames = 0;
    }

    fn start(&mut self, loaded: Loaded) {
        let Loaded { name, channels, rate, samples } = loaded;
        let duration = samples.len() as f32 / (f32::from(channels.get()) * rate.get() as f32);
        // Dropping a file means "play this": leave live mode.
        self.live = None;
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
        self.track = Some(Track { name, duration });
        self.restart_analysis(Some(rate.get()));
    }
}

/// Decode a whole file (runs on the loader thread).
fn decode(path: &PathBuf, name: String) -> Result<Loaded, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{name}: {e}"))?;
    let decoder = rodio::Decoder::try_from(file).map_err(|e| format!("{name}: {e}"))?;
    let (channels, rate) = (decoder.channels(), decoder.sample_rate());
    let samples: Vec<f32> = decoder.collect();
    Ok(Loaded { name, channels, rate, samples })
}
