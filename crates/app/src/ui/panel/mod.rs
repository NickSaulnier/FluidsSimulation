//! The side panel: one tab at a time instead of one long scrolling column.

mod info;
mod music;
mod simulate;
mod view;

use super::{Frame, Tab, widgets};
use egui_phosphor::regular as icon;

pub fn show(ui: &mut egui::Ui, f: &mut Frame<'_>) {
    ui.add_space(6.0);
    // Fixed labels: a tab title that changes width would shift the tabs after it. The
    // music and its looks first: it's what the app is mostly used for.
    let tabs = [(Tab::Music, icon::MUSIC_NOTES, "Music"), (Tab::View, icon::EYE, "View"), (Tab::Simulate, icon::SLIDERS, "Fluid"), (Tab::Info, icon::INFO, "Info")];
    widgets::tabs(ui, &mut f.state.tab, &tabs);
    ui.add_space(6.0);
    egui::ScrollArea::vertical().show(ui, |ui| match f.state.tab {
        Tab::Simulate => simulate::show(ui, f),
        Tab::View => view::show(ui, f),
        Tab::Music => music::show(ui, f),
        Tab::Info => info::show(ui, f),
    });
}
