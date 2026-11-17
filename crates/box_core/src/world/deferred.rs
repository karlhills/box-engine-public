//! Deferred structural changes: [`World::defer`] and [`Deferred`].

use std::cell::RefCell;

use super::{Component, DynamicBundle, Entity, World};

type Command = Box<dyn FnOnce(&mut World)>;

/// The queue behind [`World::defer`].
#[derive(Default)]
pub(super) struct Queue(RefCell<Vec<Command>>);

impl Queue {
    fn push(&self, command: Command) {
        self.0.borrow_mut().push(command);
    }

    fn take(&self) -> Vec<Command> {
        std::mem::take(&mut *self.0.borrow_mut())
    }

    fn is_empty(&self) -> bool {
        self.0.borrow().is_empty()
    }

    fn len(&self) -> usize {
        self.0.borrow().len()
    }
}

/// Structural changes queued through a shared `&World`, so they can be made
/// while a query is being iterated: `world.defer().despawn(e)`.
///
/// Commands apply in the order they were queued, through the same `World`
/// methods as direct calls (`spawn`, `insert`, `remove_one`, `despawn`,
/// `set_parent`), so the uid index, the root order and the hierarchy stay
/// correct. [`App`](crate::app::App) applies them after every system (and
/// after every `on_add`/`on_remove`/`on_event` callback); elsewhere call
/// [`World::apply_deferred`].
///
/// ```
/// use box_core::prelude::*;
///
/// struct Health(i32);
/// let mut world = World::new();
/// let a = world.spawn((Health(0), Transform::default()));
/// let b = world.spawn((Health(5), Transform::default()));
/// for (e, h) in world.query::<(Entity, &Health)>().iter() {
///     if h.0 <= 0 {
///         world.defer().despawn(e);
///         // Spawned entities have their `Entity` at once.
///         let corpse = world.defer().spawn((Transform::default(),));
///         world.defer().set_parent(corpse, Some(b));
///     }
/// }
/// world.apply_deferred();
/// assert!(!world.contains(a));
/// assert_eq!(world.children(b).len(), 1);
/// ```
#[derive(Clone, Copy)]
pub struct Deferred<'w> {
    world: &'w World,
}

impl<'w> Deferred<'w> {
    /// Spawn an entity with `components` when applied (a `Uid` among them is
    /// indexed, as with [`World::spawn`]). The returned `Entity` is valid at
    /// once: pass it to other deferred commands. Until applied it has no
    /// components.
    pub fn spawn(&self, components: impl DynamicBundle + 'static) -> Entity {
        let e = self.world.ecs.reserve_entity();
        self.world.deferred.push(Box::new(move |w: &mut World| {
            if w.insert(e, components).is_err() {
                tracing::debug!("deferred spawn of {e:?}: the entity was despawned first");
            }
        }));
        e
    }

    /// Add or replace components ([`World::insert`]). Skipped if the entity
    /// is gone by then.
    pub fn insert(&self, entity: Entity, components: impl DynamicBundle + 'static) -> &Self {
        self.run(move |w| {
            let _ = w.insert(entity, components);
        })
    }

    /// Add or replace one component ([`World::insert_one`]).
    pub fn insert_one<T: Component>(&self, entity: Entity, component: T) -> &Self {
        self.run(move |w| {
            let _ = w.insert_one(entity, component);
        })
    }

    /// Remove a component ([`World::remove_one`]); nothing if it is missing.
    pub fn remove<T: Component>(&self, entity: Entity) -> &Self {
        self.run(move |w| {
            let _ = w.remove_one::<T>(entity);
        })
    }

    /// Despawn an entity and its descendants ([`World::despawn`]); nothing if
    /// it is already gone.
    pub fn despawn(&self, entity: Entity) -> &Self {
        self.run(move |w| {
            w.despawn(entity);
        })
    }

    /// Set or clear a parent ([`World::set_parent`]). A missing entity or a
    /// cycle is logged as a warning and changes nothing.
    pub fn set_parent(&self, child: Entity, parent: Option<Entity>) -> &Self {
        self.run(move |w| {
            if let Err(e) = w.set_parent(child, parent) {
                tracing::warn!("deferred set_parent({child:?}, {parent:?}): {e}");
            }
        })
    }

    /// Run any change on the world when the queue is applied.
    pub fn run(&self, f: impl FnOnce(&mut World) + 'static) -> &Self {
        self.world.deferred.push(Box::new(f));
        self
    }
}

impl World {
    /// Queue structural changes (spawn, insert, remove, despawn, reparent)
    /// through `&self`, usable while queries are alive. See [`Deferred`].
    pub fn defer(&self) -> Deferred<'_> {
        Deferred { world: self }
    }

    /// Commands queued with [`World::defer`] and not applied yet.
    pub fn deferred_len(&self) -> usize {
        self.deferred.len()
    }

    /// Apply every command queued with [`World::defer`], in order, including
    /// commands queued while applying. Returns how many ran. Cheap when the
    /// queue is empty; the app calls it after every system.
    pub fn apply_deferred(&mut self) -> usize {
        let mut ran = 0;
        // Bounded so a command that always queues another cannot hang.
        for _ in 0..64 {
            if self.deferred.is_empty() {
                break;
            }
            for command in self.deferred.take() {
                command(self);
                ran += 1;
            }
        }
        if !self.deferred.is_empty() {
            tracing::warn!(
                "deferred commands keep queueing more; {} left for the next apply",
                self.deferred.len()
            );
        }
        ran
    }
}

#[cfg(test)]
mod tests {
    use crate::identity::{Name, Uid};
    use crate::transform::Transform;
    use crate::world::World;

    struct Hp(i32);

    #[test]
    fn spawn_insert_remove_and_despawn_while_iterating() {
        let mut w = World::new();
        let es: Vec<_> = (0..10).map(|i| w.spawn((Hp(i),))).collect();
        let mut spawned = Vec::new();
        for (e, hp) in w.query::<(hecs::Entity, &Hp)>().iter() {
            match hp.0 % 3 {
                0 => {
                    w.defer().despawn(e);
                }
                1 => {
                    w.defer().insert_one(e, Transform::from_xyz(1.0, 0.0, 0.0));
                }
                _ => {
                    w.defer().remove::<Hp>(e);
                    spawned.push(w.defer().spawn((Hp(100),)));
                }
            }
        }
        assert_eq!(w.deferred_len(), 13);
        assert_eq!(w.apply_deferred(), 13);
        assert_eq!(w.deferred_len(), 0);
        for (i, e) in es.iter().enumerate() {
            match i % 3 {
                0 => assert!(!w.contains(*e)),
                1 => assert!(w.has::<Transform>(*e) && w.has::<Hp>(*e)),
                _ => assert!(w.contains(*e) && !w.has::<Hp>(*e)),
            }
        }
        for e in spawned {
            assert_eq!(w.get::<Hp>(e).unwrap().0, 100);
        }
        assert_eq!(w.count::<&Hp>(), 3 + 3);
    }

    #[test]
    fn uids_roots_and_hierarchy_stay_consistent() {
        let mut w = World::new();
        let (u1, u2, u3) = (Uid::from_u128(1), Uid::from_u128(2), Uid::from_u128(3));
        let root = w.spawn_saved(u1, "root", ()).unwrap();
        let (parent, child, dup);
        {
            let d = w.defer();
            parent = d.spawn((u2, Name::new("parent"), Transform::default()));
            child = d.spawn((u3, Name::new("child")));
            // A uid already in use: the new entity gets a fresh one.
            dup = d.spawn((u1, Name::new("dup")));
            d.set_parent(child, Some(parent));
            d.set_parent(parent, Some(root));
        }
        // Not applied yet: not indexed.
        assert_eq!(w.entity_by_uid(u2), None);
        w.apply_deferred();
        assert_eq!(w.entity_by_uid(u2), Some(parent));
        assert_eq!(w.entity_by_uid(u3), Some(child));
        assert_eq!(w.entity_by_uid(u1), Some(root));
        assert_ne!(w.uid(dup), Some(u1));
        assert!(w.uid(dup).is_some());
        assert_eq!(w.roots(), &[root, dup]);
        assert_eq!(w.children(root), vec![parent]);
        assert_eq!(w.children(parent), vec![child]);

        // A cycle is refused (and logged), and changes nothing.
        w.defer().set_parent(root, Some(child));
        w.apply_deferred();
        assert_eq!(w.parent(root), None);

        // Despawning the root while iterating takes the subtree and its uids.
        for (e, n) in w.query::<(hecs::Entity, &Name)>().iter() {
            if n.as_str() == "root" {
                w.defer().despawn(e);
            }
        }
        w.apply_deferred();
        for (u, e) in [(u1, root), (u2, parent), (u3, child)] {
            assert_eq!(w.entity_by_uid(u), None);
            assert!(!w.contains(e));
        }
        assert_eq!(w.roots(), &[dup]);
    }

    #[test]
    fn commands_run_in_order_and_may_queue_more() {
        let mut w = World::new();
        w.insert_resource(Vec::<u32>::new());
        let e = w.defer().spawn((Hp(1),));
        w.defer()
            .despawn(e)
            .insert_one(e, Hp(2)) // gone by then: skipped
            .run(|w| {
                w.resource_mut::<Vec<u32>>().push(1);
                w.defer().run(|w| w.resource_mut::<Vec<u32>>().push(2));
            });
        // spawn, despawn, insert_one, run, and the run it queued.
        assert_eq!(w.apply_deferred(), 5);
        assert!(!w.contains(e));
        assert_eq!(*w.resource::<Vec<u32>>(), vec![1, 2]);
        assert_eq!(w.apply_deferred(), 0);
    }
}
