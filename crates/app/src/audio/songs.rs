//! Generated songs for measuring the analyzer (see `eval`): drum kits, bass lines, pads
//! and leads, mixed in stereo like real tracks, with exact hit times (the generator knows
//! where it put every drum). Not real music, but much richer than a bare beat, and built
//! to include what troubles detectors: bass lines with vibrato and slides, swing and
//! ghost notes, a dense mix, an ambient track with few drums, and a loud hit followed by
//! quiet ones.
//!
//! Everything is deterministic (seeded noise), so scores are repeatable.

use super::analyzer::{Frame, OnsetKind};
use super::sections::SectionKind;
use std::f32::consts::TAU;
use utils::rng::SplitMix64;

pub const RATE: u32 = 44_100;

pub struct Song {
    pub name: &'static str,
    pub frames: Vec<Frame>,
    /// Every drum hit: (seconds, kind).
    pub hits: Vec<(f32, OnsetKind)>,
    /// Every beat: (seconds, whether it starts a bar).
    pub beats: Vec<(f32, bool)>,
    /// Every change of section: (seconds, kind), and the build-ups (from, to).
    pub sections: Vec<(f32, SectionKind)>,
    pub buildups: Vec<(f32, f32)>,
}

/// All the generated songs.
pub fn all() -> Vec<Song> {
    vec![house(), breakbeat(), rock(), ambient(), dynamics(), anthem()]
}

/// A one-pole low-pass filter.
struct LowPass {
    k: f32,
    y: f32,
}

impl LowPass {
    fn new(cutoff_hz: f32) -> Self {
        Self { k: 1.0 - (-TAU * cutoff_hz / RATE as f32).exp(), y: 0.0 }
    }

    fn step(&mut self, x: f32) -> f32 {
        self.y += (x - self.y) * self.k;
        self.y
    }
}

/// A song being mixed: two channels, the noise source, and the hits placed so far.
struct Mix {
    left: Vec<f32>,
    right: Vec<f32>,
    rng: SplitMix64,
    hits: Vec<(f32, OnsetKind)>,
    beats: Vec<(f32, bool)>,
    sections: Vec<(f32, SectionKind)>,
    buildups: Vec<(f32, f32)>,
}

impl Mix {
    fn new(seconds: f32, seed: u64) -> Self {
        let n = (seconds * RATE as f32) as usize;
        Self { left: vec![0.0; n], right: vec![0.0; n], rng: SplitMix64::new(seed), hits: Vec::new(), beats: Vec::new(), sections: Vec::new(), buildups: Vec::new() }
    }

    fn seconds(&self) -> f32 {
        self.left.len() as f32 / RATE as f32
    }

    /// Add `sound` from `start` seconds, panned (-1 left, 1 right, equal power).
    fn add(&mut self, start: f32, sound: &[f32], gain: f32, pan: f32) {
        let a = 0.25 * TAU * (pan.clamp(-1.0, 1.0) + 1.0) * 0.5;
        let (gl, gr) = (gain * a.cos(), gain * a.sin());
        let first = (start * RATE as f32) as usize;
        for (k, &s) in sound.iter().enumerate() {
            let i = first + k;
            if i >= self.left.len() {
                break;
            }
            self.left[i] += gl * s;
            self.right[i] += gr * s;
        }
    }

    fn noise(&mut self, n: usize) -> Vec<f32> {
        (0..n).map(|_| self.rng.signed()).collect()
    }

    /// A kick: a sine swept from 155 down to 45 Hz, with a short click.
    fn kick(&mut self, t: f32, gain: f32) {
        self.hits.push((t, OnsetKind::Kick));
        let n = (0.35 * RATE as f32) as usize;
        let click = self.noise(n);
        let mut phase = 0.0f32;
        let sound: Vec<f32> = (0..n)
            .map(|k| {
                let tt = k as f32 / RATE as f32;
                phase += TAU * (45.0 + 110.0 * (-tt * 35.0).exp()) / RATE as f32;
                phase.sin() * (-tt * 9.0).exp() + 0.3 * click[k] * (-tt / 0.001).exp()
            })
            .collect();
        self.add(t, &sound, gain, 0.0);
    }

    /// A snare: a 185 Hz body under a burst of band-limited noise.
    fn snare(&mut self, t: f32, gain: f32) {
        self.hits.push((t, OnsetKind::Snare));
        let n = (0.25 * RATE as f32) as usize;
        let noise = self.noise(n);
        let (mut low, mut lower) = (LowPass::new(6000.0), LowPass::new(900.0));
        let sound: Vec<f32> = (0..n)
            .map(|k| {
                let tt = k as f32 / RATE as f32;
                let band = low.step(noise[k]) - lower.step(noise[k]);
                0.5 * (TAU * 185.0 * tt).sin() * (-tt * 25.0).exp() + 1.2 * band * (-tt * 18.0).exp()
            })
            .collect();
        self.add(t, &sound, gain, -0.1);
    }

    /// A hi-hat: high-passed noise, short (closed) or ringing (open).
    fn hat(&mut self, t: f32, gain: f32, open: bool) {
        self.hits.push((t, OnsetKind::Hat));
        let (length, decay) = if open { (0.35, 0.12) } else { (0.06, 0.012) };
        let n = (length * RATE as f32) as usize;
        let noise = self.noise(n);
        let mut low = LowPass::new(5000.0);
        let sound: Vec<f32> = (0..n).map(|k| (noise[k] - low.step(noise[k])) * (-(k as f32) / (decay * RATE as f32)).exp()).collect();
        self.add(t, &sound, gain, 0.4);
    }

    /// A bass note: a low-passed sawtooth, `vibrato` deep (a fraction of the pitch),
    /// gliding to `to_hz` if given. `drive` above 1 clips it (a distorted bass).
    #[allow(clippy::too_many_arguments)]
    fn bass(&mut self, t: f32, length: f32, hz: f32, to_hz: Option<f32>, gain: f32, vibrato: f32, drive: f32) {
        let n = (length * RATE as f32) as usize;
        let mut filter = LowPass::new(500.0);
        let mut phase = 0.0f32;
        let sound: Vec<f32> = (0..n)
            .map(|k| {
                let tt = k as f32 / RATE as f32;
                let glide = to_hz.map_or(hz, |to| hz + (to - hz) * (tt / length).min(1.0));
                let f = glide * (1.0 + vibrato * (TAU * 5.5 * tt).sin());
                phase = (phase + f / RATE as f32).fract();
                let envelope = (tt / 0.01).min(1.0) * ((length - tt) / 0.05).clamp(0.0, 1.0);
                (filter.step(2.0 * phase - 1.0) * drive).tanh() * envelope
            })
            .collect();
        self.add(t, &sound, gain, 0.0);
    }

    /// A pad chord: three detuned sawtooths per note, fading in slowly.
    fn pad(&mut self, t: f32, length: f32, notes: &[f32], gain: f32, drive: f32) {
        let n = (length * RATE as f32) as usize;
        let mut sound = vec![0.0f32; n];
        for &hz in notes {
            for detune in [-0.004, 0.0, 0.004] {
                let mut phase = self.rng.next_f32();
                let mut filter = LowPass::new(1500.0);
                for s in sound.iter_mut() {
                    phase = (phase + hz * (1.0 + detune) / RATE as f32).fract();
                    *s += filter.step(2.0 * phase - 1.0) / (3.0 * notes.len() as f32);
                }
            }
        }
        for (k, s) in sound.iter_mut().enumerate() {
            let tt = k as f32 / RATE as f32;
            *s = (*s * drive).tanh() * (tt / 0.8).min(1.0) * ((length - tt) / 0.5).clamp(0.0, 1.0);
        }
        self.add(t, &sound, gain, -0.3);
    }

    /// A lead note: a sine with a second harmonic and vibrato, gently attacked.
    fn lead(&mut self, t: f32, length: f32, hz: f32, gain: f32) {
        let n = (length * RATE as f32) as usize;
        let mut phase = 0.0f32;
        let sound: Vec<f32> = (0..n)
            .map(|k| {
                let tt = k as f32 / RATE as f32;
                phase += TAU * hz * (1.0 + 0.006 * (TAU * 6.0 * tt).sin()) / RATE as f32;
                let envelope = (tt / 0.03).min(1.0) * ((length - tt) / 0.08).clamp(0.0, 1.0);
                (phase.sin() + 0.3 * (2.0 * phase).sin()) * envelope
            })
            .collect();
        self.add(t, &sound, gain, 0.2);
    }

    /// A riser: noise through a low-pass opening from 300 Hz to 8 kHz, swelling over
    /// `length`.
    fn riser(&mut self, t: f32, length: f32, gain: f32) {
        let n = (length * RATE as f32) as usize;
        let noise = self.noise(n);
        let mut y = 0.0f32;
        let sound: Vec<f32> = (0..n)
            .map(|k| {
                let x = k as f32 / n as f32;
                let cutoff = 300.0 * (8000.0f32 / 300.0).powf(x);
                y += (noise[k] - y) * (1.0 - (-TAU * cutoff / RATE as f32).exp());
                y * x * x
            })
            .collect();
        self.add(t, &sound, gain, 0.0);
    }

    /// The beats from `start`, `beat` apart, to the end: four to a bar.
    fn beats(&mut self, start: f32, beat: f32) {
        let mut k = 0;
        while start + k as f32 * beat < self.seconds() {
            self.beats.push((start + k as f32 * beat, k % 4 == 0));
            k += 1;
        }
    }

    fn finish(mut self, name: &'static str) -> Song {
        self.hits.sort_by(|a, b| a.0.total_cmp(&b.0));
        let peak = self.left.iter().chain(&self.right).fold(0.0f32, |m, s| m.max(s.abs())).max(1e-6);
        let scale = 0.8 / peak;
        let frames = self.left.iter().zip(&self.right).map(|(l, r)| [l * scale, r * scale]).collect();
        Song { name, frames, hits: self.hits, beats: self.beats, sections: self.sections, buildups: self.buildups }
    }
}

/// Note frequency from a MIDI note number.
fn midi(note: f32) -> f32 {
    440.0 * 2f32.powf((note - 69.0) / 12.0)
}

/// 124 BPM four on the floor: kicks on every beat, a clap on 2 and 4, closed hats on
/// the off-beats, an off-beat bass line and a pad.
fn house() -> Song {
    let beat = 60.0 / 124.0;
    let mut m = Mix::new(20.0, 1);
    let roots = [45.0, 45.0, 41.0, 43.0];
    let mut t = 0.2;
    let mut b = 0;
    while t + beat < m.seconds() {
        m.kick(t, 1.0);
        if b % 2 == 1 {
            m.snare(t, 0.7);
        }
        m.hat(t + 0.5 * beat, 0.35, false);
        let root = roots[(b / 4) % 4];
        m.bass(t + 0.5 * beat, 0.45 * beat, midi(root), None, 0.5, 0.0, 1.0);
        if b % 4 == 0 {
            m.pad(t, 4.0 * beat, &[midi(root + 24.0), midi(root + 28.0), midi(root + 31.0)], 0.25, 1.0);
        }
        t += beat;
        b += 1;
    }
    m.beats(0.2, 60.0 / 124.0);
    m.finish("house 124")
}

/// 92 BPM swung breakbeat: kicks on 1 and the "and" of 2, snares on 2 and 4 with quiet
/// ghost notes, swung sixteenth hats with accents, a gliding legato bass with vibrato.
fn breakbeat() -> Song {
    let beat = 60.0 / 92.0;
    let swing = 0.62; // the off sixteenth lands this far through the eighth
    let mut m = Mix::new(20.0, 2);
    let mut bar_start = 0.2;
    let mut bar = 0;
    while bar_start + 4.0 * beat < m.seconds() {
        let at = |b: f32| bar_start + b * beat;
        m.kick(at(0.0), 1.0);
        m.kick(at(1.5), 0.8);
        m.snare(at(1.0), 0.8);
        m.snare(at(3.0), 0.8);
        m.snare(at(2.75), 0.2); // ghost notes
        m.snare(at(3.5) + 0.25 * beat * (swing - 0.5), 0.2);
        for eighth in 0..8 {
            let e = at(0.5 * eighth as f32);
            m.hat(e, if eighth % 2 == 0 { 0.4 } else { 0.25 }, false);
            m.hat(e + swing * 0.5 * beat, 0.15, false);
        }
        let root = if bar % 2 == 0 { 40.0 } else { 43.0 };
        m.bass(at(0.0), 1.5 * beat, midi(root), None, 0.55, 0.01, 1.0);
        m.bass(at(1.5), 2.5 * beat, midi(root + 3.0), Some(midi(root + 5.0)), 0.55, 0.015, 1.0);
        bar_start += 4.0 * beat;
        bar += 1;
    }
    m.beats(0.2, 60.0 / 92.0);
    m.finish("breakbeat 92")
}

/// 140 BPM rock: kicks on 1 and 3 with a pickup, a loud snare on 2 and 4, open hats on
/// the eighths, a distorted bass on the roots, and distorted power chords throughout.
fn rock() -> Song {
    let beat = 60.0 / 140.0;
    let mut m = Mix::new(20.0, 3);
    let roots = [40.0, 40.0, 43.0, 38.0];
    let mut bar_start = 0.2;
    let mut bar = 0;
    while bar_start + 4.0 * beat < m.seconds() {
        let at = |b: f32| bar_start + b * beat;
        m.kick(at(0.0), 1.0);
        m.kick(at(2.0), 1.0);
        m.kick(at(2.75), 0.7);
        m.snare(at(1.0), 1.0);
        m.snare(at(3.0), 1.0);
        for eighth in 0..8 {
            m.hat(at(0.5 * eighth as f32), 0.3, eighth % 4 == 3);
        }
        let root = roots[bar % 4];
        for eighth in 0..8 {
            m.bass(at(0.5 * eighth as f32), 0.45 * beat, midi(root), None, 0.45, 0.0, 3.0);
        }
        m.pad(at(0.0), 4.0 * beat, &[midi(root + 12.0), midi(root + 19.0), midi(root + 24.0)], 0.35, 4.0);
        bar_start += 4.0 * beat;
        bar += 1;
    }
    m.beats(0.2, 60.0 / 140.0);
    m.finish("rock 140")
}

/// 70 BPM ambient: long pad chords, a sustained bass with deep vibrato, a slow lead and
/// a soft hat every two beats: hardly any drums, plenty to be fooled by.
fn ambient() -> Song {
    let beat = 60.0 / 70.0;
    let mut m = Mix::new(20.0, 4);
    let chords = [[50.0, 57.0, 62.0, 65.0], [46.0, 53.0, 58.0, 62.0]];
    let mut t = 0.2;
    let mut k = 0;
    while t < m.seconds() {
        let chord = chords[k % 2];
        m.pad(t, 8.0 * beat, &chord.map(midi), 0.35, 1.0);
        m.bass(t, 8.0 * beat, midi(chord[0] - 12.0), None, 0.4, 0.03, 1.0);
        for n in 0..4 {
            m.lead(t + 2.0 * n as f32 * beat, 1.8 * beat, midi(chord[n % 4] + 12.0), 0.15);
            m.hat(t + (2.0 * n as f32 + 1.0) * beat, 0.12, true);
        }
        t += 8.0 * beat;
        k += 1;
    }
    // Hits past the end were never heard.
    let end = m.seconds();
    m.hits.retain(|h| h.0 < end);
    m.beats(0.2, 60.0 / 70.0);
    m.finish("ambient 70")
}

/// 128 BPM with a song's structure: an intro (pads and soft hats), a build-up (a snare
/// roll speeding up from quarters to 32nds and a riser, no kick or bass), a drop (four on
/// the floor, a bass line, claps, open hats and a lead), a breakdown (pads and soft hats
/// again), another build-up and a last drop. For sections; the hits are all there too.
fn anthem() -> Song {
    let beat = 60.0 / 128.0;
    let bar = 4.0 * beat;
    let mut m = Mix::new(72.0, 6);
    let start = 0.2;
    let roots = [45.0, 41.0, 43.0, 40.0];
    let parts: [(&str, usize); 6] = [("intro", 8), ("build", 4), ("drop", 8), ("breakdown", 6), ("build", 4), ("drop", 8)];
    let mut t = start;
    for (part, bars) in parts {
        match part {
            "build" => {
                m.sections.push((t, SectionKind::Change));
                m.buildups.push((t, t + bars as f32 * bar));
                m.riser(t, bars as f32 * bar, 0.5);
            }
            "drop" => m.sections.push((t, SectionKind::Drop)),
            "breakdown" => m.sections.push((t, SectionKind::Breakdown)),
            _ => {}
        }
        for b in 0..bars {
            let bar_start = t + b as f32 * bar;
            let root = roots[b % 4];
            let at = |q: f32| bar_start + q * beat;
            match part {
                "intro" | "breakdown" => {
                    m.pad(bar_start, bar, &[midi(root + 24.0), midi(root + 28.0), midi(root + 31.0)], 0.3, 1.0);
                    for e in 0..8 {
                        m.hat(at(0.5 * e as f32), if e % 2 == 1 { 0.12 } else { 0.06 }, false);
                    }
                }
                "build" => {
                    m.pad(bar_start, bar, &[midi(root + 24.0), midi(root + 28.0), midi(root + 31.0)], 0.3, 1.0);
                    // The roll: quarters, eighths, sixteenths, 32nds, a bar each, swelling.
                    let per_beat = 1usize << b.min(3);
                    for k in 0..4 * per_beat {
                        let swell = (b as f32 + k as f32 / (4 * per_beat) as f32) / bars as f32;
                        m.snare(at(k as f32 / per_beat as f32), 0.25 + 0.6 * swell);
                    }
                }
                _ => {
                    m.pad(bar_start, bar, &[midi(root + 24.0), midi(root + 28.0), midi(root + 31.0)], 0.3, 1.0);
                    for q in 0..4 {
                        m.kick(at(q as f32), 1.0);
                        m.bass(at(q as f32 + 0.5), 0.45 * beat, midi(root), None, 0.55, 0.0, 1.0);
                        m.hat(at(q as f32 + 0.5), 0.35, true);
                    }
                    m.snare(at(1.0), 0.7);
                    m.snare(at(3.0), 0.7);
                    m.lead(at(0.0), 1.5 * beat, midi(root + 36.0), 0.15);
                    m.lead(at(2.0), 1.5 * beat, midi(root + 40.0), 0.15);
                }
            }
        }
        t += bars as f32 * bar;
    }
    m.beats(start, beat);
    m.finish("anthem 128")
}

/// 110 BPM, four bars as normal, then one huge hit (kick, snare and an open hat at three
/// times the level), then four bars at a third of the level: the quiet hits after the
/// big one must still be heard.
fn dynamics() -> Song {
    let beat = 60.0 / 110.0;
    let mut m = Mix::new(20.0, 5);
    let mut t = 0.2;
    let mut b = 0;
    while t + beat < m.seconds() {
        let gain = match b {
            0..16 => 1.0,
            16 => 3.0,
            _ => 0.33,
        };
        m.kick(t, gain);
        if b % 2 == 1 || b == 16 {
            m.snare(t, 0.7 * gain);
        }
        m.hat(t + if b == 16 { 0.0 } else { 0.5 * beat }, 0.35 * gain, b == 16);
        m.bass(t, 0.9 * beat, midi(38.0), None, 0.4 * gain, 0.0, 1.0);
        if b == 17 {
            m.sections.push((t, SectionKind::Breakdown));
        }
        t += beat;
        b += 1;
    }
    m.beats(0.2, 60.0 / 110.0);
    m.finish("dynamics 110")
}
