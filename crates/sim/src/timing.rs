//! Lap timer for per-phase step breakdowns (`Stats::phases`).

use std::time::{Duration, Instant};

pub struct PhaseTimer {
    last: Instant,
    phases: Vec<(&'static str, Duration)>,
}

impl PhaseTimer {
    pub fn start() -> Self {
        Self { last: Instant::now(), phases: Vec::new() }
    }

    /// Record the time since the previous lap (or start) under `name`.
    pub fn lap(&mut self, name: &'static str) {
        let now = Instant::now();
        self.phases.push((name, now - self.last));
        self.last = now;
    }

    pub fn finish(self) -> Vec<(&'static str, Duration)> {
        self.phases
    }
}
