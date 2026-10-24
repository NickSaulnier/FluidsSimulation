//! Scene presets. Solver-agnostic: they describe geometry and fluid, nothing else.

use crate::scene::{Rect, Scene};
use glam::Vec2;

pub struct Preset {
    pub name: &'static str,
    pub build: fn() -> Scene,
}

pub static PRESETS: &[Preset] = &[
    Preset { name: "Dam break", build: dam_break },
    Preset { name: "Double dam break", build: double_dam_break },
];

const DOMAIN: Vec2 = Vec2::new(2.0, 1.0);
const MASK_CELLS_Y: usize = 128;

pub fn dam_break() -> Scene {
    let mut s = Scene::empty_box(DOMAIN, MASK_CELLS_Y);
    s.fluid_blocks.push(Rect::new(Vec2::new(0.0, 0.0), Vec2::new(0.7, 0.8)));
    s
}

pub fn double_dam_break() -> Scene {
    let mut s = Scene::empty_box(DOMAIN, MASK_CELLS_Y);
    s.fluid_blocks.push(Rect::new(Vec2::new(0.0, 0.0), Vec2::new(0.45, 0.7)));
    s.fluid_blocks.push(Rect::new(Vec2::new(1.55, 0.0), Vec2::new(2.0, 0.7)));
    s
}
