//! The audit: every look in turn against the same stretch of music, its frames grabbed
//! small and scored (`render::score`), written out as a contact sheet: a page of every
//! look, worst first, with a strip of its frames, its scores, what's wrong with it and
//! how its quality moved since the last audit in the same folder.
//!
//! `fluid-toy --audit [TRACK] [--audit-from=S] [--audit-seconds=S] [--audit-looks=TEXT]
//! [--audit-reel] [--audit-random=N] [--audit-out=FOLDER]`
//!
//! - Without a track it plays the generated "anthem" (`audio::songs`: an intro, a
//!   build-up, a drop, a breakdown), from just before its first build-up, so each look
//!   sees a build-up, a drop and the start of a breakdown.
//! - `--audit-looks` keeps to looks whose names contain the text (or any of several,
//!   separated by commas); `--audit-reel` to the fidelity reel (`REEL`: looks that
//!   show each artefact the fidelity measures look for); `--audit-random` audits that
//!   many random looks instead (seeded 1 to N, so the same each time).
//! - Besides the small frames, every `FULL_EVERY_S` it grabs two frames in a row at the
//!   window's size, for the fidelity measures (banding, sharpness, shimmer; see
//!   `render::score`), shown on the sheet but not yet counted in the quality. The middle
//!   one is kept as `full_NNN.png`, for looking at closely (linked from the sheet).
//! - The app runs as the visualizer in a 1280 × 720 window, muted, and doesn't save its
//!   settings. It quits when done; the sheet is `index.html` in the folder
//!   (`Pictures\fluid-toy\audit` unless given).
//! - `--audit-sheet` only writes the sheet again from the last audit's measures, scored
//!   as the scoring now stands (for tuning it without running every look again).

use crate::render::grab::Image;
use crate::render::score::{Fidelity, Run, Score, fidelity, shimmer};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::PathBuf;

/// The window the audit runs in, and the size frames are grabbed at.
pub const WINDOW: [u32; 2] = [1280, 720];
pub const GRAB: [u32; 2] = [256, 144];
/// Each look runs this long before its frames count (the blend in, the fluid filling,
/// the feedback building up), then is grabbed this often.
const WARM_S: f32 = 4.0;
const GRAB_EVERY_S: f32 = 0.25;
/// Frames shown on the sheet, a look.
const STRIP: usize = 6;
/// How long to wait for the last grabs to come back.
const DRAIN_S: f32 = 1.5;
/// Where the anthem's first build-up starts, seconds.
const ANTHEM_BUILD_S: f32 = 15.2;
/// Where a track of your own is played from, unless given.
const TRACK_FROM_S: f32 = 30.0;
/// Full-size pairs are grabbed this often, from a second after the warm-up.
const FULL_EVERY_S: f32 = 3.0;
/// The fidelity reel: a dark smoke and a dark ink look (banding), long trails (banding
/// in the fade, softening), a fractal dive (shimmer), fire in 2D and 3D (the tone
/// curve, the raymarch's grain), and a liquid surface. None with film grain, which
/// the measures read as noise (it hides banding, as a dither does, and flickers).
pub const REEL: [&str; 8] = ["Smoke dance", "Ink nebula", "Aurora", "Spiral galaxy", "Julia tunnel", "Firestorm", "Bonfire 3D", "Ocean pulse"];

pub struct Options {
    pub track: Option<PathBuf>,
    from: Option<f32>,
    seconds: f32,
    /// Looks whose names contain any of these (lower case); or exactly the reel's.
    filter: Vec<String>,
    reel: bool,
    random: u64,
    out: PathBuf,
}

impl Options {
    /// From the command line; none without `--audit`.
    pub fn parse(args: &[String]) -> Option<Options> {
        if !args.iter().any(|a| a == "--audit" || a == "--audit-sheet") {
            return None;
        }
        let value = |key: &str| args.iter().find_map(|a| a.strip_prefix(key).and_then(|v| v.strip_prefix('=')));
        let out = value("--audit-out").map(PathBuf::from).or_else(|| crate::output::folder("Pictures").map(|f| f.join("audit"))).unwrap_or_else(|| PathBuf::from("audit"));
        Some(Options {
            track: args.iter().find(|a| !a.starts_with("--")).map(PathBuf::from),
            from: value("--audit-from").and_then(|v| v.parse().ok()),
            seconds: value("--audit-seconds").and_then(|v| v.parse().ok()).unwrap_or(36.0f32).max(WARM_S + 4.0),
            filter: value("--audit-looks").map_or_else(Vec::new, |v| v.split(',').map(|t| t.trim().to_lowercase()).filter(|t| !t.is_empty()).collect()),
            reel: args.iter().any(|a| a == "--audit-reel"),
            random: value("--audit-random").and_then(|v| v.parse().ok()).unwrap_or(0),
            out,
        })
    }
}

/// A look to audit: a look in the library (by name), or a random one from a seed.
#[derive(Clone, Debug)]
pub enum Item {
    Look(String),
    Random(u64),
}

/// What the app should do this frame.
#[derive(Default)]
pub struct Action {
    /// Show this look (play the music from `seek` too).
    pub start: Option<Item>,
    pub seek: Option<f32>,
    /// Grab this frame, at `GRAB`; and at the window's size.
    pub grab: Option<Tag>,
    pub full: Option<Tag>,
    pub quit: bool,
}

/// A grab's place: the look's turn, and the music's activity since the grab before;
/// for a full-size one, which pair it's in and whether it's the second.
#[derive(Clone, Copy, Debug)]
pub struct Tag {
    turn: usize,
    at: f32,
    activity: f32,
    pair: Option<(u32, bool)>,
}

/// This frame, as the app saw it.
pub struct Tick<'a> {
    pub dt: f32,
    /// The track is loaded (and how long it is).
    pub track: Option<f32>,
    /// The names of the looks in the library, and the one on screen.
    pub looks: &'a [String],
    pub showing: Option<&'a str>,
    pub level: f32,
    /// Kicks and snares this frame.
    pub hits: usize,
}

enum Phase {
    Loading,
    /// Waiting for the look to come up.
    Starting,
    Running,
    Draining(f32),
    Done,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Entry {
    name: String,
    fluid: String,
    score: Score,
    #[serde(default)]
    strip: String,
    #[serde(skip)]
    before: Option<f32>,
}

pub struct Audit {
    options: Options,
    queue: Vec<Item>,
    turn: usize,
    phase: Phase,
    /// The look on screen for this turn (its name, once known), the time since it came
    /// up, and the next grab's time.
    name: Option<String>,
    t: f32,
    next_grab: f32,
    /// The music since the last grab: levels summed, frames, and hits.
    level_sum: f32,
    level_frames: u32,
    hits: usize,
    /// Frames counted while grabbing, and over how long.
    frames: u32,
    frames_s: f32,
    requested: usize,
    received: usize,
    grabs: Vec<(Tag, Image)>,
    /// Full-size pairs: the next one's time and number, whether its second frame is due
    /// this frame, the first frame of a pair waiting for its second, and the fidelity
    /// measured (with the pair's shimmer).
    next_full: f32,
    pairs: u32,
    second_due: bool,
    first: Option<(u32, Image)>,
    fidelity: Vec<(Fidelity, Option<f32>)>,
    entries: Vec<Entry>,
    /// Each look's quality in the last audit in the same folder.
    before: HashMap<String, f32>,
    from: f32,
}

impl Audit {
    pub fn new(options: Options) -> Self {
        let before = std::fs::read_to_string(options.out.join("scores.json"))
            .ok()
            .and_then(|s| serde_json::from_str::<Vec<Entry>>(&s).ok())
            .map(|e| e.into_iter().map(|e| (e.name, e.score.quality)).collect())
            .unwrap_or_default();
        Self {
            options,
            queue: Vec::new(),
            turn: 0,
            phase: Phase::Loading,
            name: None,
            t: 0.0,
            next_grab: 0.0,
            level_sum: 0.0,
            level_frames: 0,
            hits: 0,
            frames: 0,
            frames_s: 0.0,
            requested: 0,
            received: 0,
            grabs: Vec::new(),
            next_full: 0.0,
            pairs: 0,
            second_due: false,
            first: None,
            fidelity: Vec::new(),
            entries: Vec::new(),
            before,
            from: 0.0,
        }
    }

    /// How a look's fluid is named on the sheet, set by the app when it starts one.
    pub fn started(&mut self, name: &str) {
        self.name = Some(name.to_string());
    }

    pub fn update(&mut self, tick: &Tick, fluid: impl Fn(&str) -> String) -> Action {
        let mut action = Action::default();
        match self.phase {
            Phase::Loading => {
                let Some(duration) = tick.track else { return action };
                self.queue = if self.options.random > 0 {
                    (1..=self.options.random).map(Item::Random).collect()
                } else {
                    let filter = &self.options.filter;
                    if self.options.reel {
                        REEL.iter().filter(|n| tick.looks.iter().any(|l| l == *n)).map(|n| Item::Look(n.to_string())).collect()
                    } else {
                        tick.looks.iter().filter(|n| filter.is_empty() || filter.iter().any(|f| n.to_lowercase().contains(f))).map(|n| Item::Look(n.clone())).collect()
                    }
                };
                let default = if self.options.track.is_some() { TRACK_FROM_S } else { ANTHEM_BUILD_S - WARM_S };
                self.from = self.options.from.unwrap_or(default);
                if self.from + self.options.seconds > duration {
                    self.from = (duration - self.options.seconds).max(0.0);
                }
                log::info!("audit: {} looks, {:.0} s each from {:.1} s, into {}", self.queue.len(), self.options.seconds, self.from, self.options.out.display());
                return self.next(action);
            }
            Phase::Starting => {
                if self.name.is_some() && tick.showing == self.name.as_deref() {
                    self.phase = Phase::Running;
                    (self.t, self.next_grab, self.next_full) = (0.0, WARM_S, WARM_S + 1.0);
                }
            }
            Phase::Running => {
                self.t += tick.dt;
                self.level_sum += tick.level;
                self.level_frames += 1;
                self.hits += tick.hits;
                if self.t >= WARM_S {
                    self.frames += 1;
                    self.frames_s += tick.dt;
                }
                if self.t >= self.options.seconds {
                    self.phase = Phase::Draining(0.0);
                } else if self.t >= self.next_grab {
                    self.next_grab += GRAB_EVERY_S;
                    let level = self.level_sum / self.level_frames.max(1) as f32;
                    let activity = level + if self.hits > 0 { 0.5 } else { 0.0 };
                    (self.level_sum, self.level_frames, self.hits) = (0.0, 0, 0);
                    // The first grab only starts the count: its activity is the warm-up's.
                    action.grab = Some(Tag { turn: self.turn, at: self.t, activity, pair: None });
                    self.requested += 1;
                }
                // A full-size pair: two frames in a row.
                if self.second_due {
                    self.second_due = false;
                    action.full = Some(Tag { turn: self.turn, at: self.t, activity: 0.0, pair: Some((self.pairs - 1, true)) });
                    self.requested += 1;
                } else if self.t >= self.next_full && self.t < self.options.seconds {
                    self.next_full += FULL_EVERY_S;
                    action.full = Some(Tag { turn: self.turn, at: self.t, activity: 0.0, pair: Some((self.pairs, false)) });
                    (self.pairs, self.second_due) = (self.pairs + 1, true);
                    self.requested += 1;
                }
            }
            Phase::Draining(t) => {
                let t = t + tick.dt;
                self.phase = Phase::Draining(t);
                if self.received >= self.requested || t >= DRAIN_S {
                    self.finish_look(&fluid);
                    return self.next(action);
                }
            }
            Phase::Done => {}
        }
        action
    }

    /// A grab came back. A full-size one is measured at once (the first of a pair once
    /// its second is in), and not kept.
    pub fn receive(&mut self, tag: Tag, image: Image) {
        if tag.turn != self.turn {
            return;
        }
        match tag.pair {
            None => self.grabs.push((tag, image)),
            Some((n, false)) => {
                self.flush_first();
                if n == self.options_pairs() / 2 {
                    let file = format!("full_{:03}.png", self.turn + 1);
                    if let Err(e) = std::fs::create_dir_all(&self.options.out).and_then(|()| crate::output::write_png(&self.options.out.join(&file), &image).map_err(std::io::Error::other)) {
                        log::warn!("couldn't write {file}: {e}");
                    }
                }
                self.first = Some((n, image));
            }
            Some((n, true)) => match self.first.take() {
                Some((m, first)) if m == n => self.fidelity.push((fidelity(&first), shimmer(&first, &image))),
                other => {
                    self.first = other;
                    self.flush_first();
                    self.fidelity.push((fidelity(&image), None));
                }
            },
        }
        self.received += 1;
    }

    /// How many full-size pairs a look gets.
    fn options_pairs(&self) -> u32 {
        ((self.options.seconds - WARM_S - 1.0) / FULL_EVERY_S).ceil().max(1.0) as u32
    }

    /// A pair's first frame whose second never came: measured alone.
    fn flush_first(&mut self) {
        if let Some((_, first)) = self.first.take() {
            self.fidelity.push((fidelity(&first), None));
        }
    }

    /// On to the next look, or write the sheet and quit.
    fn next(&mut self, mut action: Action) -> Action {
        if let Some(item) = self.queue.get(self.turn) {
            log::info!("audit {}/{}: {item:?}", self.turn + 1, self.queue.len());
            action.start = Some(item.clone());
            action.seek = Some(self.from);
            self.phase = Phase::Starting;
            self.name = None;
            (self.level_sum, self.level_frames, self.hits, self.frames, self.frames_s, self.requested, self.received) = (0.0, 0, 0, 0, 0.0, 0, 0);
            (self.pairs, self.second_due, self.first) = (0, false, None);
            self.grabs.clear();
            self.fidelity.clear();
            return action;
        }
        self.phase = Phase::Done;
        match self.write() {
            Ok(page) => log::info!("audit written: {}", page.display()),
            Err(e) => log::error!("couldn't write the audit: {e}"),
        }
        action.quit = true;
        action
    }

    fn finish_look(&mut self, fluid: &impl Fn(&str) -> String) {
        self.grabs.sort_by(|a, b| a.0.at.total_cmp(&b.0.at));
        let mut run = Run::default();
        for (tag, image) in &self.grabs {
            run.push(image, tag.activity);
        }
        self.flush_first();
        for &(f, s) in &self.fidelity {
            run.push_fidelity(f, s);
        }
        let fps = self.frames as f32 / self.frames_s.max(1e-3);
        let score = run.score(fps);
        let name = self.name.clone().unwrap_or_else(|| format!("look {}", self.turn + 1));
        let strip = format!("{:03}.png", self.turn + 1);
        if let Err(e) = self.write_strip(&strip) {
            log::warn!("couldn't write {strip}: {e}");
        }
        log::info!("audit: {name}: {:.0} ({} frames) {:?}", score.quality, run.len(), score.problems());
        let before = self.before.get(&name).copied();
        self.entries.push(Entry { fluid: fluid(&name), name, score, strip, before });
        self.grabs.clear();
        self.fidelity.clear();
        self.turn += 1;
    }

    /// `STRIP` of this look's frames, evenly spaced, side by side.
    fn write_strip(&self, file: &str) -> Result<(), String> {
        let Some(first) = self.grabs.first() else { return Ok(()) };
        let [w, h] = first.1.size;
        let picks: Vec<&Image> = (0..STRIP).filter_map(|k| self.grabs.get(k * (self.grabs.len() - 1) / (STRIP - 1).max(1))).map(|g| &g.1).filter(|i| i.size == [w, h]).collect();
        let width = w * picks.len() as u32;
        let mut rgba = vec![0u8; (width * h * 4) as usize];
        for (k, image) in picks.iter().enumerate() {
            for y in 0..h as usize {
                let row = &image.rgba[y * w as usize * 4..(y + 1) * w as usize * 4];
                let at = (y * width as usize + k * w as usize) * 4;
                rgba[at..at + row.len()].copy_from_slice(row);
            }
        }
        std::fs::create_dir_all(&self.options.out).map_err(|e| e.to_string())?;
        crate::output::write_png(&self.options.out.join(file), &Image { size: [width, h], rgba })
    }

    /// The scores and the page; returns the page's path.
    fn write(&mut self) -> Result<PathBuf, String> {
        let out = &self.options.out;
        std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
        let scores = out.join("scores.json");
        if scores.exists() {
            let _ = std::fs::rename(&scores, out.join("scores.previous.json"));
        }
        std::fs::write(&scores, serde_json::to_string_pretty(&self.entries).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        self.entries.sort_by(|a, b| a.score.quality.total_cmp(&b.score.quality));
        let page = out.join("index.html");
        std::fs::write(&page, sheet(&self.entries, &self.options, self.from)).map_err(|e| e.to_string())?;
        Ok(page)
    }
}

/// Write the sheet again from the last audit's measures in the folder, rescored (see
/// `--audit-sheet`); returns the page's path.
pub fn rewrite_sheet(options: &Options) -> Result<PathBuf, String> {
    let read = |file: &str| -> Result<Vec<Entry>, String> {
        let text = std::fs::read_to_string(options.out.join(file)).map_err(|e| format!("{file}: {e}"))?;
        serde_json::from_str(&text).map_err(|e| format!("{file}: {e}"))
    };
    let before: HashMap<String, f32> = read("scores.previous.json").unwrap_or_default().into_iter().map(|e| (e.name, e.score.quality)).collect();
    let mut entries = read("scores.json")?;
    for (k, e) in entries.iter_mut().enumerate() {
        e.score = e.score.clone().with_quality();
        e.before = before.get(&e.name).copied();
        if e.strip.is_empty() {
            e.strip = format!("{:03}.png", k + 1);
        }
    }
    entries.sort_by(|a, b| a.score.quality.total_cmp(&b.score.quality));
    let page = options.out.join("index.html");
    std::fs::write(&page, sheet(&entries, options, options.from.unwrap_or(f32::NAN))).map_err(|e| e.to_string())?;
    Ok(page)
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The contact sheet: a table of the looks, worst first.
fn sheet(entries: &[Entry], options: &Options, from: f32) -> String {
    let track = options.track.as_ref().map_or_else(|| "the generated anthem".to_string(), |t| t.display().to_string());
    let mean = entries.iter().map(|e| e.score.quality).sum::<f32>() / entries.len().max(1) as f32;
    let mut page = String::new();
    let _ = write!(
        page,
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Look audit</title>
<style>
:root {{ --bg: #0b0c10; --panel: #12141a; --text: #f2f4f8; --muted: #8a93a6; --line: #2a2f3a; --bad: #ff6b6b; --ok: #ffd166; --good: #5dd39e; }}
body {{ background: var(--bg); color: var(--text); font: 14px/1.4 Inter, system-ui, sans-serif; margin: 0; padding: 16px; }}
h1 {{ font-size: 20px; margin: 0 0 4px; }}
p {{ color: var(--muted); margin: 0 0 16px; }}
table {{ border-collapse: collapse; width: 100%; }}
td, th {{ border-bottom: 1px solid var(--line); padding: 6px 8px; text-align: left; vertical-align: middle; }}
th {{ color: var(--muted); font-weight: 500; position: sticky; top: 0; background: var(--bg); }}
td.n {{ text-align: right; font-variant-numeric: tabular-nums; }}
.q {{ font-weight: 700; font-size: 16px; }}
.bad {{ color: var(--bad); }} .ok {{ color: var(--ok); }} .good {{ color: var(--good); }}
.muted {{ color: var(--muted); }}
img {{ display: block; height: 72px; image-rendering: auto; }}
.wrap {{ overflow-x: auto; }}
</style></head><body>
<h1>Look audit</h1>
<p>{} looks, {:.0} s each against {}{} (scored after {:.0} s). Mean quality {:.0}. Worst first.</p>
<div class="wrap"><table>
<tr><th>Quality</th><th>Look</th><th>Problems</th><th>Frames</th><th class="n">Dark</th><th class="n">White</th><th class="n">Detail</th><th class="n">Filled</th><th class="n">Contrast</th><th class="n">Motion</th><th class="n">Follows</th><th class="n">Dead</th><th class="n">fps</th><th class="n" title="Share of the picture in terraces (8-bit banding)">Banding</th><th class="n" title="Fine detail over coarse: about 1 sharp, lower soft">Sharp</th><th class="n" title="Fine detail flickering between frames: 0 stable">Shimmer</th></tr>
"#,
        entries.len(),
        options.seconds,
        escape(&track),
        if from.is_finite() { format!(" from {from:.1} s") } else { String::new() },
        WARM_S,
        mean
    );
    for e in entries {
        let s = &e.score;
        let class = if s.quality < 40.0 { "bad" } else if s.quality < 70.0 { "ok" } else { "good" };
        let change = e.before.map_or(String::new(), |b| {
            let d = s.quality - b;
            if d.abs() < 1.0 { " <span class=\"muted\">=</span>".to_string() } else { format!(" <span class=\"{}\">{d:+.0}</span>", if d > 0.0 { "good" } else { "bad" }) }
        });
        let problems = s.problems().join(", ");
        let following = s.following.map_or("–".to_string(), |f| format!("{f:.2}"));
        let banding = s.banding.map_or("–".to_string(), |b| format!("{:.0}%", 100.0 * b));
        let (sharpness, shimmer) = (s.sharpness.map_or("–".to_string(), |v| format!("{v:.2}")), s.shimmer.map_or("–".to_string(), |v| format!("{v:.2}")));
        let _ = writeln!(
            page,
            "<tr><td class=\"q {class}\">{:.0}{change}</td><td>{}<br><span class=\"muted\">{}</span></td><td class=\"bad\">{}</td><td><a href=\"full_{}\"><img src=\"{}\" alt=\"\"></a></td><td class=\"n\">{:.0}%</td><td class=\"n\">{:.0}%</td><td class=\"n\">{:.3}</td><td class=\"n\">{:.0}%</td><td class=\"n\">{:.2}</td><td class=\"n\">{:.3}</td><td class=\"n\">{following}</td><td class=\"n\">{:.0}%</td><td class=\"n\">{:.0}</td><td class=\"n\">{banding}</td><td class=\"n\">{sharpness}</td><td class=\"n\">{shimmer}</td></tr>",
            s.quality,
            escape(&e.name),
            escape(&e.fluid),
            escape(&problems),
            e.strip,
            e.strip,
            100.0 * s.dark,
            100.0 * s.white,
            s.detail,
            100.0 * s.coverage,
            s.contrast,
            s.motion,
            100.0 * s.dead,
            s.fps,
        );
    }
    page.push_str("</table></div></body></html>\n");
    page
}
