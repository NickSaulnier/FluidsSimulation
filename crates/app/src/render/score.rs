//! Scoring the picture: how a run of frames from a look reads, so weak looks can be found
//! without watching them all (see `audit`), and a random look that comes out dead can be
//! thrown away.
//!
//! Each frame (small, RGBA, as grabbed) is measured on its luma: how much of it is dark,
//! how much near white, its detail (the mean step between neighbouring pixels) and its
//! contrast (the spread between its darkest and brightest few percent); and how much of
//! it is covered: away from its background (the median colour), so a few small things
//! on an empty floor read as empty whatever colour the floor is. Between frames, the motion (the mean change of each pixel). Over the run, whether the
//! motion follows the music: the correlation between each interval's motion and the
//! music's activity in it (the level, and the hits).
//!
//! These are rough: a look that's mostly black with bright filaments is fine, so "dark"
//! only counts against a frame that's almost all black. Each measure becomes a 0..1
//! subscore between a floor (0) and a level that's plenty (1), and the quality is their
//! weighted geometric mean, so any one failing sinks it.
//!
//! **Fidelity**, from full-resolution frames (small ones lose what it looks for); not
//! yet part of the quality, until its levels are judged against looks by eye:
//! - **Banding:** the share of the frame in terraces: runs of one colour at least
//!   `TERRACE_RUN` pixels long, across or down, next to another such run a level or two
//!   away. A smooth gradient on 8 bits without dithering is all terraces; a flat
//!   background has none (no step), nor has a dithered or noisy gradient (no runs).
//! - **Sharpness:** the fine detail (each pixel's luma less its 3 × 3 neighbourhood's
//!   mean) over the same at a quarter of the size, times 4: about 1 for sharp edges,
//!   well under for a soft picture (the fine scale empty), over 1 for noise.
//! - **Shimmer:** between two frames in a row, how much the fine detail changes where
//!   the coarse picture (the neighbourhood means) hardly moves, over how much fine
//!   detail is there: about 0 for a still or smoothly moving picture, toward 1 and over
//!   for detail that flickers from frame to frame (aliasing).

use crate::render::grab::Image;

/// Luma below this is dark, above this near white.
const DARK: f32 = 0.05;
const WHITE: f32 = 0.92;
/// A frame is dead with more than this dark, or this near white, or less detail.
const DEAD_DARK: f32 = 0.97;
const DEAD_WHITE: f32 = 0.6;
const DEAD_DETAIL: f32 = 0.002;
/// A pixel this far from the background colour (in any channel, of 255) covers it.
const COVERED: u8 = 20;
/// Contrast is measured between these shares of the darkest and brightest pixels.
const CONTRAST_TAIL: f32 = 0.02;
/// Below this spread of activity, the music is too even to tell whether the picture
/// follows it.
const EVEN_MUSIC: f32 = 0.04;
/// Banding: a terrace is a run of one colour this long, next to another a step of at
/// most `TERRACE_STEP` levels (of 255, in its largest channel) away.
const TERRACE_RUN: usize = 6;
const TERRACE_STEP: u8 = 2;
/// Shimmer: where the coarse picture changed less than this (luma) between the frames,
/// and the fine detail is at least this, counted; none if under `SHIMMER_SHARE` of the
/// frame is.
const STILL: f32 = 2.0 / 255.0;
const SOME_DETAIL: f32 = 2.0 / 255.0;
const SHIMMER_SHARE: f32 = 0.01;

/// One frame's measures.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Measure {
    /// The shares of pixels dark and near white.
    pub dark: f32,
    pub white: f32,
    /// The mean luma step to the next pixel across and down.
    pub detail: f32,
    /// The mean chroma (largest channel less smallest).
    pub colour: f32,
    /// The share of pixels away from the background, and the luma's spread.
    pub coverage: f32,
    pub contrast: f32,
}

impl Measure {
    pub fn dead(&self) -> bool {
        self.dark > DEAD_DARK || self.white > DEAD_WHITE || self.detail < DEAD_DETAIL
    }
}

/// The luma of each pixel, 0..1 (gamma encoded, as stored).
pub fn luma(image: &Image) -> Vec<f32> {
    image.rgba.as_chunks::<4>().0.iter().map(|p| (0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2])) / 255.0).collect()
}

pub fn measure(image: &Image, luma: &[f32]) -> Measure {
    let [w, h] = [image.size[0] as usize, image.size[1] as usize];
    let n = (w * h).max(1) as f32;
    let dark = luma.iter().filter(|&&y| y < DARK).count() as f32 / n;
    let white = luma.iter().filter(|&&y| y > WHITE).count() as f32 / n;
    let mut steps = 0.0;
    for y in 0..h.saturating_sub(1) {
        for x in 0..w.saturating_sub(1) {
            let i = y * w + x;
            steps += (luma[i + 1] - luma[i]).abs() + (luma[i + w] - luma[i]).abs();
        }
    }
    let detail = steps / (w.saturating_sub(1) * h.saturating_sub(1)).max(1) as f32;
    let pixels = image.rgba.as_chunks::<4>().0;
    let colour = pixels.iter().map(|p| f32::from(p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]))).sum::<f32>() / (255.0 * n);
    // The background: each channel's median.
    let mut background = [0u8; 3];
    for (c, b) in background.iter_mut().enumerate() {
        let mut counts = [0usize; 256];
        for p in pixels {
            counts[usize::from(p[c])] += 1;
        }
        *b = percentile(&counts, 0.5);
    }
    let covered = pixels.iter().filter(|p| (0..3).any(|c| p[c].abs_diff(background[c]) > COVERED)).count();
    let mut counts = [0usize; 256];
    for &y in luma {
        counts[(y * 255.0).round().clamp(0.0, 255.0) as usize] += 1;
    }
    let contrast = f32::from(percentile(&counts, 1.0 - CONTRAST_TAIL) - percentile(&counts, CONTRAST_TAIL)) / 255.0;
    Measure { dark, white, detail, colour, coverage: covered as f32 / n, contrast }
}

/// The value below which `share` of a histogram's counts lie.
fn percentile(counts: &[usize; 256], share: f32) -> u8 {
    let target = (share * counts.iter().sum::<usize>() as f32) as usize;
    let mut sum = 0;
    for (v, &c) in counts.iter().enumerate() {
        sum += c;
        if sum > target {
            return v as u8;
        }
    }
    255
}

/// A full-resolution frame's banding and sharpness (see the module docs).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Fidelity {
    pub banding: f32,
    pub sharpness: f32,
}

pub fn fidelity(image: &Image) -> Fidelity {
    let [w, h] = [image.size[0] as usize, image.size[1] as usize];
    let pixels = image.rgba.as_chunks::<4>().0;
    // Banding: terraced pixels, across and down.
    let mut terraced = vec![false; w * h];
    let step = |a: &[u8; 4], b: &[u8; 4]| (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0);
    let mut line = |at: &dyn Fn(usize) -> usize, len: usize| {
        // Runs of one colour along the line: (start, length).
        let mut runs: Vec<(usize, usize)> = Vec::new();
        for k in 0..len {
            match runs.last_mut() {
                Some((start, n)) if pixels[at(*start)][..3] == pixels[at(k)][..3] => *n += 1,
                _ => runs.push((k, 1)),
            }
        }
        for pair in runs.windows(2) {
            let ((a, n), (b, m)) = (pair[0], pair[1]);
            let d = step(&pixels[at(a)], &pixels[at(b)]);
            if n >= TERRACE_RUN && m >= TERRACE_RUN && (1..=TERRACE_STEP).contains(&d) {
                for k in (a..a + n).chain(b..b + m) {
                    terraced[at(k)] = true;
                }
            }
        }
    };
    for y in 0..h {
        line(&|x| y * w + x, w);
    }
    for x in 0..w {
        line(&|y| y * w + x, h);
    }
    let banding = terraced.iter().filter(|&&t| t).count() as f32 / (w * h).max(1) as f32;
    // Sharpness: fine detail here against a quarter of the size.
    let y = luma(image);
    let quarter = shrink(&y, w, h, 4);
    let fine = |l: &[f32], w: usize, h: usize| {
        let mean = box3(l, w, h);
        l.iter().zip(&mean).map(|(a, b)| (a - b).abs()).sum::<f32>() / l.len().max(1) as f32
    };
    let coarse = fine(&quarter, w / 4, h / 4);
    let sharpness = if coarse > 1e-5 { 4.0 * fine(&y, w, h) / coarse } else { 0.0 };
    Fidelity { banding, sharpness }
}

/// How much the fine detail flickers between two frames in a row of the same size (see
/// the module docs); none if too little of the picture holds still with detail in it.
pub fn shimmer(a: &Image, b: &Image) -> Option<f32> {
    if a.size != b.size {
        return None;
    }
    let [w, h] = [a.size[0] as usize, a.size[1] as usize];
    let (ya, yb) = (luma(a), luma(b));
    let (ma, mb) = (box3(&ya, w, h), box3(&yb, w, h));
    let (mut change, mut detail, mut counted) = (0.0, 0.0, 0usize);
    for i in 0..w * h {
        let (fa, fb) = (ya[i] - ma[i], yb[i] - mb[i]);
        if (ma[i] - mb[i]).abs() < STILL && fa.abs() >= SOME_DETAIL {
            change += (fb - fa).abs();
            detail += fa.abs();
            counted += 1;
        }
    }
    (counted as f32 >= SHIMMER_SHARE * (w * h) as f32 && detail > 0.0).then(|| change / detail)
}

/// Each pixel's 3 × 3 neighbourhood's mean (clamped at the edges).
fn box3(l: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut across = vec![0.0; l.len()];
    for y in 0..h {
        for x in 0..w {
            let row = &l[y * w..(y + 1) * w];
            across[y * w + x] = (row[x.saturating_sub(1)] + row[x] + row[(x + 1).min(w - 1)]) / 3.0;
        }
    }
    let mut out = vec![0.0; l.len()];
    for y in 0..h {
        for x in 0..w {
            out[y * w + x] = (across[y.saturating_sub(1) * w + x] + across[y * w + x] + across[(y + 1).min(h - 1) * w + x]) / 3.0;
        }
    }
    out
}

/// `l` (`w` × `h`) shrunk by `k` each way, each pixel the mean of its block.
fn shrink(l: &[f32], w: usize, h: usize, k: usize) -> Vec<f32> {
    let (sw, sh) = (w / k, h / k);
    let mut out = vec![0.0; sw * sh];
    for y in 0..sh * k {
        for x in 0..sw * k {
            out[(y / k) * sw + x / k] += l[y * w + x];
        }
    }
    out.iter_mut().for_each(|v| *v /= (k * k) as f32);
    out
}

/// The mean change of each pixel's luma between two frames of the same size.
pub fn motion(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum::<f32>() / a.len().max(1) as f32
}

/// A look's score over a run.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Score {
    /// 0..100: the subscores' weighted geometric mean.
    pub quality: f32,
    /// Means over the frames.
    pub dark: f32,
    pub white: f32,
    pub detail: f32,
    pub colour: f32,
    #[serde(default)]
    pub coverage: f32,
    #[serde(default)]
    pub contrast: f32,
    /// Mean motion between frames, and its correlation with the music's activity (none
    /// when the music was too even to tell).
    pub motion: f32,
    pub following: Option<f32>,
    /// The share of frames that were dead, and the frames a second.
    pub dead: f32,
    pub fps: f32,
    /// Fidelity, from full-resolution frames (see the module docs): means over them;
    /// none without any (shimmer: without a pair to tell).
    #[serde(default)]
    pub banding: Option<f32>,
    #[serde(default)]
    pub sharpness: Option<f32>,
    #[serde(default)]
    pub shimmer: Option<f32>,
}

/// 0 at `lo`, 1 at `hi` (either way round), smooth between.
fn ramp(lo: f32, hi: f32, x: f32) -> f32 {
    let t = ((x - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Score {
    /// The subscores, 0..1, with their names, weights and floors. The softer measures
    /// (the frame rate, and following the music, which a steady motion fails) have a
    /// floor, so failing one lowers the quality without sinking it.
    pub fn parts(&self) -> Vec<(&'static str, f32, f32, f32)> {
        let mut parts = vec![
            ("lit", ramp(0.97, 0.8, self.dark), 1.0, 0.0),
            ("not blown out", ramp(0.5, 0.15, self.white), 1.0, 0.0),
            ("detail", ramp(0.003, 0.012, self.detail), 1.0, 0.0),
            ("filled", ramp(0.08, 0.3, self.coverage), 1.0, 0.0),
            ("contrast", ramp(0.1, 0.4, self.contrast), 1.0, 0.0),
            ("moving", ramp(0.003, 0.015, self.motion), 1.0, 0.0),
            ("alive", 1.0 - self.dead, 1.0, 0.0),
            ("smooth", ramp(20.0, 45.0, self.fps), 0.5, 0.4),
        ];
        if let Some(f) = self.following {
            parts.push(("following", ramp(-0.1, 0.35, f), 0.5, 0.4));
        }
        parts
    }

    /// What's wrong with it: the subscores under a half.
    pub fn problems(&self) -> Vec<&'static str> {
        self.parts().into_iter().filter(|p| p.1 < 0.5).map(|p| p.0).collect()
    }

    /// The quality worked out again from the measures (after the subscores changed).
    pub fn with_quality(mut self) -> Self {
        let parts = self.parts();
        let weights: f32 = parts.iter().map(|p| p.2).sum();
        let log: f32 = parts.iter().map(|p| p.2 * (p.3 + (1.0 - p.3) * p.1).max(1e-3).ln()).sum();
        self.quality = 100.0 * (log / weights).exp();
        self
    }
}

/// A run of frames being scored, in the order grabbed.
#[derive(Default)]
pub struct Run {
    measures: Vec<Measure>,
    /// Full-resolution frames' fidelity, and the shimmer of pairs of them.
    fidelity: Vec<Fidelity>,
    shimmers: Vec<f32>,
    /// Motion since the frame before, and the music's activity over that time.
    moves: Vec<(f32, f32)>,
    last: Option<Vec<f32>>,
}

impl Run {
    /// Add the next frame; `activity`: the music's since the last one (0..about 2).
    pub fn push(&mut self, image: &Image, activity: f32) {
        let luma = luma(image);
        self.measures.push(measure(image, &luma));
        if let Some(last) = self.last.as_ref().filter(|l| l.len() == luma.len()) {
            self.moves.push((motion(last, &luma), activity));
        }
        self.last = Some(luma);
    }

    pub fn len(&self) -> usize {
        self.measures.len()
    }

    /// Add a full-resolution frame's fidelity, and its shimmer against the frame after
    /// it (if that was grabbed and could tell).
    pub fn push_fidelity(&mut self, fidelity: Fidelity, shimmer: Option<f32>) {
        self.fidelity.push(fidelity);
        self.shimmers.extend(shimmer);
    }

    pub fn score(&self, fps: f32) -> Score {
        let n = self.measures.len().max(1) as f32;
        let mean = |f: fn(&Measure) -> f32| self.measures.iter().map(f).sum::<f32>() / n;
        let moves = self.moves.len().max(1) as f32;
        Score {
            quality: 0.0,
            dark: mean(|m| m.dark),
            white: mean(|m| m.white),
            detail: mean(|m| m.detail),
            colour: mean(|m| m.colour),
            coverage: mean(|m| m.coverage),
            contrast: mean(|m| m.contrast),
            motion: self.moves.iter().map(|m| m.0).sum::<f32>() / moves,
            following: correlation(&self.moves),
            dead: self.measures.iter().filter(|m| m.dead()).count() as f32 / n,
            fps,
            banding: average(self.fidelity.iter().map(|f| f.banding)),
            sharpness: average(self.fidelity.iter().map(|f| f.sharpness)),
            shimmer: average(self.shimmers.iter().copied()),
        }
        .with_quality()
    }
}

/// Watching a new random look's first seconds, to throw it away if it came out dead or
/// weak: `CHECK_FRAMES` small frames from `CHECK_FROM_S` (once the blend is over and the
/// fluid has had time to fill), scored as a run (at a frame rate that doesn't count),
/// thrown away under `CHECK_QUALITY`. The weakest built-in look scores about 70.
pub struct Check {
    /// The look's name, and how many looks were thrown away before it.
    pub look: String,
    pub rolls: u32,
    t: f32,
    asked: usize,
    run: Run,
}

pub const CHECK_SIZE: [u32; 2] = [128, 72];
const CHECK_FROM_S: f32 = 2.5;
const CHECK_EVERY_S: f32 = 0.5;
const CHECK_FRAMES: usize = 5;
const CHECK_QUALITY: f32 = 40.0;
/// At most this many looks in a row are thrown away.
pub const MAX_ROLLS: u32 = 3;

impl Check {
    pub fn new(look: String, rolls: u32) -> Self {
        Self { look, rolls, t: 0.0, asked: 0, run: Run::default() }
    }

    /// Advance `dt` seconds (while the look is on screen and the music playing);
    /// whether to grab a frame now.
    pub fn tick(&mut self, dt: f32) -> bool {
        self.t += dt;
        let due = self.asked < CHECK_FRAMES && self.t >= CHECK_FROM_S + self.asked as f32 * CHECK_EVERY_S;
        if due {
            self.asked += 1;
        }
        due
    }

    /// A grab couldn't be made: ask again next frame.
    pub fn refused(&mut self) {
        self.asked -= 1;
    }

    pub fn receive(&mut self, image: &Image) {
        self.run.push(image, 0.0);
    }

    /// Whether to throw the look away, once every frame is in; and its score.
    pub fn verdict(&self) -> Option<(bool, f32)> {
        (self.run.len() >= CHECK_FRAMES).then(|| {
            let quality = self.run.score(60.0).quality;
            (quality < CHECK_QUALITY, quality)
        })
    }
}

/// The mean, if there are any.
fn average(values: impl Iterator<Item = f32>) -> Option<f32> {
    let (sum, n) = values.fold((0.0, 0usize), |(s, n), v| (s + v, n + 1));
    (n > 0).then(|| sum / n as f32)
}

/// The Pearson correlation of the pairs; none if the second varies too little to say.
fn correlation(pairs: &[(f32, f32)]) -> Option<f32> {
    if pairs.len() < 8 {
        return None;
    }
    let n = pairs.len() as f32;
    let (mx, my) = (pairs.iter().map(|p| p.0).sum::<f32>() / n, pairs.iter().map(|p| p.1).sum::<f32>() / n);
    let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
    for &(x, y) in pairs {
        sxy += (x - mx) * (y - my);
        sxx += (x - mx) * (x - mx);
        syy += (y - my) * (y - my);
    }
    if (syy / n).sqrt() < EVEN_MUSIC {
        return None;
    }
    Some(if sxx > 0.0 { sxy / (sxx * syy).sqrt() } else { 0.0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(f: impl Fn(u32, u32) -> [u8; 3]) -> Image {
        let size = [64, 36];
        let rgba = (0..size[1]).flat_map(|y| (0..size[0]).map(move |x| (x, y))).flat_map(|(x, y)| {
            let [r, g, b] = f(x, y);
            [r, g, b, 255]
        });
        Image { size, rgba: rgba.collect() }
    }

    fn measured(image: &Image) -> Measure {
        measure(image, &luma(image))
    }

    #[test]
    fn black_white_and_flat_frames_are_dead() {
        assert!(measured(&image(|_, _| [0, 0, 0])).dead());
        assert!(measured(&image(|_, _| [255, 255, 255])).dead());
        assert!(measured(&image(|_, _| [90, 40, 160])).dead(), "one flat colour");
        // Mostly black with bright filaments is a picture.
        let filaments = image(|x, y| if (x + 2 * y) % 9 == 0 { [255, 200, 90] } else { [0, 0, 0] });
        let m = measured(&filaments);
        assert!(!m.dead() && m.dark > 0.8 && m.detail > 0.02, "{m:?}");
    }

    #[test]
    fn a_few_things_on_an_empty_floor_cover_little() {
        // Grey with three small squares: whatever the floor's colour, mostly empty.
        let few = measured(&image(|x, y| if y > 28 && x % 20 < 5 { [200, 60, 90] } else { [28, 30, 38] }));
        assert!(few.coverage < 0.1 && few.dark == 0.0, "{few:?}");
        let full = measured(&image(|x, y| [(x * 4) as u8, (y * 7) as u8, 128]));
        assert!(full.coverage > 0.6 && full.contrast > 0.4, "{full:?}");
        // A murky picture: everything within a narrow band of greys.
        let murky = measured(&image(|x, y| { let v = 40 + ((x + y) % 12) as u8; [v, v, v + 10] }));
        assert!(murky.contrast < 0.06, "{murky:?}");
    }

    #[test]
    fn a_moving_picture_that_follows_the_music_scores_well() {
        // Stripes that move further when the music is busier.
        let mut good = Run::default();
        let mut still = Run::default();
        let mut offset = 0u32;
        for k in 0..40 {
            let activity = if k % 4 == 0 { 1.0 } else { 0.2 };
            offset += if activity > 0.5 { 3 } else { 1 };
            good.push(&image(|x, y| if (x + y + offset) % 8 < 4 { [230, 120, 40] } else { [20, 10, 60] }), activity);
            still.push(&image(|x, y| if (x + y) % 8 < 4 { [230, 120, 40] } else { [20, 10, 60] }), activity);
        }
        let (good, still) = (good.score(60.0), still.score(60.0));
        assert!(good.following.is_some_and(|f| f > 0.5), "{good:?}");
        assert!(good.quality > 80.0 && good.problems().is_empty(), "{good:?}");
        assert!(still.quality < 50.0 && still.problems().contains(&"moving"), "{still:?}");
    }

    #[test]
    fn a_check_throws_away_a_dead_look() {
        let mut dead = Check::new("a".into(), 0);
        let mut fine = Check::new("b".into(), 0);
        let mut grabs = 0u32;
        for _ in 0..400 {
            if dead.tick(1.0 / 60.0) {
                dead.receive(&image(|_, _| [0, 0, 0]));
            }
            if fine.tick(1.0 / 60.0) {
                grabs += 1;
                fine.receive(&image(|x, y| if (x + y + 3 * grabs) % 8 < 4 { [230, 120, 40] } else { [20, 10, 60] }));
            }
        }
        assert_eq!(grabs as usize, CHECK_FRAMES);
        assert!(dead.verdict().is_some_and(|v| v.0), "{:?}", dead.verdict());
        assert!(fine.verdict().is_some_and(|v| !v.0), "{:?}", fine.verdict());
    }

    fn full(f: impl Fn(u32, u32) -> [u8; 3]) -> Image {
        let size = [256, 144];
        let rgba = (0..size[1]).flat_map(|y| (0..size[0]).map(move |x| (x, y))).flat_map(|(x, y)| {
            let [r, g, b] = f(x, y);
            [r, g, b, 255]
        });
        Image { size, rgba: rgba.collect() }
    }

    /// A dark gradient on 8 bits bands; dithered, or flat, it doesn't.
    #[test]
    fn banding_finds_terraces() {
        let level = |x: u32| 20.0 + 16.0 * x as f32 / 256.0;
        let banded = fidelity(&full(|x, _| [level(x) as u8; 3])).banding;
        let mut rng = utils::rng::SplitMix64::new(1);
        let noise: Vec<f32> = (0..256 * 144).map(|_| rng.next_f32()).collect();
        let dithered = fidelity(&full(|x, y| [(level(x) + noise[(y * 256 + x) as usize]) as u8; 3])).banding;
        let flat = fidelity(&full(|_, _| [0, 0, 0])).banding;
        assert!(banded > 0.9 && dithered < 0.2 && flat == 0.0, "{banded} {dithered} {flat}");
    }

    /// Sharp edges read about 1, the same blurred well under, noise over.
    #[test]
    fn sharpness_tells_sharp_from_soft() {
        let squares = |x: u32, y: u32| if (x / 16 + y / 16).is_multiple_of(2) { 220.0 } else { 30.0 };
        let sharp = fidelity(&full(|x, y| [squares(x, y) as u8; 3])).sharpness;
        let blurred = |x: u32, y: u32| {
            let mut sum = 0.0;
            for dy in -3i32..=3 {
                for dx in -3i32..=3 {
                    sum += squares((x as i32 + dx).max(0) as u32, (y as i32 + dy).max(0) as u32);
                }
            }
            sum / 49.0
        };
        let soft = fidelity(&full(|x, y| [blurred(x, y) as u8; 3])).sharpness;
        let mut rng = utils::rng::SplitMix64::new(2);
        let values: Vec<u8> = (0..256 * 144).map(|_| (rng.next_f32() * 255.0) as u8).collect();
        let noise = fidelity(&full(|x, y| [values[(y * 256 + x) as usize]; 3])).sharpness;
        assert!((0.6..1.6).contains(&sharp) && soft < 0.5 * sharp && noise > 1.5, "{sharp} {soft} {noise}");
    }

    /// Detail that flickers between frames shimmers; the same frame twice, or a smooth
    /// pattern moving a fraction of a pixel, doesn't.
    #[test]
    fn shimmer_finds_flickering_detail() {
        let mut rng = utils::rng::SplitMix64::new(3);
        let (a, b): (Vec<u8>, Vec<u8>) = (0..256 * 144).map(|_| ((rng.next_f32() * 255.0) as u8, (rng.next_f32() * 255.0) as u8)).unzip();
        let flicker = shimmer(&full(|x, y| [a[(y * 256 + x) as usize]; 3]), &full(|x, y| [b[(y * 256 + x) as usize]; 3]));
        let still = full(|x, y| [a[(y * 256 + x) as usize]; 3]);
        let wave = |shift: f32| full(move |x, y| [(128.0 + 100.0 * ((x as f32 + shift) * 0.4).sin() * (y as f32 * 0.3).cos()) as u8; 3]);
        let smooth = shimmer(&wave(0.0), &wave(0.1));
        assert!(flicker.is_some_and(|s| s > 0.8), "{flicker:?}");
        assert_eq!(shimmer(&still, &still), Some(0.0));
        assert!(smooth.is_some_and(|s| s < 0.3), "{smooth:?}");
    }

    #[test]
    fn a_dead_run_scores_low() {
        let mut run = Run::default();
        for k in 0..20 {
            run.push(&image(|_, _| [0, 0, (k % 3) as u8]), 0.5);
        }
        let s = run.score(60.0);
        assert!(s.quality < 5.0 && s.dead == 1.0 && s.following.is_none(), "{s:?}");
    }
}
