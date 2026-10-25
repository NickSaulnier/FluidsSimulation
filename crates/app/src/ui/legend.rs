//! The overlay legend, on the canvas's bottom-right corner: a colour bar drawn with the
//! same colour map as the overlay itself, with its range and unit, or swatches for
//! categorical fields.

use crate::render::{Palette, colormap};
use egui::{Align2, Color32, Frame, Margin, RichText, Sense, vec2};
use sim::{FieldInfo, FieldKind};

const INSET: f32 = 10.0;
const BAR: egui::Vec2 = vec2(180.0, 12.0);
const SAMPLES: usize = 64;

pub fn show(ctx: &egui::Context, canvas: egui::Rect, field: &FieldInfo, (lo, hi): (f32, f32), palette: &Palette) {
    egui::Area::new(egui::Id::new("legend"))
        .order(egui::Order::Middle)
        .pivot(Align2::RIGHT_BOTTOM)
        .fixed_pos(canvas.max - vec2(INSET, INSET))
        .interactable(false)
        .show(ctx, |ui| {
            Frame::new()
                .fill(Color32::from_rgba_unmultiplied(28, 30, 35, 225))
                .corner_radius(8)
                .inner_margin(Margin::same(8))
                .show(ui, |ui| {
                    let title = if field.unit.is_empty() { field.label.to_string() } else { format!("{} ({})", field.label, field.unit) };
                    ui.label(RichText::new(title).small().strong());
                    if field.kind == FieldKind::Categorical {
                        swatches(ui, (lo, hi));
                    } else {
                        bar(ui, field.kind, (lo, hi), &palette.dye_ramp());
                    }
                });
        });
}

fn bar(ui: &mut egui::Ui, kind: FieldKind, (lo, hi): (f32, f32), dye: &[[u8; 3]; 3]) {
    let values: Vec<f32> = (0..SAMPLES).map(|k| lo + (hi - lo) * k as f32 / (SAMPLES - 1) as f32).collect();
    let mut rgba = Vec::new();
    colormap::colourise(&values, kind, (lo, hi), dye, &mut rgba);
    let (rect, _) = ui.allocate_exact_size(BAR, Sense::hover());
    let painter = ui.painter();
    // Transparent ends (dye) read against the canvas background, as on the canvas.
    painter.rect_filled(rect, 2, Color32::from_rgb(28, 31, 40));
    let w = rect.width() / SAMPLES as f32;
    for (k, c) in rgba.as_chunks::<4>().0.iter().enumerate() {
        let x = rect.min.x + k as f32 * w;
        let cell = egui::Rect::from_min_max(egui::pos2(x, rect.min.y), egui::pos2(x + w + 0.5, rect.max.y));
        painter.rect_filled(cell, 0, Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]));
    }
    ui.horizontal(|ui| {
        ui.set_width(BAR.x);
        ui.label(RichText::new(format_value(lo)).small());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(format_value(hi)).small());
            if kind == FieldKind::Signed {
                ui.add_space(BAR.x / 2.0 - 30.0);
                ui.label(RichText::new("0").small());
            }
        });
    });
}

fn swatches(ui: &mut egui::Ui, (lo, hi): (f32, f32)) {
    let mut rgba = Vec::new();
    colormap::colourise(&[0.0, 1.0, 2.0], FieldKind::Categorical, (lo, hi), &[[0; 3]; 3], &mut rgba);
    ui.horizontal(|ui| {
        for (c, name) in rgba.as_chunks::<4>().0.iter().zip(["air", "fluid", "solid"]) {
            let (rect, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
            ui.painter().rect_filled(rect, 2, Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3].max(120)));
            ui.label(RichText::new(name).small());
        }
    });
}

/// A compact tick label: about three significant figures, no trailing noise.
pub fn format_value(v: f32) -> String {
    let a = v.abs();
    if a == 0.0 {
        "0".into()
    } else if a >= 1000.0 || a < 0.01 {
        format!("{v:.1e}")
    } else if a >= 100.0 {
        format!("{v:.0}")
    } else if a >= 10.0 {
        format!("{v:.1}")
    } else if a >= 1.0 {
        format!("{v:.2}")
    } else {
        format!("{v:.3}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_labels_stay_short() {
        assert_eq!(format_value(0.0), "0");
        assert_eq!(format_value(2.4567), "2.46");
        assert_eq!(format_value(-35.21), "-35.2");
        assert_eq!(format_value(412.7), "413");
        assert_eq!(format_value(0.0423), "0.042");
        assert_eq!(format_value(12345.0), "1.2e4");
        assert_eq!(format_value(0.00042), "4.2e-4");
    }
}
