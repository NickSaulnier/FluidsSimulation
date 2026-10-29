//! Music reactions in 3D. The reaction engine works in 2D; here it sees the volume
//! from the camera, a front view with x across the screen and y up, and its output is
//! lifted into the volume along the camera's horizontal right:
//! - Splashes and gusts push the same way on screen; vortices (and the idle drift)
//!   spin about the axis toward the camera, so they read as swirls on screen.
//! - Jets become burner bursts on the floor: flame puffs for fire, smoke puffs for
//!   smoke.
//! - Slosh tilts gravity across the screen; Breathe scales it.
//!
//! The camera orbits in the visualizer, so lifted pushes turn with the view: a gust
//! that blows left to right on screen keeps doing so from every side.

use crate::audio::reactions::ReactionOutput;
use glam::{Vec2, Vec3};
use sim3d::{Brush3d, Burner, StepInput3d};

/// Height of a jet's burner above the floor, metres.
const JET_HEIGHT: f32 = 0.03;

/// The 2D domain the engine should see for a volume of `size`.
pub fn front_view(size: Vec3) -> Vec2 {
    Vec2::new(size.x, size.y)
}

/// The 2D gravity the engine should rock and scale, for the scene's own `gravity`.
pub fn front_gravity(gravity: Vec3, right: Vec3) -> Vec2 {
    Vec2::new(gravity.dot(right), gravity.y)
}

/// Lift the engine's output for the front view into the volume of `size`, seen with
/// `right` pointing right on screen (horizontal, unit). `base_gravity` is the scene's
/// own. Returns the brushes and burners to add, and gravity for this frame.
pub fn lift(out: &ReactionOutput, size: Vec3, right: Vec3, base_gravity: Vec3) -> (StepInput3d, Vec3) {
    let toward_camera = right.cross(Vec3::Y);
    // A front-view point: across the screen through the middle of the volume, at its
    // height, kept inside the box.
    let place = |p: Vec2| {
        let centre = Vec3::new(0.5 * size.x, 0.0, 0.5 * size.z);
        (centre + (p.x - 0.5 * size.x) * right + p.y * Vec3::Y).clamp(Vec3::ZERO, size)
    };
    let direction = |v: Vec2| v.x * right + v.y * Vec3::Y;
    let mut input = StepInput3d::default();
    for b in &out.brushes {
        input.brushes.push(Brush3d {
            pos: place(b.pos),
            radius: b.radius,
            velocity: direction(b.velocity),
            spin: b.spin * toward_camera,
            strength: b.strength,
        });
    }
    for e in &out.emitters {
        let pos = place(e.pos);
        input.burners.push(Burner {
            pos: Vec3::new(pos.x, JET_HEIGHT, pos.z),
            dir: direction(e.dir).normalize_or(Vec3::Y),
            radius: e.width,
            speed: e.speed,
        });
    }
    // The part of the scene's gravity along the view (toward or away from the camera),
    // which the front view doesn't see, carries over as it is.
    let along_view = base_gravity - base_gravity.dot(right) * right - base_gravity.y * Vec3::Y;
    let gravity = direction(out.gravity) + along_view;
    (input, gravity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim::{Emitter, ForceBrush};

    const SIZE: Vec3 = Vec3::new(1.0, 1.5, 1.0);
    const G: Vec3 = Vec3::new(0.0, -9.81, 0.0);

    fn output() -> ReactionOutput {
        ReactionOutput { gravity: front_gravity(G, Vec3::X), ..ReactionOutput::default() }
    }

    /// Gravity pointing any way survives the trip through the front view: the part
    /// across the screen through it, the part along the view as it is.
    #[test]
    fn sideways_gravity_comes_back_as_it_was() {
        let right = Vec3::new(0.6, 0.0, -0.8);
        for g in [Vec3::new(9.81, 0.0, 0.0), Vec3::new(2.0, 5.0, -3.0), Vec3::new(0.0, -4.0, 7.0)] {
            let out = ReactionOutput { gravity: front_gravity(g, right), ..ReactionOutput::default() };
            let (_, back) = lift(&out, SIZE, right, g);
            assert!((back - g).length() < 1e-4, "{g} came back as {back}");
        }
    }

    #[test]
    fn without_reactions_gravity_is_the_scenes() {
        let (input, g) = lift(&output(), SIZE, Vec3::X, G);
        assert!(input.brushes.is_empty() && input.burners.is_empty());
        assert!((g - G).length() < 1e-6);
    }

    /// A push to the right on screen pushes along the camera's right, whichever way the
    /// camera faces, and the brush stays inside the volume.
    #[test]
    fn pushes_follow_the_screen() {
        let right = Vec3::new(1.0, 0.0, -1.0).normalize();
        let mut out = output();
        out.brushes.push(ForceBrush { pos: Vec2::new(0.9, 0.2), radius: 0.2, velocity: Vec2::new(2.0, 1.0), strength: 5.0, spin: 0.0 });
        let (input, _) = lift(&out, SIZE, right, G);
        let b = input.brushes[0];
        assert!((b.velocity - (2.0 * right + Vec3::Y)).length() < 1e-5, "{}", b.velocity);
        assert!(b.pos.cmpge(Vec3::ZERO).all() && b.pos.cmple(SIZE).all(), "outside: {}", b.pos);
        assert!((b.pos.y - 0.2).abs() < 1e-6);
    }

    /// A 2D anticlockwise swirl spins about the axis toward the camera (anticlockwise
    /// as seen on screen).
    #[test]
    fn swirls_face_the_camera() {
        let mut out = output();
        out.brushes.push(ForceBrush { pos: Vec2::new(0.5, 0.7), radius: 0.2, velocity: Vec2::ZERO, strength: 5.0, spin: 3.0 });
        let (input, _) = lift(&out, SIZE, Vec3::X, G);
        // Camera on +z looking toward -z: right is +x, toward the camera is +z.
        assert!((input.brushes[0].spin - Vec3::new(0.0, 0.0, 3.0)).length() < 1e-6, "{}", input.brushes[0].spin);
    }

    #[test]
    fn jets_become_burners_on_the_floor() {
        let mut out = output();
        out.emitters.push(Emitter { pos: Vec2::new(0.3, 0.07), dir: Vec2::Y, speed: 3.0, width: 0.04 });
        let (input, _) = lift(&out, SIZE, Vec3::X, G);
        let b = input.burners[0];
        assert_eq!(b.pos.y, JET_HEIGHT);
        assert!((b.dir - Vec3::Y).length() < 1e-6 && b.speed == 3.0);
    }

    /// Slosh's tilt to the right on screen leans gravity along the camera's right.
    #[test]
    fn slosh_tilts_across_the_screen() {
        let right = Vec3::new(0.0, 0.0, 1.0);
        let mut out = output();
        out.gravity = Vec2::from_angle(20f32.to_radians()).rotate(front_gravity(G, Vec3::X));
        let (_, g) = lift(&out, SIZE, right, G);
        assert!((g.length() - 9.81).abs() < 1e-4);
        assert!(g.x.abs() < 1e-6 && g.z.abs() > 3.0, "{g}");
    }
}
