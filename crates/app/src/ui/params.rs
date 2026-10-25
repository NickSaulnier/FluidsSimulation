//! Solver parameters by key (for music reactions), and renders them (from
//! `Solver::visit_params`) as form rows, one rounded
//! group per section. The visitor streams parameters one at a time, so each group is
//! opened with `Frame::begin` at its section and closed at the next one (or `finish`).
//! Advanced sections start collapsed: their header toggles them, and while collapsed
//! their parameters are skipped.

use super::theme;
use super::widgets::{self, param_text};
use egui::RichText;
use egui_phosphor::regular as icon;

/// The current solver's float parameters that can change live (no restart):
/// (key, label).
pub fn live_float_params(world: &mut sim::World) -> Vec<(&'static str, &'static str)> {
    struct List(Vec<(&'static str, &'static str)>);
    impl sim::ParamVisitor for List {
        fn float(&mut self, s: &sim::FloatSpec, _: &mut f32) {
            if !s.restart {
                self.0.push((s.key, s.label));
            }
        }
        fn int(&mut self, _: &sim::IntSpec, _: &mut u32) {}
        fn toggle(&mut self, _: &sim::ToggleSpec, _: &mut bool) {}
    }
    let mut list = List(Vec::new());
    world.solver.visit_params(&mut list);
    list.0
}

/// Read a float parameter by key, or set it (clamped to its range) if `set` is given.
/// Returns the value before any change.
pub fn float_param(world: &mut sim::World, key: &str, set: Option<f32>) -> Option<f32> {
    struct Access<'k> {
        key: &'k str,
        set: Option<f32>,
        found: Option<f32>,
    }
    impl sim::ParamVisitor for Access<'_> {
        fn float(&mut self, s: &sim::FloatSpec, v: &mut f32) {
            if s.key == self.key {
                self.found = Some(*v);
                if let Some(new) = self.set {
                    *v = new.clamp(*s.range.start(), *s.range.end());
                }
            }
        }
        fn int(&mut self, _: &sim::IntSpec, _: &mut u32) {}
        fn toggle(&mut self, _: &sim::ToggleSpec, _: &mut bool) {}
    }
    let mut access = Access { key, set, found: None };
    world.solver.visit_params(&mut access);
    access.found
}

/// Read an int parameter by key, or set it (clamped to its range) if `set` is given.
/// Returns the value before any change.
pub fn int_param(world: &mut sim::World, key: &str, set: Option<u32>) -> Option<u32> {
    int_access(world, key, set).map(|(v, _)| v)
}

/// An int parameter's value and the bottom of its range.
pub fn int_param_range(world: &mut sim::World, key: &str) -> Option<(u32, u32)> {
    int_access(world, key, None)
}

/// A float parameter's value and the top of its range.
pub fn float_param_range(world: &mut sim::World, key: &str) -> Option<(f32, f32)> {
    struct Range<'k>(&'k str, Option<(f32, f32)>);
    impl sim::ParamVisitor for Range<'_> {
        fn float(&mut self, s: &sim::FloatSpec, v: &mut f32) {
            if s.key == self.0 {
                self.1 = Some((*v, *s.range.end()));
            }
        }
        fn int(&mut self, _: &sim::IntSpec, _: &mut u32) {}
        fn toggle(&mut self, _: &sim::ToggleSpec, _: &mut bool) {}
    }
    let mut range = Range(key, None);
    world.solver.visit_params(&mut range);
    range.1
}

fn int_access(world: &mut sim::World, key: &str, set: Option<u32>) -> Option<(u32, u32)> {
    struct Access<'k> {
        key: &'k str,
        set: Option<u32>,
        found: Option<(u32, u32)>,
    }
    impl sim::ParamVisitor for Access<'_> {
        fn float(&mut self, _: &sim::FloatSpec, _: &mut f32) {}
        fn int(&mut self, s: &sim::IntSpec, v: &mut u32) {
            if s.key == self.key {
                self.found = Some((*v, *s.range.start()));
                if let Some(new) = self.set {
                    *v = new.clamp(*s.range.start(), *s.range.end());
                }
            }
        }
        fn toggle(&mut self, _: &sim::ToggleSpec, _: &mut bool) {}
    }
    let mut access = Access { key, set, found: None };
    world.solver.visit_params(&mut access);
    access.found
}

pub struct EguiParams<'u> {
    ui: &'u mut egui::Ui,
    group: Option<egui::frame::Prepared>,
    /// Inside a collapsed advanced section: don't draw parameters.
    collapsed: bool,
    restart_requested: bool,
}

impl<'u> EguiParams<'u> {
    pub fn new(ui: &'u mut egui::Ui) -> Self {
        Self { ui, group: None, collapsed: false, restart_requested: false }
    }

    /// Close the last group. Returns whether a restart-on-change parameter changed.
    pub fn finish(mut self) -> bool {
        self.close_group();
        self.restart_requested
    }

    fn close_group(&mut self) {
        if let Some(group) = self.group.take() {
            group.end(self.ui);
            self.ui.add_space(4.0);
        }
        self.collapsed = false;
    }

    fn open_group(&mut self) -> &mut egui::Ui {
        self.close_group();
        let group = self.group.insert(widgets::group_frame().begin(self.ui));
        let ui = &mut group.content_ui;
        ui.set_min_width(ui.available_width());
        ui
    }

    /// Where rows go: the open group, or the panel itself before any section.
    fn target(&mut self) -> &mut egui::Ui {
        match &mut self.group {
            Some(group) => &mut group.content_ui,
            None => self.ui,
        }
    }

    /// Parameters that need a restart apply when the drag ends (or on a click / typed
    /// value), not on every intermediate slider position.
    fn note_change(&mut self, r: &egui::Response, restart: bool) {
        if restart && (r.drag_stopped() || (r.changed() && !r.dragged())) {
            self.restart_requested = true;
        }
    }
}

impl sim::ParamVisitor for EguiParams<'_> {
    fn float(&mut self, s: &sim::FloatSpec, v: &mut f32) {
        if self.collapsed {
            return;
        }
        let (label, help) = param_text(s.label, s.help, s.restart);
        let r = widgets::row(self.target(), &label, &help, |ui| {
            ui.add(egui::Slider::new(v, s.range.clone()).logarithmic(s.log)).on_hover_text(&help)
        });
        self.note_change(&r, s.restart);
    }

    fn int(&mut self, s: &sim::IntSpec, v: &mut u32) {
        if self.collapsed {
            return;
        }
        let (label, help) = param_text(s.label, s.help, s.restart);
        let r = widgets::row(self.target(), &label, &help, |ui| {
            ui.add(egui::Slider::new(v, s.range.clone())).on_hover_text(&help)
        });
        self.note_change(&r, s.restart);
    }

    fn toggle(&mut self, s: &sim::ToggleSpec, v: &mut bool) {
        if self.collapsed {
            return;
        }
        widgets::toggle(self.target(), v, s.label, s.help);
    }

    fn section(&mut self, title: &'static str) {
        widgets::header(self.open_group(), title);
    }

    fn advanced_section(&mut self, title: &'static str) {
        let ui = self.open_group();
        let id = ui.make_persistent_id(("advanced", title));
        let mut open = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
        let caret = if open { icon::CARET_DOWN } else { icon::CARET_RIGHT };
        let header = RichText::new(format!("{caret} {title}")).strong().size(14.0).color(theme::HEADER);
        let clicked = ui
            .horizontal(|ui| {
                let clicked = ui.add(egui::Button::new(header).frame(false)).clicked();
                ui.weak("advanced");
                clicked
            })
            .inner;
        if clicked {
            open = !open;
            ui.data_mut(|d| d.insert_temp(id, open));
        }
        self.collapsed = !open;
    }
}
