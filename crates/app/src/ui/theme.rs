//! The look of the interface: a dark theme keyed to the scene's palette. The liquid
//! blue is the one accent (selected tool and tab, slider fill); amber and red are
//! reserved for warnings.

use egui::{Color32, CornerRadius, Stroke, vec2};

pub const ACCENT: Color32 = Color32::from_rgb(64, 140, 230);
pub const WARN: Color32 = Color32::from_rgb(255, 190, 90);
pub const ERROR: Color32 = Color32::from_rgb(255, 110, 110);
/// Section headers.
pub const HEADER: Color32 = Color32::from_rgb(225, 230, 240);
/// Fill of the rounded groups inside the side panel.
pub const GROUP_FILL: Color32 = Color32::from_rgb(38, 41, 48);

const PANEL: Color32 = Color32::from_rgb(28, 30, 35);
const WIDGET: Color32 = Color32::from_rgb(52, 56, 66);
const WIDGET_HOVER: Color32 = Color32::from_rgb(66, 71, 84);
const TEXT: Color32 = Color32::from_rgb(200, 205, 215);

/// Install the theme and the icon font. Always dark: the scene is drawn dark whatever
/// the system theme.
pub fn apply(ctx: &egui::Context) {
    // Phosphor icons (egui's own fonts lack most symbols), as a fallback after the text
    // font so icon codepoints can be mixed into ordinary labels.
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);
    ctx.set_theme(egui::Theme::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |s| {
        let sp = &mut s.spacing;
        sp.item_spacing = vec2(6.0, 5.0);
        sp.button_padding = vec2(8.0, 3.0);
        sp.interact_size.y = 20.0;
        sp.slider_width = 84.0;

        let v = &mut s.visuals;
        v.panel_fill = PANEL;
        v.window_fill = PANEL;
        v.extreme_bg_color = Color32::from_rgb(20, 22, 26);
        v.faint_bg_color = GROUP_FILL;
        v.hyperlink_color = ACCENT;
        v.warn_fg_color = WARN;
        v.error_fg_color = ERROR;
        v.slider_trailing_fill = true;
        v.selection.bg_fill = ACCENT;
        v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
        v.window_corner_radius = CornerRadius::same(6);
        v.menu_corner_radius = CornerRadius::same(6);

        let w = &mut v.widgets;
        for state in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            state.corner_radius = CornerRadius::same(4);
        }
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        w.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(48, 52, 60));
        w.inactive.bg_fill = WIDGET;
        w.inactive.weak_bg_fill = WIDGET;
        w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        w.hovered.bg_fill = WIDGET_HOVER;
        w.hovered.weak_bg_fill = WIDGET_HOVER;
        w.hovered.bg_stroke = Stroke::new(1.0, ACCENT.gamma_multiply(0.6));
        w.active.bg_fill = ACCENT;
        w.active.weak_bg_fill = ACCENT;
        w.open.weak_bg_fill = WIDGET_HOVER;
    });
}
