//! Sections of a song: where it changes (a drop, a breakdown, or just something new), and
//! whether it's building up to one. Fed one hop at a time by the analyzer (see
//! `analyzer`), for the director (changing looks on the music's phrases) and equations.
//!
//! The music is summarised in blocks of `BLOCK_S`: its timbre (the filter bank's
//! levels in `GROUPS` groups, in dB, relative to their own mean, so loudness doesn't
//! count twice), its loudness and low end (dB), and its onset density.
//! - **Novelty:** the last `SHORT_S` against the `LONG_S` before it. Each timbre group's
//!   and the loudness's change is measured against how much it varies anyway over the
//!   long window (a t-statistic): steady pads make small changes count, busy drums make
//!   fills count less. It's about 1 for music carrying on, several at a change.
//! - **Changes:** novelty peaks of at least `CHANGE`, `MIN_GAP_S` apart. One whose
//!   loudness or low end falls away is a breakdown, anything else a plain change; but
//!   not while building up (`BUILDING`), when the music keeps changing until it drops.
//! - **Drops:** the loudness and the low end jumping together, with the low end a large
//!   share of the mix, reported as soon as they do (not waiting for a novelty peak: at the
//!   end of a build-up novelty is already high).
//! - **Build-up:** loudness and onset density rising steadily over the last `LONG_S`
//!   (the slope of a line through them, times how well the line fits), 0..1.
//!
//! In silence (song gaps) nothing is compared: a new song isn't a section change.

/// Block length, and the windows compared, seconds.
const BLOCK_S: f32 = 0.25;
const SHORT_S: f32 = 2.0;
const LONG_S: f32 = 8.0;
/// Blocks ignored at the start (the analyzer warming up is quieter than the music).
const WARM_UP_BLOCKS: usize = 4;
/// Timbre groups (of the filter bank's 64 bands).
const GROUPS: usize = 16;
/// Novelty for a change, and the least time between changes (and before a drop), seconds.
const CHANGE: f32 = 3.0;
const MIN_GAP_S: f32 = 4.0;
/// Building up this much, plain changes aren't reported.
const BUILDING: f32 = 0.3;
const DROP_GAP_S: f32 = 2.0;
/// The least spread counted, dB (so a perfectly steady sound doesn't make every change
/// infinite).
const SPREAD_FLOOR_DB: f32 = 0.5;
/// A drop: louder by this (dB), the low end up by this (dB) and within this of the whole
/// (dB, its share of the mix).
const DROP_LOUDER_DB: f32 = 3.0;
const DROP_LOW_DB: f32 = 6.0;
const DROP_LOW_SHARE_DB: f32 = -6.0;
/// A breakdown: quieter by this, or the low end down by this (dB).
const BREAKDOWN_QUIETER_DB: f32 = -4.0;
const BREAKDOWN_LOW_DB: f32 = -8.0;
/// A build-up at its fullest: loudness rising this fast (dB/s), onset density this fast
/// (a fraction of itself per second).
const BUILD_LOUDER_DB_S: f32 = 0.8;
const BUILD_DENSER_S: f32 = 0.12;
/// Blocks quieter than this (dB of mean-square amplitude) count as silence.
const SILENT_DB: f32 = -60.0;
/// Bands centred below this are the low end.
pub const LOW_HZ: f32 = 150.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectionKind {
    /// Louder, with the low end back: the beat drops.
    Drop,
    /// Quieter, or the low end gone.
    Breakdown,
    /// Something else new.
    Change,
}

/// A section change.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Section {
    pub kind: SectionKind,
    /// When the change happened (as far as can be told), seconds since the analyzer
    /// started; it's reported up to `SHORT_S` later.
    pub time: f32,
}

/// The song's sections as of the latest hop.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sections {
    /// How different the last seconds are from the ones before: about 1 for music
    /// carrying on, `CHANGE` or more at a change.
    pub novelty: f32,
    /// How much the music is building up, 0..1.
    pub buildup: f32,
    /// Changes so far, and the latest.
    pub changes: u32,
    pub last: Option<Section>,
}

#[derive(Clone, Copy, Default)]
struct Block {
    timbre: [f32; GROUPS],
    loudness: f32,
    low: f32,
    density: f32,
}

pub struct SectionTracker {
    hops_per_block: usize,
    /// The block being summed: hops so far, and the sums (energies, then onset strength).
    hops: usize,
    groups: [f32; GROUPS],
    total: f32,
    low: f32,
    strength: f32,
    /// The last `SHORT_S + LONG_S` of blocks, a ring (next write at `pos`), how many of
    /// it are filled, and blocks seen in all.
    blocks: Vec<Block>,
    pos: usize,
    len: usize,
    seen: usize,
    /// Novelty of the last two blocks (for peaks); whether the last block looked like a
    /// drop; when the last change was.
    recent: [f32; 2],
    dropping: bool,
    last_change: f32,
    time: f32,
    state: Sections,
}

impl SectionTracker {
    pub fn new(hop_s: f32) -> Self {
        let cap = ((SHORT_S + LONG_S) / BLOCK_S).round() as usize;
        Self {
            hops_per_block: ((BLOCK_S / hop_s).round() as usize).max(1),
            hops: 0,
            groups: [0.0; GROUPS],
            total: 0.0,
            low: 0.0,
            strength: 0.0,
            blocks: vec![Block::default(); cap],
            pos: 0,
            len: 0,
            seen: 0,
            recent: [0.0; 2],
            dropping: false,
            last_change: f32::NEG_INFINITY,
            time: 0.0,
            state: Sections::default(),
        }
    }

    pub fn state(&self) -> Sections {
        self.state
    }

    /// Feed one hop: its time, the filter bank's energies (`low_bands` of them the low
    /// end), the loudness as heard (mean-square amplitude) and the onset strength.
    pub fn push(&mut self, time: f32, bank: &[f32], low_bands: usize, heard: f32, strength: f32) {
        self.time = time;
        let per = bank.len().div_ceil(GROUPS);
        for (g, chunk) in bank.chunks(per).enumerate().take(GROUPS) {
            self.groups[g] += chunk.iter().sum::<f32>();
        }
        self.total += heard;
        self.low += bank[..low_bands.min(bank.len())].iter().sum::<f32>();
        self.strength += strength;
        self.hops += 1;
        if self.hops < self.hops_per_block {
            return;
        }
        let n = self.hops as f32;
        let db = |e: f32| 10.0 * (e / n + 1e-12).log10();
        let mut timbre = self.groups.map(db);
        let mean = timbre.iter().sum::<f32>() / GROUPS as f32;
        for t in &mut timbre {
            *t -= mean;
        }
        let block = Block { timbre, loudness: db(self.total), low: db(self.low), density: self.strength / n };
        (self.groups, self.total, self.low, self.strength, self.hops) = ([0.0; GROUPS], 0.0, 0.0, 0.0, 0);
        self.seen += 1;
        if self.seen <= WARM_UP_BLOCKS {
            return;
        }
        let cap = self.blocks.len();
        self.blocks[self.pos] = block;
        self.pos = (self.pos + 1) % cap;
        self.len = (self.len + 1).min(cap);
        self.update();
    }

    /// Block `back` blocks before the newest (0: the newest).
    fn block(&self, back: usize) -> &Block {
        let cap = self.blocks.len();
        &self.blocks[(self.pos + cap - 1 - back) % cap]
    }

    /// The mean and spread (standard deviation) of `value` over `count` blocks from
    /// `from` back.
    fn stats(&self, from: usize, count: usize, value: impl Fn(&Block) -> f32) -> (f32, f32) {
        let (mut sum, mut squares) = (0.0f32, 0.0f32);
        let origin = value(self.block(from));
        for k in from..from + count {
            let v = value(self.block(k)) - origin;
            sum += v;
            squares += v * v;
        }
        let mean = sum / count as f32;
        (origin + mean, (squares / count as f32 - mean * mean).max(0.0).sqrt())
    }

    fn update(&mut self) {
        let short = (SHORT_S / BLOCK_S).round() as usize;
        let long = self.blocks.len() - short;
        if self.len < self.blocks.len() || (0..self.len).any(|k| self.block(k).loudness < SILENT_DB) {
            // Not enough music yet, or a gap in it: nothing to compare.
            (self.state.novelty, self.state.buildup, self.recent, self.dropping) = (0.0, 0.0, [0.0; 2], false);
            return;
        }
        // Each change against how much that thing varies anyway (the t-statistic of the
        // two windows' means).
        let scale = (1.0 / short as f32 + 1.0 / long as f32).sqrt();
        let contrast = |value: &dyn Fn(&Block) -> f32| {
            let ((now, _), (before, spread)) = (self.stats(0, short, value), self.stats(short, long, value));
            (now - before, (now - before) / ((spread + SPREAD_FLOOR_DB) * scale))
        };
        let timbre = ((0..GROUPS).map(|g| contrast(&|b: &Block| b.timbre[g]).1.powi(2)).sum::<f32>() / GROUPS as f32).sqrt();
        let (louder, loud_z) = contrast(&|b: &Block| b.loudness);
        let (lower, _) = contrast(&|b: &Block| b.low);
        let novelty = 0.5 * (timbre + loud_z.abs());
        let (now_loudness, _) = self.stats(0, short, |b| b.loudness);
        let (now_low, _) = self.stats(0, short, |b| b.low);
        let since = self.time - self.last_change;

        // A drop, as soon as it shows.
        let dropping = louder >= DROP_LOUDER_DB && lower >= DROP_LOW_DB && now_low - now_loudness >= DROP_LOW_SHARE_DB;
        let mut found = None;
        if dropping && !self.dropping && since >= DROP_GAP_S {
            found = Some(SectionKind::Drop);
        }
        // Other changes: the previous block's novelty was a peak, high enough, far enough
        // from the last change.
        let [two_back, one_back] = self.recent;
        if found.is_none() && !dropping && one_back >= CHANGE && one_back >= two_back && one_back > novelty && since >= MIN_GAP_S {
            let quieter = louder <= BREAKDOWN_QUIETER_DB || lower <= BREAKDOWN_LOW_DB;
            if quieter {
                found = Some(SectionKind::Breakdown);
            } else if self.state.buildup < BUILDING {
                found = Some(SectionKind::Change);
            }
        }
        if let Some(kind) = found {
            self.last_change = self.time;
            self.state.changes += 1;
            // Drops are caught as they start to show; other changes once the short window
            // is past them.
            let back = if kind == SectionKind::Drop { 0.5 * SHORT_S } else { SHORT_S };
            self.state.last = Some(Section { kind, time: self.time - back });
        }
        self.dropping = dropping;
        self.recent = [one_back, novelty];
        self.state.novelty = novelty;
        self.state.buildup = self.buildup();
    }

    /// The build-up: loudness and onset density rising along a line over the last
    /// `LONG_S`, each scaled to its fullest rate and weighted by how well the line fits.
    fn buildup(&self) -> f32 {
        let n = (LONG_S / BLOCK_S).round() as usize;
        let fit = |value: &dyn Fn(&Block) -> f32| -> (f32, f32, f32) {
            // Least squares over time (x in seconds, oldest 0); returns the slope per
            // second, R² and the mean. Values are taken from the newest, so the sums stay
            // small (f32).
            let origin = value(self.block(0));
            let (mut sx, mut sy, mut sxx, mut sxy, mut syy) = (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32);
            for k in 0..n {
                let (x, y) = ((n - 1 - k) as f32 * BLOCK_S, value(self.block(k)) - origin);
                (sx, sy, sxx, sxy, syy) = (sx + x, sy + y, sxx + x * x, sxy + x * y, syy + y * y);
            }
            let m = n as f32;
            let (vxx, vxy, vyy) = (sxx - sx * sx / m, sxy - sx * sy / m, syy - sy * sy / m);
            let slope = vxy / vxx;
            let r2 = if vyy > 1e-9 { (vxy * vxy / (vxx * vyy)).clamp(0.0, 1.0) } else { 0.0 };
            (slope, r2, origin + sy / m)
        };
        let (loud_slope, loud_fit, _) = fit(&|b| b.loudness);
        let (dense_slope, dense_fit, dense_mean) = fit(&|b| b.density);
        let louder = (loud_slope / BUILD_LOUDER_DB_S).clamp(0.0, 1.0) * loud_fit;
        let denser = if dense_mean > 1e-6 { (dense_slope / dense_mean / BUILD_DENSER_S).clamp(0.0, 1.0) * dense_fit } else { 0.0 };
        (0.5 * (louder + denser)).clamp(0.0, 1.0)
    }
}
