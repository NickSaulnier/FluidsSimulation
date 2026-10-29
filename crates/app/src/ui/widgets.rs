//! Building blocks shared by the panel tabs, so every tab looks and moves the same:
//! - groups: rounded, with a header in the display face that folds them (the chevron
//!   turning, the contents unfolding), remembered between runs (`Folds`); the advanced
//!   ones (`FOLDED_BY_DEFAULT`) start folded;
//! - form rows, the label in a fixed-width left column so every widget lines up;
//! - switches for on/off settings, the knob sliding across;
//! - icon buttons that glow under the pointer;
//! - a tab bar whose underline slides to the chosen tab;
//! - titles with a soft glow.

use super::{anim, theme};
use egui::{Align, Color32, Frame, Label, Layout, Margin, RichText, Sense, Stroke, Ui, vec2};
use std::collections::BTreeMap;

/// Width of the label column in form rows.
pub const LABEL_WIDTH: f32 = 140.0;
/// Marks parameters whose change restarts the simulation.
pub const RESTART_MARK: &str = "⟳";
/// Groups that start folded: details most people don't need at hand.
pub const FOLDED_BY_DEFAULT: &[&str] = &["Analysis", "Beat", "Reactions", "Overlay", "Step breakdown", "Render breakdown (GPU)", "Shortcuts", "Performance"];
/// Icon buttons, points.
pub const ICON_BUTTON: egui::Vec2 = vec2(36.0, 30.0);

/// Which groups are open, by title (a group not listed is at its default). The app
/// keeps it in the settings; each frame it's lent to egui's memory for the groups to
/// read and change.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Folds(pub BTreeMap<String, bool>);

impl Folds {
    fn id() -> egui::Id {
        egui::Id::new("folds")
    }

    /// Lend the folds to `ctx` for this frame's groups.
    pub fn lend(&self, ctx: &egui::Context) {
        ctx.data_mut(|d| d.insert_temp(Self::id(), self.clone()));
    }

    /// Take back what this frame's groups changed.
    pub fn take_back(&mut self, ctx: &egui::Context) {
        if let Some(folds) = ctx.data(|d| d.get_temp::<Folds>(Self::id())) {
            *self = folds;
        }
    }

    fn is_open(ui: &Ui, title: &str) -> bool {
        let stored = ui.data(|d| d.get_temp::<Folds>(Self::id())).and_then(|f| f.0.get(title).copied());
        stored.unwrap_or(!FOLDED_BY_DEFAULT.contains(&title))
    }

    fn set_open(ui: &Ui, title: &str, open: bool) {
        ui.data_mut(|d| d.get_temp_mut_or_default::<Folds>(Self::id()).0.insert(title.to_string(), open));
    }
}

/// A title owning the groups below it (the solver's name above its parameters), in the
/// display face with a soft glow.
pub fn title(ui: &mut Ui, text: &str) {
    glow_text(ui, text, theme::display(17.0), theme::TEXT);
}

/// Text with a glow: copies in the accents, faint and spread round it, under the text.
pub fn glow_text(ui: &mut Ui, text: &str, font: egui::FontId, colour: Color32) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, colour);
    let (rect, response) = ui.allocate_exact_size(galley.size() + vec2(4.0, 4.0), Sense::hover());
    let at = rect.min + vec2(2.0, 2.0);
    let painter = ui.painter();
    for (k, offset) in [vec2(-1.5, 0.0), vec2(1.5, 0.0), vec2(0.0, -1.5), vec2(0.0, 1.5), vec2(-1.0, -1.0), vec2(1.0, 1.0)].into_iter().enumerate() {
        let accent = if k % 2 == 0 { theme::ACCENT } else { theme::ACCENT_2 };
        painter.galley_with_override_text_color(at + offset, galley.clone(), accent.gamma_multiply(0.22));
    }
    painter.galley(at, galley, colour);
    response
}

/// A group header's text: the display face, capitals, spaced out, in the accent.
fn header_text(title: &str) -> RichText {
    RichText::new(title.to_uppercase()).font(theme::display(13.0)).extra_letter_spacing(1.2).color(theme::HEADER)
}

pub fn header(ui: &mut Ui, title: &str) {
    ui.label(header_text(title));
}

pub fn group_frame() -> Frame {
    Frame::new().fill(theme::GROUP_FILL).stroke(Stroke::new(1.0, theme::BORDER)).corner_radius(8).inner_margin(Margin::same(10))
}

/// A rounded, full-width group with a header that folds it (see `Folds`).
pub fn group(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui)) {
    group_frame().show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        // The group's frame is enough: no guide line for its contents (and no indent,
        // below).
        ui.visuals_mut().indent_has_left_vline = false;
        let open = Folds::is_open(ui, title);
        let response = egui::CollapsingHeader::new(header_text(title)).id_salt(("group", title)).open(Some(open)).icon(chevron).show_unindented(ui, |ui| {
            ui.add_space(2.0);
            add(ui);
        });
        if response.header_response.clicked() {
            Folds::set_open(ui, title, !open);
        }
    });
    ui.add_space(6.0);
}

/// A fold's chevron: pointing right when folded, turning down as it opens.
pub fn chevron(ui: &mut Ui, openness: f32, response: &egui::Response) {
    let colour = if response.hovered() { theme::TEXT } else { theme::HEADER };
    let c = response.rect.center();
    let r = 4.5;
    let angle = (1.0 - openness) * -std::f32::consts::FRAC_PI_2;
    let rot = egui::emath::Rot2::from_angle(angle);
    let points = [vec2(-r, -r * 0.5), vec2(0.0, r * 0.5), vec2(r, -r * 0.5)].map(|p| c + rot * p);
    ui.painter().line(points.to_vec(), Stroke::new(2.0, colour));
}

/// A folding section inside a group (the solvers' advanced settings): a header, the
/// chevron, and whether it's open (remembered like the groups).
pub fn fold_header(ui: &mut Ui, title: &str, note: &str) -> bool {
    let key = format!("section: {title}");
    let open = ui.data(|d| d.get_temp::<Folds>(Folds::id())).and_then(|f| f.0.get(&key).copied()).unwrap_or(false);
    let id = ui.make_persistent_id(("fold", title));
    let openness = anim::toward(ui.ctx(), id, open, anim::QUICK);
    let clicked = ui
        .horizontal(|ui| {
            let (rect, icon) = ui.allocate_exact_size(vec2(14.0, 18.0), Sense::click());
            chevron(ui, openness, &icon.clone().with_new_rect(rect));
            let text = ui.add(egui::Label::new(header_text(title)).sense(Sense::click()));
            if !note.is_empty() {
                ui.label(RichText::new(note).small().color(theme::TEXT_DIM));
            }
            icon.clicked() || text.clicked()
        })
        .inner;
    if clicked {
        Folds::set_open(ui, &key, !open);
    }
    open
}

/// One form row: `label` in the left column (wrapped if too long, with `help` on
/// hover), then whatever `add` puts in the right column.
pub fn row<R>(ui: &mut Ui, label: &str, help: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(|ui| {
        let height = ui.spacing().interact_size.y;
        let label = ui
            .allocate_ui_with_layout(vec2(LABEL_WIDTH, height), Layout::left_to_right(Align::Center), |ui| {
                ui.set_width(LABEL_WIDTH);
                ui.add(Label::new(RichText::new(label).color(theme::TEXT_DIM)).wrap())
            })
            .inner;
        if !help.is_empty() {
            label.on_hover_text(help);
        }
        add(ui)
    })
    .inner
}

/// An on/off setting: a switch (its knob sliding across, the track filling with the
/// accent), then its label. The whole row is clickable.
pub fn toggle(ui: &mut Ui, value: &mut bool, label: &str, help: &str) -> egui::Response {
    let track = vec2(38.0, 22.0);
    let galley = ui.painter().layout_no_wrap(label.to_string(), egui::TextStyle::Body.resolve(ui.style()), theme::TEXT);
    let width = (track.x + 10.0 + galley.size().x).min(ui.available_width().max(track.x));
    let (rect, mut response) = ui.allocate_exact_size(vec2(width, ui.spacing().interact_size.y), Sense::click());
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
    }
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), *value, label));
    let on = anim::toward(ui.ctx(), response.id, *value, anim::QUICK);
    let enabled = ui.is_enabled();
    let t = egui::Rect::from_min_size(egui::pos2(rect.min.x, rect.center().y - track.y / 2.0), track);
    let painter = ui.painter();
    let off_fill = if response.hovered() { theme::WIDGET_HOVER } else { theme::WIDGET };
    let fill = lerp_colour(off_fill, theme::ACCENT, on);
    let fade = |c: Color32| if enabled { c } else { c.gamma_multiply(0.4) };
    painter.rect(t, track.y / 2.0, fade(fill), Stroke::new(1.0, fade(if response.hovered() { theme::ACCENT } else { theme::BORDER })), egui::StrokeKind::Inside);
    let r = track.y / 2.0 - 3.0;
    let x = egui::lerp((t.min.x + 3.0 + r)..=(t.max.x - 3.0 - r), on);
    let knob = lerp_colour(theme::TEXT_DIM, theme::ON_ACCENT, on);
    painter.circle_filled(egui::pos2(x, t.center().y), r, fade(knob));
    let text_pos = egui::pos2(t.max.x + 10.0, rect.center().y - galley.size().y / 2.0);
    painter.galley(text_pos, galley, fade(theme::TEXT));
    if help.is_empty() { response } else { response.on_hover_text(help) }
}

/// An icon-only button, `ICON_BUTTON` big, that glows under the pointer.
pub fn icon_button(ui: &mut Ui, glyph: &str) -> egui::Response {
    icon_button_sized(ui, glyph, 18.0)
}

pub fn icon_button_sized(ui: &mut Ui, glyph: &str, size: f32) -> egui::Response {
    let glow = ui.painter().add(egui::Shape::Noop);
    let response = ui.add(egui::Button::new(RichText::new(glyph).size(size)).min_size(ICON_BUTTON));
    paint_glow(ui, glow, &response);
    response
}

/// How long the dice tumbles, seconds.
const TUMBLE_S: f32 = 0.6;

/// The randomize button's tumble (see `dice_button`): kick it to start.
pub fn dice_id() -> egui::Id {
    egui::Id::new("dice tumble")
}

/// The randomize button: a die that tumbles for a moment after `anim::kick(ctx,
/// dice_id())`, turning over twice as its face changes, and settles on five.
pub fn dice_button(ui: &mut Ui) -> egui::Response {
    use egui_phosphor::regular as icon;
    let glow = ui.painter().add(egui::Shape::Noop);
    let response = ui.add(egui::Button::new("").min_size(ICON_BUTTON));
    paint_glow(ui, glow, &response);
    let faces = [icon::DICE_ONE, icon::DICE_THREE, icon::DICE_SIX, icon::DICE_TWO, icon::DICE_FOUR, icon::DICE_FIVE];
    let (face, angle) = match anim::progress(ui.ctx(), dice_id(), TUMBLE_S) {
        Some(t) => {
            ui.ctx().request_repaint();
            (faces[((t * 9.0) as usize).min(faces.len() - 1)], std::f32::consts::TAU * 2.0 * anim::ease_out(t))
        }
        None => (icon::DICE_FIVE, 0.0),
    };
    let colour = ui.style().interact(&response).fg_stroke.color;
    let galley = ui.painter().layout_no_wrap(face.to_string(), egui::FontId::proportional(18.0), colour);
    // A text shape turns about its top-left corner: place that so it turns about the middle.
    let pos = response.rect.center() - egui::emath::Rot2::from_angle(angle) * (galley.size() / 2.0);
    ui.painter().add(egui::epaint::TextShape::new(pos, galley, colour).with_angle(angle));
    response
}

/// Fill the reserved `slot` with a soft accent glow round `response`, as it's hovered.
pub fn paint_glow(ui: &Ui, slot: egui::layers::ShapeIdx, response: &egui::Response) {
    let t = anim::toward(ui.ctx(), response.id.with("glow"), response.hovered() && ui.is_enabled(), anim::QUICK);
    if t > 0.0 {
        let shapes: Vec<egui::Shape> = (1..=3)
            .map(|k| {
                let spread = k as f32 * 2.0;
                egui::Shape::rect_filled(response.rect.expand(spread), 6.0 + spread, theme::ACCENT.gamma_multiply(0.07 * t))
            })
            .collect();
        ui.painter().set(slot, egui::Shape::Vec(shapes));
    }
}

/// A slider that eases to a value set from elsewhere (a look applied, an equation)
/// instead of jumping; dragging it follows the pointer at once.
pub fn eased_slider(ui: &mut Ui, value: &mut f32, range: std::ops::RangeInclusive<f32>) -> egui::Response {
    let id = ui.next_auto_id().with("eased");
    let mut shown = anim::value(ui.ctx(), id, *value, anim::SLOW);
    let response = ui.add(egui::Slider::new(&mut shown, range));
    if response.changed() {
        *value = shown;
        anim::value(ui.ctx(), id, shown, 0.0);
    }
    response
}

/// A tab bar: each tab an icon and a name in the display face; the chosen one bright,
/// with an accent underline that slides to it.
pub fn tabs<T: PartialEq + Copy>(ui: &mut Ui, current: &mut T, items: &[(T, &str, &str)]) {
    let mut chosen_rect = None;
    let id = ui.make_persistent_id("tab underline");
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for &(tab, glyph, name) in items {
            let selected = *current == tab;
            let colour = if selected { theme::TEXT } else { theme::TEXT_DIM };
            let text = RichText::new(format!("{glyph}  {}", name.to_uppercase())).font(theme::display(13.0)).extra_letter_spacing(1.0).color(colour);
            let response = ui.add(egui::Button::new(text).frame(false).min_size(vec2(84.0, 38.0)));
            if response.hovered() && !selected {
                ui.painter().line_segment([response.rect.left_bottom(), response.rect.right_bottom()], Stroke::new(1.0, theme::BORDER));
            }
            if response.clicked() {
                *current = tab;
            }
            if selected || response.clicked() {
                chosen_rect = Some(response.rect);
            }
        }
    });
    if let Some(rect) = chosen_rect {
        let x0 = anim::value(ui.ctx(), id.with(0), rect.min.x, anim::QUICK);
        let x1 = anim::value(ui.ctx(), id.with(1), rect.max.x, anim::QUICK);
        let y = rect.max.y;
        let bar = egui::Rect::from_min_max(egui::pos2(x0 + 4.0, y - 3.0), egui::pos2(x1 - 4.0, y));
        ui.painter().rect_filled(bar, 1.5, theme::ACCENT);
        ui.painter().rect_filled(bar.expand(2.0), 3.0, theme::ACCENT.gamma_multiply(0.12));
    }
}

/// `a` toward `b` by `t`.
pub fn lerp_colour(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let m = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgba_unmultiplied(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()), m(a.a(), b.a()))
}

/// Label and tooltip for a parameter, marked if changing it restarts the simulation.
pub fn param_text(label: &str, help: &str, restart: bool) -> (String, String) {
    if restart {
        (format!("{label} {RESTART_MARK}"), format!("{help}\n\n{RESTART_MARK} Changing this restarts the simulation."))
    } else {
        (label.to_string(), help.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_parameters_are_marked_in_label_and_tooltip() {
        let (label, help) = param_text("Grid cells", "Resolution.", true);
        assert!(label.ends_with(RESTART_MARK) && help.contains("restarts"));
        assert_eq!(param_text("CFL", "Travel.", false), ("CFL".to_string(), "Travel.".to_string()));
    }

    #[test]
    fn colours_blend() {
        assert_eq!(lerp_colour(Color32::BLACK, Color32::WHITE, 0.0), Color32::BLACK);
        assert_eq!(lerp_colour(Color32::BLACK, Color32::WHITE, 1.0), Color32::WHITE);
    }

    /// Groups start open unless listed as advanced; a fold sticks once made, and goes
    /// back with the settings.
    #[test]
    fn folds_are_remembered() {
        let ctx = egui::Context::default();
        let mut folds = Folds::default();
        folds.lend(&ctx);
        ctx.run_ui(egui::RawInput::default(), |ui| {
            assert!(Folds::is_open(ui, "Looks"));
            assert!(!Folds::is_open(ui, "Analysis"));
            Folds::set_open(ui, "Looks", false);
            assert!(!Folds::is_open(ui, "Looks"));
        })
        .textures_delta
        .clear();
        folds.take_back(&ctx);
        assert_eq!(folds.0.get("Looks"), Some(&false));
    }
}
