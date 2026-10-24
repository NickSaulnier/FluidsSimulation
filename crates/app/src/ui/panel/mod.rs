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
        let playing = f.music.is_playing() || f.music.is_live();
        let music = if playing { format!("{} Music •", icon::MUSIC_NOTES) } else { format!("{} Music", icon::MUSIC_NOTES) };
        for (tab, label) in [
            (Tab::Simulate, format!("{} Simulate", icon::SLIDERS)),
            (Tab::View, format!("{} View", icon::EYE)),
            (Tab::Music, music),
            (Tab::Info, format!("{} Info", icon::INFO)),
        ] {
            ui.selectable_value(&mut f.state.tab, tab, label);
        }
    });
    ui.separator();
    egui::ScrollArea::vertical().show(ui, |ui| match f.state.tab {
        Tab::Simulate => simulate::show(ui, f),
        Tab::View => view::show(ui, f),
        Tab::Music => music::show(ui, f.music),
        Tab::Info => info::show(ui, f),
    });
}
