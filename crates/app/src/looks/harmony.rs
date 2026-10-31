//! A look's colour following the harmony (see `audio::harmony`), as much as its
//! `harmony` says (0, the default, leaves it as set): the picture's hue turns with the
//! chords, a whole turn for a whole round of the circle of fifths at 1 (a change of
//! chord a small step, of key a bigger one), from where it was when the look began, so
//! a look starts in its own colours; and its saturation and glow follow the sound's
//! timbre, richer and brighter for a bright sound. Applied after the song's shape.

use super::runtime::Frame;
use crate::audio::analyzer::AudioFeatures;

/// At a fully dull and a fully bright sound, the saturation and the exposure this
/// share less and more (at amount 1).
const SATURATION: f32 = 0.3;
const GLOW: f32 = 0.12;

/// The harmony's colour for the look playing: where the chords' hue stood when it
/// began. Made afresh for each look.
#[derive(Clone, Copy, Debug, Default)]
pub struct Colour {
    start: Option<f32>,
}

impl Colour {
    pub fn apply(&mut self, frame: &mut Frame, features: Option<&AudioFeatures>, amount: f32) {
        let Some(f) = features else { return };
        let h = f.harmony;
        let start = *self.start.get_or_insert(h.hue);
        let amount = amount.clamp(0.0, 1.0);
        if amount == 0.0 {
            return;
        }
        let c = &mut frame.composite;
        c.hue += amount * 360.0 * (h.hue - start);
        let timbre = 2.0 * h.timbre - 1.0;
        c.saturation *= 1.0 + amount * SATURATION * timbre;
        c.exposure *= 1.0 + amount * GLOW * timbre;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::composite::CompositeSettings;
    use crate::render::feedback::FeedbackSettings;
    use crate::render::waves::WaveSettings;

    fn coloured(colour: &mut Colour, hue: f32, timbre: f32, amount: f32) -> CompositeSettings {
        let mut f = AudioFeatures::default();
        (f.harmony.hue, f.harmony.timbre) = (hue, timbre);
        let mut frame = Frame::plain(&FeedbackSettings::OFF, &WaveSettings::OFF, &CompositeSettings::OFF);
        colour.apply(&mut frame, Some(&f), amount);
        frame.composite
    }

    /// A look starts in its own colours, then turns with the chords (a fifth up, a
    /// twelfth of a turn); a bright sound is richer; at 0 nothing changes.
    #[test]
    fn the_hue_follows_the_chords_from_where_the_look_began() {
        let plain = CompositeSettings::OFF;
        let mut colour = Colour::default();
        let first = coloured(&mut colour, 0.4, 0.5, 1.0);
        assert!(first.hue == plain.hue && first.saturation == plain.saturation);
        let fifth = coloured(&mut colour, 0.4 + 1.0 / 12.0, 1.0, 1.0);
        assert!((fifth.hue - plain.hue - 30.0).abs() < 1e-3 && fifth.saturation > plain.saturation && fifth.exposure > plain.exposure);
        let half = coloured(&mut colour, 0.4 + 1.0 / 12.0, 0.0, 0.5);
        assert!((half.hue - plain.hue - 15.0).abs() < 1e-3 && half.saturation < plain.saturation);
        assert_eq!(coloured(&mut Colour::default(), 0.9, 1.0, 0.0), plain);
    }
}
