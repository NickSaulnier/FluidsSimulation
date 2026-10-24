//! Renders solver parameters (from `Solver::visit_params`) as egui widgets.

pub struct EguiParams<'u> {
    pub ui: &'u mut egui::Ui,
    pub restart_requested: bool,
}

impl EguiParams<'_> {
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
        let r = self
            .ui
            .add(egui::Slider::new(v, s.range.clone()).text(s.label).logarithmic(s.log))
            .on_hover_text(s.help);
        self.note_change(&r, s.restart);
    }

    fn int(&mut self, s: &sim::IntSpec, v: &mut u32) {
        let r = self.ui.add(egui::Slider::new(v, s.range.clone()).text(s.label)).on_hover_text(s.help);
        self.note_change(&r, s.restart);
    }

    fn toggle(&mut self, s: &sim::ToggleSpec, v: &mut bool) {
        self.ui.checkbox(v, s.label).on_hover_text(s.help);
    }

    fn section(&mut self, title: &'static str) {
        self.ui.add_space(4.0);
        self.ui.strong(title);
    }
}
