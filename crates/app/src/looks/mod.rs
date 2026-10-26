//! Visualizer looks: bundles of a solver, a scene, colours, the feedback and waves,
//! solver parameters, reactions and equations (see `expr`), which the director (see
//! `director`) switches between with the music. Each is a TOML file (`file`; the format
//! is in `looks/README.md`): the built-ins are compiled in, and the user's own are read
//! from a folder and read again when they change (`Library::poll`), so editing a look
//! shows at once. A file that doesn't load is listed with its error and skipped.
//!
//! Solvers and scenes are named, not indexed, so the library survives registry changes.
//! A look naming a 3D solver (`sim3d::SOLVERS`) is a 3D look: its scene is a 3D preset,
//! and the camera orbits at its `orbit` speed.

mod file;
pub mod runtime;

use crate::audio::reactions::Reaction;
use crate::expr::Program;
use crate::render::feedback::FeedbackSettings;
use crate::render::palette::Palette;
use crate::render::waves::WaveSettings;
use crate::ui::{UiState, params};
pub use file::LoadError;
use sim::{PRESETS, World};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

/// The built-in looks, compiled in: file name, contents.
pub const BUILTIN: &[(&str, &str)] = &[
    ("ocean_pulse.toml", include_str!("../../looks/ocean_pulse.toml")),
    ("ink_storm.toml", include_str!("../../looks/ink_storm.toml")),
    ("firestorm.toml", include_str!("../../looks/firestorm.toml")),
    ("wind_tunnel.toml", include_str!("../../looks/wind_tunnel.toml")),
    ("lava_lamp.toml", include_str!("../../looks/lava_lamp.toml")),
    ("mercury_rain.toml", include_str!("../../looks/mercury_rain.toml")),
    ("bonfire_3d.toml", include_str!("../../looks/bonfire_3d.toml")),
    ("inferno.toml", include_str!("../../looks/inferno.toml")),
    ("smoke_dance.toml", include_str!("../../looks/smoke_dance.toml")),
];

/// How often the user's folder is checked for changed files, seconds.
const POLL_S: f32 = 1.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Energy {
    /// For quiet, ambient passages.
    Calm,
    /// For loud, driving ones.
    Energetic,
    /// Works for either.
    Any,
}

/// Where a look came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Builtin(&'static str),
    File(PathBuf),
}

/// A look's equations, compiled: the per-frame program (its `init` part runs once, when
/// the look starts) and the per-pixel one, as shader code ("" for none); and as written,
/// for saving.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Code {
    pub init: Option<Program>,
    pub frame: Option<Program>,
    pub pixel: String,
    pub source: CodeSource,
}

/// A look's equations as written.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CodeSource {
    pub init: Option<String>,
    pub frame: Option<String>,
    pub pixel: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    pub name: String,
    pub author: String,
    pub tags: Vec<String>,
    pub energy: Energy,
    pub solver: String,
    pub scene: String,
    pub palette: Palette,
    /// Liquid surface (particle solvers).
    pub liquid: bool,
    /// 3D looks: how fast the camera circles the scene, rad/s.
    pub orbit: f32,
    /// How each frame carries the last one.
    pub feedback: FeedbackSettings,
    /// The sound drawn on screen.
    pub waves: WaveSettings,
    /// Solver parameters, by key (ints are rounded, toggles on unless 0).
    pub params: Vec<(String, f32)>,
    pub reactions: Vec<Reaction>,
    pub code: Code,
    pub source: Source,
}

impl Look {
    /// The 3D solver this look runs, if it's a 3D look.
    pub fn solver_3d(&self) -> Option<usize> {
        sim3d::SOLVERS.iter().position(|s| s.name == self.solver)
    }

    pub fn suits(&self, energetic: bool) -> bool {
        match self.energy {
            Energy::Any => true,
            Energy::Calm => !energetic,
            Energy::Energetic => energetic,
        }
    }

    /// Whether `other` differs from this look only in what can change without
    /// restarting the scene (the picture, the colours, the reactions and the code).
    pub fn same_scene(&self, other: &Look) -> bool {
        self.solver == other.solver && self.scene == other.scene && self.params == other.params && self.liquid == other.liquid
    }
}

/// Every look: the built-ins, then the user's (a user's look named like a built-in
/// takes its place), and the files that didn't load.
pub struct Library {
    pub looks: Vec<Look>,
    pub errors: Vec<LoadError>,
    /// The user's folder, if there is one.
    pub folder: Option<PathBuf>,
    /// The folder's look files as last read: path, modified time, length.
    seen: Vec<(PathBuf, Option<SystemTime>, u64)>,
    checked: Option<Instant>,
}

impl Default for Library {
    fn default() -> Self {
        Self::builtin()
    }
}

impl Library {
    /// Only the built-in looks.
    pub fn builtin() -> Self {
        let mut library = Self { looks: Vec::new(), errors: Vec::new(), folder: None, seen: Vec::new(), checked: None };
        library.load();
        library
    }

    /// The built-ins and the looks in `folder` (which needn't exist yet).
    pub fn with_folder(folder: PathBuf) -> Self {
        let mut library = Self { folder: Some(folder), ..Self::builtin() };
        library.seen = library.scan();
        library.load();
        library
    }

    /// Where the user's looks live: `looks` beside the settings file.
    pub fn user_folder() -> Option<PathBuf> {
        Some(crate::ui::settings::path()?.parent()?.join("looks"))
    }

    #[cfg(test)]
    pub fn find(&self, name: &str) -> Option<usize> {
        self.looks.iter().position(|l| l.name == name)
    }

    /// Check the folder for changed files (at most every `POLL_S`), and read every look
    /// again if any changed. Returns whether it did.
    pub fn poll(&mut self, now: Instant) -> bool {
        if self.folder.is_none() || self.checked.is_some_and(|t| now.duration_since(t).as_secs_f32() < POLL_S) {
            return false;
        }
        self.checked = Some(now);
        let seen = self.scan();
        if seen == self.seen {
            return false;
        }
        self.seen = seen;
        self.load();
        true
    }

    /// Save `look` into the folder (over the file of the user's look of the same name, if
    /// there is one; else into a new file named after it), and read the looks again.
    /// Returns where it went.
    pub fn save(&mut self, look: &Look) -> std::io::Result<PathBuf> {
        let folder = self.folder.clone().ok_or_else(|| std::io::Error::other("there's no looks folder"))?;
        std::fs::create_dir_all(&folder)?;
        let path = match self.looks.iter().find(|l| l.name == look.name).map(|l| &l.source) {
            Some(Source::File(path)) => path.clone(),
            _ => {
                let words: String = look.name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { ' ' }).collect();
                let stem = words.split_whitespace().collect::<Vec<_>>().join("_");
                let stem = if stem.is_empty() { "look".to_string() } else { stem };
                (1..)
                    .map(|n| folder.join(if n == 1 { format!("{stem}.toml") } else { format!("{stem}_{n}.toml") }))
                    .find(|p| !p.exists())
                    .expect("a free name")
            }
        };
        std::fs::write(&path, file::write(look))?;
        self.seen = self.scan();
        self.checked = Some(Instant::now());
        self.load();
        Ok(path)
    }

    /// Write the built-in looks into the folder, as a start for one's own, skipping any
    /// file already there. Returns how many were written.
    pub fn copy_builtins(&self) -> std::io::Result<usize> {
        let Some(folder) = &self.folder else { return Ok(0) };
        std::fs::create_dir_all(folder)?;
        let mut written = 0;
        for (name, text) in BUILTIN {
            let path = folder.join(name);
            if !path.exists() {
                std::fs::write(path, text)?;
                written += 1;
            }
        }
        Ok(written)
    }

    fn scan(&self) -> Vec<(PathBuf, Option<SystemTime>, u64)> {
        let Some(Ok(entries)) = self.folder.as_ref().map(std::fs::read_dir) else { return Vec::new() };
        let mut files: Vec<_> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("toml")))
            .map(|p| {
                let meta = std::fs::metadata(&p).ok();
                let modified = meta.as_ref().and_then(|m| m.modified().ok());
                (p, modified, meta.map_or(0, |m| m.len()))
            })
            .collect();
        files.sort_by(|a, b| a.0.cmp(&b.0));
        files
    }

    fn load(&mut self) {
        self.looks.clear();
        self.errors.clear();
        for (name, text) in BUILTIN {
            match file::parse(text, name, Source::Builtin(name)) {
                Ok(look) => self.looks.push(look),
                Err(e) => self.errors.push(e),
            }
        }
        for (path, _, _) in &self.seen {
            let shown = file_name(path);
            let loaded = std::fs::read_to_string(path)
                .map_err(|e| LoadError { file: shown.clone(), line: None, message: e.to_string() })
                .and_then(|text| file::parse(&text, &shown, Source::File(path.clone())));
            match loaded {
                Ok(look) => match self.looks.iter().position(|l| l.name == look.name) {
                    Some(i) if matches!(self.looks[i].source, Source::Builtin(_)) => self.looks[i] = look,
                    Some(_) => self.errors.push(LoadError { file: shown, line: None, message: format!("another file is already named \"{}\"", look.name) }),
                    None => self.looks.push(look),
                },
                Err(e) => self.errors.push(e),
            }
        }
        for e in &self.errors {
            log::warn!("look {e}");
        }
    }
}

/// Set `key` on `world`: a float, an integer (rounded) or a toggle (on unless 0)
/// parameter. Returns whether it has such a parameter.
fn set_param(world: &mut dyn params::Params, key: &str, value: f32) -> bool {
    struct Toggle<'k>(&'k str, bool, bool);
    impl sim::ParamVisitor for Toggle<'_> {
        fn float(&mut self, _: &sim::FloatSpec, _: &mut f32) {}
        fn int(&mut self, _: &sim::IntSpec, _: &mut u32) {}
        fn toggle(&mut self, s: &sim::ToggleSpec, v: &mut bool) {
            if s.key == self.0 {
                (*v, self.2) = (self.1, true);
            }
        }
    }
    if params::float_param(world, key, Some(value)).is_some() || params::int_param(world, key, Some(value.round() as u32)).is_some() {
        return true;
    }
    let mut toggle = Toggle(key, value != 0.0, false);
    world.visit_params(&mut toggle);
    toggle.2
}

/// Every parameter `world` has, as a look would set it: floats, integers and toggles (1
/// or 0), by key. A key in `bases` takes that value instead (one the music is moving).
pub fn current_params(world: &mut dyn params::Params, bases: &[(String, f32)]) -> Vec<(String, f32)> {
    struct All(Vec<(String, f32)>);
    impl sim::ParamVisitor for All {
        fn float(&mut self, s: &sim::FloatSpec, v: &mut f32) {
            self.0.push((s.key.to_string(), *v));
        }
        fn int(&mut self, s: &sim::IntSpec, v: &mut u32) {
            self.0.push((s.key.to_string(), *v as f32));
        }
        fn toggle(&mut self, s: &sim::ToggleSpec, v: &mut bool) {
            self.0.push((s.key.to_string(), f32::from(u8::from(*v))));
        }
    }
    let mut all = All(Vec::new());
    world.visit_params(&mut all);
    for (key, value) in &mut all.0 {
        if let Some((_, base)) = bases.iter().find(|(k, _)| k == key) {
            *value = *base;
        }
    }
    all.0
}

/// Switch the world and UI to `look`. Unknown solver or scene names (a renamed preset)
/// leave that part as it is. (2D looks; for 3D ones see `apply_3d`.) Returns the
/// parameters the solver doesn't have.
pub fn apply(look: &Look, world: &mut World, ui: &mut UiState) -> Vec<String> {
    if let Some(solver) = world.solvers.iter().position(|s| s.name == look.solver)
        && solver != world.solver_index
    {
        world.switch_solver(solver, false);
    }
    let unknown = look.params.iter().filter(|(key, value)| !set_param(world, key, *value)).map(|(key, _)| key.clone()).collect();
    // Loading the scene also resets the solver, which applies restart-only parameters.
    match PRESETS.iter().position(|p| p.name == look.scene) {
        Some(scene) => world.load_preset(scene),
        None => world.reset(),
    }
    apply_picture(look, ui);
    ui.liquid = look.liquid;
    ui.show_particles = true;
    ui.overlay = None;
    unknown
}

/// Set up a 3D look in `world`, which already runs the look's solver: its parameters
/// and scene, and the UI's palette, reactions and orbit speed. Returns the parameters
/// the solver doesn't have.
pub fn apply_3d(look: &Look, world: &mut sim3d::World3d, ui: &mut UiState) -> Vec<String> {
    let unknown = look.params.iter().filter(|(key, value)| !set_param(world, key, *value)).map(|(key, _)| key.clone()).collect();
    match sim3d::PRESETS.iter().position(|p| p.name == look.scene) {
        Some(scene) => world.load_preset(scene),
        None => world.reset(),
    }
    apply_picture(look, ui);
    ui.orbit = look.orbit;
    unknown
}

/// The parts of a look that change without restarting the scene: the palette, the
/// reactions, the feedback and the waves (and, for a look edited while it shows, the
/// orbit).
pub fn apply_picture(look: &Look, ui: &mut UiState) {
    ui.palette = look.palette;
    ui.reactions = look.reactions.clone();
    ui.feedback = look.feedback;
    ui.waves = look.waves;
    if look.solver_3d().is_some() {
        ui.orbit = look.orbit;
    }
}

/// A file's name for showing.
fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library() -> Library {
        let l = Library::builtin();
        assert!(l.errors.is_empty(), "built-in looks that don't load: {:?}", l.errors);
        l
    }

    /// Every 2D look names a real solver, scene, palette and parameter, and has
    /// reactions.
    #[test]
    fn every_look_refers_to_things_that_exist() {
        for look in library().looks.iter().filter(|l| l.solver_3d().is_none()) {
            assert!(!look.reactions.is_empty(), "{}: no reactions", look.name);
            let mut world = World::new(0, 0);
            let mut ui = UiState::default();
            assert!(apply(look, &mut world, &mut ui).is_empty(), "{}: unknown parameters", look.name);
            assert_eq!(world.solvers[world.solver_index].name, look.solver);
            assert_eq!(PRESETS[world.preset_index].name, look.scene);
            for (key, value) in &look.params {
                let got = params::float_param(&mut world, key, None).or(params::int_param(&mut world, key, None).map(|v| v as f32));
                assert_eq!(got, Some(*value), "{}: parameter {key}", look.name);
            }
            for reaction in &look.reactions {
                if let Some(key) = &reaction.param {
                    assert!(params::float_param(&mut world, key, None).is_some(), "{}: reaction on unknown {key}", look.name);
                }
            }
            for key in look.code.frame.iter().flat_map(|p| &p.params) {
                assert!(params::float_param(&mut world, key, None).is_some(), "{}: equation sets unknown {key}", look.name);
            }
        }
    }

    /// The same for 3D looks, which need a GPU to build their world (skipped without).
    #[test]
    fn every_3d_look_refers_to_things_that_exist() {
        let library = library();
        let looks: Vec<_> = library.looks.iter().filter(|l| l.solver_3d().is_some()).collect();
        assert!(looks.len() >= 3, "too few 3D looks");
        for look in &looks {
            assert!(look.orbit > 0.0, "{}: no orbit", look.name);
        }
        let gpu = || match sim3d::GpuContext::new() {
            Ok(g) => Some(g),
            Err(e) => {
                eprintln!("skipping the parameter checks: {e:#}");
                None
            }
        };
        for look in looks {
            let Some(gpu) = gpu() else { return };
            let mut world = sim3d::World3d::new(gpu, look.solver_3d().unwrap(), 0);
            let mut ui = UiState::default();
            assert!(apply_3d(look, &mut world, &mut ui).is_empty(), "{}: unknown parameters", look.name);
            assert_eq!(sim3d::PRESETS[world.preset_index].name, look.scene);
            assert_eq!(ui.orbit, look.orbit);
            for (key, value) in &look.params {
                assert_eq!(params::float_param(&mut world, key, None), Some(*value), "{}: parameter {key}", look.name);
            }
            for reaction in &look.reactions {
                if let Some(key) = &reaction.param {
                    assert!(params::float_param(&mut world, key, None).is_some(), "{}: reaction on unknown {key}", look.name);
                }
            }
            for key in look.code.frame.iter().flat_map(|p| &p.params) {
                assert!(params::float_param(&mut world, key, None).is_some(), "{}: equation sets unknown {key}", look.name);
            }
        }
    }

    #[test]
    fn there_are_looks_for_quiet_and_loud_music() {
        let l = library();
        assert!(l.looks.iter().filter(|l| l.suits(false)).count() >= 2);
        assert!(l.looks.iter().filter(|l| l.suits(true)).count() >= 2);
    }

    /// The current parameters: every one the solver has, a moving one at its base, and
    /// applying them sets them back.
    #[test]
    fn current_params_capture_the_solver() {
        let mut world = World::new(0, 0);
        let keys = params::live_float_params(&mut world);
        let (moving, _) = keys[0];
        let params = current_params(&mut world, &[(moving.to_string(), 0.123)]);
        assert!(keys.iter().all(|(k, _)| params.iter().any(|(p, _)| p == k)), "a live parameter is missing: {params:?}");
        assert_eq!(params.iter().find(|(k, _)| k == moving).unwrap().1, 0.123);
        for (key, value) in params.iter().filter(|(k, _)| k != moving) {
            assert!(set_param(&mut world, key, *value), "{key} can't be set back");
        }
    }

    /// Saving: a new look gets a file named after it and loads; saving it again writes
    /// over that file; another look whose name makes the same file name gets a new one;
    /// a look named like a built-in is saved as the user's, in its place.
    #[test]
    fn saving_writes_files_and_loads_them() {
        let dir = std::env::temp_dir().join(format!("fluid-toy-save-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut l = Library::with_folder(dir.clone());
        let base = l.looks[0].clone();
        let mine = Look { name: "My look!".into(), orbit: 0.0, ..base.clone() };
        let path = l.save(&mine).unwrap();
        assert_eq!(path.file_name().unwrap(), "my_look.toml");
        let i = l.find("My look!").expect("loaded");
        assert_eq!(l.looks[i].feedback, mine.feedback);
        assert_eq!(l.looks.len(), BUILTIN.len() + 1);

        let changed = Look { feedback: FeedbackSettings { trail: 0.2, ..mine.feedback }, ..mine.clone() };
        assert_eq!(l.save(&changed).unwrap(), path, "saving again goes over the same file");
        assert_eq!(l.looks[l.find("My look!").unwrap()].feedback.trail, 0.2);

        let other = Look { name: "my LOOK".into(), ..mine.clone() };
        assert_eq!(l.save(&other).unwrap().file_name().unwrap(), "my_look_2.toml");
        assert_eq!(l.looks.len(), BUILTIN.len() + 2);

        let replaced = Look { feedback: FeedbackSettings { trail: 0.1, ..base.feedback }, ..base.clone() };
        l.save(&replaced).unwrap();
        let i = l.find(&base.name).unwrap();
        assert!(matches!(l.looks[i].source, Source::File(_)) && l.looks[i].feedback.trail == 0.1);
        assert_eq!(l.looks.len(), BUILTIN.len() + 2, "the user's copy takes the built-in's place");
        assert!(l.errors.is_empty(), "{:?}", l.errors);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A folder's looks: a new one is added, one named like a built-in replaces it, a
    /// broken one is listed and skipped, and a changed file is read again.
    #[test]
    fn the_folder_adds_replaces_and_reloads() {
        let dir = std::env::temp_dir().join(format!("fluid-toy-looks-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, text: &str| std::fs::write(dir.join(name), text).unwrap();
        let mine = |name: &str, trail: f32| {
            format!("[meta]\nname = \"{name}\"\n[sim]\nsolver = \"FLIP\"\nscene = \"Dam break\"\npalette = \"Ocean\"\n[feedback]\ntrail = {trail}\n")
        };
        write("mine.toml", &mine("Mine", 0.3));
        write("ocean.toml", &mine("Ocean pulse", 0.4));
        write("broken.toml", "[meta]\nname = \"Broken\"\n[sim]\nsolver = \"FLIP\"\nscene = \"Dam break\"\npalette = \"Ocean\"\n[code]\nframe = \"zoom = bas\"\n");
        let mut l = Library::with_folder(dir.clone());
        let n = BUILTIN.len();
        assert_eq!(l.looks.len(), n + 1, "{:?}", l.looks.iter().map(|l| &l.name).collect::<Vec<_>>());
        assert_eq!(l.looks[l.find("Mine").unwrap()].feedback.trail, 0.3);
        assert_eq!(l.looks[l.find("Ocean pulse").unwrap()].feedback.trail, 0.4, "the user's file replaces the built-in");
        assert_eq!(l.errors.len(), 1);
        assert!(l.errors[0].file == "broken.toml" && l.errors[0].line == Some(8), "{}", l.errors[0]);

        let start = Instant::now();
        assert!(!l.poll(start), "nothing changed");
        std::thread::sleep(std::time::Duration::from_millis(20));
        write("mine.toml", &mine("Mine", 0.65));
        assert!(!l.poll(start + std::time::Duration::from_millis(100)), "polled too soon after the last check");
        assert!(l.poll(start + std::time::Duration::from_secs(2)), "the change wasn't seen");
        assert_eq!(l.looks[l.find("Mine").unwrap()].feedback.trail, 0.65);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
