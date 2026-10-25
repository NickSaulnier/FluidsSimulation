//! Music tab: where the music comes from (a track file, or the computer's audio output
//! live), what the analyzer hears in it, and the reactions that drive the fluid with it.
//!
//! Laid out so nothing moves while music plays: every status message has a reserved
//! line (rather than appearing and pushing the rest down), buttons that change state keep
//! their size, and the reactions are a table of fixed-width columns. The groups are always
//! shown, with or without an audio source, so switching source doesn't move the
//! reactions either.

use crate::audio::Music;
use crate::audio::analyzer::{self, AudioFeatures, BAND_NAMES, OnsetKind};
use crate::audio::reactions::{Effect, Reaction, Trigger, default_reactions};
use crate::looks::LOOKS;
use crate::ui::Frame;
use crate::ui::params::live_float_params;
use crate::ui::shortcuts::{Action, tip};
use crate::ui::theme;
use crate::ui::widgets::{self, row};
use egui::{Color32, RichText, Sense, Ui, vec2};
use egui_phosphor::regular as icon;
use std::time::Duration;

/// Reactions table column widths, points.
const TRIGGER_W: f32 = 98.0;
const EFFECT_W: f32 = 80.0;
const GAIN_W: f32 = 30.0;
const BUTTON: egui::Vec2 = vec2(32.0, 24.0);
/// Both source cards take this height, so switching source doesn't move what's below.
const CARD_HEIGHT: f32 = 86.0;
const DIM: Color32 = Color32::from_gray(90);

pub fn show(ui: &mut Ui, f: &mut Frame<'_>) {
    source(ui, f.music);
    looks(ui, f);
    analysis(ui, f.music);
    reactions(ui, f);
}

/// Visualizer looks: pick one, and whether the director changes them by itself.
fn looks(ui: &mut Ui, f: &mut Frame<'_>) {
    let d = &mut f.state.director;
    widgets::group(ui, "Looks", |ui| {
        row(ui, "Look", "A tuned solver, scene, colours and reactions. Picking one fades to it.", |ui| {
            let current = d.current.map_or("Custom", |i| LOOKS[i].name);
            let mut pick = None;
            egui::ComboBox::from_id_salt("look").selected_text(current).width(150.0).truncate().show_ui(ui, |ui| {
                for (i, look) in LOOKS.iter().enumerate() {
                    if ui.selectable_label(d.current == Some(i), look.name).clicked() {
                        pick = Some(i);
                    }
                }
            });
            if let Some(i) = pick {
                d.request(i);
            }
        });
        widgets::toggle(
            ui,
            &mut d.auto,
            &tip("Change looks automatically", Action::ToggleAutoLooks),
            "In full screen, change look on new songs, drops, and at least every interval (on a kick).",
        );
        row(ui, "Change every", "The longest a look stays (in full screen, with automatic changes)", |ui| {
            ui.add(egui::Slider::new(&mut d.interval_s, 30.0..=300.0).suffix(" s").fixed_decimals(0));
        });
    });
}

/// The source switch, then the card for the chosen source.
fn source(ui: &mut Ui, music: &mut Music) {
    widgets::group(ui, "Source", |ui| {
        let live = music.is_live();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let half = vec2((ui.available_width() - 4.0) / 2.0, 24.0);
            let track = egui::Button::selectable(!live, format!("{} Track file", icon::FILE_AUDIO)).min_size(half);
            if ui.add(track).on_hover_text("Play an audio file dropped on the window.").clicked() {
                music.set_live(false);
            }
            let system = egui::Button::selectable(live, format!("{} System audio", icon::BROADCAST)).min_size(half);
            let hover = "Listen to whatever the computer is playing (Spotify, a browser, anything), \
                         including notification sounds.";
            if ui.add(system).on_hover_text(hover).clicked() {
                music.set_live(true);
            }
        });
        ui.add_space(4.0);
        ui.vertical(|ui| {
            ui.set_min_height(CARD_HEIGHT);
            if music.is_live() { live_card(ui, music) } else { track_card(ui, music) }
        });
    });
}

fn track_card(ui: &mut Ui, music: &mut Music) {
    let track = music.track.as_ref().map(|t| (t.name.clone(), t.duration));
    // Line 1: the track, or how to get one.
    let name = match &track {
        Some((name, _)) => RichText::new(name.as_str()),
        None => RichText::new("No track loaded").weak(),
    };
    ui.add(egui::Label::new(name).truncate());
    // Line 2, reserved: an error, loading, the time, or a hint.
    let status = if let Some(e) = &music.error {
        RichText::new(e.as_str()).color(theme::ERROR)
    } else if let Some(loading) = music.loading_name() {
        RichText::new(format!("Loading {loading}...")).weak()
    } else if let Some((_, duration)) = track {
        let pos = music.position().min(duration);
        RichText::new(format!("{} / {}", clock(pos), clock(duration))).weak()
    } else {
        RichText::new("Drop an audio file on the window.").weak()
    };
    ui.add(egui::Label::new(status).truncate());
    // Progress, and fixed-size transport buttons (play and pause swap icons, not width).
    let progress = track.as_ref().map_or(0.0, |(_, d)| music.position() / d.max(1e-3));
    bar(ui, ui.available_width(), 4.0, progress, true);
    ui.add_enabled_ui(track.is_some(), |ui| {
        ui.horizontal(|ui| {
            let (glyph, what) = if music.is_playing() { (icon::PAUSE, "Pause") } else { (icon::PLAY, "Play") };
            if icon_button(ui, glyph).on_hover_text(tip(what, Action::ToggleMusic)).clicked() {
                music.toggle_play();
            }
            if icon_button(ui, icon::ARROW_COUNTER_CLOCKWISE).on_hover_text("Restart").clicked() {
                music.restart();
            }
            let (glyph, what) = if music.muted { (icon::SPEAKER_X, "Unmute") } else { (icon::SPEAKER_HIGH, "Mute") };
            if icon_button(ui, glyph).on_hover_text(what).clicked() {
                music.muted = !music.muted;
                music.apply_volume();
            }
        });
    });
}

fn live_card(ui: &mut Ui, music: &mut Music) {
    let device = music.live.as_ref().map_or("", |l| l.device_name.as_str());
    ui.add(egui::Label::new(format!("Listening to {device}")).truncate());
    let features = music.features();
    let rms = features.map_or(0.0, |f| f.rms);
    row(ui, "Volume", "Raw loudness of the captured audio", |ui| {
        // Roughly -60..0 dBFS across the bar.
        let fill = ((20.0 * rms.max(1e-6).log10() + 60.0) / 60.0).clamp(0.0, 1.0);
        bar(ui, 140.0, 8.0, fill, true);
    });
    // Reserved line: an error, silence, or all well.
    let status = if let Some(e) = &music.error {
        RichText::new(e.as_str()).color(theme::ERROR)
    } else if features.is_none_or(|f| f.silent) {
        RichText::new("Silent: play something and the fluid will follow it.").weak()
    } else {
        RichText::new("Hearing audio.").weak()
    };
    ui.add(egui::Label::new(status).truncate());
}

/// What the analyzer hears. Always laid out the same: without audio the meters read zero
/// and dim, and the status line says why.
fn analysis(ui: &mut Ui, music: &mut Music) {
    let features = music.features().cloned();
    let active = features.as_ref().is_some_and(|f| !f.silent);
    let f = features.clone().unwrap_or_default();
    widgets::group(ui, "Analysis", |ui| {
        let status = match &features {
            None => "No audio source.",
            Some(f) if f.silent => "Silent.",
            Some(_) => "Listening.",
        };
        ui.add(egui::Label::new(RichText::new(status).weak()).truncate());
        bands(ui, &f, active);
        spectrum(ui, &f, active);
        let mut wave = Vec::new();
        music.waveform(&mut wave);
        scope(ui, &wave, active);
        row(ui, "Stereo", "Balance (left to right) and width (mono to wide)", |ui| {
            ui.label(RichText::new(format!("balance {:+.2} · width {:.2}", f.balance, f.width)).small());
        });
        row(ui, "MilkDrop", "Bass, mid and treble over their running averages (1 is typical for the song)", |ui| {
            let m = f.milk;
            ui.label(RichText::new(format!("bass {:.2} · mid {:.2} · treb {:.2}", m.bass_att, m.mid_att, m.treb_att)).small());
        });
        row(ui, "Level", "Overall loudness, relative to the last few seconds", |ui| bar(ui, 140.0, 8.0, f.level, active));
        row(ui, "Brightness", "Spectral centroid: dull bass to bright treble", |ui| {
            bar(ui, 140.0, 8.0, f.brightness, active)
        });
        row(ui, "Hits", "Kicks, snares and hi-hats detected since the source started", |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for (k, kind) in OnsetKind::ALL.into_iter().enumerate() {
                let flash = music.last_hit[k].is_some_and(|t| t.elapsed() < Duration::from_millis(150));
                let colour = if flash { theme::WARN } else { DIM };
                let text = RichText::new(format!("{} {}", kind.name().to_uppercase(), music.hits[k])).small().color(colour);
                ui.add_sized([50.0, 16.0], egui::Label::new(text).truncate());
            }
        });
        row(ui, "Selectivity", "Higher = only the clearest hits (kicks, snares and hi-hats alike).", |ui| {
            ui.add(egui::Slider::new(&mut music.sensitivity, 0.5..=4.0));
        });
    });
}

/// The log-spaced spectrum, 20 Hz to 16 kHz, as thin bars.
fn spectrum(ui: &mut Ui, f: &AudioFeatures, active: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2, theme::GROUP_FILL.gamma_multiply(1.6));
    let width = rect.width() / f.spectrum_smooth.len() as f32;
    let colour = if active { theme::ACCENT } else { DIM };
    for (k, level) in f.spectrum_smooth.iter().enumerate() {
        let x = rect.min.x + k as f32 * width;
        let top = rect.max.y - rect.height() * level.clamp(0.0, 1.0);
        painter.rect_filled(egui::Rect::from_min_max(egui::pos2(x, top), egui::pos2(x + width - 1.0, rect.max.y)), 0, colour);
    }
}

/// The latest waveform, left over right, as two traces.
fn scope(ui: &mut Ui, wave: &[analyzer::Frame], active: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2, theme::GROUP_FILL.gamma_multiply(1.6));
    if wave.len() < 2 {
        return;
    }
    let colours = if active { [theme::ACCENT, theme::WARN] } else { [DIM, DIM] };
    for (channel, colour) in colours.into_iter().enumerate() {
        let points: Vec<egui::Pos2> = wave
            .iter()
            .enumerate()
            .map(|(k, frame)| {
                let x = rect.min.x + rect.width() * k as f32 / (wave.len() - 1) as f32;
                egui::pos2(x, rect.center().y - 0.5 * rect.height() * frame[channel].clamp(-1.0, 1.0))
            })
            .collect();
        painter.add(egui::Shape::line(points, egui::Stroke::new(1.0, colour)));
    }
}

/// Five vertical band meters.
fn bands(ui: &mut Ui, f: &AudioFeatures, active: bool) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let width = ((ui.available_width() - 16.0) / 5.0).floor();
        for (level, name) in f.bands.iter().zip(BAND_NAMES) {
            ui.vertical(|ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(width, 40.0), Sense::hover());
                ui.painter().rect_filled(rect, 2, theme::GROUP_FILL.gamma_multiply(1.6));
                let height = rect.height() * level.clamp(0.0, 1.0);
                let lit = egui::Rect::from_min_max(egui::pos2(rect.min.x, rect.max.y - height), rect.max);
                ui.painter().rect_filled(lit, 2, if active { theme::ACCENT } else { DIM });
                ui.add_sized([width, 12.0], egui::Label::new(RichText::new(name).small().weak()));
            });
        }
    });
}

/// The reactions: an Intensity control, then a table of fixed-width columns (on,
/// trigger, effect, gain, remove), so rows line up and nothing shifts when a value
/// changes.
fn reactions(ui: &mut Ui, f: &mut Frame<'_>) {
    let params = live_float_params(f.world);
    let list = &mut f.state.reactions;
    widgets::group(ui, "Reactions", |ui| {
        row(ui, "Intensity", "Scales every reaction.", |ui| {
            ui.add(egui::Slider::new(&mut f.music.intensity, 0.2..=3.0).logarithmic(true));
        });
        ui.add_space(2.0);
        let mut remove = None;
        for (k, r) in list.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                ui.checkbox(&mut r.enabled, "").on_hover_text("On or off");
                fixed(ui, TRIGGER_W, |ui| {
                    egui::ComboBox::from_id_salt(("trigger", k))
                        .selected_text(r.trigger.label())
                        .width(TRIGGER_W)
                        .truncate()
                        .show_ui(ui, |ui| {
                            for t in Trigger::ALL {
                                ui.selectable_value(&mut r.trigger, t, t.label());
                            }
                        });
                });
                ui.label(RichText::new(icon::ARROW_RIGHT).small().weak());
                fixed(ui, EFFECT_W, |ui| {
                    egui::ComboBox::from_id_salt(("effect", k))
                        .selected_text(r.effect.label())
                        .width(EFFECT_W)
                        .truncate()
                        .show_ui(ui, |ui| {
                            for e in Effect::ALL {
                                ui.selectable_value(&mut r.effect, e, e.label()).on_hover_text(e.help());
                            }
                        })
                        .response
                        .on_hover_text(r.effect.help());
                });
                ui.add_sized(
                    [GAIN_W, 20.0],
                    egui::DragValue::new(&mut r.gain).range(0.0..=3.0).speed(0.01).fixed_decimals(1),
                )
                .on_hover_text("Gain");
                if ui.small_button(icon::X).on_hover_text("Remove").clicked() {
                    remove = Some(k);
                }
            });
            if r.effect == Effect::Param {
                // The parameter picker, indented under the trigger and effect columns.
                ui.horizontal(|ui| {
                    ui.add_space(ui.spacing().interact_size.y + 4.0);
                    let current = r
                        .param
                        .as_deref()
                        .and_then(|key| params.iter().find(|(k, _)| *k == key))
                        .map_or("Choose a setting...", |(_, l)| l);
                    egui::ComboBox::from_id_salt(("param", k))
                        .selected_text(current)
                        .width(TRIGGER_W + EFFECT_W + 20.0)
                        .truncate()
                        .show_ui(ui, |ui| {
                            for (key, label) in &params {
                                ui.selectable_value(&mut r.param, Some((*key).to_string()), *label);
                            }
                        })
                        .response
                        .on_hover_text("The solver setting this reaction modulates around its value");
                });
            }
        }
        if let Some(k) = remove {
            list.remove(k);
        }
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            if ui.button(format!("{} Add", icon::PLUS)).clicked() {
                list.push(Reaction::new(Trigger::Kick, Effect::Splash, 1.0, true));
            }
            if ui.button("Defaults").on_hover_text("Restore the starting set of reactions").clicked() {
                *list = default_reactions();
            }
        });
    });
}

/// A horizontal meter; dimmed when `active` is false.
fn bar(ui: &mut Ui, width: f32, height: f32, value: f32, active: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    ui.painter().rect_filled(rect, 2, theme::GROUP_FILL.gamma_multiply(1.6));
    let lit = egui::Rect::from_min_size(rect.min, vec2(rect.width() * value.clamp(0.0, 1.0), rect.height()));
    ui.painter().rect_filled(lit, 2, if active { theme::ACCENT } else { DIM });
}

/// Lay out `add` in a slot exactly `width` wide: combo boxes treat their width as a
/// minimum and grow to fit their text unless the space around them is capped.
fn fixed(ui: &mut Ui, width: f32, add: impl FnOnce(&mut Ui)) {
    let height = ui.spacing().interact_size.y;
    ui.allocate_ui_with_layout(vec2(width, height), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.set_width(width);
        add(ui);
    });
}

/// A fixed-size icon button.
fn icon_button(ui: &mut Ui, glyph: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(glyph).size(16.0)).min_size(BUTTON))
}

fn clock(seconds: f32) -> String {
    let s = seconds.max(0.0) as u32;
    format!("{}:{:02}", s / 60, s % 60)
}
