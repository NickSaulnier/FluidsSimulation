//! View tab: how the fluid is drawn, the picture as a whole (glow, feedback), and the
//! field overlay.

use crate::render::Palette;
use crate::render::composite::{CompositeSettings, Symmetry};
use crate::render::feedback::{EchoFlip, FeedbackMode, FeedbackSettings};
use crate::render::fractal::{FractalKind, MAX_ITERATIONS, MAX_ZOOM};
use crate::render::waves::{WaveMode, WaveSettings};
use crate::render::palette::{GROUPS, named};
use crate::ui::Frame;
use crate::ui::shortcuts::{Action, tip};
use crate::ui::widgets::{self, row};

pub fn show(ui: &mut egui::Ui, f: &mut Frame<'_>) {
    let s = &mut *f.state;
    let solver = &f.world.solver;
    let in_3d = f.world3d.is_some();
    widgets::group(ui, "Display", |ui| {
        colours(ui, &mut s.palette);
        if in_3d {
            ui.weak("In 3D the smoke takes its tint from these colours.");
            return;
        }
        // Grid solvers have no particles to draw: they show a field (the dye) instead.
        if solver.particles().is_none() {
            let field = solver.fields().first().map_or("grid", |f| f.label);
            ui.weak(format!("{} is a grid solver: it draws its {} field.", solver.name(), field.to_lowercase()));
            return;
        }
        widgets::toggle(
            ui,
            &mut s.liquid,
            &tip("Liquid surface", Action::ToggleLiquid),
            "Blend particles into a continuous surface instead of drawing discs.",
        );
        if s.liquid {
            row(ui, "Blob size", "Splat radius in particle radii. Bigger = smoother, blobbier.", |ui| {
                ui.add(egui::Slider::new(&mut s.blob_size, 1.5..=6.0));
            });
            row(ui, "Surface level", "Where the surface sits, as a fraction of interior density. Lower = puffier.", |ui| {
                ui.add(egui::Slider::new(&mut s.surface_threshold, 0.15..=0.9));
            });
        }
        widgets::toggle(ui, &mut s.show_particles, &tip("Show particles", Action::ToggleParticles), "");
    });
    widgets::group(ui, "Picture", |ui| picture(ui, f.state));
    widgets::group(ui, "Screen", |ui| screen(ui, f.state));
    widgets::group(ui, "Interface", |ui| interface(ui, f.state));
    widgets::group(ui, "Feedback", |ui| feedback(ui, &mut f.state.feedback));
    widgets::group(ui, "Waves", |ui| waves(ui, &mut f.state.waves));
    widgets::group(ui, "Composite", |ui| composite(ui, &mut f.state.composite));
    if !in_3d {
        widgets::group(ui, "Overlay", |ui| overlay(ui, f));
    }
}

/// MilkDrop-style feedback: each frame built on the last, warped and faded.
fn feedback(ui: &mut egui::Ui, s: &mut FeedbackSettings) {
    widgets::toggle(ui, &mut s.enabled, "Feedback", "Build each frame on the last one, warped and faded: trails, tunnels and smears.");
    if !s.enabled {
        return;
    }
    let slider = |ui: &mut egui::Ui, label: &str, help: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>| {
        row(ui, label, help, |ui| {
            widgets::eased_slider(ui, value, range);
        });
    };
    row(ui, "Mode", "How the new picture combines with the old one.", |ui| {
        egui::ComboBox::from_id_salt("feedback_mode").selected_text(mode_label(s.mode)).width(150.0).show_ui(ui, |ui| {
            for mode in [FeedbackMode::Smear, FeedbackMode::Brightest] {
                ui.selectable_value(&mut s.mode, mode, mode_label(mode));
            }
        });
    });
    slider(ui, "Trail", "How much of the picture is left after a second.", &mut s.trail, 0.0..=0.98);
    slider(ui, "Flow", "How strongly the fluid's motion carries the picture (1: exactly with it).", &mut s.flow, 0.0..=3.0);
    row(ui, "Zoom", "Growth per second from the centre: above 1 a tunnel outward, below 1 inward.", |ui| {
        ui.add(egui::Slider::new(&mut s.zoom, 0.5..=2.0).logarithmic(true));
    });
    slider(ui, "Turn", "Rotation, radians per second.", &mut s.rotation, -1.5..=1.5);
    slider(ui, "Drift up", "Upward shift, screen heights per second.", &mut s.drift[1], -0.5..=0.5);
    slider(ui, "Drift right", "Sideways shift, screen heights per second.", &mut s.drift[0], -0.5..=0.5);
    slider(ui, "Wobble", "Slow waves bending the motion.", &mut s.warp, 0.0..=0.05);
    slider(ui, "Wobble speed", "", &mut s.warp_speed, 0.0..=4.0);
    slider(ui, "Bass zoom", "Extra zoom while the bass is loud.", &mut s.bass_zoom, 0.0..=1.0);
    slider(ui, "Echo", "A zoomed copy shown over the picture.", &mut s.echo, 0.0..=1.0);
    if s.echo > 0.0 {
        slider(ui, "Echo zoom", "", &mut s.echo_zoom, 0.5..=3.0);
        row(ui, "Echo flip", "", |ui| {
            egui::ComboBox::from_id_salt("echo_flip").selected_text(flip_label(s.echo_flip)).width(150.0).show_ui(ui, |ui| {
                for flip in [EchoFlip::None, EchoFlip::Horizontal, EchoFlip::Vertical, EchoFlip::Both] {
                    ui.selectable_value(&mut s.echo_flip, flip, flip_label(flip));
                }
            });
        });
    }
}

/// The picture's last touches, never fed back: symmetry, colour grading, lens.
fn composite(ui: &mut egui::Ui, s: &mut CompositeSettings) {
    let slider = |ui: &mut egui::Ui, label: &str, help: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>| {
        row(ui, label, help, |ui| {
            widgets::eased_slider(ui, value, range);
        });
    };
    row(ui, "Symmetry", "Mirror the picture, or fold it into a kaleidoscope round the centre.", |ui| {
        egui::ComboBox::from_id_salt("symmetry").selected_text(s.symmetry.label()).width(150.0).show_ui(ui, |ui| {
            for symmetry in Symmetry::ALL {
                ui.selectable_value(&mut s.symmetry, symmetry, symmetry.label());
            }
        });
    });
    if s.symmetry == Symmetry::Kaleidoscope {
        row(ui, "Wedges", "How many mirrored wedges round the centre.", |ui| {
            ui.add(egui::Slider::new(&mut s.segments, 2..=16));
        });
        slider(ui, "Kaleidoscope turn", "Radians per second.", &mut s.symmetry_spin, -2.0..=2.0);
    }
    slider(ui, "Hue", "Turn every colour round the colour wheel, degrees.", &mut s.hue, -180.0..=180.0);
    slider(ui, "Hue drift", "Keep turning the colours, degrees per second.", &mut s.hue_speed, -90.0..=90.0);
    slider(ui, "Saturation", "0 grey, 1 as is, above 1 more vivid.", &mut s.saturation, 0.0..=2.0);
    slider(ui, "Contrast", "1 as is.", &mut s.contrast, 0.5..=2.0);
    slider(ui, "Exposure", "Brighter or darker, as a multiple.", &mut s.exposure, 0.25..=2.0);
    slider(ui, "Palette map", "Recolour the picture by its brightness with the fluid's colours (dark to deep, base, highlight).", &mut s.palette_map, 0.0..=1.0);
    slider(ui, "Invert", "", &mut s.invert, 0.0..=1.0);
    slider(ui, "Fringes", "Chromatic aberration: colour fringes toward the edges.", &mut s.aberration, 0.0..=0.05);
    slider(ui, "Vignette", "Darker corners.", &mut s.vignette, 0.0..=1.0);
    slider(ui, "Grain", "Film grain.", &mut s.grain, 0.0..=0.3);
    if *s != CompositeSettings::OFF && ui.button("Reset").on_hover_text("Everything as it was: no symmetry, no grading, no lens").clicked() {
        *s = CompositeSettings::OFF;
    }
}

/// The sound drawn on screen: a scope, the spectrum or a left-right figure, and shapes.
fn waves(ui: &mut egui::Ui, s: &mut WaveSettings) {
    let slider = |ui: &mut egui::Ui, label: &str, help: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>| {
        row(ui, label, help, |ui| {
            widgets::eased_slider(ui, value, range);
        });
    };
    row(ui, "Wave", "The sound drawn on screen. With feedback on, it streaks into trails and tunnels.", |ui| {
        egui::ComboBox::from_id_salt("wave_mode").selected_text(s.mode.label()).width(150.0).show_ui(ui, |ui| {
            for mode in WaveMode::ALL {
                ui.selectable_value(&mut s.mode, mode, mode.label());
            }
        });
    });
    if s.mode != WaveMode::Off {
        slider(ui, "Swing", "How far the sound moves the wave.", &mut s.scale, 0.0..=2.0);
        if matches!(s.mode, WaveMode::Circle | WaveMode::Spectrum | WaveMode::MirrorSpectrum) {
            slider(ui, "Size", "The ring's radius, or the bars' height.", &mut s.size, 0.05..=1.0);
        }
        if matches!(s.mode, WaveMode::Line | WaveMode::MirrorSpectrum) {
            slider(ui, "Height", "Where the line sits, from the middle.", &mut s.y, -1.0..=1.0);
        }
        slider(ui, "Line width", "Pixels.", &mut s.thickness, 0.5..=12.0);
        if matches!(s.mode, WaveMode::Line | WaveMode::Circle) {
            slider(ui, "Push", "The wave pushes the fluid as the sound moves it.", &mut s.push, 0.0..=1.0);
        }
    }
    for (k, shape) in s.shapes.iter_mut().enumerate() {
        widgets::toggle(ui, &mut shape.enabled, &format!("Shape {}", k + 1), "A spinning polygon that pulses with the bass.");
        if shape.enabled {
            row(ui, "Sides", "", |ui| {
                ui.add(egui::Slider::new(&mut shape.sides, 3..=12));
            });
            slider(ui, "Radius", "", &mut shape.radius, 0.02..=1.0);
            slider(ui, "Across", "", &mut shape.centre[0], -1.5..=1.5);
            slider(ui, "Up", "", &mut shape.centre[1], -1.0..=1.0);
            slider(ui, "Spin", "Radians per second.", &mut shape.spin, -3.0..=3.0);
            slider(ui, "Outline", "Pixels (0: none).", &mut shape.thickness, 0.0..=12.0);
            slider(ui, "Bass pulse", "", &mut shape.bass_pulse, 0.0..=1.0);
            slider(ui, "Fill", "", &mut shape.fill, 0.0..=1.0);
        }
    }
    if s.any() {
        slider(ui, "Brightness", "Light added: above 1 glows.", &mut s.brightness, 0.0..=4.0);
    }
    let art = &mut s.art;
    widgets::toggle(ui, &mut art.enabled, "Album art", "The cover of the song playing (listening live, from what the computer reports), drawn into the picture so the feedback carries it.");
    if art.enabled {
        slider(ui, "Size", "", &mut art.size, 0.05..=1.0);
        slider(ui, "Opacity", "", &mut art.opacity, 0.0..=1.0);
        slider(ui, "Across", "", &mut art.centre[0], -1.5..=1.5);
        slider(ui, "Up", "", &mut art.centre[1], -1.0..=1.0);
        slider(ui, "Spin", "Radians per second.", &mut art.spin, -3.0..=3.0);
        slider(ui, "Bass pulse", "", &mut art.bass_pulse, 0.0..=1.0);
        slider(ui, "Light", "Above 1 glows.", &mut art.brightness, 0.0..=4.0);
    }
    let fractal = &mut s.fractal;
    widgets::toggle(ui, &mut fractal.enabled, "Fractal", "An escape-time fractal in the fluid's colours, drawn into the picture so the feedback carries it.");
    if fractal.enabled {
        row(ui, "Kind", "", |ui| {
            egui::ComboBox::from_id_salt("fractal_kind").selected_text(fractal.kind.label()).width(150.0).show_ui(ui, |ui| {
                for kind in FractalKind::ALL {
                    ui.selectable_value(&mut fractal.kind, kind, kind.label());
                }
            });
        });
        row(ui, "Zoom", "", |ui| {
            ui.add(egui::Slider::new(&mut fractal.zoom, 0.2..=MAX_ZOOM).logarithmic(true));
        });
        slider(ui, "Across", "The point at the middle of the canvas.", &mut fractal.centre[0], -2.5..=2.5);
        slider(ui, "Up", "", &mut fractal.centre[1], -2.0..=2.0);
        if fractal.kind == FractalKind::Julia {
            slider(ui, "Julia real", "The constant c: its shape.", &mut fractal.julia[0], -2.0..=2.0);
            slider(ui, "Julia imaginary", "", &mut fractal.julia[1], -2.0..=2.0);
        }
        slider(ui, "Turn", "Radians.", &mut fractal.rotation, -3.15..=3.15);
        slider(ui, "Spin", "Radians per second.", &mut fractal.spin, -1.0..=1.0);
        if fractal.kind == FractalKind::Julia {
            slider(ui, "Wander", "The Julia point travels along the Mandelbrot set's edge: about laps a minute at full loudness.", &mut fractal.wander, -4.0..=4.0);
        }
        slider(ui, "Dive", "Travel endlessly into the fractal (negative: out of it): zoom doublings a second, faster when the music is loud. It flies to a place where the picture repeats itself and loops there.", &mut fractal.dive, -3.0..=3.0);
        slider(ui, "Bass zoom", "How much the zoom follows the bass.", &mut fractal.bass_zoom, 0.0..=1.0);
        slider(ui, "Bend", "How far the fluid's flow carries the fractal (1: with the fluid). It springs back when the fluid stills.", &mut fractal.bend, 0.0..=2.0);
        widgets::toggle(ui, &mut fractal.tour, "Tour", "Move to another spot at each new section of the song (a Julia set changes shape).");
        row(ui, "Detail", "Iterations: more shows finer filaments when zoomed in, and costs more.", |ui| {
            ui.add(egui::Slider::new(&mut fractal.iterations, 16..=MAX_ITERATIONS).logarithmic(true));
        });
        slider(ui, "Bands", "How many times the palette repeats toward the edge.", &mut fractal.bands, 0.0..=8.0);
        slider(ui, "Colour speed", "Palette cycles per second.", &mut fractal.colour_speed, -1.0..=1.0);
        slider(ui, "Opacity", "", &mut fractal.opacity, 0.0..=1.0);
        slider(ui, "Light", "Above 1 glows.", &mut fractal.brightness, 0.0..=4.0);
    }
}

fn mode_label(mode: FeedbackMode) -> &'static str {
    match mode {
        FeedbackMode::Smear => "Smear",
        FeedbackMode::Brightest => "Brightest",
    }
}

fn flip_label(flip: EchoFlip) -> &'static str {
    match flip {
        EchoFlip::None => "None",
        EchoFlip::Horizontal => "Left-right",
        EchoFlip::Vertical => "Upside down",
        EchoFlip::Both => "Both",
    }
}

/// Glow and the render scale: they apply to every scene, 2D or 3D.
fn picture(ui: &mut egui::Ui, s: &mut crate::ui::UiState) {
    row(ui, "Glow", "Bloom: bright parts of the picture glow onto their surroundings.", |ui| {
        widgets::eased_slider(ui, &mut s.glow, 0.0..=1.0);
    });
    let label = |scale: f32| if scale > 0.0 { format!("{:.0}%", 100.0 * scale) } else { "Automatic".to_string() };
    row(ui, "Render scale", "The scene's resolution relative to the window. Automatic draws 4K at 75%.", |ui| {
        egui::ComboBox::from_id_salt("render_scale").selected_text(label(s.render_scale)).width(150.0).show_ui(ui, |ui| {
            for scale in [0.0, 1.0, 0.75, 0.5] {
                ui.selectable_value(&mut s.render_scale, scale, label(scale));
            }
        });
    });
}

/// The displays as Windows arranges them, numbered, the chosen one lit and the app's
/// marked; clicking one chooses it.
fn display_map(ui: &mut egui::Ui, displays: &[crate::ui::Display], chosen: &mut Option<String>, window: Option<&str>) {
    use crate::ui::theme;
    let (min_x, min_y) = displays.iter().fold((i32::MAX, i32::MAX), |(x, y), d| (x.min(d.rect[0]), y.min(d.rect[1])));
    let (max_x, max_y) = displays.iter().fold((i32::MIN, i32::MIN), |(x, y), d| (x.max(d.rect[0] + d.rect[2]), y.max(d.rect[1] + d.rect[3])));
    let (span_w, span_h) = ((max_x - min_x).max(1) as f32, (max_y - min_y).max(1) as f32);
    let width = ui.available_width().min(320.0);
    let scale = (width / span_w).min(110.0 / span_h);
    let (area, _) = ui.allocate_exact_size(egui::vec2(width, span_h * scale), egui::Sense::hover());
    let left = area.min.x + (width - span_w * scale) / 2.0;
    for d in displays {
        let r = egui::Rect::from_min_size(
            egui::pos2(left + (d.rect[0] - min_x) as f32 * scale, area.min.y + (d.rect[1] - min_y) as f32 * scale),
            egui::vec2(d.rect[2] as f32 * scale, d.rect[3] as f32 * scale),
        )
        .shrink(2.0);
        let response = ui.interact(r, ui.id().with(("display", &d.id)), egui::Sense::click()).on_hover_text(&d.label);
        let lit = chosen.as_deref() == Some(d.id.as_str()) || (chosen.is_none() && window == Some(d.id.as_str()));
        let fill = if lit { theme::ACCENT.gamma_multiply(0.25) } else if response.hovered() { theme::WIDGET_HOVER } else { theme::WIDGET };
        let stroke = egui::Stroke::new(if lit { 2.0 } else { 1.0 }, if lit { theme::ACCENT } else { theme::BORDER });
        ui.painter().rect(r, 4.0, fill, stroke, egui::StrokeKind::Inside);
        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, d.number.to_string(), theme::display(16.0), if lit { theme::TEXT } else { theme::TEXT_DIM });
        if window == Some(d.id.as_str()) {
            ui.painter().text(r.center_bottom() - egui::vec2(0.0, 4.0), egui::Align2::CENTER_BOTTOM, "app", egui::FontId::proportional(11.0), theme::ACCENT);
        }
        if response.clicked() {
            *chosen = Some(d.id.clone());
        }
    }
}

/// The interface's size, and whether it moves.
fn interface(ui: &mut egui::Ui, s: &mut crate::ui::UiState) {
    row(ui, "Size", "How big the panels, text and buttons are drawn", |ui| {
        egui::ComboBox::from_id_salt("ui_size").selected_text(s.ui_size.label()).width(150.0).show_ui(ui, |ui| {
            for size in crate::ui::UiSize::ALL {
                ui.selectable_value(&mut s.ui_size, size, size.label());
            }
        });
    });
    widgets::toggle(ui, &mut s.reduce_motion, "Reduce motion", "Change things at once instead of sliding and fading them (also on when Windows' animation effects are off)");
}

/// Where the visualizer shows, and the frame-rate cap.
fn screen(ui: &mut egui::Ui, s: &mut crate::ui::UiState) {
    let screen = &mut s.screen;
    widgets::toggle(
        ui,
        &mut screen.windowed,
        "Visualizer in a window",
        "F11 shows the visualizer in this window (its controls hidden) instead of full screen",
    );
    ui.add_enabled_ui(screen.windowed, |ui| {
        widgets::toggle(ui, &mut screen.on_top, "Keep it on top", "While the visualizer shows in a window, keep that window above the others");
    });
    row(ui, "Display", "Where the app and the visualizer show: the app moves there at once (Shift+F11 moves to the next)", |ui| {
        let label = |id: &Option<String>| match id {
            Some(id) => s.monitors.iter().find(|d| &d.id == id).map_or_else(|| id.clone(), |d| d.label.clone()),
            None => "Where the window is".into(),
        };
        egui::ComboBox::from_id_salt("monitor").selected_text(label(&screen.monitor)).width(200.0).truncate().show_ui(ui, |ui| {
            ui.selectable_value(&mut screen.monitor, None, "Where the window is");
            for d in &s.monitors {
                ui.selectable_value(&mut screen.monitor, Some(d.id.clone()), &d.label);
            }
        });
    });
    if s.monitors.len() > 1 {
        display_map(ui, &s.monitors, &mut screen.monitor, s.window_display.as_deref());
    }
    let label = |cap: u32| if cap == 0 { "No cap".to_string() } else { format!("{cap} fps") };
    row(ui, "Frame rate", "The most frames per second: lower saves power (and heat) on a fast display", |ui| {
        egui::ComboBox::from_id_salt("fps_cap").selected_text(label(screen.fps_cap)).width(150.0).show_ui(ui, |ui| {
            for cap in crate::ui::FPS_CAPS {
                ui.selectable_value(&mut screen.fps_cap, cap, label(cap));
            }
        });
    });
}

/// A preset menu, then the palette's three colours, each editable (which makes it a
/// custom palette). They colour particles, the liquid surface and dye alike.
fn colours(ui: &mut egui::Ui, palette: &mut Palette) {
    row(ui, "Colours", "The fluid's colours, for every solver", |ui| {
        egui::ComboBox::from_id_salt("palette").selected_text(palette.name().unwrap_or("Custom")).width(150.0).height(520.0).show_ui(ui, |ui| {
            for (group, names) in GROUPS {
                ui.label(egui::RichText::new(group.to_uppercase()).font(crate::ui::theme::display(11.0)).color(crate::ui::theme::TEXT_DIM));
                for name in *names {
                    let Some(preset) = named(name) else { continue };
                    if swatch_row(ui, name, &preset, *palette == preset).clicked() {
                        *palette = preset;
                    }
                }
                ui.add_space(4.0);
            }
        });
    });
    row(ui, "", "", |ui| {
        for (colour, what) in [
            (&mut palette.deep, "Deep: thick liquid"),
            (&mut palette.base, "Base: the fluid's own colour"),
            (&mut palette.highlight, "Highlight: fast flow, foam and dense dye"),
        ] {
            ui.color_edit_button_srgb(colour).on_hover_text(what);
        }
    });
}

/// A palette in the menu: its three colours as swatches, then its name.
fn swatch_row(ui: &mut egui::Ui, name: &str, p: &Palette, selected: bool) -> egui::Response {
    // The swatches and the name are drawn at fixed places (padding the name with spaces
    // depends on the font's space width).
    let response = ui.add(egui::Button::selectable(selected, "").min_size(egui::vec2(190.0, 26.0)));
    let left = response.rect.left_center() + egui::vec2(8.0, 0.0);
    for (k, c) in [p.deep, p.base, p.highlight].into_iter().enumerate() {
        let r = egui::Rect::from_center_size(left + egui::vec2(8.0 + 15.0 * k as f32, 0.0), egui::vec2(13.0, 13.0));
        ui.painter().rect(r, 3.0, egui::Color32::from_rgb(c[0], c[1], c[2]), egui::Stroke::new(1.0, crate::ui::theme::BORDER), egui::StrokeKind::Outside);
    }
    let colour = ui.style().interact_selectable(&response, selected).text_color();
    ui.painter().text(left + egui::vec2(52.0, 0.0), egui::Align2::LEFT_CENTER, name, egui::TextStyle::Button.resolve(ui.style()), colour);
    response
}

fn overlay(ui: &mut egui::Ui, f: &mut Frame<'_>) {
    let (world, s) = (&*f.world, &mut *f.state);
    let fields = world.solver.fields();
    let current = s.overlay.and_then(|key| fields.iter().find(|f| f.key == key));
    row(ui, "Field", &tip("Solver field drawn over the scene; cycle with", Action::NextOverlay), |ui| {
        egui::ComboBox::from_id_salt("overlay").selected_text(current.map_or("None", |f| f.label)).width(150.0).show_ui(
            ui,
            |ui| {
                ui.selectable_value(&mut s.overlay, None, "None");
                for f in fields {
                    ui.selectable_value(&mut s.overlay, Some(f.key), f.label);
                }
            },
        );
    });
    if fields.is_empty() {
        ui.weak(format!("{} has no grid fields", world.solver.name()));
    }
    // The legend for the field is on the canvas, next to what it describes.
    if current.is_some() {
        row(ui, "Opacity", "", |ui| ui.add(egui::Slider::new(&mut s.overlay_opacity, 0.1..=1.0)));
    }
}
