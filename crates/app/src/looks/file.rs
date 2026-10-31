//! Reading a look file: TOML, checked closely (unknown keys, names that don't exist and
//! equations that don't compile are errors with a line number), since a typo that's
//! quietly ignored is worse than one that's reported. The format is in
//! `looks/README.md`.
//!
//! A `[[variant]]` is written as the picture's settings it changes, over the look's: the
//! look's file with them merged in is read like any look, and its picture taken. Written
//! back the same way, from the difference between the two looks' files.

use super::{Code, CodeSource, Energy, Look, MAX_VARIANTS, Source, Variant};
use crate::audio::reactions::{Effect, Reaction, Trigger};
use crate::expr::{self, Program, Stage};
use crate::render::feedback::{self, FeedbackSettings};
use crate::render::palette::{PALETTES, Palette};
use crate::render::art::ArtSettings;
use crate::render::fractal::FractalSettings;
use crate::render::composite::CompositeSettings;
use crate::render::waves::{ShapeSettings, WaveSettings};
use serde::Deserialize;
use std::fmt;
use toml::Spanned;

/// A look file that didn't load: which, where (a line, from 1, if known) and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadError {
    pub file: String,
    pub line: Option<usize>,
    pub message: String,
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "{}, line {line}: {}", self.file, self.message),
            None => write!(f, "{}: {}", self.file, self.message),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Doc {
    meta: Meta,
    sim: Sim,
    feedback: Option<FeedbackSettings>,
    waves: Option<WaveSettings>,
    #[serde(default)]
    shape: Vec<ShapeSettings>,
    composite: Option<CompositeSettings>,
    art: Option<ArtSettings>,
    fractal: Option<FractalSettings>,
    #[serde(default)]
    code: CodeDoc,
    #[serde(default)]
    variant: Vec<toml::Table>,
}

/// What a variant can change: the palette (in `[sim]`), and these sections.
const PICTURE: [&str; 6] = ["feedback", "waves", "shape", "art", "fractal", "composite"];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Meta {
    name: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    energy: EnergyDoc,
    #[serde(default = "full")]
    dynamics: f32,
    #[serde(default)]
    harmony: f32,
}

fn full() -> f32 {
    1.0
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "lowercase")]
enum EnergyDoc {
    Calm,
    Energetic,
    #[default]
    Any,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Sim {
    /// Both left out: a look without fluid (the picture is only waves, shapes and the
    /// feedback).
    solver: Option<Spanned<String>>,
    scene: Option<Spanned<String>>,
    /// A palette's name, or its colours (`{ deep = [r, g, b], base = ..., highlight = ... }`).
    palette: Spanned<toml::Value>,
    #[serde(default)]
    liquid: bool,
    #[serde(default)]
    relief: f32,
    #[serde(default)]
    orbit: f32,
    #[serde(default)]
    params: toml::Table,
    #[serde(default)]
    reactions: Vec<Spanned<ReactionDoc>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReactionDoc {
    trigger: String,
    effect: String,
    #[serde(default = "one")]
    gain: f32,
    param: Option<String>,
    #[serde(default = "yes")]
    enabled: bool,
}

fn one() -> f32 {
    1.0
}

fn yes() -> bool {
    true
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct CodeDoc {
    init: Option<Spanned<String>>,
    frame: Option<Spanned<String>>,
    pixel: Option<Spanned<String>>,
}

/// The settings' keys, as a table (their values unused).
fn keys(settings: impl serde::Serialize) -> toml::Table {
    match toml::Value::try_from(settings) {
        Ok(toml::Value::Table(t)) => t,
        _ => toml::Table::new(),
    }
}

/// Lower case, letters and digits only: "Hi-hat" and "hihat" match.
fn simple(s: &str) -> String {
    s.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_lowercase()).collect()
}

/// Parse `text` (the file `file`, from `source`).
pub fn parse(text: &str, file: &str, source: Source) -> Result<Look, LoadError> {
    let line_of = |offset: usize| text[..offset.min(text.len())].matches('\n').count() + 1;
    let fail = |offset: Option<usize>, message: String| LoadError { file: file.to_string(), line: offset.map(line_of), message };
    let mut doc: Doc = toml::from_str(text).map_err(|e| fail(e.span().map(|s| s.start), e.message().trim().to_string()))?;
    // Keys the settings don't know (they take defaults for missing ones, which also lets
    // unknown ones through).
    let raw: toml::Table = toml::from_str(text).map_err(|e| fail(None, e.message().to_string()))?;
    let unknown = |section: &str, table: Option<&toml::Value>, known: toml::Table| -> Result<(), LoadError> {
        let Some(toml::Value::Table(table)) = table else { return Ok(()) };
        let Some(key) = table.keys().find(|k| !known.contains_key(*k)) else { return Ok(()) };
        let mut names: Vec<&str> = known.keys().map(String::as_str).collect();
        names.sort();
        let offset = text.find(&format!("\n{key} ")).or_else(|| text.find(&format!("\n{key}="))).map(|o| o + 1);
        Err(fail(offset, format!("[{section}] has no setting `{key}` (it has: {})", names.join(", "))))
    };
    let mut waves_known = keys(WaveSettings::OFF);
    waves_known.remove("shapes");
    waves_known.remove("art");
    waves_known.remove("fractal");
    unknown("art", raw.get("art"), keys(ArtSettings::OFF))?;
    unknown("fractal", raw.get("fractal"), keys(FractalSettings::OFF))?;
    unknown("feedback", raw.get("feedback"), keys(FeedbackSettings::OFF))?;
    unknown("waves", raw.get("waves"), waves_known)?;
    unknown("composite", raw.get("composite"), keys(CompositeSettings::OFF))?;
    if let Some(toml::Value::Array(shapes)) = raw.get("shape") {
        let shape_known = keys(ShapeSettings::OFF);
        for shape in shapes {
            unknown("shape", Some(shape), shape_known.clone())?;
        }
    }
    let section = |name: &str| raw.get(name).and_then(toml::Value::as_table);

    // The fluid: a solver and a scene of its kind (or neither, for no fluid), and a
    // palette.
    let sim = doc.sim;
    let (solver, scene) = match (&sim.solver, &sim.scene) {
        (None, None) => {
            if !sim.params.is_empty() || sim.liquid || sim.orbit != 0.0 || sim.relief != 0.0 {
                return Err(fail(None, "a look without a solver has no fluid: `params`, `liquid`, `relief` and `orbit` need one".into()));
            }
            (String::new(), String::new())
        }
        (None, Some(scene)) => return Err(fail(Some(scene.span().start), "a scene needs a `solver` too (or leave both out, for no fluid)".into())),
        (Some(solver), None) => return Err(fail(Some(solver.span().start), "a solver needs a `scene` too".into())),
        (Some(solver_at), Some(scene_at)) => {
            let solver = solver_at.get_ref().clone();
            let three_d = sim3d::SOLVERS.iter().any(|s| s.name == solver);
            // The 2D solvers are the CPU ones and the GPU ones, as the app runs them.
            let solvers_2d = || sim::SOLVERS.iter().chain(sim_gpu::SOLVERS);
            if !three_d && !solvers_2d().any(|s| s.name == solver) {
                let names: Vec<&str> = solvers_2d().map(|s| s.name).chain(sim3d::SOLVERS.iter().map(|s| s.name)).collect();
                return Err(fail(Some(solver_at.span().start), format!("no solver \"{solver}\" (there are: {})", names.join(", "))));
            }
            let scene = scene_at.get_ref().clone();
            let scenes: Vec<&str> = if three_d { sim3d::PRESETS.iter().map(|p| p.name).collect() } else { sim::PRESETS.iter().map(|p| p.name).collect() };
            if !scenes.contains(&scene.as_str()) {
                let kind = if three_d { "3D " } else { "" };
                return Err(fail(Some(scene_at.span().start), format!("no {kind}scene \"{scene}\" (there are: {})", scenes.join(", "))));
            }
            (solver, scene)
        }
    };
    let at = Some(sim.palette.span().start);
    let palette = match sim.palette.get_ref() {
        toml::Value::String(name) => match PALETTES.iter().find(|(n, _)| n == name) {
            Some((_, p)) => *p,
            None => {
                let names: Vec<&str> = PALETTES.iter().map(|(n, _)| *n).collect();
                return Err(fail(at, format!("no palette \"{name}\" (there are: {}, or give its colours)", names.join(", "))));
            }
        },
        colours => colours
            .clone()
            .try_into::<Palette>()
            .map_err(|e| fail(at, format!("a palette is a name or {{ deep = [r, g, b], base = [r, g, b], highlight = [r, g, b] }}: {}", e.message())))?,
    };
    let mut params = Vec::new();
    for (key, value) in &sim.params {
        let number = value.as_float().map(|f| f as f32).or_else(|| value.as_integer().map(|i| i as f32));
        let Some(number) = number else { return Err(fail(None, format!("[sim] params: `{key}` isn't a number"))) };
        params.push((key.clone(), number));
    }

    let mut reactions = Vec::new();
    for spanned in sim.reactions {
        let at = Some(spanned.span().start);
        let r = spanned.into_inner();
        let Some(trigger) = Trigger::ALL.into_iter().find(|t| simple(t.label()) == simple(&r.trigger) || simple(&format!("{t:?}")) == simple(&r.trigger)) else {
            let names: Vec<String> = Trigger::ALL.iter().map(|t| simple(&format!("{t:?}"))).collect();
            return Err(fail(at, format!("no trigger \"{}\" (there are: {})", r.trigger, names.join(", "))));
        };
        let Some(effect) = Effect::ALL.into_iter().find(|e| simple(e.label()) == simple(&r.effect) || simple(&format!("{e:?}")) == simple(&r.effect)) else {
            let names: Vec<String> = Effect::ALL.iter().map(|e| simple(&format!("{e:?}"))).collect();
            return Err(fail(at, format!("no effect \"{}\" (there are: {})", r.effect, names.join(", "))));
        };
        match (effect, &r.param) {
            (Effect::Param, None) => return Err(fail(at, "a `param` reaction needs `param = \"<key>\"`".into())),
            (e, Some(_)) if e != Effect::Param => return Err(fail(at, format!("only `param` reactions take a `param` (this is `{}`)", simple(&format!("{e:?}"))))),
            _ => {}
        }
        reactions.push(Reaction { param: r.param, ..Reaction::new(trigger, effect, r.gain, r.enabled) });
    }

    // The picture: feedback (on, if the section is there), waves and shapes (each on).
    let mut feedback = doc.feedback.unwrap_or(FeedbackSettings::OFF);
    if section("feedback").is_some_and(|t| !t.contains_key("enabled")) {
        feedback.enabled = true;
    }
    let mut waves = doc.waves.unwrap_or(WaveSettings::OFF);
    if doc.shape.len() > waves.shapes.len() {
        return Err(fail(None, format!("at most {} [[shape]]s", waves.shapes.len())));
    }
    let shape_tables: Vec<&toml::Table> = match raw.get("shape") {
        Some(toml::Value::Array(a)) => a.iter().filter_map(toml::Value::as_table).collect(),
        _ => Vec::new(),
    };
    for (i, mut shape) in doc.shape.into_iter().enumerate() {
        if shape_tables.get(i).is_some_and(|t| !t.contains_key("enabled")) {
            shape.enabled = true;
        }
        waves.shapes[i] = shape;
    }
    if let Some(mut art) = doc.art {
        art.enabled = section("art").is_none_or(|t| !t.contains_key("enabled")) || art.enabled;
        waves.art = art;
    }
    if let Some(mut fractal) = doc.fractal {
        fractal.enabled = section("fractal").is_none_or(|t| !t.contains_key("enabled")) || fractal.enabled;
        waves.fractal = fractal;
    }

    let code = compile_doc_code(text, &doc.code).map_err(|(offset, message)| fail(offset, message))?;

    // The variants: the file with each one's changes merged in, read as a look.
    let tables = std::mem::take(&mut doc.variant);
    if tables.len() > MAX_VARIANTS {
        return Err(fail(text.match_indices("[[variant]]").nth(MAX_VARIANTS).map(|(o, _)| o), format!("at most {MAX_VARIANTS} [[variant]]s")));
    }
    let mut variants = Vec::new();
    let mut base = raw.clone();
    base.remove("variant");
    for (i, table) in tables.iter().enumerate() {
        let at = text.match_indices("[[variant]]").nth(i).map(|(o, _)| o);
        let wrong = |message: String| fail(at, format!("[[variant]] {}: {message}", i + 1));
        let merged = merge_variant(&base, table).map_err(wrong)?;
        let merged = toml::to_string(&merged).map_err(|e| wrong(e.to_string()))?;
        let look = parse(&merged, file, source.clone()).map_err(|e| wrong(e.message))?;
        variants.push(look.picture());
    }

    let meta = doc.meta;
    Ok(Look {
        name: meta.name,
        author: meta.author,
        tags: meta.tags,
        energy: match meta.energy {
            EnergyDoc::Calm => Energy::Calm,
            EnergyDoc::Energetic => Energy::Energetic,
            EnergyDoc::Any => Energy::Any,
        },
        dynamics: meta.dynamics.clamp(0.0, 1.0),
        harmony: meta.harmony.clamp(0.0, 1.0),
        solver,
        scene,
        palette,
        liquid: sim.liquid,
        relief: sim.relief.clamp(0.0, 1.0),
        orbit: sim.orbit,
        feedback,
        waves,
        composite: doc.composite.unwrap_or(CompositeSettings::OFF),
        params,
        reactions,
        code,
        variants,
        source,
    })
}

/// A look's file (as a table, without variants) with a variant's changes merged in.
fn merge_variant(base: &toml::Table, changes: &toml::Table) -> Result<toml::Table, String> {
    let mut out = base.clone();
    for (key, value) in changes {
        match key.as_str() {
            "palette" => {
                if let Some(toml::Value::Table(sim)) = out.get_mut("sim") {
                    sim.insert(key.clone(), value.clone());
                }
            }
            "shape" => {
                out.insert(key.clone(), value.clone());
            }
            k if PICTURE.contains(&k) => {
                let Some(changes) = value.as_table() else { return Err(format!("`{k}` is a table of settings")) };
                let section = out.entry(k).or_insert_with(|| toml::Value::Table(toml::Table::new()));
                if let toml::Value::Table(section) = section {
                    section.extend(changes.iter().map(|(a, b)| (a.clone(), b.clone())));
                }
            }
            other => {
                return Err(format!(
                    "a variant changes only the picture: `palette`, [variant.feedback], [variant.waves], [[variant.shape]], [variant.art], [variant.fractal] or [variant.composite], not `{other}`"
                ));
            }
        }
    }
    Ok(out)
}

/// The changes that make `look`'s file into `variant`'s (see `merge_variant`).
fn variant_changes(look: &Look, variant: &Variant) -> toml::Table {
    let table = |l: &Look| toml::from_str::<toml::Table>(&write(l)).unwrap_or_default();
    let mut plain = look.clone();
    plain.variants.clear();
    let base = table(&plain);
    plain.set_picture(variant);
    let now = table(&plain);
    let mut changes = toml::Table::new();
    let palette = |t: &toml::Table| t.get("sim").and_then(|s| s.get("palette")).cloned();
    if let (true, Some(p)) = (palette(&base) != palette(&now), palette(&now)) {
        changes.insert("palette".into(), p);
    }
    let mut waves = keys(WaveSettings::OFF);
    for k in ["shapes", "art", "fractal"] {
        waves.remove(k);
    }
    let sections = [
        ("feedback", keys(FeedbackSettings::OFF), true),
        ("waves", waves, false),
        ("art", keys(ArtSettings::OFF), true),
        ("fractal", keys(FractalSettings::OFF), true),
        ("composite", keys(CompositeSettings::OFF), false),
    ];
    for (name, defaults, switched) in sections {
        let (was, is) = (base.get(name).and_then(toml::Value::as_table), now.get(name).and_then(toml::Value::as_table));
        // A section that's there without `enabled` is on.
        let default = |k: &str| if switched && k == "enabled" { Some(toml::Value::Boolean(true)) } else { defaults.get(k).cloned() };
        if was.is_none() && is.is_none() {
            continue;
        }
        // Switched off: written as off, the look's settings back to their defaults.
        let off: toml::Table = [("enabled".to_string(), toml::Value::Boolean(false))].into_iter().collect();
        let empty = toml::Table::new();
        let now = match is {
            Some(now) => now,
            None if switched => &off,
            None => &empty,
        };
        let was_ = was.unwrap_or(&empty);
        let mut section: toml::Table = now.iter().filter(|(k, v)| was_.get(*k) != Some(v)).map(|(k, v)| (k.clone(), v.clone())).collect();
        section.extend(was_.keys().filter(|k| !now.contains_key(*k)).filter_map(|k| Some((k.clone(), default(k)?))));
        if switched && is.is_none() {
            section.insert("enabled".into(), toml::Value::Boolean(false));
        }
        // A section newly there switches it on, even with nothing else changed.
        if section.is_empty() && !(switched && was.is_none()) {
            continue;
        }
        changes.insert(name.into(), toml::Value::Table(section));
    }
    if base.get("shape") != now.get("shape") {
        changes.insert("shape".into(), now.get("shape").cloned().unwrap_or(toml::Value::Array(Vec::new())));
    }
    changes
}

/// `look` as a file, in the form `parse` reads: settings at their defaults are left out.
pub fn write(look: &Look) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let text = |s: &str| toml::Value::String(s.to_string()).to_string();
    let value = |v: toml::Value| tidy(v).to_string();
    let _ = writeln!(out, "[meta]\nname = {}", text(&look.name));
    if !look.author.is_empty() {
        let _ = writeln!(out, "author = {}", text(&look.author));
    }
    if !look.tags.is_empty() {
        let _ = writeln!(out, "tags = {}", value(toml::Value::Array(look.tags.iter().map(|t| toml::Value::String(t.clone())).collect())));
    }
    let energy = match look.energy {
        Energy::Calm => "calm",
        Energy::Energetic => "energetic",
        Energy::Any => "any",
    };
    let _ = writeln!(out, "energy = {}", text(energy));
    if look.dynamics != 1.0 {
        let _ = writeln!(out, "dynamics = {}", value(toml::Value::Float(f64::from(look.dynamics))));
    }
    if look.harmony != 0.0 {
        let _ = writeln!(out, "harmony = {}", value(toml::Value::Float(f64::from(look.harmony))));
    }
    let _ = writeln!(out, "\n[sim]");
    if look.has_fluid() {
        let _ = writeln!(out, "solver = {}\nscene = {}", text(&look.solver), text(&look.scene));
    }
    let palette = match look.palette.name() {
        Some(name) => text(name),
        None => value(toml::Value::try_from(look.palette).expect("serialisable")),
    };
    let _ = writeln!(out, "palette = {palette}");
    if look.liquid {
        let _ = writeln!(out, "liquid = true");
    }
    if look.relief != 0.0 {
        let _ = writeln!(out, "relief = {}", value(toml::Value::Float(f64::from(look.relief))));
    }
    if look.orbit != 0.0 {
        let _ = writeln!(out, "orbit = {}", value(toml::Value::Float(f64::from(look.orbit))));
    }
    if !look.params.is_empty() {
        let params: toml::Table = look.params.iter().map(|(k, v)| (k.clone(), toml::Value::Float(f64::from(*v)))).collect();
        let _ = writeln!(out, "params = {}", value(toml::Value::Table(params)));
    }
    if !look.reactions.is_empty() {
        let _ = writeln!(out, "reactions = [");
        for r in &look.reactions {
            let (trigger, effect) = (simple(&format!("{:?}", r.trigger)), simple(&format!("{:?}", r.effect)));
            let mut line = format!("    {{ trigger = {}, effect = {}, gain = {}", text(&trigger), text(&effect), value(toml::Value::Float(f64::from(r.gain))));
            if let Some(key) = &r.param {
                let _ = write!(line, ", param = {}", text(key));
            }
            if !r.enabled {
                line += ", enabled = false";
            }
            let _ = writeln!(out, "{line} }},");
        }
        let _ = writeln!(out, "]");
    }
    let mut section = |header: &str, keys: Vec<(String, toml::Value)>, always: bool| {
        if always || !keys.is_empty() {
            let _ = writeln!(out, "\n{header}");
            for (k, v) in keys {
                let _ = writeln!(out, "{k} = {}", value(v));
            }
        }
    };
    let fb = look.feedback;
    let mut feedback = changed(fb, FeedbackSettings::OFF, &["enabled"]);
    if !fb.enabled && !feedback.is_empty() {
        feedback.insert(0, ("enabled".into(), toml::Value::Boolean(false)));
    }
    section("[feedback]", feedback, fb.enabled);
    section("[waves]", changed(look.waves, WaveSettings::OFF, &["shapes", "art", "fractal"]), false);
    for shape in look.waves.shapes.iter().filter(|s| s.enabled) {
        section("[[shape]]", changed(*shape, ShapeSettings::OFF, &["enabled"]), true);
    }
    if look.waves.art.enabled {
        section("[art]", changed(look.waves.art, ArtSettings::OFF, &["enabled"]), true);
    }
    if look.waves.fractal.enabled {
        section("[fractal]", changed(look.waves.fractal, FractalSettings::OFF, &["enabled"]), true);
    }
    section("[composite]", changed(look.composite, CompositeSettings::OFF, &[]), false);
    let code = &look.code.source;
    if code.init.is_some() || code.frame.is_some() || code.pixel.is_some() {
        let _ = writeln!(out, "\n[code]");
        for (key, src) in [("init", &code.init), ("frame", &code.frame), ("pixel", &code.pixel)] {
            let Some(src) = src else { continue };
            let src = if src.ends_with('\n') { src.clone() } else { format!("{src}\n") };
            // A literal string keeps the code exactly as written, unless it holds the
            // closing quotes itself.
            let quoted = if src.contains("'''") { text(&src) } else { format!("'''\n{src}'''") };
            let _ = writeln!(out, "{key} = {quoted}");
        }
    }
    if !look.variants.is_empty() {
        let variants = look.variants.iter().map(|v| tidy(toml::Value::Table(variant_changes(look, v)))).collect();
        let doc: toml::Table = [("variant".to_string(), toml::Value::Array(variants))].into_iter().collect();
        if let Ok(text) = toml::to_string(&doc) {
            let _ = write!(out, "\n{text}");
        }
    }
    out
}

/// The settings that differ from `defaults` (but `skip`), in order.
fn changed(settings: impl serde::Serialize, defaults: impl serde::Serialize, skip: &[&str]) -> Vec<(String, toml::Value)> {
    let (now, base) = (keys(settings), keys(defaults));
    now.into_iter().filter(|(k, v)| !skip.contains(&k.as_str()) && base.get(k) != Some(v)).collect()
}

/// Numbers as their shortest single-precision form (0.3, not 0.30000001192092896).
fn tidy(v: toml::Value) -> toml::Value {
    match v {
        toml::Value::Float(f) => toml::Value::Float(format!("{}", f as f32).parse().unwrap_or(f)),
        toml::Value::Array(a) => toml::Value::Array(a.into_iter().map(tidy).collect()),
        toml::Value::Table(t) => toml::Value::Table(t.into_iter().map(|(k, v)| (k, tidy(v))).collect()),
        other => other,
    }
}

/// Where a string's contents start in the file, from its span (which includes the
/// quotes; a multi-line string's first newline isn't part of it).
fn content_start(text: &str, span: std::ops::Range<usize>) -> usize {
    let raw = &text[span.clone()];
    let quotes = if raw.starts_with("\"\"\"") || raw.starts_with("'''") { 3 } else { 1 };
    let after = span.start + quotes;
    if quotes == 3 {
        if text[after..].starts_with("\r\n") {
            return after + 2;
        }
        if text[after..].starts_with('\n') {
            return after + 1;
        }
    }
    after
}

/// The `[code]` section compiled (see `compile_code`). Errors are file offsets (exact
/// unless the string has escapes) and messages.
fn compile_doc_code(text: &str, doc: &CodeDoc) -> Result<Code, (Option<usize>, String)> {
    let raw = |s: &Option<Spanned<String>>| s.as_ref().map(|s| s.get_ref().clone());
    compile_code(&CodeSource { init: raw(&doc.init), frame: raw(&doc.frame), pixel: raw(&doc.pixel) }).map_err(|e| {
        let spanned = match e.part {
            Part::Init => &doc.init,
            Part::Frame => &doc.frame,
            Part::Pixel => &doc.pixel,
        };
        let offset = spanned.as_ref().map(|s| match e.offset {
            Some(offset) => content_start(text, s.span()) + offset,
            None => s.span().start,
        });
        (offset, e.message)
    })
}

/// One of a look's three programs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Init,
    Frame,
    Pixel,
}

/// A mistake in one of a look's programs: which, where in it (a byte offset, if known)
/// and what.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeError {
    pub part: Part,
    pub offset: Option<usize>,
    pub message: String,
}

/// A look's programs compiled, from the source of each. `init` and `frame` share their
/// variables: they're compiled together and split. (Kept as written, less trailing
/// space, for saving.)
pub fn compile_code(source: &CodeSource) -> Result<Code, CodeError> {
    let written = |s: &Option<String>| s.as_ref().map(|s| s.trim_end().to_string()).filter(|s| !s.trim().is_empty());
    let mut code = Code { source: CodeSource { init: written(&source.init), frame: written(&source.frame), pixel: written(&source.pixel) }, ..Code::default() };
    let init = source.init.as_deref().unwrap_or("");
    let frame = source.frame.as_deref().unwrap_or("");
    if !init.trim().is_empty() || !frame.trim().is_empty() {
        let joined = format!("{init}\n{frame}");
        let program = expr::compile(&joined, Stage::Frame).map_err(|e| {
            let (part, offset) = if e.offset <= init.len() { (Part::Init, e.offset) } else { (Part::Frame, e.offset - init.len() - 1) };
            CodeError { part, offset: Some(offset), message: e.message }
        })?;
        let n_init = expr::compile(init, Stage::Frame).map(|p| p.stmts.len()).unwrap_or(0);
        let split = |stmts: Vec<expr::Stmt>| Program { stmts, ..program.clone() };
        let (init_stmts, frame_stmts) = program.stmts.split_at(n_init.min(program.stmts.len()));
        code.init = (!init_stmts.is_empty()).then(|| split(init_stmts.to_vec()));
        code.frame = Some(split(frame_stmts.to_vec()));
    }
    if let Some(pixel) = &source.pixel
        && !pixel.trim().is_empty()
    {
        let program = expr::compile(pixel, Stage::Pixel).map_err(|e| CodeError { part: Part::Pixel, offset: Some(e.offset), message: e.message })?;
        code.pixel = expr::wgsl::pixel(&program);
        feedback::validate_pixel(&code.pixel)
            .map_err(|e| CodeError { part: Part::Pixel, offset: None, message: format!("the per-pixel code didn't make a valid shader:\n{e}") })?;
    }
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::fractal::FractalKind;

    const BASE: &str = "[meta]\nname = \"T\"\n[sim]\nsolver = \"FLIP\"\nscene = \"Dam break\"\npalette = \"Ocean\"\n";

    fn load(extra: &str) -> Result<Look, LoadError> {
        parse(&format!("{BASE}{extra}"), "t.toml", Source::Builtin("t.toml"))
    }

    fn error(extra: &str) -> LoadError {
        load(extra).err().unwrap_or_else(|| panic!("{extra:?} loaded"))
    }

    /// A variant changes the picture it names over the look's, keeping the rest; it can
    /// switch a section on or off; it can't touch the fluid; and it's written back as it
    /// was read.
    #[test]
    fn variants_change_the_picture_over_the_look() {
        let look = load(
            "[feedback]\ntrail = 0.5\nrotation = 0.2\n[composite]\nvignette = 0.4\n\
             [[variant]]\npalette = \"Lava\"\n[variant.feedback]\nmode = \"brightest\"\n[variant.fractal]\nkind = \"julia\"\n\
             [[variant]]\n[variant.feedback]\nenabled = false\n[variant.composite]\nsymmetry = \"mirror_x\"\n",
        )
        .unwrap();
        assert_eq!(look.variants.len(), 2);
        let one = look.variant(1);
        assert_eq!(one.palette, PALETTES.iter().find(|p| p.0 == "Lava").unwrap().1);
        assert!(one.feedback.enabled && one.feedback.mode == feedback::FeedbackMode::Brightest && one.feedback.trail == 0.5 && one.feedback.rotation == 0.2);
        assert!(one.waves.fractal.enabled && one.waves.fractal.kind == FractalKind::Julia && one.composite.vignette == 0.4);
        assert_eq!((one.solver.as_str(), &one.code), (look.solver.as_str(), &look.code));
        let two = look.variant(2);
        assert!(!two.feedback.enabled && two.palette == look.palette && two.composite.vignette == 0.4);
        assert_eq!(look.variant(0), look);
        assert_eq!(look.describe_variant(1), "colours, feedback mode, fractal");
        let back = parse(&write(&look), "t.toml", Source::Builtin("t.toml")).unwrap_or_else(|e| panic!("{e}\n{}", write(&look)));
        assert_eq!(back.variants, look.variants, "{}", write(&look));

        let e = error("[[variant]]\nsolver = \"PBF\"\n");
        assert!(e.message.contains("[[variant]] 1") && e.message.contains("only the picture") && e.line == Some(7), "{e:?}");
        assert!(error("[[variant]]\n[variant.composite]\nsymetry = \"mirror_x\"\n").message.contains("symetry"));
        assert!(error(&"[[variant]]\n".repeat(4)).message.contains("at most 3"));
    }

    /// Relief is read (kept to 0..1), written back, and needs a fluid to light.
    #[test]
    fn relief_needs_a_fluid() {
        let base = "[meta]
name = \"T\"
[sim]
solver = \"Stable fluids (dye)\"
scene = \"Wind tunnel\"
palette = \"Ink\"
";
        let look = parse(&format!("{base}relief = 0.6
"), "t.toml", Source::Builtin("t.toml")).unwrap();
        assert_eq!(look.relief, 0.6);
        assert_eq!(parse(&write(&look), "t.toml", Source::Builtin("t.toml")).unwrap().relief, 0.6);
        assert_eq!(parse(&format!("{base}relief = 3.0
"), "t.toml", Source::Builtin("t.toml")).unwrap().relief, 1.0);
        let none = parse("[meta]
name = \"T\"
[sim]
palette = \"Ink\"
relief = 0.5
", "t.toml", Source::Builtin("t.toml"));
        assert!(none.is_err_and(|e| e.message.contains("relief")));
    }

    #[test]
    fn a_minimal_look_loads_with_everything_off() {
        let look = load("").unwrap();
        assert_eq!((look.name.as_str(), look.energy), ("T", Energy::Any));
        assert!(!look.feedback.enabled && !look.waves.any() && look.code == Code::default());
    }

    #[test]
    fn sections_turn_things_on() {
        let art = load("[art]\nsize = 0.5\nspin = 0.2\n").unwrap().waves.art;
        assert!(art.enabled && art.size == 0.5 && art.spin == 0.2, "{art:?}");
        assert!(!load("").unwrap().waves.art.enabled);
        assert!(error("[art]\nzise = 0.5\n").message.contains("size"));
        let fractal = load("[fractal]\nkind = \"julia\"\nzoom = 2.0\n").unwrap().waves.fractal;
        assert!(fractal.enabled && fractal.kind == FractalKind::Julia && fractal.zoom == 2.0, "{fractal:?}");
        assert!(!load("").unwrap().waves.fractal.enabled);
        assert!(error("[fractal]\nzoon = 2.0\n").message.contains("zoom"));
        let look = load("[feedback]\ntrail = 0.3\n[[shape]]\nsides = 5\n").unwrap();
        assert!(look.feedback.enabled && look.feedback.trail == 0.3);
        assert!(look.waves.shapes[0].enabled && look.waves.shapes[0].sides == 5 && !look.waves.shapes[1].enabled);
        assert!(!load("[feedback]\nenabled = false\n").unwrap().feedback.enabled);
    }

    #[test]
    fn reactions_take_friendly_names() {
        let look = load("reactions = [{ trigger = \"Hi-hat\", effect = \"flash\" }, { trigger = \"bass\", effect = \"Parameter\", param = \"gravity\", gain = 0.5 }]\n").unwrap();
        assert_eq!(look.reactions[0], Reaction::new(Trigger::Hat, Effect::Flash, 1.0, true));
        assert_eq!(look.reactions[1].param.as_deref(), Some("gravity"));
    }

    /// Mistakes are reported with the line they're on.
    #[test]
    fn mistakes_are_errors_with_lines() {
        let line = |extra: &str| error(extra).line;
        assert_eq!(line("[feedback]\ntrial = 0.3\n"), Some(8), "{}", error("[feedback]\ntrial = 0.3\n"));
        assert!(error("[feedback]\ntrial = 0.3\n").message.contains("trail"));
        assert!(error("[waves]\nmode = \"spiral\"\n").message.contains("spiral"));
        assert!(error("[colour]\nx = 1\n").message.contains("colour"));
        assert!(parse("[meta]\nname = \"T\"\n[sim]\nsolver = \"Magic\"\nscene = \"Dam break\"\npalette = \"Ocean\"\n", "t", Source::Builtin("t")).unwrap_err().message.contains("no solver"));
        assert!(parse("[meta]\nname = \"T\"\n[sim]\nsolver = \"FLIP\"\nscene = \"Torch\"\npalette = \"Ocean\"\n", "t", Source::Builtin("t")).unwrap_err().message.contains("no scene"));
        assert!(error("reactions = [{ trigger = \"cowbell\", effect = \"flash\" }]\n").message.contains("cowbell"));
        assert!(error("reactions = [{ trigger = \"kick\", effect = \"param\" }]\n").message.contains("param"));
        // Equations: the line of the mistake within the file.
        let e = error("[code]\nframe = \"\"\"\nzoom = 1.1\nrot = bas\n\"\"\"\n");
        assert_eq!(e.line, Some(10), "{e}");
        assert!(e.message.contains("bass"), "{e}");
        assert_eq!(error("[code]\ninit = \"a = 1\"\nframe = \"zoom = b\"\n").line, Some(9));
        assert!(error("[code]\npixel = \"trail = 0.5\"\n").message.contains("per frame"));
    }

    /// Every built-in loads from a Windows checkout's `\r\n` line endings too, with the
    /// same settings and equations that run the same.
    #[test]
    fn looks_load_with_windows_line_endings() {
        for (name, text) in super::super::BUILTIN {
            let lf = parse(&text.replace("\r\n", "\n"), name, Source::Builtin(name)).unwrap();
            let crlf = parse(&text.replace("\r\n", "\n").replace('\n', "\r\n"), name, Source::Builtin(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!((crlf.feedback, crlf.waves, crlf.composite, &crlf.reactions), (lf.feedback, lf.waves, lf.composite, &lf.reactions), "{name}");
            assert_eq!((&crlf.code.init, &crlf.code.frame, &crlf.code.pixel), (&lf.code.init, &lf.code.frame, &lf.code.pixel), "{name}: the equations differ");
        }
    }

    /// Every built-in written out and read back is the same look; so is a custom palette,
    /// a toggle parameter, a disabled reaction and feedback kept off.
    #[test]
    fn looks_survive_writing_and_reading() {
        for (name, text) in super::super::BUILTIN {
            let look = parse(text, name, Source::Builtin(name)).unwrap();
            let written = write(&look);
            let back = parse(&written, name, Source::Builtin(name)).unwrap_or_else(|e| panic!("{name}: {e}\n{written}"));
            assert_eq!(back, look, "{name} changed:\n{written}");
        }
        let mut look = load("[code]\nframe = \"zoom = 1.1\"\n").unwrap();
        look.palette = Palette { deep: [1, 2, 3], base: [40, 50, 60], highlight: [200, 210, 220] };
        look.params = vec![("buoyancy".into(), 0.3), ("liquid_mode".into(), 1.0)]; // as a table reads back: by key
        look.reactions = vec![Reaction::new(Trigger::Beat, Effect::Hue, 0.25, false)];
        look.feedback = FeedbackSettings { enabled: false, zoom: 1.3, ..FeedbackSettings::OFF };
        let written = write(&look);
        let back = parse(&written, "t", Source::Builtin("t")).unwrap();
        assert_eq!(back, Look { source: Source::Builtin("t"), ..look }, "{written}");
    }

    /// A look on a GPU solver (as Save as writes one, running it) loads and round-trips.
    #[test]
    fn gpu_solvers_load() {
        let text = "[meta]\nname = \"T\"\n[sim]\nsolver = \"PBF (GPU)\"\nscene = \"Dam break\"\npalette = \"Lava\"\n";
        let look = parse(text, "t", Source::Builtin("t")).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(look.solver, "PBF (GPU)");
        assert_eq!(parse(&write(&look), "t", Source::Builtin("t")).unwrap(), look);
    }

    /// Leaving out the solver and scene makes a look without fluid; it needs both or
    /// neither, and nothing that only a fluid takes.
    #[test]
    fn a_look_can_have_no_fluid() {
        let text = "[meta]\nname = \"T\"\n[sim]\npalette = \"Ink\"\n[waves]\nmode = \"circle\"\n";
        let look = parse(text, "t", Source::Builtin("t")).unwrap();
        assert!(!look.has_fluid() && look.waves.any());
        assert_eq!(parse(&write(&look), "t", Source::Builtin("t")).unwrap(), look, "{}", write(&look));
        let fails = |sim: &str| parse(&format!("[meta]\nname = \"T\"\n[sim]\npalette = \"Ink\"\n{sim}"), "t", Source::Builtin("t")).unwrap_err().message;
        assert!(fails("scene = \"Dam break\"\n").contains("solver"));
        assert!(fails("solver = \"FLIP\"\n").contains("scene"));
        assert!(fails("params = { buoyancy = 0.2 }\n").contains("no fluid"));
    }

    #[test]
    fn a_fractal_survives_writing() {
        let look = load("[fractal]\nkind = \"burning_ship\"\ncentre = [-1.75, -0.03]\nzoom = 20.0\niterations = 300\n").unwrap();
        assert_eq!(look.waves.fractal.kind, FractalKind::BurningShip);
        let source = Source::Builtin("t.toml");
        assert_eq!(parse(&write(&look), "t.toml", source).unwrap(), look, "{}", write(&look));
    }

    /// `init` and `frame` share variables; `pixel` becomes shader code.
    #[test]
    fn code_compiles_into_its_parts() {
        let look = load("[code]\ninit = \"spin = 0.5\"\nframe = \"rot = spin * bass; q1 = time\"\npixel = \"zoom = zoom + q1 * rad\"\n").unwrap();
        let (init, frame) = (look.code.init.unwrap(), look.code.frame.unwrap());
        assert_eq!((init.stmts.len(), frame.stmts.len()), (1, 2));
        assert_eq!(init.locals, frame.locals);
        assert!(look.code.pixel.contains("m_zoom"));
    }
}
