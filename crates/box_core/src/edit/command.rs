//! JSON commands (`box scene apply`): a serde-deserializable [`Command`] list
//! applied through the [`EditStack`] as one undoable, all-or-nothing transaction.
//!
//! ```json
//! [
//!   { "op": "spawn", "name": "Lamp", "parent": "Room",
//!     "components": { "Transform": { "translation": [0.0, 2.0, 0.0], "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [1.0, 1.0, 1.0] },
//!                     "Primitive": { "shape": { "Sphere": { "radius": 0.2, "sectors": 16, "stacks": 8 } }, "cast_shadows": false } } },
//!   { "op": "set", "entity": "Lamp", "path": "Transform.translation.y", "value": 2.5 },
//!   { "op": "rename", "entity": "Lamp", "name": "Ceiling lamp" },
//!   { "op": "reparent", "entity": "Ceiling lamp", "keep": "local" }
//! ]
//! ```
//!
//! `entity` and `parent` take a uid string or a unique entity name. Values use
//! the scene-file representation of the field's type.
//!
//! Components and resources of schema-only types (a native game's types
//! known from its dumped registry, `Registry::add_schema_types`) are
//! validated against the schema and stored as raw JSON, so `box scene apply`
//! can edit them without the game's code.

use serde::Deserialize;
use serde_json::{Map, Value as Json};

use super::{EditError, EditStack, Keep};
use crate::identity::{Name, Uid};
use crate::reflect::{FieldPath, ReflectError, SchemaType, Value};
use crate::scene::{UnknownComponents, UnknownResources};
use crate::world::{Placement, World};

/// One scene edit, tagged by `"op"`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    /// Spawn a saved entity. `uid` defaults to a new one; `parent` to none
    /// (a root); `index` to last. A default `Transform` is added unless
    /// `components` has one.
    Spawn {
        #[serde(default)]
        uid: Option<Uid>,
        name: String,
        #[serde(default)]
        parent: Option<String>,
        #[serde(default)]
        index: Option<usize>,
        #[serde(default)]
        components: Map<String, Json>,
    },
    /// Set a component field (or a whole component): `"path": "Transform.translation.x"`.
    Set {
        entity: String,
        path: String,
        value: Json,
    },
    /// Set a resource field (or a whole resource): `"path": "Environment.fog.density"`.
    SetResource {
        path: String,
        value: Json,
    },
    /// Add a component, with `value` or its default.
    AddComponent {
        entity: String,
        component: String,
        #[serde(default)]
        value: Option<Json>,
    },
    RemoveComponent {
        entity: String,
        component: String,
    },
    /// Move under `parent` (a root when absent) at `index` (last when absent).
    /// Keeps the world position unless `"keep": "local"`.
    Reparent {
        entity: String,
        #[serde(default)]
        parent: Option<String>,
        #[serde(default)]
        index: Option<usize>,
        #[serde(default)]
        keep: Keep,
    },
    /// Despawn the entity and its subtree.
    Despawn {
        entity: String,
    },
    Rename {
        entity: String,
        name: String,
    },
    /// Copy the entity's subtree with fresh uids next to the original.
    Duplicate {
        entity: String,
    },
}

impl Command {
    /// The `"op"` tag.
    pub fn op(&self) -> &'static str {
        match self {
            Command::Spawn { .. } => "spawn",
            Command::Set { .. } => "set",
            Command::SetResource { .. } => "set_resource",
            Command::AddComponent { .. } => "add_component",
            Command::RemoveComponent { .. } => "remove_component",
            Command::Reparent { .. } => "reparent",
            Command::Despawn { .. } => "despawn",
            Command::Rename { .. } => "rename",
            Command::Duplicate { .. } => "duplicate",
        }
    }

    fn entity(&self) -> Option<&str> {
        match self {
            Command::Spawn { name, .. } => Some(name),
            Command::Set { entity, .. }
            | Command::AddComponent { entity, .. }
            | Command::RemoveComponent { entity, .. }
            | Command::Reparent { entity, .. }
            | Command::Despawn { entity }
            | Command::Rename { entity, .. }
            | Command::Duplicate { entity } => Some(entity),
            Command::SetResource { .. } => None,
        }
    }

    fn path(&self) -> Option<&str> {
        match self {
            Command::Set { path, .. } | Command::SetResource { path, .. } => Some(path),
            Command::AddComponent { component, .. }
            | Command::RemoveComponent { component, .. } => Some(component),
            _ => None,
        }
    }
}

/// A command that failed; names the command, the entity and the path.
#[derive(Debug, thiserror::Error)]
#[error("command {index} ({op}){}{}: {source}",
    entity.as_ref().map(|e| format!(", entity `{e}`")).unwrap_or_default(),
    path.as_ref().map(|p| format!(", path `{p}`")).unwrap_or_default())]
pub struct CommandError {
    /// Position in the command list.
    pub index: usize,
    pub op: &'static str,
    pub entity: Option<String>,
    pub path: Option<String>,
    #[source]
    pub source: Box<EditError>,
}

#[derive(Debug, thiserror::Error)]
pub enum CommandParseError {
    #[error("invalid JSON: {0}")]
    Json(serde_json::Error),
    #[error("command {index}: {source}")]
    Command {
        index: usize,
        source: serde_json::Error,
    },
}

/// Parse a JSON array of commands, or a single command object.
pub fn parse_commands(text: &str) -> Result<Vec<Command>, CommandParseError> {
    let tree: Json = serde_json::from_str(text).map_err(CommandParseError::Json)?;
    let items = match tree {
        Json::Array(items) => items,
        single => vec![single],
    };
    items
        .into_iter()
        .enumerate()
        .map(|(index, v)| {
            serde_json::from_value(v).map_err(|source| CommandParseError::Command { index, source })
        })
        .collect()
}

/// What [`apply_commands_with_warnings`] did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Applied {
    /// Uids of entities created by `spawn` and `duplicate`, in order.
    pub created: Vec<Uid>,
    /// Things that were applied, but differently from what was asked: a
    /// number outside its field's `@Range` was clamped.
    pub warnings: Vec<String>,
}

/// Apply `commands` as one transaction labelled `label`. All or nothing: on
/// the first error the transaction is rolled back and the error returned
/// (redo history is kept). If the rollback itself fails, which needs the world
/// to have been changed behind the stack's back, the error says so and the
/// commands applied so far stay as one undo step.
/// Returns the uids of entities created by `spawn` and `duplicate`, in order.
///
/// A `set`/`set_resource` number outside the field's `@Range` is clamped to
/// the range and logged as a warning; use [`apply_commands_with_warnings`]
/// to get the warnings back.
pub fn apply_commands(
    world: &mut World,
    stack: &mut EditStack,
    commands: &[Command],
    label: &str,
) -> Result<Vec<Uid>, CommandError> {
    let applied = apply_commands_with_warnings(world, stack, commands, label)?;
    for w in &applied.warnings {
        tracing::warn!("{w}");
    }
    Ok(applied.created)
}

/// [`apply_commands`], returning the warnings (clamped values) instead of
/// logging them.
pub fn apply_commands_with_warnings(
    world: &mut World,
    stack: &mut EditStack,
    commands: &[Command],
    label: &str,
) -> Result<Applied, CommandError> {
    stack.begin(label);
    let mut created = Vec::new();
    let mut warnings = Vec::new();
    for (index, command) in commands.iter().enumerate() {
        let mut warn = |message: String| {
            warnings.push(format!("command {index} ({}): {message}", command.op()));
        };
        if let Err(source) = apply_one(world, stack, command, &mut created, &mut warn) {
            let source = match stack.rollback(world) {
                Ok(()) => source,
                Err(rollback) => {
                    stack.commit();
                    EditError::Invalid(format!(
                        "{source}; rolling back the earlier commands also failed ({rollback}), \
                         so they stay applied as one undo step"
                    ))
                }
            };
            return Err(CommandError {
                index,
                op: command.op(),
                entity: command.entity().map(str::to_string),
                path: command.path().map(str::to_string),
                source: Box::new(source),
            });
        }
    }
    stack.commit();
    Ok(Applied { created, warnings })
}

const INTEGER_TYPES: [&str; 12] = [
    "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize",
];

/// `json` clamped to the `@Range` of the field at `path`, with a warning, if
/// it is a number outside the range.
fn clamp_to_range(world: &World, path: &FieldPath, json: &Json) -> (Json, Option<String>) {
    let unchanged = (json.clone(), None);
    let (Some(x), Some(meta)) = (json.as_f64(), world.registry().field_meta(path)) else {
        return unchanged;
    };
    let Some(range) = meta.range else {
        return unchanged;
    };
    if x >= range.min && x <= range.max {
        return unchanged;
    }
    let clamped = x.clamp(range.min, range.max);
    let value = if INTEGER_TYPES.contains(&meta.type_path) {
        let rounded = clamped.round();
        if rounded >= 0.0 {
            Json::from(rounded as u64)
        } else {
            Json::from(rounded as i64)
        }
    } else {
        serde_json::Number::from_f64(clamped).map_or(Json::Null, Json::Number)
    };
    let warning = format!(
        "`{path}` = {json} is outside its range {}..={}; clamped to {value}",
        range.min, range.max
    );
    (value, Some(warning))
}

/// Resolve a uid string or a unique entity name.
pub fn resolve_entity(world: &World, reference: &str) -> Result<Uid, EditError> {
    if let Some(uid) = Uid::parse(reference) {
        return world
            .entity_by_uid(uid)
            .map(|_| uid)
            .ok_or(EditError::NoEntity(uid));
    }
    // Generated entities (prefab contents, derived parts) are not editable,
    // so they never make a name ambiguous.
    let matches: Vec<Uid> = world
        .query::<crate::prefab::NotGenerated<(&Uid, &Name)>>()
        .iter()
        .filter(|(_, n)| n.as_str() == reference)
        .map(|(u, _)| *u)
        .collect();
    match matches.as_slice() {
        [uid] => Ok(*uid),
        [] => Err(EditError::UnknownEntity(reference.to_string())),
        _ => Err(EditError::AmbiguousName {
            name: reference.to_string(),
            count: matches.len(),
        }),
    }
}

fn component_value(
    world: &World,
    component: &str,
    json: &Json,
    warn: &mut dyn FnMut(String),
) -> Result<Value, EditError> {
    let registry = world.registry();
    let ty = registry
        .component(component)
        .ok_or_else(|| ReflectError::UnknownType(component.to_string()))?;
    let json = completed(world, ty.type_id(), json, component, warn)?;
    Ok(registry.value_from_json(ty.type_id(), &json)?)
}

/// `json` with the fields it leaves out of an enum variant it switches to
/// filled, so a partial variant such as `{ "Sphere": { "radius": 0.5 } }`
/// keeps its variant: from the enum's `VariantDefaults` when it has them
/// (silently, like a struct's missing fields), else with each field type's
/// default (a warning names those fields).
fn completed(
    world: &World,
    type_id: std::any::TypeId,
    json: &Json,
    path: &str,
    warn: &mut dyn FnMut(String),
) -> Result<Json, EditError> {
    let mut json = json.clone();
    let filled = world
        .registry()
        .complete_variants(type_id, &mut json, path)?;
    if !filled.is_empty() {
        warn(format!(
            "{} not given: using the field type's default (give the field to choose a value)",
            filled
                .iter()
                .map(|f| format!("`{f}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok(json)
}

/// JSON for the field at `path`, typed by the field's current value.
fn field_value(
    world: &World,
    current: &dyn bevy_reflect::PartialReflect,
    path: &FieldPath,
    json: &Json,
    warn: &mut dyn FnMut(String),
) -> Result<Value, EditError> {
    let info = current
        .get_represented_type_info()
        .ok_or_else(|| ReflectError::Unregistered(format!("type of `{path}`")))?;
    if !world.registry().type_registry().contains(info.type_id()) {
        return Err(ReflectError::Unregistered(info.type_path().to_string()).into());
    }
    let json = completed(world, info.type_id(), json, &path.to_string(), warn)?;
    Ok(world.registry().value_from_json(info.type_id(), &json)?)
}

/// The schema-only type `name`, when the world's registry has no Rust type
/// of that name but a schema.
fn schema_of(world: &World, name: &str) -> Option<SchemaType> {
    let registry = world.registry();
    if registry.contains(name) {
        return None;
    }
    registry.schema(name).cloned()
}

/// The raw value of a schema-only component on an entity.
fn raw_component(world: &World, uid: Uid, name: &str) -> Result<Option<Json>, EditError> {
    let e = world.entity_by_uid(uid).ok_or(EditError::NoEntity(uid))?;
    Ok(world
        .get::<UnknownComponents>(e)
        .and_then(|u| u.0.get(name).cloned()))
}

/// Validates a whole schema-only component or resource value (`None`: its
/// default), passing warnings on.
fn schema_value(
    world: &World,
    schema: &SchemaType,
    value: Option<&Json>,
    warn: &mut dyn FnMut(String),
) -> Result<Json, EditError> {
    let value = match value {
        Some(v) => v.clone(),
        None => schema
            .default_value()
            .cloned()
            .ok_or_else(|| ReflectError::NoDefault(schema.name().to_string()))?,
    };
    let (value, warnings) = schema
        .validate(world.registry(), &value)
        .map_err(EditError::Invalid)?;
    warnings.into_iter().for_each(&mut *warn);
    Ok(value)
}

/// `"Spin.axis.y"` → `("Spin", ["axis", "y"])`.
fn split_path(path: &str) -> (&str, Vec<&str>) {
    let mut parts = path.split('.');
    let target = parts.next().unwrap_or_default();
    (target, parts.collect())
}

/// A `set` or `set_resource` of a schema-only type, if `path` names one.
/// Returns `Ok(false)` when it is not one.
fn set_schema_field(
    world: &mut World,
    stack: &mut EditStack,
    entity: Option<&str>,
    path: &str,
    value: &Json,
    warn: &mut dyn FnMut(String),
) -> Result<bool, EditError> {
    let (target, rest) = split_path(path);
    let Some(schema) = schema_of(world, target) else {
        return Ok(false);
    };
    match (entity, schema.is_resource()) {
        (Some(_), true) => {
            return Err(EditError::Invalid(format!(
                "`{target}` is a resource; use set_resource"
            )));
        }
        (None, false) => {
            return Err(ReflectError::NotAResource(target.to_string()).into());
        }
        _ => {}
    }
    let current = match entity {
        Some(reference) => {
            let uid = resolve_entity(world, reference)?;
            let current = raw_component(world, uid, target)?
                .ok_or_else(|| ReflectError::Missing(target.to_string()))?;
            Some((uid, current))
        }
        None => None,
    };
    let base = match &current {
        Some((_, c)) => Some(c.clone()),
        None => world
            .get_resource::<UnknownResources>()
            .and_then(|u| u.0.get(target).cloned()),
    };
    let (new, warnings) = schema
        .set_path(world.registry(), base.as_ref(), &rest, value)
        .map_err(EditError::Invalid)?;
    match current {
        Some((uid, _)) => stack.set_raw_component(world, uid, target, Some(new))?,
        None => stack.set_raw_resource(world, target, Some(new))?,
    }
    warnings.into_iter().for_each(&mut *warn);
    Ok(true)
}

fn apply_one(
    world: &mut World,
    stack: &mut EditStack,
    command: &Command,
    created: &mut Vec<Uid>,
    warn: &mut dyn FnMut(String),
) -> Result<(), EditError> {
    match command {
        Command::Spawn {
            uid,
            name,
            parent,
            index,
            components,
        } => {
            let uid = uid.unwrap_or_else(Uid::new);
            let parent = parent
                .as_deref()
                .map(|p| resolve_entity(world, p))
                .transpose()?;
            let mut values = Vec::new();
            let mut raw = Vec::new();
            for (n, v) in components {
                match schema_of(world, n).filter(|s| !s.is_resource()) {
                    Some(schema) => raw.push((n, schema_value(world, &schema, Some(v), warn)?)),
                    None => values.push((n.as_str(), component_value(world, n, v, warn)?)),
                }
            }
            let refs: Vec<(&str, &dyn bevy_reflect::PartialReflect)> =
                values.iter().map(|(n, v)| (*n, v.as_ref())).collect();
            let at = Placement {
                parent,
                index: index.unwrap_or(usize::MAX),
            };
            stack.spawn(world, uid, name, at, &refs)?;
            for (n, v) in raw {
                stack.set_raw_component(world, uid, n, Some(v))?;
            }
            created.push(uid);
        }
        Command::Set {
            entity,
            path,
            value,
        } => {
            if set_schema_field(world, stack, Some(entity), path, value, warn)? {
                return Ok(());
            }
            let uid = resolve_entity(world, entity)?;
            let path = world.field_path(path)?;
            if path.is_resource() {
                return Err(EditError::Invalid(format!(
                    "`{}` is a resource; use set_resource",
                    path.target()
                )));
            }
            let e = world.entity_by_uid(uid).ok_or(EditError::NoEntity(uid))?;
            let current = world.get_field(e, &path)?;
            let (value, warning) = clamp_to_range(world, &path, value);
            let v = field_value(world, current.as_ref(), &path, &value, warn)?;
            stack.set_field(world, uid, &path, v.as_ref())?;
            if let Some(w) = warning {
                warn(w);
            }
        }
        Command::SetResource { path, value } => {
            if set_schema_field(world, stack, None, path, value, warn)? {
                return Ok(());
            }
            let path = world.field_path(path)?;
            let current = world.get_resource_field(&path)?;
            let (value, warning) = clamp_to_range(world, &path, value);
            if let Some(w) = warning {
                warn(w);
            }
            let v = field_value(world, current.as_ref(), &path, &value, warn)?;
            stack.set_resource_field(world, &path, v.as_ref())?;
        }
        Command::AddComponent {
            entity,
            component,
            value,
        } => {
            let uid = resolve_entity(world, entity)?;
            if let Some(schema) = schema_of(world, component).filter(|s| !s.is_resource()) {
                if raw_component(world, uid, component)?.is_some() {
                    return Err(ReflectError::AlreadyPresent(component.clone()).into());
                }
                let v = schema_value(world, &schema, value.as_ref(), warn)?;
                stack.set_raw_component(world, uid, component, Some(v))?;
                return Ok(());
            }
            let v = value
                .as_ref()
                .map(|json| component_value(world, component, json, warn))
                .transpose()?;
            stack.add_component(world, uid, component, v.as_deref())?;
        }
        Command::RemoveComponent { entity, component } => {
            let uid = resolve_entity(world, entity)?;
            if schema_of(world, component).is_some_and(|s| !s.is_resource()) {
                if raw_component(world, uid, component)?.is_none() {
                    return Err(ReflectError::Missing(component.clone()).into());
                }
                stack.set_raw_component(world, uid, component, None)?;
                return Ok(());
            }
            stack.remove_component(world, uid, component)?;
        }
        Command::Reparent {
            entity,
            parent,
            index,
            keep,
        } => {
            let uid = resolve_entity(world, entity)?;
            let parent = parent
                .as_deref()
                .map(|p| resolve_entity(world, p))
                .transpose()?;
            stack.reparent_keeping(world, uid, parent, *index, *keep)?;
        }
        Command::Despawn { entity } => {
            let uid = resolve_entity(world, entity)?;
            stack.despawn(world, uid)?;
        }
        Command::Rename { entity, name } => {
            let uid = resolve_entity(world, entity)?;
            stack.rename(world, uid, name)?;
        }
        Command::Duplicate { entity } => {
            let uid = resolve_entity(world, entity)?;
            created.push(stack.duplicate(world, uid)?);
        }
    }
    Ok(())
}
