//! Every keyboard shortcut, in one table. The key handler, the tooltips, the cheat sheet
//! (F1) and the Info tab all read it, so they can't drift apart.
//!
//! In full screen the visualizer's keys (`Group::Visualizer`, MilkDrop's) take over the
//! keys they share with the rest: Space, H and L change looks there instead of pausing
//! or hiding things.

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
    UndoWall,
    TiltLeft,
    TiltRight,
    Stronger,
    Weaker,
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
    BackLook,
    RateUp,
    RateDown,
    ShowTitle,
    ToggleBrowser,
    ToggleEditor,
    Screenshot,
    ToggleRecording,
    NextDisplay,
    NextLookFade,
    HardCut,
    LockLook,
    RandomLook,
    VaryLook,
}

/// Cheat-sheet columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Simulation,
    Tools,
    View,
    /// Only in full screen, over the others' keys.
    Visualizer,
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
    /// The modifier held with the key.
    pub mods: Mods,
}

/// A modifier held with a shortcut's key (one at most).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mods {
    #[default]
    None,
    Shift,
    Ctrl,
}

const fn s(key: KeyCode, keys: &'static str, action: Action, what: &'static str, group: Group) -> Shortcut {
    Shortcut { key, keys, action, what, group, repeats: false, mods: Mods::None }
}

const fn held(key: KeyCode, keys: &'static str, action: Action, what: &'static str, group: Group) -> Shortcut {
    Shortcut { key, keys, action, what, group, repeats: true, mods: Mods::None }
}

const fn shifted(key: KeyCode, keys: &'static str, action: Action, what: &'static str, group: Group) -> Shortcut {
    Shortcut { key, keys, action, what, group, repeats: false, mods: Mods::Shift }
}

const fn ctrl(key: KeyCode, keys: &'static str, action: Action, what: &'static str, group: Group) -> Shortcut {
    Shortcut { key, keys, action, what, group, repeats: false, mods: Mods::Ctrl }
}

use Action as A;
use Group as G;
pub const SHORTCUTS: &[Shortcut] = &[
    s(KeyCode::Space, "Space", A::TogglePause, "pause / play", G::Simulation),
    s(KeyCode::KeyS, "S", A::Step, "step (while paused)", G::Simulation),
    s(KeyCode::KeyR, "R", A::Reset, "reset the scene", G::Simulation),
    s(KeyCode::KeyP, "P", A::NextPreset, "next scene", G::Simulation),
    s(KeyCode::KeyN, "N", A::NextSolver, "next solver", G::Simulation),
    held(KeyCode::ArrowLeft, "Left", A::TiltLeft, "turn gravity anticlockwise", G::Simulation),
    held(KeyCode::ArrowRight, "Right", A::TiltRight, "turn gravity clockwise", G::Simulation),
    held(KeyCode::ArrowUp, "Up", A::Stronger, "stronger gravity", G::Simulation),
    held(KeyCode::ArrowDown, "Down", A::Weaker, "weaker gravity", G::Simulation),
    s(KeyCode::Digit1, "1", A::SelectTool(Tool::Push), "push tool", G::Tools),
    s(KeyCode::Digit2, "2", A::SelectTool(Tool::PaintWall), "wall tool", G::Tools),
    s(KeyCode::Digit3, "3", A::SelectTool(Tool::EraseWall), "erase tool", G::Tools),
    s(KeyCode::Digit4, "4", A::SelectTool(Tool::Faucet), "faucet tool", G::Tools),
    held(KeyCode::BracketLeft, "[", A::BrushSmaller, "smaller brush", G::Tools),
    held(KeyCode::BracketRight, "]", A::BrushBigger, "bigger brush", G::Tools),
    ctrl(KeyCode::KeyZ, "Ctrl+Z", A::UndoWall, "undo the last wall stroke", G::Tools),
    s(KeyCode::KeyO, "O", A::NextOverlay, "next overlay field", G::View),
    s(KeyCode::KeyH, "H", A::ToggleParticles, "hide / show particles", G::View),
    s(KeyCode::KeyL, "L", A::ToggleLiquid, "liquid surface on / off", G::View),
    s(KeyCode::KeyM, "M", A::ToggleMusic, "play / pause music", G::View),
    s(KeyCode::Backquote, "`", A::TogglePanel, "hide / show the side panel", G::View),
    s(KeyCode::F1, "F1", A::ToggleCheatSheet, "this cheat sheet", G::View),
    s(KeyCode::F11, "F11", A::TogglePresentation, "full-screen visualizer", G::View),
    shifted(KeyCode::F11, "Shift+F11", A::NextDisplay, "move to the next display", G::View),
    s(KeyCode::Comma, ",", A::PreviousLook, "previous visualizer look", G::View),
    s(KeyCode::Period, ".", A::NextLook, "next visualizer look", G::View),
    s(KeyCode::KeyA, "A", A::ToggleAutoLooks, "change looks automatically", G::View),
    s(KeyCode::Backspace, "Backspace", A::BackLook, "back to the look before", G::View),
    s(KeyCode::KeyD, "D", A::RandomLook, "a new random look", G::View),
    shifted(KeyCode::KeyD, "Shift+D", A::VaryLook, "vary the look on screen", G::View),
    s(KeyCode::Equal, "+", A::RateUp, "rate the look higher", G::View),
    s(KeyCode::Minus, "-", A::RateDown, "rate the look lower", G::View),
    s(KeyCode::KeyT, "T", A::ShowTitle, "show the song's title", G::View),
    s(KeyCode::KeyB, "B", A::ToggleBrowser, "browse the looks", G::View),
    s(KeyCode::KeyE, "E", A::ToggleEditor, "edit the look's equations", G::View),
    s(KeyCode::F12, "F12", A::Screenshot, "save a screenshot", G::View),
    s(KeyCode::F10, "F10", A::ToggleRecording, "start / stop recording a video", G::View),
    s(KeyCode::Space, "Space", A::NextLookFade, "next look", G::Visualizer),
    s(KeyCode::KeyH, "H", A::HardCut, "next look, cut without a fade", G::Visualizer),
    s(KeyCode::KeyL, "L", A::LockLook, "keep this look / let them change", G::Visualizer),
];

/// Mouse actions, for the cheat sheet and the Info tab.
pub const MOUSE: &[(&str, &str)] = &[
    ("Left drag", "use the selected tool"),
    ("Right drag", "push, whatever the tool"),
    ("Left drag in 3D", "orbit the camera"),
    ("Right drag in 3D", "push the gas, or hold a torch"),
    ("Wheel in 3D", "zoom"),
    ("Drop a file", "load music"),
    ("Esc", "close this sheet, leave full screen, or quit"),
];

/// The action bound to `key` with `mods` held, if any, ignoring auto-repeat unless the
/// action repeats; `presenting`: in full screen, where the visualizer's keys come first.
pub fn lookup(key: KeyCode, repeat: bool, presenting: bool, mods: Mods) -> Option<Action> {
    let find = |visualizer: bool| SHORTCUTS.iter().find(|s| s.key == key && s.mods == mods && (s.group == Group::Visualizer) == visualizer && (s.repeats || !repeat));
    presenting.then(|| find(true)).flatten().or_else(|| find(false)).map(|s| s.action)
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
                if (a.group == Group::Visualizer) == (b.group == Group::Visualizer) {
                    assert_ne!((a.key, a.mods), (b.key, b.mods), "{} and {} share a key", a.what, b.what);
                }
                assert_ne!(a.action, b.action, "{} and {} share an action", a.what, b.what);
            }
        }
    }

    #[test]
    fn auto_repeat_only_fires_repeating_actions() {
        assert_eq!(lookup(KeyCode::KeyR, false, false, Mods::None), Some(Action::Reset));
        assert_eq!(lookup(KeyCode::KeyR, true, false, Mods::None), None);
        assert_eq!(lookup(KeyCode::ArrowLeft, true, false, Mods::None), Some(Action::TiltLeft));
        assert_eq!(lookup(KeyCode::KeyQ, false, false, Mods::None), None);
    }

    /// In full screen the visualizer's keys win; elsewhere they keep their old meaning.
    #[test]
    fn full_screen_keys_take_over() {
        assert_eq!(lookup(KeyCode::Space, false, false, Mods::None), Some(Action::TogglePause));
        assert_eq!(lookup(KeyCode::Space, false, true, Mods::None), Some(Action::NextLookFade));
        assert_eq!(lookup(KeyCode::KeyH, false, true, Mods::None), Some(Action::HardCut));
        assert_eq!(lookup(KeyCode::KeyR, false, true, Mods::None), Some(Action::Reset), "the rest still work");
        assert_eq!(lookup(KeyCode::F11, false, false, Mods::Shift), Some(Action::NextDisplay), "Shift makes another key");
        assert_eq!(lookup(KeyCode::F11, false, false, Mods::None), Some(Action::TogglePresentation));
        assert_eq!(lookup(KeyCode::KeyD, false, true, Mods::None), Some(Action::RandomLook));
        assert_eq!(lookup(KeyCode::KeyD, false, false, Mods::Shift), Some(Action::VaryLook));
    }

    /// Ctrl+Z undoes a wall stroke; Z alone, or with Ctrl held, does nothing else.
    #[test]
    fn ctrl_makes_another_key() {
        assert_eq!(lookup(KeyCode::KeyZ, false, false, Mods::Ctrl), Some(Action::UndoWall));
        assert_eq!(lookup(KeyCode::KeyZ, false, false, Mods::None), None);
        assert_eq!(lookup(KeyCode::KeyR, false, false, Mods::Ctrl), None);
    }

    #[test]
    fn tips_name_the_key() {
        assert_eq!(tip("Reset", Action::Reset), "Reset (R)");
        assert_eq!(tip("Faucet", Action::SelectTool(Tool::Faucet)), "Faucet (4)");
    }
}
