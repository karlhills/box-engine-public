//! Scene JSON through the registry.
//!
//! Layout: `{ "version", "entities": [ { "uid", "name", "parent", "components" } ] }`.
//! Entities are in hierarchy pre-order (roots sorted by `Uid`, children in
//! `Children` order); components are sorted by name; fields follow declaration
//! order. Unregistered components are kept verbatim in [`UnknownComponents`].

use std::collections::{BTreeMap, HashSet};

use bevy_reflect::TypeRegistry;
use bevy_reflect::serde::{TypedReflectDeserializer, TypedReflectSerializer};
use box_core::transform::Parent;
use box_core::world::{Entity, Ref, World};
use serde::de::DeserializeSeed;
use serde::ser::{SerializeMap, SerializeStruct};
use serde::{Serialize, Serializer};
use serde_json::Value;

use crate::components::{Name, Uid};
use crate::ops::{self, Placement};
use crate::registry::{ComponentRef, Registry};
use crate::{Error, Result};

pub const SCENE_VERSION: u64 = 1;

/// Hoisted to the entity record; never inside `"components"`.
const HOISTED: [&str; 2] = ["Uid", "Name"];

/// Components whose type is not registered, preserved as raw JSON.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnknownComponents(pub BTreeMap<String, Value>);

struct Record<'w> {
    uid: Uid,
    name: Option<Ref<'w, Name>>,
    parent: Option<Uid>,
    known: Vec<(&'static str, ComponentRef<'w>)>,
    unknown: Option<Ref<'w, UnknownComponents>>,
    types: &'w TypeRegistry,
}

struct Components<'a, 'w>(&'a Record<'w>);

impl Serialize for Components<'_, '_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let r = self.0;
        let unknown = r.unknown.as_deref().map(|u| &u.0);
        let mut map = s.serialize_map(Some(r.known.len() + unknown.map_or(0, |u| u.len())))?;
        // Merge two sorted sequences so the output is sorted by name.
        let mut k = r.known.iter().peekable();
        let mut u = unknown.into_iter().flatten().peekable();
        loop {
            let take_known = match (k.peek(), u.peek()) {
                (Some((kn, _)), Some((un, _))) => *kn <= un.as_str(),
                (Some(_), None) => true,
                (None, Some(_)) => false,
                (None, None) => break,
            };
            if take_known {
                let (name, value) = k.next().unwrap();
                map.serialize_entry(
                    name,
                    &TypedReflectSerializer::new(value.as_partial_reflect(), r.types),
                )?;
            } else {
                let (name, value) = u.next().unwrap();
                map.serialize_entry(name, value)?;
            }
        }
        map.end()
    }
}

impl Serialize for Record<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("Entity", 4)?;
        st.serialize_field("uid", &self.uid.0)?;
        st.serialize_field("name", &self.name.as_deref().map(|n| n.0.as_str()))?;
        st.serialize_field("parent", &self.parent.map(|p| p.0))?;
        st.serialize_field("components", &Components(self))?;
        st.end()
    }
}

#[derive(Serialize)]
struct SceneOut<'w> {
    version: u64,
    entities: Vec<Record<'w>>,
}

fn record<'w>(world: &'w World, registry: &'w Registry, e: Entity) -> Option<Record<'w>> {
    let uid = ops::uid_of(world, e)?;
    let known = ops::list_components(world, registry, e)
        .into_iter()
        .filter(|n| !HOISTED.contains(n))
        .filter_map(|n| Some((n, (registry.component(n)?.data.get)(world, e)?)))
        .collect();
    Some(Record {
        uid,
        name: world.get::<Name>(e),
        parent: world.get::<Parent>(e).and_then(|p| ops::uid_of(world, p.0)),
        known,
        unknown: world.get::<UnknownComponents>(e),
        types: &registry.types,
    })
}

/// Pre-order walk of `root`'s subtree, skipping entities without a `Uid`.
fn collect_subtree<'w>(
    world: &'w World,
    registry: &'w Registry,
    root: Entity,
    out: &mut Vec<Record<'w>>,
) {
    let mut stack = vec![root];
    while let Some(e) = stack.pop() {
        let Some(r) = record(world, registry, e) else {
            continue;
        };
        out.push(r);
        let children = world.children(e);
        stack.extend(children.into_iter().rev());
    }
}

fn roots(world: &World) -> Vec<(Uid, Entity)> {
    let mut roots: Vec<(Uid, Entity)> = world
        .query::<box_core::world::Without<(Entity, &Uid), &Parent>>()
        .iter()
        .map(|(e, u)| (*u, e))
        .collect();
    roots.sort_unstable_by_key(|(u, _)| *u);
    roots
}

fn records<'w>(world: &'w World, registry: &'w Registry) -> Vec<Record<'w>> {
    let mut out = Vec::with_capacity(world.entity_count() as usize);
    for (_, e) in roots(world) {
        collect_subtree(world, registry, e, &mut out);
    }
    out
}

/// Serialize the whole world to pretty JSON bytes.
pub fn save(world: &World, registry: &Registry) -> Result<Vec<u8>> {
    let scene = SceneOut {
        version: SCENE_VERSION,
        entities: records(world, registry),
    };
    let mut out = Vec::with_capacity(256 * scene.entities.len());
    serde_json::to_writer_pretty(&mut out, &scene)?;
    Ok(out)
}

pub fn save_string(world: &World, registry: &Registry) -> Result<String> {
    Ok(String::from_utf8(save(world, registry)?).expect("serde_json writes UTF-8"))
}

/// A subtree as an array of entity records (the `Despawn`/`Spawn` undo snapshot).
pub fn subtree_value(world: &World, registry: &Registry, root: Entity) -> Result<Value> {
    let mut out = Vec::new();
    collect_subtree(world, registry, root, &mut out);
    Ok(serde_json::to_value(&out)?)
}

/// Load a scene file into a fresh world.
pub fn load(bytes: &[u8], registry: &Registry) -> Result<World> {
    let tree: Value = serde_json::from_slice(bytes)?;
    load_value(&tree, registry)
}

/// Load from the untyped value tree (where migrations would run).
pub fn load_value(tree: &Value, registry: &Registry) -> Result<World> {
    let version = tree
        .get("version")
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::Scene("missing version".into()))?;
    if version != SCENE_VERSION {
        return Err(Error::Scene(format!("unsupported version {version}")));
    }
    let entities = tree
        .get("entities")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Scene("missing entities".into()))?;
    let mut world = World::new();
    instantiate(&mut world, registry, entities)?;
    Ok(world)
}

fn parse_uid(v: &Value) -> Result<Uid> {
    v.as_str()
        .and_then(|s| uuid::Uuid::parse_str(s).ok())
        .map(Uid)
        .ok_or_else(|| Error::Scene(format!("bad uid {v}")))
}

/// Spawn entity records in order; parents must precede children. Returns the
/// spawned entities in record order.
pub fn instantiate(
    world: &mut World,
    registry: &Registry,
    records: &[Value],
) -> Result<Vec<Entity>> {
    let mut seen = HashSet::with_capacity(records.len());
    let mut spawned = Vec::with_capacity(records.len());
    for rec in records {
        let uid = parse_uid(rec.get("uid").unwrap_or(&Value::Null))?;
        if !seen.insert(uid) || ops::entity(world, uid).is_ok() {
            return Err(Error::Scene(format!("duplicate uid {}", uid.0)));
        }
        let name = rec.get("name").and_then(Value::as_str);
        let e = ops::spawn_with_uid(world, uid, name);
        spawned.push(e);
        match rec.get("parent") {
            None | Some(Value::Null) => {}
            Some(p) => {
                let parent = parse_uid(p)?;
                ops::place(
                    world,
                    e,
                    Placement {
                        parent: Some(parent),
                        index: usize::MAX,
                    },
                )?;
            }
        }
        let Some(components) = rec.get("components").and_then(Value::as_object) else {
            continue;
        };
        let mut unknown = BTreeMap::new();
        for (name, value) in components {
            match registry.component(name) {
                Some(ty) => {
                    let boxed = TypedReflectDeserializer::new(ty.registration, &registry.types)
                        .deserialize(value)
                        .map_err(|err| Error::Scene(format!("{}.{name}: {err}", uid.0)))?;
                    (ty.data.insert)(world, e, boxed.as_ref())?;
                }
                None => {
                    unknown.insert(name.clone(), value.clone());
                }
            }
        }
        if !unknown.is_empty() {
            let _ = world.insert_one(e, UnknownComponents(unknown));
        }
    }
    Ok(spawned)
}

/// Play-mode snapshot: a new world built only from serialized data. Uids are
/// preserved, so `ops::entity(&play, uid)` maps an edit entity to its play twin.
pub fn clone_world(world: &World, registry: &Registry) -> Result<World> {
    let bytes = save(world, registry)?;
    load(&bytes, registry)
}
