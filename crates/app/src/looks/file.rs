//! Reading a look file: TOML, checked closely (unknown keys, names that don't exist and
//! equations that don't compile are errors with a line number), since a typo that's
//! quietly ignored is worse than one that's reported. The format is in
//! `looks/README.md`.

use super::{Code, CodeSource, Energy, Look, Source};
use crate::audio::reactions::{Effect, Reaction, Trigger};
use crate::expr::{self, Program, Stage};
use crate::render::feedback::{self, FeedbackSettings};
use crate::render::palette::{PALETTES, Palette};
use crate::render::art::ArtSettings;
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
    #[serde(default)]
    code: CodeDoc,
}

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
    let doc: Doc = toml::from_str(text).map_err(|e| fail(e.span().map(|s| s.start), e.message().trim().to_string()))?;
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
    unknown("art", raw.get("art"), keys(ArtSettings::OFF))?;
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
            if !sim.params.is_empty() || sim.liquid || sim.orbit != 0.0 {
                return Err(fail(None, "a look without a solver has no fluid: `params`, `liquid` and `orbit` need one".into()));
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

    let code = compile_doc_code(text, &doc.code).map_err(|(offset, message)| fail(offset, message))?;
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
        solver,
        scene,
        palette,
        liquid: sim.liquid,
        orbit: sim.orbit,
        feedback,
        waves,
        composite: doc.composite.unwrap_or(CompositeSettings::OFF),
        params,
        reactions,
        code,
        source,
    })
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
    let _ = writeln!(out, "energy = {}\n\n[sim]", text(energy));
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
    section("[waves]", changed(look.waves, WaveSettings::OFF, &["shapes", "art"]), false);
    for shape in look.waves.shapes.iter().filter(|s| s.enabled) {
        section("[[shape]]", changed(*shape, ShapeSettings::OFF, &["enabled"]), true);
    }
    if look.waves.art.enabled {
        section("[art]", changed(look.waves.art, ArtSettings::OFF, &["enabled"]), true);
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

    const BASE: &str = "[meta]\nname = \"T\"\n[sim]\nsolver = \"FLIP\"\nscene = \"Dam break\"\npalette = \"Ocean\"\n";

    fn load(extra: &str) -> Result<Look, LoadError> {
        parse(&format!("{BASE}{extra}"), "t.toml", Source::Builtin("t.toml"))
    }

    fn error(extra: &str) -> LoadError {
        load(extra).err().unwrap_or_else(|| panic!("{extra:?} loaded"))
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
