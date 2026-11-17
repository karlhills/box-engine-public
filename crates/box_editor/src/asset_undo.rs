//! Undo for asset file edits (DX review E12). Each write of a material
//! edited in the inspector (debounced: when the drag or typing ends, or
//! 300 ms after the last change) is one step of that file's own history:
//! the bytes before and after. While the inspector shows an editable
//! material, undo and redo (Cmd+Z, Cmd+Shift+Z, Edit menu) step that
//! history, rewriting the file through `AssetServer::write` (which
//! hot-reloads it); the scene's undo stack is untouched. With an entity in
//! the inspector, undo and redo are the scene's again.
//!
//! Histories are kept per file for the session (closing and reopening a
//! material keeps its steps) and are dropped with the asset server (project
//! or scene switch).

use std::collections::HashMap;

use box_core::asset_ref::AssetId;

use crate::assets::file_name;
use crate::state::EditorState;

/// One write of an asset file.
#[derive(Clone, Debug)]
pub(crate) struct FileStep {
    /// What changed, e.g. `Roughness`.
    pub label: String,
    pub before: Vec<u8>,
    pub after: Vec<u8>,
}

/// One file's undo and redo steps.
#[derive(Clone, Debug, Default)]
pub(crate) struct FileHistory {
    pub undo: Vec<FileStep>,
    pub redo: Vec<FileStep>,
}

/// Every file's history, part of the browser state.
#[derive(Clone, Debug, Default)]
pub(crate) struct AssetHistories(pub HashMap<AssetId, FileHistory>);

impl AssetHistories {
    /// Records a write of `id`: a new step, and nothing left to redo.
    pub fn record(&mut self, id: AssetId, step: FileStep) {
        let h = self.0.entry(id).or_default();
        h.redo.clear();
        h.undo.push(step);
    }
}

impl EditorState {
    /// The file whose history undo and redo step: the material shown in the
    /// inspector, when it is an editable file.
    pub fn asset_undo_target(&self) -> Option<AssetId> {
        let insp = self.assets.inspect.as_ref()?;
        let m = insp.material.as_ref()?;
        (!m.read_only && insp.item.label.is_none()).then_some(insp.item.id)
    }

    /// Undo and redo act on an asset file's history (see the module docs).
    pub fn asset_undo_active(&self) -> bool {
        self.asset_undo_target().is_some()
    }

    /// The Edit menu's text for undo or redo: `"Undo stone.material.json:
    /// Roughness"` while a material is shown, `"Undo Cube: Translation"`
    /// otherwise; and whether it can run (during Play only asset steps can).
    pub fn undo_menu_item(&self, redo: bool) -> (String, bool) {
        let verb = if redo { "Redo" } else { "Undo" };
        if let Some(id) = self.asset_undo_target() {
            let pending = !redo && self.material_edit_pending();
            let step = self
                .assets
                .histories
                .0
                .get(&id)
                .and_then(|h| if redo { h.redo.last() } else { h.undo.last() });
            let file = self
                .asset_server()
                .and_then(|s| s.path_of(id))
                .map(|p| file_name(&p).to_owned())
                .unwrap_or_else(|| id.to_string());
            let pending_label = self.pending_material_label();
            return match (step, pending) {
                (_, true) => (
                    format!("{verb} {file}: {}", pending_label.unwrap_or("Edit".into())),
                    true,
                ),
                (Some(s), _) => (format!("{verb} {file}: {}", s.label), true),
                (None, _) => (format!("{verb} {file}"), false),
            };
        }
        let label = if redo {
            self.edits.redo_label()
        } else {
            self.edits.undo_label()
        };
        let can = !self.is_playing()
            && if redo {
                self.edits.can_redo()
            } else {
                self.edits.can_undo()
            };
        match label {
            Some(l) => (format!("{verb} {l}"), can),
            None => (verb.to_owned(), can),
        }
    }

    fn material_edit_pending(&self) -> bool {
        self.assets
            .inspect
            .as_ref()
            .and_then(|i| i.material.as_ref())
            .is_some_and(|m| m.pending_since.is_some())
    }

    fn pending_material_label(&self) -> Option<String> {
        self.assets
            .inspect
            .as_ref()
            .and_then(|i| i.material.as_ref())
            .and_then(|m| m.pending_label.clone())
    }

    /// Steps the shown material file's history back (or forward with
    /// `redo`): pending edits are written first, then the file gets the
    /// step's bytes through the asset server and the inspector its values.
    /// Returns whether a step was taken.
    pub(crate) fn step_asset_history(&mut self, redo: bool) -> bool {
        let Some(id) = self.asset_undo_target() else {
            return false;
        };
        // An edit still waiting for its write is a step of its own.
        self.flush_material_edit(f64::INFINITY, true);
        let Some(server) = self.asset_server.clone() else {
            return false;
        };
        let Some(h) = self.assets.histories.0.get_mut(&id) else {
            return false;
        };
        let Some(step) = (if redo { h.redo.pop() } else { h.undo.pop() }) else {
            return false;
        };
        let bytes = if redo { &step.after } else { &step.before };
        if let Err(e) = server.write(id, bytes) {
            tracing::error!(
                "material: could not {}: {e}",
                if redo { "redo" } else { "undo" }
            );
            // Back where it was: nothing changed.
            let h = self.assets.histories.0.entry(id).or_default();
            if redo {
                h.redo.push(step)
            } else {
                h.undo.push(step)
            }
            return false;
        }
        let registry = self.world.registry().clone();
        let value = crate::assets::material_from_file(&registry, bytes);
        if let Some(m) = self
            .assets
            .inspect
            .as_mut()
            .and_then(|i| i.material.as_mut())
        {
            if let Some(v) = value {
                m.value = v;
            }
            m.written = true;
            m.last_written = Some(bytes.clone());
        }
        let file = server
            .path_of(id)
            .map(|p| file_name(&p).to_owned())
            .unwrap_or_default();
        let verb = if redo { "Redo" } else { "Undo" };
        tracing::info!("{} {file}: {}", verb.to_lowercase(), step.label);
        self.notify(&format!("{verb}: {file}: {}", step.label));
        let h = self.assets.histories.0.entry(id).or_default();
        if redo {
            h.undo.push(step);
        } else {
            h.redo.push(step);
        }
        true
    }

    /// Undo steps recorded for the asset file `id`.
    pub fn asset_undo_len(&self, id: AssetId) -> usize {
        self.assets.histories.0.get(&id).map_or(0, |h| h.undo.len())
    }
}
