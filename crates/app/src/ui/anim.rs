//! Interface animation: quick and purposeful. Most moves take `QUICK` with an ease-out
//! curve (fast, then settling). The app redraws every frame, so animating costs no
//! extra repaints.
//!
//! - `toward`: a 0..1 transition that follows a bool (a panel open, a switch on).
//! - `value`: a number easing to a target (an underline's position, a dial's angle).
//! - `kick` and `progress`: a one-off move after an event (a heart that pops, stars
//!   filling one after another), 0..1 over its time.
//!
//! With reduced motion (a setting, or Windows asking for less animation) every change
//! is instant and kicks don't play.

use egui::{Context, Id};

/// How long most moves take, seconds; and the slower ones (a panel, a title card).
pub const QUICK: f32 = 0.18;
pub const SLOW: f32 = 0.3;
/// egui's own animation time (folding, fading windows), restored after reduced motion.
const EGUI_TIME: f32 = 0.2;

/// Ease out (cubic): starts fast, settles gently.
pub fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// A bounce that overshoots and settles (for pops and snaps): 0 to 1, peaking above 1.
pub fn overshoot(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let c = 1.7;
    1.0 + (c + 1.0) * (t - 1.0).powi(3) + c * (t - 1.0).powi(2)
}

fn reduce_id() -> Id {
    Id::new("reduce motion")
}

/// Turn reduced motion on or off (the app sets it every frame from the settings). It
/// also stops egui's own animations (folding, fading windows).
pub fn set_reduced(ctx: &Context, on: bool) {
    if reduced(ctx) != on {
        ctx.data_mut(|d| d.insert_temp(reduce_id(), on));
        ctx.all_styles_mut(|s| s.animation_time = if on { 0.0 } else { EGUI_TIME });
    }
}

pub fn reduced(ctx: &Context) -> bool {
    ctx.data(|d| d.get_temp(reduce_id())).unwrap_or(false)
}

/// 0..1 following `on` over `time` seconds, eased out.
pub fn toward(ctx: &Context, id: Id, on: bool, time: f32) -> f32 {
    if reduced(ctx) {
        return f32::from(u8::from(on));
    }
    ctx.animate_bool_with_time_and_easing(id, on, time, ease_out)
}

/// A number moving to `target` over `time` seconds.
pub fn value(ctx: &Context, id: Id, target: f32, time: f32) -> f32 {
    if reduced(ctx) {
        return ctx.animate_value_with_time(id, target, 0.0);
    }
    ctx.animate_value_with_time(id, target, time)
}

/// Mark that the move `id` starts now.
pub fn kick(ctx: &Context, id: Id) {
    let now = ctx.input(|i| i.time);
    ctx.data_mut(|d| d.insert_temp(id.with("kick"), now));
}

/// How far the move `id` has gone, 0..1 over `time` seconds since its kick, or none if
/// it isn't playing (never kicked, finished, or motion reduced).
pub fn progress(ctx: &Context, id: Id, time: f32) -> Option<f32> {
    if reduced(ctx) {
        return None;
    }
    let started: f64 = ctx.data(|d| d.get_temp(id.with("kick")))?;
    let t = ((ctx.input(|i| i.time) - started) as f32 / time.max(1e-3)).max(0.0);
    (t < 1.0).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easing_runs_from_0_to_1() {
        assert_eq!((ease_out(0.0), ease_out(1.0)), (0.0, 1.0));
        assert!(ease_out(0.5) > 0.5, "it starts fast");
        assert!((overshoot(0.0)).abs() < 1e-6 && (overshoot(1.0) - 1.0).abs() < 1e-6);
        assert!((0..100).map(|k| overshoot(k as f32 / 100.0)).fold(0.0, f32::max) > 1.05, "it overshoots");
    }

    /// Without reduced motion a transition takes time; with it, changes are instant and
    /// kicks don't play.
    #[test]
    fn reduced_motion_is_instant() {
        let ctx = Context::default();
        let id = Id::new("t");
        let frame = |f: &dyn Fn(&Context)| {
            // (The app uploads the font texture egui makes; here it's let go.)
            ctx.run_ui(egui::RawInput::default(), |ui| f(ui.ctx())).textures_delta.clear();
        };
        frame(&|ctx| {
            toward(ctx, id, false, QUICK);
        });
        frame(&|ctx| {
            let t = toward(ctx, id, true, QUICK);
            assert!(t < 1.0, "jumped: {t}");
        });
        frame(&|ctx| {
            set_reduced(ctx, true);
            assert_eq!(toward(ctx, id.with(2), true, QUICK), 1.0);
            kick(ctx, id);
            assert_eq!(progress(ctx, id, QUICK), None);
        });
        frame(&|ctx| {
            set_reduced(ctx, false);
            kick(ctx, id);
            assert_eq!(progress(ctx, id, QUICK), Some(0.0));
        });
    }
}
