//! Mouse state and the tools it drives: push the fluid, paint or erase walls.

use crate::ui::{Tool, UiState};
use crate::view::View;
use sim::{ForceBrush, StepInput, Vec2, World};
use winit::event::{ElementState, MouseButton, WindowEvent};

/// Cap on the push velocity derived from mouse motion, so a flick can't blow things up.
const MAX_PUSH_SPEED: f32 = 6.0;

#[derive(Default)]
pub struct Input {
    /// Cursor position in physical pixels, if it is over the window.
    cursor: Option<Vec2>,
    /// Which button started the current stroke (only strokes begun outside egui count).
    stroke: Option<MouseButton>,
    /// Cursor world position at the previous frame of the stroke.
    prev_world: Option<Vec2>,
}

impl Input {
    /// `ui_owns_it` is true when egui consumed the event (the pointer is over the panel).
    pub fn handle(&mut self, event: &WindowEvent, ui_owns_it: bool) {
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = Some(Vec2::new(position.x as f32, position.y as f32));
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

    fn end_stroke(&mut self) {
        self.stroke = None;
        self.prev_world = None;
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
                });
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
