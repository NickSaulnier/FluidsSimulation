//! The title card: when a song starts (and on T), its cover, title, artist and album
//! along the bottom left, fading in, staying a few seconds and fading out, MilkDrop
//! style.

use egui::{Align2, Color32, RichText, vec2};
use std::time::Instant;

const FADE_IN_S: f32 = 0.6;
const HOLD_S: f32 = 5.0;
const FADE_OUT_S: f32 = 1.5;
const INSET: f32 = 40.0;
const COVER: f32 = 112.0;

/// The song playing, as the interface shows it.
#[derive(Clone)]
pub struct Song {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub playing: bool,
    pub cover: Option<egui::TextureHandle>,
}

/// How visible a card shown at `since` is `now`: 0 to 1, and 0 once it's gone.
pub fn opacity(since: Instant, now: Instant) -> f32 {
    let t = now.saturating_duration_since(since).as_secs_f32();
    if t < FADE_IN_S {
        t / FADE_IN_S
    } else if t < FADE_IN_S + HOLD_S {
        1.0
    } else {
        (1.0 - (t - FADE_IN_S - HOLD_S) / FADE_OUT_S).max(0.0)
    }
}

/// Draw the card for `song` over `canvas`, `alpha` visible.
pub fn show(ctx: &egui::Context, canvas: egui::Rect, song: &Song, alpha: f32) {
    egui::Area::new(egui::Id::new("title_card"))
        .order(egui::Order::Foreground)
        .pivot(Align2::LEFT_BOTTOM)
        .fixed_pos(canvas.left_bottom() + vec2(INSET, -INSET))
        .interactable(false)
        .show(ctx, |ui| {
            ui.multiply_opacity(alpha);
            egui::Frame::new().fill(Color32::from_rgba_unmultiplied(10, 12, 16, 170)).corner_radius(12).inner_margin(egui::Margin::same(14)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    if let Some(cover) = &song.cover {
                        let size = cover.size_vec2();
                        let scale = COVER / size.x.max(size.y).max(1.0);
                        ui.add(egui::Image::new(cover).fit_to_exact_size(size * scale).corner_radius(6));
                        ui.add_space(8.0);
                    }
                    ui.vertical(|ui| {
                        ui.add_space(4.0);
                        ui.label(RichText::new(&song.title).size(30.0).strong().color(Color32::WHITE));
                        if !song.artist.is_empty() {
                            ui.label(RichText::new(&song.artist).size(20.0).color(Color32::from_gray(225)));
                        }
                        if !song.album.is_empty() {
                            ui.label(RichText::new(&song.album).size(14.0).color(Color32::from_gray(170)));
                        }
                    });
                });
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_card_fades_in_holds_and_fades_out() {
        let start = Instant::now();
        let at = |s: f32| opacity(start, start + Duration::from_secs_f32(s));
        assert_eq!(at(0.0), 0.0);
        assert!((at(0.3) - 0.5).abs() < 1e-3);
        assert_eq!(at(3.0), 1.0);
        assert!(at(FADE_IN_S + HOLD_S + 0.5 * FADE_OUT_S) > 0.4);
        assert_eq!(at(10.0), 0.0);
    }
}
