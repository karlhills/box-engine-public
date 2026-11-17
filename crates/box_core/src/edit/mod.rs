//! The undoable command layer: every edit to an edit world goes through
//! [`EditStack`]. No UI dependencies, so the editor, the `box` CLI and a future
//! MCP server share it.
//!
//! - Edits address entities by persistent [`Uid`], so they survive despawn and respawn.
//! - Values are reflected values, so new component types need no undo code.
//! - A transaction ([`EditStack::begin`] .. [`EditStack::commit`]) is one undo
//!   step. Inside one, successive `set_field` calls on the same entity and path
//!   merge into a single edit (gizmo and slider drags), also when sets of the
//!   same kind on other entities come in between (multi-object drags).
//! - Spawn and despawn snapshots reuse the scene serializer, so unknown
//!   components survive delete and undo.
//! - Reparenting keeps the world transform by default ([`Keep`]).
//! - Undo, redo and rollback are all-or-nothing: if an edit cannot be applied
//!   (the world was changed behind the stack's back), the world and the stack
//!   are left as they were and the error is returned.
//! - An edit that changes nothing records nothing. Values containing NaN or
//!   an infinity are rejected ([`EditError::Invalid`]); scene files cannot
//!   hold them.
//! - Clear the stack when a scene is loaded, not on Play.
//!
//! JSON commands for `box scene apply` are in [`command`].

pub mod command;

use std::collections::HashSet;

use bevy_reflect::PartialReflect;
use serde_json::Value as Json;

use crate::identity::{Name, Uid};
use crate::prefab::is_generated;
use crate::reflect::{FieldPath, ReflectError, ReflectRef, Value, clone_value};
use crate::scene::{self, SceneError, UnknownComponents, UnknownResources};
use crate::transform::{Transform, compute_global};
use crate::world::{Entity, Placement, World};
use glam::Affine3A;

pub use command::{
    Applied, Command, CommandError, CommandParseError, apply_commands,
    apply_commands_with_warnings, parse_commands, resolve_entity,
};

#[derive(Debug, thiserror::Error)]
pub enum EditError {
    #[error("no entity with uid {0}")]
    NoEntity(Uid),
    #[error("no entity named `{0}`")]
    UnknownEntity(String),
    #[error("{count} entities are named `{name}`; use a uid")]
    AmbiguousName { name: String, count: usize },
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Reflect(#[from] ReflectError),
    #[error(transparent)]
    Scene(#[from] SceneError),
}

/// A subtree serialized as scene records (root first) plus where its root sat.
#[derive(Clone, Debug, PartialEq)]
pub struct EntitySnapshot {
    pub uid: Uid,
    pub placement: Placement,
    pub records: Vec<Json>,
}

/// One reversible change.
#[derive(Debug)]
pub enum Edit {
    /// A component field (or a whole component; `Name` for renames).
    SetField {
        uid: Uid,
        path: FieldPath,
        old: Value,
        new: Value,
    },
    /// A resource field (or a whole resource), e.g. `Environment.fog.density`.
    SetResource {
        path: FieldPath,
        old: Value,
        new: Value,
    },
    AddComponent {
        uid: Uid,
        component: &'static str,
        data: Value,
    },
    RemoveComponent {
        uid: Uid,
        component: &'static str,
        data: Value,
    },
    Spawn {
        snapshot: EntitySnapshot,
    },
    Despawn {
        snapshot: EntitySnapshot,
    },
    Reparent {
        uid: Uid,
        old: Placement,
        new: Placement,
    },
    /// A component kept as raw JSON ([`UnknownComponents`]): a schema-only
    /// type (`crate::reflect::SchemaType`). `None` is absent.
    SetRawComponent {
        uid: Uid,
        component: String,
        old: Option<Json>,
        new: Option<Json>,
    },
    /// A resource kept as raw JSON ([`UnknownResources`]).
    SetRawResource {
        name: String,
        old: Option<Json>,
        new: Option<Json>,
    },
}

/// Writes (or with `None` removes) a raw component of `e`.
fn write_raw_component(world: &mut World, e: Entity, component: &str, value: Option<&Json>) {
    let mut map = world
        .get::<UnknownComponents>(e)
        .map(|u| u.0.clone())
        .unwrap_or_default();
    match value {
        Some(v) => {
            map.insert(component.to_string(), v.clone());
        }
        None => {
            map.remove(component);
        }
    }
    if map.is_empty() {
        let _ = world.remove_one::<UnknownComponents>(e);
    } else {
        let _ = world.insert_one(e, UnknownComponents(map));
    }
}

/// Writes (or with `None` removes) a raw resource.
fn write_raw_resource(world: &mut World, name: &str, value: Option<&Json>) {
    let mut map = world
        .get_resource::<UnknownResources>()
        .map(|u| u.0.clone())
        .unwrap_or_default();
    match value {
        Some(v) => {
            map.insert(name.to_string(), v.clone());
        }
        None => {
            map.remove(name);
        }
    }
    if map.is_empty() {
        world.remove_resource::<UnknownResources>();
    } else {
        world.insert_resource(UnknownResources(map));
    }
}

fn entity(world: &World, uid: Uid) -> Result<Entity, EditError> {
    world.entity_by_uid(uid).ok_or(EditError::NoEntity(uid))
}

/// [`entity`] for a new edit: generated entities (instantiated from a
/// prefab, or derived by a rule) are runtime data rebuilt from their source,
/// so they cannot be edited.
fn editable(world: &World, uid: Uid) -> Result<Entity, EditError> {
    let e = entity(world, uid)?;
    if is_generated(world, e) {
        return Err(generated_error(world, uid, e));
    }
    Ok(e)
}

fn generated_error(world: &World, uid: Uid, e: Entity) -> EditError {
    let root = crate::prefab::editable_ancestor(world, e);
    if let Some(d) = world.get::<crate::derived::Derived>(e) {
        let owner = world
            .uid(root)
            .map_or_else(|| "its owner".to_string(), |u| u.to_string());
        return EditError::Invalid(format!(
            "entity {uid} was generated by rule `{}` (part `{}`) and is rebuilt from its owner's intent; edit the owner {owner} instead",
            d.rule,
            d.key.role()
        ));
    }
    let root = world
        .uid(root)
        .map_or_else(|| "its instance root".to_string(), |u| u.to_string());
    EditError::Invalid(format!(
        "entity {uid} is part of a prefab instance and is rebuilt from its source scene; edit the instance root {root} (its PrefabInstance, Name or Transform) or the source scene instead"
    ))
}

/// Refuse placing an entity under a generated entity: it would be despawned
/// with the generated content when its source changes.
fn check_parent(world: &World, parent: Option<Uid>) -> Result<(), EditError> {
    if let Some(p) = parent
        && let Some(e) = world.entity_by_uid(p)
        && is_generated(world, e)
    {
        return Err(generated_error(world, p, e));
    }
    Ok(())
}

/// What [`EditStack::reparent_keeping`] holds fixed when an entity moves to a
/// new parent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Keep {
    /// The world transform: the local `Transform` is recomputed against the
    /// new parent. The world position is kept exactly (up to float rounding).
    /// Orientation and scale are exact unless the new parent chain has
    /// non-uniform scale combined with a rotation of the child; that would
    /// need shear, which `Transform` cannot hold, so the nearest
    /// scale-rotation is used. The local transform is kept instead when the
    /// entity has no `Transform` (its children then move), when the new
    /// parent's world transform is singular (a zero scale), or when the old
    /// and new parents have the same world transform.
    #[default]
    World,
    /// The local `Transform` as-is, so the entity moves with its new parent.
    Local,
}

/// The local transform that keeps `e`'s current world transform under
/// `new_parent`, or `None` when the local transform should stay (see [`Keep::World`]).
fn local_keeping_world(world: &World, e: Entity, new_parent: Option<Entity>) -> Option<Transform> {
    world.get::<Transform>(e)?;
    let parent_global =
        |p: Option<Entity>| p.map_or(Affine3A::IDENTITY, |p| compute_global(world, p));
    let new_parent_global = parent_global(new_parent);
    if parent_global(world.parent(e)) == new_parent_global {
        return None;
    }
    let det = new_parent_global.matrix3.determinant();
    if det == 0.0 || !det.is_finite() {
        return None;
    }
    let local = new_parent_global.inverse() * compute_global(world, e);
    let (scale, rotation, translation) = local.to_scale_rotation_translation();
    let t = Transform {
        translation,
        rotation: rotation.normalize(),
        scale,
    };
    (t.translation.is_finite() && t.rotation.is_finite() && t.scale.is_finite()).then_some(t)
}

/// False if `value` holds a NaN or an infinity anywhere inside it.
fn is_finite(value: &dyn PartialReflect) -> bool {
    match value.reflect_ref() {
        ReflectRef::Struct(s) => s.iter_fields().all(|(_, f)| is_finite(f)),
        ReflectRef::TupleStruct(s) => s.iter_fields().all(is_finite),
        ReflectRef::Tuple(s) => s.iter_fields().all(is_finite),
        ReflectRef::List(s) => s.iter().all(is_finite),
        ReflectRef::Array(s) => s.iter().all(is_finite),
        ReflectRef::Enum(s) => s.iter_fields().all(|f| is_finite(f.value())),
        ReflectRef::Map(s) => s.iter().all(|(_, v)| is_finite(v)),
        _ => {
            value
                .try_downcast_ref::<f32>()
                .is_none_or(|v| v.is_finite())
                && value
                    .try_downcast_ref::<f64>()
                    .is_none_or(|v| v.is_finite())
        }
    }
}

/// Reject a value for `target` (a path or component name) that is not finite.
fn check_finite(
    target: &dyn std::fmt::Display,
    value: &dyn PartialReflect,
) -> Result<(), EditError> {
    if is_finite(value) {
        return Ok(());
    }
    Err(EditError::Invalid(format!(
        "value for `{target}` contains a non-finite number (NaN or infinity)"
    )))
}

/// [`check_finite`] for every registered component in `root`'s subtree.
fn check_subtree_finite(world: &World, root: Entity) -> Result<(), EditError> {
    let mut stack = vec![root];
    while let Some(e) = stack.pop() {
        for name in world.component_names(e) {
            check_finite(&name, world.get_reflect(e, name)?.as_partial_reflect())?;
        }
        stack.extend(world.children(e));
    }
    Ok(())
}

/// Apply `edits` in order (`forward`) or in reverse order, backward. All or
/// nothing: if one fails, those already applied are reverted again.
fn apply_all(world: &mut World, edits: &[Edit], forward: bool) -> Result<(), EditError> {
    let nth = |k: usize| &edits[if forward { k } else { edits.len() - 1 - k }];
    for k in 0..edits.len() {
        if let Err(err) = nth(k).apply(world, forward) {
            for done in (0..k).rev() {
                // The inverse of an edit that was applied a moment ago.
                let _ = nth(done).apply(world, !forward);
            }
            return Err(err);
        }
    }
    Ok(())
}

/// Set `e`'s whole `Transform` and return the edit that records it.
fn set_transform(world: &mut World, e: Entity, uid: Uid, t: &Transform) -> Result<Edit, EditError> {
    let path = world.field_path("Transform")?;
    let old = world.get_field(e, &path)?;
    world.set_field(e, &path, t)?;
    let new = world.get_field(e, &path)?;
    Ok(Edit::SetField {
        uid,
        path,
        old,
        new,
    })
}

/// The `new` value of the open transaction's latest edit to `uid`, if that
/// edit is a `SetField` of the same `path`, so a further set of that field
/// can merge into it. Only `SetField`s of other entities may come after it:
/// they commute with it, so a drag over several entities (each frame sets
/// every entity once) stays one edit per entity instead of growing every
/// frame. Any other edit after it (structural, or a resource) stops the
/// search, and so does an edit of another field of `uid`.
fn mergeable<'a>(edits: &'a mut [Edit], uid: Uid, path: &FieldPath) -> Option<&'a mut Value> {
    for edit in edits.iter_mut().rev() {
        match edit {
            Edit::SetField {
                uid: u0,
                path: p0,
                new,
                ..
            } => {
                if *u0 != uid {
                    continue;
                }
                return (p0 == path).then_some(new);
            }
            _ => return None,
        }
    }
    None
}

/// True if an edit from `old` to `new` changed nothing.
fn unchanged(old: &dyn PartialReflect, new: &dyn PartialReflect) -> bool {
    old.reflect_partial_eq(new) == Some(true)
}

impl Edit {
    /// Apply forward (`true`, redo) or backward (`false`, undo).
    pub fn apply(&self, world: &mut World, forward: bool) -> Result<(), EditError> {
        match self {
            Edit::SetField {
                uid,
                path,
                old,
                new,
            } => {
                let e = entity(world, *uid)?;
                let v = if forward { new } else { old };
                world.set_field(e, path, v.as_ref())?;
            }
            Edit::SetResource { path, old, new } => {
                let v = if forward { new } else { old };
                world.set_resource_field(path, v.as_ref())?;
            }
            Edit::AddComponent {
                uid,
                component,
                data,
            }
            | Edit::RemoveComponent {
                uid,
                component,
                data,
            } => {
                let e = entity(world, *uid)?;
                if matches!(self, Edit::AddComponent { .. }) == forward {
                    world.insert_reflect(e, component, data.as_ref())?;
                } else {
                    world.remove_reflect(e, component)?;
                }
            }
            Edit::Spawn { snapshot } | Edit::Despawn { snapshot } => {
                if matches!(self, Edit::Spawn { .. }) == forward {
                    scene::instantiate(world, &snapshot.records, Some(snapshot.placement))?;
                } else {
                    let e = entity(world, snapshot.uid)?;
                    world.despawn(e);
                }
            }
            Edit::Reparent { uid, old, new } => {
                let e = entity(world, *uid)?;
                world.place(e, if forward { *new } else { *old })?;
            }
            Edit::SetRawComponent {
                uid,
                component,
                old,
                new,
            } => {
                let e = entity(world, *uid)?;
                let v = if forward { new } else { old };
                write_raw_component(world, e, component, v.as_ref());
            }
            Edit::SetRawResource { name, old, new } => {
                let v = if forward { new } else { old };
                write_raw_resource(world, name, v.as_ref());
            }
        }
        Ok(())
    }
}

/// A labelled group of edits: one undo step.
#[derive(Debug, Default)]
pub struct Transaction {
    pub label: String,
    pub edits: Vec<Edit>,
}

/// Undo and redo stacks plus the open transaction. The world is passed to each
/// call, so the stack can live next to the world rather than own it.
#[derive(Debug, Default)]
pub struct EditStack {
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
    open: Option<Transaction>,
    /// The redo stack as it was before the open transaction's first edit:
    /// dropped when the transaction commits, restored when it is rolled back.
    shelved_redo: Vec<Transaction>,
}

impl EditStack {
    pub fn new() -> Self {
        Self::default()
    }

    // ----- transactions -----

    /// Open a transaction (for a drag or a multi-step command). Commits any
    /// transaction that is already open.
    pub fn begin(&mut self, label: impl Into<String>) {
        self.commit();
        self.open = Some(Transaction {
            label: label.into(),
            edits: Vec::new(),
        });
    }

    /// Close the open transaction. Returns true if it recorded an undo step
    /// (empty transactions are dropped).
    pub fn commit(&mut self) -> bool {
        match self.open.take() {
            Some(t) if !t.edits.is_empty() => {
                self.shelved_redo.clear();
                self.undo.push(t);
                true
            }
            _ => false,
        }
    }

    /// Revert and drop the open transaction; redo history is as it was before
    /// the transaction. On error nothing is reverted and the transaction
    /// stays open.
    pub fn rollback(&mut self, world: &mut World) -> Result<(), EditError> {
        let Some(t) = self.open.take() else {
            return Ok(());
        };
        if let Err(err) = apply_all(world, &t.edits, false) {
            self.open = Some(t);
            return Err(err);
        }
        self.redo.append(&mut self.shelved_redo);
        Ok(())
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Undo the last step (committing an open transaction first). Returns
    /// false if there was nothing to undo. On error the world is unchanged
    /// and the step stays on the undo stack.
    pub fn undo(&mut self, world: &mut World) -> Result<bool, EditError> {
        self.commit();
        let Some(t) = self.undo.pop() else {
            return Ok(false);
        };
        if let Err(err) = apply_all(world, &t.edits, false) {
            self.undo.push(t);
            return Err(err);
        }
        self.redo.push(t);
        Ok(true)
    }

    /// Redo the last undone step. Returns false if there was nothing to redo.
    /// On error the world is unchanged and the step stays on the redo stack.
    pub fn redo(&mut self, world: &mut World) -> Result<bool, EditError> {
        self.commit();
        let Some(t) = self.redo.pop() else {
            return Ok(false);
        };
        if let Err(err) = apply_all(world, &t.edits, true) {
            self.redo.push(t);
            return Err(err);
        }
        self.undo.push(t);
        Ok(true)
    }

    /// Forget all history (on scene load).
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.shelved_redo.clear();
        self.open = None;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty() || self.open.as_ref().is_some_and(|t| !t.edits.is_empty())
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Committed undo steps.
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }

    /// Label of the step `undo` would revert (for "Undo Move" menu items).
    pub fn undo_label(&self) -> Option<&str> {
        match &self.open {
            Some(t) if !t.edits.is_empty() => Some(&t.label),
            _ => self.undo.last().map(|t| t.label.as_str()),
        }
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|t| t.label.as_str())
    }

    /// Labels of the committed undo steps, oldest first (for a history list).
    pub fn undo_labels(&self) -> impl DoubleEndedIterator<Item = &str> + ExactSizeIterator {
        self.undo.iter().map(|t| t.label.as_str())
    }

    /// Labels of the redo steps, the one `redo` would apply first.
    pub fn redo_labels(&self) -> impl DoubleEndedIterator<Item = &str> + ExactSizeIterator {
        self.redo.iter().rev().map(|t| t.label.as_str())
    }

    // ----- recording -----

    fn record(&mut self, label: &str, edit: Edit) {
        let Some(t) = &mut self.open else {
            self.redo.clear();
            self.undo.push(Transaction {
                label: label.into(),
                edits: vec![edit],
            });
            return;
        };
        // Not cleared yet: a rollback brings it back.
        self.shelved_redo.append(&mut self.redo);
        if let Edit::SetField { uid, path, new, .. } = &edit
            && let Some(n0) = mergeable(&mut t.edits, *uid, path)
        {
            // Drag merging: keep the first `old`, take the latest `new`.
            if let Ok(v) = clone_value(new.as_ref()) {
                *n0 = v;
                return;
            }
        }
        t.edits.push(edit);
    }

    /// Apply a prepared edit forward and record it. This is the entry point for
    /// plugin tools that build their own [`Edit`]s.
    pub fn apply(&mut self, world: &mut World, label: &str, edit: Edit) -> Result<(), EditError> {
        match &edit {
            Edit::SetField { path, new, .. } | Edit::SetResource { path, new, .. } => {
                check_finite(path, new.as_ref())?;
            }
            Edit::AddComponent {
                component, data, ..
            } => check_finite(component, data.as_ref())?,
            _ => {}
        }
        match &edit {
            Edit::SetField { uid, .. }
            | Edit::AddComponent { uid, .. }
            | Edit::RemoveComponent { uid, .. }
            | Edit::SetRawComponent { uid, .. } => {
                editable(world, *uid)?;
            }
            Edit::Reparent { uid, new, .. } => {
                editable(world, *uid)?;
                check_parent(world, new.parent)?;
            }
            Edit::Despawn { snapshot } => {
                editable(world, snapshot.uid)?;
            }
            Edit::Spawn { snapshot } => check_parent(world, snapshot.placement.parent)?,
            Edit::SetResource { .. } | Edit::SetRawResource { .. } => {}
        }
        edit.apply(world, true)?;
        if let Edit::Spawn { snapshot } = &edit
            && let Err(err) =
                entity(world, snapshot.uid).and_then(|e| check_subtree_finite(world, e))
        {
            let _ = edit.apply(world, false);
            return Err(err);
        }
        self.record(label, edit);
        Ok(())
    }

    // ----- edits -----

    /// Set a component field (or a whole component). Nothing is recorded if
    /// the field already has that value.
    pub fn set_field(
        &mut self,
        world: &mut World,
        uid: Uid,
        path: &FieldPath,
        value: &dyn PartialReflect,
    ) -> Result<(), EditError> {
        if path.is_resource() {
            return Err(ReflectError::NotAComponent(path.target().to_string()).into());
        }
        check_finite(path, value)?;
        let e = editable(world, uid)?;
        let old = world.get_field(e, path)?;
        world.set_field(e, path, value)?;
        let new = world.get_field(e, path)?;
        if unchanged(old.as_ref(), new.as_ref()) {
            return Ok(());
        }
        let edit = Edit::SetField {
            uid,
            path: path.clone(),
            old,
            new,
        };
        self.record(&format!("Set {path}"), edit);
        Ok(())
    }

    /// Set a resource field (or a whole resource). Nothing is recorded if the
    /// field already has that value.
    pub fn set_resource_field(
        &mut self,
        world: &mut World,
        path: &FieldPath,
        value: &dyn PartialReflect,
    ) -> Result<(), EditError> {
        if !path.is_resource() {
            return Err(ReflectError::NotAResource(path.target().to_string()).into());
        }
        check_finite(path, value)?;
        let old = world.get_resource_field(path)?;
        world.set_resource_field(path, value)?;
        let new = world.get_resource_field(path)?;
        if unchanged(old.as_ref(), new.as_ref()) {
            return Ok(());
        }
        let edit = Edit::SetResource {
            path: path.clone(),
            old,
            new,
        };
        self.record(&format!("Set {path}"), edit);
        Ok(())
    }

    /// Add a component with `value`, or its default value when `None`.
    pub fn add_component(
        &mut self,
        world: &mut World,
        uid: Uid,
        component: &str,
        value: Option<&dyn PartialReflect>,
    ) -> Result<(), EditError> {
        let e = editable(world, uid)?;
        let name = world
            .registry()
            .component(component)
            .map(|c| c.name())
            .ok_or_else(|| ReflectError::UnknownType(component.to_string()))?;
        if let Some(v) = value {
            check_finite(&name, v)?;
        }
        match value {
            Some(v) => world.insert_reflect(e, name, v)?,
            None => world.insert_default(e, name)?,
        }
        let data = world.get_field(e, &world.field_path(name)?)?;
        self.record(
            &format!("Add {name}"),
            Edit::AddComponent {
                uid,
                component: name,
                data,
            },
        );
        Ok(())
    }

    pub fn remove_component(
        &mut self,
        world: &mut World,
        uid: Uid,
        component: &str,
    ) -> Result<(), EditError> {
        let e = editable(world, uid)?;
        let name = world
            .registry()
            .component(component)
            .map(|c| c.name())
            .ok_or_else(|| ReflectError::UnknownType(component.to_string()))?;
        if name == "Uid" {
            return Err(ReflectError::ReadOnly("Uid".into()).into());
        }
        let data = world.remove_reflect(e, name)?;
        self.record(
            &format!("Remove {name}"),
            Edit::RemoveComponent {
                uid,
                component: name,
                data,
            },
        );
        Ok(())
    }

    /// Spawn a saved entity with `Uid`, `Name`, the given components and a
    /// default `Transform` unless one is given, at `at`. Nothing is spawned on error.
    pub fn spawn(
        &mut self,
        world: &mut World,
        uid: Uid,
        name: &str,
        at: Placement,
        components: &[(&str, &dyn PartialReflect)],
    ) -> Result<Entity, EditError> {
        for (component, value) in components {
            check_finite(component, *value)?;
        }
        check_parent(world, at.parent)?;
        let e = world.spawn_saved(uid, name, ())?;
        let fill = |world: &mut World| -> Result<(), EditError> {
            for (component, value) in components {
                world.insert_reflect(e, component, *value)?;
            }
            if !world.has::<Transform>(e) {
                world
                    .insert_one(e, Transform::default())
                    .map_err(|_| ReflectError::NoEntity(e))?;
            }
            world.place(e, at)?;
            Ok(())
        };
        if let Err(err) = fill(world) {
            world.despawn(e);
            return Err(err);
        }
        let snapshot = EntitySnapshot {
            uid,
            placement: world.placement(e),
            records: scene::save_subtree(world, e)?,
        };
        self.record(&format!("Create {name}"), Edit::Spawn { snapshot });
        Ok(e)
    }

    /// Spawn scene records as one subtree (paste): the first record is the
    /// root, placed at `at`; every other record must have a parent earlier in
    /// the list. Use [`scene::remap_uids`] first when pasting copies.
    pub fn instantiate(
        &mut self,
        world: &mut World,
        records: &[Json],
        at: Placement,
    ) -> Result<Vec<Entity>, EditError> {
        let uid_of = |r: &Json| r.get("uid").and_then(Json::as_str).and_then(Uid::parse);
        let mut seen = HashSet::with_capacity(records.len());
        let mut root_uid = None;
        for (i, r) in records.iter().enumerate() {
            let uid = uid_of(r)
                .ok_or_else(|| EditError::Invalid(format!("record {i} has no valid uid")))?;
            if i > 0 {
                let parent = r.get("parent").and_then(Json::as_str).and_then(Uid::parse);
                if !parent.is_some_and(|p| seen.contains(&p)) {
                    return Err(EditError::Invalid(format!(
                        "record {i} ({uid}) is not inside the subtree of record 0"
                    )));
                }
            }
            seen.insert(uid);
            root_uid.get_or_insert(uid);
        }
        let root_uid =
            root_uid.ok_or_else(|| EditError::Invalid("no records to instantiate".into()))?;
        check_parent(world, at.parent)?;
        let spawned = scene::instantiate(world, records, Some(at))?;
        let root = *spawned
            .first()
            .ok_or_else(|| EditError::Invalid("no records to instantiate".into()))?;
        if let Err(err) = check_subtree_finite(world, root) {
            world.despawn(root);
            return Err(err);
        }
        let snapshot = EntitySnapshot {
            uid: root_uid,
            placement: world.placement(root),
            records: scene::save_subtree(world, root)?,
        };
        self.record("Paste", Edit::Spawn { snapshot });
        Ok(spawned)
    }

    /// Despawn an entity and its subtree.
    pub fn despawn(&mut self, world: &mut World, uid: Uid) -> Result<(), EditError> {
        let e = editable(world, uid)?;
        let snapshot = EntitySnapshot {
            uid,
            placement: world.placement(e),
            records: scene::save_subtree(world, e)?,
        };
        world.despawn(e);
        self.record("Delete", Edit::Despawn { snapshot });
        Ok(())
    }

    /// Move an entity under `parent` (or to the roots) at sibling `index`
    /// (last when `None`), keeping its world position ([`Keep::World`]). No
    /// step is recorded if nothing moved.
    pub fn reparent(
        &mut self,
        world: &mut World,
        uid: Uid,
        parent: Option<Uid>,
        index: Option<usize>,
    ) -> Result<(), EditError> {
        self.reparent_keeping(world, uid, parent, index, Keep::World)
    }

    /// [`EditStack::reparent`], choosing what stays fixed. With
    /// [`Keep::World`] and a new parent, the step is a `Reparent` plus a
    /// `SetField` on the whole `Transform` (one undo step; undo restores both).
    /// See [`Keep::World`] for the limits.
    pub fn reparent_keeping(
        &mut self,
        world: &mut World,
        uid: Uid,
        parent: Option<Uid>,
        index: Option<usize>,
        keep: Keep,
    ) -> Result<(), EditError> {
        let e = editable(world, uid)?;
        check_parent(world, parent)?;
        let old = world.placement(e);
        // Computed before moving; the new parent's world transform does not
        // depend on `e` unless it is a descendant, which `place` rejects.
        let kept = match (keep, parent) {
            (Keep::Local, _) => None,
            (Keep::World, Some(p)) => {
                let p = world.entity_by_uid(p).ok_or(ReflectError::NoUid(p))?;
                local_keeping_world(world, e, Some(p))
            }
            (Keep::World, None) => local_keeping_world(world, e, None),
        };
        world.place(
            e,
            Placement {
                parent,
                index: index.unwrap_or(usize::MAX),
            },
        )?;
        let new = world.placement(e);
        if old == new {
            return Ok(());
        }
        let transform_edit = match kept {
            Some(t) => match set_transform(world, e, uid, &t) {
                Ok(edit) => Some(edit),
                Err(err) => {
                    let _ = world.place(e, old);
                    return Err(err);
                }
            },
            None => None,
        };
        let opened = !self.is_open();
        if opened {
            self.begin("Reparent");
        }
        self.record("Reparent", Edit::Reparent { uid, old, new });
        if let Some(edit) = transform_edit {
            self.record("Reparent", edit);
        }
        if opened {
            self.commit();
        }
        Ok(())
    }

    /// Set (or with `None` remove) a component kept as raw JSON: a
    /// schema-only type, whose value the caller has validated
    /// (`SchemaType::validate`). Nothing is recorded if it already has that
    /// value.
    pub fn set_raw_component(
        &mut self,
        world: &mut World,
        uid: Uid,
        component: &str,
        value: Option<Json>,
    ) -> Result<(), EditError> {
        let e = editable(world, uid)?;
        let old = world
            .get::<UnknownComponents>(e)
            .and_then(|u| u.0.get(component).cloned());
        if old == value {
            return Ok(());
        }
        write_raw_component(world, e, component, value.as_ref());
        let label = match (&old, &value) {
            (None, Some(_)) => format!("Add {component}"),
            (Some(_), None) => format!("Remove {component}"),
            _ => format!("Set {component}"),
        };
        self.record(
            &label,
            Edit::SetRawComponent {
                uid,
                component: component.to_string(),
                old,
                new: value,
            },
        );
        Ok(())
    }

    /// Set (or with `None` remove) a resource kept as raw JSON (a
    /// schema-only type). Nothing is recorded if it already has that value.
    pub fn set_raw_resource(
        &mut self,
        world: &mut World,
        name: &str,
        value: Option<Json>,
    ) -> Result<(), EditError> {
        let old = world
            .get_resource::<UnknownResources>()
            .and_then(|u| u.0.get(name).cloned());
        if old == value {
            return Ok(());
        }
        write_raw_resource(world, name, value.as_ref());
        self.record(
            &format!("Set {name}"),
            Edit::SetRawResource {
                name: name.to_string(),
                old,
                new: value,
            },
        );
        Ok(())
    }

    /// Rename: a `SetField` on `Name` (or adding `Name` if it is missing).
    pub fn rename(&mut self, world: &mut World, uid: Uid, name: &str) -> Result<(), EditError> {
        let e = editable(world, uid)?;
        let value = Name::new(name);
        if world.has::<Name>(e) {
            let path = world.field_path("Name")?;
            self.set_field(world, uid, &path, &value)
        } else {
            self.add_component(world, uid, "Name", Some(&value))
        }
    }

    /// Copy an entity's subtree with fresh uids, as the next sibling of the
    /// original. References between entities inside the subtree are remapped.
    /// Returns the copy's uid.
    pub fn duplicate(&mut self, world: &mut World, uid: Uid) -> Result<Uid, EditError> {
        let e = editable(world, uid)?;
        let mut records = scene::save_subtree(world, e)?;
        let map = scene::remap_uids(&mut records);
        let new_uid = map.get(&uid).copied().ok_or(EditError::NoEntity(uid))?;
        let at = world.placement(e);
        let at = Placement {
            index: at.index.saturating_add(1),
            ..at
        };
        let spawned = scene::instantiate(world, &records, Some(at))?;
        let root = *spawned.first().ok_or(EditError::NoEntity(new_uid))?;
        let snapshot = EntitySnapshot {
            uid: new_uid,
            placement: world.placement(root),
            records: scene::save_subtree(world, root)?,
        };
        self.record("Duplicate", Edit::Spawn { snapshot });
        Ok(new_uid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_len(stack: &EditStack) -> usize {
        stack.open.as_ref().map_or(0, |t| t.edits.len())
    }

    fn set(stack: &mut EditStack, w: &mut World, uid: Uid, path: &str, v: f32) {
        let p = w.field_path(path).unwrap();
        stack.set_field(w, uid, &p, &v).unwrap();
    }

    #[test]
    fn history_labels_list_undo_oldest_first_and_redo_next_first() {
        let mut w = World::new();
        let u = Uid::from_u128(1);
        w.spawn_saved(u, "E", (Transform::default(),)).unwrap();
        let mut stack = EditStack::new();
        for (i, label) in ["a", "b", "c"].into_iter().enumerate() {
            stack.begin(label);
            set(
                &mut stack,
                &mut w,
                u,
                "Transform.translation.x",
                i as f32 + 1.0,
            );
            stack.commit();
        }
        stack.undo(&mut w).unwrap();
        stack.undo(&mut w).unwrap();
        assert_eq!(stack.undo_labels().collect::<Vec<_>>(), ["a"]);
        assert_eq!(stack.redo_labels().collect::<Vec<_>>(), ["b", "c"]);
    }

    #[test]
    fn a_drag_over_several_entities_keeps_one_edit_per_entity() {
        let mut w = World::new();
        let ids: Vec<Uid> = (1..=3).map(Uid::from_u128).collect();
        for (i, u) in ids.iter().enumerate() {
            w.spawn_saved(*u, &format!("E{i}"), (Transform::default(),))
                .unwrap();
        }
        let before = scene::save(&w).unwrap();
        let mut stack = EditStack::new();
        stack.begin("Move 3 entities");
        for frame in 1..=50 {
            for u in &ids {
                set(
                    &mut stack,
                    &mut w,
                    *u,
                    "Transform.translation.x",
                    frame as f32,
                );
            }
        }
        assert_eq!(open_len(&stack), 3, "merged per entity, not per frame");
        assert!(stack.commit());
        assert!(stack.undo(&mut w).unwrap());
        assert_eq!(scene::save(&w).unwrap(), before);
    }

    #[test]
    fn merging_stops_at_another_field_or_a_structural_edit() {
        let mut w = World::new();
        let (a, b) = (Uid::from_u128(1), Uid::from_u128(2));
        w.spawn_saved(a, "A", (Transform::default(),)).unwrap();
        w.spawn_saved(b, "B", (Transform::default(),)).unwrap();
        let before = scene::save(&w).unwrap();
        let mut stack = EditStack::new();
        stack.begin("Mixed");
        set(&mut stack, &mut w, a, "Transform.translation.x", 1.0);
        set(&mut stack, &mut w, a, "Transform.translation.y", 2.0);
        set(&mut stack, &mut w, a, "Transform.translation.x", 3.0);
        assert_eq!(
            open_len(&stack),
            3,
            "x, y, x: the second x cannot jump over y"
        );
        stack.reparent(&mut w, b, Some(a), None).unwrap();
        let len = open_len(&stack);
        set(&mut stack, &mut w, a, "Transform.translation.x", 4.0);
        assert_eq!(open_len(&stack), len + 1, "no merge across a reparent");
        assert!(stack.commit());
        assert!(stack.undo(&mut w).unwrap());
        assert_eq!(scene::save(&w).unwrap(), before);
    }
}
