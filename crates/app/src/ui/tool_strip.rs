//! The tool strip: a narrow column on the canvas's left edge, as in paint programs.
//! The four tools on top, then vertical sliders for the brush size and, for tools that
//! have one, the tool's own setting (push force, faucet speed).

use super::shortcuts::{Action, tip};
use super::{BRUSH_RADIUS, Tool, UiState, theme};
use egui::{Align2, Color32, Frame, Margin, RichText, vec2};
use egui_phosphor::regular as icon;

/// Gap between the canvas edge and the strip, points.
const INSET: f32 = 10.0;
const SLIDER_HEIGHT: f32 = 110.0;
/// Content width. Fixed: centred layouts inside a free-floating area otherwise claim
/// the whole window's width.
const WIDTH: f32 = 52.0;
const MARGIN: i8 = 6;
/// Canvas width the strip occupies, which the scene is fitted to leave free.
pub const RESERVED_WIDTH: f32 = WIDTH + 2.0 * MARGIN as f32 + 2.0 * INSET;

pub const TOOLS: [(Tool, &str, &str); 4] = [
    (Tool::Push, icon::HAND, "Push: drag to push the fluid (right drag always pushes)"),
    (Tool::PaintWall, icon::WALL, "Wall: drag to paint walls"),
    (Tool::EraseWall, icon::ERASER, "Erase: drag to erase walls"),
    (Tool::Faucet, icon::DROP, "Faucet: hold to pour, along the drag direction"),
];
/// In 3D the left button orbits, so the tools are on the right button; walls are
/// shapes in the scene, not painted.
pub const TOOLS_3D: [(Tool, &str, &str); 2] = [
    (Tool::Push, icon::HAND, "Push: right drag to push the gas (left drag orbits)"),
    (Tool::Faucet, icon::FIRE, "Torch: right drag to hold a burner, pointing along the drag"),
];

/// The strip, with the 3D tools when `in_3d`.
pub fn show(ctx: &egui::Context, canvas: egui::Rect, s: &mut UiState, in_3d: bool) {
    if in_3d && matches!(s.tool, Tool::PaintWall | Tool::EraseWall) {
        s.tool = Tool::Push;
    }
    let tools: &[(Tool, &str, &str)] = if in_3d { &TOOLS_3D } else { &TOOLS };
    egui::Area::new(egui::Id::new("tool_strip"))
        .order(egui::Order::Middle)
        .pivot(Align2::LEFT_TOP)
        .fixed_pos(canvas.min + vec2(INSET, INSET))
        .show(ctx, |ui| {
            Frame::new()
                .fill(Color32::from_rgba_unmultiplied(28, 30, 35, 225))
                .corner_radius(8)
                .inner_margin(Margin::same(MARGIN))
                .show(ui, |ui| {
                    ui.set_width(WIDTH);
                    ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                    ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                        for &(tool, glyph, help) in tools {
                            let button = egui::Button::selectable(s.tool == tool, RichText::new(glyph).size(20.0))
                                .min_size(vec2(36.0, 34.0));
                            if ui.add(button).on_hover_text(tip(help, Action::SelectTool(tool))).clicked() {
                                s.tool = tool;
                            }
                        }
                        ui.separator();
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().slider_width = SLIDER_HEIGHT;
                            ui.spacing_mut().item_spacing.x = 0.0;
                            slider(ui, "size", &mut s.brush_radius, BRUSH_RADIUS, true, "Brush radius, m ([ and ])", 2);
                            match s.tool {
                                Tool::Push => {
                                    slider(ui, "force", &mut s.push_strength, 2.0..=80.0, true, "Push strength", 0);
                                }
                                Tool::Faucet => {
                                    let help = if in_3d { "Torch speed, m/s" } else { "Faucet speed, m/s" };
                                    slider(ui, "flow", &mut s.faucet_speed, 0.2..=6.0, false, help, 1);
                                }
                                Tool::PaintWall | Tool::EraseWall => {}
                            }
                        });
                    });
                });
        });
}

/// A vertical slider with a caption above and its value below.
fn slider(
    ui: &mut egui::Ui,
    caption: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    log: bool,
    help: &str,
    decimals: usize,
) {
    ui.allocate_ui_with_layout(vec2(WIDTH / 2.0, 0.0), egui::Layout::top_down(egui::Align::Center), |ui| {
        ui.set_width(WIDTH / 2.0);
        ui.label(RichText::new(caption).small().weak());
        ui.add(egui::Slider::new(value, range).vertical().logarithmic(log).show_value(false)).on_hover_text(help);
        ui.label(RichText::new(format!("{value:.decimals$}")).small().color(theme::HEADER));
    });
}
