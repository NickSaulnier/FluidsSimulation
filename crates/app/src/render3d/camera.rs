//! The 3D orbit camera: yaw and pitch around a target, at a distance.

use glam::camera::rh::{proj::directx, view};
use glam::{Mat4, Vec2, Vec3};

/// Radians of turn per pixel of drag.
const ORBIT_PER_PX: f32 = 0.008;
/// Pitch stays short of straight up or down, where the view would flip.
const MAX_PITCH: f32 = 1.4;
const ZOOM_STEP: f32 = 0.9;
const FOV_Y: f32 = 0.8; // radians, about 46°

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrbitCamera {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub target: Vec3,
}

impl OrbitCamera {
    /// A three-quarter view of a box of `size` (with its minimum corner at the origin),
    /// far enough back to see all of it.
    pub fn framing(size: Vec3) -> Self {
        Self { yaw: 0.6, pitch: 0.35, distance: 1.6 * size.length(), target: 0.5 * size }
    }

    pub fn eye(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        self.target + self.distance * Vec3::new(cp * sy, sp, cp * cy)
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        let near = 0.02 * self.distance;
        // WebGPU clip space is DirectX's: depth 0..1, y up.
        directx::perspective(FOV_Y, aspect.max(1e-3), near, 10.0 * self.distance) * view::look_at_mat4(self.eye(), self.target, Vec3::Y)
    }

    /// Turn by a mouse drag of (`dx`, `dy`) pixels: right swings the view right, down
    /// looks from higher up.
    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.yaw -= dx * ORBIT_PER_PX;
        self.pitch = (self.pitch + dy * ORBIT_PER_PX).clamp(-MAX_PITCH, MAX_PITCH);
    }

    /// The horizontal direction pointing right on screen.
    pub fn right(&self) -> Vec3 {
        (self.target - self.eye()).cross(Vec3::Y).normalize_or(Vec3::X)
    }

    /// The point under screen position `ndc` (-1..1, y up) on the plane through the
    /// target facing the camera: where a drag on screen acts in the volume.
    pub fn point_on_target_plane(&self, ndc: Vec2, aspect: f32) -> Vec3 {
        let eye = self.eye();
        let far = self.view_proj(aspect).inverse().project_point3(ndc.extend(1.0));
        let dir = (far - eye).normalize();
        let normal = (self.target - eye).normalize();
        let t = (self.target - eye).dot(normal) / dir.dot(normal).max(1e-4);
        eye + dir * t
    }

    /// Zoom by wheel `steps` (positive: closer).
    pub fn zoom(&mut self, steps: f32, framing_distance: f32) {
        self.distance = (self.distance * ZOOM_STEP.powf(steps)).clamp(0.2 * framing_distance, 4.0 * framing_distance);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(m: Mat4, p: Vec3) -> Vec3 {
        let c = m * p.extend(1.0);
        c.truncate() / c.w
    }

    /// The screen's centre is the target; a point right of centre is to the camera's
    /// right, on the plane through the target.
    #[test]
    fn screen_points_land_on_the_target_plane() {
        let cam = OrbitCamera::framing(Vec3::new(1.0, 1.5, 1.0));
        assert!(cam.point_on_target_plane(Vec2::ZERO, 1.5).distance(cam.target) < 1e-4);
        let p = cam.point_on_target_plane(Vec2::new(0.5, 0.0), 1.5);
        assert!((p - cam.target).dot(cam.right()) > 0.1, "{p}");
        assert!((p - cam.target).dot(cam.target - cam.eye()).abs() < 1e-3, "off the plane");
        assert!(cam.right().y.abs() < 1e-6);
    }

    #[test]
    fn the_target_is_centred_and_in_front() {
        let cam = OrbitCamera::framing(Vec3::new(1.0, 1.5, 1.0));
        let c = clip(cam.view_proj(16.0 / 9.0), cam.target);
        assert!(c.x.abs() < 1e-5 && c.y.abs() < 1e-5, "{c}");
        assert!((0.0..1.0).contains(&c.z), "depth {}", c.z);
        assert!((cam.eye().distance(cam.target) - cam.distance).abs() < 1e-4);
    }

    #[test]
    fn up_is_up_and_the_whole_box_fits() {
        let size = Vec3::new(1.0, 1.5, 1.0);
        let cam = OrbitCamera::framing(size);
        let m = cam.view_proj(1.0);
        assert!(clip(m, cam.target + Vec3::Y * 0.1).y > 0.0, "up should project up");
        for x in [0.0, size.x] {
            for y in [0.0, size.y] {
                for z in [0.0, size.z] {
                    let c = clip(m, Vec3::new(x, y, z));
                    assert!(c.x.abs() <= 1.0 && c.y.abs() <= 1.0, "corner ({x},{y},{z}) off screen: {c}");
                }
            }
        }
    }

    #[test]
    fn orbit_clamps_pitch_and_zoom_clamps_distance() {
        let mut cam = OrbitCamera::framing(Vec3::ONE);
        let d = cam.distance;
        cam.orbit(0.0, 1e6);
        assert_eq!(cam.pitch, MAX_PITCH);
        cam.zoom(100.0, d);
        assert!((cam.distance - 0.2 * d).abs() < 1e-5);
        cam.zoom(-100.0, d);
        assert!((cam.distance - 4.0 * d).abs() < 1e-4);
    }
}
