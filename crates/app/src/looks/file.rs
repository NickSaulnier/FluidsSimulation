//! Reading a look file: TOML, checked closely (unknown keys, names that don't exist and
//! equations that don't compile are errors with a line number), since a typo that's
//! quietly ignored is worse than one that's reported. The format is in
//! `looks/README.md`.

use super::{Code, Energy, Look, Source};
use crate::audio::reactions::{Effect, Reaction, Trigger};
use crate::expr::{self, Program, Stage};
use crate::render::feedback::{self, FeedbackSettings};
use crate::render::palette::PALETTES;
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
    solver: Spanned<String>,
    scene: Spanned<String>,
    palette: Spanned<String>,
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
    unknown("feedback", raw.get("feedback"), keys(FeedbackSettings::OFF))?;
    unknown("waves", raw.get("waves"), waves_known)?;
    if let Some(toml::Value::Array(shapes)) = raw.get("shape") {
        let shape_known = keys(ShapeSettings::OFF);
        for shape in shapes {
            unknown("shape", Some(shape), shape_known.clone())?;
        }
    }
    let section = |name: &str| raw.get(name).and_then(toml::Value::as_table);

    // The fluid: a solver and a scene of its kind, and a palette.
    let sim = doc.sim;
    let solver = sim.solver.get_ref().clone();
    let three_d = sim3d::SOLVERS.iter().any(|s| s.name == solver);
    if !three_d && !sim::SOLVERS.iter().any(|s| s.name == solver) {
        let names: Vec<&str> = sim::SOLVERS.iter().map(|s| s.name).chain(sim3d::SOLVERS.iter().map(|s| s.name)).collect();
        return Err(fail(Some(sim.solver.span().start), format!("no solver \"{solver}\" (there are: {})", names.join(", "))));
    }
    let scene = sim.scene.get_ref().clone();
    let scenes: Vec<&str> = if three_d { sim3d::PRESETS.iter().map(|p| p.name).collect() } else { sim::PRESETS.iter().map(|p| p.name).collect() };
    if !scenes.contains(&scene.as_str()) {
        let kind = if three_d { "3D " } else { "" };
        return Err(fail(Some(sim.scene.span().start), format!("no {kind}scene \"{scene}\" (there are: {})", scenes.join(", "))));
    }
    let palette = sim.palette.get_ref().clone();
    if !PALETTES.iter().any(|(n, _)| *n == palette) {
        let names: Vec<&str> = PALETTES.iter().map(|(n, _)| *n).collect();
        return Err(fail(Some(sim.palette.span().start), format!("no palette \"{palette}\" (there are: {})", names.join(", "))));
    }
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

    let code = compile_code(text, &doc.code).map_err(|(offset, message)| fail(offset, message))?;
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
        params,
        reactions,
        code,
        source,
    })
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

/// The `[code]` section compiled. `init` and `frame` share their variables: they're
/// compiled together and split. Errors are file offsets (exact unless the string has
/// escapes) and messages.
fn compile_code(text: &str, doc: &CodeDoc) -> Result<Code, (Option<usize>, String)> {
    let at = |s: &Spanned<String>, offset: usize| content_start(text, s.span()) + offset;
    let mut code = Code::default();
    let init = doc.init.as_ref().map_or("", |s| s.get_ref().as_str());
    let frame = doc.frame.as_ref().map_or("", |s| s.get_ref().as_str());
    if !init.trim().is_empty() || !frame.trim().is_empty() {
        let joined = format!("{init}\n{frame}");
        let program = expr::compile(&joined, Stage::Frame).map_err(|e| {
            let offset = if e.offset <= init.len() {
                doc.init.as_ref().map(|s| at(s, e.offset))
            } else {
                doc.frame.as_ref().map(|s| at(s, e.offset - init.len() - 1))
            };
            (offset, e.message)
        })?;
        let n_init = expr::compile(init, Stage::Frame).map(|p| p.stmts.len()).unwrap_or(0);
        let split = |stmts: Vec<expr::Stmt>| Program { stmts, ..program.clone() };
        let (init_stmts, frame_stmts) = program.stmts.split_at(n_init.min(program.stmts.len()));
        code.init = (!init_stmts.is_empty()).then(|| split(init_stmts.to_vec()));
        code.frame = Some(split(frame_stmts.to_vec()));
    }
    if let Some(pixel) = &doc.pixel
        && !pixel.get_ref().trim().is_empty()
    {
        let program = expr::compile(pixel.get_ref(), Stage::Pixel).map_err(|e| (Some(at(pixel, e.offset)), e.message))?;
        code.pixel = expr::wgsl::pixel(&program);
        feedback::validate_pixel(&code.pixel).map_err(|e| (Some(pixel.span().start), format!("the per-pixel code didn't make a valid shader:\n{e}")))?;
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
