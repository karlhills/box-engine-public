//! Derived entities: [`reconcile`] makes the parts a rule computed for an
//! owner exist in the world, touching only what changed.
//!
//! A rule (a plain function from saved intent to parts, see
//! docs/PROCEDURAL_DIRECTION.md §3) returns a list of [`Part`]s for an
//! owner, each with a stable [`PartKey`]. `reconcile`:
//!
//! - **spawns** missing parts as children of the owner (their `Transform`
//!   is relative to it), marked [`Derived`] `{ owner, key, rule }` with the
//!   uid [`PartKey::uid`] and the key's role as `Name`;
//! - **updates** existing parts in place, writing a component only when its
//!   value differs (`PartialEq`), so unchanged parts are not touched and
//!   whatever reacts to their components (generators, colliders) does not
//!   run again; a component the part no longer lists is removed;
//! - **despawns** parts whose key disappeared, with their subtrees;
//! - never touches parts of another owner or another rule;
//! - applies the owner's [`Overrides`] of this rule after the rule's
//!   output and before anything hashes it: patched fields are written over
//!   the rule's values, hidden parts are not spawned, and an override whose
//!   part or field does not exist is kept and reported as a warning;
//! - replaces this rule's entries in the owner's [`GeneratedWarnings`]
//!   (rule-level warnings plus each part's), deduplicated, logging each new
//!   warning once with `warn!` (target `box_proc::derive`, so the editor
//!   toasts it; not for a [`Preview`] owner, whose warnings change with
//!   every pointer move).
//!
//! Derived parts are children of the owner, so despawning the owner
//! despawns them too (`World::despawn` is recursive); deleting the owner
//! through `box_core::edit` and undoing it restores only the owner, and the
//! next rule pass spawns its parts again with the same uids.
//!
//! ```
//! use box_core::derived::{Derived, PartKey};
//! use box_core::prelude::*;
//! use box_proc::derive::{Part, reconcile};
//!
//! #[derive(Clone, PartialEq)]
//! struct Post { height: f32 }
//!
//! let mut world = World::new();
//! let owner = world.spawn((Uid::new(), Transform::IDENTITY));
//! let uid = world.uid(owner).unwrap();
//! let parts = vec![
//!     Part::new(PartKey::new(uid, "post/start")).with(Post { height: 1.0 }),
//!     Part::new(PartKey::new(uid, "post/end"))
//!         .at(Transform::from_xyz(2.0, 0.0, 0.0))
//!         .with(Post { height: 1.0 }),
//! ];
//! let report = reconcile(&mut world, owner, "fence", parts.clone()).unwrap();
//! assert_eq!(report.spawned, 2);
//! // The same parts again: nothing is written.
//! let report = reconcile(&mut world, owner, "fence", parts).unwrap();
//! assert_eq!((report.unchanged, report.changed()), (2, false));
//! assert_eq!(world.query::<&Derived>().iter().count(), 2);
//! ```

use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

use box_core::derived::{
    Derived, GeneratedRevision, GeneratedWarning, GeneratedWarnings, PartKey, Preview,
};
use box_core::identity::{Name, Uid};
use box_core::reflect::{FromReflect, PartialReflect, Registry};
use box_core::transform::Transform;
use box_core::world::{Component, Entity, World};

use crate::overrides::{FieldOverride, Overrides, PartOverride};

/// One part a rule wants for an owner: its key, its transform relative to
/// the owner, its components and its warnings.
pub struct Part {
    pub key: PartKey,
    /// Relative to the owner (the part's parent).
    pub transform: Transform,
    components: Vec<Box<dyn PartComponent>>,
    /// Shown on the owner, for example "corner sharper than the mitre
    /// limit, bevelled".
    pub warnings: Vec<String>,
}

impl Part {
    /// A part at the owner's origin with no components.
    pub fn new(key: PartKey) -> Self {
        Self {
            key,
            transform: Transform::IDENTITY,
            components: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// The part's transform relative to its owner.
    pub fn at(mut self, transform: Transform) -> Self {
        self.transform = transform;
        self
    }

    /// Adds a component (replacing one of the same type). Reconcile writes
    /// it only when it differs from the part's current value, compared with
    /// `PartialEq`: compare parameters only, never runtime caches. A
    /// `Transform` sets [`Part::transform`].
    pub fn with<T: Component + PartialEq + Clone>(mut self, component: T) -> Self {
        if let Some(t) = (&component as &dyn Any).downcast_ref::<Transform>() {
            self.transform = *t;
            return self;
        }
        self.components
            .retain(|c| c.component_type() != TypeId::of::<T>());
        self.components.push(Box::new(Typed(component)));
        self
    }

    /// Adds a warning about this part (shown on the owner).
    pub fn warn(mut self, message: impl Into<String>) -> Self {
        self.warnings.push(message.into());
        self
    }

    /// Adds a component given as a reflected value of a registered
    /// component type (scripts, tools): the same as [`Part::with`] for its
    /// concrete type. The value is compared with
    /// `PartialReflect::reflect_partial_eq` and written with
    /// `World::put_reflect`. A `Transform` sets [`Part::transform`]. Fails
    /// when the value's type is not a registered component.
    pub fn with_reflect(
        self,
        registry: &Registry,
        value: Box<dyn PartialReflect>,
    ) -> Result<Self, String> {
        self.with_shared_reflect(registry, value.into())
    }

    /// [`Part::with_reflect`] with a shared value: parts made from the same
    /// `Arc` (a converter's cache) compare equal by pointer
    /// ([`Part::same`]) without walking the value.
    pub fn with_shared_reflect(
        mut self,
        registry: &Registry,
        value: Arc<dyn PartialReflect>,
    ) -> Result<Self, String> {
        let v: &dyn PartialReflect = &*value;
        let info = v
            .get_represented_type_info()
            .ok_or_else(|| format!("`{}` is not a typed value", v.reflect_type_path()))?;
        let type_id = info.type_id();
        let ty = registry.component_by_id(type_id).ok_or_else(|| {
            format!(
                "`{}` is not a registered component",
                v.reflect_short_type_path()
            )
        })?;
        if type_id == TypeId::of::<Transform>() {
            self.transform = Transform::from_reflect(v)
                .ok_or_else(|| "the Transform value is incomplete".to_owned())?;
            return Ok(self);
        }
        let name = ty.name();
        self.components.retain(|c| c.component_type() != type_id);
        self.components.push(Box::new(Reflected {
            type_id,
            name,
            value,
        }));
        Ok(self)
    }

    /// Whether `other` is the same part: key, transform, warnings, and the
    /// same components (type and value) in the same order. Reconciling
    /// either gives the same world.
    pub fn same(&self, other: &Part) -> bool {
        self.key == other.key
            && self.transform == other.transform
            && self.warnings == other.warnings
            && self.components.len() == other.components.len()
            && self
                .components
                .iter()
                .zip(&other.components)
                .all(|(a, b)| a.same(b.as_ref()))
    }

    /// Short type names of the components, in the order added.
    pub fn component_names(&self) -> Vec<&'static str> {
        self.components.iter().map(|c| c.short_name()).collect()
    }
}

impl Clone for Part {
    fn clone(&self) -> Self {
        Self {
            key: self.key.clone(),
            transform: self.transform,
            components: self.components.iter().map(|c| c.boxed_clone()).collect(),
            warnings: self.warnings.clone(),
        }
    }
}

impl fmt::Debug for Part {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Part")
            .field("key", &self.key)
            .field("transform", &self.transform)
            .field("components", &self.component_names())
            .field("warnings", &self.warnings)
            .finish()
    }
}

/// A typed component of a [`Part`], type-erased.
trait PartComponent: Send + Sync {
    fn component_type(&self) -> TypeId;
    fn as_any(&self) -> &dyn Any;
    /// Same type and equal value.
    fn same(&self, other: &dyn PartComponent) -> bool;
    fn short_name(&self) -> &'static str;
    fn differs(&self, world: &World, e: Entity) -> bool;
    fn insert(self: Box<Self>, world: &mut World, e: Entity);
    fn remover(&self) -> Remover;
    fn boxed_clone(&self) -> Box<dyn PartComponent>;
}

/// How reconcile removes a component a part no longer lists.
#[derive(Clone, Copy)]
enum Remover {
    Typed(fn(&mut World, Entity)),
    /// A registered component by name (reflected parts).
    Named(&'static str),
}

impl Remover {
    fn remove(self, world: &mut World, e: Entity) {
        match self {
            Remover::Typed(f) => f(world, e),
            Remover::Named(name) => {
                let _ = world.remove_reflect(e, name);
            }
        }
    }
}

struct Typed<T>(T);

fn remove<T: Component>(world: &mut World, e: Entity) {
    let _ = world.remove_one::<T>(e);
}

impl<T: Component + PartialEq + Clone> PartComponent for Typed<T> {
    fn component_type(&self) -> TypeId {
        TypeId::of::<T>()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn same(&self, other: &dyn PartComponent) -> bool {
        other
            .as_any()
            .downcast_ref::<Typed<T>>()
            .is_some_and(|o| o.0 == self.0)
    }

    fn short_name(&self) -> &'static str {
        let full = std::any::type_name::<T>();
        full.rsplit("::").next().unwrap_or(full)
    }

    fn differs(&self, world: &World, e: Entity) -> bool {
        world.get::<T>(e).is_none_or(|c| *c != self.0)
    }

    fn insert(self: Box<Self>, world: &mut World, e: Entity) {
        let _ = world.insert_one(e, self.0);
    }

    fn remover(&self) -> Remover {
        Remover::Typed(remove::<T>)
    }

    fn boxed_clone(&self) -> Box<dyn PartComponent> {
        Box::new(Typed(self.0.clone()))
    }
}

/// A part component given as a reflected value ([`Part::with_reflect`]).
struct Reflected {
    type_id: TypeId,
    /// The registry's short name.
    name: &'static str,
    /// Shared: never mutated, so clones of the part are cheap.
    value: Arc<dyn PartialReflect>,
}

impl PartComponent for Reflected {
    fn component_type(&self) -> TypeId {
        self.type_id
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn same(&self, other: &dyn PartComponent) -> bool {
        other.as_any().downcast_ref::<Reflected>().is_some_and(|o| {
            o.type_id == self.type_id
                && (Arc::ptr_eq(&o.value, &self.value)
                    || (*o.value).reflect_partial_eq(&*self.value) == Some(true))
        })
    }

    fn short_name(&self) -> &'static str {
        self.name
    }

    fn differs(&self, world: &World, e: Entity) -> bool {
        match world.get_reflect(e, self.name) {
            Ok(current) => {
                current
                    .as_partial_reflect()
                    .reflect_partial_eq(self.value.as_ref())
                    != Some(true)
            }
            Err(_) => true,
        }
    }

    fn insert(self: Box<Self>, world: &mut World, e: Entity) {
        if let Err(err) = world.put_reflect(e, self.name, self.value.as_ref()) {
            tracing::warn!("part component `{}` could not be written: {err}", self.name);
        }
    }

    fn remover(&self) -> Remover {
        Remover::Named(self.name)
    }

    fn boxed_clone(&self) -> Box<dyn PartComponent> {
        Box::new(Reflected {
            type_id: self.type_id,
            name: self.name,
            value: self.value.clone(),
        })
    }
}

/// The components a part's entity got from reconcile, so components the
/// part no longer lists can be removed, and what its overrides patched.
/// Runtime only.
#[derive(Default)]
struct PartState {
    types: Vec<(TypeId, Remover)>,
    /// The rule's own (unpatched) values of the components the overrides
    /// patch: compared with the next rule output instead of the world.
    base: Vec<Box<dyn PartComponent>>,
    /// The field overrides applied over `base`.
    patch: Vec<FieldOverride>,
    /// Why some of `patch` could not be applied (kept as warnings).
    patch_warnings: Vec<String>,
}

/// What one [`reconcile`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    /// Parts spawned (their key was new).
    pub spawned: usize,
    /// Parts whose transform or components changed.
    pub updated: usize,
    /// Parts left as they were.
    pub unchanged: usize,
    /// Parts despawned (their key disappeared).
    pub removed: usize,
    /// Parts refused (invalid or duplicate key, or a key another rule of
    /// the owner uses); each is a warning on the owner.
    pub rejected: usize,
    /// Parts not built because an override hides them.
    pub hidden: usize,
    /// This rule's warnings on the owner after the pass.
    pub warnings: usize,
    /// The owner's warnings of this rule changed (and were logged).
    pub warnings_changed: bool,
}

impl ReconcileReport {
    /// Anything was spawned, updated, removed, or the warnings changed.
    pub fn changed(&self) -> bool {
        self.spawned + self.updated + self.removed > 0 || self.warnings_changed
    }

    /// Parts that exist after the pass.
    pub fn parts(&self) -> usize {
        self.spawned + self.updated + self.unchanged
    }
}

impl std::ops::AddAssign for ReconcileReport {
    fn add_assign(&mut self, o: Self) {
        self.spawned += o.spawned;
        self.updated += o.updated;
        self.unchanged += o.unchanged;
        self.removed += o.removed;
        self.rejected += o.rejected;
        self.hidden += o.hidden;
        self.warnings += o.warnings;
        self.warnings_changed |= o.warnings_changed;
    }
}

/// Why [`reconcile`] could not run.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ReconcileError {
    #[error("the owner entity {0:?} does not exist")]
    NoOwner(Entity),
    #[error(
        "the owner entity {0:?} has no Uid: owners are saved entities, and part keys start with the owner's uid"
    )]
    OwnerWithoutUid(Entity),
}

/// Makes `owner`'s parts from `rule` exactly `parts` (see the module docs).
pub fn reconcile(
    world: &mut World,
    owner: Entity,
    rule: &str,
    parts: Vec<Part>,
) -> Result<ReconcileReport, ReconcileError> {
    reconcile_with(world, owner, rule, parts, Vec::new())
}

/// [`reconcile`] plus rule-level warnings for the owner (not about one
/// part), such as "three walls meet at one point; butted against the
/// longest pair".
pub fn reconcile_with(
    world: &mut World,
    owner: Entity,
    rule: &str,
    parts: Vec<Part>,
    rule_warnings: Vec<String>,
) -> Result<ReconcileReport, ReconcileError> {
    reconcile_logged(world, owner, rule, parts, rule_warnings, true)
}

/// [`reconcile_with`]; `log` false keeps new warnings off the log (the
/// caller reported them once for every owner, as a failing script rule
/// does).
#[profiling::function]
pub(crate) fn reconcile_logged(
    world: &mut World,
    owner: Entity,
    rule: &str,
    parts: Vec<Part>,
    rule_warnings: Vec<String>,
    log: bool,
) -> Result<ReconcileReport, ReconcileError> {
    reconcile_inner(world, owner, rule, parts, rule_warnings, log, true)
}

/// Removes `owner`'s parts and warnings of `rule`, the owner's overrides
/// of it included: the rule no longer serves the owner (its intent went,
/// or the rule was switched off), so the owner looks as if the rule had
/// never seen it.
pub(crate) fn release(
    world: &mut World,
    owner: Entity,
    rule: &str,
) -> Result<ReconcileReport, ReconcileError> {
    reconcile_inner(world, owner, rule, Vec::new(), Vec::new(), true, false)
}

fn reconcile_inner(
    world: &mut World,
    owner: Entity,
    rule: &str,
    parts: Vec<Part>,
    rule_warnings: Vec<String>,
    log: bool,
    use_overrides: bool,
) -> Result<ReconcileReport, ReconcileError> {
    if !world.contains(owner) {
        return Err(ReconcileError::NoOwner(owner));
    }
    let owner_uid = world
        .uid(owner)
        .ok_or(ReconcileError::OwnerWithoutUid(owner))?;
    let mut existing: HashMap<PartKey, Entity> = HashMap::new();
    let mut foreign: HashMap<PartKey, String> = HashMap::new();
    for c in world.children(owner) {
        if let Some(d) = world.get::<Derived>(c)
            && d.owner == owner
        {
            if d.rule == rule {
                existing.insert(d.key.clone(), c);
            } else {
                foreign.insert(d.key.clone(), d.rule.clone());
            }
        }
    }
    let overrides: Vec<PartOverride> = world
        .get::<Overrides>(owner)
        .filter(|_| use_overrides)
        .map(|o| o.parts.iter().filter(|p| p.rule == rule).cloned().collect())
        .unwrap_or_default();
    let warning = |key: Option<&PartKey>, message: String| GeneratedWarning {
        rule: rule.to_owned(),
        key: key.cloned(),
        message,
    };
    let mut warnings: Vec<GeneratedWarning> = rule_warnings
        .into_iter()
        .map(|m| warning(None, m))
        .collect();
    let mut report = ReconcileReport::default();
    let mut seen: HashSet<PartKey> = HashSet::new();
    for part in parts {
        if let Err(m) = part.key.check(owner_uid) {
            warnings.push(warning(None, format!("{m}; part skipped")));
            report.rejected += 1;
            continue;
        }
        if seen.contains(&part.key) {
            warnings.push(warning(
                Some(&part.key),
                "two parts share this key; the second was skipped".into(),
            ));
            report.rejected += 1;
            continue;
        }
        if let Some(other) = foreign.get(&part.key) {
            warnings.push(warning(
                Some(&part.key),
                format!(
                    "rule `{other}` already made a part with this key for this owner; keys are unique per owner, part skipped"
                ),
            ));
            report.rejected += 1;
            continue;
        }
        seen.insert(part.key.clone());
        let patch = overrides.iter().find(|o| o.part == part.key.role());
        if patch.is_some_and(|o| o.hidden) {
            // Hidden: not built; an existing entity is despawned below.
            report.hidden += 1;
            continue;
        }
        let patch: &[FieldOverride] = patch.map_or(&[], |o| &o.fields);
        warnings.extend(
            part.warnings
                .iter()
                .map(|m| warning(Some(&part.key), m.clone())),
        );
        let key = part.key.clone();
        let mut patch_warnings = Vec::new();
        match existing.remove(&part.key) {
            Some(e) => {
                if update(world, e, part, patch, &mut patch_warnings) {
                    report.updated += 1;
                } else {
                    report.unchanged += 1;
                }
            }
            None => match spawn(world, owner, rule, part, patch, &mut patch_warnings) {
                Ok(_) => report.spawned += 1,
                Err(m) => {
                    warnings.push(warning(Some(&key), m));
                    report.rejected += 1;
                }
            },
        }
        warnings.extend(patch_warnings.into_iter().map(|m| warning(Some(&key), m)));
    }
    // Overrides of parts the rule no longer makes are kept, and said so.
    for o in &overrides {
        if !seen.iter().any(|k| k.role() == o.part) {
            warnings.push(warning(
                Some(&PartKey::new(owner_uid, &o.part)),
                "an override names this part, but the rule no longer makes it; the override \
                 is kept (reset it in the owner's Overrides if it is no longer needed)"
                    .into(),
            ));
        }
    }
    for (_, e) in existing {
        world.despawn(e);
        report.removed += 1;
    }
    report.warnings_changed = set_warnings(world, owner, rule, warnings, &mut report.warnings, log);
    if report.changed() {
        world.resource_or_default::<GeneratedRevision>().0 += 1;
    }
    Ok(report)
}

fn spawn(
    world: &mut World,
    owner: Entity,
    rule: &str,
    part: Part,
    patch: &[FieldOverride],
    patch_warnings: &mut Vec<String>,
) -> Result<Entity, String> {
    let uid = part.key.uid();
    if world.entity_by_uid(uid).is_some() {
        return Err(format!(
            "the uid {uid} of this part is taken by another entity (was a part moved out of its owner?); part skipped"
        ));
    }
    let Part {
        key,
        transform,
        components,
        ..
    } = part;
    let name = Name::new(key.role());
    let e = world.spawn((
        uid,
        name,
        Derived {
            owner,
            key,
            rule: rule.to_owned(),
        },
        transform,
    ));
    let _ = world.set_parent(e, Some(owner));
    let types = components
        .iter()
        .map(|c| (c.component_type(), c.remover()))
        .collect();
    let mut state = PartState {
        types,
        ..PartState::default()
    };
    patch_missing(world, &components, patch, patch_warnings);
    for c in components {
        let patched = patch_of(world, c.as_ref(), patch);
        if !patched.is_empty() {
            state.base.push(c.boxed_clone());
        }
        c.insert(world, e);
        apply_patch(world, e, &patched, &mut state.patch_warnings);
    }
    if !state.base.is_empty() {
        state.patch = patch.to_vec();
        patch_warnings.extend(state.patch_warnings.iter().cloned());
    }
    let _ = world.insert_one(e, state);
    Ok(e)
}

/// The registry's short name of a part component (the type's last path
/// segment when it is not registered).
fn component_name(world: &World, c: &dyn PartComponent) -> &'static str {
    world
        .registry()
        .component_by_id(c.component_type())
        .map_or_else(|| c.short_name(), |t| t.name())
}

/// The component a field path starts with (`WallGen` of `WallGen.height`).
fn path_target(path: &str) -> &str {
    path.split(['.', '[']).next().unwrap_or(path)
}

/// The field overrides that patch `c`.
fn patch_of<'p>(
    world: &World,
    c: &dyn PartComponent,
    patch: &'p [FieldOverride],
) -> Vec<&'p FieldOverride> {
    if patch.is_empty() {
        return Vec::new();
    }
    let name = component_name(world, c);
    patch
        .iter()
        .filter(|f| path_target(&f.path) == name)
        .collect()
}

/// Warnings for overrides of components the part does not have.
fn patch_missing(
    world: &World,
    components: &[Box<dyn PartComponent>],
    patch: &[FieldOverride],
    warnings: &mut Vec<String>,
) {
    for f in patch {
        let target = path_target(&f.path);
        if !components
            .iter()
            .any(|c| component_name(world, c.as_ref()) == target)
        {
            warnings.push(format!(
                "override `{}`: this part has no `{target}`; the override is kept",
                f.path
            ));
        }
    }
}

/// Writes the overridden values over the part's components; failures
/// become warnings (the rest still apply).
fn apply_patch(world: &mut World, e: Entity, patch: &[&FieldOverride], warnings: &mut Vec<String>) {
    for f in patch {
        if let Err(m) = apply_field(world, e, f) {
            warnings.push(format!(
                "override `{}` could not be applied ({m}); the override is kept",
                f.path
            ));
        }
    }
}

fn apply_field(world: &mut World, e: Entity, f: &FieldOverride) -> Result<(), String> {
    let path = world.field_path(&f.path).map_err(|e| e.to_string())?;
    let current = world.get_field(e, &path).map_err(|e| e.to_string())?;
    let info = current
        .get_represented_type_info()
        .ok_or_else(|| "the field's type is not known".to_owned())?;
    let json: serde_json::Value =
        serde_json::from_str(&f.value).map_err(|e| format!("bad JSON value: {e}"))?;
    let value = world
        .registry()
        .value_from_json(info.type_id(), &json)
        .map_err(|e| e.to_string())?;
    world
        .set_field(e, &path, value.as_ref())
        .map_err(|e| e.to_string())
}

/// Writes what differs; true if anything was written or removed. A
/// component with field overrides is compared with the rule's value it was
/// last patched from (the world holds the patched one) and rewritten and
/// patched again only when that value or the patch changed.
fn update(
    world: &mut World,
    e: Entity,
    part: Part,
    patch: &[FieldOverride],
    patch_warnings: &mut Vec<String>,
) -> bool {
    let mut changed = false;
    if world
        .get::<Transform>(e)
        .is_none_or(|t| *t != part.transform)
    {
        let _ = world.insert_one(e, part.transform);
        changed = true;
    }
    let new_types: Vec<(TypeId, Remover)> = part
        .components
        .iter()
        .map(|c| (c.component_type(), c.remover()))
        .collect();
    let (old_types, had_patch): (Vec<(TypeId, Remover)>, bool) = world
        .get::<PartState>(e)
        .map(|s| (s.types.clone(), !s.base.is_empty()))
        .unwrap_or_default();
    for (ty, remover) in &old_types {
        if !new_types.iter().any(|(t, _)| t == ty) {
            remover.remove(world, e);
            changed = true;
        }
    }
    let same_types = old_types.len() == new_types.len()
        && old_types
            .iter()
            .all(|(t, _)| new_types.iter().any(|(n, _)| n == t));
    if patch.is_empty() && !had_patch {
        // No overrides: compare with the world, the common path.
        for c in part.components {
            if c.differs(world, e) {
                c.insert(world, e);
                changed = true;
            }
        }
        if !same_types {
            let _ = world.insert_one(
                e,
                PartState {
                    types: new_types,
                    ..PartState::default()
                },
            );
        }
        return changed;
    }
    patch_missing(world, &part.components, patch, patch_warnings);
    // Taken out of the component in place (no archetype move), put back
    // below.
    let mut state = world
        .get_mut::<PartState>(e)
        .map(|mut s| std::mem::take(&mut *s))
        .unwrap_or_default();
    let same_patch = state.patch == patch;
    let mut old_base = std::mem::take(&mut state.base);
    let mut base = Vec::new();
    let mut failed = Vec::new();
    let mut repatched = false;
    for c in part.components {
        let patched = patch_of(world, c.as_ref(), patch);
        if patched.is_empty() {
            if c.differs(world, e) {
                c.insert(world, e);
                changed = true;
            }
            continue;
        }
        let kept = same_patch
            .then(|| old_base.iter().position(|b| b.same(c.as_ref())))
            .flatten();
        match kept {
            // The rule's value and the patch are as last applied.
            Some(i) => base.push(old_base.swap_remove(i)),
            None => {
                base.push(c.boxed_clone());
                c.insert(world, e);
                apply_patch(world, e, &patched, &mut failed);
                changed = true;
                repatched = true;
            }
        }
    }
    if repatched || base.is_empty() {
        state.patch_warnings = failed;
    }
    if base.is_empty() {
        state.patch.clear();
    } else if !same_patch {
        state.patch = patch.to_vec();
    }
    state.base = base;
    state.types = new_types;
    patch_warnings.extend(state.patch_warnings.iter().cloned());
    let missing = match world.get_mut::<PartState>(e) {
        Some(mut s) => {
            *s = state;
            None
        }
        None => Some(state),
    };
    if let Some(state) = missing {
        let _ = world.insert_one(e, state);
    }
    changed
}

/// Replaces `rule`'s entries of the owner's warnings; logs the new ones.
/// Returns true if they changed; `count` gets the rule's warning count.
fn set_warnings(
    world: &mut World,
    owner: Entity,
    rule: &str,
    mut new: Vec<GeneratedWarning>,
    count: &mut usize,
    log: bool,
) -> bool {
    new.sort();
    new.dedup();
    *count = new.len();
    let current = world
        .get::<GeneratedWarnings>(owner)
        .map(|w| w.0.clone())
        .unwrap_or_default();
    let (mine, mut others): (Vec<_>, Vec<_>) = current.into_iter().partition(|w| w.rule == rule);
    if mine == new {
        return false;
    }
    if log && !world.has::<Preview>(owner) {
        let who = owner_label(world, owner);
        for w in new.iter().filter(|w| !mine.contains(w)) {
            tracing::warn!("{who}: {w}");
        }
    }
    others.extend(new);
    others.sort();
    if others.is_empty() {
        let _ = world.remove_one::<GeneratedWarnings>(owner);
    } else {
        let _ = world.insert_one(owner, GeneratedWarnings(others));
    }
    true
}

fn owner_label(world: &World, owner: Entity) -> String {
    match world.get::<Name>(owner) {
        Some(n) if !n.as_str().is_empty() => n.as_str().to_owned(),
        _ => world
            .uid(owner)
            .map_or_else(|| format!("{owner:?}"), |u: Uid| u.to_string()),
    }
}

/// The derived parts of `owner` (any rule), as (entity, marker), in child
/// order. For inspection and tests.
pub fn parts_of(world: &World, owner: Entity) -> Vec<(Entity, Derived)> {
    world
        .children(owner)
        .into_iter()
        .filter_map(|c| {
            let d: Derived = world.get::<Derived>(c).map(|d| (*d).clone())?;
            (d.owner == owner).then_some((c, d))
        })
        .collect()
}
