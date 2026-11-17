//! Component registry: bevy_reflect's `TypeRegistry` plus a stable-name index and
//! the `ReflectComponent` type data that bridges reflection and hecs storage.

use std::any::TypeId;
use std::collections::BTreeMap;
use std::ops::Deref;

use bevy_reflect::{
    CreateTypeData, FromReflect, GetTypeRegistration, PartialReflect, Reflect, TypePath,
    TypeRegistration, TypeRegistry,
};
use box_core::world::{Component, Entity, Ref, World};

use crate::Error;

/// A shared borrow of one component, type-erased.
pub type ComponentRef<'w> = Box<dyn Deref<Target = dyn Reflect> + 'w>;

struct Guard<'w, T: Component>(Ref<'w, T>);

impl<T: Component + Reflect> Deref for Guard<'_, T> {
    type Target = dyn Reflect;
    fn deref(&self) -> &dyn Reflect {
        &*self.0
    }
}

/// Type data inserted by `#[reflect(Component)]`. Function pointers are
/// monomorphised once per component type; callers never see the concrete type.
#[derive(Clone)]
pub struct ReflectComponent {
    pub contains: fn(&World, Entity) -> bool,
    pub get: for<'w> fn(&'w World, Entity) -> Option<ComponentRef<'w>>,
    pub get_mut: for<'w> fn(&'w mut World, Entity) -> Option<&'w mut dyn Reflect>,
    /// Insert (or replace) from any reflected value with the right shape,
    /// including dynamic values produced by the deserializer.
    pub insert: fn(&mut World, Entity, &dyn PartialReflect) -> Result<(), Error>,
    pub remove: fn(&mut World, Entity) -> Option<Box<dyn Reflect>>,
}

impl<T> CreateTypeData<T> for ReflectComponent
where
    T: Component + Reflect + FromReflect + TypePath,
{
    fn create_type_data(_: ()) -> Self {
        Self {
            contains: |w, e| w.has::<T>(e),
            get: |w, e| {
                w.get::<T>(e)
                    .map(|r| Box::new(Guard(r)) as ComponentRef<'_>)
            },
            get_mut: |w, e| {
                w.ecs_mut()
                    .query_one_mut::<&mut T>(e)
                    .ok()
                    .map(|t| t as &mut dyn Reflect)
            },
            insert: |w, e, value| {
                let v = T::from_reflect(value).ok_or_else(|| Error::Shape {
                    expected: T::type_path().to_string(),
                    got: value.reflect_type_path().to_string(),
                })?;
                w.insert_one(e, v).map_err(|_| Error::NoEntity)
            },
            remove: |w, e| {
                w.remove_one::<T>(e)
                    .ok()
                    .map(|t| Box::new(t) as Box<dyn Reflect>)
            },
        }
    }
}

/// One registry for the inspector, scenes, undo and (later) scripting.
#[derive(Default)]
pub struct Registry {
    pub types: TypeRegistry,
    /// Stable scene name (the short type path) to type id. A `BTreeMap` so that
    /// listing components is ordered without extra work.
    by_name: BTreeMap<&'static str, TypeId>,
}

/// A registered component type.
#[derive(Clone, Copy)]
pub struct ComponentType<'r> {
    pub name: &'static str,
    pub registration: &'r TypeRegistration,
    pub data: &'r ReflectComponent,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a component. The stable name is its short type path, so
    /// `#[type_path = "game"] struct Health` is `"Health"` in scene files and
    /// paths. Two components with the same short name are rejected.
    pub fn register_component<T>(&mut self) -> Result<(), Error>
    where
        T: GetTypeRegistration + TypePath + Component,
    {
        let name = T::short_type_path();
        if let Some(&existing) = self.by_name.get(name)
            && existing != TypeId::of::<T>()
        {
            return Err(Error::DuplicateName(name.to_string()));
        }
        self.types.register::<T>();
        if self
            .types
            .get_type_data::<ReflectComponent>(TypeId::of::<T>())
            .is_none()
        {
            return Err(Error::NotAComponent(T::type_path().to_string()));
        }
        self.by_name.insert(name, TypeId::of::<T>());
        Ok(())
    }

    pub fn component(&self, name: &str) -> Option<ComponentType<'_>> {
        let id = *self.by_name.get(name)?;
        let registration = self.types.get(id)?;
        let data = registration.data::<ReflectComponent>()?;
        Some(ComponentType {
            name: registration.type_info().type_path_table().short_path(),
            registration,
            data,
        })
    }

    pub fn registration(&self, name: &str) -> Option<&TypeRegistration> {
        self.types.get(*self.by_name.get(name)?)
    }

    /// All registered components, ordered by name.
    pub fn components(&self) -> impl Iterator<Item = ComponentType<'_>> {
        self.by_name.keys().filter_map(|n| self.component(n))
    }
}
