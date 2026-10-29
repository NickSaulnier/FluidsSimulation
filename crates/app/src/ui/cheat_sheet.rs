//! The F1 cheat sheet: every shortcut and mouse action, over the canvas.

use super::shortcuts::{Group, MOUSE, SHORTCUTS};
use super::widgets;
use egui::{Align2, Margin, RichText};

pub fn show(ctx: &egui::Context, canvas: egui::Rect, open: &mut bool) {
    egui::Area::new(egui::Id::new("cheat_sheet"))
        .order(egui::Order::Foreground)
        .pivot(Align2::CENTER_CENTER)
        .fixed_pos(canvas.center())
        .show(ctx, |ui| {
            super::presentation::panel_frame()
                .corner_radius(10)
                .inner_margin(Margin::same(18))
                .show(ui, |ui| {
                    widgets::title(ui, "Keyboard and mouse");
                    ui.add_space(6.0);
                    ui.horizontal_top(|ui| {
                        for (group, title) in [(Group::Simulation, "Simulation"), (Group::Tools, "Tools"), (Group::View, "View"), (Group::Visualizer, "In full screen")] {
                            ui.vertical(|ui| {
                                widgets::header(ui, title);
                                egui::Grid::new(title).num_columns(2).spacing([14.0, 4.0]).show(ui, |ui| {
                                    for s in SHORTCUTS.iter().filter(|s| s.group == group) {
                                        key_cell(ui, s.keys);
                                        ui.label(s.what);
                                        ui.end_row();
                                    }
                                });
                            });
                            ui.add_space(18.0);
                        }
                    });
                    ui.add_space(8.0);
                    widgets::header(ui, "Mouse");
                    egui::Grid::new("mouse").num_columns(2).spacing([14.0, 4.0]).show(ui, |ui| {
                        for (keys, what) in MOUSE {
                            key_cell(ui, keys);
                            ui.label(*what);
                            ui.end_row();
                        }
                    });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.weak("F1 or Esc closes this.");
                        if ui.small_button("Close").clicked() {
                            *open = false;
                        }
                    });
                });
        });
}

fn key_cell(ui: &mut egui::Ui, keys: &str) {
    ui.label(RichText::new(keys).strong().color(super::theme::ACCENT));
}
