//! World (metres, y up) to clip space, fitting the domain into the window with letterboxing.

use glam::Vec2;
use winit::dpi::PhysicalSize;

#[derive(Clone, Copy, Debug)]
pub struct View {
    /// clip = world * scale + offset
    pub scale: Vec2,
    pub offset: Vec2,
}

impl View {
    pub fn fit(domain: Vec2, window: PhysicalSize<u32>, margin_px: f32) -> Self {
        let win = Vec2::new(window.width as f32, window.height as f32).max(Vec2::ONE);
        let avail = (win - 2.0 * margin_px).max(Vec2::ONE);
        let k = (avail / domain).min_element();
        // pixel = world * k + (win - domain * k) / 2, then clip = 2 * pixel / win - 1.
        Self { scale: 2.0 * k / win, offset: -domain * k / win }
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
}
