//! Metric health: solvers report metrics with tolerances (the invariant checks), shown
//! as a green, amber or red dot so a problem stands out without reading numbers.

use super::theme;
use egui::{Color32, Sense, vec2};
use sim::Metric;

/// Above this fraction of its tolerance a metric is flagged amber.
const NEAR: f32 = 0.8;
const OK_COLOUR: Color32 = Color32::from_rgb(90, 200, 120);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Health {
    Ok,
    Near,
    Over,
}

impl Health {
    /// `None` for metrics without a tolerance (plain readouts).
    pub fn of(m: &Metric) -> Option<Self> {
        let tolerance = m.tolerance?;
        Some(if m.value > tolerance {
            Self::Over
        } else if m.value > NEAR * tolerance {
            Self::Near
        } else {
            Self::Ok
        })
    }

    pub fn colour(self) -> Color32 {
        match self {
            Self::Ok => OK_COLOUR,
            Self::Near => theme::WARN,
            Self::Over => theme::ERROR,
        }
    }
}

/// The least healthy checked metric (the first, among equals).
pub fn worst(metrics: &[Metric]) -> Option<(Health, &Metric)> {
    metrics.iter().filter_map(|m| Health::of(m).map(|h| (h, m))).fold(None, |worst, (h, m)| match worst {
        Some((w, _)) if w >= h => worst,
        _ => Some((h, m)),
    })
}

/// A small filled circle in the health's colour.
pub fn dot(ui: &mut egui::Ui, health: Health) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, health.colour());
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(name: &'static str, value: f32, tolerance: Option<f32>) -> Metric {
        Metric { name, value, tolerance }
    }

    #[test]
    fn classifies_against_the_tolerance() {
        assert_eq!(Health::of(&m("a", 0.1, Some(1.0))), Some(Health::Ok));
        assert_eq!(Health::of(&m("a", 0.85, Some(1.0))), Some(Health::Near));
        assert_eq!(Health::of(&m("a", 1.2, Some(1.0))), Some(Health::Over));
        assert_eq!(Health::of(&m("a", 99.0, None)), None);
    }

    #[test]
    fn worst_picks_the_least_healthy_checked_metric() {
        let metrics = [m("fine", 0.1, Some(1.0)), m("readout", 50.0, None), m("close", 0.9, Some(1.0)), m("also close", 0.95, Some(1.0))];
        let (h, metric) = worst(&metrics).unwrap();
        assert_eq!((h, metric.name), (Health::Near, "close"));
        assert!(worst(&[m("readout", 1.0, None)]).is_none());
    }
}
