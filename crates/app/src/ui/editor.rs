//! The equation editor (E): the look on screen's three programs, editable while it
//! runs. Every keystroke that leaves them compiling takes effect at once (the clock and
//! the variables carry on; see `Runtime::replace`); a mistake is underlined, and the
//! message shown under its program, while the look keeps running the last good version.
//! Save writes the look to the user's folder (a built-in becomes the user's own copy).

use super::Frame;
use super::theme;
use crate::looks::{CodeError, CodeSource, Part, compile_code};
use egui::{Color32, RichText, TextFormat, text::LayoutJob};
use egui_phosphor::regular as icon;
use std::sync::Arc;

const PARTS: [(Part, &str, &str); 3] = [
    (Part::Init, "Init", "Runs once, when the look starts"),
    (Part::Frame, "Per frame", "Runs every frame: sets the warp, waves, shapes, composite and q1 to q8"),
    (Part::Pixel, "Per pixel", "Runs for every pixel of the feedback's warp, on the GPU (reads x, y, rad, ang and q1 to q8)"),
];

#[derive(Default)]
pub struct Editor {
    /// The look being edited (by name), and its programs as typed: init, frame, pixel.
    look: Option<String>,
    text: [String; 3],
    /// As they were when loaded or last saved.
    saved: [String; 3],
    error: Option<CodeError>,
}

impl Editor {
    fn load(&mut self, name: &str, source: &CodeSource) {
        let text = [&source.init, &source.frame, &source.pixel].map(|s| s.clone().unwrap_or_default());
        *self = Self { look: Some(name.to_string()), saved: text.clone(), text, error: None };
    }

    fn source(&self) -> CodeSource {
        let [init, frame, pixel] = self.text.clone().map(Some);
        CodeSource { init, frame, pixel }
    }
}

pub fn show(ctx: &egui::Context, f: &mut Frame<'_>) {
    let mut open = f.state.editor_open;
    egui::Window::new(format!("{} Equations", icon::FUNCTION))
        .open(&mut open)
        .default_size([420.0, 560.0])
        .default_pos(ctx.content_rect().right_top() + egui::vec2(-440.0, 60.0))
        .collapsible(false)
        .show(ctx, |ui| contents(ui, f));
    f.state.editor_open = open;
}

fn contents(ui: &mut egui::Ui, f: &mut Frame<'_>) {
    let s = &mut *f.state;
    let Some(look) = s.director.current.and_then(|i| s.looks.looks.get(i)) else {
        ui.weak("No look on screen: pick one first.");
        return;
    };
    let name = look.name.clone();
    if s.editor.look.as_deref() != Some(name.as_str()) {
        s.editor.load(&name, &look.code.source);
    }
    let builtin = matches!(look.source, crate::looks::Source::Builtin(_));
    ui.label(RichText::new(&name).strong());
    ui.weak("Changes apply as you type, whenever they compile. Every name is in looks/README.md.");
    ui.add_space(4.0);

    let editor = &mut s.editor;
    let mut edited = None;
    egui::ScrollArea::vertical().auto_shrink([false, true]).max_height(ui.available_height() - 60.0).show(ui, |ui| {
        for (k, (part, title, help)) in PARTS.into_iter().enumerate() {
            ui.label(RichText::new(title).color(theme::HEADER)).on_hover_text(help);
            let error = editor.error.as_ref().filter(|e| e.part == part).cloned();
            let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap: f32| -> Arc<egui::Galley> {
                ui.fonts_mut(|fonts| fonts.layout_job(highlight(ui, text.as_str(), error.as_ref(), wrap)))
            };
            let rows = if part == Part::Init { 2 } else { 6 };
            let response = ui.add(
                egui::TextEdit::multiline(&mut editor.text[k])
                    .code_editor()
                    .desired_rows(rows)
                    .desired_width(f32::INFINITY)
                    .layouter(&mut layouter),
            );
            if response.changed() {
                edited = Some(part);
            }
            if let Some(e) = &error {
                ui.add(egui::Label::new(RichText::new(&e.message).color(theme::ERROR).small()).wrap());
            }
            ui.add_space(6.0);
        }
    });
    let mut apply = edited.map(|part| part == Part::Init);
    ui.separator();
    ui.horizontal(|ui| {
        let changed = editor.text != editor.saved;
        let save_tip = if builtin { "Save into your looks folder, as your own copy in place of the built-in one" } else { "Save over your look's file" };
        let can_save = changed && editor.error.is_none() && s.looks.folder.is_some();
        if ui.add_enabled(can_save, egui::Button::new(format!("{} Save", icon::FLOPPY_DISK))).on_hover_text(save_tip).clicked() {
            s.save_look_request = Some(name.clone());
            editor.saved = editor.text.clone();
        }
        if ui.add_enabled(changed, egui::Button::new(format!("{} Revert", icon::ARROW_COUNTER_CLOCKWISE))).on_hover_text("Back to the equations as last loaded or saved").clicked() {
            editor.text = editor.saved.clone();
            apply = Some(true);
        }
        let status = match &editor.error {
            Some(_) => RichText::new("A mistake: running the last version that worked").color(theme::ERROR).small(),
            None if changed => RichText::new("Running, not saved").color(theme::WARN).small(),
            None => RichText::new("Saved").weak().small(),
        };
        ui.label(status);
    });
    if let Some(rerun_init) = apply {
        match compile_code(&editor.source()) {
            Ok(code) => {
                editor.error = None;
                s.code_request = Some((code, rerun_init));
            }
            Err(e) => editor.error = Some(e),
        }
    }
}

/// `text` laid out as code, with `error`'s place underlined: the word it points at (or
/// the first line, without a place).
fn highlight(ui: &egui::Ui, text: &str, error: Option<&CodeError>, wrap: f32) -> LayoutJob {
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let plain = TextFormat::simple(font.clone(), ui.visuals().text_color());
    let mut job = LayoutJob { wrap: egui::text::TextWrapping { max_width: wrap, ..Default::default() }, ..Default::default() };
    let span = error.map(|e| span(text, e.offset));
    match span {
        Some((a, b)) if a < text.len() || b > a => {
            let bad = TextFormat { underline: egui::Stroke::new(1.5, theme::ERROR), background: Color32::from_rgba_unmultiplied(200, 40, 40, 50), ..plain.clone() };
            job.append(&text[..a], 0.0, plain.clone());
            job.append(&text[a..b], 0.0, bad);
            job.append(&text[b..], 0.0, plain);
        }
        Some(_) => {
            // At the very end (something missing): mark the end.
            job.append(text, 0.0, plain.clone());
            job.append(" ", 0.0, TextFormat { background: Color32::from_rgba_unmultiplied(200, 40, 40, 120), ..plain });
        }
        None => job.append(text, 0.0, plain),
    }
    job
}

/// The byte range to underline for a mistake at `offset`: the word or symbol there.
fn span(text: &str, offset: Option<usize>) -> (usize, usize) {
    let Some(start) = offset.map(|o| o.min(text.len())).filter(|&o| text.is_char_boundary(o)) else {
        return (0, text.find('\n').unwrap_or(text.len()));
    };
    let rest = &text[start..];
    let word = |c: char| c.is_alphanumeric() || c == '_' || c == '.';
    let len = match rest.chars().next() {
        Some(c) if word(c) => rest.find(|c: char| !word(c)).unwrap_or(rest.len()),
        Some('\n') => 0,
        Some(c) => c.len_utf8(),
        None => 0,
    };
    (start, start + len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_underline_covers_the_word() {
        let text = "zoom = 1.1\nrot = bas * 2";
        let e = compile_code(&CodeSource { frame: Some(text.into()), ..CodeSource::default() }).unwrap_err();
        assert_eq!(e.part, Part::Frame);
        let (a, b) = span(text, e.offset);
        assert_eq!(&text[a..b], "bas", "{e:?}");
        assert_eq!(span(text, None), (0, 10), "no place: the first line");
        assert_eq!(span("a = (", Some(5)), (5, 5));
    }
}
