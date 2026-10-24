//! Music tab: the loaded track and how its kicks drive the fluid.

use crate::audio::{KickAction, Music};
use std::time::Duration;

pub fn show(ui: &mut egui::Ui, music: &mut Music) {
    if let Some(name) = music.loading_name() {
        ui.label(format!("Loading {name}..."));
    }
    if let Some(e) = &music.error {
        ui.colored_label(egui::Color32::from_rgb(255, 120, 120), e);
    }
    let Some(track) = &music.track else {
        ui.weak("Drop an audio file on the window to make the water dance to its kick drums.");
        return;
    };
    let (name, duration, kicks) = (track.name.clone(), track.duration, track.kicks.len());
    let pos = music.position().min(duration);
    ui.label(format!("{name}  {}:{:02} / {}:{:02}", pos as u32 / 60, pos as u32 % 60, duration as u32 / 60, duration as u32 % 60));
    ui.horizontal(|ui| {
        if ui.button(if music.is_playing() { "Pause (M)" } else { "Play (M)" }).clicked() {
            music.toggle_play();
        }
        if ui.button("Restart").clicked() {
            music.restart();
        }
        if ui.checkbox(&mut music.muted, "Mute").changed() {
            music.apply_volume();
        }
        let flash = music.last_kick.is_some_and(|t| t.elapsed() < Duration::from_millis(150));
        let colour = if flash { egui::Color32::from_rgb(255, 210, 90) } else { egui::Color32::from_gray(70) };
        // Plain text: egui's default font has no "●" glyph.
        ui.colored_label(colour, "KICK");
    });
    ui.horizontal(|ui| {
        ui.radio_value(&mut music.action, KickAction::Splash, "Splash").on_hover_text("Kicks shove the water upward.");
        ui.radio_value(&mut music.action, KickAction::Jet, "Jet")
            .on_hover_text("Kicks fire jets from the floor (adds water; the Fountain preset drains it).");
    });
    ui.add(egui::Slider::new(&mut music.sensitivity, 0.5..=4.0).text("Selectivity"))
        .on_hover_text(format!("Higher = only the clearest kicks. {kicks} kicks detected."));
    ui.add(egui::Slider::new(&mut music.gain, 0.2..=3.0).text("Strength").logarithmic(true));
}
