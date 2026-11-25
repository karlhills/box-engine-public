//! Machine-readable description of every registered type (`box registry --json`).

use bevy_reflect::enums::VariantInfo;
use bevy_reflect::{PartialReflect, ReflectDeserialize, TypeInfo};
use serde_json::{Map, Value as Json, json};

use super::meta::{self, FieldMeta};
use super::{Registry, Value};

fn kind_name(info: &TypeInfo) -> &'static str {
    match info {
        TypeInfo::Struct(_) => "struct",
        TypeInfo::TupleStruct(_) => "tuple_struct",
        TypeInfo::Tuple(_) => "tuple",
        TypeInfo::List(_) => "list",
        TypeInfo::Array(_) => "array",
        TypeInfo::Map(_) => "map",
        TypeInfo::Set(_) => "set",
        TypeInfo::Enum(_) => "enum",
        TypeInfo::Opaque(_) => "opaque",
    }
}

fn variants(registry: &Registry, info: &TypeInfo, depth: usize) -> Option<Json> {
    let TypeInfo::Enum(e) = info else { return None };
    Some(Json::Array(
        e.iter()
            .map(|v| {
                let kind = match v {
                    VariantInfo::Struct(_) => "struct",
                    VariantInfo::Tuple(_) => "tuple",
                    VariantInfo::Unit(_) => "unit",
                };
                let fields = field_list(registry, meta::variant_fields(v), v_infos(v), depth);
                json!({ "name": v.name(), "kind": kind, "doc": v.docs().map(str::trim), "fields": fields })
            })
            .collect(),
    ))
}

fn v_infos(v: &VariantInfo) -> Vec<Option<&'static TypeInfo>> {
    match v {
        VariantInfo::Struct(s) => s.iter().map(|f| f.type_info()).collect(),
        VariantInfo::Tuple(t) => t.iter().map(|f| f.type_info()).collect(),
        VariantInfo::Unit(_) => Vec::new(),
    }
}

fn s_infos(info: &TypeInfo) -> Vec<Option<&'static TypeInfo>> {
    match info {
        TypeInfo::Struct(s) => s.iter().map(|f| f.type_info()).collect(),
        TypeInfo::TupleStruct(t) => t.iter().map(|f| f.type_info()).collect(),
        _ => Vec::new(),
    }
}

/// Field rows. Fields whose type is an enum list its variants, and fields
/// whose type is a struct written field by field (not through its own serde
/// form, like vectors and colours) list its fields (two levels deep at most,
/// enough for shape enums and settings groups without recursing through
/// every type). Tools that validate values from the dump alone (schema-only
/// types, `crate::reflect::SchemaType`) use them.
fn field_list(
    registry: &Registry,
    fields: Vec<FieldMeta>,
    infos: Vec<Option<&'static TypeInfo>>,
    depth: usize,
) -> Json {
    Json::Array(
        fields
            .into_iter()
            .zip(infos)
            .map(|(f, info)| {
                let mut row = Map::new();
                row.insert("name".into(), json!(f.name));
                row.insert("type_path".into(), json!(f.type_path));
                row.insert("doc".into(), json!(f.doc));
                row.insert(
                    "range".into(),
                    f.range
                        .map_or(Json::Null, |r| json!({ "min": r.min, "max": r.max })),
                );
                row.insert("unit".into(), json!(f.unit));
                row.insert("step".into(), json!(f.step));
                row.insert("hidden".into(), json!(f.hidden));
                if depth < 2
                    && let Some(v) = info.and_then(|i| variants(registry, i, depth + 1))
                {
                    row.insert("variants".into(), v);
                }
                if depth < 2
                    && let Some(TypeInfo::Struct(s)) = info
                    && registry
                        .type_registry()
                        .get_type_data::<ReflectDeserialize>(s.type_id())
                        .is_none()
                {
                    let fields = meta::fields_of(info.expect("matched above"));
                    let infos = s_infos(info.expect("matched above"));
                    row.insert(
                        "fields".into(),
                        field_list(registry, fields, infos, depth + 1),
                    );
                }
                Json::Object(row)
            })
            .collect(),
    )
}

struct EntryFlags {
    saved: bool,
    hidden: bool,
    settings: bool,
}

fn type_entry(
    registry: &Registry,
    name: &str,
    info: &'static TypeInfo,
    doc: Option<&str>,
    flags: EntryFlags,
    default: Option<Value>,
) -> Json {
    let default = default
        .as_deref()
        .and_then(|d: &dyn PartialReflect| registry.value_to_json(d).ok())
        .unwrap_or(Json::Null);
    let mut entry = Map::new();
    entry.insert("name".into(), json!(name));
    entry.insert("type_path".into(), json!(info.type_path()));
    entry.insert("kind".into(), json!(kind_name(info)));
    entry.insert("doc".into(), json!(doc));
    entry.insert("saved".into(), json!(flags.saved));
    entry.insert("hidden".into(), json!(flags.hidden));
    let version = registry.type_version(name);
    if version > 1 {
        // Stored values carry `"$version"`; tools without the game's code
        // detect old data with it.
        entry.insert("version".into(), json!(version));
    }
    if flags.settings {
        entry.insert("settings".into(), json!(true));
        entry.insert("file".into(), json!(format!("assets/settings/{name}.json")));
    }
    entry.insert(
        "fields".into(),
        field_list(registry, meta::fields_of(info), s_infos(info), 0),
    );
    if let Some(v) = variants(registry, info, 0) {
        entry.insert("variants".into(), v);
    }
    entry.insert("default".into(), default);
    Json::Object(entry)
}

impl Registry {
    /// Every registered component and resource type with its fields, type
    /// paths, docs, ranges, units and default value, ordered by name. Field
    /// order is declaration order. Settings resources have `"settings": true`
    /// and their `"file"`. Used by `box registry --json` and written to a
    /// native project's `.box/registry.json`. Schema-only types are not
    /// included. `"presets"` lists the placeable presets (`add_preset`).
    pub fn to_json(&self) -> Json {
        let components: Vec<Json> = self
            .components()
            .map(|c| {
                type_entry(
                    self,
                    c.name(),
                    c.type_info(),
                    c.doc(),
                    EntryFlags {
                        saved: c.is_saved(),
                        hidden: c.is_hidden(),
                        settings: false,
                    },
                    c.default_value(),
                )
            })
            .collect();
        let resources: Vec<Json> = self
            .resources()
            .map(|r| {
                type_entry(
                    self,
                    r.name(),
                    r.type_info(),
                    r.doc(),
                    EntryFlags {
                        saved: r.is_saved(),
                        hidden: r.is_hidden(),
                        settings: r.is_settings(),
                    },
                    r.default_value(),
                )
            })
            .collect();
        let presets: Vec<Json> = self
            .presets()
            .map(|p| {
                json!({
                    "name": p.name,
                    "title": p.title,
                    "category": p.category,
                    "description": p.description,
                    "entities": p.entities.iter().map(|e| e.name.clone()).collect::<Vec<_>>(),
                })
            })
            .collect();
        let mut dump =
            json!({ "components": components, "resources": resources, "presets": presets });
        let renames: Map<String, Json> = self
            .migrations()
            .renames()
            .map(|(old, new)| (old.to_string(), json!(new)))
            .collect();
        if !renames.is_empty() {
            dump["renames"] = Json::Object(renames);
        }
        dump
    }
}
