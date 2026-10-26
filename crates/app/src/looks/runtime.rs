//! A look's equations, running: each frame the inputs are filled from the music and
//! the clock, the motion and the frame outputs start from the look's settings (as the
//! View tab has them), the per-frame program runs, and what it set becomes this frame's
//! feedback, waves, gravity tilt, reaction intensity and solver parameters. Values that
//! come out as no number (a NaN, an infinity) fall back to the setting.

use super::Code;
use crate::audio::analyzer::{AudioFeatures, Onset, OnsetKind};
use crate::audio::sections::SectionKind;
use crate::expr::eval::{Layout, Machine};
use crate::expr::{FRAME_OUTPUTS, MOTION, Program, SHARED, Stage, slot};
use crate::render::composite::CompositeSettings;
use crate::render::feedback::FeedbackSettings;
use crate::render::waves::WaveSettings;
use utils::rng::SplitMix64;

/// Hit and beat pulses fall from 1 with this time constant, seconds; section changes
/// (bigger moments) with this one.
const PULSE_S: f32 = 0.1;
const SECTION_PULSE_S: f32 = 0.5;

/// What the music did this frame, for the inputs.
#[derive(Clone, Copy, Default)]
pub struct Sound<'a> {
    pub features: Option<&'a AudioFeatures>,
    pub onsets: &'a [Onset],
    /// Beats heard this frame.
    pub beats: usize,
}

/// One frame's results.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub feedback: FeedbackSettings,
    pub waves: WaveSettings,
    pub composite: CompositeSettings,
    /// The values the per-pixel shader reads (inputs, motion, q's).
    pub shared: [f32; SHARED],
    /// Gravity's tilt, degrees (anticlockwise), and the reactions' intensity (a multiple).
    pub gravity_tilt: f32,
    pub intensity: f32,
    /// Solver parameters to set: key, value.
    pub params: Vec<(String, f32)>,
}

impl Frame {
    /// A frame of the settings as they are, with no equations.
    pub fn plain(feedback: &FeedbackSettings, waves: &WaveSettings, composite: &CompositeSettings) -> Self {
        Self { feedback: *feedback, waves: *waves, composite: *composite, shared: feedback.shared(0.0), gravity_tilt: 0.0, intensity: 1.0, params: Vec::new() }
    }
}

pub struct Runtime {
    init: Option<Machine>,
    frame: Option<Machine>,
    params: Vec<String>,
    layout: Layout,
    block: Vec<f32>,
    rng: SplitMix64,
    started: bool,
    time: f32,
    frames: f32,
    /// Kick, snare, hat and beat, each 1 when it happens and falling after; any section
    /// change, a drop and a breakdown, the same; and the section changes seen so far.
    pulses: [f32; 4],
    section_pulses: [f32; 3],
    sections_seen: u32,
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new(&Code::default())
    }
}

impl Runtime {
    /// Start `code` (none for a look without equations).
    pub fn new(code: &Code) -> Self {
        let frame = code.frame.as_ref().map(Machine::new);
        let layout = frame.as_ref().map_or_else(|| Layout::of(&Program::empty(Stage::Frame)), |m| m.layout);
        Self {
            init: code.init.as_ref().map(Machine::new),
            frame,
            params: code.frame.as_ref().map_or_else(Vec::new, |p| p.params.clone()),
            block: vec![0.0; layout.len],
            layout,
            rng: SplitMix64::new(0x6c6f_6f6b),
            started: false,
            time: 0.0,
            frames: 0.0,
            pulses: [0.0; 4],
            section_pulses: [0.0; 3],
            sections_seen: 0,
        }
    }

    /// Run a frame of `dt` seconds (0 while paused) on a picture `aspect` wide, from the
    /// look's settings as they stand (`feedback`, `waves`, `composite`) and the music.
    pub fn step(&mut self, feedback: &FeedbackSettings, waves: &WaveSettings, composite: &CompositeSettings, sound: Sound<'_>, dt: f32, aspect: f32) -> Frame {
        self.time += dt;
        self.frames += 1.0;
        let fall = (-dt / PULSE_S).exp();
        for p in &mut self.pulses {
            *p *= fall;
        }
        for o in sound.onsets {
            let k = OnsetKind::ALL.iter().position(|&kind| kind == o.kind).unwrap_or(0);
            self.pulses[k] = 1.0;
        }
        if sound.beats > 0 {
            self.pulses[3] = 1.0;
        }
        let fall = (-dt / SECTION_PULSE_S).exp();
        for p in &mut self.section_pulses {
            *p *= fall;
        }
        let sections = sound.features.map(|f| f.sections).unwrap_or_default();
        if sections.changes > self.sections_seen
            && let Some(s) = sections.last
        {
            self.section_pulses[0] = 1.0;
            match s.kind {
                SectionKind::Drop => self.section_pulses[1] = 1.0,
                SectionKind::Breakdown => self.section_pulses[2] = 1.0,
                SectionKind::Change => {}
            }
        }
        self.sections_seen = sections.changes;

        // The inputs.
        let f = sound.features.cloned().unwrap_or_else(|| AudioFeatures { silent: true, ..AudioFeatures::default() });
        let b = &mut self.block;
        let mut set = |name: &str, v: f32| b[slot(name)] = v;
        set("time", self.time);
        set("frame", self.frames);
        set("fps", if dt > 0.0 { 1.0 / dt } else { 0.0 });
        set("dt", dt);
        set("aspect", aspect);
        let m = f.milk;
        for (name, v) in [("bass", m.bass), ("mid", m.mid), ("treb", m.treb), ("bass_att", m.bass_att), ("mid_att", m.mid_att), ("treb_att", m.treb_att)] {
            set(name, v);
        }
        for (name, v) in ["band_sub", "band_bass", "band_lowmid", "band_mid", "band_high"].into_iter().zip(f.bands) {
            set(name, v);
        }
        set("level", f.level);
        set("brightness", f.brightness);
        set("loudness", f.loudness);
        for (name, v) in ["kick", "snare", "hat", "beat"].into_iter().zip(self.pulses) {
            set(name, v);
        }
        let t = f.musical;
        set("beat_phase", t.beat_phase);
        set("bar_phase", t.bar_phase);
        set("bpm", t.bpm);
        set("beating", f32::from(u8::from(t.beating)));
        set("silent", f32::from(u8::from(f.silent)));
        set("novelty", f.sections.novelty);
        set("buildup", f.sections.buildup);
        for (name, v) in ["section", "drop", "breakdown"].into_iter().zip(self.section_pulses) {
            set(name, v);
        }

        // The motion and the outputs start from the settings each frame (the q's and the
        // program's own variables carry over).
        let motion = feedback.motion(f.bands[1]);
        let start = slot(MOTION[0]);
        self.block[start..start + MOTION.len()].copy_from_slice(&motion);
        let outputs = outputs(feedback, waves, composite);
        self.block[self.layout.outputs..self.layout.outputs + FRAME_OUTPUTS.len()].copy_from_slice(&outputs);

        if !self.started {
            self.started = true;
            if let Some(init) = &mut self.init {
                init.run(&mut self.block, &mut self.rng);
            }
        }
        if let Some(frame) = &mut self.frame {
            frame.run(&mut self.block, &mut self.rng);
        }

        // Back into settings, falling back where the equations made no number.
        let clean = |v: f32, fallback: f32| if v.is_finite() { v } else { fallback };
        let mut shared = [0.0; SHARED];
        shared.copy_from_slice(&self.block[..SHARED]);
        for (i, &base) in motion.iter().enumerate() {
            shared[start + i] = clean(shared[start + i], base);
        }
        let out: Vec<f32> = (0..FRAME_OUTPUTS.len()).map(|i| clean(self.block[self.layout.outputs + i], outputs[i])).collect();
        let get = |name: &str| out[FRAME_OUTPUTS.iter().position(|n| *n == name).expect("an output")];
        let mut fb = *feedback;
        fb.trail = get("trail").clamp(0.0, 0.98);
        fb.warp_speed = get("warp_speed");
        fb.flow = get("flow").clamp(0.0, 10.0);
        fb.echo = get("echo").clamp(0.0, 1.0);
        fb.echo_zoom = get("echo_zoom").clamp(0.1, 10.0);
        let mut w = *waves;
        w.scale = get("wave_scale").clamp(0.0, 4.0);
        w.size = get("wave_size").clamp(0.0, 2.0);
        w.y = get("wave_y").clamp(-2.0, 2.0);
        w.thickness = get("wave_thickness").clamp(0.0, 40.0);
        w.brightness = get("wave_brightness").clamp(0.0, 8.0);
        w.push = get("wave_push").clamp(0.0, 1.0);
        for (k, shape) in w.shapes.iter_mut().enumerate() {
            let s = |field: &str| get(&format!("shape{}_{field}", k + 1));
            shape.radius = s("radius").clamp(0.0, 4.0);
            shape.centre = [s("x").clamp(-4.0, 4.0), s("y").clamp(-4.0, 4.0)];
            shape.spin = s("spin").clamp(-100.0, 100.0);
            shape.fill = s("fill").clamp(0.0, 1.0);
        }
        let mut c = *composite;
        c.segments = get("segments").round().clamp(2.0, 16.0) as u32;
        c.symmetry_spin = get("symmetry_spin").clamp(-10.0, 10.0);
        c.hue = get("hue");
        c.hue_speed = get("hue_speed").clamp(-720.0, 720.0);
        c.saturation = get("saturation").clamp(0.0, 4.0);
        c.contrast = get("contrast").clamp(0.0, 4.0);
        c.exposure = get("exposure").clamp(0.0, 4.0);
        c.palette_map = get("palette_map").clamp(0.0, 1.0);
        c.invert = get("invert").clamp(0.0, 1.0);
        c.aberration = get("aberration").clamp(0.0, 0.1);
        c.vignette = get("vignette").clamp(0.0, 1.0);
        c.grain = get("grain").clamp(0.0, 1.0);
        // Every parameter a program names, it sets each frame (statements always run).
        let params = self.params.iter().enumerate().filter_map(|(i, key)| Some(key.clone()).zip(Some(self.block[self.layout.params + i]).filter(|v| v.is_finite()))).collect();
        Frame { feedback: fb, waves: w, composite: c, shared, gravity_tilt: get("gravity").clamp(-360.0, 360.0), intensity: get("intensity").clamp(0.0, 10.0), params }
    }
}

/// The frame outputs as the settings have them (`FRAME_OUTPUTS` order).
fn outputs(feedback: &FeedbackSettings, waves: &WaveSettings, composite: &CompositeSettings) -> Vec<f32> {
    let [s1, s2] = waves.shapes;
    FRAME_OUTPUTS
        .iter()
        .map(|name| match *name {
            "trail" => feedback.trail,
            "warp_speed" => feedback.warp_speed,
            "flow" => feedback.flow,
            "echo" => feedback.echo,
            "echo_zoom" => feedback.echo_zoom,
            "wave_scale" => waves.scale,
            "wave_size" => waves.size,
            "wave_y" => waves.y,
            "wave_thickness" => waves.thickness,
            "wave_brightness" => waves.brightness,
            "wave_push" => waves.push,
            "gravity" => 0.0,
            "intensity" => 1.0,
            "segments" => composite.segments as f32,
            "symmetry_spin" => composite.symmetry_spin,
            "hue" => composite.hue,
            "hue_speed" => composite.hue_speed,
            "saturation" => composite.saturation,
            "contrast" => composite.contrast,
            "exposure" => composite.exposure,
            "palette_map" => composite.palette_map,
            "invert" => composite.invert,
            "aberration" => composite.aberration,
            "vignette" => composite.vignette,
            "grain" => composite.grain,
            other => {
                let (shape, field) = other.split_once('_').expect("shapeN_field");
                let s = if shape == "shape1" { s1 } else { s2 };
                match field {
                    "radius" => s.radius,
                    "x" => s.centre[0],
                    "y" => s.centre[1],
                    "spin" => s.spin,
                    "fill" => s.fill,
                    _ => unreachable!("an output with no setting: {other}"),
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::compile;

    fn code(init: &str, frame: &str) -> Code {
        let joined = format!("{init}\n{frame}");
        let p = compile(&joined, Stage::Frame).unwrap();
        let n = compile(init, Stage::Frame).unwrap().stmts.len();
        let (a, b) = p.stmts.split_at(n);
        Code { init: Some(Program { stmts: a.to_vec(), ..p.clone() }), frame: Some(Program { stmts: b.to_vec(), ..p.clone() }), ..Code::default() }
    }

    /// Without equations a frame is the settings, the motion among the shared values.
    #[test]
    fn without_equations_the_settings_pass_through() {
        let fb = FeedbackSettings { enabled: true, zoom: 1.3, rotation: 0.2, trail: 0.4, ..FeedbackSettings::OFF };
        let mut r = Runtime::default();
        let f = r.step(&fb, &WaveSettings::OFF, &CompositeSettings::OFF, Sound::default(), 1.0 / 60.0, 1.5);
        assert_eq!((f.feedback, f.waves, f.gravity_tilt, f.intensity), (fb, WaveSettings::OFF, 0.0, 1.0));
        assert_eq!(f.shared[slot("zoom")], 1.3);
        assert_eq!(f.shared[slot("rot")], 0.2);
        assert_eq!(f.shared[slot("aspect")], 1.5);
        assert!(f.params.is_empty());
    }

    /// The equations set the motion, the outputs and parameters; `init` runs once and
    /// its variables carry on; the clock and the music arrive as inputs.
    #[test]
    fn equations_drive_the_frame() {
        let c = code("n = 10", "n = n + 1; zoom = 1 + n / 100; trail = 0.2 + bass; q1 = time; wave_scale = 2; shape1_spin = -1; gravity = 15; param_buoyancy = 0.5 * kick");
        let mut r = Runtime::new(&c);
        let mut features = AudioFeatures::default();
        features.milk.bass = 0.3;
        let kick = [Onset { kind: OnsetKind::Kick, time: 0.0, strength: 1.0 }];
        let sound = Sound { features: Some(&features), onsets: &kick, beats: 1 };
        let f = r.step(&FeedbackSettings::OFF, &WaveSettings::OFF, &CompositeSettings::OFF, sound, 0.5, 1.0);
        assert!((f.shared[slot("zoom")] - 1.11).abs() < 1e-6);
        assert!((f.feedback.trail - 0.5).abs() < 1e-6);
        assert_eq!((f.shared[slot("q1")], f.waves.scale, f.waves.shapes[0].spin, f.gravity_tilt), (0.5, 2.0, -1.0, 15.0));
        assert_eq!(f.params, [("buoyancy".to_string(), 0.5)]);
        let f = r.step(&FeedbackSettings::OFF, &WaveSettings::OFF, &CompositeSettings::OFF, Sound { features: Some(&features), ..Sound::default() }, 0.5, 1.0);
        assert!((f.shared[slot("zoom")] - 1.12).abs() < 1e-6, "n carried on from init");
        assert!(f.params[0].1 < 0.01, "the kick's pulse fell: {:?}", f.params);
    }

    /// A section change arrives as pulses (any change, and its kind), falling slowly;
    /// novelty and build-up as they are.
    #[test]
    fn sections_arrive_as_pulses() {
        let c = code("", "q1 = section; q2 = drop; q3 = breakdown; q4 = buildup");
        let mut r = Runtime::new(&c);
        let mut f = AudioFeatures::default();
        f.sections.buildup = 0.7;
        f.sections.changes = 1;
        f.sections.last = Some(crate::audio::sections::Section { kind: SectionKind::Drop, time: 0.0 });
        let sound = Sound { features: Some(&f), ..Sound::default() };
        let out = r.step(&FeedbackSettings::OFF, &WaveSettings::OFF, &CompositeSettings::OFF, sound, 1.0 / 60.0, 1.0);
        let q = |i: usize| out.shared[slot(&format!("q{i}"))];
        assert!(q(1) > 0.9 && q(2) > 0.9 && q(3) == 0.0 && (q(4) - 0.7).abs() < 1e-6, "{:?}", [q(1), q(2), q(3), q(4)]);
        let out = r.step(&FeedbackSettings::OFF, &WaveSettings::OFF, &CompositeSettings::OFF, sound, 0.5, 1.0);
        let drop = out.shared[slot("q2")];
        assert!((0.2..0.5).contains(&drop), "the same change again isn't new, and the pulse falls over half a second: {drop}");
    }

    /// The composite's outputs start from its settings and take the equations' values,
    /// in range.
    #[test]
    fn equations_drive_the_composite() {
        let c = code("", "hue = 30; segments = 7.6; saturation = 5; vignette = vignette + 0.25");
        let base = CompositeSettings { vignette: 0.5, contrast: 1.2, ..CompositeSettings::OFF };
        let f = Runtime::new(&c).step(&FeedbackSettings::OFF, &WaveSettings::OFF, &base, Sound::default(), 0.1, 1.0);
        let out = f.composite;
        assert_eq!((out.hue, out.segments, out.saturation, out.vignette, out.contrast), (30.0, 8, 4.0, 0.75, 1.2));
    }

    /// Equations that make no number fall back to the settings, and outputs stay in range.
    #[test]
    fn bad_values_fall_back() {
        let c = code("", "zoom = 0 / 0 + log(0) * 1e30 * 1e30; rot = sqrt(-1) / 0; trail = 5; echo = -1");
        let fb = FeedbackSettings { enabled: true, zoom: 1.2, rotation: 0.1, ..FeedbackSettings::OFF };
        let f = Runtime::new(&c).step(&fb, &WaveSettings::OFF, &CompositeSettings::OFF, Sound::default(), 0.1, 1.0);
        assert!(f.shared.iter().all(|v| v.is_finite()));
        assert_eq!(f.shared[slot("zoom")], 1.2, "an infinite zoom falls back");
        assert_eq!((f.feedback.trail, f.feedback.echo), (0.98, 0.0));
    }
}
