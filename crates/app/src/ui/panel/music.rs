//! Music tab: where the music comes from (a track file, or the computer's audio output
//! live), what the analyzer hears in it, and how its kicks drive the fluid.

use crate::audio::analyzer::{BAND_NAMES, OnsetKind};
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
    if music.features().is_some() {
        analysis(ui, music);
        kicks(ui, music);
    }
}

/// What the analyzer hears: band levels, overall level and brightness, and hits.
fn analysis(ui: &mut egui::Ui, music: &Music) {
    let Some(f) = music.features() else { return };
    widgets::group(ui, "Analysis", |ui| {
        // Five vertical band meters, normalised by the analyzer's gain control.
        ui.horizontal(|ui| {
            for (level, name) in f.bands.iter().zip(BAND_NAMES) {
                ui.vertical(|ui| {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(40.0, 46.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 2, theme::GROUP_FILL.gamma_multiply(1.6));
                    let height = rect.height() * level.clamp(0.0, 1.0);
                    let lit = egui::Rect::from_min_max(egui::pos2(rect.min.x, rect.max.y - height), rect.max);
                    ui.painter().rect_filled(lit, 2, theme::ACCENT);
                    ui.add_sized([40.0, 12.0], egui::Label::new(egui::RichText::new(name).small().weak()));
                });
            }
        });
        meter(ui, "Level", "Overall loudness, relative to the last few seconds", f.level);
        meter(ui, "Brightness", "Spectral centroid: dull bass to bright treble", f.brightness);
        row(ui, "Hits", "Kicks, snares and hi-hats detected since the source started", |ui| {
            for (k, kind) in OnsetKind::ALL.into_iter().enumerate() {
                let flash = music.last_hit[k].is_some_and(|t| t.elapsed() < Duration::from_millis(150));
                let colour = if flash { theme::WARN } else { egui::Color32::from_gray(90) };
                ui.colored_label(colour, format!("{} {}", kind.name().to_uppercase(), music.hits[k]));
            }
        });
        if f.silent {
            ui.weak("Silent.");
        }
    });
}

fn meter(ui: &mut egui::Ui, label: &str, help: &str, value: f32) {
    row(ui, label, help, |ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(140.0, 8.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 2, theme::GROUP_FILL.gamma_multiply(1.6));
        let lit = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() * value.clamp(0.0, 1.0), rect.height()));
        ui.painter().rect_filled(lit, 2, theme::ACCENT);
    });
}

fn live_input(ui: &mut egui::Ui, music: &mut Music) {
    let Some(input) = &music.live else { return };
    let rms = music.features().map_or(0.0, |f| f.rms);
    widgets::group(ui, "Live input", |ui| {
        ui.label(format!("Listening to {}", input.device_name));
        row(ui, "Volume", "Raw loudness of the captured audio", |ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(140.0, 8.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 2, theme::GROUP_FILL.gamma_multiply(1.6));
            // Roughly -60..0 dBFS across the bar.
            let db = 20.0 * rms.max(1e-6).log10();
            let fill = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
            let lit = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() * fill, rect.height()));
            ui.painter().rect_filled(lit, 2, theme::ACCENT);
        });
        if music.features().is_none_or(|f| f.silent) {
            ui.weak("Silent: play something (Spotify, a video...) and the fluid will follow it.");
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
    widgets::group(ui, "Kick reaction", |ui| {
        row(ui, "Kicks make", "", |ui| {
            ui.radio_value(&mut music.action, KickAction::Splash, "Splash").on_hover_text("Kicks shove the water upward.");
            ui.radio_value(&mut music.action, KickAction::Jet, "Jet")
                .on_hover_text("Kicks fire jets from the floor (adds water; the Fountain preset drains it).");
        });
        row(ui, "Selectivity", "Higher = only the clearest hits (kicks, snares and hi-hats alike).", |ui| {
            ui.add(egui::Slider::new(&mut music.sensitivity, 0.5..=4.0));
        });
        row(ui, "Strength", "How hard each kick pushes.", |ui| {
            ui.add(egui::Slider::new(&mut music.gain, 0.2..=3.0).logarithmic(true));
        });
    });
}
