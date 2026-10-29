//! Presentation mode's only UI: a bar along the bottom that rises into view when the
//! mouse moves (and stays while it's hovered), and brief notices that slide down from
//! the top.

use super::shortcuts::{Action, tip};
use super::{Frame, anim, theme, top_bar, widgets};
use egui::{Align2, Color32, Frame as EguiFrame, Margin, RichText, Stroke, vec2};
use egui_phosphor::regular as icon;

const INSET: f32 = 16.0;
/// How far the bar rises and notices drop as they appear, points.
const RISE: f32 = 70.0;
const DROP: f32 = 24.0;

/// The bar, `shown` of the way into view (0 hidden, 1 in place).
pub fn overlay(ctx: &egui::Context, canvas: egui::Rect, f: &mut Frame<'_>, shown: f32) {
    egui::Area::new(egui::Id::new("presentation_overlay"))
        .order(egui::Order::Foreground)
        .pivot(Align2::CENTER_BOTTOM)
        .fixed_pos(canvas.center_bottom() - vec2(0.0, INSET - (1.0 - shown) * RISE))
        .show(ctx, |ui| {
            ui.multiply_opacity(shown);
            panel_frame().show(ui, |ui| {
                ui.horizontal(|ui| {
                    let exit = egui::Button::new(RichText::new(format!("{} Exit full screen", icon::CORNERS_IN)));
                    if ui.add(exit).on_hover_text(tip("Leave the visualizer (Esc also works)", Action::TogglePresentation)).clicked() {
                        f.state.presenting = false;
                    }
                    ui.separator();
                    looks(ui, f);
                    ui.separator();
                    top_bar::scene_and_solver(ui, f);
                    ui.separator();
                    music(ui, f);
                    ui.separator();
                    if f.state.monitors.len() > 1 && widgets::icon_button(ui, icon::MONITOR).on_hover_text(tip("Move to the next display", Action::NextDisplay)).clicked() {
                        f.state.next_display();
                    }
                    if widgets::icon_button(ui, icon::KEYBOARD).on_hover_text(tip("Keyboard and mouse shortcuts", Action::ToggleCheatSheet)).clicked() {
                        f.state.cheat_sheet = !f.state.cheat_sheet;
                    }
                });
            });
        });
}

/// The look on screen, with previous / next and the automatic-change lock.
fn looks(ui: &mut egui::Ui, f: &mut Frame<'_>) {
    let (d, looks) = (&mut f.state.director, &f.state.looks.looks);
    if widgets::icon_button(ui, icon::CARET_LEFT).on_hover_text(tip("Previous look", Action::PreviousLook)).clicked() {
        d.request(d.neighbour(-1, looks.len()));
    }
    let name = d.current.and_then(|i| looks.get(i)).map_or("Custom", |l| l.name.as_str());
    sliding_name(ui, "full screen", name, vec2(190.0, 30.0));
    if widgets::icon_button(ui, icon::CARET_RIGHT).on_hover_text(tip("Next look", Action::NextLook)).clicked() {
        d.request(d.neighbour(1, looks.len()));
    }
    if widgets::icon_button(ui, icon::SQUARES_FOUR).on_hover_text(tip("Browse the looks", Action::ToggleBrowser)).clicked() {
        f.state.browser_open = !f.state.browser_open;
    }
    let d = &mut f.state.director;
    let (glyph, what) = if d.auto {
        (icon::LOCK_OPEN, "Looks change with the music: click to keep this one")
    } else {
        (icon::LOCK, "This look stays: click to let looks change with the music")
    };
    // The lock snaps (a quick pop of its icon) when it changes.
    let id = egui::Id::new("lock snap");
    let pop = anim::progress(ui.ctx(), id, 0.3).map_or(0.0, |t| (t * std::f32::consts::PI).sin());
    if widgets::icon_button_sized(ui, glyph, 18.0 + 5.0 * pop).on_hover_text(tip(what, Action::ToggleAutoLooks)).clicked() {
        d.auto = !d.auto;
        anim::kick(ui.ctx(), id);
    }
}

/// The look's name in the display face, `size` big; when it changes, the old name
/// slides out to the left as the new one slides in from the right.
pub fn sliding_name(ui: &mut egui::Ui, id_salt: &str, name: &str, size: egui::Vec2) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    let id = egui::Id::new(("look name", id_salt));
    let shown: Option<String> = ui.data(|d| d.get_temp(id));
    if shown.as_deref() != Some(name) {
        ui.data_mut(|d| {
            d.insert_temp(id.with("old"), shown.unwrap_or_default());
            d.insert_temp(id, name.to_string());
        });
        anim::kick(ui.ctx(), id);
    }
    let painter = ui.painter_at(rect);
    let font = theme::display(14.0);
    let draw = |text: &str, dx: f32, alpha: f32| {
        let galley = painter.layout_no_wrap(text.to_string(), font.clone(), theme::TEXT);
        let pos = egui::pos2(rect.center().x - galley.size().x.min(rect.width()) / 2.0 + dx, rect.center().y - galley.size().y / 2.0);
        painter.galley(pos, galley, theme::TEXT.gamma_multiply(alpha));
    };
    match anim::progress(ui.ctx(), id, anim::SLOW) {
        Some(t) => {
            let e = anim::ease_out(t);
            let old: String = ui.data(|d| d.get_temp(id.with("old"))).unwrap_or_default();
            draw(&old, -30.0 * e, 1.0 - e);
            draw(name, 30.0 * (1.0 - e), e);
        }
        None => draw(name, 0.0, 1.0),
    }
    response.on_hover_text(name)
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

/// A notice along the top of the canvas (e.g. the resolution was lowered), `shown` of
/// the way in: it drops into place as it fades in, and rises as it fades out.
pub fn notice(ctx: &egui::Context, canvas: egui::Rect, text: &str, shown: f32) {
    egui::Area::new(egui::Id::new("presentation_notice"))
        .order(egui::Order::Foreground)
        .pivot(Align2::CENTER_TOP)
        .fixed_pos(canvas.center_top() + vec2(0.0, INSET - (1.0 - shown) * DROP))
        .interactable(false)
        .show(ctx, |ui| {
            ui.multiply_opacity(shown);
            panel_frame().show(ui, |ui| {
                ui.label(RichText::new(text).color(theme::TEXT));
            });
        });
}

pub fn panel_frame() -> EguiFrame {
    EguiFrame::new()
        .fill(Color32::from_rgba_unmultiplied(11, 12, 16, 232))
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(12)
        .inner_margin(Margin::same(10))
        .shadow(egui::Shadow { offset: [0, 6], blur: 20, spread: 0, color: Color32::from_black_alpha(140) })
}
