//! `App::on_add` / `App::on_remove`: callbacks for entities that gained or
//! lost a component, found without scanning every entity.
//!
//! Each hooked type `T` has a runtime marker `Seen<T>` (not registered, so
//! never saved, copied into Play or recorded by undo). An entity with `T`
//! and no marker is new: it gets the marker and the `on_add` callbacks run.
//! An entity with the marker and no `T` lost it while alive: the marker goes
//! and the `on_remove` callbacks run. Despawns are recorded by
//! [`World::despawn`] for hooked types. The queries filter whole archetypes,
//! so a pass costs O(archetypes) plus O(new or removed entities), never
//! O(entities).

use std::any::{Any, TypeId};
use std::marker::PhantomData;

use crate::schedule::run_named;
use crate::world::{Component, Entity, World};

/// Marks entities whose `T` the hooks have seen.
struct Seen<T>(PhantomData<fn() -> T>);

pub(crate) type HookFn = Box<dyn FnMut(&mut World, Entity)>;

type Callback = (&'static str, HookFn);

/// The hooks of one component type.
struct TypeHooks<T> {
    on_add: Vec<Callback>,
    on_remove: Vec<Callback>,
    _marker: PhantomData<fn() -> T>,
}

pub(crate) trait HookSet {
    /// One pass. True if any callback ran.
    fn run(&mut self, world: &mut World) -> bool;
    fn as_any(&mut self) -> &mut dyn Any;
}

impl<T: Component> HookSet for TypeHooks<T> {
    fn run(&mut self, world: &mut World) -> bool {
        let added: Vec<Entity> = world
            .query::<Entity>()
            .with::<&T>()
            .without::<&Seen<T>>()
            .iter()
            .collect();
        let removed: Vec<Entity> = world
            .query::<Entity>()
            .with::<&Seen<T>>()
            .without::<&T>()
            .iter()
            .collect();
        let despawned = if self.on_remove.is_empty() {
            Vec::new()
        } else {
            world.watch_despawns::<T>();
            world.take_despawned::<T>()
        };
        if added.is_empty() && removed.is_empty() && despawned.is_empty() {
            return false;
        }
        for &e in &added {
            let _ = world.ecs_mut().insert_one(e, Seen::<T>(PhantomData));
        }
        for &e in &removed {
            let _ = world.ecs_mut().remove_one::<Seen<T>>(e);
        }
        let mut ran = false;
        for e in added {
            for (name, f) in &mut self.on_add {
                // An earlier callback may have removed it again.
                if world.has::<T>(e) {
                    run_named(name, || f(world, e));
                    world.apply_deferred();
                    ran = true;
                }
            }
        }
        for e in removed.into_iter().chain(despawned) {
            for (name, f) in &mut self.on_remove {
                run_named(name, || f(world, e));
                world.apply_deferred();
                ran = true;
            }
        }
        ran
    }

    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

/// Every hooked type's hooks, in the order the types were first hooked.
#[derive(Default)]
pub(crate) struct Hooks {
    sets: Vec<(TypeId, Box<dyn HookSet>)>,
}

impl Hooks {
    fn of<T: Component>(&mut self) -> &mut TypeHooks<T> {
        let ty = TypeId::of::<T>();
        let i = match self.sets.iter().position(|(t, _)| *t == ty) {
            Some(i) => i,
            None => {
                self.sets.push((
                    ty,
                    Box::new(TypeHooks::<T> {
                        on_add: Vec::new(),
                        on_remove: Vec::new(),
                        _marker: PhantomData,
                    }),
                ));
                self.sets.len() - 1
            }
        };
        self.sets[i]
            .1
            .as_any()
            .downcast_mut()
            .expect("hook set keyed by its type")
    }

    pub(crate) fn add<T: Component>(&mut self, name: &'static str, f: HookFn) {
        self.of::<T>().on_add.push((name, f));
    }

    pub(crate) fn remove<T: Component>(
        &mut self,
        world: &mut World,
        name: &'static str,
        f: HookFn,
    ) {
        world.watch_despawns::<T>();
        self.of::<T>().on_remove.push((name, f));
    }

    /// Run passes until no callback runs (callbacks may add hooked
    /// components), at most a few times per call.
    pub(crate) fn run(&mut self, world: &mut World) {
        if self.sets.is_empty() {
            return;
        }
        profiling::scope!("hooks");
        for _ in 0..8 {
            let mut ran = false;
            for (_, set) in &mut self.sets {
                ran |= set.run(world);
            }
            if !ran {
                break;
            }
        }
    }
}
