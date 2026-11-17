//! The [`World`]: entities with components, plus singleton resources.
//!
//! Entity storage is `hecs`. Query types are re-exported rather than wrapped.
//!
//! The world also holds the shared reflection [`Registry`], the `Uid -> Entity`
//! index and the order of saved root entities. Both are maintained by
//! [`World::spawn`], [`World::spawn_saved`], [`World::insert`],
//! [`World::insert_one`], [`World::remove_one`], [`World::despawn`] and
//! [`World::set_parent`]. Raw access through [`World::ecs_mut`] bypasses them.
//!
//! Structural changes can also be queued through a shared reference while
//! queries are alive ([`World::defer`]); the app applies them after every
//! system.

mod deferred;
mod reflective;

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

pub use hecs::{
    Bundle, Component, ComponentError, DynamicBundle, Entity, NoSuchEntity, Query, QueryBorrow,
    QueryMut, Ref, RefMut, Satisfies, With, Without,
};

pub use deferred::Deferred;
pub use reflective::Placement;

use crate::identity::Uid;
use crate::reflect::Registry;
use crate::transform::{Children, Parent};

type ResourceMap = HashMap<TypeId, Box<dyn Any>>;

/// Tests whether an entity has one component type (see `watch_despawns`).
type HasComponent = fn(&hecs::World, Entity) -> bool;

/// A tuple of distinct resource types for [`World::resources_mut`]:
/// `(A, B)` up to `(A, B, C, D, E, F)`.
pub trait ResourceSet {
    /// The mutable references, `(&mut A, &mut B, ...)`.
    type Mut<'a>;
    #[doc(hidden)]
    fn type_ids() -> Vec<TypeId>;
    #[doc(hidden)]
    fn type_names() -> Vec<&'static str>;
    #[doc(hidden)]
    fn fetch(resources: &mut ResourceMap) -> Option<Self::Mut<'_>>;
}

macro_rules! resource_tuple {
    ($($t:ident),+) => {
        impl<$($t: 'static),+> ResourceSet for ($($t,)+) {
            type Mut<'a> = ($(&'a mut $t,)+);

            fn type_ids() -> Vec<TypeId> {
                vec![$(TypeId::of::<$t>()),+]
            }

            fn type_names() -> Vec<&'static str> {
                vec![$(std::any::type_name::<$t>()),+]
            }

            #[allow(non_snake_case)]
            fn fetch(resources: &mut ResourceMap) -> Option<Self::Mut<'_>> {
                // Panics if a type repeats (overlapping keys).
                let [$($t),+] = resources.get_disjoint_mut([$(&TypeId::of::<$t>()),+]);
                Some(($($t?.downcast_mut::<$t>()?,)+))
            }
        }
    };
}

resource_tuple!(A, B);
resource_tuple!(A, B, C);
resource_tuple!(A, B, C, D);
resource_tuple!(A, B, C, D, E);
resource_tuple!(A, B, C, D, E, F);

/// Entities, components and resources.
pub struct World {
    ecs: hecs::World,
    resources: HashMap<TypeId, Box<dyn Any>>,
    registry: Arc<Registry>,
    uids: HashMap<Uid, Entity>,
    /// Saved (Uid-carrying) entities without a parent, in user order.
    roots: Vec<Entity>,
    /// Commands queued through [`World::defer`].
    deferred: deferred::Queue,
    /// Component types whose despawns are recorded (`App::on_remove`).
    despawn_watch: Vec<(TypeId, HasComponent)>,
    /// Entities despawned while they had a watched component.
    despawned: Vec<(TypeId, Entity)>,
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

/// The registry shared by worlds created with [`World::new`]: box_core's own types.
fn core_registry() -> Arc<Registry> {
    static CORE: OnceLock<Arc<Registry>> = OnceLock::new();
    CORE.get_or_init(|| Arc::new(Registry::with_core_types()))
        .clone()
}

impl World {
    /// An empty world whose registry holds box_core's own types
    /// ([`crate::register_core_types`]).
    pub fn new() -> Self {
        Self::with_registry(core_registry())
    }

    /// An empty world sharing `registry` (game and plugin types included).
    pub fn with_registry(registry: Arc<Registry>) -> Self {
        Self {
            ecs: hecs::World::new(),
            resources: HashMap::new(),
            registry,
            uids: HashMap::new(),
            roots: Vec::new(),
            deferred: deferred::Queue::default(),
            despawn_watch: Vec::new(),
            despawned: Vec::new(),
        }
    }

    /// The reflection registry. Clone the `Arc` to build worlds that share it.
    pub fn registry(&self) -> &Arc<Registry> {
        &self.registry
    }

    /// Replace the registry, e.g. after registering game types at startup.
    pub fn set_registry(&mut self, registry: Arc<Registry>) {
        self.registry = registry;
    }

    // ----- entities -----

    /// Spawn an entity. A [`Uid`] in the bundle is indexed. Two live entities
    /// never share a uid: if the bundle's uid is already used, the existing
    /// entity keeps it and the new one gets a fresh [`Uid::new`] instead
    /// (this function cannot fail; use [`World::spawn_saved`] to reject a
    /// duplicate, and read the result back with [`World::uid`]).
    pub fn spawn(&mut self, components: impl DynamicBundle) -> Entity {
        let saved = components.has::<Uid>();
        let e = self.ecs.spawn(components);
        if saved {
            self.uid_changed(e, None);
        }
        e
    }

    /// Spawn a saved entity: `Uid`, `Name` and `components`. Fails if the uid
    /// is already used by a live entity. A `Uid` or `Name` in `components`
    /// replaces the given one, as with [`World::insert`].
    pub fn spawn_saved(
        &mut self,
        uid: Uid,
        name: &str,
        components: impl DynamicBundle,
    ) -> Result<Entity, crate::reflect::ReflectError> {
        if self.entity_by_uid(uid).is_some() {
            return Err(crate::reflect::ReflectError::DuplicateUid(uid));
        }
        let e = self.spawn((uid, crate::identity::Name::new(name)));
        let _ = self.insert(e, components);
        Ok(e)
    }

    /// Despawn an entity and, recursively, its children. Detaches it from its parent.
    /// Returns `false` if the entity did not exist.
    pub fn despawn(&mut self, entity: Entity) -> bool {
        if !self.ecs.contains(entity) {
            return false;
        }
        self.set_parent_unchecked(entity, None);
        self.remove_root(entity);
        let mut stack = vec![entity];
        while let Some(e) = stack.pop() {
            if let Ok(children) = self.ecs.get::<&Children>(e) {
                stack.extend(children.0.iter().copied());
            }
            if let Ok(uid) = self.ecs.get::<&Uid>(e).map(|u| *u)
                && self.uids.get(&uid) == Some(&e)
            {
                self.uids.remove(&uid);
            }
            for (ty, has) in &self.despawn_watch {
                if has(&self.ecs, e) {
                    self.despawned.push((*ty, e));
                }
            }
            let _ = self.ecs.despawn(e);
        }
        true
    }

    /// The live entity with this uid.
    pub fn entity_by_uid(&self, uid: Uid) -> Option<Entity> {
        self.uids
            .get(&uid)
            .copied()
            .filter(|e| self.ecs.contains(*e))
    }

    /// An entity's uid, if it is a saved entity.
    pub fn uid(&self, entity: Entity) -> Option<Uid> {
        self.ecs.get::<&Uid>(entity).ok().map(|u| *u)
    }

    /// Saved root entities (with a `Uid` and no parent) in their persisted order.
    /// This is the order of roots in scene files and the hierarchy panel.
    pub fn roots(&self) -> &[Entity] {
        &self.roots
    }

    /// Bookkeeping after an entity's `Uid` was inserted or replaced (`old` is
    /// the one it had before). A uid that another live entity already uses is
    /// not accepted: `e` keeps `old`, or gets a fresh uid if it had none.
    fn uid_changed(&mut self, e: Entity, old: Option<Uid>) {
        let Some(mut uid) = self.uid(e) else {
            return;
        };
        if self.entity_by_uid(uid).is_some_and(|owner| owner != e) {
            uid = old.unwrap_or_else(Uid::new);
            let _ = self.ecs.insert_one(e, uid);
        }
        if let Some(old) = old
            && old != uid
            && self.uids.get(&old) == Some(&e)
        {
            self.uids.remove(&old);
        }
        self.uids.insert(uid, e);
        if old.is_none() && !self.ecs.satisfies::<&Parent>(e) {
            self.roots.push(e);
        }
    }

    fn remove_root(&mut self, e: Entity) {
        // Usually the most recently added root (spawn, then parent), so search from the end.
        if let Some(i) = self.roots.iter().rposition(|r| *r == e) {
            self.roots.remove(i);
        }
    }

    pub fn contains(&self, entity: Entity) -> bool {
        self.ecs.contains(entity)
    }

    /// Add or replace components. A [`Uid`] in the bundle updates the uid
    /// index. A uid that another live entity already uses is not taken: the
    /// entity keeps the uid it had, or gets a fresh [`Uid::new`] if it had none.
    pub fn insert(
        &mut self,
        entity: Entity,
        components: impl DynamicBundle,
    ) -> Result<(), NoSuchEntity> {
        let saved = components.has::<Uid>();
        let old = if saved { self.uid(entity) } else { None };
        self.ecs.insert(entity, components)?;
        if saved {
            self.uid_changed(entity, old);
        }
        Ok(())
    }

    /// Add or replace one component. Inserting a [`Uid`] updates the uid index,
    /// with the same handling of a uid already in use as [`World::insert`].
    pub fn insert_one<T: Component>(
        &mut self,
        entity: Entity,
        component: T,
    ) -> Result<(), NoSuchEntity> {
        let saved = TypeId::of::<T>() == TypeId::of::<Uid>();
        let old = if saved { self.uid(entity) } else { None };
        self.ecs.insert_one(entity, component)?;
        if saved {
            self.uid_changed(entity, old);
        }
        Ok(())
    }

    /// Remove one component. Removing the [`Uid`] drops the entity from the
    /// uid index and the root order.
    pub fn remove_one<T: Component>(&mut self, entity: Entity) -> Result<T, ComponentError> {
        let saved = TypeId::of::<T>() == TypeId::of::<Uid>();
        let old = if saved { self.uid(entity) } else { None };
        let removed = self.ecs.remove_one::<T>(entity)?;
        if let Some(old) = old {
            if self.uids.get(&old) == Some(&entity) {
                self.uids.remove(&old);
            }
            self.remove_root(entity);
        }
        Ok(removed)
    }

    /// Borrow one component, checked at run time.
    ///
    /// # Panics
    /// Panics (inside hecs, without pointing at your code) if the component
    /// is mutably borrowed at the same time: a live [`World::get_mut`] guard,
    /// or a `world.query::<&mut T>()` being iterated.
    pub fn get<T: Component>(&self, entity: Entity) -> Option<Ref<'_, T>> {
        self.ecs.get::<&T>(entity).ok()
    }

    /// Mutably borrow one component through `&self`, checked at run time.
    /// With `&mut World` prefer [`World::component_mut`], which the compiler
    /// checks.
    ///
    /// # Panics
    /// Panics (inside hecs, "already borrowed", without pointing at your
    /// code) if the component is borrowed at the same time: another live
    /// `get`/`get_mut` guard for it, or a `world.query::<&T>()` being
    /// iterated. Collect what the loop needs first, or use
    /// [`World::query_mut`].
    pub fn get_mut<T: Component>(&self, entity: Entity) -> Option<RefMut<'_, T>> {
        self.ecs.get::<&mut T>(entity).ok()
    }

    /// Mutable access to one component, borrow-checked by the compiler (no
    /// run-time borrow flags, no panic): `world.component_mut::<Health>(e)`.
    /// `None` if the entity is gone or lacks the component.
    pub fn component_mut<T: Component>(&mut self, entity: Entity) -> Option<&mut T> {
        self.ecs.query_one_mut::<&mut T>(entity).ok()
    }

    /// The first entity named `name` (the [`Name`](crate::identity::Name)
    /// component), in storage order. Names need not be unique; this returns
    /// one of them.
    pub fn find_by_name(&self, name: &str) -> Option<Entity> {
        self.ecs
            .query::<(Entity, &crate::identity::Name)>()
            .iter()
            .find(|(_, n)| n.as_str() == name)
            .map(|(e, _)| e)
    }

    /// The first entity with a `T` component, in storage order: the player,
    /// the active level. `None` if there is none.
    pub fn first_with<T: Component>(&self) -> Option<Entity> {
        self.ecs.query::<Entity>().with::<&T>().iter().next()
    }

    /// How many entities match a query: `world.count::<&Coin>()`,
    /// `world.count::<(&Enemy, &Alive)>()`.
    pub fn count<Q: Query>(&self) -> usize {
        self.ecs.query::<Q>().iter().count()
    }

    pub fn has<T: Component>(&self, entity: Entity) -> bool {
        self.ecs.satisfies::<&T>(entity)
    }

    /// Borrow-checked query through a shared reference. Mutable component access
    /// (`&mut T`) is allowed and checked at run time.
    pub fn query<Q: Query>(&self) -> QueryBorrow<'_, Q> {
        self.ecs.query::<Q>()
    }

    /// Statically checked query through an exclusive reference.
    pub fn query_mut<Q: Query>(&mut self) -> QueryMut<'_, Q> {
        self.ecs.query_mut::<Q>()
    }

    pub fn entity_count(&self) -> u32 {
        self.ecs.len()
    }

    /// Escape hatch to the underlying storage.
    pub fn ecs(&self) -> &hecs::World {
        &self.ecs
    }

    pub fn ecs_mut(&mut self) -> &mut hecs::World {
        &mut self.ecs
    }

    // ----- hierarchy -----

    /// Set or clear an entity's parent, keeping `Parent` and `Children` consistent.
    /// The local transform is kept as-is. A saved entity that becomes a root is
    /// appended to the root order. See [`World::place`] to also set the sibling index.
    ///
    /// Fails without changing anything if `child` or `parent` does not exist
    /// ([`ReflectError::NoEntity`](crate::reflect::ReflectError::NoEntity)) or if `parent` is `child` or one of its
    /// descendants ([`ReflectError::Cycle`](crate::reflect::ReflectError::Cycle)).
    pub fn set_parent(
        &mut self,
        child: Entity,
        parent: Option<Entity>,
    ) -> Result<(), crate::reflect::ReflectError> {
        use crate::reflect::ReflectError;
        if !self.ecs.contains(child) {
            return Err(ReflectError::NoEntity(child));
        }
        if let Some(p) = parent {
            if !self.ecs.contains(p) {
                return Err(ReflectError::NoEntity(p));
            }
            if self.is_ancestor_or_self(child, p) {
                return Err(ReflectError::Cycle);
            }
        }
        self.set_parent_unchecked(child, parent);
        Ok(())
    }

    /// `set_parent` without the existence and cycle checks; callers have done them.
    fn set_parent_unchecked(&mut self, child: Entity, parent: Option<Entity>) {
        let saved = self.ecs.satisfies::<&Uid>(child);
        let had_parent = match self.ecs.remove_one::<Parent>(child) {
            Ok(old) => {
                // Usually the most recently added child (load, paste and their
                // cleanup work at the end of the list), so search from the end.
                if let Ok(mut children) = self.ecs.get::<&mut Children>(old.0)
                    && let Some(i) = children.0.iter().rposition(|c| *c == child)
                {
                    children.0.remove(i);
                }
                true
            }
            Err(_) => false,
        };
        let parent = parent.filter(|p| self.ecs.contains(*p) && self.ecs.contains(child));
        if saved {
            match (had_parent, parent.is_some()) {
                (false, true) => self.remove_root(child),
                (true, false) => self.roots.push(child),
                _ => {}
            }
        }
        if let Some(p) = parent {
            let _ = self.ecs.insert_one(child, Parent(p));
            let pushed = match self.ecs.get::<&mut Children>(p) {
                Ok(mut children) => {
                    children.0.push(child);
                    true
                }
                Err(_) => false,
            };
            if !pushed {
                let _ = self.ecs.insert_one(p, Children(vec![child]));
            }
        }
    }

    pub fn parent(&self, entity: Entity) -> Option<Entity> {
        self.ecs.get::<&Parent>(entity).ok().map(|p| p.0)
    }

    pub fn children(&self, entity: Entity) -> Vec<Entity> {
        self.ecs
            .get::<&Children>(entity)
            .map(|c| c.0.clone())
            .unwrap_or_default()
    }

    /// The nearest ancestor that has a [`Uid`] and is not generated (from a
    /// prefab or a rule): the parent as scene files and [`Placement`] see it.
    pub(crate) fn saved_parent(&self, entity: Entity) -> Option<Entity> {
        let mut cur = self.parent(entity);
        while let Some(p) = cur {
            if self.ecs.satisfies::<&Uid>(p)
                && !self.ecs.satisfies::<crate::prefab::AnyGenerated>(p)
            {
                return Some(p);
            }
            cur = self.parent(p);
        }
        None
    }

    /// True if `ancestor` is `entity` or one of its ancestors.
    fn is_ancestor_or_self(&self, ancestor: Entity, entity: Entity) -> bool {
        let mut cur = Some(entity);
        while let Some(e) = cur {
            if e == ancestor {
                return true;
            }
            cur = self.parent(e);
        }
        false
    }

    // ----- resources -----

    pub fn insert_resource<R: 'static>(&mut self, resource: R) {
        self.resources.insert(TypeId::of::<R>(), Box::new(resource));
    }

    pub fn remove_resource<R: 'static>(&mut self) -> Option<R> {
        self.resources
            .remove(&TypeId::of::<R>())
            .and_then(|b| b.downcast::<R>().ok())
            .map(|b| *b)
    }

    pub fn contains_resource<R: 'static>(&self) -> bool {
        self.resources.contains_key(&TypeId::of::<R>())
    }

    pub fn get_resource<R: 'static>(&self) -> Option<&R> {
        self.resources
            .get(&TypeId::of::<R>())
            .and_then(|b| b.downcast_ref())
    }

    pub fn get_resource_mut<R: 'static>(&mut self) -> Option<&mut R> {
        self.resources
            .get_mut(&TypeId::of::<R>())
            .and_then(|b| b.downcast_mut())
    }

    /// # Panics
    /// Panics if the resource is missing.
    pub fn resource<R: 'static>(&self) -> &R {
        self.get_resource()
            .unwrap_or_else(|| panic!("missing resource {}", std::any::type_name::<R>()))
    }

    /// # Panics
    /// Panics if the resource is missing.
    pub fn resource_mut<R: 'static>(&mut self) -> &mut R {
        self.get_resource_mut()
            .unwrap_or_else(|| panic!("missing resource {}", std::any::type_name::<R>()))
    }

    /// Several resources mutably at once:
    /// `let (score, scenes) = world.resources_mut::<(Score, Scenes)>();`.
    /// Takes tuples of 2 to 6 distinct types.
    ///
    /// # Panics
    /// Panics if a resource is missing (naming it) or a type appears twice.
    /// See [`World::get_resources_mut`] for the non-panicking form.
    pub fn resources_mut<S: ResourceSet>(&mut self) -> S::Mut<'_> {
        if let Some(missing) = S::type_ids()
            .into_iter()
            .zip(S::type_names())
            .find(|(id, _)| !self.resources.contains_key(id))
        {
            panic!("missing resource {}", missing.1);
        }
        S::fetch(&mut self.resources).expect("resources checked above")
    }

    /// [`World::resources_mut`], or `None` if any of the resources is missing.
    ///
    /// # Panics
    /// Panics if a type appears twice in the tuple.
    pub fn get_resources_mut<S: ResourceSet>(&mut self) -> Option<S::Mut<'_>> {
        S::fetch(&mut self.resources)
    }

    /// Get a resource, inserting `R::default()` first if it is missing.
    pub fn resource_or_default<R: Default + 'static>(&mut self) -> &mut R {
        self.resources
            .entry(TypeId::of::<R>())
            .or_insert_with(|| Box::new(R::default()))
            .downcast_mut()
            .expect("resource type mismatch")
    }

    /// Temporarily take a resource out of the world so it can be used together
    /// with `&mut World`.
    ///
    /// # Panics
    /// Panics if the resource is missing.
    pub fn resource_scope<R: 'static, T>(&mut self, f: impl FnOnce(&mut World, &mut R) -> T) -> T {
        let mut r = self
            .remove_resource::<R>()
            .unwrap_or_else(|| panic!("missing resource {}", std::any::type_name::<R>()));
        let out = f(self, &mut r);
        self.insert_resource(r);
        out
    }

    // ----- events -----

    /// Send an event: `world.send(Collected { points: 5 })`. Needs the
    /// `Events<T>` queue that `app.add_event::<T>()` (or `app.on_event`)
    /// installs; without it the event is dropped and a warning naming the
    /// fix is logged (once per event type).
    pub fn send<T: 'static>(&mut self, event: T) {
        match self.get_resource_mut::<crate::event::Events<T>>() {
            Some(events) => events.send(event),
            None => crate::event::warn_no_queue::<T>(),
        }
    }

    // ----- despawn records (for `App::on_remove`) -----

    /// Record despawns of entities that have a `T` from now on, for
    /// [`World::take_despawned`]. Idempotent.
    #[doc(hidden)]
    pub fn watch_despawns<T: Component>(&mut self) {
        let ty = TypeId::of::<T>();
        if !self.despawn_watch.iter().any(|(t, _)| *t == ty) {
            fn has<T: Component>(ecs: &hecs::World, e: Entity) -> bool {
                ecs.satisfies::<&T>(e)
            }
            self.despawn_watch.push((ty, has::<T>));
        }
    }

    /// Entities despawned with a `T` since the last call (see
    /// [`World::watch_despawns`]).
    #[doc(hidden)]
    pub fn take_despawned<T: Component>(&mut self) -> Vec<Entity> {
        if self.despawned.is_empty() {
            return Vec::new();
        }
        let ty = TypeId::of::<T>();
        let mut out = Vec::new();
        self.despawned.retain(|(t, e)| {
            if *t == ty {
                out.push(*e);
                false
            } else {
                true
            }
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resources_roundtrip() {
        let mut w = World::new();
        w.insert_resource(5u32);
        *w.resource_mut::<u32>() += 1;
        assert_eq!(*w.resource::<u32>(), 6);
        let v = w.resource_scope(|_w, r: &mut u32| *r * 2);
        assert_eq!(v, 12);
        assert_eq!(w.remove_resource::<u32>(), Some(6));
        assert!(!w.contains_resource::<u32>());
    }

    #[test]
    fn hierarchy_links_and_despawn() {
        let mut w = World::new();
        let a = w.spawn((1u8,));
        let b = w.spawn((2u8,));
        let c = w.spawn((3u8,));
        w.set_parent(b, Some(a)).unwrap();
        w.set_parent(c, Some(b)).unwrap();
        assert_eq!(w.children(a), vec![b]);
        assert_eq!(w.parent(c), Some(b));
        // Reparent c to a.
        w.set_parent(c, Some(a)).unwrap();
        assert_eq!(w.children(b), Vec::<Entity>::new());
        assert_eq!(w.children(a), vec![b, c]);
        // Despawning a removes the whole subtree.
        assert!(w.despawn(a));
        assert!(!w.contains(b) && !w.contains(c));
    }

    #[test]
    fn hierarchy_rejects_cycles() {
        use crate::reflect::ReflectError;
        let mut w = World::new();
        let a = w.spawn((1u8,));
        let b = w.spawn((2u8,));
        w.set_parent(b, Some(a)).unwrap();
        assert!(matches!(w.set_parent(a, Some(b)), Err(ReflectError::Cycle)));
        assert!(matches!(w.set_parent(a, Some(a)), Err(ReflectError::Cycle)));
        // Nothing changed.
        assert_eq!(w.parent(a), None);
        assert_eq!(w.children(a), vec![b]);
    }

    #[test]
    fn reparent_to_dead_entity_keeps_old_link() {
        use crate::reflect::ReflectError;
        let mut w = World::new();
        let a = w.spawn((1u8,));
        let b = w.spawn((2u8,));
        let dead = w.spawn((3u8,));
        w.despawn(dead);
        w.set_parent(b, Some(a)).unwrap();
        assert!(matches!(
            w.set_parent(b, Some(dead)),
            Err(ReflectError::NoEntity(e)) if e == dead
        ));
        assert_eq!(w.parent(b), Some(a));
        assert_eq!(w.children(a), vec![b]);
        assert!(matches!(
            w.set_parent(dead, None),
            Err(ReflectError::NoEntity(_))
        ));
    }

    #[test]
    fn uid_index_and_root_order() {
        use crate::identity::{Name, Uid};
        let mut w = World::new();
        let (u1, u2, u3) = (Uid::from_u128(1), Uid::from_u128(2), Uid::from_u128(3));
        let a = w.spawn_saved(u1, "a", ()).unwrap();
        let b = w.spawn((u2, Name::new("b")));
        let c = w.spawn((7u8,));
        assert!(w.spawn_saved(u1, "dup", ()).is_err());
        assert_eq!(w.entity_by_uid(u1), Some(a));
        assert_eq!(w.entity_by_uid(u2), Some(b));
        // Inserting a Uid later makes the entity saved.
        w.insert_one(c, u3).unwrap();
        assert_eq!(w.entity_by_uid(u3), Some(c));
        assert_eq!(w.roots(), &[a, b, c]);
        // Parenting removes from the roots; unparenting appends.
        w.set_parent(a, Some(c)).unwrap();
        assert_eq!(w.roots(), &[b, c]);
        w.set_parent(a, None).unwrap();
        assert_eq!(w.roots(), &[b, c, a]);
        // Root reordering through place.
        w.place(
            a,
            Placement {
                parent: None,
                index: 0,
            },
        )
        .unwrap();
        assert_eq!(w.roots(), &[a, b, c]);
        assert_eq!(
            w.placement(b),
            Placement {
                parent: None,
                index: 1
            }
        );
        // Replacing and removing a Uid keeps the index exact.
        let u4 = Uid::from_u128(4);
        w.insert(b, (u4,)).unwrap();
        assert_eq!(w.entity_by_uid(u2), None);
        assert_eq!(w.entity_by_uid(u4), Some(b));
        w.remove_one::<Uid>(b).unwrap();
        assert_eq!(w.entity_by_uid(u4), None);
        assert_eq!(w.roots(), &[a, c]);
        // Despawning a subtree drops every uid in it.
        w.place(a, Placement::last(Some(u3))).unwrap();
        w.despawn(c);
        assert_eq!(w.entity_by_uid(u1), None);
        assert_eq!(w.entity_by_uid(u3), None);
        assert!(w.roots().is_empty());
    }

    #[test]
    fn place_rejects_cycles_and_restores_sibling_index() {
        use crate::identity::Uid;
        use crate::reflect::ReflectError;
        let mut w = World::new();
        let u = |i| Uid::from_u128(i);
        let p = w.spawn_saved(u(1), "p", ()).unwrap();
        let kids: Vec<Entity> = (2..5)
            .map(|i| {
                let e = w.spawn_saved(u(i), "k", ()).unwrap();
                w.set_parent(e, Some(p)).unwrap();
                e
            })
            .collect();
        w.place(
            kids[2],
            Placement {
                parent: Some(u(1)),
                index: 0,
            },
        )
        .unwrap();
        assert_eq!(w.children(p), vec![kids[2], kids[0], kids[1]]);
        assert!(matches!(
            w.place(p, Placement::last(Some(u(3)))),
            Err(ReflectError::Cycle)
        ));
        assert!(matches!(
            w.place(p, Placement::last(Some(u(99)))),
            Err(ReflectError::NoUid(_))
        ));
        assert_eq!(w.parent(p), None);
    }
}
