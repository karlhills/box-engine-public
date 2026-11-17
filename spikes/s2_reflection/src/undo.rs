//! Undo/redo over reflected values. Edits address entities by `Uid`; values are
//! boxed reflected values, so new component types need no undo code.

use bevy_reflect::PartialReflect;
use box_core::world::World;
use serde_json::Value;

use crate::components::Uid;
use crate::ops::{self, Placement};
use crate::registry::Registry;
use crate::scene;
use crate::{Error, Result};

/// A subtree serialized as scene records plus where its root sat.
#[derive(Clone, Debug)]
pub struct EntitySnapshot {
    pub uid: Uid,
    pub placement: Placement,
    pub records: Value,
}

#[derive(Debug)]
pub enum Edit {
    SetField {
        uid: Uid,
        component: &'static str,
        path: String,
        old: Box<dyn PartialReflect>,
        new: Box<dyn PartialReflect>,
    },
    AddComponent {
        uid: Uid,
        component: &'static str,
        data: Box<dyn PartialReflect>,
    },
    RemoveComponent {
        uid: Uid,
        component: &'static str,
        data: Box<dyn PartialReflect>,
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
}

#[derive(Debug, Default)]
pub struct Transaction {
    pub label: String,
    pub edits: Vec<Edit>,
}

fn component_name(registry: &Registry, path: &str) -> Result<&'static str> {
    let comp = path.split('.').next().unwrap_or(path);
    registry
        .component(comp)
        .map(|c| c.name)
        .ok_or_else(|| Error::UnknownComponent(comp.into()))
}

fn field_part(path: &str) -> &str {
    path.split_once('.').map_or("", |(_, f)| f)
}

fn restore(world: &mut World, registry: &Registry, s: &EntitySnapshot) -> Result<()> {
    let records = s
        .records
        .as_array()
        .ok_or_else(|| Error::Scene("snapshot is not an array".into()))?;
    let spawned = scene::instantiate(world, registry, records)?;
    // Records carry the parent uid; the root also needs its sibling index back.
    if let Some(&root) = spawned.first() {
        ops::place(world, root, s.placement)?;
    }
    Ok(())
}

impl Edit {
    fn apply(&self, world: &mut World, registry: &Registry, forward: bool) -> Result<()> {
        match self {
            Edit::SetField {
                uid,
                component,
                path,
                old,
                new,
            } => {
                let v = if forward { new } else { old };
                let full = if path.is_empty() {
                    component.to_string()
                } else {
                    format!("{component}.{path}")
                };
                ops::set_field(world, registry, *uid, &full, v.as_ref())
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
                let adding = matches!(self, Edit::AddComponent { .. }) == forward;
                if adding {
                    ops::insert(world, registry, *uid, component, data.as_ref())
                } else {
                    ops::remove(world, registry, *uid, component).map(drop)
                }
            }
            Edit::Spawn { snapshot } | Edit::Despawn { snapshot } => {
                let spawning = matches!(self, Edit::Spawn { .. }) == forward;
                if spawning {
                    restore(world, registry, snapshot)
                } else {
                    ops::despawn(world, snapshot.uid)
                }
            }
            Edit::Reparent { uid, old, new } => {
                let e = ops::entity(world, *uid)?;
                ops::place(world, e, if forward { *new } else { *old })
            }
        }
    }
}

/// The single mutation path for the edit world.
pub struct Editor {
    pub world: World,
    pub registry: Registry,
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
    open: Option<Transaction>,
}

impl Editor {
    pub fn new(world: World, registry: Registry) -> Self {
        Self {
            world,
            registry,
            undo: Vec::new(),
            redo: Vec::new(),
            open: None,
        }
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    /// Open a transaction (gizmo or slider drag). Successive `set_field` calls on
    /// the same field inside it merge into one edit.
    pub fn begin(&mut self, label: &str) {
        self.commit();
        self.open = Some(Transaction {
            label: label.into(),
            edits: Vec::new(),
        });
    }

    pub fn commit(&mut self) {
        if let Some(t) = self.open.take()
            && !t.edits.is_empty()
        {
            self.undo.push(t);
        }
    }

    fn record(&mut self, label: &str, edit: Edit) {
        self.redo.clear();
        match &mut self.open {
            Some(t) => {
                if let (
                    Some(Edit::SetField {
                        uid: u0,
                        component: c0,
                        path: p0,
                        new: n0,
                        ..
                    }),
                    Edit::SetField {
                        uid,
                        component,
                        path,
                        new,
                        ..
                    },
                ) = (t.edits.last_mut(), &edit)
                    && (*u0, *c0, p0.as_str()) == (*uid, *component, path.as_str())
                {
                    *n0 = ops::clone_value(new.as_ref()).expect("cloned before");
                    return;
                }
                t.edits.push(edit);
            }
            None => self.undo.push(Transaction {
                label: label.into(),
                edits: vec![edit],
            }),
        }
    }

    pub fn set_field(&mut self, uid: Uid, path: &str, value: &dyn PartialReflect) -> Result<()> {
        let component = component_name(&self.registry, path)?;
        let old = ops::get_field(&self.world, &self.registry, uid, path)?;
        ops::set_field(&mut self.world, &self.registry, uid, path, value)?;
        let new = ops::get_field(&self.world, &self.registry, uid, path)?;
        let edit = Edit::SetField {
            uid,
            component,
            path: field_part(path).to_string(),
            old,
            new,
        };
        self.record("Set field", edit);
        Ok(())
    }

    pub fn add_component(&mut self, uid: Uid, name: &str) -> Result<()> {
        let component = component_name(&self.registry, name)?;
        ops::add_default(&mut self.world, &self.registry, uid, component)?;
        let data = ops::get_field(&self.world, &self.registry, uid, component)?;
        self.record(
            "Add component",
            Edit::AddComponent {
                uid,
                component,
                data,
            },
        );
        Ok(())
    }

    pub fn remove_component(&mut self, uid: Uid, name: &str) -> Result<()> {
        let component = component_name(&self.registry, name)?;
        let data =
            ops::remove(&mut self.world, &self.registry, uid, component)?.into_partial_reflect();
        self.record(
            "Remove component",
            Edit::RemoveComponent {
                uid,
                component,
                data,
            },
        );
        Ok(())
    }

    pub fn spawn(&mut self, uid: Uid, name: &str, parent: Option<Uid>) -> Result<()> {
        if ops::entity(&self.world, uid).is_ok() {
            return Err(Error::Scene(format!("duplicate uid {}", uid.0)));
        }
        let parent_e = parent.map(|p| ops::entity(&self.world, p)).transpose()?;
        let e = ops::spawn_with_uid(&mut self.world, uid, Some(name));
        let _ = self
            .world
            .insert_one(e, crate::components::Transform::default());
        if let Some(p) = parent_e {
            self.world.set_parent(e, Some(p));
        }
        let snapshot = EntitySnapshot {
            uid,
            placement: ops::placement(&self.world, e),
            records: scene::subtree_value(&self.world, &self.registry, e)?,
        };
        self.record("Spawn", Edit::Spawn { snapshot });
        Ok(())
    }

    pub fn despawn(&mut self, uid: Uid) -> Result<()> {
        let e = ops::entity(&self.world, uid)?;
        let snapshot = EntitySnapshot {
            uid,
            placement: ops::placement(&self.world, e),
            records: scene::subtree_value(&self.world, &self.registry, e)?,
        };
        ops::despawn(&mut self.world, uid)?;
        self.record("Despawn", Edit::Despawn { snapshot });
        Ok(())
    }

    pub fn reparent(&mut self, uid: Uid, parent: Option<Uid>) -> Result<()> {
        let e = ops::entity(&self.world, uid)?;
        let old = ops::placement(&self.world, e);
        ops::place(
            &mut self.world,
            e,
            Placement {
                parent,
                index: usize::MAX,
            },
        )?;
        let new = ops::placement(&self.world, e);
        self.record("Reparent", Edit::Reparent { uid, old, new });
        Ok(())
    }

    pub fn undo(&mut self) -> Result<bool> {
        self.commit();
        let Some(t) = self.undo.pop() else {
            return Ok(false);
        };
        for edit in t.edits.iter().rev() {
            edit.apply(&mut self.world, &self.registry, false)?;
        }
        self.redo.push(t);
        Ok(true)
    }

    pub fn redo(&mut self) -> Result<bool> {
        let Some(t) = self.redo.pop() else {
            return Ok(false);
        };
        for edit in &t.edits {
            edit.apply(&mut self.world, &self.registry, true)?;
        }
        self.undo.push(t);
        Ok(true)
    }
}
