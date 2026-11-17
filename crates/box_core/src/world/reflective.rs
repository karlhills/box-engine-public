//! Reflective world access by component name and field path, and hierarchy
//! placement. Used by the inspector, scene files, undo and the CLI.

use bevy_reflect::{PartialReflect, Reflect, ReflectPath};

use super::World;
use crate::identity::Uid;
use crate::reflect::{
    ComponentRef, ComponentType, FieldPath, ReflectError, Registry, ResourceType, Value,
    clone_value, replace, represented_type,
};
use crate::transform::Children;
use crate::world::Entity;

/// Where an entity sits in the hierarchy: its parent's uid (`None` for a root)
/// and its index among its siblings (or among the roots).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub parent: Option<Uid>,
    pub index: usize,
}

impl Placement {
    /// Last child of `parent` (or last root).
    pub fn last(parent: Option<Uid>) -> Self {
        Self {
            parent,
            index: usize::MAX,
        }
    }
}

fn path_err(path: &FieldPath, e: impl std::fmt::Display) -> ReflectError {
    ReflectError::Path {
        path: path.to_string(),
        message: e.to_string(),
    }
}

/// A path that did not resolve: names the missing field and lists the ones
/// that exist when that is the problem, else bevy's own message.
fn resolve_err(registry: &Registry, path: &FieldPath, e: impl std::fmt::Display) -> ReflectError {
    match registry.explain_missing_field(path) {
        Some(message) => ReflectError::Path {
            path: path.to_string(),
            message,
        },
        None => path_err(path, e),
    }
}

fn check_shape(
    target: &dyn PartialReflect,
    value: &dyn PartialReflect,
) -> Result<(), ReflectError> {
    if represented_type(target) != represented_type(value) {
        return Err(ReflectError::Shape {
            expected: target.reflect_type_path().to_string(),
            got: value.reflect_type_path().to_string(),
        });
    }
    Ok(())
}

const UID: &str = "Uid";

impl World {
    // ----- registry lookups -----

    fn component_type(&self, name: &str) -> Result<ComponentType<'_>, ReflectError> {
        self.registry.component(name).ok_or_else(|| {
            if self.registry.resource(name).is_some() {
                ReflectError::NotAComponent(name.to_string())
            } else {
                ReflectError::UnknownType(name.to_string())
            }
        })
    }

    fn resource_type(&self, name: &str) -> Result<ResourceType<'_>, ReflectError> {
        self.registry.resource(name).ok_or_else(|| {
            if self.registry.component(name).is_some() {
                ReflectError::NotAResource(name.to_string())
            } else {
                ReflectError::UnknownType(name.to_string())
            }
        })
    }

    /// Parse a field path against this world's registry.
    pub fn field_path(&self, path: &str) -> Result<FieldPath, ReflectError> {
        FieldPath::parse(&self.registry, path)
    }

    fn live(&self, e: Entity) -> Result<(), ReflectError> {
        if self.ecs.contains(e) {
            Ok(())
        } else {
            Err(ReflectError::NoEntity(e))
        }
    }

    // ----- components -----

    /// Names of the registered components on an entity, sorted. Unregistered
    /// components (`GlobalTransform`, `Parent`, `Children`, ...) are not listed.
    pub fn component_names(&self, e: Entity) -> Vec<&'static str> {
        let Ok(entity) = self.ecs.entity(e) else {
            return Vec::new();
        };
        let mut names: Vec<&'static str> = entity
            .component_types()
            .filter_map(|id| self.registry.component_by_id(id))
            .map(|c| c.name())
            .collect();
        names.sort_unstable();
        names
    }

    /// Borrow a component by name.
    pub fn get_reflect(
        &self,
        e: Entity,
        component: &str,
    ) -> Result<ComponentRef<'_>, ReflectError> {
        let ty = self.component_type(component)?;
        self.live(e)?;
        (ty.data.get)(self, e).ok_or_else(|| ReflectError::Missing(ty.name().to_string()))
    }

    /// Mutably borrow a component by name. `Uid` is read-only (its index
    /// would go stale); replace it with [`World::insert_reflect`] instead.
    pub fn get_reflect_mut(
        &mut self,
        e: Entity,
        component: &str,
    ) -> Result<&mut dyn Reflect, ReflectError> {
        let ty = self.component_type(component)?;
        if ty.name() == UID {
            return Err(ReflectError::ReadOnly(UID.into()));
        }
        let (name, get_mut) = (ty.name(), ty.data.get_mut);
        self.live(e)?;
        get_mut(self, e).ok_or_else(|| ReflectError::Missing(name.to_string()))
    }

    /// An owned copy of a component field (or of the whole component).
    pub fn get_field(&self, e: Entity, path: &FieldPath) -> Result<Value, ReflectError> {
        let root = self.get_reflect(e, path.target())?;
        let target = path
            .parsed()
            .reflect_element(root.as_partial_reflect())
            .map_err(|err| resolve_err(&self.registry, path, err))?;
        clone_value(target)
    }

    /// Replace a component field (or the whole component) with `value`, which
    /// may be concrete or dynamic but must represent the field's type.
    pub fn set_field(
        &mut self,
        e: Entity,
        path: &FieldPath,
        value: &dyn PartialReflect,
    ) -> Result<(), ReflectError> {
        let registry = self.registry.clone();
        let root = self.get_reflect_mut(e, path.target())?;
        let target = path
            .parsed()
            .reflect_element_mut(root.as_partial_reflect_mut())
            .map_err(|err| resolve_err(&registry, path, err))?;
        check_shape(target, value)?;
        replace(target, value).map_err(|err| path_err(path, err))
    }

    /// Add a component from a reflected value. Fails if it is already present
    /// (use [`World::set_field`] to overwrite). Inserting a `Uid` updates the index.
    pub fn insert_reflect(
        &mut self,
        e: Entity,
        component: &str,
        value: &dyn PartialReflect,
    ) -> Result<(), ReflectError> {
        let ty = self.component_type(component)?;
        let (name, data) = (ty.name(), ty.data.clone());
        self.live(e)?;
        if (data.contains)(self, e) {
            return Err(ReflectError::AlreadyPresent(name.to_string()));
        }
        if name == UID {
            let uid = Uid::from_reflect_value(value).ok_or_else(|| ReflectError::Shape {
                expected: "box_engine::Uid".into(),
                got: value.reflect_type_path().to_string(),
            })?;
            if self.entity_by_uid(uid).is_some() {
                return Err(ReflectError::DuplicateUid(uid));
            }
        }
        (data.insert)(self, e, value)
    }

    /// Add a component from a reflected value, or overwrite it in place
    /// when present (no archetype move, no remove/add hooks). Not for
    /// `Uid` (use [`World::insert_reflect`], which keeps the uid index).
    pub fn put_reflect(
        &mut self,
        e: Entity,
        component: &str,
        value: &dyn PartialReflect,
    ) -> Result<(), ReflectError> {
        let ty = self.component_type(component)?;
        if ty.name() == UID {
            return Err(ReflectError::ReadOnly(UID.into()));
        }
        let insert = ty.data.insert;
        self.live(e)?;
        insert(self, e, value)
    }

    /// Add a component with its default value (`#[reflect(Default)]`).
    pub fn insert_default(&mut self, e: Entity, component: &str) -> Result<(), ReflectError> {
        let ty = self.component_type(component)?;
        let name = ty.name();
        let value = ty
            .default_value()
            .ok_or_else(|| ReflectError::NoDefault(name.to_string()))?;
        self.insert_reflect(e, name, value.as_ref())
    }

    /// Remove a component and return its value.
    pub fn remove_reflect(&mut self, e: Entity, component: &str) -> Result<Value, ReflectError> {
        let ty = self.component_type(component)?;
        let (name, remove) = (ty.name(), ty.data.remove);
        self.live(e)?;
        remove(self, e)
            .map(|v| v.into_partial_reflect())
            .ok_or_else(|| ReflectError::Missing(name.to_string()))
    }

    // ----- resources -----

    /// Names of the registered resources present in the world, sorted.
    pub fn resource_names(&self) -> Vec<&'static str> {
        self.registry
            .resources()
            .filter(|r| (r.data.get)(self).is_some())
            .map(|r| r.name())
            .collect()
    }

    /// Borrow a resource by name.
    pub fn get_resource_reflect(&self, name: &str) -> Result<&dyn Reflect, ReflectError> {
        let ty = self.resource_type(name)?;
        (ty.data.get)(self).ok_or_else(|| ReflectError::MissingResource(ty.name().to_string()))
    }

    /// An owned copy of a resource field (or the whole resource).
    pub fn get_resource_field(&self, path: &FieldPath) -> Result<Value, ReflectError> {
        let root = self.get_resource_reflect(path.target())?;
        let target = path
            .parsed()
            .reflect_element(root.as_partial_reflect())
            .map_err(|err| resolve_err(&self.registry, path, err))?;
        clone_value(target)
    }

    /// Replace a resource field (or the whole resource).
    pub fn set_resource_field(
        &mut self,
        path: &FieldPath,
        value: &dyn PartialReflect,
    ) -> Result<(), ReflectError> {
        let ty = self.resource_type(path.target())?;
        let (name, get_mut) = (ty.name(), ty.data.get_mut);
        let registry = self.registry.clone();
        let root = get_mut(self).ok_or_else(|| ReflectError::MissingResource(name.to_string()))?;
        let target = path
            .parsed()
            .reflect_element_mut(root.as_partial_reflect_mut())
            .map_err(|err| resolve_err(&registry, path, err))?;
        check_shape(target, value)?;
        replace(target, value).map_err(|err| path_err(path, err))
    }

    /// Insert or overwrite a resource from a reflected value.
    pub fn insert_resource_reflect(
        &mut self,
        name: &str,
        value: &dyn PartialReflect,
    ) -> Result<(), ReflectError> {
        let insert = self.resource_type(name)?.data.insert;
        insert(self, value)
    }

    /// Remove a resource and return its value.
    pub fn remove_resource_reflect(&mut self, name: &str) -> Result<Value, ReflectError> {
        let ty = self.resource_type(name)?;
        let (name, remove) = (ty.name(), ty.data.remove);
        remove(self)
            .map(|v| v.into_partial_reflect())
            .ok_or_else(|| ReflectError::MissingResource(name.to_string()))
    }

    // ----- hierarchy -----

    /// Where `e` sits: its parent's uid and its sibling index (or root index).
    /// Unsaved roots report index 0. Parents without a `Uid` cannot be
    /// addressed, so an entity below one reports where a scene file puts it:
    /// after the last child of its nearest saved ancestor, or after the last
    /// root if there is none.
    pub fn placement(&self, e: Entity) -> Placement {
        match self.parent(e) {
            Some(p)
                if !self.ecs.satisfies::<&Uid>(p)
                    || self.ecs.satisfies::<crate::prefab::AnyGenerated>(p) =>
            {
                let ancestor = self.saved_parent(e);
                Placement {
                    parent: ancestor.and_then(|a| self.uid(a)),
                    index: match ancestor {
                        Some(a) => self.ecs.get::<&Children>(a).map_or(0, |c| c.0.len()),
                        None => self.roots.len(),
                    },
                }
            }
            Some(p) => Placement {
                parent: self.uid(p),
                index: self
                    .ecs
                    .get::<&Children>(p)
                    .ok()
                    .and_then(|c| c.0.iter().position(|c| *c == e))
                    .unwrap_or(0),
            },
            None => Placement {
                parent: None,
                index: self.roots.iter().position(|r| *r == e).unwrap_or(0),
            },
        }
    }

    /// Move `e` to `placement`, keeping its local transform. The index is
    /// clamped to the number of siblings. Returns [`ReflectError::Cycle`]
    /// instead of panicking when the new parent is `e` or a descendant.
    pub fn place(&mut self, e: Entity, placement: Placement) -> Result<(), ReflectError> {
        self.live(e)?;
        let parent = match placement.parent {
            Some(p) => Some(self.entity_by_uid(p).ok_or(ReflectError::NoUid(p))?),
            None => None,
        };
        if let Some(p) = parent
            && self.is_ancestor_or_self(e, p)
        {
            return Err(ReflectError::Cycle);
        }
        if self.parent(e) != parent {
            self.set_parent_unchecked(e, parent);
        }
        let siblings = match parent {
            Some(p) => match self.ecs.query_one_mut::<&mut Children>(p) {
                Ok(children) => &mut children.0,
                Err(_) => return Ok(()),
            },
            None => &mut self.roots,
        };
        // A new child or root was just pushed at the end, so search from
        // there: appending (load, paste) is O(1).
        if let Some(pos) = siblings.iter().rposition(|c| *c == e) {
            let index = placement.index.min(siblings.len() - 1);
            if index != pos {
                siblings.remove(pos);
                siblings.insert(index, e);
            }
        }
        Ok(())
    }
}

impl Uid {
    fn from_reflect_value(value: &dyn PartialReflect) -> Option<Self> {
        <Uid as bevy_reflect::FromReflect>::from_reflect(value)
    }
}
