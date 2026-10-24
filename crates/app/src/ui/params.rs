//! Renders solver parameters (from `Solver::visit_params`) as form rows, one rounded
//! group per section. The visitor streams parameters one at a time, so each group is
//! opened with `Frame::begin` at its section and closed at the next one (or `finish`).

use super::widgets::{self, param_text};

pub struct EguiParams<'u> {
    ui: &'u mut egui::Ui,
    group: Option<egui::frame::Prepared>,
    restart_requested: bool,
}

impl<'u> EguiParams<'u> {
    pub fn new(ui: &'u mut egui::Ui) -> Self {
        Self { ui, group: None, restart_requested: false }
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
        let (label, help) = param_text(s.label, s.help, s.restart);
        let r = widgets::row(self.target(), &label, &help, |ui| {
            ui.add(egui::Slider::new(v, s.range.clone()).logarithmic(s.log)).on_hover_text(&help)
        });
        self.note_change(&r, s.restart);
    }

    fn int(&mut self, s: &sim::IntSpec, v: &mut u32) {
        let (label, help) = param_text(s.label, s.help, s.restart);
        let r = widgets::row(self.target(), &label, &help, |ui| {
            ui.add(egui::Slider::new(v, s.range.clone())).on_hover_text(&help)
        });
        self.note_change(&r, s.restart);
    }

    fn toggle(&mut self, s: &sim::ToggleSpec, v: &mut bool) {
        widgets::toggle(self.target(), v, s.label, s.help);
    }

    fn section(&mut self, title: &'static str) {
        self.close_group();
        let mut group = widgets::group_frame().begin(self.ui);
        let ui = &mut group.content_ui;
        ui.set_min_width(ui.available_width());
        widgets::header(ui, title);
        self.group = Some(group);
    }
}
