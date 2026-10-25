//! Measuring the analyzer: hit detection scored against known hit times, on the
//! generated songs (`songs`) and on any annotated clips in `tests/fixtures/audio`, with
//! a baseline so a change that hears worse is caught (`hit_detection_holds_its_baseline`).
//!
//! The scoring is the usual one for onset detection: a found hit matches a true hit of
//! the same kind within `TOLERANCE_S`, each at most once; precision is the share of found
//! hits that match, recall the share of true hits found, and the F-measure their harmonic
//! mean. The first `WARM_UP_S` seconds aren't scored (the detector needs context).
//!
//! Run `cargo test --release -p app hit_detection -- --nocapture` to see the table;
//! set `UPDATE_ANALYZER_BASELINE=1` to record new scores as the baseline.

use super::analyzer::{Analyzer, Frame, Onset, OnsetKind};
use super::live::to_stereo;
use super::songs;
use std::path::{Path, PathBuf};

/// How close a found hit must be to a true one, seconds.
pub const TOLERANCE_S: f32 = 0.05;
const WARM_UP_S: f32 = 1.5;
const SENSITIVITY: f32 = 1.5;
/// Audio is fed in chunks of this many frames, like a frame loop would.
const CHUNK: usize = 1024;
/// A score may fall this far below its baseline before the test fails (small changes
/// shift a hit or two).
const SLACK: f32 = 0.02;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Score {
    pub matched: usize,
    pub found: usize,
    pub truth: usize,
}

impl Score {
    pub fn precision(&self) -> f32 {
        if self.found == 0 { if self.truth == 0 { 1.0 } else { 0.0 } } else { self.matched as f32 / self.found as f32 }
    }

    pub fn recall(&self) -> f32 {
        if self.truth == 0 { if self.found == 0 { 1.0 } else { 0.0 } } else { self.matched as f32 / self.truth as f32 }
    }

    /// The harmonic mean of precision and recall: 1 for perfect, 0 for nothing right
    /// (including false hits where there were none to find).
    pub fn f_measure(&self) -> f32 {
        let (p, r) = (self.precision(), self.recall());
        if p + r == 0.0 { 0.0 } else { 2.0 * p * r / (p + r) }
    }
}

/// Match `found` hit times against `truth` (both sorted), each used at most once.
pub fn score(found: &[f32], truth: &[f32], tolerance: f32) -> Score {
    let (mut i, mut j, mut matched) = (0, 0, 0);
    while i < truth.len() && j < found.len() {
        let d = found[j] - truth[i];
        if d.abs() <= tolerance {
            matched += 1;
            i += 1;
            j += 1;
        } else if d < 0.0 {
            j += 1;
        } else {
            i += 1;
        }
    }
    Score { matched, found: found.len(), truth: truth.len() }
}

/// Audio with known hits.
pub struct Clip {
    pub name: String,
    pub rate: u32,
    pub frames: Vec<Frame>,
    pub hits: Vec<(f32, OnsetKind)>,
}

/// The generated songs, then the annotated clips in the fixtures folder.
pub fn clips() -> Vec<Clip> {
    let mut clips: Vec<Clip> =
        songs::all().into_iter().map(|s| Clip { name: s.name.to_string(), rate: songs::RATE, frames: s.frames, hits: s.hits }).collect();
    clips.extend(fixture_clips(&fixtures_dir()));
    clips
}

pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join("audio")
}

/// Every audio file in `dir` with a hit list beside it (the same name, `.txt`: one hit
/// per line, `seconds kind`, kind being kick, snare or hat; `#` starts a comment).
fn fixture_clips(dir: &Path) -> Vec<Clip> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| ["wav", "flac", "ogg", "mp3"].contains(&x.to_string_lossy().to_lowercase().as_str())))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .filter_map(|audio| {
            let text = std::fs::read_to_string(audio.with_extension("txt")).ok()?;
            let name = audio.file_stem()?.to_string_lossy().into_owned();
            let loaded = match super::decode(&audio, name.clone()) {
                Ok(l) => l,
                Err(e) => panic!("fixture {}: {e}", audio.display()),
            };
            let c = usize::from(loaded.channels.get());
            let frames = loaded.samples.chunks_exact(c).map(|f| to_stereo(f.iter().copied())).collect();
            Some(Clip { name, rate: loaded.rate.get(), frames, hits: parse_hits(&text, &audio) })
        })
        .collect()
}

fn parse_hits(text: &str, path: &Path) -> Vec<(f32, OnsetKind)> {
    let mut hits: Vec<(f32, OnsetKind)> = text
        .lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|l| !l.is_empty())
        .map(|l| {
            let mut parts = l.split_whitespace();
            let time = parts.next().and_then(|t| t.parse().ok());
            let kind = parts.next().and_then(|k| OnsetKind::ALL.into_iter().find(|kind| kind.name() == k));
            match (time, kind) {
                (Some(t), Some(k)) => (t, k),
                _ => panic!("{}: can't read hit line {l:?} (want `seconds kick|snare|hat`)", path.display()),
            }
        })
        .collect();
    hits.sort_by(|a, b| a.0.total_cmp(&b.0));
    hits
}

/// The analyzer's hits on `clip`, fed in frame-loop-sized chunks.
pub fn detect(clip: &Clip) -> Vec<Onset> {
    let mut analyzer = Analyzer::new(clip.rate);
    let mut onsets = Vec::new();
    for chunk in clip.frames.chunks(CHUNK) {
        analyzer.process(chunk, SENSITIVITY);
        analyzer.drain_onsets(&mut onsets);
    }
    onsets
}

/// Scores per hit kind (`OnsetKind::ALL` order), past the warm-up.
pub fn evaluate(clip: &Clip) -> [Score; 3] {
    let onsets = detect(clip);
    OnsetKind::ALL.map(|kind| {
        let found: Vec<f32> = onsets.iter().filter(|o| o.kind == kind && o.time > WARM_UP_S).map(|o| o.time).collect();
        let truth: Vec<f32> = clip.hits.iter().filter(|h| h.1 == kind && h.0 > WARM_UP_S).map(|h| h.0).collect();
        score(&found, &truth, TOLERANCE_S)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoring_matches_each_hit_once_within_the_tolerance() {
        let s = score(&[1.0, 1.02, 2.1, 3.0], &[1.01, 2.0, 3.04], TOLERANCE_S);
        assert_eq!(s, Score { matched: 2, found: 4, truth: 3 }, "1.0 matches 1.01 (1.02 can't too); 2.1 is too late; 3.0 matches 3.04");
        assert!((s.precision() - 0.5).abs() < 1e-6 && (s.recall() - 2.0 / 3.0).abs() < 1e-6);
        assert_eq!(score(&[], &[], TOLERANCE_S).f_measure(), 1.0, "nothing to find, nothing found");
        assert_eq!(score(&[1.0], &[], TOLERANCE_S).f_measure(), 0.0, "a false hit where there was none");
    }

    #[test]
    fn hit_lists_parse_with_comments() {
        let hits = parse_hits("# clip\n0.50 kick\n1.25 snare  # a ghost\n\n0.75 hat\n", Path::new("x.txt"));
        assert_eq!(hits, vec![(0.5, OnsetKind::Kick), (0.75, OnsetKind::Hat), (1.25, OnsetKind::Snare)]);
    }

    /// The generated songs place every hit inside the song, in order.
    #[test]
    fn generated_songs_are_well_formed() {
        for song in songs::all() {
            let length = song.frames.len() as f32 / songs::RATE as f32;
            assert!(length > 10.0, "{} is only {length} s", song.name);
            assert!(song.hits.windows(2).all(|w| w[0].0 <= w[1].0), "{}: hits out of order", song.name);
            assert!(song.hits.iter().all(|h| (0.0..length).contains(&h.0)), "{}: a hit outside the song", song.name);
            let peak = song.frames.iter().fold(0.0f32, |m, f| m.max(f[0].abs()).max(f[1].abs()));
            assert!((peak - 0.8).abs() < 1e-3, "{}: peak {peak}", song.name);
        }
    }

    type Row = (String, OnsetKind, Score);

    fn baseline_path() -> PathBuf {
        fixtures_dir().join("baseline.txt")
    }

    /// Every clip's hit detection, per kind, at or above its recorded baseline. Prints
    /// the table; with `UPDATE_ANALYZER_BASELINE` set, records it as the new baseline.
    #[test]
    fn hit_detection_holds_its_baseline() {
        let rows: Vec<Row> =
            clips().iter().flat_map(|clip| OnsetKind::ALL.into_iter().zip(evaluate(clip)).map(|(kind, s)| (clip.name.clone(), kind, s))).collect();
        eprintln!("{:<16} {:<6} {:>6} {:>6} {:>6} {:>12}", "clip", "kind", "P", "R", "F", "found/true");
        for (name, kind, s) in &rows {
            eprintln!(
                "{:<16} {:<6} {:>6.2} {:>6.2} {:>6.2} {:>12}",
                name,
                kind.name(),
                s.precision(),
                s.recall(),
                s.f_measure(),
                format!("{}/{}", s.found, s.truth)
            );
        }
        if std::env::var_os("UPDATE_ANALYZER_BASELINE").is_some() {
            let mut text = String::from("# Hit detection F-measures (see audio/eval.rs). Regenerate with UPDATE_ANALYZER_BASELINE=1.\n");
            for (name, kind, s) in &rows {
                text += &format!("{name}\t{}\t{:.3}\n", kind.name(), s.f_measure());
            }
            std::fs::write(baseline_path(), text).expect("write the baseline");
            return;
        }
        let baseline = std::fs::read_to_string(baseline_path()).expect("no baseline: run with UPDATE_ANALYZER_BASELINE=1");
        let mut failures = Vec::new();
        for line in baseline.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
            let fields: Vec<&str> = line.split('\t').collect();
            let [name, kind, expected] = fields[..] else { panic!("bad baseline line {line:?}") };
            let expected: f32 = expected.parse().expect("a number");
            match rows.iter().find(|(n, k, _)| n == name && k.name() == kind) {
                Some((_, _, s)) if s.f_measure() + SLACK < expected => {
                    failures.push(format!("{name} {kind}: F {:.3}, baseline {expected:.3}", s.f_measure()))
                }
                Some(_) => {}
                None => failures.push(format!("{name} {kind}: in the baseline but not measured")),
            }
        }
        assert!(failures.is_empty(), "hit detection got worse:\n{}", failures.join("\n"));
    }
}
