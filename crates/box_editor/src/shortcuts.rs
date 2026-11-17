//! The editor's keyboard shortcuts: one table read by the shortcut handling
//! (`Editor::shortcuts`, `Editor::play_frame`, the hierarchy and asset
//! panes), the menus and tooltips, and Help ▸ Keyboard Shortcuts, so what is
//! shown never drifts from what is handled.
//!
//! Modifiers match exactly (Cmd+Shift+S never triggers Cmd+S, Shift+W never
//! triggers W), so the order of the checks does not matter. Mouse and
//! trackpad navigation is described next to its implementation, in
//! [`crate::camera::NAVIGATION`].

use egui::{Event, InputState, Key, KeyboardShortcut, Modifiers};

/// An action with a keyboard shortcut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Palette,
    Move,
    Rotate,
    Scale,
    FinishTool,
    ToggleSnap,
    Frame,
    Rename,
    Delete,
    Duplicate,
    Deselect,
    Hide,
    UnhideAll,
    Undo,
    Redo,
    Save,
    SaveAs,
    Play,
    Pause,
    Step,
    Rebuild,
    BrushSmaller,
    BrushLarger,
    BrushWeaker,
    BrushStronger,
    BrushErase,
}

/// One row of the shortcut table.
pub struct Shortcut {
    pub command: Command,
    /// What it does, as shown in Help ▸ Keyboard Shortcuts.
    pub label: &'static str,
    /// The heading it is listed under.
    pub group: &'static str,
    /// Any of these triggers it.
    pub keys: &'static [KeyboardShortcut],
}

const fn key(modifiers: Modifiers, key: Key) -> KeyboardShortcut {
    KeyboardShortcut::new(modifiers, key)
}

const CMD: Modifiers = Modifiers::COMMAND;
const CMD_SHIFT: Modifiers = Modifiers::COMMAND.plus(Modifiers::SHIFT);
const CMD_ALT: Modifiers = Modifiers::COMMAND.plus(Modifiers::ALT);
const NONE: Modifiers = Modifiers::NONE;
const ALT: Modifiers = Modifiers::ALT;

/// Every keyboard shortcut of the editor, in the order Help lists them.
pub const SHORTCUTS: &[Shortcut] = &[
    Shortcut {
        command: Command::Palette,
        label: "Command palette: search every action, scene, asset and recent project",
        group: "General",
        // Cmd+Shift+P is Pause, so the second key is F1 (as in VS Code).
        keys: &[key(CMD, Key::K), key(NONE, Key::F1)],
    },
    Shortcut {
        command: Command::Move,
        label: "Move tool",
        group: "Tools",
        keys: &[key(NONE, Key::W)],
    },
    Shortcut {
        command: Command::Rotate,
        label: "Rotate tool",
        group: "Tools",
        keys: &[key(NONE, Key::E)],
    },
    Shortcut {
        command: Command::Scale,
        label: "Scale tool",
        group: "Tools",
        keys: &[key(NONE, Key::R)],
    },
    Shortcut {
        command: Command::FinishTool,
        label: "Finish a viewport tool (Edit Spline, Place): back to the transform tools",
        group: "Tools",
        keys: &[key(NONE, Key::Enter)],
    },
    Shortcut {
        command: Command::ToggleSnap,
        label: "Snapping on or off (hold Cmd/Ctrl while dragging a gizmo to invert it)",
        group: "Tools",
        keys: &[key(NONE, Key::Y)],
    },
    Shortcut {
        command: Command::Frame,
        label: "Frame the selection (the scene when nothing is selected)",
        group: "View",
        keys: &[key(NONE, Key::F)],
    },
    Shortcut {
        command: Command::Rename,
        label: "Rename the selected entity, or the asset after a click in Assets",
        group: "Selection",
        keys: &[key(NONE, Key::F2)],
    },
    Shortcut {
        command: Command::Delete,
        label: "Delete the selection; in Assets, move the selected file to the trash; \
                in Edit Spline, the selected point",
        group: "Selection",
        keys: &[key(NONE, Key::Delete), key(NONE, Key::Backspace)],
    },
    Shortcut {
        command: Command::Duplicate,
        label: "Duplicate the selection",
        group: "Selection",
        keys: &[key(CMD, Key::D)],
    },
    Shortcut {
        command: Command::Deselect,
        label: "Select nothing; cancel a drag, marquee or asset drag",
        group: "Selection",
        keys: &[key(NONE, Key::Escape)],
    },
    Shortcut {
        command: Command::Hide,
        label: "Hide the selection in the viewport, or show it when it is hidden",
        group: "Selection",
        keys: &[key(NONE, Key::H)],
    },
    Shortcut {
        command: Command::UnhideAll,
        label: "Show every hidden entity",
        group: "Selection",
        keys: &[key(ALT, Key::H)],
    },
    Shortcut {
        command: Command::Undo,
        label: "Undo",
        group: "Edit",
        keys: &[key(CMD, Key::Z)],
    },
    Shortcut {
        command: Command::Redo,
        label: "Redo",
        group: "Edit",
        keys: &[key(CMD_SHIFT, Key::Z)],
    },
    Shortcut {
        command: Command::Save,
        label: "Save the scene",
        group: "Edit",
        keys: &[key(CMD, Key::S)],
    },
    Shortcut {
        command: Command::SaveAs,
        label: "Save the scene as a new file",
        group: "Edit",
        keys: &[key(CMD_SHIFT, Key::S)],
    },
    Shortcut {
        command: Command::Play,
        label: "Play, or Stop while playing",
        group: "Play",
        keys: &[key(CMD, Key::P)],
    },
    Shortcut {
        command: Command::Pause,
        label: "Pause or resume",
        group: "Play",
        keys: &[key(CMD_SHIFT, Key::P)],
    },
    Shortcut {
        command: Command::Step,
        label: "Step one frame while paused",
        group: "Play",
        keys: &[key(CMD_ALT, Key::P)],
    },
    Shortcut {
        command: Command::Rebuild,
        label: "Rebuild the game's code and restart the editor (native projects)",
        group: "Game",
        keys: &[key(CMD_SHIFT, Key::B)],
    },
    Shortcut {
        command: Command::BrushSmaller,
        label: "Smaller brush (Paint Scatter tool)",
        group: "Paint Scatter",
        keys: &[key(NONE, Key::OpenBracket)],
    },
    Shortcut {
        command: Command::BrushLarger,
        label: "Larger brush (Paint Scatter tool)",
        group: "Paint Scatter",
        keys: &[key(NONE, Key::CloseBracket)],
    },
    Shortcut {
        command: Command::BrushWeaker,
        label: "Weaker brush (Paint Scatter tool)",
        group: "Paint Scatter",
        keys: &[key(NONE, Key::Minus)],
    },
    Shortcut {
        command: Command::BrushStronger,
        label: "Stronger brush (Paint Scatter tool)",
        group: "Paint Scatter",
        keys: &[key(NONE, Key::Equals)],
    },
    Shortcut {
        command: Command::BrushErase,
        label: "Switch between adding and erasing (Paint Scatter tool; Shift while dragging erases)",
        group: "Paint Scatter",
        keys: &[key(NONE, Key::X)],
    },
];

/// The table row of `command`.
pub fn shortcut(command: Command) -> &'static Shortcut {
    SHORTCUTS
        .iter()
        .find(|s| s.command == command)
        .expect("every command has a row in SHORTCUTS")
}

/// The first (main) key of `command`, for menus that show one.
pub fn main_key(command: Command) -> &'static KeyboardShortcut {
    &shortcut(command).keys[0]
}

fn matches(event: &Event, k: &KeyboardShortcut) -> bool {
    matches!(
        event,
        Event::Key { key, modifiers, pressed: true, .. }
            if *key == k.logical_key && modifiers.matches_exact(k.modifiers)
    )
}

/// True when one of `command`'s keys was pressed this frame with exactly
/// its modifiers. Does not consume the key (for keys several panes look at).
pub fn pressed(i: &InputState, command: Command) -> bool {
    let keys = shortcut(command).keys;
    i.events.iter().any(|e| keys.iter().any(|k| matches(e, k)))
}

/// Like [`pressed`], and removes the key press from egui's input so no
/// widget acts on it too.
pub fn consume(i: &mut InputState, command: Command) -> bool {
    let keys = shortcut(command).keys;
    let before = i.events.len();
    i.events.retain(|e| !keys.iter().any(|k| matches(e, k)));
    i.events.len() != before
}

/// The keys of `command` as shown to the user, e.g. `⌘Z` or `Del / ⌫`.
pub fn text(ctx: &egui::Context, command: Command) -> String {
    shortcut(command)
        .keys
        .iter()
        .map(|k| ctx.format_shortcut(k))
        .collect::<Vec<_>>()
        .join(" / ")
}

/// `label (keys)`, for tooltips.
pub fn tip(ctx: &egui::Context, label: &str, command: Command) -> String {
    format!("{label} ({})", text(ctx, command))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(key: Key, modifiers: Modifiers) -> InputState {
        let mut i = InputState::default();
        i.events.push(Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        });
        i.modifiers = modifiers;
        i
    }

    #[test]
    fn every_command_has_one_row_and_keys() {
        for s in SHORTCUTS {
            assert!(!s.keys.is_empty(), "{:?}", s.command);
            assert_eq!(
                SHORTCUTS.iter().filter(|o| o.command == s.command).count(),
                1
            );
        }
    }

    #[test]
    fn no_two_commands_share_a_key() {
        let all: Vec<_> = SHORTCUTS.iter().flat_map(|s| s.keys.iter()).collect();
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert!(a != b, "{a:?} is bound twice");
            }
        }
    }

    #[test]
    fn modifiers_match_exactly() {
        let mut i = input(Key::S, Modifiers::COMMAND.plus(Modifiers::SHIFT));
        assert!(!pressed(&i, Command::Save));
        assert!(consume(&mut i, Command::SaveAs));
        assert!(!pressed(&i, Command::SaveAs), "consumed");
        let i = input(Key::W, Modifiers::SHIFT);
        assert!(!pressed(&i, Command::Move));
        let i = input(Key::Backspace, Modifiers::NONE);
        assert!(pressed(&i, Command::Delete));
    }
}
