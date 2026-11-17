//! Generic world operations by name and path. No per-type code: everything goes
//! through the registry.

use std::collections::HashMap;

use bevy_reflect::std_traits::ReflectDefault;
use bevy_reflect::{GetPath, PartialReflect, Reflect};
use box_core::world::{Entity, World};

use crate::components::{Name, Uid};
use crate::registry::Registry;
use crate::{Error, Result};

/// `Uid` to `Entity` lookup, kept as a world resource.
#[derive(Default)]
pub struct UidIndex(pub HashMap<Uid, Entity>);

pub fn spawn_with_uid(world: &mut World, uid: Uid, name: Option<&str>) -> Entity {
    let e = world.spawn((uid,));
    if let Some(n) = name {
        let _ = world.insert_one(e, Name(n.to_string()));
    }
    world.resource_or_default::<UidIndex>().0.insert(uid, e);
    e
}

pub fn entity(world: &World, uid: Uid) -> Result<Entity> {
    world
        .get_resource::<UidIndex>()
        .and_then(|i| i.0.get(&uid).copied())
        .filter(|e| world.contains(*e))
        .ok_or(Error::NoUid(uid.0))
}

pub fn uid_of(world: &World, e: Entity) -> Option<Uid> {
    world.get::<Uid>(e).map(|u| *u)
}

/// Despawn an entity and its subtree, keeping the index consistent.
pub fn despawn(world: &mut World, uid: Uid) -> Result<()> {
    let root = entity(world, uid)?;
    let mut stack = vec![root];
    let mut uids = Vec::new();
    while let Some(e) = stack.pop() {
        uids.extend(uid_of(world, e));
        stack.extend(world.children(e));
    }
    world.despawn(root);
    let index = world.resource_or_default::<UidIndex>();
    for u in uids {
        index.0.remove(&u);
    }
    Ok(())
}

/// Names of registered components on an entity, sorted.
pub fn list_components(world: &World, registry: &Registry, e: Entity) -> Vec<&'static str> {
    let Ok(entity) = world.ecs().entity(e) else {
        return Vec::new();
    };
    let mut names: Vec<&'static str> = entity
        .component_types()
        .filter_map(|id| registry.types.get(id))
        .filter(|r| r.data::<crate::registry::ReflectComponent>().is_some())
        .map(|r| r.type_info().type_path_table().short_path())
        .collect();
    names.sort_unstable();
    names
}

fn split_path(path: &str) -> (&str, &str) {
    match path.split_once('.') {
        Some((c, f)) => (c, f),
        None => (path, ""),
    }
}

fn path_err(path: &str, e: impl std::fmt::Display) -> Error {
    Error::Path {
        path: path.to_string(),
        message: e.to_string(),
    }
}

/// Read a field (or the whole component when the path has no field part) and
/// return an owned copy of it (concrete when possible, dynamic otherwise).
pub fn get_field(
    world: &World,
    registry: &Registry,
    uid: Uid,
    path: &str,
) -> Result<Box<dyn PartialReflect>> {
    let (comp, field) = split_path(path);
    let ty = registry
        .component(comp)
        .ok_or_else(|| Error::UnknownComponent(comp.into()))?;
    let e = entity(world, uid)?;
    let root = (ty.data.get)(world, e).ok_or_else(|| Error::Missing(comp.into()))?;
    let target: &dyn PartialReflect = if field.is_empty() {
        root.as_partial_reflect()
    } else {
        root.reflect_path(field)
            .map_err(|err| path_err(path, err))?
    };
    clone_value(target).map_err(|err| path_err(path, err))
}

/// Owned copy of a reflected value. `reflect_clone` is not implemented for every
/// type (fixed-size arrays and anything containing one fail in 0.20.1, and arrays
/// get no `ReflectFromReflect`), so fall back to a dynamic copy.
pub fn clone_value(
    value: &dyn PartialReflect,
) -> Result<Box<dyn PartialReflect>, bevy_reflect::ReflectCloneError> {
    match value.reflect_clone() {
        Ok(v) => Ok(v.into_partial_reflect()),
        Err(_) => value.to_dynamic(),
    }
}

/// Make `target` equal to `value`, structurally.
///
/// `try_apply` is not a replace: on a list it never shrinks (extra elements
/// survive), and `Reflect::set` fails on `#[reflect_remote]` wrappers and needs a
/// concrete value. This walks both values and truncates lists on the way, then
/// lets `try_apply` handle the leaves and enum variant switches.
pub fn replace(
    target: &mut dyn PartialReflect,
    value: &dyn PartialReflect,
) -> Result<(), bevy_reflect::ApplyError> {
    use bevy_reflect::{ReflectMut as M, ReflectRef as R};
    match (target.reflect_mut(), value.reflect_ref()) {
        (M::Struct(t), R::Struct(v)) => {
            for i in 0..v.field_len() {
                let name = v.name_at(i).unwrap_or_default();
                match t.field_mut(name) {
                    Some(f) => replace(f, v.field_at(i).unwrap())?,
                    None => {
                        return Err(bevy_reflect::ApplyError::MissingEnumField {
                            variant_name: "<struct>".into(),
                            field_name: name.to_string().into(),
                        });
                    }
                }
            }
            Ok(())
        }
        (M::TupleStruct(t), R::TupleStruct(v)) => {
            for i in 0..v.field_len().min(t.field_len()) {
                replace(t.field_mut(i).unwrap(), v.field(i).unwrap())?;
            }
            Ok(())
        }
        (M::Tuple(t), R::Tuple(v)) => {
            for i in 0..v.field_len().min(t.field_len()) {
                replace(t.field_mut(i).unwrap(), v.field(i).unwrap())?;
            }
            Ok(())
        }
        (M::Array(t), R::Array(v)) if t.len() == v.len() => {
            for i in 0..v.len() {
                replace(t.get_mut(i).unwrap(), v.get(i).unwrap())?;
            }
            Ok(())
        }
        (M::List(t), R::List(v)) => {
            while t.len() > v.len() {
                t.pop();
            }
            for i in 0..v.len() {
                if i < t.len() {
                    replace(t.get_mut(i).unwrap(), v.get(i).unwrap())?;
                } else {
                    t.push(v.get(i).unwrap().to_dynamic().map_err(|_| {
                        bevy_reflect::ApplyError::DifferentSize {
                            from_size: v.len(),
                            to_size: t.len(),
                        }
                    })?);
                }
            }
            Ok(())
        }
        (M::Enum(t), R::Enum(v)) if t.variant_name() == v.variant_name() => {
            for i in 0..v.field_len().min(t.field_len()) {
                replace(t.field_at_mut(i).unwrap(), v.field_at(i).unwrap())?;
            }
            Ok(())
        }
        // Different enum variant (builds the new one from `value`), opaque
        // values, maps and sets. Maps/sets would need the same "remove extras"
        // treatment as lists; no spike component has one.
        _ => target.try_apply(value),
    }
}

fn represented(v: &dyn PartialReflect) -> Option<std::any::TypeId> {
    v.get_represented_type_info().map(|i| i.type_id())
}

/// Replace a field (or a whole component) with `value`, which may be concrete
/// or dynamic.
pub fn set_field(
    world: &mut World,
    registry: &Registry,
    uid: Uid,
    path: &str,
    value: &dyn PartialReflect,
) -> Result<()> {
    let (comp, field) = split_path(path);
    let ty = registry
        .component(comp)
        .ok_or_else(|| Error::UnknownComponent(comp.into()))?;
    let e = entity(world, uid)?;
    let root = (ty.data.get_mut)(world, e).ok_or_else(|| Error::Missing(comp.into()))?;
    let target: &mut dyn PartialReflect = if field.is_empty() {
        root.as_partial_reflect_mut()
    } else {
        root.reflect_path_mut(field)
            .map_err(|err| path_err(path, err))?
    };
    if represented(target) != represented(value) {
        return Err(Error::Shape {
            expected: target.reflect_type_path().to_string(),
            got: value.reflect_type_path().to_string(),
        });
    }
    replace(target, value).map_err(|e| path_err(path, e))
}

/// Add a component with its default value.
pub fn add_default(world: &mut World, registry: &Registry, uid: Uid, name: &str) -> Result<()> {
    let ty = registry
        .component(name)
        .ok_or_else(|| Error::UnknownComponent(name.into()))?;
    let default = ty
        .registration
        .data::<ReflectDefault>()
        .ok_or_else(|| Error::NoDefault(name.into()))?
        .default();
    insert(world, registry, uid, name, default.as_partial_reflect())
}

/// Insert a component from a reflected value. Fails if it is already present.
pub fn insert(
    world: &mut World,
    registry: &Registry,
    uid: Uid,
    name: &str,
    value: &dyn PartialReflect,
) -> Result<()> {
    let ty = registry
        .component(name)
        .ok_or_else(|| Error::UnknownComponent(name.into()))?;
    let e = entity(world, uid)?;
    if (ty.data.contains)(world, e) {
        return Err(Error::AlreadyPresent(name.into()));
    }
    (ty.data.insert)(world, e, value)
}

pub fn remove(
    world: &mut World,
    registry: &Registry,
    uid: Uid,
    name: &str,
) -> Result<Box<dyn Reflect>> {
    let ty = registry
        .component(name)
        .ok_or_else(|| Error::UnknownComponent(name.into()))?;
    let e = entity(world, uid)?;
    (ty.data.remove)(world, e).ok_or_else(|| Error::Missing(name.into()))
}

/// True if `ancestor` is `e` or one of its ancestors.
pub fn is_ancestor_or_self(world: &World, ancestor: Entity, e: Entity) -> bool {
    let mut cur = Some(e);
    while let Some(c) = cur {
        if c == ancestor {
            return true;
        }
        cur = world.parent(c);
    }
    false
}

/// Where an entity sits in the hierarchy. Roots are ordered by `Uid` in scene
/// files, so `index` is ignored for them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub parent: Option<Uid>,
    pub index: usize,
}

pub fn placement(world: &World, e: Entity) -> Placement {
    match world.parent(e) {
        Some(p) => Placement {
            parent: uid_of(world, p),
            index: world.children(p).iter().position(|c| *c == e).unwrap_or(0),
        },
        None => Placement {
            parent: None,
            index: 0,
        },
    }
}

/// Move `e` to `placement`, preserving its local transform.
pub fn place(world: &mut World, e: Entity, placement: Placement) -> Result<()> {
    let parent = match placement.parent {
        Some(p) => Some(entity(world, p)?),
        None => None,
    };
    if let Some(p) = parent
        && is_ancestor_or_self(world, e, p)
    {
        return Err(Error::Cycle);
    }
    world.set_parent(e, parent);
    if let Some(p) = parent
        && let Some(mut children) = world.get_mut::<box_core::transform::Children>(p)
    {
        let v = &mut children.0;
        if let Some(pos) = v.iter().position(|c| *c == e) {
            v.remove(pos);
            v.insert(placement.index.min(v.len()), e);
        }
    }
    Ok(())
}
