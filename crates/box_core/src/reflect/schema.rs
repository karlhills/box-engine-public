//! Schema-only types: component and resource types known from a registry
//! dump (`box registry --json`, a native project's `.box/registry.json`)
//! rather than from Rust code.
//!
//! Tools that do not link the game's code (the `box` CLI, `box mcp`) load a
//! native project's dumped registry with [`Registry::add_schema_types`]. Scene
//! data of a schema-only type stays raw JSON in
//! [`UnknownComponents`](crate::scene::UnknownComponents) /
//! [`UnknownResources`](crate::scene::UnknownResources), so files keep their
//! exact contents; edit commands validate new values against the schema
//! before storing them raw:
//!
//! - field names (with a did-you-mean),
//! - field types: engine types (numbers, vectors, colours, asset references,
//!   ...) are checked by deserializing with the engine's registry, enums
//!   against their dumped variants, lists and options element by element,
//!   structs that the dump describes field by field; anything else is taken
//!   as given,
//! - ranges: a number outside its field's `@Range` is clamped, with a warning
//!   (as for reflected types).
//!
//! Missing fields take the dumped default.

use serde_json::{Map, Value as Json};

use super::{Registry, did_you_mean};

/// A component or resource type described by a registry dump entry.
#[derive(Clone, Debug, PartialEq)]
pub struct SchemaType {
    name: String,
    resource: bool,
    /// The dump entry: `fields`, `variants`, `default`, `doc`, ...
    entry: Json,
}

/// The result of validating a value: the value to store and warnings
/// (clamped numbers).
pub type Checked = (Json, Vec<String>);

impl SchemaType {
    pub(crate) fn new(entry: &Json, resource: bool) -> Self {
        Self {
            name: entry["name"].as_str().unwrap_or_default().to_string(),
            resource,
            entry: entry.clone(),
        }
    }

    /// The short name used in scene files.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// True for a resource, false for a component.
    pub fn is_resource(&self) -> bool {
        self.resource
    }

    /// True for a project settings resource.
    pub fn is_settings(&self) -> bool {
        self.entry["settings"] == true
    }

    /// The dump entry (fields, variants, default, docs), as `box registry
    /// --json` prints it.
    pub fn entry(&self) -> &Json {
        &self.entry
    }

    /// The type's stored-data version from the dump (`"version"`, written
    /// when the game registered migration steps), 1 without one.
    pub fn version(&self) -> u32 {
        self.entry["version"]
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(1)
            .max(1)
    }

    /// The default value, if the dump has one.
    pub fn default_value(&self) -> Option<&Json> {
        self.entry.get("default").filter(|d| !d.is_null())
    }

    fn fields(&self) -> &[Json] {
        self.entry["fields"].as_array().map_or(&[], Vec::as_slice)
    }

    /// Validates a whole value: an object for a struct type (missing fields
    /// from the default, unknown fields refused), checked field by field.
    pub fn validate(&self, registry: &Registry, value: &Json) -> Result<Checked, String> {
        let mut warnings = Vec::new();
        let kind = self.entry["kind"].as_str().unwrap_or("struct");
        let value = if kind == "struct" {
            // A stored value's version is not a field.
            let mut value = value.clone();
            if let Some(obj) = value.as_object_mut() {
                obj.shift_remove(crate::migrate::VERSION_KEY);
            }
            let mut merged = merge_default(self.default_value(), &value, &self.name)?;
            check_struct(registry, &self.name, self.fields(), &merged, &mut warnings)?;
            // Stored in the game's current version.
            if self.version() > 1
                && let Some(obj) = merged.as_object_mut()
            {
                obj.shift_insert(
                    0,
                    crate::migrate::VERSION_KEY.into(),
                    Json::from(self.version()),
                );
            }
            merged
        } else if let Some(variants) = self.entry["variants"].as_array() {
            check_enum(registry, &self.name, variants, value, &mut warnings)?;
            value.clone()
        } else {
            value.clone()
        };
        Ok((value, warnings))
    }

    /// The whole value after setting the field at `path` (the segments after
    /// the type name: `["speed"]`, `["axis", "y"]`, `["items[2]"]`) of
    /// `current` (the default when `None`) to `value`, validated. A number
    /// set on a field with a range is clamped into it, with a warning.
    pub fn set_path(
        &self,
        registry: &Registry,
        current: Option<&Json>,
        path: &[&str],
        value: &Json,
    ) -> Result<Checked, String> {
        if path.is_empty() {
            return self.validate(registry, value);
        }
        let base = match current.or(self.default_value()) {
            Some(b) => b.clone(),
            None => return Err(format!("`{}` has no default value to edit", self.name)),
        };
        let field = path[0].split('[').next().unwrap_or(path[0]);
        let Some(row) = self.fields().iter().find(|f| f["name"] == field) else {
            return Err(self.missing_field(field));
        };
        let mut warnings = Vec::new();
        let mut value = value.clone();
        if let (Some(x), Some(min), Some(max)) = (
            value.as_f64(),
            row["range"]["min"].as_f64(),
            row["range"]["max"].as_f64(),
        ) && !(min..=max).contains(&x)
        {
            let clamped = x.clamp(min, max);
            let integer = row["type_path"].as_str().is_some_and(is_integer_type);
            value = if integer {
                let r = clamped.round();
                if r >= 0.0 {
                    Json::from(r as u64)
                } else {
                    Json::from(r as i64)
                }
            } else {
                serde_json::Number::from_f64(clamped).map_or(Json::Null, Json::Number)
            };
            warnings.push(format!(
                "`{}.{}` = {x} is outside its range {min}..={max}; clamped to {value}",
                self.name,
                path.join(".")
            ));
        }
        let mut whole = base;
        let mut slot = &mut whole;
        for segment in segments(path)? {
            slot = step(slot, &segment).ok_or_else(|| {
                format!(
                    "bad field path `{}.{}`: no `{}` there",
                    self.name,
                    path.join("."),
                    segment.text()
                )
            })?;
        }
        *slot = value;
        let (whole, more) = self.validate(registry, &whole)?;
        warnings.extend(more);
        Ok((whole, warnings))
    }

    fn missing_field(&self, field: &str) -> String {
        let names: Vec<&str> = self
            .fields()
            .iter()
            .filter_map(|f| f["name"].as_str())
            .collect();
        missing_field_message(&self.name, field, &names)
    }
}

fn missing_field_message(owner: &str, field: &str, names: &[&str]) -> String {
    let mut m = format!(
        "`{owner}` has no field `{field}`; fields: {}",
        names.join(", ")
    );
    if let Some(guess) = did_you_mean(field, names.iter().copied()) {
        m.push_str(&format!(" (did you mean `{guess}`?)"));
    }
    m
}

fn is_integer_type(t: &str) -> bool {
    matches!(
        t,
        "u8" | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
    )
}

/// One step of a field path below the type.
enum Segment {
    Field(String),
    Index(usize),
}

impl Segment {
    fn text(&self) -> String {
        match self {
            Segment::Field(f) => f.clone(),
            Segment::Index(i) => format!("[{i}]"),
        }
    }
}

/// `["items[2]", "x"]` → `Field(items), Index(2), Field(x)`.
fn segments(path: &[&str]) -> Result<Vec<Segment>, String> {
    let mut out = Vec::new();
    for part in path {
        let mut rest = *part;
        let name_end = rest.find('[').unwrap_or(rest.len());
        if name_end > 0 {
            out.push(Segment::Field(rest[..name_end].to_string()));
        }
        rest = &rest[name_end..];
        while let Some(inner) = rest.strip_prefix('[') {
            let close = inner
                .find(']')
                .ok_or_else(|| format!("bad field path segment `{part}`"))?;
            let index = inner[..close]
                .parse()
                .map_err(|_| format!("bad index in `{part}`"))?;
            out.push(Segment::Index(index));
            rest = &inner[close + 1..];
        }
        if !rest.is_empty() {
            return Err(format!("bad field path segment `{part}`"));
        }
    }
    Ok(out)
}

/// The slot one segment below `v`: an object key, an array index, or
/// `x`/`y`/`z`/`w` into a numeric array (vectors and quaternions are
/// written as arrays). An enum value `{"Variant": {...}}` is looked through.
fn step<'a>(v: &'a mut Json, segment: &Segment) -> Option<&'a mut Json> {
    match (v, segment) {
        (Json::Object(o), Segment::Field(f)) => {
            if o.contains_key(f) {
                return o.get_mut(f);
            }
            // An externally tagged enum value: look inside its one variant.
            if o.len() == 1 {
                return o.values_mut().next().and_then(|inner| match inner {
                    Json::Object(io) => io.get_mut(f),
                    _ => None,
                });
            }
            None
        }
        (Json::Array(a), Segment::Index(i)) => a.get_mut(*i),
        (Json::Array(a), Segment::Field(f)) => {
            let i = ["x", "y", "z", "w"].iter().position(|c| c == f)?;
            a.get_mut(i)
        }
        _ => None,
    }
}

/// `value` over `default` (both objects): fields of `value` replace the
/// default's; unknown fields are caught by the struct check.
fn merge_default(default: Option<&Json>, value: &Json, owner: &str) -> Result<Json, String> {
    let Json::Object(given) = value else {
        return Err(format!(
            "`{owner}` expects an object with its fields, got {}",
            short(value)
        ));
    };
    let mut out = match default {
        Some(Json::Object(d)) => d.clone(),
        _ => Map::new(),
    };
    for (k, v) in given {
        out.insert(k.clone(), v.clone());
    }
    Ok(Json::Object(out))
}

/// A value shortened for messages.
fn short(v: &Json) -> String {
    let s = v.to_string();
    if s.chars().count() > 60 {
        format!("{}…", s.chars().take(60).collect::<String>())
    } else {
        s
    }
}

/// Checks an object against field rows: no unknown fields, every present
/// field of the right type.
fn check_struct(
    registry: &Registry,
    owner: &str,
    rows: &[Json],
    value: &Json,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    let Json::Object(o) = value else {
        return Err(format!("`{owner}` expects an object, got {}", short(value)));
    };
    let names: Vec<&str> = rows.iter().filter_map(|f| f["name"].as_str()).collect();
    for (key, v) in o {
        let Some(row) = rows.iter().find(|r| r["name"] == key.as_str()) else {
            return Err(missing_field_message(owner, key, &names));
        };
        check_field(registry, &format!("{owner}.{key}"), row, v, warnings)?;
    }
    Ok(())
}

/// Checks one field value against its dump row.
fn check_field(
    registry: &Registry,
    path: &str,
    row: &Json,
    v: &Json,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    let type_path = row["type_path"].as_str().unwrap_or_default();
    check_type(registry, path, type_path, row, v, warnings)
}

fn check_type(
    registry: &Registry,
    path: &str,
    type_path: &str,
    row: &Json,
    v: &Json,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    // Engine (and primitive) types: the engine's own deserializer decides.
    if let Some(reg) = registry.type_registry().get_with_type_path(type_path) {
        return registry
            .value_from_json(reg.type_id(), v)
            .map(|_| ())
            .map_err(|e| format!("`{path}`: {}", deserialize_message(&e.to_string())));
    }
    if let Some(inner) = type_path
        .strip_prefix("core::option::Option<")
        .and_then(|t| t.strip_suffix('>'))
    {
        if v.is_null() {
            return Ok(());
        }
        return check_type(registry, path, inner, &Json::Null, v, warnings);
    }
    if let Some(inner) = type_path
        .strip_prefix("alloc::vec::Vec<")
        .and_then(|t| t.strip_suffix('>'))
    {
        let Json::Array(items) = v else {
            return Err(format!("`{path}` expects a list, got {}", short(v)));
        };
        for (i, item) in items.iter().enumerate() {
            check_type(
                registry,
                &format!("{path}[{i}]"),
                inner,
                &Json::Null,
                item,
                warnings,
            )?;
        }
        return Ok(());
    }
    if let Some(variants) = row["variants"].as_array() {
        return check_enum(registry, path, variants, v, warnings);
    }
    if let Some(rows) = row["fields"].as_array() {
        return check_struct(registry, path, rows, v, warnings);
    }
    // A game type the dump does not describe: taken as given.
    Ok(())
}

/// `"cannot convert `f32` from JSON: invalid type: string \"x\", expected
/// f32"` → the part after the type.
fn deserialize_message(m: &str) -> String {
    match m.split_once("from JSON: ") {
        Some((_, rest)) => rest.to_string(),
        None => m.to_string(),
    }
}

/// An externally tagged enum value: `"Unit"` or `{"Variant": fields}`.
fn check_enum(
    registry: &Registry,
    path: &str,
    variants: &[Json],
    v: &Json,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    let names: Vec<&str> = variants.iter().filter_map(|x| x["name"].as_str()).collect();
    let (name, inner) = match v {
        Json::String(s) => (s.as_str(), None),
        Json::Object(o) if o.len() == 1 => {
            let (k, inner) = o.iter().next().expect("one entry");
            (k.as_str(), Some(inner))
        }
        _ => {
            return Err(format!(
                "`{path}` expects one of {} (a variant name, or {{\"Variant\": {{...}}}}), got {}",
                names.join(", "),
                short(v)
            ));
        }
    };
    let Some(variant) = variants.iter().find(|x| x["name"] == name) else {
        let mut m = format!(
            "`{path}`: no variant `{name}`; variants: {}",
            names.join(", ")
        );
        if let Some(guess) = did_you_mean(name, names.iter().copied()) {
            m.push_str(&format!(" (did you mean `{guess}`?)"));
        }
        return Err(m);
    };
    let rows = variant["fields"].as_array().map_or(&[][..], Vec::as_slice);
    match (variant["kind"].as_str(), inner) {
        (Some("unit"), None) => Ok(()),
        (Some("unit"), Some(_)) => Err(format!("`{path}`: variant `{name}` has no fields")),
        (Some("struct"), Some(inner)) => {
            check_struct(registry, &format!("{path}.{name}"), rows, inner, warnings)
        }
        (Some("tuple"), Some(inner)) => {
            let items: Vec<&Json> = match inner {
                Json::Array(a) if rows.len() != 1 => a.iter().collect(),
                other => vec![other],
            };
            if items.len() != rows.len() {
                return Err(format!(
                    "`{path}`: variant `{name}` has {} field(s), got {}",
                    rows.len(),
                    items.len()
                ));
            }
            for (i, (row, item)) in rows.iter().zip(items).enumerate() {
                check_field(registry, &format!("{path}.{name}.{i}"), row, item, warnings)?;
            }
            Ok(())
        }
        (_, None) => Err(format!(
            "`{path}`: variant `{name}` needs its fields: {{\"{name}\": ...}}"
        )),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spin() -> SchemaType {
        SchemaType::new(
            &json!({
                "name": "Spin",
                "type_path": "game::Spin",
                "kind": "struct",
                "fields": [
                    { "name": "speed", "type_path": "f32", "range": { "min": -4.0, "max": 4.0 } },
                    { "name": "axis", "type_path": "glam::Vec3", "range": null },
                    { "name": "count", "type_path": "u32", "range": { "min": 0.0, "max": 10.0 } },
                    { "name": "mode", "type_path": "game::Mode", "range": null, "variants": [
                        { "name": "Off", "kind": "unit", "fields": [] },
                        { "name": "Wobble", "kind": "struct", "fields": [
                            { "name": "amount", "type_path": "f32", "range": null }
                        ] }
                    ] },
                    { "name": "tags", "type_path": "alloc::vec::Vec<alloc::string::String>", "range": null }
                ],
                "default": { "speed": 0.25, "axis": [0.0, 1.0, 0.0], "count": 1, "mode": "Off", "tags": [] }
            }),
            false,
        )
    }

    fn registry() -> Registry {
        let mut r = Registry::with_core_types();
        r.register_value::<glam::Vec3>();
        r.register_value::<Vec<String>>();
        r
    }

    #[test]
    fn validates_fields_types_and_variants() {
        let r = registry();
        let s = spin();
        let (v, w) = s.validate(&r, &json!({ "speed": 1.0 })).unwrap();
        assert!(w.is_empty());
        assert_eq!(v["axis"], json!([0.0, 1.0, 0.0]), "default filled in");
        let e = s.validate(&r, &json!({ "sped": 1.0 })).unwrap_err();
        assert!(e.contains("did you mean `speed`"), "{e}");
        let e = s.validate(&r, &json!({ "speed": "fast" })).unwrap_err();
        assert!(e.contains("`Spin.speed`") && e.contains("f32"), "{e}");
        let e = s.validate(&r, &json!({ "count": 1.5 })).unwrap_err();
        assert!(e.contains("`Spin.count`"), "{e}");
        let e = s.validate(&r, &json!({ "axis": [1, 2] })).unwrap_err();
        assert!(e.contains("`Spin.axis`"), "{e}");
        assert!(
            s.validate(&r, &json!({ "mode": { "Wobble": { "amount": 2.0 } } }))
                .is_ok()
        );
        let e = s.validate(&r, &json!({ "mode": "Wobbel" })).unwrap_err();
        assert!(e.contains("did you mean `Wobble`"), "{e}");
        let e = s
            .validate(&r, &json!({ "mode": { "Wobble": { "amont": 2.0 } } }))
            .unwrap_err();
        assert!(e.contains("no field `amont`"), "{e}");
        let e = s.validate(&r, &json!({ "tags": ["a", 3] })).unwrap_err();
        assert!(e.contains("`Spin.tags"), "{e}");
        assert!(s.validate(&r, &json!(3)).is_err());
    }

    #[test]
    fn set_path_edits_clamps_and_validates() {
        let r = registry();
        let s = spin();
        let (v, w) = s.set_path(&r, None, &["speed"], &json!(9.0)).unwrap();
        assert_eq!(v["speed"], json!(4.0));
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("clamped"), "{w:?}");
        let (v, _) = s
            .set_path(&r, Some(&v), &["axis", "x"], &json!(2.5))
            .unwrap();
        assert_eq!(v["axis"], json!([2.5, 1.0, 0.0]));
        let (v, w) = s.set_path(&r, Some(&v), &["count"], &json!(20)).unwrap();
        assert_eq!(v["count"], json!(10));
        assert_eq!(w.len(), 1);
        let e = s.set_path(&r, Some(&v), &["nope"], &json!(1)).unwrap_err();
        assert!(e.contains("no field `nope`"), "{e}");
        let e = s
            .set_path(&r, Some(&v), &["speed"], &json!("x"))
            .unwrap_err();
        assert!(e.contains("Spin.speed"), "{e}");
        let (v, _) = s
            .set_path(
                &r,
                Some(&v),
                &["mode"],
                &json!({ "Wobble": { "amount": 1.0 } }),
            )
            .unwrap();
        let (v, _) = s
            .set_path(&r, Some(&v), &["mode", "amount"], &json!(3.0))
            .unwrap();
        assert_eq!(v["mode"], json!({ "Wobble": { "amount": 3.0 } }));
    }
}
