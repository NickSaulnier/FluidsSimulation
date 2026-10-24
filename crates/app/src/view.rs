//! World (metres, y up) to clip space, fitting the domain into the window with letterboxing.

use glam::Vec2;
use winit::dpi::PhysicalSize;

#[derive(Clone, Copy, Debug)]
pub struct View {
    /// clip = world * scale + offset
    pub scale: Vec2,
    pub offset: Vec2,
    pub px_per_metre: f32,
    /// Window size in physical pixels.
    window: Vec2,
}

impl View {
    pub fn fit(domain: Vec2, window: PhysicalSize<u32>, margin_px: f32) -> Self {
        let win = Vec2::new(window.width as f32, window.height as f32).max(Vec2::ONE);
        let avail = (win - 2.0 * margin_px).max(Vec2::ONE);
        let k = (avail / domain).min_element();
        // pixel = world * k + (win - domain * k) / 2, then clip = 2 * pixel / win - 1.
        Self { scale: 2.0 * k / win, offset: -domain * k / win, px_per_metre: k, window: win }
    }

    /// Fit computed for a sub-area anchored at the window's top-left (e.g. the window
    /// minus a side panel): keep its projection but express it in full-window clip space.
    pub fn with_window(self, window: PhysicalSize<u32>) -> Self {
        let full = Vec2::new(window.width as f32, window.height as f32).max(Vec2::ONE);
        let ratio = self.window / full;
        // clip_full = (clip_area + 1) * ratio - 1
        Self { scale: self.scale * ratio, offset: (self.offset + 1.0) * ratio - 1.0, window: full, ..self }
    }

    /// Physical pixel position (y down, as winit reports it) to world coordinates.
    pub fn screen_to_world(&self, px: Vec2) -> Vec2 {
        let clip = Vec2::new(2.0 * px.x / self.window.x - 1.0, 1.0 - 2.0 * px.y / self.window.y);
        (clip - self.offset) / self.scale
    }

    #[cfg(test)]
    fn to_clip(self, p: Vec2) -> Vec2 {
        p * self.scale + self.offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_is_centred_and_fits() {
        let domain = Vec2::new(2.0, 1.0);
        for size in [PhysicalSize::new(1280, 720), PhysicalSize::new(600, 900)] {
            let v = View::fit(domain, size, 0.0);
            let lo = v.to_clip(Vec2::ZERO);
            let hi = v.to_clip(domain);
            assert!((lo + hi).length() < 1e-5, "not centred: {lo} {hi}");
            assert!(hi.x <= 1.0 + 1e-5 && hi.y <= 1.0 + 1e-5);
            assert!((hi.x - 1.0).abs() < 1e-5 || (hi.y - 1.0).abs() < 1e-5, "doesn't touch an edge");
        }
    }

    #[test]
    fn panel_aware_view_maps_pixels_consistently() {
        // Fit into the left 700 px of a 1000 px window; a pixel inside that area must map
        // to the same world point whichever way we compute it.
        let area = View::fit(Vec2::new(2.0, 1.0), PhysicalSize::new(700, 800), 10.0);
        let full = area.with_window(PhysicalSize::new(1000, 800));
        let px = Vec2::new(350.0, 400.0);
        assert!((area.screen_to_world(px) - full.screen_to_world(px)).length() < 1e-5);
        assert!((full.screen_to_world(px) - Vec2::new(1.0, 0.5)).length() < 1e-5);
    }

    #[test]
    fn screen_to_world_inverts_the_projection() {
        let v = View::fit(Vec2::new(2.0, 1.0), PhysicalSize::new(1000, 800), 10.0);
        // The window centre is the domain centre; y grows downward on screen.
        assert!((v.screen_to_world(Vec2::new(500.0, 400.0)) - Vec2::new(1.0, 0.5)).length() < 1e-5);
        let above = v.screen_to_world(Vec2::new(500.0, 300.0));
        assert!(above.y > 0.5);
        let world = Vec2::new(0.3, 0.8);
        let clip = v.to_clip(world);
        let px = Vec2::new((clip.x + 1.0) * 500.0, (1.0 - clip.y) * 400.0);
        assert!((v.screen_to_world(px) - world).length() < 1e-5);
    }
}
