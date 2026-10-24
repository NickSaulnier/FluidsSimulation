//! Music tab: where the beat comes from (a track file, or the computer's audio output
//! live), and how its kicks drive the fluid.

use crate::audio::{KickAction, Music};
use crate::ui::shortcuts::{Action, tip};
use crate::ui::theme;
use crate::ui::widgets::{self, row};
use std::time::Duration;

pub fn show(ui: &mut egui::Ui, music: &mut Music) {
    widgets::group(ui, "Source", |ui| {
        let mut live = music.is_live();
        ui.radio_value(&mut live, false, "Track file").on_hover_text("Play an audio file dropped on the window.");
        ui.radio_value(&mut live, true, "System audio (live)")
            .on_hover_text("Listen to whatever the computer is playing: Spotify, a browser, anything.");
        music.set_live(live);
    });
    if music.is_live() {
        live_input(ui, music);
    } else {
        track(ui, music);
    }
    if music.track.is_some() || music.is_live() {
        kicks(ui, music);
    }
}

fn live_input(ui: &mut egui::Ui, music: &mut Music) {
    let Some(input) = &music.live else { return };
    widgets::group(ui, "Live input", |ui| {
        ui.label(format!("Listening to {}", input.device_name));
        row(ui, "Level", "Loudness of the captured audio", |ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(120.0, 8.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 2, theme::GROUP_FILL.gamma_multiply(1.6));
            // Roughly -60..0 dBFS across the bar.
            let db = 20.0 * input.level.max(1e-6).log10();
            let fill = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
            let lit = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() * fill, rect.height()));
            ui.painter().rect_filled(lit, 2, theme::ACCENT);
        });
        if input.level < 1e-4 {
            ui.weak("Silent: play something (Spotify, a video...) and the fluid will follow its kicks.");
        }
        ui.weak("Reacts to everything the computer plays, including notification sounds.");
    });
    if let Some(e) = &music.error {
        ui.colored_label(theme::ERROR, e);
    }
}

fn track(ui: &mut egui::Ui, music: &mut Music) {
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
}

fn kicks(ui: &mut egui::Ui, music: &mut Music) {
    let detected = match (&music.live, &music.track) {
        (Some(input), _) => input.kicks_detected,
        (None, Some(track)) => track.kicks.len(),
        (None, None) => 0,
    };
    widgets::group(ui, "Kicks", |ui| {
        row(ui, "Detected", "", |ui| {
            ui.label(detected.to_string());
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
