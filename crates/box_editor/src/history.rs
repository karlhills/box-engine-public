//! The undo history as a list (Edit ▸ History and the command palette):
//! every undo and redo step by label, most recent first, with the current
//! position marked; picking one undoes or redoes up to it.

use crate::state::EditorState;

/// One state of the undo history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryEntry {
    /// Undo steps applied in this state (0: as the scene was opened).
    pub depth: usize,
    /// The step that leads to it ("Move Cube"), or "Scene opened".
    pub label: String,
    /// The world is in this state now.
    pub current: bool,
    /// A redo step (ahead of the current state).
    pub redo: bool,
}

/// The label of the state before any edit.
pub const HISTORY_START: &str = "Scene opened";

impl EditorState {
    /// The undo history, most recent first: the redo steps (furthest
    /// first), then the undo steps, then [`HISTORY_START`]. Empty when there
    /// is nothing to undo or redo.
    pub fn history_entries(&self) -> Vec<HistoryEntry> {
        let undo: Vec<&str> = self.edits.undo_labels().collect();
        let redo: Vec<&str> = self.edits.redo_labels().collect();
        if undo.is_empty() && redo.is_empty() {
            return Vec::new();
        }
        let cur = undo.len();
        let mut out = Vec::with_capacity(undo.len() + redo.len() + 1);
        for (i, label) in redo.iter().enumerate().rev() {
            out.push(HistoryEntry {
                depth: cur + i + 1,
                label: (*label).to_owned(),
                current: false,
                redo: true,
            });
        }
        for (i, label) in undo.iter().enumerate().rev() {
            out.push(HistoryEntry {
                depth: i + 1,
                label: (*label).to_owned(),
                current: i + 1 == cur,
                redo: false,
            });
        }
        out.push(HistoryEntry {
            depth: 0,
            label: HISTORY_START.into(),
            current: cur == 0,
            redo: false,
        });
        out
    }

    /// Undoes or redoes until `depth` undo steps are applied (clamped to the
    /// history), with one note for the whole jump. Not during Play (the
    /// history belongs to the edit world). Stops at a step that fails.
    pub fn jump_history(&mut self, depth: usize) {
        if self.is_playing() {
            return;
        }
        self.finish_edit();
        let total = self.edits.undo_len() + self.edits.redo_len();
        let depth = depth.min(total);
        let start = self.edits.undo_len();
        while self.edits.undo_len() > depth {
            if !self.step_history(false, false) {
                break;
            }
        }
        while self.edits.undo_len() < depth {
            if !self.step_history(true, false) {
                break;
            }
        }
        let now = self.edits.undo_len();
        if now != start {
            let label = self
                .edits
                .undo_label()
                .map_or_else(|| HISTORY_START.to_owned(), str::to_owned);
            let steps = now.abs_diff(start);
            let verb = if now < start { "Undo" } else { "Redo" };
            self.notify(&format!(
                "{verb} {steps} step{}: {label}",
                if steps == 1 { "" } else { "s" }
            ));
        }
    }
}
