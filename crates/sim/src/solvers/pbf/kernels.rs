//! 2D SPH smoothing kernels (Müller et al. 2003), with 2D normalisation constants.
//!
//! Most references quote the 3D constants (315/64πh⁹, 15/πh⁶); using those in 2D gives
//! densities off by a resolution-dependent factor.

use glam::Vec2;
use std::f32::consts::PI;

#[derive(Clone, Copy, Debug)]
pub(super) struct Kernels {
    h: f32,
    h2: f32,
    /// poly6: W(r) = 4/(π h⁸) (h² - r²)³
    poly6: f32,
    /// spiky: W(r) = 10/(π h⁵) (h - r)³, so dW/dr = -30/(π h⁵) (h - r)²
    spiky_grad: f32,
}

impl Kernels {
    pub fn new(h: f32) -> Self {
        Self { h, h2: h * h, poly6: 4.0 / (PI * h.powi(8)), spiky_grad: -30.0 / (PI * h.powi(5)) }
    }

    pub fn radius(&self) -> f32 {
        self.h
    }

    /// Density kernel, from the squared distance.
    #[inline]
    pub fn w(&self, r2: f32) -> f32 {
        if r2 >= self.h2 {
            return 0.0;
        }
        let d = self.h2 - r2;
        self.poly6 * d * d * d
    }

    /// Gradient (with respect to particle i) of the spiky kernel at `d = x_i - x_j`.
    /// Points from i toward j, since W falls off with distance.
    #[inline]
    pub fn grad_w(&self, d: Vec2) -> Vec2 {
        let r = d.length();
        if r >= self.h || r < 1e-9 * self.h {
            return Vec2::ZERO;
        }
        let f = self.h - r;
        d * (self.spiky_grad * f * f / r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Integrate f over a fine grid covering the kernel's support.
    fn integrate(h: f32, f: impl Fn(Vec2) -> f32) -> f32 {
        let n = 400;
        let dx = 2.0 * h / n as f32;
        let mut sum = 0.0;
        for j in 0..n {
            for i in 0..n {
                let p = Vec2::new(-h + (i as f32 + 0.5) * dx, -h + (j as f32 + 0.5) * dx);
                sum += f(p);
            }
        }
        sum * dx * dx
    }

    #[test]
    fn kernels_integrate_to_one_in_2d() {
        let h = 0.013;
        let k = Kernels::new(h);
        let poly6 = integrate(h, |p| k.w(p.length_squared()));
        assert!((poly6 - 1.0).abs() < 1e-3, "poly6 integrates to {poly6}");
        let spiky = integrate(h, |p| {
            let r = p.length();
            if r < h { 10.0 / (PI * h.powi(5)) * (h - r).powi(3) } else { 0.0 }
        });
        assert!((spiky - 1.0).abs() < 1e-3, "spiky integrates to {spiky}");
    }

    #[test]
    fn spiky_gradient_matches_finite_differences() {
        let h = 0.02;
        let k = Kernels::new(h);
        let w = |p: Vec2| {
            let r = p.length();
            if r < h { 10.0 / (PI * h.powi(5)) * (h - r).powi(3) } else { 0.0 }
        };
        let e = 1e-5;
        for p in [Vec2::new(0.005, 0.0), Vec2::new(0.004, -0.009), Vec2::new(-0.012, 0.007)] {
            let fd = Vec2::new(w(p + Vec2::X * e) - w(p - Vec2::X * e), w(p + Vec2::Y * e) - w(p - Vec2::Y * e)) / (2.0 * e);
            let g = k.grad_w(p);
            assert!((g - fd).length() < 1e-3 * fd.length().max(1.0), "at {p}: {g} vs {fd}");
            // Points toward the neighbour at the origin.
            assert!(g.dot(p) < 0.0);
        }
    }
}
