//! The side panel: one tab at a time instead of one long scrolling column.

mod info;
mod music;
mod simulate;
mod view;

use super::{Frame, Tab};
use egui_phosphor::regular as icon;

pub fn show(ui: &mut egui::Ui, f: &mut Frame<'_>) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        // Fixed labels: a tab title that changes width would shift the tabs after it.
        // (Whether music is playing shows in the status bar.)
        for (tab, label) in [
            (Tab::Simulate, format!("{} Simulate", icon::SLIDERS)),
            (Tab::View, format!("{} View", icon::EYE)),
            (Tab::Music, format!("{} Music", icon::MUSIC_NOTES)),
            (Tab::Info, format!("{} Info", icon::INFO)),
        ] {
            ui.selectable_value(&mut f.state.tab, tab, label);
        }
    });
    ui.separator();
    egui::ScrollArea::vertical().show(ui, |ui| match f.state.tab {
        Tab::Simulate => simulate::show(ui, f),
        Tab::View => view::show(ui, f),
        Tab::Music => music::show(ui, f),
        Tab::Info => info::show(ui, f),
    });
}
