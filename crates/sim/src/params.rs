//! Solver parameters exposed through a visitor.
//!
//! Each solver keeps its parameters in a plain typed struct and walks it, handing out
//! `&mut` references together with a static description. The UI, a CLI or a preset
//! loader are just different visitors, so nothing outside the solver needs to know which
//! parameters exist, and the solver never looks anything up by name in hot code.

use std::ops::RangeInclusive;

pub struct FloatSpec {
    /// Stable identifier for CLIs and preset files, e.g. `"flip_ratio"`.
    pub key: &'static str,
    pub label: &'static str,
    pub range: RangeInclusive<f32>,
    pub log: bool,
    /// Changing it only takes effect after `Solver::reset` (e.g. grid resolution).
    pub restart: bool,
    pub help: &'static str,
}

pub struct IntSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub range: RangeInclusive<u32>,
    pub restart: bool,
    pub help: &'static str,
}

pub struct ToggleSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub help: &'static str,
}

pub trait ParamVisitor {
    fn float(&mut self, spec: &FloatSpec, value: &mut f32);
    fn int(&mut self, spec: &IntSpec, value: &mut u32);
    fn toggle(&mut self, spec: &ToggleSpec, value: &mut bool);
    /// Optional grouping hint.
    fn section(&mut self, _title: &'static str) {}
}
