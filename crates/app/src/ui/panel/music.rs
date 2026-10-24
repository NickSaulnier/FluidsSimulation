//! Music tab: the loaded track and how its kicks drive the fluid.

use crate::audio::{KickAction, Music};
use crate::ui::shortcuts::{Action, tip};
use crate::ui::theme;
use crate::ui::widgets::{self, row};
use std::time::Duration;

pub fn show(ui: &mut egui::Ui, music: &mut Music) {
    widgets::group(ui, "Track", |ui| {
        if let Some(name) = music.loading_name() {
            ui.label(format!("Loading {name}..."));
        }
        if let Some(e) = &music.error {
            ui.colored_label(theme::ERROR, e);
        }
        let Some(track) = &music.track else {
            ui.weak("Drop an audio file on the window to make the water dance to its kick drums.");
            return;
        };
        let (name, duration) = (track.name.clone(), track.duration);
        let pos = music.position().min(duration);
        ui.label(&name);
        ui.weak(format!("{}:{:02} / {}:{:02}", pos as u32 / 60, pos as u32 % 60, duration as u32 / 60, duration as u32 % 60));
        ui.horizontal(|ui| {
            let label = if music.is_playing() { "Pause" } else { "Play" };
            if ui.button(label).on_hover_text(tip(label, Action::ToggleMusic)).clicked() {
                music.toggle_play();
            }
            if ui.button("Restart").clicked() {
                music.restart();
            }
            if ui.checkbox(&mut music.muted, "Mute").changed() {
                music.apply_volume();
            }
        });
    });
    let Some(track) = &music.track else { return };
    let kicks = track.kicks.len();
    widgets::group(ui, "Kicks", |ui| {
        row(ui, "Detected", "", |ui| {
            ui.label(kicks.to_string());
            let flash = music.last_kick.is_some_and(|t| t.elapsed() < Duration::from_millis(150));
            let colour = if flash { theme::WARN } else { egui::Color32::from_gray(70) };
            // Plain text: egui's default font has no "●" glyph.
            ui.colored_label(colour, "KICK");
        });
        row(ui, "Kicks make", "", |ui| {
            ui.radio_value(&mut music.action, KickAction::Splash, "Splash").on_hover_text("Kicks shove the water upward.");
            ui.radio_value(&mut music.action, KickAction::Jet, "Jet")
                .on_hover_text("Kicks fire jets from the floor (adds water; the Fountain preset drains it).");
        });
        row(ui, "Selectivity", "Higher = only the clearest kicks.", |ui| {
            ui.add(egui::Slider::new(&mut music.sensitivity, 0.5..=4.0));
        });
        row(ui, "Strength", "How hard each kick pushes.", |ui| {
            ui.add(egui::Slider::new(&mut music.gain, 0.2..=3.0).logarithmic(true));
        });
    });
}
