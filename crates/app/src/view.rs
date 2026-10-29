//! World (metres, y up) to clip space: a part of the domain (the inside of the scene's
//! walls) covering the canvas edge to edge, whatever sticks out cropped (`cover`).
//! (`fit_in`, letterboxing the whole domain, is kept for comparison in the tests.)

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
    #[cfg(test)]
    fn fit(domain: Vec2, window: PhysicalSize<u32>, margin_px: f32) -> Self {
        let win = Vec2::new(window.width as f32, window.height as f32);
        Self::fit_in(domain, Vec2::ZERO, win, margin_px, window)
    }

    /// Fit the domain, centred, into the area of the window (physical pixels, y down)
    /// at `area_min` of size `area_size`: the canvas left over by the UI's bars and
    /// panel.
    #[cfg(test)]
    pub fn fit_in(domain: Vec2, area_min: Vec2, area_size: Vec2, margin_px: f32, window: PhysicalSize<u32>) -> Self {
        let win = Vec2::new(window.width as f32, window.height as f32).max(Vec2::ONE);
        let avail = (area_size - 2.0 * margin_px).max(Vec2::ONE);
        let k = (avail / domain).min_element();
        // The domain centre sits at the area centre c: pixel = c + (world - domain/2) * k
        // with y flipped, then clip = 2 * pixel / win - 1 (y up).
        let c = area_min + 0.5 * area_size;
        let offset = Vec2::new(
            2.0 * (c.x - 0.5 * domain.x * k) / win.x - 1.0,
            1.0 - 2.0 * (c.y + 0.5 * domain.y * k) / win.y,
        );
        Self { scale: 2.0 * k / win, offset, px_per_metre: k, window: win }
    }

    /// Show the world rectangle at `visible_min`, `visible_size` big, centred, covering
    /// the area of the window at `area_min` of size `area_size` (physical pixels, y
    /// down): it fills the area, and whatever sticks out on one axis is cropped.
    pub fn cover(visible_min: Vec2, visible_size: Vec2, area_min: Vec2, area_size: Vec2, window: PhysicalSize<u32>) -> Self {
        let win = Vec2::new(window.width as f32, window.height as f32).max(Vec2::ONE);
        let k = (area_size.max(Vec2::ONE) / visible_size.max(Vec2::splat(1e-6))).max_element();
        // The visible centre v sits at the area centre c: pixel = c + (world - v) * k
        // with y flipped, then clip = 2 * pixel / win - 1 (y up).
        let (c, v) = (area_min + 0.5 * area_size, visible_min + 0.5 * visible_size);
        let offset = Vec2::new(2.0 * (c.x - v.x * k) / win.x - 1.0, 1.0 - 2.0 * (c.y + v.y * k) / win.y);
        Self { scale: 2.0 * k / win, offset, px_per_metre: k, window: win }
    }

    /// Physical pixel position (y down, as winit reports it) to world coordinates.
    pub fn screen_to_world(&self, px: Vec2) -> Vec2 {
        let clip = Vec2::new(2.0 * px.x / self.window.x - 1.0, 1.0 - 2.0 * px.y / self.window.y);
        (clip - self.offset) / self.scale
    }

    #[cfg(test)]
    pub fn to_clip(self, p: Vec2) -> Vec2 {
        p * self.scale + self.offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Covering: the visible rectangle's centre at the area's, and it fills the area,
    /// cropped on the axis where it's too long.
    #[test]
    fn cover_fills_the_area() {
        let window = PhysicalSize::new(1600, 900);
        let (min, size) = (Vec2::new(0.03, 0.03), Vec2::new(1.94, 0.94));
        let v = View::cover(min, size, Vec2::ZERO, Vec2::new(1600.0, 900.0), window);
        let centre = v.to_clip(min + 0.5 * size);
        assert!(centre.length() < 1e-5, "{centre}");
        // Wider than the window: the height fills it, the sides are cropped.
        assert!((v.to_clip(min).y + 1.0).abs() < 1e-5 && (v.to_clip(min + size).y - 1.0).abs() < 1e-5);
        assert!(v.to_clip(min).x < -1.0 && v.to_clip(min + size).x > 1.0);
        // The same shape as the area: it fits exactly.
        let exact = View::cover(Vec2::ZERO, Vec2::new(16.0, 9.0), Vec2::ZERO, Vec2::new(1600.0, 900.0), window);
        assert!((exact.to_clip(Vec2::ZERO) + Vec2::ONE).length() < 1e-5 && (exact.to_clip(Vec2::new(16.0, 9.0)) - Vec2::ONE).length() < 1e-5);
    }

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
    fn canvas_fit_centres_the_domain_in_its_area() {
        // A canvas below a top bar and left of a side panel.
        let (min, size) = (Vec2::new(0.0, 40.0), Vec2::new(700.0, 700.0));
        let v = View::fit_in(Vec2::new(2.0, 1.0), min, size, 10.0, PhysicalSize::new(1000, 800));
        let centre = min + 0.5 * size;
        assert!((v.screen_to_world(centre) - Vec2::new(1.0, 0.5)).length() < 1e-5);
        // The domain's left and right edges sit `margin` inside the area (width-limited).
        assert!((v.screen_to_world(Vec2::new(10.0, centre.y)).x).abs() < 1e-4);
        assert!((v.screen_to_world(Vec2::new(690.0, centre.y)).x - 2.0).abs() < 1e-4);
        assert!((v.px_per_metre - 340.0).abs() < 1e-3);
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
