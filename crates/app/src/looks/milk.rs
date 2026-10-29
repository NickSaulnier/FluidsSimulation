//! Classic MilkDrop presets (`.milk`) converted to looks.
//!
//! A preset is `key=value` lines: settings (`fDecay`, `zoom`, `nWaveMode`...), numbered
//! code lines (`per_frame_init_N`, `per_frame_N`, `per_pixel_N`), custom waves and
//! shapes (`wavecode_N_*`, `shapecode_N_*`), and in MilkDrop 2 HLSL shaders (`warp_N`,
//! `comp_N`). What maps onto a look:
//! - The warp and decay, as `[feedback]`. MilkDrop counts per frame, at whatever rate it
//!   ran; looks count per second, so its values are taken at 60 frames a second (a zoom
//!   of 1.01 a frame is 1.01^60 a second). Its y runs down the screen, ours up.
//! - Its waveform, as `[waves]` (the nearest of our modes), coloured by a palette made
//!   from the wave's colour; its first two custom shapes, as `[[shape]]`s (without their
//!   equations); its echo, gamma and invert, as `[feedback]` and `[composite]`.
//! - Its equations, as `[code]`. Our language reads them almost as written (it has
//!   MilkDrop's functions); each program is wrapped in lines that convert our values to
//!   MilkDrop's units before it and back after, so its arithmetic works unchanged.
//!   Per pixel, `y`, `rad` and `ang` are renamed to MilkDrop's versions of them.
//!
//! What doesn't map is left out and listed at the top of the file: MilkDrop 2 shaders,
//! custom waves, shape equations, borders and motion vectors. Equations that don't
//! compile are left out too (with the reason), so the rest of the preset still loads.

use super::{Code, CodeSource, Energy, Look, Source, compile_code, file};
use crate::render::Palette;
use crate::render::composite::CompositeSettings;
use crate::render::feedback::{EchoFlip, FeedbackMode, FeedbackSettings};
use crate::render::waves::{ShapeSettings, WaveMode, WaveSettings};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The frame rate MilkDrop's per-frame values are taken at.
const FPS: f32 = 60.0;
/// The picture shape assumed for static settings (equations use the real one).
const ASPECT: f32 = 16.0 / 9.0;
/// Our warp (screen heights) for MilkDrop's warp amount of 1.
const WARP: f32 = 0.01;

/// A converted preset: the look file's text, and what was left out.
pub struct Imported {
    pub name: String,
    pub text: String,
    pub left_out: Vec<String>,
}

/// Convert a preset (`text`, from a file named `stem`).
pub fn convert(text: &str, stem: &str) -> Result<Imported, String> {
    let mut values: HashMap<String, String> = HashMap::new();
    let mut lines: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('[') || line.starts_with("//") {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let key = key.trim().to_lowercase();
        lines.push((key.clone(), value.to_string()));
        values.entry(key).or_insert_with(|| value.trim().to_string());
    }
    if values.is_empty() {
        return Err("not a MilkDrop preset (no settings)".into());
    }
    let num = |key: &str, default: f32| values.get(key).and_then(|v| v.trim().parse::<f32>().ok()).filter(|v| v.is_finite()).unwrap_or(default);
    let on = |key: &str| num(key, 0.0) != 0.0;
    let mut left_out = Vec::new();

    // The warp and decay.
    let decay = num("fdecay", 0.98).clamp(0.0, 1.0);
    let echo_flip = match num("nvideoechoorientation", 0.0) as i32 {
        1 => EchoFlip::Horizontal,
        2 => EchoFlip::Vertical,
        3 => EchoFlip::Both,
        _ => EchoFlip::None,
    };
    let feedback = FeedbackSettings {
        enabled: true,
        mode: FeedbackMode::Brightest,
        trail: decay.powf(FPS).clamp(0.0, 0.98),
        zoom: num("zoom", 1.0).max(0.0).powf(FPS).clamp(0.05, 20.0),
        rotation: num("rot", 0.0) * FPS,
        drift: [num("dx", 0.0) * FPS * ASPECT, -num("dy", 0.0) * FPS],
        warp: num("warp", 1.0) * WARP,
        warp_speed: num("fwarpanimspeed", 1.0),
        flow: 0.0,
        echo: num("fvideoechoalpha", 0.0).clamp(0.0, 1.0),
        echo_zoom: num("fvideoechozoom", 1.0).clamp(0.1, 10.0),
        echo_flip,
        ..FeedbackSettings::OFF
    };
    if num("fzoomexponent", 1.0) != 1.0 {
        left_out.push("the zoom exponent".into());
    }
    if num("sx", 1.0) != 1.0 || num("sy", 1.0) != 1.0 || num("cx", 0.5) != 0.5 || num("cy", 0.5) != 0.5 {
        left_out.push("the stretch and the centre as settings (equations keep them)".into());
    }

    // The waveform: the nearest of our modes, coloured by its colour.
    let mode = match num("nwavemode", 0.0) as i32 {
        0 => WaveMode::Circle,
        4 | 6 | 7 => WaveMode::Line,
        _ => WaveMode::Xy,
    };
    let wave_y = (0.5 - num("wave_y", 0.5)) * 2.0;
    let mut waves = WaveSettings {
        mode,
        scale: num("fwavescale", 1.0).clamp(0.0, 2.0),
        size: 0.4,
        y: wave_y.clamp(-1.0, 1.0),
        thickness: if on("bwavethick") { 4.0 } else { 2.0 },
        brightness: num("fwavealpha", 0.8).clamp(0.0, 4.0),
        ..WaveSettings::OFF
    };
    if on("bwavedots") {
        left_out.push("dotted waves (drawn as lines)".into());
    }
    let colour = [num("wave_r", 1.0), num("wave_g", 1.0), num("wave_b", 1.0)].map(|c| c.clamp(0.0, 1.0));
    let byte = |k: f32| colour.map(|c| (c * k * 255.0).round() as u8);
    let palette = Palette { deep: byte(0.25), base: byte(0.6), highlight: byte(1.0) };

    // Custom shapes: the first two that are on, without their equations.
    let mut shapes = 0;
    for n in 0..4 {
        let key = |field: &str| format!("shapecode_{n}_{field}");
        if !on(&key("enabled")) {
            continue;
        }
        if shapes == waves.shapes.len() {
            left_out.push(format!("custom shape {n} (only two are drawn)"));
            continue;
        }
        let fill = num(&key("a"), 1.0).clamp(0.0, 1.0);
        let border = num(&key("border_a"), 0.0);
        waves.shapes[shapes] = ShapeSettings {
            enabled: true,
            sides: (num(&key("sides"), 4.0) as u32).clamp(3, 12),
            radius: (num(&key("rad"), 0.1) * 2.0).clamp(0.02, 2.0),
            centre: [(num(&key("x"), 0.5) - 0.5) * 2.0 * ASPECT, (num(&key("y"), 0.5) - 0.5) * 2.0],
            spin: 0.0,
            thickness: if border > 0.0 { 2.0 } else { 0.0 },
            bass_pulse: 0.0,
            fill,
        };
        shapes += 1;
        if lines.iter().any(|(k, _)| k.starts_with(&format!("shape_{n}_per_frame"))) {
            left_out.push(format!("custom shape {n}'s equations"));
        }
    }
    if (0..4).any(|n| on(&format!("wavecode_{n}_enabled"))) {
        left_out.push("custom waves".into());
    }
    if lines.iter().any(|(k, _)| k.starts_with("warp_") || k.starts_with("comp_")) {
        left_out.push("MilkDrop 2 shaders".into());
    }
    if num("ob_a", 0.0) > 0.0 || num("ib_a", 0.0) > 0.0 {
        left_out.push("the borders".into());
    }
    if num("nmotionvectorsx", 0.0) > 0.0 && num("mv_a", 0.0) > 0.0 {
        left_out.push("motion vectors".into());
    }
    for (key, what) in [("bbrighten", "brighten"), ("bdarken", "darken"), ("bsolarize", "solarize"), ("bdarkencenter", "darken centre")] {
        if on(key) {
            left_out.push(format!("the {what} filter"));
        }
    }

    let composite = CompositeSettings {
        invert: if on("binvert") { 1.0 } else { 0.0 },
        exposure: (num("fgammaadj", 2.0) / 2.0).clamp(0.25, 2.0),
        ..CompositeSettings::OFF
    };

    // The equations, wrapped to work in MilkDrop's units.
    let program = |prefix: &str| -> Option<String> {
        let mut numbered: Vec<(u32, &str)> = lines
            .iter()
            .filter_map(|(k, v)| {
                let n = k.strip_prefix(prefix)?;
                Some((n.parse().ok()?, v.as_str()))
            })
            .collect();
        numbered.sort_by_key(|(n, _)| *n);
        let joined: Vec<&str> = numbered.iter().map(|(_, v)| v.split("//").next().unwrap_or("")).collect();
        let statements = statements(&joined.join(" "));
        (!statements.is_empty()).then(|| statements.join("\n"))
    };
    let init = program("per_frame_init_");
    let frame = program("per_frame_").map(|code| format!("{FRAME_IN}\n{code}\n{FRAME_OUT}"));
    let pixel = program("per_pixel_").map(|code| format!("{PIXEL_IN}\n{}\n{PIXEL_OUT}", rename(&code, &[("y", "md_y"), ("rad", "md_rad"), ("ang", "md_ang")])));
    let mut source = CodeSource { init, frame, pixel };
    let code = loop {
        match compile_code(&source) {
            Ok(code) => break code,
            Err(e) => {
                let (part, what) = match e.part {
                    super::Part::Init => (&mut source.init, "the init equations"),
                    super::Part::Frame => (&mut source.frame, "the per-frame equations"),
                    super::Part::Pixel => (&mut source.pixel, "the per-pixel equations"),
                };
                left_out.push(format!("{what} ({})", e.message.lines().next().unwrap_or("")));
                *part = None;
            }
        }
    };

    let (author, title) = match stem.split_once(" - ") {
        Some((a, t)) => (a.trim().to_string(), t.trim().to_string()),
        None => (String::new(), stem.trim().to_string()),
    };
    let name = if title.is_empty() { stem.to_string() } else { title };
    let look = Look {
        name: name.clone(),
        author,
        tags: vec!["milkdrop".into(), "no fluid".into()],
        energy: Energy::Any,
        solver: String::new(),
        scene: String::new(),
        palette,
        liquid: false,
        orbit: 0.0,
        feedback,
        waves,
        composite,
        params: Vec::new(),
        reactions: Vec::new(),
        code: Code { ..code },
        source: Source::Builtin("milkdrop"),
    };
    let mut text = format!("# Imported from the MilkDrop preset \"{stem}\".\n");
    if !left_out.is_empty() {
        text += "# Left out (no equivalent here):\n";
        for what in &left_out {
            text += &format!("# - {what}\n");
        }
    }
    text += "\n";
    text += &file::write(&look);
    Ok(Imported { name, text, left_out })
}

/// Our values in MilkDrop's units, before a preset's per-frame equations: per frame at
/// 60 a second, y down, its own names for decay, echo, gamma and the wave; and the
/// inputs it has that we don't.
const FRAME_IN: &str = "// MilkDrop's units (per frame, y down), from ours
zoom = pow(zoom, 1 / 60); rot = rot / 60; dx = dx / (60 * aspect); dy = -dy / 60; cy = 1 - cy
sx = pow(sx, 1 / 60); sy = pow(sy, 1 / 60); warp = warp / 0.01
decay = pow(trail, 1 / 60); echo_alpha = echo; gamma = exposure * 2; wave_a = wave_brightness; wave_y = 0.5 - wave_y / 2
progress = 1; meshx = 48; meshy = 36; pixelsx = 1080 * aspect; pixelsy = 1080; aspectx = 1 / aspect; aspecty = 1
// The preset's equations";

/// And back to ours after them.
const FRAME_OUT: &str = "// Back to our units
zoom = pow(zoom, 60); rot = rot * 60; dx = dx * 60 * aspect; dy = -dy * 60; cy = 1 - cy
sx = pow(sx, 60); sy = pow(sy, 60); warp = warp * 0.01
trail = pow(decay, 60); echo = echo_alpha; exposure = gamma / 2; wave_brightness = wave_a; wave_y = (0.5 - wave_y) * 2";

/// The same per pixel, with MilkDrop's y (down), radius (1 in the corners) and angle.
const PIXEL_IN: &str = "// MilkDrop's units (per frame, y down), from ours
zoom = pow(zoom, 1 / 60); rot = rot / 60; dx = dx / (60 * aspect); dy = -dy / 60; cy = 1 - cy
sx = pow(sx, 1 / 60); sy = pow(sy, 1 / 60); warp = warp / 0.01
md_y = 1 - y; md_rad = rad / sqrt(aspect * aspect + 1); md_ang = 6.2831853 - ang
// The preset's equations";

const PIXEL_OUT: &str = "// Back to our units
zoom = pow(zoom, 60); rot = rot * 60; dx = dx * 60 * aspect; dy = -dy * 60; cy = 1 - cy
sx = pow(sx, 60); sy = pow(sy, 60); warp = warp * 0.01";

/// MilkDrop code as our statements, one a line: split at `;`, empty ones dropped,
/// `a += b` (and `-=`, `*=`, `/=`, `%=`) written out, and `$pi`-style constants as
/// numbers.
fn statements(code: &str) -> Vec<String> {
    let code = code.replace("$PI", "3.14159265").replace("$pi", "3.14159265").replace("$E", "2.71828183").replace("$e", "2.71828183").replace("$PHI", "1.61803399").replace("$phi", "1.61803399");
    code.split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            for op in ["+", "-", "*", "/", "%"] {
                if let Some((target, value)) = s.split_once(&format!("{op}=")) {
                    let target = target.trim();
                    if !target.is_empty() && target.chars().all(|c| c.is_alphanumeric() || c == '_') {
                        return format!("{target} = {target} {op} ({})", value.trim());
                    }
                }
            }
            s.split_whitespace().collect::<Vec<_>>().join(" ")
        })
        .collect()
}

/// `code` with whole names renamed (case aside), as `(from, to)`.
fn rename(code: &str, names: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(code.len());
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        let to = names.iter().find(|(from, _)| from.eq_ignore_ascii_case(word)).map(|(_, to)| *to);
        out.push_str(to.unwrap_or(word));
        word.clear();
    };
    for c in code.chars() {
        if c.is_alphanumeric() || c == '_' {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            out.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

/// Convert the preset at `path` into a look file in `folder` (over an earlier import of
/// it). Returns where it went and what was imported.
pub fn import_file(path: &Path, folder: &Path) -> Result<(PathBuf, Imported), String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    // Presets are often in the Windows code page: read what's text, replace the rest.
    let text = String::from_utf8_lossy(&bytes);
    let stem = path.file_stem().map_or_else(|| "preset".into(), |s| s.to_string_lossy().into_owned());
    let imported = convert(&text, &stem)?;
    std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    let out = folder.join(format!("milk_{}.toml", super::file_stem(&imported.name)));
    std::fs::write(&out, &imported.text).map_err(|e| e.to_string())?;
    Ok((out, imported))
}

/// The `.milk` files at `path` (a file, or a folder and the folders in it).
pub fn presets_in(path: &Path) -> Vec<PathBuf> {
    if path.is_file() {
        return vec![path.to_path_buf()];
    }
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(path) else { return out };
    let mut entries: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            out.extend(presets_in(&p));
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("milk")) {
            out.push(p);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A preset in MilkDrop 1's style (written for this test).
    const CLASSIC: &str = "[preset00]
fRating=4.000000
fGammaAdj=2.000000
fDecay=0.980000
fVideoEchoZoom=1.500000
fVideoEchoAlpha=0.300000
nVideoEchoOrientation=1
nWaveMode=0
bWaveDots=0
fWaveAlpha=1.200000
fWaveScale=0.900000
fWarpAnimSpeed=1.000000
zoom=1.010000
rot=0.003000
cx=0.500000
cy=0.500000
dx=0.000000
dy=0.002000
warp=0.500000
wave_r=0.900000
wave_g=0.400000
wave_b=0.100000
wave_y=0.500000
shapecode_0_enabled=1
shapecode_0_sides=6
shapecode_0_rad=0.150000
shapecode_0_x=0.500000
shapecode_0_y=0.500000
shapecode_0_a=0.300000
shapecode_0_border_a=1.000000
per_frame_init_1=spin = 0;
per_frame_1=spin = spin + 0.01*bass;
per_frame_2=rot = rot + 0.01*sin(time); // a comment
per_frame_3=zoom = zoom + 0.02*sqr(bass_att) - 0.01; q1 = spin;
per_frame_4=decay += 0.005;
per_pixel_1=zoom = zoom + 0.01*rad*sin(ang + q1);
per_pixel_2=dy = dy + 0.001*(y - 0.5);
";

    #[test]
    fn a_classic_preset_becomes_a_look() {
        let imported = convert(CLASSIC, "Somebody - Warm spiral").unwrap();
        assert_eq!(imported.name, "Warm spiral");
        assert!(imported.left_out.is_empty(), "{:?}", imported.left_out);
        let look = file::parse(&imported.text, "t", Source::Builtin("t")).unwrap_or_else(|e| panic!("{e}\n{}", imported.text));
        assert_eq!(look.author, "Somebody");
        assert!(!look.has_fluid());
        let fb = look.feedback;
        assert!((fb.trail - 0.98f32.powf(60.0)).abs() < 1e-3, "{fb:?}");
        assert!((fb.zoom - 1.01f32.powf(60.0)).abs() < 1e-3);
        assert!((fb.rotation - 0.18).abs() < 1e-4);
        assert!((fb.drift[1] + 0.12).abs() < 1e-4, "y is down in MilkDrop: {:?}", fb.drift);
        assert_eq!((fb.echo, fb.echo_zoom, fb.echo_flip), (0.3, 1.5, EchoFlip::Horizontal));
        assert_eq!(look.waves.mode, WaveMode::Circle);
        assert!(look.waves.shapes[0].enabled && look.waves.shapes[0].sides == 6);
        assert_eq!(look.palette.highlight, [230, 102, 26]);
        assert!(look.code.init.is_some() && look.code.frame.is_some() && !look.code.pixel.is_empty());
    }

    /// The wrapped equations work in MilkDrop's units: with no change, our values come
    /// back as they went in; a MilkDrop zoom of +0.01 a frame is about 1.01^60 a second.
    #[test]
    fn equations_run_in_milkdrops_units() {
        use crate::looks::runtime::{Runtime, Sound};
        use crate::render::feedback::FeedbackSettings as Fb;
        let run = |code: &str| {
            let text = format!("[preset00]\nzoom=1\n{code}");
            let look = file::parse(&convert(&text, "t").unwrap().text, "t", Source::Builtin("t")).unwrap();
            let fb = Fb { enabled: true, zoom: 1.0, rotation: 0.3, trail: 0.4, ..Fb::OFF };
            let frame = Runtime::new(&look.code).step(&fb, &WaveSettings::OFF, &CompositeSettings::OFF, Sound::default(), 1.0 / 60.0, 16.0 / 9.0);
            (frame.shared[crate::expr::slot("zoom")], frame.shared[crate::expr::slot("rot")], frame.feedback.trail)
        };
        let (zoom, rot, trail) = run("per_frame_1=x_unused = 1;");
        assert!((zoom - 1.0).abs() < 1e-4 && (rot - 0.3).abs() < 1e-4 && (trail - 0.4).abs() < 1e-3, "{zoom} {rot} {trail}");
        let (zoom, ..) = run("per_frame_1=zoom = zoom + 0.01;");
        assert!((zoom - 1.01f32.powf(60.0)).abs() < 0.01, "{zoom}");
    }

    /// What doesn't map is listed; equations that don't compile are left out, and the
    /// rest still loads.
    #[test]
    fn what_doesnt_map_is_listed() {
        let text = "[preset00]\nfDecay=0.9\nwavecode_0_enabled=1\nwarp_1=`shader_body { ret = 1; }\nper_frame_1=zoom = megabuf(3);\nob_a=0.5\n";
        let imported = convert(text, "x").unwrap();
        let left = imported.left_out.join("; ");
        for what in ["custom waves", "MilkDrop 2 shaders", "the borders", "the per-frame equations"] {
            assert!(left.contains(what), "{what} missing from: {left}");
        }
        assert!(imported.text.starts_with("# Imported"));
        let look = file::parse(&imported.text, "t", Source::Builtin("t")).unwrap();
        assert!(look.code.frame.is_none());
        assert!(convert("just some text", "x").is_err());
    }

    #[test]
    fn code_is_tidied_into_our_statements() {
        assert_eq!(statements("a = 1;; b += 2*c ; d = $PI;"), ["a = 1", "b = b + (2*c)", "d = 3.14159265"]);
        assert_eq!(rename("y = y + ys*Y; rad2 = rad", &[("y", "md_y"), ("rad", "md_rad")]), "md_y = md_y + ys*md_y; rad2 = md_rad");
    }

    #[test]
    fn folders_are_searched_for_presets() {
        let dir = std::env::temp_dir().join(format!("fluid-toy-milk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("pack")).unwrap();
        std::fs::write(dir.join("a.milk"), CLASSIC).unwrap();
        std::fs::write(dir.join("pack").join("b.MILK"), CLASSIC).unwrap();
        std::fs::write(dir.join("notes.txt"), "").unwrap();
        assert_eq!(presets_in(&dir), [dir.join("a.milk"), dir.join("pack").join("b.MILK")]);
        let out = dir.join("looks");
        let (path, imported) = import_file(&dir.join("a.milk"), &out).unwrap();
        assert_eq!(path, out.join("milk_a.toml"));
        assert_eq!(imported.name, "a");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
