//! Game-facing visibility ([`Visible`]) and runtime-only entities
//! ([`Transient`]).
//!
//! `Visible(false)` hides an entity and everything below it: the renderer
//! does not draw its meshes (main, shadow and outline passes), its particle
//! emitters stop and lose their particles, and editor picking passes through
//! it. Colliders, scripts and every other component keep working: hiding is
//! about drawing only. It is scene data (saved, undoable, shown in the
//! inspector), unlike the editor's hierarchy eye, which is editor state and
//! never reaches a game.
//!
//! [`hidden_entities`] lists what is hidden this frame. It costs one query
//! over `Visible` components when nothing is hidden, plus one walk over each
//! hidden subtree otherwise; consumers call it once per frame.

use std::collections::HashSet;

use crate::reflect::{Hidden, NotSaved, Reflect, ReflectComponent, ReflectDefault};
use crate::world::{Entity, World};

/// Whether this entity and its descendants are drawn (default `true`). A
/// hidden entity's meshes are not drawn, its particle emitters stop (their
/// particles are dropped) and picking ignores it; physics and scripts are
/// unaffected. A hidden parent hides its children whatever theirs say.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Reflect)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Visible(pub bool);

impl Default for Visible {
    fn default() -> Self {
        Self(true)
    }
}

/// Component: a runtime-only entity (pooled projectiles, spawned enemies,
/// effects). It and its descendants are never written to scene or save
/// files, and loading a save game despawns them (the loaded state does not
/// contain them). Scripts spawn one with `world.spawn{ ..., transient = true }`.
/// Not shown in the add-component menu.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
#[reflect(Component, Default, Clone, Debug, PartialEq, @NotSaved, @Hidden)]
#[type_path = "box_engine"]
pub struct Transient;

/// Entities hidden by a `Visible(false)` on themselves or an ancestor.
/// Empty (no allocation beyond the set) when no entity is hidden.
pub fn hidden_entities(world: &World) -> HashSet<Entity> {
    let mut out = HashSet::new();
    let mut stack: Vec<Entity> = world
        .query::<(Entity, &Visible)>()
        .iter()
        .filter(|(_, v)| !v.0)
        .map(|(e, _)| e)
        .collect();
    while let Some(e) = stack.pop() {
        if out.insert(e) {
            stack.extend(world.children(e));
        }
    }
    out
}

/// True when `e` or an ancestor has `Visible(false)` (walks the parents;
/// for one entity, where [`hidden_entities`] would walk every hidden
/// subtree).
pub fn is_hidden(world: &World, e: Entity) -> bool {
    let mut cur = Some(e);
    // Bounded: a malformed parent chain must not loop forever.
    for _ in 0..4096 {
        let Some(c) = cur else { return false };
        if world.get::<Visible>(c).is_some_and(|v| !v.0) {
            return true;
        }
        cur = world.parent(c);
    }
    false
}

/// Entities that are [`Transient`] or below one.
pub fn transient_entities(world: &World) -> HashSet<Entity> {
    let mut out = HashSet::new();
    let mut stack: Vec<Entity> = world
        .query::<(Entity, &Transient)>()
        .iter()
        .map(|(e, _)| e)
        .collect();
    while let Some(e) = stack.pop() {
        if out.insert(e) {
            stack.extend(world.children(e));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Uid;
    use crate::reflect::Registry;
    use crate::scene;
    use crate::transform::Transform;

    #[test]
    fn a_hidden_parent_hides_its_subtree() {
        let mut w = World::new();
        let a = w.spawn((Transform::IDENTITY,));
        let b = w.spawn((Transform::IDENTITY, Visible(true)));
        let c = w.spawn((Transform::IDENTITY,));
        let other = w.spawn((Transform::IDENTITY,));
        w.set_parent(b, Some(a)).unwrap();
        w.set_parent(c, Some(b)).unwrap();
        assert!(hidden_entities(&w).is_empty());
        let _ = w.insert_one(a, Visible(false));
        let hidden = hidden_entities(&w);
        assert_eq!(hidden, [a, b, c].into_iter().collect());
        assert!(is_hidden(&w, c) && !is_hidden(&w, other));
    }

    #[test]
    fn visible_is_saved_and_transient_entities_are_not() {
        let mut w = World::with_registry(std::sync::Arc::new(Registry::with_core_types()));
        let (kept, gone, child) = (Uid::new(), Uid::new(), Uid::new());
        w.spawn_saved(kept, "Kept", (Transform::IDENTITY, Visible(false)))
            .unwrap();
        let ge = w
            .spawn_saved(gone, "Projectile", (Transform::IDENTITY, Transient))
            .unwrap();
        let ce = w
            .spawn_saved(child, "Trail", (Transform::IDENTITY,))
            .unwrap();
        w.set_parent(ce, Some(ge)).unwrap();
        let json = scene::save_string(&w).unwrap();
        assert!(json.contains("\"Visible\": false"), "{json}");
        assert!(json.contains(&kept.to_string()));
        assert!(!json.contains(&gone.to_string()) && !json.contains(&child.to_string()));
        assert!(!json.contains("Transient"));
        assert_eq!(transient_entities(&w).len(), 2);
    }
}
