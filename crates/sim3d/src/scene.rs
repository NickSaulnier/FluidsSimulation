//! 3D scenes: a box domain (metres, y up), gravity, burners, vents and solid shapes.
//! Solids are analytic shapes (spheres and boxes), not painted voxels.

use glam::Vec3;

/// A burner: releases gas (lit fuel, for fire) through a disc facing `dir`.
#[derive(Clone, Copy, Debug)]
pub struct Burner {
    pub pos: Vec3,
    pub dir: Vec3,
    pub radius: f32,
    /// Exit speed, m/s.
    pub speed: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub fn new(min: Vec3, max: Vec3) -> Self {
        Self { min, max }
    }

    pub fn contains(&self, p: Vec3) -> bool {
        p.cmpge(self.min).all() && p.cmple(self.max).all()
    }

    /// Signed distance: negative inside.
    pub fn distance(&self, p: Vec3) -> f32 {
        let centre = 0.5 * (self.min + self.max);
        let half = 0.5 * (self.max - self.min);
        let q = (p - centre).abs() - half;
        q.max(Vec3::ZERO).length() + q.max_element().min(0.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    Sphere { centre: Vec3, radius: f32 },
    Cuboid(Aabb),
}

impl Shape {
    /// Signed distance: negative inside.
    pub fn distance(&self, p: Vec3) -> f32 {
        match self {
            Shape::Sphere { centre, radius } => p.distance(*centre) - radius,
            Shape::Cuboid(b) => b.distance(p),
        }
    }
}

/// Regions open to the outside (pressure fixed, gas leaves freely).
#[derive(Clone, Debug)]
pub struct Scene3d {
    pub size: Vec3,
    pub gravity: Vec3,
    pub burners: Vec<Burner>,
    pub vents: Vec<Aabb>,
    pub solids: Vec<Shape>,
}

impl Scene3d {
    pub fn empty_box(size: Vec3) -> Self {
        Self { size, gravity: Vec3::new(0.0, -9.81, 0.0), burners: Vec::new(), vents: Vec::new(), solids: Vec::new() }
    }

    pub fn domain(&self) -> Aabb {
        Aabb::new(Vec3::ZERO, self.size)
    }

    pub fn is_solid(&self, p: Vec3) -> bool {
        self.solids.iter().any(|s| s.distance(p) < 0.0)
    }
}

pub struct Preset3d {
    pub name: &'static str,
    pub build: fn() -> Scene3d,
}

pub static PRESETS: &[Preset3d] = &[
    Preset3d { name: "Campfire 3D", build: campfire },
    Preset3d { name: "Torch", build: torch },
    Preset3d { name: "Twin burners", build: twin_burners },
    Preset3d { name: "Chimney", build: chimney },
];

const DOMAIN: Vec3 = Vec3::new(1.0, 1.5, 1.0);
/// Depth of the vent across the top of most scenes.
const VENT: f32 = 0.06;

fn top_vent() -> Aabb {
    Aabb::new(Vec3::new(0.0, DOMAIN.y - VENT, 0.0), DOMAIN)
}

fn floor_burner(x: f32, z: f32, radius: f32, speed: f32) -> Burner {
    Burner { pos: Vec3::new(x, 0.03, z), dir: Vec3::Y, radius, speed }
}

/// A wide, gentle burner on the floor, open at the top.
pub fn campfire() -> Scene3d {
    let mut s = Scene3d::empty_box(DOMAIN);
    s.burners.push(floor_burner(0.5, 0.5, 0.14, 0.3));
    s.vents.push(top_vent());
    s
}

/// A narrow, fast jet of flame.
pub fn torch() -> Scene3d {
    let mut s = Scene3d::empty_box(DOMAIN);
    s.burners.push(floor_burner(0.5, 0.5, 0.05, 1.2));
    s.vents.push(top_vent());
    s
}

/// Two burners with a ball hanging between their plumes.
pub fn twin_burners() -> Scene3d {
    let mut s = Scene3d::empty_box(DOMAIN);
    s.burners.push(floor_burner(0.3, 0.5, 0.08, 0.5));
    s.burners.push(floor_burner(0.7, 0.5, 0.08, 0.5));
    s.solids.push(Shape::Sphere { centre: Vec3::new(0.5, 0.8, 0.5), radius: 0.14 });
    s.vents.push(top_vent());
    s
}

/// A broad burner under a roof with a square flue in the middle: the smoke funnels out.
pub fn chimney() -> Scene3d {
    let mut s = Scene3d::empty_box(DOMAIN);
    s.burners.push(floor_burner(0.5, 0.5, 0.22, 0.3));
    let (y0, y1) = (1.1, 1.16);
    let (a, b) = (0.42, 0.58); // the flue opening
    for (min, max) in [
        (Vec3::new(0.0, y0, 0.0), Vec3::new(a, y1, 1.0)),
        (Vec3::new(b, y0, 0.0), Vec3::new(1.0, y1, 1.0)),
        (Vec3::new(a, y0, 0.0), Vec3::new(b, y1, a)),
        (Vec3::new(a, y0, b), Vec3::new(b, y1, 1.0)),
    ] {
        s.solids.push(Shape::Cuboid(Aabb::new(min, max)));
    }
    s.vents.push(top_vent());
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_distances() {
        let b = Aabb::new(Vec3::ZERO, Vec3::ONE);
        assert!((b.distance(Vec3::splat(0.5)) + 0.5).abs() < 1e-6);
        assert!((b.distance(Vec3::new(2.0, 0.5, 0.5)) - 1.0).abs() < 1e-6);
        let s = Shape::Sphere { centre: Vec3::ZERO, radius: 1.0 };
        assert!((s.distance(Vec3::new(0.0, 3.0, 0.0)) - 2.0).abs() < 1e-6);
    }

    /// Burners and vents sit inside the domain and outside every solid.
    #[test]
    fn presets_are_well_formed() {
        for p in PRESETS {
            let s = (p.build)();
            let domain = s.domain();
            assert!(!s.burners.is_empty(), "{}: no burner", p.name);
            for b in &s.burners {
                assert!(domain.contains(b.pos), "{}: burner outside the domain", p.name);
                assert!(!s.is_solid(b.pos), "{}: burner inside a solid", p.name);
                assert!((b.dir.length() - 1.0).abs() < 1e-5, "{}: burner direction not unit", p.name);
            }
            for v in &s.vents {
                assert!(domain.contains(v.min) && domain.contains(v.max), "{}: vent outside the domain", p.name);
            }
        }
    }

    #[test]
    fn the_chimney_roof_has_a_flue() {
        let s = chimney();
        assert!(s.is_solid(Vec3::new(0.2, 1.13, 0.2)), "roof");
        assert!(!s.is_solid(Vec3::new(0.5, 1.13, 0.5)), "flue");
    }
}
