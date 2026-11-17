//! Declared generator inputs ([`Inputs`]): components of other entities and
//! loaded assets a generator reads, watched by the runtime.

use std::any::{Any, TypeId};
use std::fmt;
use std::ops::Deref;

use bevy_reflect::{Reflect, TypePath};
use box_core::asset_ref::AssetRef;
use box_core::assets::{AssetLookup, Assets, Handle};
use box_core::identity::Uid;
use box_core::transform::GlobalTransform;
use box_core::world::{Component, Ref, World};

use crate::hash::{StableHasher, reflect_hash};

/// One declared input.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum InputId {
    /// The `type_name` component of the entity `uid`.
    Component {
        uid: Uid,
        type_id: TypeId,
        type_name: &'static str,
    },
    /// The loaded asset `asset` of type `type_name`.
    Asset {
        asset: AssetRef,
        type_id: TypeId,
        type_name: &'static str,
    },
    /// The world transform (`GlobalTransform`) of the entity `uid`.
    WorldTransform { uid: Uid },
}

impl InputId {
    pub(crate) fn is_component<C: 'static>(&self, of: Uid) -> bool {
        matches!(self, Self::Component { uid, type_id, .. } if *uid == of && *type_id == TypeId::of::<C>())
    }

    pub(crate) fn is_world_transform(&self, of: Uid) -> bool {
        matches!(self, Self::WorldTransform { uid } if *uid == of)
    }

    pub(crate) fn is_asset<A: 'static>(&self, of: &AssetRef) -> bool {
        matches!(self, Self::Asset { asset, type_id, .. } if asset == of && *type_id == TypeId::of::<A>())
    }
}

impl fmt::Display for InputId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Component { uid, type_name, .. } => {
                write!(f, "`{type_name}` of entity {uid}")
            }
            Self::Asset {
                asset, type_name, ..
            } => write!(f, "{type_name} asset {asset}"),
            Self::WorldTransform { uid } => write!(f, "the world transform of entity {uid}"),
        }
    }
}

/// A resolved input value, borrowed from the world while a generator runs
/// (or from a golden case's stored inputs).
pub(crate) enum InputValue<'w> {
    Component(Box<dyn Deref<Target = dyn Any> + 'w>),
    Borrowed(&'w dyn Any),
}

impl InputValue<'_> {
    pub(crate) fn downcast_ref<T: 'static>(&self) -> Option<&T> {
        match self {
            Self::Component(c) => (**c).downcast_ref(),
            Self::Borrowed(a) => a.downcast_ref(),
        }
    }
}

/// An input declaration with its resolved value, as [`GenContext`](crate::GenContext) holds it.
pub(crate) struct ResolvedInput<'w> {
    pub id: InputId,
    pub value: Option<InputValue<'w>>,
}

/// Watches one input for changes between frames (a cached copy of the
/// component, or the asset's handle and version).
pub(crate) trait Probe {
    /// True when the input differs from the last call (or creation).
    fn changed(&mut self, world: &World) -> bool;
}

/// Monomorphised accessors of one declared input.
#[derive(Clone)]
pub(crate) struct InputDecl {
    pub id: InputId,
    pub resolve: for<'w> fn(&'w World, &InputId) -> Option<InputValue<'w>>,
    /// A hash of the current value (a constant marker when missing).
    pub key: fn(&World, &InputId) -> u64,
    pub probe: fn(&World, &InputId) -> Box<dyn Probe>,
}

/// Where a generator declares what it reads ([`Generator::inputs`](crate::Generator::inputs)).
///
/// ```ignore
/// fn inputs(&self, inputs: &mut Inputs) {
///     inputs.component::<Spline>(self.path);
///     inputs.asset::<MeshData>(&self.profile);
/// }
/// ```
#[derive(Default)]
pub struct Inputs {
    pub(crate) decls: Vec<InputDecl>,
    pub(crate) owner: Option<Uid>,
}

impl Inputs {
    /// Inputs of a generator on the entity `owner`.
    pub(crate) fn for_owner(owner: Option<Uid>) -> Self {
        Self {
            decls: Vec::new(),
            owner,
        }
    }

    /// The `Uid` of the entity the generator is on (`None` for an entity
    /// without one). A generator that reads a component of its own entity
    /// (a `Sweep` of the entity's own `Spline`) declares it with this uid.
    pub fn owner(&self) -> Option<Uid> {
        self.owner
    }

    /// Read the world transform of the entity `uid` (with
    /// [`GenContext::world_transform`](crate::GenContext::world_transform)):
    /// for a generator that places geometry relative to another entity,
    /// such as a sweep along a spline on another entity. Moving that
    /// entity, or a parent of it, regenerates the generator.
    pub fn world_transform(&mut self, uid: Uid) -> &mut Self {
        self.push(InputDecl {
            id: InputId::WorldTransform { uid },
            resolve: resolve_component::<GlobalTransform>,
            key: key_world_transform,
            probe: probe_component::<GlobalTransform>,
        })
    }

    /// Read the `C` component of the entity `uid` (for example a spline).
    /// `C` needs `Reflect` (its value is part of the cache key) and
    /// `PartialEq` (it is compared with a cached copy every frame).
    pub fn component<C: Component + Reflect + TypePath + Clone + PartialEq>(
        &mut self,
        uid: Uid,
    ) -> &mut Self {
        self.push(InputDecl {
            id: InputId::Component {
                uid,
                type_id: TypeId::of::<C>(),
                type_name: C::short_type_path(),
            },
            resolve: resolve_component::<C>,
            key: key_component::<C>,
            probe: probe_component::<C>,
        })
    }

    /// Read the loaded asset `asset` of type `A` (resolved through the
    /// world's `AssetLookup`; edits and hot reloads of the file regenerate).
    pub fn asset<A: Send + Sync + 'static>(&mut self, asset: &AssetRef) -> &mut Self {
        self.push(InputDecl {
            id: InputId::Asset {
                asset: asset.clone(),
                type_id: TypeId::of::<A>(),
                type_name: short_name::<A>(),
            },
            resolve: resolve_asset::<A>,
            key: key_asset::<A>,
            probe: probe_asset::<A>,
        })
    }

    /// The declared inputs, in declaration order.
    pub fn ids(&self) -> impl Iterator<Item = &InputId> {
        self.decls.iter().map(|d| &d.id)
    }

    pub fn len(&self) -> usize {
        self.decls.len()
    }

    pub fn is_empty(&self) -> bool {
        self.decls.is_empty()
    }

    fn push(&mut self, decl: InputDecl) -> &mut Self {
        if !self.decls.iter().any(|d| d.id == decl.id) {
            self.decls.push(decl);
        }
        self
    }
}

fn short_name<A>() -> &'static str {
    let full = std::any::type_name::<A>();
    let base = full.split('<').next().unwrap_or(full);
    base.rsplit("::").next().unwrap_or(base)
}

const MISSING: u64 = 0x6d69_7373_696e_6721;

struct AnyRef<'w, C: Component>(Ref<'w, C>);

impl<C: Component> Deref for AnyRef<'_, C> {
    type Target = dyn Any;
    fn deref(&self) -> &(dyn Any + 'static) {
        &*self.0
    }
}

fn component_uid(id: &InputId) -> Option<Uid> {
    match id {
        InputId::Component { uid, .. } | InputId::WorldTransform { uid } => Some(*uid),
        InputId::Asset { .. } => None,
    }
}

fn asset_ref(id: &InputId) -> Option<&AssetRef> {
    match id {
        InputId::Asset { asset, .. } => Some(asset),
        InputId::Component { .. } | InputId::WorldTransform { .. } => None,
    }
}

/// A hash of the entity's world transform (its bits; a constant marker
/// when missing).
fn key_world_transform(world: &World, id: &InputId) -> u64 {
    match current::<GlobalTransform>(world, component_uid(id)) {
        Some(g) => {
            let mut s = StableHasher::new();
            for v in g.0.to_cols_array() {
                s.write_u32(v.to_bits());
            }
            s.finish()
        }
        None => MISSING,
    }
}

fn resolve_component<'w, C: Component>(world: &'w World, id: &InputId) -> Option<InputValue<'w>> {
    let e = world.entity_by_uid(component_uid(id)?)?;
    let r = world.get::<C>(e)?;
    Some(InputValue::Component(Box::new(AnyRef(r))))
}

fn key_component<C: Component + Reflect>(world: &World, id: &InputId) -> u64 {
    component_uid(id)
        .and_then(|uid| world.entity_by_uid(uid))
        .and_then(|e| world.get::<C>(e).map(|c| reflect_hash(&*c)))
        .unwrap_or(MISSING)
}

struct ComponentProbe<C> {
    uid: Option<Uid>,
    cached: Option<C>,
}

fn current<C: Component>(world: &World, uid: Option<Uid>) -> Option<Ref<'_, C>> {
    world.get::<C>(world.entity_by_uid(uid?)?)
}

impl<C: Component + Clone + PartialEq> Probe for ComponentProbe<C> {
    fn changed(&mut self, world: &World) -> bool {
        let current = current::<C>(world, self.uid);
        if current.as_deref() == self.cached.as_ref() {
            return false;
        }
        self.cached = current.map(|c| (*c).clone());
        true
    }
}

fn probe_component<C: Component + Clone + PartialEq>(
    world: &World,
    id: &InputId,
) -> Box<dyn Probe> {
    let uid = component_uid(id);
    let cached = current::<C>(world, uid).map(|c| (*c).clone());
    Box::new(ComponentProbe { uid, cached })
}

fn asset_handle<A: 'static>(world: &World, asset: &AssetRef) -> Option<Handle<A>> {
    world.get_resource::<AssetLookup>()?.get::<A>(asset)
}

fn resolve_asset<'w, A: Send + Sync + 'static>(
    world: &'w World,
    id: &InputId,
) -> Option<InputValue<'w>> {
    let handle = asset_handle::<A>(world, asset_ref(id)?)?;
    let value = world.get_resource::<Assets<A>>()?.get(handle)?;
    Some(InputValue::Borrowed(value))
}

/// The asset's handle and version: enough to tell a reload or an edit
/// from the in-memory cache key's point of view.
fn asset_state<A: 'static>(world: &World, handle: Option<Handle<A>>) -> Option<(u64, u64)> {
    let handle = handle?;
    let version = world.get_resource::<Assets<A>>()?.version(handle)?;
    Some((handle.id(), version))
}

fn key_asset<A: Send + Sync + 'static>(world: &World, id: &InputId) -> u64 {
    let handle = asset_ref(id).and_then(|a| asset_handle::<A>(world, a));
    match asset_state(world, handle) {
        Some((h, v)) => {
            let mut s = StableHasher::new();
            s.write_u64(h);
            s.write_u64(v);
            s.finish()
        }
        None => MISSING,
    }
}

struct AssetProbe<A: 'static> {
    asset: AssetRef,
    /// `AssetLookup::generation` when `handle` was resolved.
    lookup: Option<u64>,
    handle: Option<Handle<A>>,
    state: Option<(u64, u64)>,
}

impl<A: Send + Sync + 'static> Probe for AssetProbe<A> {
    fn changed(&mut self, world: &World) -> bool {
        let generation = world
            .get_resource::<AssetLookup>()
            .map(AssetLookup::generation);
        if generation != self.lookup {
            self.lookup = generation;
            self.handle = asset_handle::<A>(world, &self.asset);
        }
        let state = asset_state(world, self.handle);
        if state == self.state {
            return false;
        }
        self.state = state;
        true
    }
}

fn probe_asset<A: Send + Sync + 'static>(world: &World, id: &InputId) -> Box<dyn Probe> {
    let asset = asset_ref(id).cloned().unwrap_or_default();
    let handle = asset_handle::<A>(world, &asset);
    Box::new(AssetProbe::<A> {
        lookup: world
            .get_resource::<AssetLookup>()
            .map(AssetLookup::generation),
        state: asset_state(world, handle),
        handle,
        asset,
    })
}
