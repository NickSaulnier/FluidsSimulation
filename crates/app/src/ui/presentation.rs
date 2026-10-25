//! Presentation mode's only UI: an overlay along the bottom that appears when the mouse
//! moves (and stays while it's hovered), and brief notices along the top.

use super::shortcuts::{Action, tip};
use super::{Frame, theme, top_bar};
use egui::{Align2, Color32, Frame as EguiFrame, Margin, RichText, vec2};
use egui_phosphor::regular as icon;

const INSET: f32 = 16.0;

pub fn overlay(ctx: &egui::Context, canvas: egui::Rect, f: &mut Frame<'_>) {
    egui::Area::new(egui::Id::new("presentation_overlay"))
        .order(egui::Order::Foreground)
        .pivot(Align2::CENTER_BOTTOM)
        .fixed_pos(canvas.center_bottom() - vec2(0.0, INSET))
        .show(ctx, |ui| {
            panel_frame().show(ui, |ui| {
                ui.horizontal(|ui| {
                    let exit = egui::Button::new(RichText::new(format!("{} Exit full screen", icon::CORNERS_IN)));
                    if ui.add(exit).on_hover_text(tip("Leave the visualizer (Esc also works)", Action::TogglePresentation)).clicked() {
                        f.state.presenting = false;
                    }
                    ui.separator();
                    top_bar::scene_and_solver(ui, f);
                    ui.separator();
                    music(ui, f);
                    ui.separator();
                    if ui.button(icon::KEYBOARD).on_hover_text(tip("Keyboard and mouse shortcuts", Action::ToggleCheatSheet)).clicked() {
                        f.state.cheat_sheet = !f.state.cheat_sheet;
                    }
                });
            });
        });
}

/// What's driving the reactions, and a way to start listening if nothing is.
fn music(ui: &mut egui::Ui, f: &mut Frame<'_>) {
    let m = &mut *f.music;
    if m.is_live() {
        ui.label(format!("{} Live audio", icon::BROADCAST));
    } else if let (true, Some(track)) = (m.is_playing(), &m.track) {
        ui.add(egui::Label::new(format!("{} {}", icon::MUSIC_NOTES, track.name)).truncate());
    } else if ui.button(format!("{} Listen to system audio", icon::BROADCAST)).clicked() {
        m.set_live(true);
    }
}

/// A notice along the top of the canvas (e.g. the resolution was lowered).
pub fn notice(ctx: &egui::Context, canvas: egui::Rect, text: &str) {
    egui::Area::new(egui::Id::new("presentation_notice"))
        .order(egui::Order::Foreground)
        .pivot(Align2::CENTER_TOP)
        .fixed_pos(canvas.center_top() + vec2(0.0, INSET))
        .interactable(false)
        .show(ctx, |ui| {
            panel_frame().show(ui, |ui| {
                ui.label(RichText::new(text).color(theme::HEADER));
            });
        });
}

fn panel_frame() -> EguiFrame {
    EguiFrame::new().fill(Color32::from_rgba_unmultiplied(20, 22, 26, 215)).corner_radius(10).inner_margin(Margin::same(10))
}
