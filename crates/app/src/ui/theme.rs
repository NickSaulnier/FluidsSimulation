//! The look of the interface: near-black, high contrast, with one electric accent
//! (cyan, fading to magenta in titles) for what's selected and what matters. Amber and
//! red are kept for warnings and errors.
//!
//! Two fonts, both under the Open Font License and bundled (`assets/fonts`): Inter for
//! text, clear at small sizes and in numbers, and Orbitron, a wide geometric display
//! face, for titles, tab names and group headers (`DISPLAY`). Inter SemiBold is `BOLD`.
//! The icon font follows each, so icons mix into any text.

use egui::{Color32, CornerRadius, FontFamily, FontId, Shadow, Stroke, TextStyle, vec2};

/// The accent: selected things, slider fills, headers. Its partner, for gradients and
/// glows in titles.
pub const ACCENT: Color32 = Color32::from_rgb(0x22, 0xD3, 0xEE);
pub const ACCENT_2: Color32 = Color32::from_rgb(0xE0, 0x40, 0xFB);
pub const WARN: Color32 = Color32::from_rgb(255, 190, 90);
pub const ERROR: Color32 = Color32::from_rgb(255, 107, 107);
/// Group headers (in the display font).
pub const HEADER: Color32 = ACCENT;
/// Text: values and labels, then hints and secondary labels.
pub const TEXT: Color32 = Color32::from_rgb(0xF2, 0xF4, 0xF8);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x8A, 0x93, 0xA6);
/// Text on an accent fill (a selected button or menu item).
pub const ON_ACCENT: Color32 = Color32::from_rgb(0x05, 0x10, 0x16);
/// Backgrounds, darkest first: panels, the rounded groups in them, controls.
pub const PANEL: Color32 = Color32::from_rgb(0x0B, 0x0C, 0x10);
pub const GROUP_FILL: Color32 = Color32::from_rgb(0x12, 0x14, 0x1A);
pub const WIDGET: Color32 = Color32::from_rgb(0x1B, 0x1E, 0x26);
pub const WIDGET_HOVER: Color32 = Color32::from_rgb(0x26, 0x2A, 0x35);
pub const BORDER: Color32 = Color32::from_rgb(0x2A, 0x2F, 0x3A);
/// A control being pressed.
const PRESSED: Color32 = Color32::from_rgb(0x30, 0x37, 0x47);
/// Text fields and other wells.
const WELL: Color32 = Color32::from_rgb(0x07, 0x08, 0x0B);

/// The display face (titles, tabs, headers) and the bold text face.
pub const DISPLAY: &str = "display";
pub const BOLD: &str = "bold";

/// Text sizes, points.
pub const SMALL: f32 = 12.5;
pub const BODY: f32 = 15.0;
pub const HEADING: f32 = 20.0;
/// Rows and buttons, points.
pub const ROW: f32 = 30.0;

pub fn display(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(DISPLAY.into()))
}

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(BOLD.into()))
}

/// Install the fonts and the theme. Always dark: the scene is drawn dark whatever the
/// system theme.
pub fn apply(ctx: &egui::Context) {
    ctx.set_fonts(fonts());
    ctx.set_theme(egui::Theme::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |s| {
        s.text_styles = [
            (TextStyle::Small, FontId::proportional(SMALL)),
            (TextStyle::Body, FontId::proportional(BODY)),
            (TextStyle::Button, FontId::proportional(BODY)),
            (TextStyle::Heading, display(HEADING)),
            (TextStyle::Monospace, FontId::monospace(13.5)),
        ]
        .into();
        let sp = &mut s.spacing;
        sp.item_spacing = vec2(8.0, 6.0);
        sp.button_padding = vec2(10.0, 5.0);
        sp.interact_size = vec2(40.0, ROW);
        sp.slider_width = 130.0;
        sp.icon_width = 18.0;
        sp.icon_width_inner = 10.0;
        sp.combo_height = 320.0;

        let v = &mut s.visuals;
        v.panel_fill = PANEL;
        v.window_fill = PANEL;
        v.window_stroke = Stroke::new(1.0, BORDER);
        v.window_shadow = Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(160) };
        v.popup_shadow = Shadow { offset: [0, 6], blur: 16, spread: 0, color: Color32::from_black_alpha(150) };
        v.extreme_bg_color = WELL;
        v.faint_bg_color = GROUP_FILL;
        v.code_bg_color = WELL;
        v.hyperlink_color = ACCENT;
        v.warn_fg_color = WARN;
        v.error_fg_color = ERROR;
        v.slider_trailing_fill = true;
        v.selection.bg_fill = ACCENT;
        v.selection.stroke = Stroke::new(1.5, ON_ACCENT);
        v.window_corner_radius = CornerRadius::same(10);
        v.menu_corner_radius = CornerRadius::same(8);

        let w = &mut v.widgets;
        for state in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            state.corner_radius = CornerRadius::same(6);
        }
        w.noninteractive.bg_fill = GROUP_FILL;
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        w.inactive.bg_fill = WIDGET;
        w.inactive.weak_bg_fill = WIDGET;
        w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        w.inactive.bg_stroke = Stroke::new(1.0, BORDER);
        // Hovered: brighter, an accent edge, and a point larger (it lifts).
        w.hovered.bg_fill = WIDGET_HOVER;
        w.hovered.weak_bg_fill = WIDGET_HOVER;
        w.hovered.fg_stroke = Stroke::new(1.5, TEXT);
        w.hovered.bg_stroke = Stroke::new(1.0, ACCENT.gamma_multiply(0.7));
        w.hovered.expansion = 1.0;
        // Pressed: brighter still, an accent edge, back to its size (it presses in). (Its
        // text colour is also egui's "strong" text colour, so it stays light.)
        w.active.bg_fill = PRESSED;
        w.active.weak_bg_fill = PRESSED;
        w.active.fg_stroke = Stroke::new(1.5, TEXT);
        w.active.bg_stroke = Stroke::new(1.5, ACCENT);
        w.active.expansion = 0.0;
        w.open.weak_bg_fill = WIDGET_HOVER;
        w.open.bg_stroke = Stroke::new(1.0, ACCENT.gamma_multiply(0.7));
    });
}

/// Inter for text and Orbitron for display, each followed by egui's own fonts (for
/// symbols outside their character sets) and the icon font.
fn fonts() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    for (name, bytes) in [
        ("inter", &include_bytes!("../../assets/fonts/Inter-Regular.ttf")[..]),
        ("inter_semibold", &include_bytes!("../../assets/fonts/Inter-SemiBold.ttf")[..]),
        ("orbitron", &include_bytes!("../../assets/fonts/Orbitron-Bold.ttf")[..]),
    ] {
        fonts.font_data.insert(name.into(), egui::FontData::from_static(bytes).into());
    }
    let defaults = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let with = |first: &[&str]| first.iter().map(|s| s.to_string()).chain(defaults.iter().cloned()).collect::<Vec<_>>();
    fonts.families.insert(FontFamily::Proportional, with(&["inter"]));
    fonts.families.insert(FontFamily::Name(DISPLAY.into()), with(&["orbitron", "inter"]));
    fonts.families.insert(FontFamily::Name(BOLD.into()), with(&["inter_semibold", "inter"]));
    // The icons, second in every family (so they don't shadow the text font).
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    for family in [DISPLAY, BOLD] {
        if let Some(keys) = fonts.families.get_mut(&FontFamily::Name(family.into())) {
            keys.insert(1, "phosphor".into());
        }
    }
    fonts
}

/// WCAG contrast ratio of two colours (1 to 21).
#[cfg(test)]
fn contrast(a: Color32, b: Color32) -> f32 {
    let luminance = |c: Color32| {
        let lin = |v: u8| {
            let v = f32::from(v) / 255.0;
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
    };
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every text colour reads against every background it's drawn on (WCAG AA: 4.5:1).
    #[test]
    fn text_has_contrast() {
        for (text, name) in [(TEXT, "text"), (TEXT_DIM, "dim text"), (ACCENT, "accent"), (WARN, "warning"), (ERROR, "error")] {
            for (bg, bg_name) in [(PANEL, "panel"), (GROUP_FILL, "group"), (WIDGET, "control"), (WIDGET_HOVER, "hovered control")] {
                let ratio = contrast(text, bg);
                assert!(ratio >= 4.5, "{name} on {bg_name}: {ratio:.2}");
            }
        }
        assert!(contrast(ON_ACCENT, ACCENT) >= 4.5, "text on the accent: {:.2}", contrast(ON_ACCENT, ACCENT));
        // A pressed control shows its text at full brightness.
        assert!(contrast(TEXT, PRESSED) >= 4.5, "text on a pressed control: {:.2}", contrast(TEXT, PRESSED));
        assert!((contrast(Color32::WHITE, Color32::BLACK) - 21.0).abs() < 0.01);
    }

    /// The fonts load and every family has the icons.
    #[test]
    fn the_fonts_are_installed() {
        let fonts = fonts();
        for family in [FontFamily::Proportional, FontFamily::Name(DISPLAY.into()), FontFamily::Name(BOLD.into())] {
            let keys = &fonts.families[&family];
            assert!(keys.iter().any(|k| k == "phosphor"), "{family:?}: {keys:?}");
        }
        assert_eq!(fonts.families[&FontFamily::Proportional][0], "inter");
        assert_eq!(fonts.families[&FontFamily::Name(DISPLAY.into())][0], "orbitron");
    }
}
