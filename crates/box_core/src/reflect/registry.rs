//! The type registry: bevy's `TypeRegistry` plus a stable short-name index and
//! the type data that bridges reflection to hecs components and world resources.

use std::any::TypeId;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Deref;

use bevy_reflect::serde::{TypedReflectDeserializer, TypedReflectSerializer};
use bevy_reflect::{
    CreateTypeData, FromReflect, GetTypeRegistration, PartialReflect, Reflect, TypeData, TypeInfo,
    TypePath, TypeRegistration, TypeRegistry,
};
use serde::de::DeserializeSeed;

use super::meta::{self, FieldMeta};
use super::schema::SchemaType;
use super::{Hidden, NotSaved, ReflectDefault, ReflectError, Value};
use crate::world::{Component, Entity, Ref, World};

/// A shared borrow of one component, type-erased.
pub type ComponentRef<'w> = Box<dyn Deref<Target = dyn Reflect> + 'w>;

struct Guard<'w, T: Component>(Ref<'w, T>);

impl<T: Component + Reflect> Deref for Guard<'_, T> {
    type Target = dyn Reflect;
    fn deref(&self) -> &dyn Reflect {
        &*self.0
    }
}

/// Type data inserted by `#[reflect(Component)]`: monomorphised accessors into
/// the world's hecs storage, so callers never need the concrete type.
#[derive(Clone)]
pub struct ReflectComponent {
    pub(crate) contains: fn(&World, Entity) -> bool,
    /// Every entity with the component, in storage order (a typed query).
    pub(crate) entities: fn(&World) -> Vec<Entity>,
    pub(crate) get: for<'w> fn(&'w World, Entity) -> Option<ComponentRef<'w>>,
    pub(crate) get_mut: for<'w> fn(&'w mut World, Entity) -> Option<&'w mut dyn Reflect>,
    /// Insert (or overwrite) from any value of the right shape, including dynamic ones.
    pub(crate) insert: fn(&mut World, Entity, &dyn PartialReflect) -> Result<(), ReflectError>,
    pub(crate) remove: fn(&mut World, Entity) -> Option<Box<dyn Reflect>>,
}

impl<T> CreateTypeData<T> for ReflectComponent
where
    T: Component + Reflect + FromReflect + TypePath,
{
    fn create_type_data(_: ()) -> Self {
        Self {
            contains: |w, e| w.has::<T>(e),
            entities: |w| w.query::<Entity>().with::<&T>().iter().collect(),
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
                let v = T::from_reflect(value).ok_or_else(|| ReflectError::Shape {
                    expected: T::type_path().to_string(),
                    got: value.reflect_type_path().to_string(),
                })?;
                w.insert_one(e, v).map_err(|_| ReflectError::NoEntity(e))
            },
            remove: |w, e| {
                w.remove_one::<T>(e)
                    .ok()
                    .map(|t| Box::new(t) as Box<dyn Reflect>)
            },
        }
    }
}

/// Type data inserted by `#[reflect(Resource)]`: accessors into the world's
/// resource map.
#[derive(Clone)]
pub struct ReflectResource {
    pub(crate) get: fn(&World) -> Option<&dyn Reflect>,
    pub(crate) get_mut: fn(&mut World) -> Option<&mut dyn Reflect>,
    /// Insert (or overwrite) from any value of the right shape.
    pub(crate) insert: fn(&mut World, &dyn PartialReflect) -> Result<(), ReflectError>,
    pub(crate) remove: fn(&mut World) -> Option<Box<dyn Reflect>>,
}

impl<T> CreateTypeData<T> for ReflectResource
where
    T: Reflect + FromReflect + TypePath,
{
    fn create_type_data(_: ()) -> Self {
        Self {
            get: |w| w.get_resource::<T>().map(|r| r as &dyn Reflect),
            get_mut: |w| w.get_resource_mut::<T>().map(|r| r as &mut dyn Reflect),
            insert: |w, value| {
                let v = T::from_reflect(value).ok_or_else(|| ReflectError::Shape {
                    expected: T::type_path().to_string(),
                    got: value.reflect_type_path().to_string(),
                })?;
                w.insert_resource(v);
                Ok(())
            },
            remove: |w| {
                w.remove_resource::<T>()
                    .map(|r| Box::new(r) as Box<dyn Reflect>)
            },
        }
    }
}

/// Type data attached by [`Registry::register_event`]: send and read an
/// event type's [`Events`](crate::event::Events) queue without its concrete
/// type (scripts, tools).
#[derive(Clone)]
pub struct ReflectEvent {
    pub(crate) send: fn(&mut World, &dyn PartialReflect) -> Result<(), ReflectError>,
    pub(crate) read: fn(&World, &mut u64) -> Vec<Box<dyn Reflect>>,
}

impl ReflectEvent {
    fn new<T>() -> Self
    where
        T: Reflect + FromReflect + TypePath + Clone,
    {
        Self {
            send: |w, value| {
                let v = T::from_reflect(value).ok_or_else(|| ReflectError::Shape {
                    expected: T::type_path().to_string(),
                    got: value.reflect_type_path().to_string(),
                })?;
                match w.get_resource_mut::<crate::event::Events<T>>() {
                    Some(events) => {
                        events.send(v);
                        Ok(())
                    }
                    None => Err(ReflectError::MissingResource(format!(
                        "Events<{}>",
                        T::short_type_path()
                    ))),
                }
            },
            read: |w, cursor| match w.get_resource::<crate::event::Events<T>>() {
                Some(events) => events
                    .read_from(cursor)
                    .map(|e| Box::new(e.clone()) as Box<dyn Reflect>)
                    .collect(),
                None => Vec::new(),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Component,
    Resource,
}

#[derive(Clone, Copy)]
struct Entry {
    type_id: TypeId,
    kind: Kind,
}

/// One registry for the inspector, scene files, undo, the CLI and (later)
/// scripting. Shared by edit and play worlds through an `Arc` held by each
/// [`World`].
///
/// Component and resource types are addressed by their **short name** (the
/// last segment of the type path), which is what scene files and field paths
/// use. Short names are unique across components and resources.
pub struct Registry {
    types: TypeRegistry,
    by_name: BTreeMap<&'static str, Entry>,
    /// Failed registrations not yet taken ([`Registry::take_errors`]).
    errors: Vec<String>,
    /// Resource types registered with [`Registry::register_settings`].
    settings: BTreeSet<&'static str>,
    /// Schema-only types ([`Registry::add_schema_types`]), by name.
    schemas: BTreeMap<String, SchemaType>,
    /// Event types ([`Registry::register_event`]), by short name.
    events: BTreeMap<&'static str, TypeId>,
    /// Stored-data migrations by type name ([`Registry::add_migration`]).
    migrations: crate::migrate::TypeMigrations,
}

/// A tuple of component types for [`Registry::register_all`]:
/// `(Coin,)`, `(Coin, Player, Door)`, up to 16 types.
pub trait ComponentTypes {
    /// Register every type in the tuple, returning the errors.
    fn register_into(registry: &mut Registry) -> Vec<ReflectError>;
}

macro_rules! component_tuple {
    ($($t:ident),+) => {
        impl<$($t),+> ComponentTypes for ($($t,)+)
        where
            $($t: Component + GetTypeRegistration + TypePath,)+
        {
            fn register_into(registry: &mut Registry) -> Vec<ReflectError> {
                [$(registry.register::<$t>()),+]
                    .into_iter()
                    .filter_map(Result::err)
                    .collect()
            }
        }
    };
}

component_tuple!(A);
component_tuple!(A, B);
component_tuple!(A, B, C);
component_tuple!(A, B, C, D);
component_tuple!(A, B, C, D, E);
component_tuple!(A, B, C, D, E, F);
component_tuple!(A, B, C, D, E, F, G);
component_tuple!(A, B, C, D, E, F, G, H);
component_tuple!(A, B, C, D, E, F, G, H, I);
component_tuple!(A, B, C, D, E, F, G, H, I, J);
component_tuple!(A, B, C, D, E, F, G, H, I, J, K);
component_tuple!(A, B, C, D, E, F, G, H, I, J, K, L);
component_tuple!(A, B, C, D, E, F, G, H, I, J, K, L, M);
component_tuple!(A, B, C, D, E, F, G, H, I, J, K, L, M, N);
component_tuple!(A, B, C, D, E, F, G, H, I, J, K, L, M, N, O);
component_tuple!(A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P);

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry")
            .field("names", &self.by_name.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Registry {
    /// An empty registry (primitive value types only).
    pub fn new() -> Self {
        Self {
            types: TypeRegistry::new(),
            by_name: BTreeMap::new(),
            errors: Vec::new(),
            settings: BTreeSet::new(),
            schemas: BTreeMap::new(),
            events: BTreeMap::new(),
            migrations: Default::default(),
        }
    }

    /// Adds the next migration step of the stored data of type `name` (a
    /// component, resource or file type, registered here or not): the first
    /// call upgrades version 1 to 2, the next 2 to 3, and so on, so the
    /// type's version is the number of steps plus one. Values of a type
    /// above version 1 are written with a `"$version"` key. Call it in
    /// `Plugin::register` next to the type's registration, in version order,
    /// and never remove or reorder steps that shipped. See
    /// [`crate::migrate`].
    pub fn add_migration(&mut self, name: &str, step: crate::migrate::Step) -> &mut Self {
        self.migrations.add(name, step);
        self
    }

    /// Records that the type stored as `old` is now called `new`: files are
    /// read with the new name (scene keys, resource keys) and written with
    /// it. A type keeps its version and its earlier steps through a rename;
    /// register them under the new name.
    pub fn rename_type(&mut self, old: &str, new: &str) -> &mut Self {
        self.migrations.rename(old, new);
        self
    }

    /// The type chains and renames ([`add_migration`](Self::add_migration),
    /// [`rename_type`](Self::rename_type)).
    pub fn migrations(&self) -> &crate::migrate::TypeMigrations {
        &self.migrations
    }

    /// The version this registry writes for type `name`: its number of
    /// migration steps plus one, or, for a schema-only type, the version its
    /// registry dump records. 1 for any other type.
    pub fn type_version(&self, name: &str) -> u32 {
        if let Some(chain) = self.migrations.chain(name) {
            return chain.current();
        }
        self.schemas.get(name).map_or(1, SchemaType::version)
    }

    /// A registry with box_core's own scene types: see [`crate::register_core_types`].
    pub fn with_core_types() -> Self {
        let mut r = Self::new();
        crate::register_core_types(&mut r);
        r
    }

    /// Register a component type (it must have `#[reflect(Component)]`). Field
    /// types are registered too. Registering the same type twice is a no-op; a
    /// different type with the same short name is rejected.
    pub fn register<T>(&mut self) -> Result<(), ReflectError>
    where
        T: Component + GetTypeRegistration + TypePath,
    {
        self.register_kind::<T>(Kind::Component)
    }

    /// Register a resource type (it must have `#[reflect(Resource)]`). Resources
    /// are saved in the scene's `"resources"` object unless marked `@NotSaved`.
    pub fn register_resource<T>(&mut self) -> Result<(), ReflectError>
    where
        T: GetTypeRegistration + TypePath + Send + Sync + 'static,
    {
        self.register_kind::<T>(Kind::Resource)
    }

    /// Register a project settings resource (it must have `#[reflect(Resource)]`
    /// and `#[reflect(Default)]`): a resource stored in its own file,
    /// `assets/settings/<TypeName>.json`, instead of in scene files. The
    /// player, the editor and Play load it into every world at startup and
    /// hot-reload it when the file changes (`box_engine::settings`); the
    /// editor shows it under Game ▸ Project Settings.
    pub fn register_settings<T>(&mut self) -> Result<(), ReflectError>
    where
        T: GetTypeRegistration + TypePath + Send + Sync + 'static,
    {
        self.register_kind::<T>(Kind::Resource)?;
        if self
            .types
            .get_type_data::<ReflectDefault>(TypeId::of::<T>())
            .is_none()
        {
            let name = T::short_type_path();
            self.by_name.remove(name);
            let e = ReflectError::NoDefault(T::type_path().to_string());
            self.errors.push(e.to_string());
            return Err(e);
        }
        self.settings.insert(T::short_type_path());
        Ok(())
    }

    /// Register an event type (`#[derive(Reflect, Clone)]`, plus
    /// `#[reflect(Default)]` so a partial value can be sent): scripts and
    /// tools can then send and read it by short name ([`EventType::send`],
    /// [`EventType::read`]). The app still needs the queue
    /// (`app.add_event::<T>()`). Event names share the short-name space with
    /// components and resources; registering twice is a no-op.
    pub fn register_event<T>(&mut self) -> Result<(), ReflectError>
    where
        T: Reflect + FromReflect + GetTypeRegistration + TypePath + Clone + Send + Sync,
    {
        let name = T::short_type_path();
        let type_id = TypeId::of::<T>();
        let clash = match (self.events.get(name), self.by_name.get(name)) {
            (Some(existing), _) if *existing == type_id => return Ok(()),
            (Some(existing), _) => Some(*existing),
            (None, Some(entry)) => Some(entry.type_id),
            (None, None) => None,
        };
        if let Some(existing) = clash {
            let existing = self
                .types
                .get(existing)
                .map_or("<unknown>", |r| r.type_info().type_path());
            let e = ReflectError::DuplicateName {
                name: name.to_string(),
                existing,
                new: T::type_path(),
            };
            self.errors.push(e.to_string());
            return Err(e);
        }
        self.types.register::<T>();
        if let Some(registration) = self.types.get_mut(type_id) {
            registration.insert(ReflectEvent::new::<T>());
        }
        self.events.insert(name, type_id);
        Ok(())
    }

    /// An event type by short name ([`register_event`](Self::register_event)).
    pub fn event(&self, name: &str) -> Option<EventType<'_>> {
        let type_id = *self.events.get(name)?;
        let registration = self.types.get(type_id)?;
        Some(EventType {
            name: registration.type_info().type_path_table().short_path(),
            registration,
            data: registration.data::<ReflectEvent>()?,
        })
    }

    /// All event types, ordered by name.
    pub fn events(&self) -> impl Iterator<Item = EventType<'_>> {
        self.events.keys().filter_map(|n| self.event(n))
    }

    /// Register a plain value type (not a component or resource) so it can be
    /// serialized and deserialized on its own, for example as a command value.
    /// Field types of registered components are registered automatically; this
    /// is for types that may be used before any component referencing them is
    /// registered. It gets no short name.
    pub fn register_value<T: GetTypeRegistration>(&mut self) {
        self.types.register::<T>();
    }

    /// Register several component types at once:
    /// `registry.register_all::<(Coin, Player, Door)>()`. Every type is
    /// attempted; the errors (a short name clash, a missing
    /// `#[reflect(Component)]`) are returned and also kept for
    /// [`take_errors`](Self::take_errors), which `box_engine::Game` reads to
    /// log them with the plugin's name. So in [`Plugin::register`] the result
    /// can be ignored.
    ///
    /// [`Plugin::register`]: crate::app::Plugin::register
    pub fn register_all<T: ComponentTypes>(&mut self) -> Vec<ReflectError> {
        T::register_into(self)
    }

    /// Messages of the registrations that failed since the last call, oldest
    /// first. Failures of [`register`](Self::register),
    /// [`register_resource`](Self::register_resource) and
    /// [`register_all`](Self::register_all) are all kept here, even when the
    /// caller ignored the returned error.
    pub fn take_errors(&mut self) -> Vec<String> {
        std::mem::take(&mut self.errors)
    }

    /// True if a component or resource type has this short name.
    pub fn contains(&self, name: &str) -> bool {
        self.by_name.contains_key(name)
    }

    /// Settings resource types ([`register_settings`](Self::register_settings)),
    /// ordered by name.
    pub fn settings(&self) -> impl Iterator<Item = ResourceType<'_>> {
        self.settings.iter().filter_map(|n| self.resource(n))
    }

    /// Adds schema-only types from a registry dump (the
    /// [`to_json`](Self::to_json) format, as `<project>/.box/registry.json`
    /// holds it): every component and resource whose name is not registered
    /// here becomes a [`SchemaType`]. Scene data of such a type stays raw JSON
    /// ([`crate::scene::UnknownComponents`]), but it is no longer unknown:
    /// edit commands validate it against the schema (field names, types,
    /// ranges) and store it raw, so tools without the game's Rust types can
    /// still edit its components. Returns the names added, sorted.
    pub fn add_schema_types(&mut self, dump: &serde_json::Value) -> Vec<String> {
        let mut added = Vec::new();
        for (list, resource) in [("components", false), ("resources", true)] {
            for entry in dump[list].as_array().into_iter().flatten() {
                let Some(name) = entry["name"].as_str() else {
                    continue;
                };
                if self.by_name.contains_key(name) || self.schemas.contains_key(name) {
                    continue;
                }
                self.schemas
                    .insert(name.to_string(), SchemaType::new(entry, resource));
                added.push(name.to_string());
            }
        }
        for (old, new) in dump["renames"].as_object().into_iter().flatten() {
            if let Some(new) = new.as_str() {
                self.migrations.rename(old, new);
            }
        }
        added.sort();
        added
    }

    /// The schema-only type named `name` ([`add_schema_types`](Self::add_schema_types)).
    pub fn schema(&self, name: &str) -> Option<&SchemaType> {
        self.schemas.get(name)
    }

    /// Every schema-only type, ordered by name.
    pub fn schemas(&self) -> impl Iterator<Item = &SchemaType> {
        self.schemas.values()
    }

    /// Short names of every registered component and resource type, sorted.
    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.by_name.keys().copied()
    }

    fn register_kind<T: GetTypeRegistration + TypePath + 'static>(
        &mut self,
        kind: Kind,
    ) -> Result<(), ReflectError> {
        let result = self.try_register_kind::<T>(kind);
        if let Err(e) = &result {
            self.errors.push(e.to_string());
        }
        result
    }

    fn try_register_kind<T: GetTypeRegistration + TypePath + 'static>(
        &mut self,
        kind: Kind,
    ) -> Result<(), ReflectError> {
        let name = T::short_type_path();
        let type_id = TypeId::of::<T>();
        if let Some(existing) = self.events.get(name) {
            let existing = self
                .types
                .get(*existing)
                .map_or("<unknown>", |r| r.type_info().type_path());
            return Err(ReflectError::DuplicateName {
                name: name.to_string(),
                existing,
                new: T::type_path(),
            });
        }
        if let Some(existing) = self.by_name.get(name) {
            if existing.type_id == type_id && existing.kind == kind {
                return Ok(());
            }
            let existing = self
                .types
                .get(existing.type_id)
                .map_or("<unknown>", |r| r.type_info().type_path());
            return Err(ReflectError::DuplicateName {
                name: name.to_string(),
                existing,
                new: T::type_path(),
            });
        }
        self.types.register::<T>();
        let ok = match kind {
            Kind::Component => self
                .types
                .get_type_data::<ReflectComponent>(type_id)
                .is_some(),
            Kind::Resource => self
                .types
                .get_type_data::<ReflectResource>(type_id)
                .is_some(),
        };
        if !ok {
            return Err(match kind {
                Kind::Component => ReflectError::NotAComponent(T::type_path().to_string()),
                Kind::Resource => ReflectError::NotAResource(T::type_path().to_string()),
            });
        }
        self.by_name.insert(name, Entry { type_id, kind });
        Ok(())
    }

    /// A component type by short name.
    pub fn component(&self, name: &str) -> Option<ComponentType<'_>> {
        let entry = self.by_name.get(name)?;
        (entry.kind == Kind::Component).then_some(())?;
        self.component_by_id(entry.type_id)
    }

    /// A registered component type by its Rust `TypeId`.
    pub fn component_by_id(&self, type_id: TypeId) -> Option<ComponentType<'_>> {
        let registration = self.types.get(type_id)?;
        let data = registration.data::<ReflectComponent>()?;
        let name = registration.type_info().type_path_table().short_path();
        // Only types registered through `register` (not mere field types that
        // happen to carry `#[reflect(Component)]`).
        (self.by_name.get(name)?.type_id == type_id).then_some(ComponentType {
            name,
            registration,
            data,
        })
    }

    /// All component types, ordered by name.
    pub fn components(&self) -> impl Iterator<Item = ComponentType<'_>> {
        self.by_name
            .iter()
            .filter(|(_, e)| e.kind == Kind::Component)
            .filter_map(|(_, e)| self.component_by_id(e.type_id))
    }

    /// A resource type by short name.
    pub fn resource(&self, name: &str) -> Option<ResourceType<'_>> {
        let entry = self.by_name.get(name)?;
        (entry.kind == Kind::Resource).then_some(())?;
        let registration = self.types.get(entry.type_id)?;
        let name = registration.type_info().type_path_table().short_path();
        Some(ResourceType {
            name,
            registration,
            data: registration.data::<ReflectResource>()?,
            settings: self.settings.contains(name),
        })
    }

    /// All resource types, ordered by name.
    pub fn resources(&self) -> impl Iterator<Item = ResourceType<'_>> {
        self.by_name
            .iter()
            .filter(|(_, e)| e.kind == Kind::Resource)
            .filter_map(|(n, _)| self.resource(n))
    }

    pub(crate) fn kind_of(&self, name: &str) -> Option<(&'static str, Kind)> {
        self.by_name.get_key_value(name).map(|(n, e)| (*n, e.kind))
    }

    pub(crate) fn info_of(&self, name: &str) -> Option<&'static TypeInfo> {
        let entry = self.by_name.get(name)?;
        Some(self.types.get(entry.type_id)?.type_info())
    }

    /// Attach type data `data` to the registered type `T` (a crate's own
    /// trait bridge, such as `box_proc`'s `ReflectGenerator`), replacing any
    /// earlier `D`. Read it back with [`ComponentType::data`]. Returns false
    /// (and attaches nothing) when `T` is not registered.
    pub fn insert_type_data<T: 'static, D: TypeData>(&mut self, data: D) -> bool {
        match self.types.get_mut(TypeId::of::<T>()) {
            Some(registration) => {
                registration.insert(data);
                true
            }
            None => false,
        }
    }

    /// The underlying bevy registry. An escape hatch: prefer the methods above.
    pub fn type_registry(&self) -> &TypeRegistry {
        &self.types
    }

    /// Metadata for a field path such as `"Transform.translation.x"` or
    /// `"Primitive.shape.radius"`. Nested fields inherit doc, range, unit and
    /// step from their parents when they have none of their own (so
    /// `translation.x` reports the unit of `translation`). Fields inside enum
    /// variants are looked up by name across variants.
    pub fn field_meta(&self, path: &super::FieldPath) -> Option<FieldMeta> {
        meta::lookup(self.info_of(path.target())?, path.parsed())
    }

    /// If `path` names a field that does not exist, an explanation listing
    /// the fields that do, with a did-you-mean:
    /// "`Coin` has no field `pionts`; fields: points, spin (did you mean `points`?)".
    pub fn explain_missing_field(&self, path: &super::FieldPath) -> Option<String> {
        meta::explain_missing_field(self.info_of(path.target())?, path.target(), path.parsed())
    }

    /// Fills the fields a JSON value leaves out of an enum variant it switches
    /// to (`{ "Sphere": { "radius": 0.5 } }`) with each field type's default,
    /// recursing through structs, newtypes, lists and variant payloads.
    /// Without this, building the value falls back to the type's default
    /// variant. Returns the paths of the fields it filled (`prefix.shape.sectors`);
    /// a missing field whose type has no default is an error.
    pub fn complete_variants(
        &self,
        type_id: TypeId,
        json: &mut serde_json::Value,
        prefix: &str,
    ) -> Result<Vec<String>, ReflectError> {
        let mut filled = Vec::new();
        self.complete_at(type_id, json, prefix, &mut filled)?;
        Ok(filled)
    }

    fn complete_at(
        &self,
        type_id: TypeId,
        json: &mut serde_json::Value,
        path: &str,
        filled: &mut Vec<String>,
    ) -> Result<(), ReflectError> {
        use bevy_reflect::enums::VariantInfo;
        use serde_json::Value as Json;
        let Some(info) = self.types.get(type_id).map(|r| r.type_info()) else {
            return Ok(());
        };
        match (info, json) {
            (TypeInfo::Struct(s), Json::Object(o)) => {
                for (key, value) in o.iter_mut() {
                    if let Some(f) = s.field(key) {
                        self.complete_at(f.type_id(), value, &format!("{path}.{key}"), filled)?;
                    }
                }
            }
            (TypeInfo::TupleStruct(t), json) if t.field_len() == 1 => {
                if let Some(f) = t.field_at(0) {
                    self.complete_at(f.type_id(), json, path, filled)?;
                }
            }
            (TypeInfo::List(l), Json::Array(items)) => {
                for (i, item) in items.iter_mut().enumerate() {
                    self.complete_at(l.item_ty().id(), item, &format!("{path}[{i}]"), filled)?;
                }
            }
            (TypeInfo::Enum(e), Json::Object(o)) if o.len() == 1 => {
                let Some((name, payload)) = o.iter_mut().next() else {
                    return Ok(());
                };
                let Some(variant) = e.variant(name) else {
                    return Ok(());
                };
                let at = format!("{path}.{name}");
                match (variant, payload) {
                    (VariantInfo::Struct(v), Json::Object(fields)) => {
                        for f in v.iter() {
                            let field_path = format!("{at}.{}", f.name());
                            match fields.get_mut(f.name()) {
                                Some(value) => {
                                    self.complete_at(f.type_id(), value, &field_path, filled)?
                                }
                                None => {
                                    let default = self
                                        .types
                                        .get(f.type_id())
                                        .and_then(default_of)
                                        .ok_or_else(|| {
                                            ReflectError::NoDefault(format!(
                                                "{field_path} (give it in the value)"
                                            ))
                                        })?;
                                    fields.insert(
                                        f.name().to_string(),
                                        self.value_to_json(default.as_ref())?,
                                    );
                                    filled.push(field_path);
                                }
                            }
                        }
                    }
                    (VariantInfo::Tuple(t), payload) if t.field_len() == 1 => {
                        if let Some(f) = t.field_at(0) {
                            self.complete_at(f.type_id(), payload, &at, filled)?;
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Convert JSON to a reflected value of a registered type. The result is
    /// usually dynamic; it can be passed to `set_field`, `insert_reflect` and friends.
    pub fn value_from_json(
        &self,
        type_id: TypeId,
        json: &serde_json::Value,
    ) -> Result<Value, ReflectError> {
        let registration = self
            .types
            .get(type_id)
            .ok_or_else(|| ReflectError::Unregistered(format!("{type_id:?}")))?;
        TypedReflectDeserializer::new(registration, &self.types)
            .deserialize(json)
            .map_err(|e| ReflectError::Deserialize {
                type_path: registration.type_info().type_path().to_string(),
                message: e.to_string(),
            })
    }

    /// Convert a reflected value to JSON, in the scene-file representation.
    pub fn value_to_json(
        &self,
        value: &dyn PartialReflect,
    ) -> Result<serde_json::Value, ReflectError> {
        serde_json::to_value(TypedReflectSerializer::new(value, &self.types)).map_err(|e| {
            ReflectError::Serialize {
                type_path: value.reflect_type_path().to_string(),
                message: e.to_string(),
            }
        })
    }
}

fn type_attr_hidden(info: &TypeInfo) -> bool {
    type_has::<Hidden>(info)
}

fn type_has<T: Reflect>(info: &TypeInfo) -> bool {
    match info {
        TypeInfo::Struct(i) => i.has_attribute::<T>(),
        TypeInfo::TupleStruct(i) => i.has_attribute::<T>(),
        TypeInfo::Enum(i) => i.has_attribute::<T>(),
        _ => false,
    }
}

fn trimmed_doc(info: &'static TypeInfo) -> Option<&'static str> {
    info.docs().map(str::trim).filter(|d| !d.is_empty())
}

fn default_of(registration: &TypeRegistration) -> Option<Value> {
    registration
        .data::<ReflectDefault>()
        .map(|d| d.default().into_partial_reflect())
}

/// A registered component type.
///
/// This is the description the inspector, the scene serializer, undo and the
/// registry dump consume. Its internals are private so that script-declared
/// component types (M7) can be added behind the same interface.
#[derive(Clone, Copy)]
pub struct ComponentType<'r> {
    name: &'static str,
    registration: &'r TypeRegistration,
    pub(crate) data: &'r ReflectComponent,
}

impl<'r> ComponentType<'r> {
    /// The stable short name used in scene files and field paths.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The full type path, e.g. `"box_engine::Transform"`.
    pub fn type_path(&self) -> &'static str {
        self.registration.type_info().type_path()
    }

    /// The type's doc comment.
    pub fn doc(&self) -> Option<&'static str> {
        trimmed_doc(self.registration.type_info())
    }

    /// False for `@NotSaved` types.
    pub fn is_saved(&self) -> bool {
        !type_has::<NotSaved>(self.registration.type_info())
    }

    /// True for `@Hidden` types (not offered in the inspector or add-component menu).
    pub fn is_hidden(&self) -> bool {
        type_attr_hidden(self.registration.type_info())
    }

    /// Top-level fields in declaration order. Tuple-struct fields are named
    /// `"0"`, `"1"`, ...; enums have none (see [`Registry::field_meta`]).
    pub fn fields(&self) -> Vec<FieldMeta> {
        meta::fields_of(self.registration.type_info())
    }

    /// A fresh default value, if the type has `#[reflect(Default)]`.
    pub fn default_value(&self) -> Option<Value> {
        default_of(self.registration)
    }

    /// Type data of kind `D` attached to this type (by a `#[reflect(...)]`
    /// attribute or [`Registry::insert_type_data`]).
    pub fn data<D: TypeData>(&self) -> Option<&'r D> {
        self.registration.data::<D>()
    }

    /// True if `entity` has this component.
    pub fn contained_in(&self, world: &World, entity: Entity) -> bool {
        (self.data.contains)(world, entity)
    }

    /// Every entity with this component, in storage order: a typed query
    /// for callers that only know the name (scripts, tools).
    pub fn entities(&self, world: &World) -> Vec<Entity> {
        (self.data.entities)(world)
    }

    pub fn type_id(&self) -> TypeId {
        self.registration.type_id()
    }

    /// The type's reflection info (fields, variants), for code generators.
    pub fn type_info(&self) -> &'static TypeInfo {
        self.registration.type_info()
    }
}

/// A registered resource type. See [`ComponentType`].
#[derive(Clone, Copy)]
pub struct ResourceType<'r> {
    name: &'static str,
    registration: &'r TypeRegistration,
    pub(crate) data: &'r ReflectResource,
    settings: bool,
}

impl<'r> ResourceType<'r> {
    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn type_path(&self) -> &'static str {
        self.registration.type_info().type_path()
    }

    pub fn doc(&self) -> Option<&'static str> {
        trimmed_doc(self.registration.type_info())
    }

    /// False for `@NotSaved` types and settings resources (which live in
    /// their own files, not in scenes).
    pub fn is_saved(&self) -> bool {
        !self.settings && !type_has::<NotSaved>(self.registration.type_info())
    }

    /// True for a project settings resource ([`Registry::register_settings`]).
    pub fn is_settings(&self) -> bool {
        self.settings
    }

    pub fn is_hidden(&self) -> bool {
        type_attr_hidden(self.registration.type_info())
    }

    pub fn fields(&self) -> Vec<FieldMeta> {
        meta::fields_of(self.registration.type_info())
    }

    pub fn default_value(&self) -> Option<Value> {
        default_of(self.registration)
    }

    /// The type's id (for [`Registry::value_from_json`]).
    pub fn type_id(&self) -> TypeId {
        self.registration.type_id()
    }

    /// The type's reflection info (fields, variants), for code generators.
    pub fn type_info(&self) -> &'static TypeInfo {
        self.registration.type_info()
    }
}

/// A registered event type ([`Registry::register_event`]).
#[derive(Clone, Copy)]
pub struct EventType<'r> {
    name: &'static str,
    registration: &'r TypeRegistration,
    data: &'r ReflectEvent,
}

impl<'r> EventType<'r> {
    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn type_path(&self) -> &'static str {
        self.registration.type_info().type_path()
    }

    pub fn doc(&self) -> Option<&'static str> {
        trimmed_doc(self.registration.type_info())
    }

    pub fn fields(&self) -> Vec<FieldMeta> {
        meta::fields_of(self.registration.type_info())
    }

    /// A fresh default value, if the type has `#[reflect(Default)]`.
    pub fn default_value(&self) -> Option<Value> {
        default_of(self.registration)
    }

    pub fn type_id(&self) -> TypeId {
        self.registration.type_id()
    }

    /// The type's reflection info (fields, variants), for code generators.
    pub fn type_info(&self) -> &'static TypeInfo {
        self.registration.type_info()
    }

    /// Sends `value` (concrete or dynamic, of this type's shape) on the
    /// world's queue. Fails when the shape is wrong or the world has no
    /// queue for it.
    pub fn send(&self, world: &mut World, value: &dyn PartialReflect) -> Result<(), ReflectError> {
        (self.data.send)(world, value)
    }

    /// Copies of the events sent since `cursor` (an event id; start at 0),
    /// oldest first; advances the cursor. Like an
    /// [`EventCursor`](crate::event::EventCursor) it sees every event once
    /// when read at least every other frame.
    pub fn read(&self, world: &World, cursor: &mut u64) -> Vec<Box<dyn Reflect>> {
        (self.data.read)(world, cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;

    /// A reflected event.
    #[derive(Reflect, Clone, Debug, Default, PartialEq)]
    #[reflect(Default, Clone)]
    struct Hit {
        damage: u32,
    }

    mod other {
        use crate::reflect::{Reflect, ReflectComponent, ReflectDefault};

        /// A component with the event's short name.
        #[derive(Reflect, Clone, Default)]
        #[reflect(Component, Default, Clone)]
        pub struct Hit;

        /// An event with a component's short name.
        #[derive(Reflect, Clone, Default)]
        #[reflect(Default, Clone)]
        pub struct Name;
    }

    #[test]
    fn events_are_sent_and_read_by_name() {
        let mut registry = Registry::with_core_types();
        registry.register_event::<Hit>().unwrap();
        registry.register_event::<Hit>().unwrap();
        assert_eq!(
            registry.events().map(|e| e.name()).collect::<Vec<_>>(),
            ["Hit"]
        );
        assert!(registry.component("Hit").is_none());
        let mut app = App::new();
        app.world.set_registry(std::sync::Arc::new(registry));
        let registry = app.world.registry().clone();
        let hit = registry.event("Hit").unwrap();
        // No queue yet.
        assert!(hit.send(&mut app.world, &Hit { damage: 1 }).is_err());
        app.add_event::<Hit>();
        let mut value = hit.default_value().unwrap();
        crate::reflect::replace(value.as_mut(), &Hit { damage: 7 }).unwrap();
        hit.send(&mut app.world, value.as_ref()).unwrap();
        let mut cursor = 0;
        let read = hit.read(&app.world, &mut cursor);
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].downcast_ref::<Hit>(), Some(&Hit { damage: 7 }));
        assert!(hit.read(&app.world, &mut cursor).is_empty());
    }

    #[test]
    fn event_names_share_the_short_name_space() {
        let mut registry = Registry::with_core_types();
        registry.register_event::<Hit>().unwrap();
        assert!(registry.register::<other::Hit>().is_err());
        assert!(registry.register_event::<other::Name>().is_err());
        assert_eq!(registry.take_errors().len(), 2);
    }
}
