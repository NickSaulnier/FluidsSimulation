//! Every keyboard shortcut, in one table. The key handler, the tooltips, the cheat sheet
//! (F1) and the Info tab all read it, so they can't drift apart.

use super::Tool;
use winit::keyboard::KeyCode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    TogglePause,
    Step,
    Reset,
    NextPreset,
    NextSolver,
    SelectTool(Tool),
    BrushSmaller,
    BrushBigger,
    TiltLeft,
    TiltRight,
    NextOverlay,
    ToggleParticles,
    ToggleLiquid,
    ToggleMusic,
    TogglePanel,
    ToggleCheatSheet,
    TogglePresentation,
    PreviousLook,
    NextLook,
    ToggleAutoLooks,
}

/// Cheat-sheet columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Simulation,
    Tools,
    View,
}

pub struct Shortcut {
    pub key: KeyCode,
    /// How the key is written in hints.
    pub keys: &'static str,
    pub action: Action,
    pub what: &'static str,
    pub group: Group,
    /// Whether holding the key repeats the action.
    pub repeats: bool,
}

const fn s(key: KeyCode, keys: &'static str, action: Action, what: &'static str, group: Group) -> Shortcut {
    Shortcut { key, keys, action, what, group, repeats: false }
}

const fn held(key: KeyCode, keys: &'static str, action: Action, what: &'static str, group: Group) -> Shortcut {
    Shortcut { key, keys, action, what, group, repeats: true }
}

use Action as A;
use Group as G;
pub const SHORTCUTS: &[Shortcut] = &[
    s(KeyCode::Space, "Space", A::TogglePause, "pause / play", G::Simulation),
    s(KeyCode::KeyS, "S", A::Step, "step (while paused)", G::Simulation),
    s(KeyCode::KeyR, "R", A::Reset, "reset the scene", G::Simulation),
    s(KeyCode::KeyP, "P", A::NextPreset, "next scene", G::Simulation),
    s(KeyCode::KeyN, "N", A::NextSolver, "next solver", G::Simulation),
    held(KeyCode::ArrowLeft, "Left", A::TiltLeft, "tilt gravity left", G::Simulation),
    held(KeyCode::ArrowRight, "Right", A::TiltRight, "tilt gravity right", G::Simulation),
    s(KeyCode::Digit1, "1", A::SelectTool(Tool::Push), "push tool", G::Tools),
    s(KeyCode::Digit2, "2", A::SelectTool(Tool::PaintWall), "wall tool", G::Tools),
    s(KeyCode::Digit3, "3", A::SelectTool(Tool::EraseWall), "erase tool", G::Tools),
    s(KeyCode::Digit4, "4", A::SelectTool(Tool::Faucet), "faucet tool", G::Tools),
    held(KeyCode::BracketLeft, "[", A::BrushSmaller, "smaller brush", G::Tools),
    held(KeyCode::BracketRight, "]", A::BrushBigger, "bigger brush", G::Tools),
    s(KeyCode::KeyO, "O", A::NextOverlay, "next overlay field", G::View),
    s(KeyCode::KeyH, "H", A::ToggleParticles, "hide / show particles", G::View),
    s(KeyCode::KeyL, "L", A::ToggleLiquid, "liquid surface on / off", G::View),
    s(KeyCode::KeyM, "M", A::ToggleMusic, "play / pause music", G::View),
    s(KeyCode::Backquote, "`", A::TogglePanel, "hide / show the side panel", G::View),
    s(KeyCode::F1, "F1", A::ToggleCheatSheet, "this cheat sheet", G::View),
    s(KeyCode::F11, "F11", A::TogglePresentation, "full-screen visualizer", G::View),
    s(KeyCode::Comma, ",", A::PreviousLook, "previous visualizer look", G::View),
    s(KeyCode::Period, ".", A::NextLook, "next visualizer look", G::View),
    s(KeyCode::KeyA, "A", A::ToggleAutoLooks, "change looks automatically", G::View),
];

/// Mouse actions, for the cheat sheet and the Info tab.
pub const MOUSE: &[(&str, &str)] = &[
    ("Left drag", "use the selected tool"),
    ("Right drag", "push, whatever the tool"),
    ("Left drag in 3D", "orbit the camera"),
    ("Wheel in 3D", "zoom"),
    ("Drop a file", "load music"),
    ("Esc", "close this sheet, leave full screen, or quit"),
];

/// The action bound to `key`, if any (ignoring auto-repeat unless the action repeats).
pub fn lookup(key: KeyCode, repeat: bool) -> Option<Action> {
    SHORTCUTS.iter().find(|s| s.key == key && (s.repeats || !repeat)).map(|s| s.action)
}

/// How the key for `action` is written, for hints.
pub fn keys(action: Action) -> Option<&'static str> {
    SHORTCUTS.iter().find(|s| s.action == action).map(|s| s.keys)
}

/// `text`, followed by the action's key in parentheses: "Reset (R)".
pub fn tip(text: &str, action: Action) -> String {
    match keys(action) {
        Some(k) => format!("{text} ({k})"),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_and_action_appears_once() {
        for (i, a) in SHORTCUTS.iter().enumerate() {
            for b in &SHORTCUTS[i + 1..] {
                assert_ne!(a.key, b.key, "{} and {} share a key", a.what, b.what);
                assert_ne!(a.action, b.action, "{} and {} share an action", a.what, b.what);
            }
        }
    }

    #[test]
    fn auto_repeat_only_fires_repeating_actions() {
        assert_eq!(lookup(KeyCode::KeyR, false), Some(Action::Reset));
        assert_eq!(lookup(KeyCode::KeyR, true), None);
        assert_eq!(lookup(KeyCode::ArrowLeft, true), Some(Action::TiltLeft));
        assert_eq!(lookup(KeyCode::KeyQ, false), None);
    }

    #[test]
    fn tips_name_the_key() {
        assert_eq!(tip("Reset", Action::Reset), "Reset (R)");
        assert_eq!(tip("Faucet", Action::SelectTool(Tool::Faucet)), "Faucet (4)");
    }
}
