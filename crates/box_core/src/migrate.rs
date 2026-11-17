//! File-format migration: ordered, pure JSON → JSON steps.
//!
//! Every saved file is read into an untyped `serde_json::Value` tree first,
//! so older data is upgraded on that tree before anything is deserialized.
//! Two kinds of chains exist:
//!
//! - **Format chains** ([`Chain`]) upgrade a file's envelope, keyed by its
//!   top-level `"version"` (scenes, the project file, `.meta` files, save
//!   games). Each format owns one static chain; step *i* upgrades version
//!   *i + 1* to *i + 2*, so the current version is `steps + 1`.
//! - **Type chains** upgrade the stored value of one component, resource or
//!   file type (a material file is a `StandardMaterial` value). They are
//!   registered on the [`Registry`] by the engine or by a game plugin in
//!   `Plugin::register`, so they apply wherever the type is stored: scenes,
//!   prefabs, save games, material, animation graph and settings files. A
//!   value of a type whose version is above 1 carries it as its first key,
//!   `"$version"`; a value without one is version 1. Renamed types are
//!   looked up through a rename table ([`Registry::rename_type`]).
//!
//! A step is a list of declarative operations on a JSON object (rename,
//! move, default, remove a field, transform a value) plus, when those do not
//! suffice, a custom function. Steps never see the `World` and never do I/O,
//! so they can be tested on literal JSON.
//!
//! ```
//! use box_core::migrate::Step;
//! use box_core::reflect::Registry;
//! use serde_json::json;
//!
//! let mut registry = Registry::new();
//! // Version 1 -> 2 of the game's `Spinner`: `speed` became `turns_per_second`.
//! registry.add_migration(
//!     "Spinner",
//!     Step::new("speed is now turns_per_second")
//!         .rename_field("speed", "turns_per_second")
//!         .default_field("axis", json!([0.0, 1.0, 0.0])),
//! );
//! let mut value = json!({ "speed": 2.0 });
//! let applied = box_core::migrate::upgrade_value(&registry, "Spinner", &mut value).unwrap();
//! assert_eq!(applied.len(), 1);
//! assert_eq!(value, json!({ "$version": 2, "turns_per_second": 2.0, "axis": [0.0, 1.0, 0.0] }));
//! ```
//!
//! Data this engine does not understand is kept, never dropped: unknown
//! component and resource types stay raw (`scene::UnknownComponents`), and
//! unknown top-level fields of a known struct type are split off with
//! [`split_unknown_fields`] and written back by the saver.

use std::any::TypeId;
use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Map, Value as Json};

use crate::reflect::{Registry, TypeInfo};

/// The key holding a value's type version (only written when it is above 1).
pub const VERSION_KEY: &str = "$version";

/// A value transform for [`Step::map_field`].
pub type MapFn = Arc<dyn Fn(Json) -> Result<Json, String> + Send + Sync>;
/// A whole-value edit for [`Step::custom`].
pub type CustomFn = Arc<dyn Fn(&mut Json) -> Result<(), String> + Send + Sync>;

/// A migration failure.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum MigrateError {
    /// The data was written by a newer engine or game.
    #[error(
        "{what} version {found} is newer than this {by} supports ({supported}); update {update}"
    )]
    Newer {
        what: String,
        found: u64,
        supported: u32,
        /// "engine" or "game".
        by: &'static str,
        /// What to update ("box-engine" or "the game").
        update: &'static str,
    },
    /// A step failed.
    #[error("{what}: migrating from version {from} (\"{step}\") failed: {message}")]
    Step {
        what: String,
        from: u32,
        step: String,
        message: String,
    },
    /// The type's steps are not known here (a game type known only from its
    /// registry dump: run the game's own player or editor to upgrade).
    #[error(
        "{what} is at version {found}, but the game is at version {current} and its migration steps are only in the game's code; upgrade with the game's player (`--upgrade`)"
    )]
    NeedsGameCode {
        what: String,
        found: u32,
        current: u32,
    },
    #[error("{0}")]
    Format(String),
}

#[derive(Clone)]
enum Op {
    Rename { from: String, to: String },
    Move { from: Vec<String>, to: Vec<String> },
    Default { path: Vec<String>, value: Json },
    Remove { path: Vec<String> },
    Map { path: Vec<String>, f: MapFn },
    Custom(CustomFn),
}

fn split_path(path: &str) -> Vec<String> {
    path.split('.').map(str::to_string).collect()
}

/// One migration step: version *n* to *n + 1* of a format or a type. The
/// operations run in the order they were added. A field path is dotted
/// (`"shadow.bias"`) and walks nested objects.
#[derive(Clone)]
pub struct Step {
    description: String,
    ops: Vec<Op>,
}

impl std::fmt::Debug for Step {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Step")
            .field("description", &self.description)
            .field("ops", &self.ops.len())
            .finish()
    }
}

impl Step {
    /// A step that does nothing yet; `description` is shown in upgrade
    /// summaries ("speed is now turns_per_second").
    pub fn new(description: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            ops: Vec::new(),
        }
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    /// Renames the top-level field `from` to `to`, keeping its position.
    /// Nothing happens when `from` is missing; both present is an error.
    pub fn rename_field(mut self, from: &str, to: &str) -> Self {
        self.ops.push(Op::Rename {
            from: from.into(),
            to: to.into(),
        });
        self
    }

    /// Moves the value at path `from` to path `to` (missing parent objects
    /// are created). Nothing happens when `from` is missing; an existing
    /// value at `to` is an error.
    pub fn move_field(mut self, from: &str, to: &str) -> Self {
        self.ops.push(Op::Move {
            from: split_path(from),
            to: split_path(to),
        });
        self
    }

    /// Sets the field at `path` to `value` when it is missing (a new field
    /// whose default differs from the type's `Default`, or one an old file
    /// must get explicitly).
    pub fn default_field(mut self, path: &str, value: Json) -> Self {
        self.ops.push(Op::Default {
            path: split_path(path),
            value,
        });
        self
    }

    /// Removes the field at `path` when present.
    pub fn remove_field(mut self, path: &str) -> Self {
        self.ops.push(Op::Remove {
            path: split_path(path),
        });
        self
    }

    /// Replaces the value at `path` with `f(value)` when present (unit
    /// changes, enum variant renames, `[r, g, b]` → `[r, g, b, a]`, ...).
    pub fn map_field(
        mut self,
        path: &str,
        f: impl Fn(Json) -> Result<Json, String> + Send + Sync + 'static,
    ) -> Self {
        self.ops.push(Op::Map {
            path: split_path(path),
            f: Arc::new(f),
        });
        self
    }

    /// Edits the whole value (anything the declarative operations cannot
    /// express). Keep it pure: no I/O, no globals.
    pub fn custom(
        mut self,
        f: impl Fn(&mut Json) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        self.ops.push(Op::Custom(Arc::new(f)));
        self
    }

    /// Runs the step on `value`.
    pub fn apply(&self, value: &mut Json) -> Result<(), String> {
        for op in &self.ops {
            match op {
                Op::Rename { from, to } => {
                    let obj = as_object(value, "")?;
                    if !obj.contains_key(from) {
                        continue;
                    }
                    if obj.contains_key(to) {
                        return Err(format!("cannot rename `{from}` to `{to}`: both exist"));
                    }
                    rename_key(obj, from, to);
                }
                Op::Move { from, to } => {
                    let Some(v) = take_path(value, from)? else {
                        continue;
                    };
                    let (last, parents) = to.split_last().ok_or("empty path")?;
                    let parent = object_at(value, parents, true)?.ok_or("unreachable")?;
                    if parent.contains_key(last) {
                        return Err(format!(
                            "cannot move `{}` to `{}`: the target exists",
                            from.join("."),
                            to.join(".")
                        ));
                    }
                    parent.insert(last.clone(), v);
                }
                Op::Default { path, value: v } => {
                    let (last, parents) = path.split_last().ok_or("empty path")?;
                    let parent = object_at(value, parents, true)?.ok_or("unreachable")?;
                    if !parent.contains_key(last) {
                        parent.insert(last.clone(), v.clone());
                    }
                }
                Op::Remove { path } => {
                    take_path(value, path)?;
                }
                Op::Map { path, f } => {
                    let (last, parents) = path.split_last().ok_or("empty path")?;
                    let Some(parent) = object_at(value, parents, false)? else {
                        continue;
                    };
                    if let Some(slot) = parent.get_mut(last) {
                        let old = std::mem::take(slot);
                        *slot = f(old).map_err(|e| format!("`{}`: {e}", path.join(".")))?;
                    }
                }
                Op::Custom(f) => f(value)?,
            }
        }
        Ok(())
    }
}

fn as_object<'a>(value: &'a mut Json, at: &str) -> Result<&'a mut Map<String, Json>, String> {
    value.as_object_mut().ok_or_else(|| {
        if at.is_empty() {
            "expected an object".to_string()
        } else {
            format!("`{at}` is not an object")
        }
    })
}

/// Renames a key in place (its position is kept).
fn rename_key(obj: &mut Map<String, Json>, from: &str, to: &str) {
    let old = std::mem::take(obj);
    for (k, v) in old {
        if k == from {
            obj.insert(to.to_string(), v);
        } else {
            obj.insert(k, v);
        }
    }
}

/// The object at `path` (empty: `value` itself). With `create`, missing
/// objects on the way are added; without, a missing one is `None`.
fn object_at<'a>(
    value: &'a mut Json,
    path: &[String],
    create: bool,
) -> Result<Option<&'a mut Map<String, Json>>, String> {
    let mut cur = value;
    for (i, key) in path.iter().enumerate() {
        let obj = as_object(cur, &path[..i].join("."))?;
        if !obj.contains_key(key) {
            if !create {
                return Ok(None);
            }
            obj.insert(key.clone(), Json::Object(Map::new()));
        }
        cur = obj.get_mut(key).ok_or("unreachable")?;
    }
    as_object(cur, &path.join(".")).map(Some)
}

fn take_path(value: &mut Json, path: &[String]) -> Result<Option<Json>, String> {
    let (last, parents) = path.split_last().ok_or("empty path")?;
    let Some(parent) = object_at(value, parents, false)? else {
        return Ok(None);
    };
    Ok(parent.shift_remove(last))
}

/// An ordered list of steps: step *i* upgrades version *i + 1* to *i + 2*.
#[derive(Clone, Debug, Default)]
pub struct Chain {
    steps: Vec<Step>,
}

impl Chain {
    pub fn new(steps: Vec<Step>) -> Self {
        Self { steps }
    }

    /// The version the chain upgrades to: `steps + 1`.
    pub fn current(&self) -> u32 {
        self.steps.len() as u32 + 1
    }

    pub fn push(&mut self, step: Step) {
        self.steps.push(step);
    }

    /// Upgrades `value` from version `from` to [`current`](Self::current).
    /// Returns the descriptions of the steps that ran. `what` names the data
    /// in errors.
    pub fn upgrade(
        &self,
        what: &str,
        from: u32,
        value: &mut Json,
    ) -> Result<Vec<String>, MigrateError> {
        let from = from.max(1);
        if from > self.current() {
            return Err(MigrateError::Newer {
                what: what.into(),
                found: u64::from(from),
                supported: self.current(),
                by: "engine",
                update: "box-engine",
            });
        }
        let mut applied = Vec::new();
        for (i, step) in self.steps.iter().enumerate().skip(from as usize - 1) {
            step.apply(value).map_err(|message| MigrateError::Step {
                what: what.into(),
                from: i as u32 + 1,
                step: step.description.clone(),
                message,
            })?;
            applied.push(step.description.clone());
        }
        Ok(applied)
    }
}

/// The type chains and the rename table of a [`Registry`].
#[derive(Clone, Debug, Default)]
pub struct TypeMigrations {
    chains: BTreeMap<String, Chain>,
    renames: BTreeMap<String, String>,
}

impl TypeMigrations {
    pub(crate) fn add(&mut self, name: &str, step: Step) {
        self.chains.entry(name.to_string()).or_default().push(step);
    }

    pub(crate) fn rename(&mut self, old: &str, new: &str) {
        self.renames.insert(old.to_string(), new.to_string());
    }

    /// The type chain of `name`, if it has steps.
    pub fn chain(&self, name: &str) -> Option<&Chain> {
        self.chains.get(name)
    }

    /// The current name of a type stored under `name` (itself when it was
    /// never renamed). Follows chains of renames; a cycle stops at the first
    /// repeat.
    pub fn current_name<'a>(&'a self, name: &'a str) -> &'a str {
        let mut cur = name;
        for _ in 0..=self.renames.len() {
            match self.renames.get(cur) {
                Some(next) if next != name => cur = next,
                _ => break,
            }
        }
        cur
    }

    /// `(old, new)` names, sorted by the old name.
    pub fn renames(&self) -> impl Iterator<Item = (&str, &str)> {
        self.renames.iter().map(|(a, b)| (a.as_str(), b.as_str()))
    }
}

/// The version a stored value says it has: its `"$version"`, else 1.
pub fn stored_version(value: &Json) -> Result<u32, MigrateError> {
    match value.get(VERSION_KEY) {
        None => Ok(1),
        Some(v) => v
            .as_u64()
            .filter(|v| *v >= 1)
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| {
                MigrateError::Format(format!(
                    "`{VERSION_KEY}` must be a positive integer, got {v}"
                ))
            }),
    }
}

/// Whether `name` is a type this registry reads (registered, schema-only,
/// or a file type with migration steps).
fn is_known(registry: &Registry, name: &str) -> bool {
    registry.contains(name)
        || registry.schema(name).is_some()
        || registry.migrations().chain(name).is_some()
}

/// Upgrades one stored value of type `name` (already renamed) to the
/// registry's version of the type, leaving `"$version"` as its first key
/// when that version is above 1 (and no `"$version"` otherwise). Values of
/// types the registry does not know are left exactly as they are. Returns
/// the descriptions of the steps that ran.
pub fn upgrade_value(
    registry: &Registry,
    name: &str,
    value: &mut Json,
) -> Result<Vec<String>, MigrateError> {
    if !value.is_object() || !is_known(registry, name) {
        return Ok(Vec::new());
    }
    let found = stored_version(value)?;
    let current = registry.type_version(name);
    let what = format!("`{name}`");
    if found > current {
        return Err(MigrateError::Newer {
            what,
            found: u64::from(found),
            supported: current,
            by: "game or engine",
            update: "box-engine or the game",
        });
    }
    let applied = match registry.migrations().chain(name) {
        Some(chain) => {
            if let Some(obj) = value.as_object_mut() {
                obj.shift_remove(VERSION_KEY);
            }
            chain.upgrade(&what, found, value)?
        }
        None if found < current => {
            return Err(MigrateError::NeedsGameCode {
                what,
                found,
                current,
            });
        }
        None => Vec::new(),
    };
    stamp_version(registry, name, value);
    Ok(applied)
}

/// Sets a value's `"$version"` to the registry's version of `name` (first
/// key), or removes it when that version is 1.
pub fn stamp_version(registry: &Registry, name: &str, value: &mut Json) {
    let current = registry.type_version(name);
    let Some(obj) = value.as_object_mut() else {
        return;
    };
    obj.shift_remove(VERSION_KEY);
    if current > 1 {
        obj.shift_insert(0, VERSION_KEY.to_string(), Json::from(current));
    }
}

/// Removes `"$version"` from a value of a type the registry knows (the
/// deserializer does not accept it). Unknown types keep theirs.
pub fn strip_version(registry: &Registry, name: &str, value: &mut Json) {
    if is_known(registry, name)
        && let Some(obj) = value.as_object_mut()
    {
        obj.shift_remove(VERSION_KEY);
    }
}

/// The top-level field names of a struct type, or `None` for any other
/// kind of type (whose JSON form is not split).
pub fn struct_fields(registry: &Registry, type_id: TypeId) -> Option<&'static [&'static str]> {
    match registry.type_registry().get(type_id)?.type_info() {
        TypeInfo::Struct(s) => Some(s.field_names()),
        _ => None,
    }
}

/// Removes and returns the top-level fields of a struct value that its type
/// (`type_id`) does not have, in file order, so a newer file's data survives
/// a round trip through this engine. Non-struct types return nothing.
pub fn split_unknown_fields(
    registry: &Registry,
    type_id: TypeId,
    value: &mut Json,
) -> Map<String, Json> {
    let mut extra = Map::new();
    let (Some(fields), Some(obj)) = (struct_fields(registry, type_id), value.as_object_mut())
    else {
        return extra;
    };
    if obj.keys().all(|k| fields.contains(&k.as_str())) {
        return extra;
    }
    let old = std::mem::take(obj);
    for (k, v) in old {
        if fields.contains(&k.as_str()) {
            obj.insert(k, v);
        } else {
            extra.insert(k, v);
        }
    }
    extra
}

/// Appends `extra` fields to an object, skipping names it already has (a
/// field the type has gained since wins).
pub fn append_fields(value: &mut Json, extra: &Map<String, Json>) {
    if let Some(obj) = value.as_object_mut() {
        for (k, v) in extra {
            if !obj.contains_key(k) {
                obj.insert(k.clone(), v.clone());
            }
        }
    }
}

/// A single-value file as [`read_value_file`] reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct ValueFile {
    /// The value, ready for [`Registry::value_from_json`].
    pub value: Json,
    /// Top-level fields the type does not have, to keep when the file is
    /// written again ([`value_file_json`]).
    pub unknown: Map<String, Json>,
    /// The migration steps that ran.
    pub steps: Vec<String>,
}

/// Reads a whole file that holds one value of type `name` (a material, an
/// animation graph, a settings file): upgrades it, then splits off
/// `"$version"` and unknown top-level fields.
pub fn read_value_file(
    registry: &Registry,
    name: &str,
    type_id: TypeId,
    json: &Json,
) -> Result<ValueFile, MigrateError> {
    let mut value = json.clone();
    let steps = upgrade_value(registry, name, &mut value)?;
    strip_version(registry, name, &mut value);
    let unknown = split_unknown_fields(registry, type_id, &mut value);
    Ok(ValueFile {
        value,
        unknown,
        steps,
    })
}

/// A value's file form: `json` (the value's reflected JSON) with
/// `"$version"` when the type is versioned and the `keep` fields (unknown
/// fields read from the file before) appended.
pub fn value_file_json(
    registry: &Registry,
    name: &str,
    mut json: Json,
    keep: &Map<String, Json>,
) -> Json {
    stamp_version(registry, name, &mut json);
    append_fields(&mut json, keep);
    json
}

/// The unknown fields of `file` (an object) for a struct type: the fields
/// it has that `type_id` does not, minus `"$version"`.
pub fn unknown_fields_of(registry: &Registry, type_id: TypeId, file: &Json) -> Map<String, Json> {
    let mut value = file.clone();
    if let Some(obj) = value.as_object_mut() {
        obj.shift_remove(VERSION_KEY);
    }
    split_unknown_fields(registry, type_id, &mut value)
}

/// What upgrading a scene-shaped tree changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TreeReport {
    /// `(from, to)` of the envelope `"version"`, when it changed.
    pub format: Option<(u64, u32)>,
    /// Envelope steps that ran.
    pub format_steps: Vec<String>,
    /// `(old, new)` type names renamed, with how many values each.
    pub renamed: BTreeMap<(String, String), usize>,
    /// Per type: the steps that ran and how many values they ran on.
    pub types: BTreeMap<String, (Vec<String>, usize)>,
}

impl TreeReport {
    /// True when nothing changed.
    pub fn is_empty(&self) -> bool {
        self.format.is_none() && self.renamed.is_empty() && self.types.is_empty()
    }

    /// One line per change, for summaries.
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some((from, to)) = self.format {
            let steps = if self.format_steps.is_empty() {
                String::new()
            } else {
                format!(": {}", self.format_steps.join("; "))
            };
            out.push(format!("format version {from} -> {to}{steps}"));
        }
        for ((old, new), n) in &self.renamed {
            out.push(format!("`{old}` renamed to `{new}` ({n}x)"));
        }
        for (name, (steps, n)) in &self.types {
            out.push(format!("`{name}` ({n}x): {}", steps.join("; ")));
        }
        out
    }

    fn record(&mut self, name: &str, steps: Vec<String>) {
        if steps.is_empty() {
            return;
        }
        let entry = self
            .types
            .entry(name.to_string())
            .or_insert_with(|| (steps, 0));
        entry.1 += 1;
    }
}

/// Renames and upgrades the values of one `{ "TypeName": value, ... }` map
/// (a scene's resources or an entity's components) in place.
pub fn upgrade_type_map(
    registry: &Registry,
    map: &mut Map<String, Json>,
    report: &mut TreeReport,
) -> Result<(), MigrateError> {
    let migrations = registry.migrations();
    let renames: Vec<(String, String)> = map
        .keys()
        .filter_map(|k| {
            let new = migrations.current_name(k);
            (new != k).then(|| (k.clone(), new.to_string()))
        })
        .collect();
    for (old, new) in renames {
        if map.contains_key(&new) {
            return Err(MigrateError::Format(format!(
                "`{old}` was renamed to `{new}`, and both are present"
            )));
        }
        rename_key(map, &old, &new);
        *report.renamed.entry((old, new)).or_default() += 1;
    }
    for (name, value) in map.iter_mut() {
        let steps = upgrade_value(registry, name, value)?;
        report.record(name, steps);
    }
    Ok(())
}

/// Upgrades the components and resources of a scene-shaped tree (a scene, a
/// prefab, a save game: `"resources"` and `"entities"[].components`) to the
/// registry's type versions, in place. The envelope is the caller's.
pub fn upgrade_scene_types(
    registry: &Registry,
    tree: &mut Json,
    report: &mut TreeReport,
) -> Result<(), MigrateError> {
    if let Some(Json::Object(resources)) = tree.get_mut("resources") {
        upgrade_type_map(registry, resources, report)?;
    }
    if let Some(Json::Array(entities)) = tree.get_mut("entities") {
        for e in entities {
            if let Some(Json::Object(components)) = e.get_mut("components") {
                upgrade_type_map(registry, components, report)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rename_keeps_position_and_refuses_clashes() {
        let step = Step::new("r").rename_field("b", "beta");
        let mut v = json!({ "a": 1, "b": 2, "c": 3 });
        step.apply(&mut v).unwrap();
        assert_eq!(
            v.as_object().unwrap().keys().collect::<Vec<_>>(),
            ["a", "beta", "c"]
        );
        // Missing source: nothing to do.
        let mut w = json!({ "a": 1 });
        step.apply(&mut w).unwrap();
        assert_eq!(w, json!({ "a": 1 }));
        let mut both = json!({ "b": 1, "beta": 2 });
        assert!(step.apply(&mut both).unwrap_err().contains("both exist"));
    }

    #[test]
    fn move_default_remove_and_map() {
        let step = Step::new("m")
            .move_field("shadow_bias", "shadow.bias")
            .default_field("shadow.cascades", json!(4))
            .default_field("shadow.bias", json!(9.0))
            .remove_field("legacy")
            .map_field("color", |c| match c {
                Json::Array(mut a) if a.len() == 3 => {
                    a.push(json!(1.0));
                    Ok(Json::Array(a))
                }
                other => Ok(other),
            })
            .map_field("missing.deep", |_| Err("never called".into()));
        let mut v = json!({ "shadow_bias": 0.1, "legacy": true, "color": [1.0, 0.5, 0.0] });
        step.apply(&mut v).unwrap();
        assert_eq!(
            v,
            json!({ "color": [1.0, 0.5, 0.0, 1.0], "shadow": { "bias": 0.1, "cascades": 4 } })
        );
        // Moving onto an existing value is refused.
        let mut clash = json!({ "shadow_bias": 0.1, "shadow": { "bias": 0.2 } });
        assert!(step.apply(&mut clash).is_err());
        // A path through a non-object is an error, not a panic.
        let mut bad = json!({ "shadow": 3, "shadow_bias": 1 });
        assert!(step.apply(&mut bad).unwrap_err().contains("not an object"));
    }

    #[test]
    fn custom_steps_and_map_errors_name_the_field() {
        let step = Step::new("c").custom(|v| {
            v["n"] = json!(v["n"].as_i64().ok_or("n must be an integer")? * 2);
            Ok(())
        });
        let mut v = json!({ "n": 21 });
        step.apply(&mut v).unwrap();
        assert_eq!(v["n"], 42);
        assert!(step.apply(&mut json!({ "n": "x" })).is_err());
        let failing = Step::new("f").map_field("a.b", |_| Err("bad unit".into()));
        let e = failing.apply(&mut json!({ "a": { "b": 1 } })).unwrap_err();
        assert_eq!(e, "`a.b`: bad unit");
    }

    #[test]
    fn chains_run_from_the_stored_version_and_refuse_newer() {
        let chain = Chain::new(vec![
            Step::new("one").default_field("v2", json!(true)),
            Step::new("two").rename_field("v2", "v3"),
        ]);
        assert_eq!(chain.current(), 3);
        let mut v = json!({});
        assert_eq!(chain.upgrade("x", 1, &mut v).unwrap(), ["one", "two"]);
        assert_eq!(v, json!({ "v3": true }));
        let mut w = json!({ "v2": 1 });
        assert_eq!(chain.upgrade("x", 2, &mut w).unwrap(), ["two"]);
        assert!(chain.upgrade("x", 3, &mut json!({})).unwrap().is_empty());
        let e = chain.upgrade("scene", 4, &mut json!({})).unwrap_err();
        assert!(
            matches!(
                e,
                MigrateError::Newer {
                    found: 4,
                    supported: 3,
                    ..
                }
            ),
            "{e}"
        );
        let failing = Chain::new(vec![Step::new("needs a").rename_field("a", "b")]);
        let e = failing.upgrade("t", 1, &mut json!(3)).unwrap_err();
        assert!(e.to_string().contains("\"needs a\""), "{e}");
    }

    #[test]
    fn renames_follow_chains_and_stop_on_cycles() {
        let mut m = TypeMigrations::default();
        m.rename("A", "B");
        m.rename("B", "C");
        assert_eq!(m.current_name("A"), "C");
        assert_eq!(m.current_name("C"), "C");
        m.rename("C", "A");
        // A cycle never loops forever.
        let _ = m.current_name("A");
    }
}
