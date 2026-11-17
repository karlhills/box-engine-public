//! Luau type definitions for editor completion (luau-lsp), generated from
//! the registry and the installed [`ScriptApi`]s.
//!
//! The output is a luau-lsp *definitions file*: a `type` per registered
//! component, resource and event (fields in declaration order), a
//! `<Name>Patch` type with every field optional (what `set` takes),
//! `Entity:get` / `Entity:set` overloads per component name, and each API's
//! own declarations, and the script rule types (`RuleIntent`, `RuleOut`,
//! `RuleComponents`; [`crate::rules::definitions`]). Names are sorted, so
//! the text only changes when the
//! registry or an API changes. Point VS Code's luau-lsp at it with
//! `"luau-lsp.types.definitionFiles": [".box/types/box.d.luau"]`.

use std::any::TypeId;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bevy_reflect::TypeInfo;
use bevy_reflect::enums::VariantInfo;
use box_core::asset_ref::AssetRef;
use box_core::color::Color;
use box_core::identity::Uid;
use box_core::reflect::Registry;
use glam::{Quat, Vec2, Vec3, Vec3A};

use crate::api::{ScriptApi, api_definitions, builtin_apis};

/// Where the definitions file goes inside a project.
pub const DEFINITIONS_PATH: &str = ".box/types/box.d.luau";

/// The definitions file for `registry` and the built-in APIs plus `extra`.
pub fn definitions(registry: &Registry, extra: &[Arc<dyn ScriptApi>]) -> String {
    let mut apis = builtin_apis();
    apis.extend(extra.iter().cloned());
    let mut out = String::from(
        "-- box-engine script API: luau-lsp definitions, generated from the type registry.\n\
         -- Do not edit; regenerate with `box script types --project <dir>`.\n\n",
    );
    let types = RegisteredTypes::of(registry);
    for (title, list) in [
        ("-- Components\n", &types.components),
        ("\n-- Resources\n", &types.resources),
        ("\n-- Events\n", &types.events),
    ] {
        out.push_str(title);
        for t in list {
            type_pair(&mut out, registry, t);
        }
    }
    out.push('\n');
    let components = &types.components;

    let mut methods = entity_extra_methods(&apis);
    for RegisteredType { name, .. } in components {
        methods.push_str(&format!(
            "    --- A copy of this entity's `{name}` component, or nil when it has none.\n    \
             function get(self, component: \"{name}\"): {name}?\n    \
             --- Patches this entity's `{name}` component with the given fields.\n    \
             function set(self, component: \"{name}\", value: {name}Patch): ()\n"
        ));
    }
    methods.push_str(GENERIC_ACCESSORS);
    let mut resource_get = String::new();
    for RegisteredType { name, .. } in &types.resources {
        resource_get.push_str(&format!("((name: \"{name}\") -> {name}?) & "));
    }
    resource_get.push_str("((name: string) -> any)");
    let mut event_send = String::new();
    for RegisteredType { name, .. } in &types.events {
        event_send.push_str(&format!(
            "((name: \"{name}\", event: {name}Patch?) -> ()) & "
        ));
    }
    event_send.push_str("((name: string, payload: any?) -> ())");

    out.push_str(
        &api_definitions(&apis)
            .replace("@COMPONENT_METHODS@", &methods)
            .replace("@RESOURCE_GET@", &resource_get)
            .replace("@EVENT_SEND@", &event_send),
    );
    out.push_str(SCRIPT_MODULE);
    let names: Vec<&str> = components.iter().map(|t| t.name).collect();
    out.push('\n');
    out.push_str(&crate::rules::definitions(&names));
    export_aliases(&out)
}

/// luau-lsp makes a definitions file's type aliases visible to scripts only
/// when they are exported: `type X = …` at the start of a line becomes
/// `export type X = …` (checked against luau-lsp 1.70.1).
fn export_aliases(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 1024);
    for line in text.split_inclusive('\n') {
        if line.starts_with("type ") {
            out.push_str("export ");
        }
        out.push_str(line);
    }
    out
}

/// The generic component accessors, after the typed overloads
/// (`@COMPONENT_METHODS@`).
pub(crate) const GENERIC_ACCESSORS: &str = r#"    --- A copy of a component (`e:get("Transform")`) or of one of its fields (`e:get("Transform.translation")`), or nil when the entity does not have the component. Always a copy, never a live view: change it with `set`. Unknown names fail with a did-you-mean.
    function get(self, path: string): any
    --- Patches a component or one field: `e:set("Transform", { translation = v })` changes only the fields given, `e:set("Transform.scale", v)` one field. Values are checked against the component's field types. The entity must have the component (`add` adds one).
    function set(self, path: string, value: any): ()
"#;

/// The method lines other APIs add to `Entity` (their
/// [`ScriptApi::entity_definitions`]), indented for the class body.
pub(crate) fn entity_extra_methods(apis: &[Arc<dyn ScriptApi>]) -> String {
    let mut methods = String::new();
    for api in apis {
        let extra = api.entity_definitions();
        if !extra.trim().is_empty() {
            methods.push_str(&format!("    -- {}\n", api.name()));
            for line in extra.lines().filter(|l| !l.trim().is_empty()) {
                methods.push_str(&format!("    {}\n", line.trim()));
            }
        }
    }
    methods
}

/// What a script module may define (for authors' own annotations), with
/// the lifecycle callbacks the runtime calls.
pub(crate) const SCRIPT_MODULE: &str = r#"-- Script modules
--- One declared script property: what each entry of a module's `properties` table takes. Every field is optional. A bare value (`speed = 4`, `enabled = true`, `tint = Color.rgb(1, 0, 0)`) is shorthand for a declaration with only a `default`.
type PropertyDecl = {
    --- The property type. Omit it to infer it from `default`; a number infers "number", so write "integer" for whole numbers.
    type: ("number" | "integer" | "boolean" | "string" | "vector" | "color" | "entity" | "asset")?,
    --- The default value, used until the inspector or the scene overrides it. Entity properties default to none; asset properties take an assets-relative path.
    default: any?,
    --- The smallest value the inspector allows (numbers and integers).
    min: number?,
    --- The largest value the inspector allows (numbers and integers).
    max: number?,
    --- The inspector's drag step.
    step: number?,
    --- A unit shown next to the value in the inspector, such as "m" or "deg/s".
    unit: string?,
    --- The inspector tooltip.
    doc: string?,
}

--- The `self` table of a running script instance (one per entity with a `Script`). Fields the script sets on it are the instance's runtime state. Its metatable is the module, so module functions can be called as methods (`self:helper()`).
type ScriptSelf = {
    --- The entity the script is attached to.
    entity: Entity,
    --- The effective property values: declared defaults overridden by the entity's `Script` component. Entity properties arrive as entity handles (or nil), asset properties as paths. Refreshed on hot reload and inspector edits.
    props: { [string]: any },
}

--- The table a gameplay script file returns. Every field is optional; the runtime calls the callbacks the module defines. Instances run in scene order, so a scene plays the same way every time.
type ScriptModule = {
    --- Declared properties: name to declaration (or to a bare default value). They appear in the inspector in declaration order and are saved with the scene.
    properties: { [string]: PropertyDecl | number | boolean | string | vector | Color }?,
    --- Runs once when the instance starts (when the entity has its `Script` and the file has loaded), as a task: it may `wait`.
    on_start: ((self: ScriptSelf) -> ())?,
    --- Runs every frame in Update, after the game's native Update systems. `dt` is the frame's game time in seconds (scaled by `time.scale`, 0 while paused).
    on_update: ((self: ScriptSelf, dt: number) -> ())?,
    --- Runs every fixed step (60 Hz by default) in FixedUpdate, before characters move, so `character.move` and `character.jump` apply in the same step. `dt` is the fixed step.
    on_fixed_update: ((self: ScriptSelf, dt: number) -> ())?,
    --- Runs after the entity or its `Script` component is gone; its components can no longer be read.
    on_destroy: ((self: ScriptSelf) -> ())?,
    --- Called when another collider starts overlapping this entity's trigger collider, or this entity enters another's trigger. Both entities are told.
    on_trigger_enter: ((self: ScriptSelf, other: Entity) -> ())?,
    --- Called when an overlap reported by `on_trigger_enter` ends.
    on_trigger_exit: ((self: ScriptSelf, other: Entity) -> ())?,
    --- Called when this rigid body or character starts touching `other`; `impulse` is the contact impulse in newton-seconds. Needs the engine's `dynamics` feature (on in the prebuilt editor and player).
    on_collision_enter: ((self: ScriptSelf, other: Entity, impulse: number) -> ())?,
    --- Called when a contact reported by `on_collision_enter` ends. Needs the `dynamics` feature.
    on_collision_exit: ((self: ScriptSelf, other: Entity) -> ())?,
    --- Returns the instance's runtime state as plain data (numbers, strings, booleans, vectors, Quats, entities and tables of them) for a save game. Properties are saved with the scene and need no hook.
    on_save: ((self: ScriptSelf) -> any)?,
    --- Restores what `on_save` returned when a save game loads. Runs after the respawned instance's `on_start` has reached its first wait, so set defaults in `on_start` and restore them here.
    on_load: ((self: ScriptSelf, data: any) -> ())?,
    --- Called on each instance when the file is saved and reloaded during Play. When the module defines it, the instance's tasks and handlers are cancelled first, so restart them here.
    on_reload: ((self: ScriptSelf) -> ())?,
}
"#;

/// A registered component, resource or event type, or a schema-only one
/// (a native project's game types read from `.box/registry.json` by tools
/// without the game's code).
pub(crate) struct RegisteredType<'a> {
    pub name: &'a str,
    pub kind: TypeKind,
    pub shape: Shape<'a>,
    pub doc: Option<&'a str>,
    /// Hidden from the editor's Add menu.
    pub hidden: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TypeKind {
    Component,
    Resource,
    Event,
}

/// Every type scripts reach by name, sorted by name within each kind.
pub(crate) struct RegisteredTypes<'a> {
    pub components: Vec<RegisteredType<'a>>,
    pub resources: Vec<RegisteredType<'a>>,
    pub events: Vec<RegisteredType<'a>>,
}

impl<'a> RegisteredTypes<'a> {
    pub fn of(registry: &'a Registry) -> Self {
        let schema_of = |resource: bool| {
            registry
                .schemas()
                .filter(move |s| s.is_resource() == resource)
                .map(move |s| RegisteredType {
                    name: s.name(),
                    kind: if resource {
                        TypeKind::Resource
                    } else {
                        TypeKind::Component
                    },
                    shape: Shape::Schema(s.entry()),
                    doc: s.entry()["doc"].as_str(),
                    hidden: s.entry()["hidden"].as_bool().unwrap_or(false),
                })
        };
        let mut components: Vec<RegisteredType> = registry
            .components()
            .map(|c| RegisteredType {
                name: c.name(),
                kind: TypeKind::Component,
                shape: Shape::Info(c.type_info()),
                doc: c.doc(),
                hidden: c.is_hidden(),
            })
            .chain(schema_of(false))
            .collect();
        let mut resources: Vec<RegisteredType> = registry
            .resources()
            .map(|r| RegisteredType {
                name: r.name(),
                kind: TypeKind::Resource,
                shape: Shape::Info(r.type_info()),
                doc: r.doc(),
                hidden: r.is_hidden(),
            })
            .chain(schema_of(true))
            .collect();
        let mut events: Vec<RegisteredType> = registry
            .events()
            .map(|e| RegisteredType {
                name: e.name(),
                kind: TypeKind::Event,
                shape: Shape::Info(e.type_info()),
                doc: e.doc(),
                hidden: false,
            })
            .collect();
        for list in [&mut components, &mut resources, &mut events] {
            list.sort_by_key(|t| t.name);
        }
        Self {
            components,
            resources,
            events,
        }
    }
}

/// A type's description: reflection info, or a schema-only type's registry
/// entry (`name`, `fields` with `type_path`s).
pub(crate) enum Shape<'a> {
    Info(&'static TypeInfo),
    Schema(&'a serde_json::Value),
}

/// A Rust doc comment as plain Markdown for script authors: lines trimmed,
/// intra-doc links (`` [`Type`] ``, `[text](Self::field)`) reduced to their
/// text.
pub(crate) fn clean_rustdoc(doc: &str) -> String {
    let text = doc.lines().map(str::trim).collect::<Vec<_>>().join("\n");
    let text = text.trim();
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        out.push_str(&rest[..open]);
        let tail = &rest[open..];
        let Some(close) = tail.find(']') else {
            out.push_str(tail);
            rest = "";
            break;
        };
        let inner = &tail[1..close];
        let after = &tail[close + 1..];
        if after.starts_with('(')
            && let Some(end) = after.find(')')
        {
            out.push_str(inner);
            rest = &after[end + 1..];
        } else if inner.len() >= 2 && inner.starts_with('`') && inner.ends_with('`') {
            out.push_str(inner);
            rest = after;
        } else {
            out.push('[');
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

/// A `--- text` line for a definitions file: the first paragraph of a
/// doc comment on one line (empty without a doc).
pub(crate) fn doc_comment(indent: &str, doc: Option<&str>) -> String {
    let Some(doc) = doc.map(clean_rustdoc).filter(|d| !d.is_empty()) else {
        return String::new();
    };
    let doc = doc.as_str();
    let first = doc.split("\n\n").next().unwrap_or(doc);
    let words: Vec<&str> = first.split_whitespace().collect();
    format!("{indent}--- {}\n", words.join(" "))
}

fn type_pair(out: &mut String, registry: &Registry, t: &RegisteredType) {
    let name = t.name;
    let (full, patch) = match &t.shape {
        Shape::Info(info) => (luau_type(info, 0, false), luau_type(info, 0, true)),
        Shape::Schema(entry) => (
            schema_type(registry, entry, false),
            schema_type(registry, entry, true),
        ),
    };
    out.push_str(&doc_comment("", t.doc));
    out.push_str(&format!("type {name} = {full}\n"));
    out.push_str(&format!(
        "--- The fields of `{name}` to change, each optional (what `set` takes).\n"
    ));
    out.push_str(&format!("type {name}Patch = {patch}\n"));
}

/// The Luau type of a schema-only type's field (`type_path`), `any` when
/// the engine does not know it.
pub(crate) fn schema_field_type(registry: &Registry, type_path: Option<&str>) -> String {
    type_path
        .and_then(|p| registry.type_registry().get_with_type_path(p))
        .map_or_else(|| "any".to_owned(), |r| luau_type(r.type_info(), 1, false))
}

/// A schema-only type's fields: engine field types (numbers, vectors,
/// colours, ...) are looked up in the registry, others are `any`.
pub(crate) fn schema_type(registry: &Registry, entry: &serde_json::Value, patch: bool) -> String {
    let fields: Vec<String> = entry["fields"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| {
            let name = f["name"].as_str()?;
            let t = schema_field_type(registry, f["type_path"].as_str());
            Some(format!(
                "{}: {}",
                table_key(name),
                if patch { optional(t) } else { t }
            ))
        })
        .collect();
    if fields.is_empty() {
        "{}".into()
    } else {
        format!("{{ {} }}", fields.join(", "))
    }
}

/// Luau's reserved words: a table type field with one of these names must
/// be written `["end"]: T`.
const LUAU_KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "if", "in", "local",
    "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

/// A table type key: the bare name when it is a Luau identifier, else
/// `["name"]` (reserved words such as `end`, or names that are not
/// identifiers).
fn table_key(name: &str) -> String {
    let ident = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if ident && !LUAU_KEYWORDS.contains(&name) {
        name.to_owned()
    } else {
        format!("[{name:?}]")
    }
}

fn is(info: &TypeInfo, t: TypeId) -> bool {
    info.type_id() == t
}

fn number_like(info: &TypeInfo) -> bool {
    [
        TypeId::of::<f32>(),
        TypeId::of::<f64>(),
        TypeId::of::<i8>(),
        TypeId::of::<i16>(),
        TypeId::of::<i32>(),
        TypeId::of::<i64>(),
        TypeId::of::<isize>(),
        TypeId::of::<u8>(),
        TypeId::of::<u16>(),
        TypeId::of::<u32>(),
        TypeId::of::<u64>(),
        TypeId::of::<usize>(),
    ]
    .contains(&info.type_id())
}

pub(crate) fn field_type(info: Option<&'static TypeInfo>, depth: usize) -> String {
    match info {
        Some(i) => luau_type(i, depth + 1, false),
        None => "any".into(),
    }
}

fn optional(t: String) -> String {
    if t.ends_with('?') || t == "any" {
        t
    } else if t.contains('|') || t.contains("->") {
        format!("({t})?")
    } else {
        format!("{t}?")
    }
}

/// The Luau type of a reflected type (see `convert` for the mapping).
/// `patch` makes a struct's fields optional.
pub(crate) fn luau_type(info: &'static TypeInfo, depth: usize, patch: bool) -> String {
    if number_like(info) {
        return "number".into();
    }
    if is(info, TypeId::of::<bool>()) {
        return "boolean".into();
    }
    if is(info, TypeId::of::<String>()) || is(info, TypeId::of::<Uid>()) {
        return "string".into();
    }
    if is(info, TypeId::of::<AssetRef>()) {
        return "string?".into();
    }
    if is(info, TypeId::of::<Vec3>())
        || is(info, TypeId::of::<Vec3A>())
        || is(info, TypeId::of::<Vec2>())
    {
        return "vector".into();
    }
    if is(info, TypeId::of::<Quat>()) {
        return "Quat".into();
    }
    if is(info, TypeId::of::<Color>()) {
        return "Color".into();
    }
    if depth > 6 {
        return "any".into();
    }
    match info {
        TypeInfo::Struct(s) => {
            if s.field_len() == 0 {
                return "{}".into();
            }
            let fields: Vec<String> = s
                .iter()
                .map(|f| {
                    let t = field_type(f.type_info(), depth);
                    let t = if patch { optional(t) } else { t };
                    format!("{}: {t}", table_key(f.name()))
                })
                .collect();
            format!("{{ {} }}", fields.join(", "))
        }
        TypeInfo::TupleStruct(t) if t.field_len() == 1 => {
            field_type(t.field_at(0).and_then(|f| f.type_info()), depth)
        }
        TypeInfo::TupleStruct(_) | TypeInfo::Tuple(_) => "{ any }".into(),
        TypeInfo::List(l) => format!("{{ {} }}", field_type(l.item_info(), depth)),
        TypeInfo::Array(a) => format!("{{ {} }}", field_type(a.item_info(), depth)),
        TypeInfo::Set(_) => "{ any }".into(),
        TypeInfo::Map(m) => format!(
            "{{ [{}]: {} }}",
            field_type(m.key_info(), depth),
            field_type(m.value_info(), depth)
        ),
        TypeInfo::Enum(e) => {
            if e.type_path().starts_with("core::option::Option") {
                let inner = e.variant("Some").and_then(|v| match v {
                    VariantInfo::Tuple(t) => t.field_at(0).and_then(|f| f.type_info()),
                    _ => None,
                });
                return optional(field_type(inner, depth));
            }
            let parts: Vec<String> = e
                .iter()
                .map(|v| match v {
                    VariantInfo::Unit(u) => format!("\"{}\"", u.name()),
                    VariantInfo::Tuple(t) if t.field_len() == 1 => format!(
                        "{{ {}: {} }}",
                        t.name(),
                        field_type(t.field_at(0).and_then(|f| f.type_info()), depth)
                    ),
                    VariantInfo::Tuple(t) => format!("{{ {}: {{ any }} }}", t.name()),
                    VariantInfo::Struct(s) => {
                        let fields: Vec<String> = s
                            .iter()
                            .map(|f| {
                                format!(
                                    "{}: {}",
                                    f.name(),
                                    optional(field_type(f.type_info(), depth))
                                )
                            })
                            .collect();
                        format!("{{ {}: {{ {} }} }}", s.name(), fields.join(", "))
                    }
                })
                .collect();
            if parts.is_empty() {
                "never".into()
            } else {
                parts.join(" | ")
            }
        }
        _ => "any".into(),
    }
}

/// Writes `definitions` to `<project>/.box/types/box.d.luau` when it
/// differs from what is there. Returns the path.
pub fn write_definitions(project_root: &Path, definitions: &str) -> std::io::Result<PathBuf> {
    let path = project_root.join(DEFINITIONS_PATH);
    if std::fs::read_to_string(&path).is_ok_and(|old| old == definitions) {
        return Ok(path);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, definitions)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definitions_cover_components_and_are_stable() {
        let mut registry = Registry::with_core_types();
        crate::register_script_types(&mut registry);
        let a = definitions(&registry, &[]);
        let b = definitions(&registry, &[]);
        assert_eq!(a, b);
        assert!(
            a.contains("type Transform = { translation: vector, rotation: Quat, scale: vector }"),
            "{a}"
        );
        assert!(a.contains(
            "type TransformPatch = { translation: vector?, rotation: Quat?, scale: vector? }"
        ));
        assert!(a.contains("function get(self, component: \"Transform\"): Transform?"));
        assert!(a.contains("declare world: {"));
        assert!(a.contains("declare extern type Entity with"));
        assert!(!a.contains('@'), "placeholders left: {a}");
        assert!(a.contains("type Name = string"));
    }

    /// The type expressions parse as Luau: every `type` alias, and every
    /// `declare name: T` as `type _name = T` (`declare extern type` and
    /// `declare function` are definition-file syntax the compiler does not
    /// take; they are fixed text).
    #[test]
    fn generated_types_are_valid_luau() {
        let mut registry = Registry::with_core_types();
        crate::register_script_types(&mut registry);
        box_physics::register_physics_types(&mut registry);
        let defs = definitions(&registry, &[]);
        let mut luau = String::new();
        let mut lines = defs.lines().peekable();
        let mut checked = 0;
        while let Some(line) = lines.next() {
            let head = if let Some(rest) = line.strip_prefix("declare extern type ") {
                let _ = rest;
                for l in lines.by_ref() {
                    if l == "end" {
                        break;
                    }
                }
                continue;
            } else if line.starts_with("declare function") || line.starts_with("--") {
                continue;
            } else if let Some(rest) = line.strip_prefix("declare ") {
                let (name, ty) = rest.split_once(": ").unwrap();
                format!("type _{name} = {ty}")
            } else if let Some(alias) = line.strip_prefix("export type ") {
                format!("type {alias}")
            } else {
                continue;
            };
            let mut block = head;
            // Doc comments may mention braces: count code only.
            let open = |s: &str| {
                if s.trim_start().starts_with("--") {
                    0
                } else {
                    s.matches('{').count() as i64 - s.matches('}').count() as i64
                }
            };
            let mut depth = open(&block);
            while depth > 0 {
                let l = lines.next().unwrap();
                depth += open(l);
                block.push('\n');
                block.push_str(l);
            }
            luau.push_str(&block);
            luau.push('\n');
            checked += 1;
        }
        // Names the aliases refer to.
        let prelude = "type Entity = any\ntype Quat = any\ntype UiNode = any\n";
        let source = format!("{prelude}{luau}");
        if let Err(e) = mlua::chunk::Compiler::new().compile(&source) {
            panic!("{e}\n{source}");
        }
        assert!(checked > 40, "{checked}");
    }
}
