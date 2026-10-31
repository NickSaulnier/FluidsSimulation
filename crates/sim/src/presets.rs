//! Scene presets. Solver-agnostic: they describe geometry and fluid, nothing else.

use crate::scene::{Emitter, Feed, Rect, Scene};
use glam::Vec2;

pub struct Preset {
    pub name: &'static str,
    /// The scene, built for a domain of the given size (metres; see `domain_for`).
    pub build: fn(Vec2) -> Scene,
}

pub static PRESETS: &[Preset] = &[
    Preset { name: "Dam break", build: dam_break_in },
    Preset { name: "Double dam break", build: double_dam_break_in },
    Preset { name: "Tank with obstacle", build: tank_with_obstacle_in },
    Preset { name: "Fountain", build: fountain_in },
    Preset { name: "Martin & Moyce column", build: martin_moyce_in },
    Preset { name: "Wind tunnel", build: wind_tunnel_in },
    Preset { name: "Campfire", build: campfire_in },
    Preset { name: "Pond", build: pond_in },
    Preset { name: "Breakwater", build: breakwater_in },
    Preset { name: "Falling blocks", build: falling_blocks_in },
    Preset { name: "Five bands", build: five_bands_in },
    Preset { name: "Tempo vortex", build: tempo_vortex_in },
    Preset { name: "Two jets", build: two_jets_in },
    Preset { name: "Rain", build: rain_in },
];

/// Column width for the Martin & Moyce validation case (height is twice this).
pub const MARTIN_MOYCE_WIDTH: f32 = 0.25;

/// The domain the scenes were designed in, and that tests and the bench use.
pub const DEFAULT_DOMAIN: Vec2 = Vec2::new(2.0, 1.0);
const MASK_CELLS_Y: usize = 128;
/// The narrowest and widest domains `domain_for` makes (a portrait screen, an
/// ultrawide one), so every scene still fits.
const ASPECTS: std::ops::RangeInclusive<f32> = 0.5..=3.6;

/// A domain of `aspect` (width over height) whose shorter side is 1 m, the size the
/// scenes are designed around: wider for a wide window, taller for a tall one.
pub fn domain_for(aspect: f32) -> Vec2 {
    let a = if aspect.is_finite() { aspect.clamp(*ASPECTS.start(), *ASPECTS.end()) } else { DEFAULT_DOMAIN.x };
    if a >= 1.0 { Vec2::new(a, 1.0) } else { Vec2::new(1.0, 1.0 / a) }
}

/// A box of `domain`, its walls `MASK_CELLS_Y` cells to a metre (the scenes' resolution
/// at their designed size).
fn tank(domain: Vec2) -> Scene {
    Scene::empty_box(domain, (MASK_CELLS_Y as f32 * domain.y).round() as usize)
}

pub fn dam_break() -> Scene {
    dam_break_in(DEFAULT_DOMAIN)
}

/// A block of water against the left wall, collapsing along the floor.
pub fn dam_break_in(d: Vec2) -> Scene {
    let mut s = tank(d);
    s.fluid_blocks.push(Rect::new(Vec2::new(0.0, 0.0), Vec2::new(0.7, 0.8)));
    s
}

pub fn tank_with_obstacle() -> Scene {
    tank_with_obstacle_in(DEFAULT_DOMAIN)
}

/// Calm water around a half-submerged pillar in the middle: tilt gravity to slosh it,
/// or stir.
pub fn tank_with_obstacle_in(d: Vec2) -> Scene {
    let mut s = tank(d);
    s.fluid_blocks.push(Rect::new(Vec2::new(0.0, 0.0), Vec2::new(d.x, 0.42)));
    s.solid.paint_disc(Vec2::new(0.5 * d.x, 0.42), 0.14, true);
    s.solid.update_derived();
    s
}

pub fn fountain() -> Scene {
    fountain_in(DEFAULT_DOMAIN)
}

/// A jet from the floor of a shallow pool, in the middle. Overflow drains in both
/// corners, just above the pool surface, hold the water level (and catch splashes
/// running up the walls).
pub fn fountain_in(d: Vec2) -> Scene {
    let mut s = tank(d);
    s.fluid_blocks.push(Rect::new(Vec2::new(0.0, 0.0), Vec2::new(d.x, 0.2)));
    s.emitters.push(Emitter { pos: Vec2::new(0.5 * d.x, 0.1), dir: Vec2::Y, speed: 3.2, width: 0.04, dye: 1.0 });
    for x in [0.03, d.x - 0.09] {
        s.drains.push(Rect::new(Vec2::new(x, 0.21), Vec2::new(x + 0.06, 0.6)));
    }
    s
}

pub fn martin_moyce() -> Scene {
    martin_moyce_in(DEFAULT_DOMAIN)
}

/// Martin & Moyce (1952) validation case: a column exactly a wide and 2a tall (n² = 2),
/// measured from the faces of the left wall and the floor, collapsing along the tank.
pub fn martin_moyce_in(d: Vec2) -> Scene {
    let mut s = tank(d);
    let corner = s.solid.interior().min;
    let a = MARTIN_MOYCE_WIDTH;
    s.fluid_blocks.push(Rect::new(Vec2::ZERO, corner + Vec2::new(a, 2.0 * a)));
    s
}

pub fn wind_tunnel() -> Scene {
    wind_tunnel_in(DEFAULT_DOMAIN)
}

/// Weightless flow past a cylinder: an inlet filling the left end, an outlet at the
/// right. Grid solvers with open boundaries (Lattice Boltzmann) shed a von Karman
/// vortex street behind the cylinder; particle solvers fire a slab of fluid through.
/// The cylinder sits slightly off the centreline so the wake's symmetry breaks quickly.
pub fn wind_tunnel_in(d: Vec2) -> Scene {
    let mut s = tank(d);
    s.gravity = Vec2::ZERO;
    let inner = s.solid.interior();
    // The inlet reaches into the walls (which win), so it spans the tunnel at any
    // resolution: a gap left at its ends would be the only way into the pocket behind
    // it, which it draws from, and a fine grid forced the whole inflow through it.
    let height = inner.size().y + 0.02;
    s.emitters.push(Emitter { pos: Vec2::new(inner.min.x + 0.01, 0.5 * d.y), dir: Vec2::X, speed: 1.0, width: height, dye: 1.0 });
    s.drains.push(Rect::new(Vec2::new(inner.max.x - 0.12, inner.min.y), inner.max));
    s.solid.paint_disc(Vec2::new(0.5, 0.52 * d.y), 0.06, true);
    s.solid.update_derived();
    s
}

pub fn campfire() -> Scene {
    campfire_in(DEFAULT_DOMAIN)
}

/// A wide, gentle burner on the floor in the open: the top and the sides (above a low
/// sill) are open, through drains in the border just outside the window, so the
/// smoke-and-fire solver's smoke rises out of the picture instead of pooling under a
/// ceiling. (Other solvers get a slow upwelling jet, its water spilling out at the
/// sides.)
pub fn campfire_in(d: Vec2) -> Scene {
    let mut s = tank(d);
    let inner = s.solid.interior();
    // Slow and wide, so buoyancy rather than the jet shapes the flame.
    s.emitters.push(Emitter { pos: Vec2::new(0.5 * d.x, inner.min.y + 0.01), dir: Vec2::Y, speed: 0.3, width: 0.24, dye: 1.0 });
    // Each reaches a centimetre into the picture, so particles at the edge leave too.
    let (reach, sill) = (0.01, inner.min.y + 0.15);
    s.drains.push(Rect::new(Vec2::new(0.0, inner.max.y - reach), d));
    s.drains.push(Rect::new(Vec2::new(0.0, sill), Vec2::new(inner.min.x + reach, d.y)));
    s.drains.push(Rect::new(Vec2::new(inner.max.x - reach, sill), d));
    s
}

pub fn double_dam_break() -> Scene {
    double_dam_break_in(DEFAULT_DOMAIN)
}

/// Two blocks against the side walls, meeting in the middle.
pub fn double_dam_break_in(d: Vec2) -> Scene {
    let mut s = tank(d);
    s.fluid_blocks.push(Rect::new(Vec2::new(0.0, 0.0), Vec2::new(0.45, 0.7)));
    s.fluid_blocks.push(Rect::new(Vec2::new(d.x - 0.45, 0.0), Vec2::new(d.x, 0.7)));
    s
}

/// Rocks in still water, and a stone dropped among them: made for shallow water (seen
/// from above, the stone's splash rings out and wraps round the rocks). Other solvers
/// get a small blob falling into an empty tank: pour with the faucet.
pub fn pond_in(d: Vec2) -> Scene {
    let mut s = tank(d);
    let c = Vec2::new(0.5 * d.x, 0.5 * d.y);
    for (offset, radius) in [(Vec2::new(-0.45, 0.12), 0.07), (Vec2::new(0.3, -0.2), 0.1), (Vec2::new(0.55, 0.22), 0.05), (Vec2::new(-0.2, -0.28), 0.04)] {
        s.solid.paint_disc(c + offset, radius, true);
    }
    s.solid.update_derived();
    s.fluid_blocks.push(Rect::new(c - 0.05, c + 0.05));
    s
}

/// A wall across the water with two gaps, and a band of raised water below it: made for
/// shallow water (seen from above, the band rolls up as one wave, and each gap spreads
/// it out again in rings that cross beyond the wall). Other solvers get a shallow pool
/// under a shelf with two holes.
pub fn breakwater_in(d: Vec2) -> Scene {
    let mut s = tank(d);
    let y = 0.42 * d.y;
    let gaps = [0.36 * d.x, 0.64 * d.x];
    let (half_gap, thick) = (0.05, 0.02);
    let mut x = 0.0;
    while x <= d.x {
        if gaps.iter().all(|g| (x - g).abs() > half_gap + thick) {
            s.solid.paint_disc(Vec2::new(x, y), thick, true);
        }
        x += 0.5 * thick;
    }
    s.solid.update_derived();
    s.fluid_blocks.push(Rect::new(Vec2::ZERO, Vec2::new(d.x, 0.15 * d.y)));
    s
}

/// Three blocks in mid-air, at different heights, over a shallow layer: made for the
/// MPM materials (jelly lands and wobbles, snow packs or bursts, sand pours), but every
/// solver drops them somehow (liquids splash; shallow water, seen from above, rings).
pub fn falling_blocks_in(d: Vec2) -> Scene {
    let mut s = tank(d);
    let inner = s.solid.interior();
    s.fluid_blocks.push(Rect::new(inner.min, Vec2::new(inner.max.x, inner.min.y + 0.06)));
    let side = 0.18;
    for (x, y) in [(0.25, 0.45), (0.5, 0.75), (0.75, 0.55)] {
        let c = Vec2::new(x * d.x, y * d.y);
        s.fluid_blocks.push(Rect::new(c - 0.5 * side, c + 0.5 * side));
    }
    s
}

/// Weightless and open on every side, through drains in the border just outside the
/// window (each reaching a centimetre in), so what the feeds pour in leaves again
/// instead of filling a closed box: the music scenes' frame.
fn open(d: Vec2) -> Scene {
    let mut s = tank(d);
    s.gravity = Vec2::ZERO;
    let inner = s.solid.interior();
    let reach = 0.01;
    s.drains.push(Rect::new(Vec2::ZERO, Vec2::new(d.x, inner.min.y + reach)));
    s.drains.push(Rect::new(Vec2::new(0.0, inner.max.y - reach), d));
    s.drains.push(Rect::new(Vec2::ZERO, Vec2::new(inner.min.x + reach, d.y)));
    s.drains.push(Rect::new(Vec2::new(inner.max.x - reach, 0.0), d));
    s
}

/// Five feeds round the window, one per band (the lowest at the bottom, a lighter shade
/// for each band up), each pouring in with its own band: made for the dye solvers.
/// They aim past the middle, so the streams curl round each other.
pub fn five_bands_in(d: Vec2) -> Scene {
    let mut s = open(d);
    let inner = s.solid.interior();
    let (middle, reach) = (0.5 * d, 0.5 * inner.size() - 0.08);
    for band in 0..5 {
        let angle = -std::f32::consts::FRAC_PI_2 + band as f32 * std::f32::consts::TAU / 5.0;
        let pos = middle + Vec2::from_angle(angle) * reach;
        let dir = Vec2::from_angle(0.35).rotate((middle - pos).normalize());
        let emitter = Emitter { pos, dir, speed: 1.6, width: 0.06, dye: 0.4 + 0.15 * band as f32 };
        s.feeds.push(Feed { emitter, band, turns_per_bar: 0.0 });
    }
    s
}

/// Two feeds on opposite sides of a circle round the middle, pouring along it and
/// going round it once a bar: the bass's bright, the mids' darker. Their streams wind
/// up a swirl in the middle that turns with the bar.
pub fn tempo_vortex_in(d: Vec2) -> Scene {
    let mut s = open(d);
    let (middle, r) = (0.5 * d, 0.28 * d.min_element());
    let feed = |side: f32, speed, dye, band| Feed { emitter: Emitter { pos: middle + Vec2::new(side * r, 0.0), dir: Vec2::new(0.0, side), speed, width: 0.06, dye }, band, turns_per_bar: 1.0 };
    s.feeds.push(feed(1.0, 1.6, 1.0, 1));
    s.feeds.push(feed(-1.0, 1.2, 0.5, 3));
    s
}

/// Two jets facing each other across the middle, a little apart so they curl where
/// they meet: the bass's from the left, bright, the treble's from the right, darker.
pub fn two_jets_in(d: Vec2) -> Scene {
    let mut s = open(d);
    let inner = s.solid.interior();
    let (y, apart) = (0.5 * d.y, 0.05 * d.y);
    let left = Emitter { pos: Vec2::new(inner.min.x + 0.05, y + apart), dir: Vec2::X, speed: 2.2, width: 0.08, dye: 1.0 };
    let right = Emitter { pos: Vec2::new(inner.max.x - 0.05, y - apart), dir: Vec2::NEG_X, speed: 2.2, width: 0.08, dye: 0.45 };
    s.feeds.push(Feed { emitter: left, band: 1, turns_per_bar: 0.0 });
    s.feeds.push(Feed { emitter: right, band: 4, turns_per_bar: 0.0 });
    s
}

/// Open water with nothing in it: made for shallow water, the look's drops (on the
/// hits) ringing out and crossing. Other solvers get an empty box.
pub fn rain_in(d: Vec2) -> Scene {
    tank(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Domains keep the window's shape with the shorter side 1 m, within limits.
    #[test]
    fn domains_follow_the_window() {
        assert_eq!(domain_for(16.0 / 9.0), Vec2::new(16.0 / 9.0, 1.0));
        assert_eq!(domain_for(0.5), Vec2::new(1.0, 2.0));
        assert_eq!(domain_for(10.0), Vec2::new(3.6, 1.0));
        assert_eq!(domain_for(f32::NAN), DEFAULT_DOMAIN);
    }

    /// Every scene, at every shape from portrait to ultrawide, has its fluid, emitters
    /// and drains inside its walls, and its domain is the one asked for.
    #[test]
    fn every_scene_fits_any_shape() {
        for aspect in [0.5, 1.0, 16.0 / 9.0, 2.0, 3.6] {
            let d = domain_for(aspect);
            for p in PRESETS {
                let s = (p.build)(d);
                assert_eq!(s.size, d, "{} at {aspect}", p.name);
                let inner = s.solid.interior();
                let inside = |q: Vec2| q.x >= 0.0 && q.y >= 0.0 && q.x <= d.x && q.y <= d.y;
                for b in &s.fluid_blocks {
                    assert!(inside(b.min) && inside(b.max) && b.size().x > 0.0, "{} at {aspect}: block {b:?}", p.name);
                }
                for e in &s.emitters {
                    assert!(inner.contains(e.pos), "{} at {aspect}: emitter at {}", p.name, e.pos);
                }
                // Feeds, wherever they turn to.
                for f in &s.feeds {
                    let r = (f.emitter.pos - 0.5 * d).length();
                    let reach = if f.turns_per_bar != 0.0 { Rect::new(0.5 * d - r, 0.5 * d + r) } else { Rect::new(f.emitter.pos, f.emitter.pos) };
                    assert!(inner.contains(reach.min) && inner.contains(reach.max), "{} at {aspect}: feed at {}", p.name, f.emitter.pos);
                    assert!(f.band < 5, "{}: band {}", p.name, f.band);
                }
                for r in &s.drains {
                    assert!(inside(r.min) && inside(r.max), "{} at {aspect}: drain {r:?}", p.name);
                }
            }
        }
    }

    /// The campfire's smoke leaves through its open top and sides instead of pooling
    /// under a ceiling: after a while there's much less smoke up high than in the same
    /// scene closed.
    #[test]
    fn campfire_smoke_leaves_through_the_open_edges() {
        let solver = crate::SOLVERS.iter().position(|s| s.name == "Smoke & fire").expect("the smoke solver");
        let campfire = PRESETS.iter().position(|p| p.name == "Campfire").expect("the campfire");
        // A coarse grid, so the test stays quick.
        struct Coarse;
        impl crate::ParamVisitor for Coarse {
            fn float(&mut self, _: &crate::FloatSpec, _: &mut f32) {}
            fn int(&mut self, spec: &crate::IntSpec, value: &mut u32) {
                if spec.key == "cells_y" {
                    *value = 48;
                }
            }
            fn toggle(&mut self, _: &crate::ToggleSpec, _: &mut bool) {}
        }
        let smoke_up_high = |open: bool| {
            let mut world = crate::World::new(solver, campfire);
            world.solver.visit_params(&mut Coarse);
            if !open {
                world.scene.drains.clear();
            }
            world.reset();
            for _ in 0..5 * 60 {
                world.advance(1.0 / 60.0, &crate::StepInput::default());
            }
            let index = world.solver.fields().iter().position(|f| f.key == "smoke").expect("a smoke field");
            let mut values = Vec::new();
            let layout = world.solver.fill_field(index, &mut values).expect("smoke");
            let top = world.scene.solid.interior().max.y;
            let mut sum = 0.0;
            for j in 0..layout.ny {
                let y = (j as f32 + 0.5) * layout.cell;
                if y > top - 0.2 && y < top {
                    sum += (0..layout.nx).map(|i| values[j * layout.nx + i]).filter(|v| v.is_finite()).sum::<f32>();
                }
            }
            sum
        };
        let (open, closed) = (smoke_up_high(true), smoke_up_high(false));
        assert!(closed > 0.0, "no smoke reached the top");
        assert!(open < 0.5 * closed, "open {open} against closed {closed}");
    }
}
