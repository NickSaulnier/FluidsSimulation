//! Building blocks shared by the panel tabs: rounded groups with a header, and form rows
//! with the label in a fixed-width left column, so every widget lines up. Long labels
//! wrap rather than truncate; on/off settings are full-width checkboxes instead.

use super::theme;
use egui::{Align, Frame, Label, Layout, Margin, RichText, Ui, vec2};

/// Width of the label column in form rows.
pub const LABEL_WIDTH: f32 = 120.0;
/// Marks parameters whose change restarts the simulation.
pub const RESTART_MARK: &str = "⟳";

/// A title owning the groups below it (the solver's name above its parameters).
pub fn title(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).strong().size(16.0).color(theme::HEADER));
}

pub fn header(ui: &mut Ui, title: &str) {
    ui.label(RichText::new(title).strong().size(14.0).color(theme::HEADER));
}

pub fn group_frame() -> Frame {
    Frame::new().fill(theme::GROUP_FILL).corner_radius(6).inner_margin(Margin::same(8))
}

/// A rounded, full-width group with a header.
pub fn group<R>(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    let inner = group_frame()
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            header(ui, title);
            add(ui)
        })
        .inner;
    ui.add_space(4.0);
    inner
}

/// One form row: `label` in the left column (wrapped if too long, with `help` on
/// hover), then whatever `add` puts in the right column.
pub fn row<R>(ui: &mut Ui, label: &str, help: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(|ui| {
        let height = ui.spacing().interact_size.y;
        let label = ui
            .allocate_ui_with_layout(vec2(LABEL_WIDTH, height), Layout::left_to_right(Align::Center), |ui| {
                ui.set_width(LABEL_WIDTH);
                ui.add(Label::new(label).wrap())
            })
            .inner;
        if !help.is_empty() {
            label.on_hover_text(help);
        }
        add(ui)
    })
    .inner
}

/// A full-width checkbox row, for on/off settings.
pub fn toggle(ui: &mut Ui, value: &mut bool, label: &str, help: &str) -> egui::Response {
    let r = ui.checkbox(value, label);
    if help.is_empty() { r } else { r.on_hover_text(help) }
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
}
