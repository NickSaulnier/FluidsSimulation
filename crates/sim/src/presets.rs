//! Scene presets. Solver-agnostic: they describe geometry and fluid, nothing else.

use crate::scene::{Emitter, Rect, Scene};
use glam::Vec2;

pub struct Preset {
    pub name: &'static str,
    pub build: fn() -> Scene,
}

pub static PRESETS: &[Preset] = &[
    Preset { name: "Dam break", build: dam_break },
    Preset { name: "Double dam break", build: double_dam_break },
    Preset { name: "Tank with obstacle", build: tank_with_obstacle },
    Preset { name: "Fountain", build: fountain },
    Preset { name: "Martin & Moyce column", build: martin_moyce },
];

/// Column width for the Martin & Moyce validation case (height is twice this).
pub const MARTIN_MOYCE_WIDTH: f32 = 0.25;

const DOMAIN: Vec2 = Vec2::new(2.0, 1.0);
const MASK_CELLS_Y: usize = 128;

pub fn dam_break() -> Scene {
    let mut s = Scene::empty_box(DOMAIN, MASK_CELLS_Y);
    s.fluid_blocks.push(Rect::new(Vec2::new(0.0, 0.0), Vec2::new(0.7, 0.8)));
    s
}

/// Calm water around a half-submerged pillar: tilt gravity to slosh it, or stir.
pub fn tank_with_obstacle() -> Scene {
    let mut s = Scene::empty_box(DOMAIN, MASK_CELLS_Y);
    s.fluid_blocks.push(Rect::new(Vec2::new(0.0, 0.0), Vec2::new(2.0, 0.42)));
    s.solid.paint_disc(Vec2::new(1.0, 0.42), 0.14, true);
    s.solid.update_derived();
    s
}

/// A jet from the floor of a shallow pool. Overflow drains in both corners, just above
/// the pool surface, hold the water level (and catch splashes running up the walls).
pub fn fountain() -> Scene {
    let mut s = Scene::empty_box(DOMAIN, MASK_CELLS_Y);
    s.fluid_blocks.push(Rect::new(Vec2::new(0.0, 0.0), Vec2::new(2.0, 0.2)));
    s.emitters.push(Emitter { pos: Vec2::new(1.0, 0.1), dir: Vec2::Y, speed: 3.2, width: 0.04 });
    for x in [0.03, 1.91] {
        s.drains.push(Rect::new(Vec2::new(x, 0.21), Vec2::new(x + 0.06, 0.6)));
    }
    s
}

/// Martin & Moyce (1952) validation case: a column exactly a wide and 2a tall (n² = 2),
/// measured from the faces of the left wall and the floor, collapsing along the tank.
pub fn martin_moyce() -> Scene {
    let mut s = Scene::empty_box(DOMAIN, MASK_CELLS_Y);
    let corner = s.solid.interior().min;
    let a = MARTIN_MOYCE_WIDTH;
    s.fluid_blocks.push(Rect::new(Vec2::ZERO, corner + Vec2::new(a, 2.0 * a)));
    s
}

pub fn double_dam_break() -> Scene {
    let mut s = Scene::empty_box(DOMAIN, MASK_CELLS_Y);
    s.fluid_blocks.push(Rect::new(Vec2::new(0.0, 0.0), Vec2::new(0.45, 0.7)));
    s.fluid_blocks.push(Rect::new(Vec2::new(1.55, 0.0), Vec2::new(2.0, 0.7)));
    s
}
