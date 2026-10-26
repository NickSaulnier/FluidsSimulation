//! The look browser (B, or the Looks group's button): every look as a card with its
//! thumbnail, stars and favourite heart, found by searching names and tags or picking
//! tags. Clicking a card fades to it.

use super::shortcuts::{Action, tip};
use super::{Frame, theme};
use crate::looks::{Look, Source};
use egui::{Color32, RichText, Sense, vec2};
use egui_phosphor::regular as icon;

const CARD: egui::Vec2 = vec2(192.0, 108.0);
const GAP: f32 = 10.0;

/// What the browser is showing: the search text, the tags picked (a look needs them
/// all) and whether only favourites.
#[derive(Default)]
pub struct Filter {
    pub search: String,
    pub tags: Vec<String>,
    pub favourites: bool,
}

impl Filter {
    fn shows(&self, look: &Look, favourite: bool) -> bool {
        let words = self.search.to_lowercase();
        let text = |s: &str| s.to_lowercase().contains(words.trim());
        (words.trim().is_empty() || text(&look.name) || look.tags.iter().any(|t| text(t)))
            && self.tags.iter().all(|t| look.tags.contains(t))
            && (!self.favourites || favourite)
    }
}

pub fn show(ctx: &egui::Context, f: &mut Frame<'_>) {
    let mut open = f.state.browser_open;
    egui::Window::new(format!("{} Looks", icon::SQUARES_FOUR))
        .open(&mut open)
        .default_size([830.0, 600.0])
        .resizable(true)
        .collapsible(false)
        .show(ctx, |ui| contents(ui, f));
    f.state.browser_open = open;
}

fn contents(ui: &mut egui::Ui, f: &mut Frame<'_>) {
    let s = &mut *f.state;
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut s.browser.search).hint_text(format!("{} Search names and tags", icon::MAGNIFYING_GLASS)).desired_width(240.0));
        let heart = if s.browser.favourites { icon::HEART } else { icon::HEART_STRAIGHT };
        if ui.selectable_label(s.browser.favourites, format!("{heart} Favourites")).on_hover_text("Show only your favourites").clicked() {
            s.browser.favourites = !s.browser.favourites;
        }
        if ui.button("Clear").on_hover_text("Show every look").clicked() {
            s.browser = Filter::default();
        }
        ui.checkbox(&mut s.director.favourites_only, "Auto: favourites only")
            .on_hover_text("When looks change by themselves, pick only favourites (if there are any). One-star looks are never picked by themselves.");
    });
    let mut tags: Vec<&String> = s.looks.looks.iter().flat_map(|l| &l.tags).collect();
    tags.sort();
    tags.dedup();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        for tag in tags {
            let picked = s.browser.tags.contains(tag);
            if ui.selectable_label(picked, RichText::new(tag.as_str()).small()).clicked() {
                if picked {
                    s.browser.tags.retain(|t| t != tag);
                } else {
                    s.browser.tags.push(tag.clone());
                }
            }
        }
    });
    ui.separator();

    let shown: Vec<usize> = (0..s.looks.looks.len()).filter(|&i| s.browser.shows(&s.looks.looks[i], s.looks.mark(&s.looks.looks[i].name).favourite)).collect();
    if shown.is_empty() {
        ui.weak("No look matches.");
    }
    let mut pick = None;
    // Rows of as many cards as fit (a wrapping layout would widen the window instead).
    let columns = ((ui.available_width() - 16.0 + GAP) / (CARD.x + GAP)).floor().max(1.0) as usize;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for row in shown.chunks(columns) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = vec2(GAP, 12.0);
            for &i in row {
                let look = s.looks.looks[i].clone();
                let current = s.director.current == Some(i);
                ui.vertical(|ui| {
                    ui.set_width(CARD.x);
                    let (rect, response) = ui.allocate_exact_size(CARD, Sense::click());
                    match s.thumbnails.get(ui.ctx(), &look) {
                        Some(texture) => {
                            ui.painter().image(texture.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                        }
                        None => stand_in(ui, rect, &look),
                    }
                    let stroke = if current {
                        egui::Stroke::new(2.0, theme::ACCENT)
                    } else if response.hovered() {
                        egui::Stroke::new(1.0, Color32::from_gray(200))
                    } else {
                        egui::Stroke::new(1.0, Color32::from_gray(60))
                    };
                    ui.painter().rect_stroke(rect, 4, stroke, egui::StrokeKind::Inside);
                    if response.on_hover_text(hover(&look)).clicked() {
                        pick = Some(i);
                    }
                    ui.horizontal(|ui| {
                        let mine = if matches!(look.source, Source::File(_)) { format!(" {}", icon::PENCIL_SIMPLE) } else { String::new() };
                        ui.add(egui::Label::new(RichText::new(format!("{}{mine}", look.name)).strong()).truncate());
                    });
                    marks(ui, s, &look.name);
                });
            }
        });
        ui.add_space(10.0);
        }
    });
    if let Some(i) = pick {
        s.director.request(i);
    }
}

/// The stars (click one to rate) and the heart.
fn marks(ui: &mut egui::Ui, s: &mut super::UiState, name: &str) {
    let mark = s.looks.mark(name);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for k in 1..=5u8 {
            let lit = mark.rating != 0 && k <= mark.rating;
            let glyph = if lit { RichText::new(icon::STAR).color(theme::WARN) } else { RichText::new(icon::STAR).color(Color32::from_gray(90)) };
            let what = if k == 1 { "1 star: never picked by itself".to_string() } else { format!("{k} stars") };
            if ui.add(egui::Button::new(glyph).frame(false)).on_hover_text(tip(&what, Action::RateUp)).clicked() {
                s.looks.marks.entry(name.to_string()).or_default().rating = if mark.rating == k { 0 } else { k };
            }
        }
        ui.add_space(8.0);
        let (glyph, colour) = if mark.favourite { (icon::HEART, Color32::from_rgb(240, 90, 120)) } else { (icon::HEART, Color32::from_gray(90)) };
        if ui.add(egui::Button::new(RichText::new(glyph).color(colour)).frame(false)).on_hover_text("Favourite").clicked() {
            s.looks.marks.entry(name.to_string()).or_default().favourite = !mark.favourite;
        }
    });
}

/// Before a look has been watched: its palette as a gradient, and what it is.
fn stand_in(ui: &egui::Ui, rect: egui::Rect, look: &Look) {
    let p = look.palette;
    let colour = |c: [u8; 3]| Color32::from_rgb(c[0], c[1], c[2]);
    let mut mesh = egui::Mesh::default();
    let stops = [(0.0, colour(p.deep)), (0.6, colour(p.base)), (1.0, colour(p.highlight))];
    for (k, w) in stops.windows(2).enumerate() {
        let (a, b) = (w[0], w[1]);
        let x = |t: f32| rect.min.x + rect.width() * t;
        let base = (k * 4) as u32;
        mesh.colored_vertex(egui::pos2(x(a.0), rect.min.y), a.1.gamma_multiply(0.5));
        mesh.colored_vertex(egui::pos2(x(b.0), rect.min.y), b.1.gamma_multiply(0.5));
        mesh.colored_vertex(egui::pos2(x(a.0), rect.max.y), a.1.gamma_multiply(0.2));
        mesh.colored_vertex(egui::pos2(x(b.0), rect.max.y), b.1.gamma_multiply(0.2));
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base + 1, base + 3, base + 2);
    }
    ui.painter().add(mesh);
    let kind = if !look.has_fluid() {
        "no fluid".to_string()
    } else if look.solver_3d().is_some() {
        format!("3D · {}", look.scene)
    } else {
        look.solver.clone()
    };
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, kind, egui::FontId::proportional(12.0), Color32::from_gray(230));
    ui.painter().text(rect.center() + vec2(0.0, 16.0), egui::Align2::CENTER_CENTER, "(thumbnail after it's shown)", egui::FontId::proportional(10.0), Color32::from_gray(170));
}

fn hover(look: &Look) -> String {
    let energy = match look.energy {
        crate::looks::Energy::Calm => "calm",
        crate::looks::Energy::Energetic => "energetic",
        crate::looks::Energy::Any => "any music",
    };
    let fluid = if look.has_fluid() { format!("{}, {}", look.solver, look.scene) } else { "no fluid".into() };
    let tags = if look.tags.is_empty() { String::new() } else { format!("\n{}", look.tags.join(" · ")) };
    format!("Show {}\n{fluid}; for {energy}{tags}", look.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_filter_searches_names_and_tags() {
        let library = crate::looks::Library::builtin();
        let count = |f: &Filter| library.looks.iter().filter(|l| f.shows(l, l.name == "Prism")).count();
        assert_eq!(count(&Filter::default()), library.looks.len());
        assert_eq!(count(&Filter { search: "PRISM".into(), ..Filter::default() }), 1);
        let fire = count(&Filter { search: "fire".into(), ..Filter::default() });
        assert!(fire >= 4, "names and tags: {fire}");
        let no_fluid = library.looks.iter().filter(|l| !l.has_fluid()).count();
        assert_eq!(count(&Filter { tags: vec!["no fluid".into()], ..Filter::default() }), no_fluid);
        assert_eq!(count(&Filter { favourites: true, ..Filter::default() }), 1);
    }
}
