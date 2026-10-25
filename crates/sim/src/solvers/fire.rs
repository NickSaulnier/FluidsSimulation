//! Combustion for the smoke-and-fire mode of the stable-fluids solver: fuel and
//! temperature fields alongside the dye, which plays the smoke.
//!
//! Two kinds of fuel:
//! - Gas fuel, from burners, is carried by the flow.
//! - Solid fuel (a preset's fluid blocks: wood, wax) stays where it is and burns more
//!   slowly, so a block is a burning pile that the fire climbs through. As a gas, the
//!   block was mostly swept along by its own hot air before it could burn.
//!
//! Once solid fuel catches, it smoulders until it's used up, like embers, whatever the
//! passing gas does. Without that the airflow carried heat off faster than the fuel
//! released it, and a pile went out with most of its fuel left (measured: lit almost to
//! the top, then out at 2.5 s with 60% unburned).
//!
//! Per substep, after the flow has moved everything:
//! - Gas fuel burns at `burn_rate` where the gas is at least `ignition` hot. Solid
//!   fuel catches there and then burns at `SOLID_BURN` times that rate until it's gone.
//!   Either way the burned fuel releases `heat` (temperature) and `soot` (smoke).
//! - Hot gas cools as dT/dt = -cooling * TÂ²: like radiation, much faster when very
//!   hot. That keeps flames short (burning gas at T = 5 dims below visibility within a
//!   few tenths of a second) while merely warm gas stays above ignition long enough to
//!   light the fuel above it. A single exponential rate couldn't do both: one short
//!   enough for flames stalled the fire in a pile.
//! - A little heat diffuses to neighbouring cells, so a flame front can creep into fuel
//!   that isn't moving.
//!
//! Temperature is above ambient and dimensionless: ignition at 0.3, visible flame from
//! about 1, burning gas up to about 5. Its buoyancy is a Boussinesq term like the
//! dye's, but opposite gravity: hot gas rises, tilting gravity makes flames lean, and
//! without gravity they don't rise at all.
//!
//! Heat release has to beat the losses for a fire to spread: a burning cell gains heat
//! at `heat * SOLID_BURN * burn_rate` and loses `cooling * TÂ²` plus whatever the airflow
//! carries off. With heat 1.5 (and linear cooling) it settled barely above ignition and
//! fires stalled; at 6 the hot gas rising off burning fuel reliably lights the fuel
//! above.

use crate::grid::Grid2;
use crate::grid::mac::{CellType, MacGrid};
use crate::params::{FloatSpec, ParamVisitor};
use rayon::prelude::*;

#[derive(Clone, Debug)]
pub struct FireParams {
    /// Temperature at which fuel burns.
    pub ignition: f32,
    /// Fraction of the fuel at a burning point consumed per second (exponential).
    pub burn_rate: f32,
    /// Temperature released per unit of fuel burned.
    pub heat: f32,
    /// Radiative-style cooling: dT/dt = -cooling * TÂ².
    pub cooling: f32,
    /// Smoke released per unit of fuel burned.
    pub soot: f32,
    /// Upward acceleration per unit temperature, as a fraction of g.
    pub rise: f32,
    /// Heat diffusivity, mÂ²/s.
    pub diffusion: f32,
    /// Temperature of the gas leaving a burner (an emitter), so it's alight.
    pub burner: f32,
}

impl Default for FireParams {
    fn default() -> Self {
        Self { ignition: 0.3, burn_rate: 3.0, heat: 6.0, cooling: 2.5, soot: 0.3, rise: 0.12, diffusion: 3e-4, burner: 1.0 }
    }
}

const BURN_RATE: FloatSpec = FloatSpec {
    key: "burn_rate",
    label: "Burn rate (1/s)",
    range: 0.2..=20.0,
    log: true,
    restart: false,
    help: "How fast burning fuel is consumed. Faster: short, hot flames.",
};
const HEAT: FloatSpec = FloatSpec {
    key: "heat",
    label: "Heat per fuel",
    range: 0.5..=20.0,
    log: true,
    restart: false,
    help: "Temperature released by burning a unit of fuel.",
};
const COOLING: FloatSpec = FloatSpec {
    key: "cooling",
    label: "Cooling",
    range: 0.05..=10.0,
    log: true,
    restart: false,
    help: "How fast hot gas loses heat (the hotter, the faster, like radiation). Slower: tall flames.",
};
const SOOT: FloatSpec = FloatSpec {
    key: "soot",
    label: "Soot per fuel",
    range: 0.0..=2.0,
    log: false,
    restart: false,
    help: "Smoke released by burning a unit of fuel. 0 burns clean.",
};
const RISE: FloatSpec = FloatSpec {
    key: "rise",
    label: "Hot-gas lift",
    range: 0.0..=0.5,
    log: false,
    restart: false,
    help: "Upward acceleration of hot gas per unit temperature, as a fraction of gravity.",
};
const IGNITION: FloatSpec = FloatSpec {
    key: "ignition",
    label: "Ignition temperature",
    range: 0.05..=1.0,
    log: true,
    restart: false,
    help: "Fuel burns where the gas is at least this hot.",
};
const DIFFUSION: FloatSpec = FloatSpec {
    key: "heat_diffusion",
    label: "Heat diffusion (mÂ²/s)",
    range: 0.0..=0.003,
    log: false,
    restart: false,
    help: "How far heat spreads on its own, which lets a flame creep into still fuel.",
};
const BURNER: FloatSpec = FloatSpec {
    key: "burner",
    label: "Burner temperature",
    range: 0.1..=3.0,
    log: true,
    restart: false,
    help: "Temperature of the gas from burners (emitters and the faucet).",
};

/// Solid fuel burns this much slower than gas, so a pile lasts.
const SOLID_BURN: f32 = 0.3;
/// Smouldering fuel below this is spent.
const EMBERS_OUT: f32 = 0.01;
/// (See `flicker`.) Burner output varies by this fraction, changing every `FLICKER_HOLD` substeps.
const FLICKER: f32 = 0.4;
const FLICKER_HOLD: u64 = 3;

/// Temperatures drawn as the faintest and the fullest flame: warm gas below the first
/// is invisible, so a flame ends where its gas cools, not where the heat runs out.
const FLAME_FAINT: f32 = 1.0;
const FLAME_FULL: f32 = 3.0;

pub struct Combustion {
    pub params: FireParams,
    /// Gas fuel, carried by the flow.
    pub fuel: Grid2<f32>,
    /// Solid fuel, which stays put.
    pub solid_fuel: Grid2<f32>,
    /// Solid fuel that has caught and is smouldering.
    pub burning: Grid2<bool>,
    pub temperature: Grid2<f32>,
    scratch: [Grid2<f32>; 2],
}

impl Combustion {
    pub fn new(params: FireParams, nx: usize, ny: usize) -> Self {
        let g = || Grid2::new(nx, ny);
        Self { params, fuel: g(), solid_fuel: g(), burning: Grid2::new(nx, ny), temperature: g(), scratch: [g(), g()] }
    }

    pub fn resize(&mut self, nx: usize, ny: usize) {
        *self = Self::new(self.params.clone(), nx, ny);
    }

    /// All the fuel at a cell, gas and solid.
    pub fn total_fuel(&self, i: usize, j: usize) -> f32 {
        self.fuel[(i, j)] + self.solid_fuel[(i, j)]
    }

    /// How bright the flame is at a cell, 0 to 1.
    pub fn flame(&self, i: usize, j: usize) -> f32 {
        ((self.temperature[(i, j)] - FLAME_FAINT) / (FLAME_FULL - FLAME_FAINT)).clamp(0.0, 1.0)
    }

    /// How much fuel a burner releases at cell `i` on substep `step`, relative to its
    /// steady output: it varies by up to Â±`FLICKER` along the nozzle and over time,
    /// which seeds the buoyant instabilities that make flames flicker. A hash, not a
    /// random generator, so runs are reproducible.
    pub fn flicker(i: usize, step: u64) -> f32 {
        let mut x = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (step / FLICKER_HOLD).wrapping_mul(0xD1B5_4A32_D192_ED03);
        x ^= x >> 29;
        x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x ^= x >> 32;
        let unit = (x >> 40) as f32 / (1u64 << 24) as f32; // 0..1
        1.0 + FLICKER * (2.0 * unit - 1.0)
    }

    pub fn visit(&mut self, v: &mut dyn ParamVisitor) {
        let p = &mut self.params;
        v.section("Fire");
        v.float(&BURN_RATE, &mut p.burn_rate);
        v.float(&HEAT, &mut p.heat);
        v.float(&COOLING, &mut p.cooling);
        v.float(&RISE, &mut p.rise);
        v.float(&SOOT, &mut p.soot);
    }

    pub fn visit_advanced(&mut self, v: &mut dyn ParamVisitor) {
        let p = &mut self.params;
        v.advanced_section("Combustion");
        v.float(&IGNITION, &mut p.ignition);
        v.float(&BURNER, &mut p.burner);
        v.float(&DIFFUSION, &mut p.diffusion);
    }

    /// Burn, release heat and soot into `smoke`, cool, and diffuse heat, over `dt`.
    /// Solid cells (walls, and burner nozzles, which hold their own values) are left
    /// alone; heat doesn't diffuse into walls.
    pub fn react(&mut self, grid: &MacGrid, smoke: &mut Grid2<f32>, dt: f32) {
        let p = &self.params;
        let gas_fraction = 1.0 - (-p.burn_rate * dt).exp();
        let solid_fraction = 1.0 - (-SOLID_BURN * p.burn_rate * dt).exp();
        let cell = &grid.cell;
        self.fuel
            .par_rows_mut()
            .zip(self.solid_fuel.par_rows_mut())
            .zip(self.burning.par_rows_mut())
            .zip(self.temperature.par_rows_mut())
            .zip(smoke.par_rows_mut())
            .enumerate()
            .for_each(|(j, ((((fuel, solid), burning), temp), smoke))| {
                for i in 0..fuel.len() {
                    if cell[(i, j)] == CellType::Solid {
                        continue;
                    }
                    let hot = temp[i] >= p.ignition;
                    burning[i] |= hot && solid[i] > 0.0;
                    let mut burned = 0.0;
                    if hot {
                        burned += fuel[i] * gas_fraction;
                        fuel[i] *= 1.0 - gas_fraction;
                    }
                    if burning[i] {
                        burned += solid[i] * solid_fraction;
                        solid[i] *= 1.0 - solid_fraction;
                        if solid[i] < EMBERS_OUT {
                            (solid[i], burning[i]) = (0.0, false);
                        }
                    }
                    temp[i] += p.heat * burned;
                    smoke[i] = (smoke[i] + p.soot * burned).min(1.0);
                    // dT/dt = -cooling TÂ², integrated implicitly: stable at any dt.
                    temp[i] /= 1.0 + p.cooling * temp[i].max(0.0) * dt;
                }
            });

        // Explicit diffusion, capped for stability (it's a small effect anyway).
        let k = (p.diffusion * dt / (grid.h * grid.h)).min(0.2);
        if k > 0.0 {
            let (nx, ny) = (grid.nx, grid.ny);
            let t = &self.temperature;
            let open = |i: usize, j: usize| cell[(i, j)] != CellType::Solid;
            self.scratch[0].par_rows_mut().enumerate().for_each(|(j, row)| {
                for (i, out) in row.iter_mut().enumerate() {
                    *out = t[(i, j)];
                    if !open(i, j) {
                        continue;
                    }
                    let mut flux = 0.0;
                    for (a, b) in [(i.wrapping_sub(1), j), (i + 1, j), (i, j.wrapping_sub(1)), (i, j + 1)] {
                        if a < nx && b < ny && open(a, b) {
                            flux += t[(a, b)] - t[(i, j)];
                        }
                    }
                    *out += k * flux;
                }
            });
            std::mem::swap(&mut self.temperature, &mut self.scratch[0]);
        }
    }

    /// Carry fuel and temperature with the flow (see `super::maccormack`).
    pub fn advect(
        &mut self,
        h: f32,
        dt: f32,
        velocity: impl Fn(glam::Vec2) -> glam::Vec2 + Sync + Copy,
        frozen: impl Fn(usize, usize) -> bool + Sync + Copy,
    ) {
        let [forward, back] = &mut self.scratch;
        super::maccormack(&mut self.fuel, forward, back, h, dt, velocity, frozen);
        super::maccormack(&mut self.temperature, forward, back, h, dt, velocity, frozen);
    }
}
