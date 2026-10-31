//! Every look follows the song's shape (the energy, see `audio::dynamics`): gentler,
//! slower and dimmer while the song is quiet, livelier when it's loud, tensing through a
//! build-up and flashing at the drop. Applied to each frame after the look's equations,
//! scaled by the look's `dynamics` (0 leaves it as set); at the neutral energy a look is
//! as it's set. Without music nothing changes.

use super::runtime::Frame;
use crate::audio::analyzer::AudioFeatures;
use crate::audio::dynamics::NEUTRAL;
use crate::audio::sections::SectionKind;
use crate::expr::slot;

/// What the energy moves, as multiples of the look's own value: at no energy, and at
/// full (1 at the neutral energy).
const REACTIONS: (f32, f32) = (0.4, 1.25);
/// The feedback's motion: the zoom (as growth), turn, shift and wobble.
const MOTION: (f32, f32) = (0.35, 1.3);
const EXPOSURE: (f32, f32) = (0.75, 1.1);
const SATURATION: (f32, f32) = (0.7, 1.15);
const WAVE_THICKNESS: (f32, f32) = (0.6, 1.3);
/// A fractal's dive speed.
const DIVE: (f32, f32) = (0.4, 1.3);
/// Through a full build-up: the zoom creeping in (growth per second, on top), the
/// picture brighter and the reactions stronger, as shares.
const TENSION_ZOOM: f32 = 0.12;
const TENSION_EXPOSURE: f32 = 0.12;
const TENSION_REACTIONS: f32 = 0.4;
/// At a drop, falling over `DROP_S`: a flash (exposure, a share on top) and a burst (the
/// reactions this much stronger), on whichever look the drop cuts to.
const DROP_FLASH: f32 = 0.6;
const DROP_BURST: f32 = 1.5;
const DROP_S: f32 = 0.6;

/// The song's shape this frame, as it moves a look.
#[derive(Clone, Copy, Debug)]
pub struct Shape {
    energy: f32,
    buildup: f32,
    /// The drop's pulse, 1 falling to 0.
    drop: f32,
    /// The look's `dynamics`.
    amount: f32,
}

impl Shape {
    /// From the music (none without it) and the look's `dynamics`.
    pub fn new(features: Option<&AudioFeatures>, amount: f32) -> Option<Self> {
        let f = features?;
        let drop = match f.sections.last {
            Some(s) if s.kind == SectionKind::Drop => (-(f.time - s.time).max(0.0) / DROP_S).exp(),
            _ => 0.0,
        };
        Some(Self { energy: f.energy, buildup: f.sections.buildup, drop, amount: amount.clamp(0.0, 1.0) })
    }

    /// The multiple of a value in `range` at this energy, scaled by the look's amount.
    fn scale(&self, (low, high): (f32, f32)) -> f32 {
        let e = self.energy;
        let full = if e < NEUTRAL { low + (1.0 - low) * e / NEUTRAL } else { 1.0 + (high - 1.0) * (e - NEUTRAL) / (1.0 - NEUTRAL) };
        1.0 + self.amount * (full - 1.0)
    }

    /// A share on top, scaled by the look's amount.
    fn more(&self, share: f32) -> f32 {
        1.0 + self.amount * share
    }

    /// The multiple of a fractal's dive speed.
    pub fn dive(&self) -> f32 {
        self.scale(DIVE)
    }

    pub fn apply(&self, frame: &mut Frame) {
        frame.intensity *= self.scale(REACTIONS) * self.more(TENSION_REACTIONS * self.buildup) * self.more(DROP_BURST * self.drop);
        let motion = self.scale(MOTION);
        let zoom = &mut frame.shared[slot("zoom")];
        *zoom = (1.0 + (*zoom - 1.0) * motion) * self.more(TENSION_ZOOM * self.buildup);
        for name in ["rot", "dx", "dy", "warp"] {
            frame.shared[slot(name)] *= motion;
        }
        let c = &mut frame.composite;
        c.exposure *= self.scale(EXPOSURE) * self.more(TENSION_EXPOSURE * self.buildup) * self.more(DROP_FLASH * self.drop);
        c.saturation *= self.scale(SATURATION);
        frame.waves.thickness *= self.scale(WAVE_THICKNESS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::sections::Section;
    use crate::render::composite::CompositeSettings;
    use crate::render::feedback::FeedbackSettings;
    use crate::render::waves::WaveSettings;

    fn frame() -> Frame {
        let mut fb = FeedbackSettings::OFF;
        (fb.zoom, fb.rotation) = (1.2, 0.3);
        Frame::plain(&fb, &WaveSettings::OFF, &CompositeSettings::OFF)
    }

    fn shaped(energy: f32, amount: f32, f: impl Fn(&mut AudioFeatures)) -> Frame {
        let mut features = AudioFeatures { energy, time: 10.0, ..AudioFeatures::default() };
        f(&mut features);
        let mut frame = frame();
        Shape::new(Some(&features), amount).expect("music").apply(&mut frame);
        frame
    }

    /// At the neutral energy a look is as set; quieter it's calmer and dimmer, louder
    /// livelier; with no dynamics, or no music, it's as set whatever the energy.
    #[test]
    fn the_energy_calms_and_livens_a_look() {
        let plain = frame();
        let neutral = shaped(NEUTRAL, 1.0, |_| {});
        assert!((neutral.intensity - 1.0).abs() < 1e-5 && (neutral.shared[slot("zoom")] - plain.shared[slot("zoom")]).abs() < 1e-5);
        let quiet = shaped(0.0, 1.0, |_| {});
        let loud = shaped(1.0, 1.0, |_| {});
        assert!(quiet.intensity < 0.5 && loud.intensity > 1.2);
        let turn = |f: &Frame| f.shared[slot("rot")];
        assert!(turn(&quiet) < turn(&plain) && turn(&plain) < turn(&loud));
        assert!(quiet.composite.exposure < 1.0 && quiet.shared[slot("zoom")] < plain.shared[slot("zoom")]);
        assert_eq!(shaped(0.0, 0.0, |_| {}), plain);
        assert!(Shape::new(None, 1.0).is_none());
    }

    /// A build-up zooms in and brightens; a drop just now flashes and bursts.
    #[test]
    fn build_ups_tense_and_drops_burst() {
        let plain = shaped(NEUTRAL, 1.0, |_| {});
        let building = shaped(NEUTRAL, 1.0, |f| f.sections.buildup = 1.0);
        assert!(building.shared[slot("zoom")] > plain.shared[slot("zoom")] && building.composite.exposure > plain.composite.exposure);
        let dropped = shaped(NEUTRAL, 1.0, |f| f.sections.last = Some(Section { kind: SectionKind::Drop, time: 10.0 }));
        assert!(dropped.composite.exposure > 1.5 && dropped.intensity > 2.0);
        let later = shaped(NEUTRAL, 1.0, |f| f.sections.last = Some(Section { kind: SectionKind::Drop, time: 5.0 }));
        assert!(later.intensity < 1.01, "the burst is over: {}", later.intensity);
    }
}
