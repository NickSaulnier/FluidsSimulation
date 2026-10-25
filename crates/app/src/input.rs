//! Mouse state and the tools it drives: push the fluid, paint or erase walls. In 3D the
//! left button orbits the camera and the wheel zooms, and the right button pushes the
//! gas or holds a torch.

use crate::render3d::OrbitCamera;
use crate::ui::{Tool, UiState};
use crate::view::View;
use glam::Vec3;
use sim::{Emitter, ForceBrush, StepInput, Vec2, World};
use sim3d::{Brush3d, Burner, StepInput3d};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};

/// Cap on the push velocity derived from mouse motion, so a flick can't blow things up.
const MAX_PUSH_SPEED: f32 = 6.0;
/// Touchpads scroll in pixels; this many make one wheel step.
const PIXELS_PER_WHEEL_STEP: f32 = 40.0;

#[derive(Default)]
pub struct Input {
    /// Cursor position in physical pixels, if it is over the window.
    cursor: Option<Vec2>,
    /// Which button started the current stroke (only strokes begun outside egui count).
    stroke: Option<MouseButton>,
    /// Cursor world position at the previous frame of the stroke.
    prev_world: Option<Vec2>,
    /// The same in 3D: the cursor's point in the volume.
    prev_volume: Option<Vec3>,
    /// Cursor motion (pixels) during the stroke, and wheel steps, since last taken.
    drag: Vec2,
    wheel: f32,
}

impl Input {
    /// `ui_owns_it` is true when egui consumed the event (the pointer is over the panel).
    pub fn handle(&mut self, event: &WindowEvent, ui_owns_it: bool) {
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                let here = Vec2::new(position.x as f32, position.y as f32);
                if let (Some(_), Some(before)) = (self.stroke, self.cursor) {
                    self.drag += here - before;
                }
                self.cursor = Some(here);
            }
            WindowEvent::MouseWheel { delta, .. } if !ui_owns_it => {
                self.wheel += match delta {
                    MouseScrollDelta::LineDelta(_, y) => *y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / PIXELS_PER_WHEEL_STEP,
                };
            }
            WindowEvent::CursorLeft { .. } => {
                self.cursor = None;
                self.end_stroke();
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button, .. }
                if !ui_owns_it && self.stroke.is_none() && matches!(button, MouseButton::Left | MouseButton::Right) =>
            {
                self.stroke = Some(*button);
            }
            WindowEvent::MouseInput { state: ElementState::Released, button, .. } if self.stroke == Some(*button) => {
                self.end_stroke();
            }
            _ => {}
        }
    }

    pub fn cursor(&self) -> Option<Vec2> {
        self.cursor
    }

    /// The 3D camera's share of the mouse since last frame: the left-drag motion (if a
    /// left drag is under way) and wheel steps. Call every frame, so 2D strokes don't
    /// pile up motion.
    pub fn take_orbit(&mut self) -> (Option<Vec2>, f32) {
        let drag = std::mem::take(&mut self.drag);
        (self.stroke.filter(|b| *b == MouseButton::Left).map(|_| drag), std::mem::take(&mut self.wheel))
    }

    fn end_stroke(&mut self) {
        self.stroke = None;
        self.prev_world = None;
        self.prev_volume = None;
    }

    /// Apply this frame's 3D stroke: the right button acts where the cursor meets the
    /// plane through the volume's centre facing the camera (the left button orbits).
    /// The faucet tool holds a burner there, a torch pointing along the drag (up while
    /// still); every other tool pushes. `viewport` is the canvas in physical pixels.
    pub fn apply_3d(&mut self, camera: &OrbitCamera, viewport: [f32; 4], size: Vec3, ui: &UiState, real_dt: f32) -> StepInput3d {
        let mut input = StepInput3d::default();
        let [x, y, w, h] = viewport;
        let (Some(MouseButton::Right), Some(cursor)) = (self.stroke, self.cursor) else {
            return input;
        };
        if w < 1.0 || h < 1.0 {
            return input;
        }
        let ndc = Vec2::new((cursor.x - x) / w * 2.0 - 1.0, 1.0 - (cursor.y - y) / h * 2.0);
        let here = camera.point_on_target_plane(ndc, w / h).clamp(Vec3::ZERO, size);
        let motion = here - self.prev_volume.unwrap_or(here);
        self.prev_volume = Some(here);
        if ui.tool == Tool::Faucet {
            let dir = if motion.length() > 1e-3 { motion.normalize() } else { Vec3::Y };
            input.burners.push(Burner { pos: here, dir, radius: 0.5 * ui.brush_radius, speed: ui.faucet_speed });
        } else {
            let velocity = (motion / real_dt.max(1e-4)).clamp_length_max(MAX_PUSH_SPEED);
            input.brushes.push(Brush3d { pos: here, radius: ui.brush_radius, velocity, spin: Vec3::ZERO, strength: ui.push_strength });
        }
        input
    }

    /// Apply this frame's stroke. Wall edits go straight into the scene; a push becomes
    /// a brush in the returned `StepInput`. Left button uses the selected tool, right
    /// button always pushes.
    pub fn apply(&mut self, view: &View, ui: &UiState, world: &mut World, real_dt: f32) -> StepInput {
        let mut input = StepInput::default();
        let (Some(button), Some(cursor)) = (self.stroke, self.cursor) else {
            return input;
        };
        let here = view.screen_to_world(cursor);
        let prev = self.prev_world.unwrap_or(here);
        self.prev_world = Some(here);

        let tool = if button == MouseButton::Right { Tool::Push } else { ui.tool };
        match tool {
            Tool::Push => {
                let velocity = ((here - prev) / real_dt.max(1e-4)).clamp_length_max(MAX_PUSH_SPEED);
                input.brushes.push(ForceBrush {
                    pos: here,
                    radius: ui.brush_radius,
                    velocity,
                    strength: ui.push_strength,
                    spin: 0.0,
                });
            }
            Tool::Faucet => {
                // Pour along the drag direction; straight down while holding still.
                let motion = here - prev;
                let dir = if motion.length() > 1e-3 { motion.normalize() } else { Vec2::NEG_Y };
                input.emitters.push(Emitter { pos: here, dir, speed: ui.faucet_speed, width: ui.brush_radius });
            }
            Tool::PaintWall | Tool::EraseWall => {
                // Stamp along the segment since last frame, so fast strokes leave no gaps.
                let solid = tool == Tool::PaintWall;
                let steps = ((here - prev).length() / (0.25 * ui.brush_radius)).ceil().max(1.0) as usize;
                for k in 1..=steps {
                    let p = prev.lerp(here, k as f32 / steps as f32);
                    world.scene.solid.paint_disc(p, ui.brush_radius, solid);
                }
            }
        }
        input
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::dpi::PhysicalPosition;
    use winit::event::DeviceId;

    fn moved(x: f64, y: f64) -> WindowEvent {
        WindowEvent::CursorMoved { device_id: DeviceId::dummy(), position: PhysicalPosition::new(x, y) }
    }

    fn button(state: ElementState, button: MouseButton) -> WindowEvent {
        WindowEvent::MouseInput { device_id: DeviceId::dummy(), state, button }
    }

    #[test]
    fn a_left_drag_orbits_and_other_motion_does_not() {
        let mut input = Input::default();
        input.handle(&moved(100.0, 100.0), false);
        input.handle(&moved(130.0, 90.0), false);
        assert_eq!(input.take_orbit(), (None, 0.0), "hovering is not a drag");

        input.handle(&button(ElementState::Pressed, MouseButton::Left), false);
        input.handle(&moved(150.0, 95.0), false);
        input.handle(&moved(170.0, 120.0), false);
        assert_eq!(input.take_orbit(), (Some(Vec2::new(40.0, 30.0)), 0.0));
        assert_eq!(input.take_orbit(), (Some(Vec2::ZERO), 0.0), "motion is taken once");
        input.handle(&button(ElementState::Released, MouseButton::Left), false);

        // A right drag is a push, not an orbit; a press on the panel starts nothing.
        input.handle(&button(ElementState::Pressed, MouseButton::Right), false);
        input.handle(&moved(200.0, 120.0), false);
        assert_eq!(input.take_orbit().0, None);
        input.handle(&button(ElementState::Released, MouseButton::Right), false);
        input.handle(&button(ElementState::Pressed, MouseButton::Left), true);
        input.handle(&moved(260.0, 120.0), false);
        assert_eq!(input.take_orbit().0, None);
    }

    /// A right drag in 3D pushes along the drag (on screen: rightward is the camera's
    /// right); with the faucet it holds a burner instead. Left drags don't touch the gas.
    #[test]
    fn a_right_drag_in_3d_pushes_or_lights_a_torch() {
        let size = Vec3::new(1.0, 1.5, 1.0);
        let camera = OrbitCamera::framing(size);
        let viewport = [0.0, 0.0, 800.0, 600.0];
        let mut ui = UiState::default();
        let mut input = Input::default();
        input.handle(&moved(400.0, 300.0), false);
        input.handle(&button(ElementState::Pressed, MouseButton::Left), false);
        assert!(input.apply_3d(&camera, viewport, size, &ui, 0.016).brushes.is_empty(), "a left drag pushed");
        input.handle(&button(ElementState::Released, MouseButton::Left), false);

        input.handle(&button(ElementState::Pressed, MouseButton::Right), false);
        let first = input.apply_3d(&camera, viewport, size, &ui, 0.016);
        assert!((first.brushes[0].pos - camera.target).length() < 1e-3, "the screen centre is the target");
        input.handle(&moved(440.0, 300.0), false);
        let push = input.apply_3d(&camera, viewport, size, &ui, 0.016).brushes[0];
        assert!(push.velocity.dot(camera.right()) > 0.5, "pushed {} for a rightward drag", push.velocity);

        ui.tool = Tool::Faucet;
        let torch = input.apply_3d(&camera, viewport, size, &ui, 0.016);
        assert!(torch.brushes.is_empty());
        assert_eq!(torch.burners.len(), 1);
        assert!((torch.burners[0].dir - Vec3::Y).length() < 1e-6, "a still torch points up");
    }

    #[test]
    fn the_wheel_counts_steps_unless_over_the_panel() {
        let mut input = Input::default();
        let wheel = |delta| WindowEvent::MouseWheel { device_id: DeviceId::dummy(), delta, phase: winit::event::TouchPhase::Moved };
        input.handle(&wheel(MouseScrollDelta::LineDelta(0.0, 2.0)), false);
        input.handle(&wheel(MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, -PIXELS_PER_WHEEL_STEP as f64))), false);
        input.handle(&wheel(MouseScrollDelta::LineDelta(0.0, 5.0)), true);
        assert_eq!(input.take_orbit(), (None, 1.0));
        assert_eq!(input.take_orbit(), (None, 0.0));
    }
}
