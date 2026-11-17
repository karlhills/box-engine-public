//! Hide and lock (DX review E17): editor-only sets of entities, by uid.
//!
//! - **Hidden** entities (and everything below them) are not drawn in the
//!   editor viewport and cannot be picked there. The renderer skips every
//!   entity carrying the runtime marker [`box_render::Hidden`]; the editor
//!   keeps the markers in step with the set on the edit world
//!   ([`EditorState::sync_hidden_markers`], before each render). The marker
//!   is not registered, so it is never saved, duplicated or recorded by
//!   undo, and the Play world, built from a scene snapshot, never has it.
//! - **Locked** entities (and everything below them) cannot be picked,
//!   marquee-selected or moved with the gizmo in the viewport; they stay
//!   selectable in the hierarchy and editable in the inspector.
//!
//! Neither is scene data nor an undo step. Both last for the session (they
//! are not written to `.box/editor.json`): uids of other scenes in the sets
//! are harmless, and Alt+H shows everything again.

use std::collections::HashSet;

use box_core::identity::Uid;
use box_core::prelude::{Entity, World};
use box_render::Hidden;

use crate::state::EditorState;

/// The hide and lock sets, part of [`EditorState`].
#[derive(Clone, Debug, Default)]
pub struct Visibility {
    pub(crate) hidden: HashSet<Uid>,
    pub(crate) locked: HashSet<Uid>,
    /// Entities that got a [`Hidden`] marker from the last sync.
    marked: HashSet<Entity>,
}

/// `e` or one of its ancestors has a uid in `set`.
fn in_subtree_of(world: &World, set: &HashSet<Uid>, e: Entity) -> bool {
    if set.is_empty() {
        return false;
    }
    let mut cur = Some(e);
    while let Some(c) = cur {
        if world.uid(c).is_some_and(|u| set.contains(&u)) {
            return true;
        }
        cur = world.parent(c);
    }
    false
}

impl EditorState {
    /// `uid` itself is hidden (its eye is off).
    pub fn is_hidden(&self, uid: Uid) -> bool {
        self.visibility.hidden.contains(&uid)
    }

    /// `uid` itself is locked.
    pub fn is_locked(&self, uid: Uid) -> bool {
        self.visibility.locked.contains(&uid)
    }

    /// `e` is not drawn in the viewport: it or an ancestor is hidden.
    pub fn hidden_in_viewport(&self, e: Entity) -> bool {
        in_subtree_of(&self.world, &self.visibility.hidden, e)
    }

    /// `e` cannot be picked, marquee-selected or moved in the viewport: it
    /// or an ancestor is locked (or hidden: not drawn, so not pickable).
    pub fn blocked_in_viewport(&self, e: Entity) -> bool {
        in_subtree_of(&self.world, &self.visibility.locked, e) || self.hidden_in_viewport(e)
    }

    /// Hides or shows `uids` (each with its subtree). Not an undo step.
    pub fn set_hidden(&mut self, uids: &[Uid], hidden: bool) {
        for &u in uids {
            if hidden {
                self.visibility.hidden.insert(u);
            } else {
                self.visibility.hidden.remove(&u);
            }
        }
    }

    /// The eye toggle of a hierarchy row.
    pub fn toggle_hidden(&mut self, uid: Uid) {
        let hide = !self.is_hidden(uid);
        self.set_hidden(&[uid], hide);
    }

    /// H: hides the selection, or shows it when all of it is hidden.
    pub fn toggle_hide_selection(&mut self) {
        if self.is_playing() || self.selection.is_empty() {
            return;
        }
        let hide = !self.selection.iter().all(|u| self.is_hidden(*u));
        let uids = self.selection.clone();
        self.set_hidden(&uids, hide);
        let what = match uids.len() {
            1 => "1 entity".to_string(),
            n => format!("{n} entities"),
        };
        self.notify(&format!("{} {what}", if hide { "Hid" } else { "Showed" }));
    }

    /// Alt+H: shows every hidden entity.
    pub fn unhide_all(&mut self) {
        let n = self.visibility.hidden.len();
        self.visibility.hidden.clear();
        if n > 0 {
            self.notify("Showed all hidden entities");
        }
    }

    /// Locks or unlocks `uids` (each with its subtree). Not an undo step.
    pub fn set_locked(&mut self, uids: &[Uid], locked: bool) {
        for &u in uids {
            if locked {
                self.visibility.locked.insert(u);
            } else {
                self.visibility.locked.remove(&u);
            }
        }
    }

    /// The lock toggle of a hierarchy row.
    pub fn toggle_locked(&mut self, uid: Uid) {
        let lock = !self.is_locked(uid);
        self.set_locked(&[uid], lock);
    }

    /// Puts the renderer's [`Hidden`] marker on every entity of the hidden
    /// subtrees in the edit world and takes it off the others it marked.
    /// Not during Play (the play world is never marked). Called before each
    /// viewport render; cost: nothing while nothing is or was hidden,
    /// otherwise one walk over the hidden subtrees.
    pub fn sync_hidden_markers(&mut self) {
        if self.is_playing() {
            return;
        }
        let v = &mut self.visibility;
        if v.hidden.is_empty() && v.marked.is_empty() {
            return;
        }
        let world = &mut self.world;
        let mut want: HashSet<Entity> = HashSet::new();
        let mut stack: Vec<Entity> = v
            .hidden
            .iter()
            .filter_map(|u| world.entity_by_uid(*u))
            .collect();
        while let Some(e) = stack.pop() {
            if want.insert(e) {
                stack.extend(world.children(e));
            }
        }
        for &e in &want {
            if !world.has::<Hidden>(e) {
                let _ = world.insert_one(e, Hidden);
            }
        }
        for &e in v.marked.difference(&want) {
            if world.contains(e) {
                let _ = world.remove_one::<Hidden>(e);
            }
        }
        v.marked = want;
    }

    /// Forgets the entities marked in the previous edit world (a new world
    /// was loaded; the hidden and locked uids stay).
    pub(crate) fn reset_hidden_markers(&mut self) {
        self.visibility.marked.clear();
    }
}
