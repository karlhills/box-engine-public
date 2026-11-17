//! Script files as assets: [`ScriptAsset`] and the `.luau` importer.
//!
//! The importer checks the syntax and runs the file's top level once in a
//! throwaway sandboxed VM (on the import worker) to read its declared
//! `properties`, so the editor can show them without running the game. At
//! that point only value types (`vector`, `Quat`, `Color`), `log` and
//! `require` work; other engine globals are stand-ins that fail when called
//! (scripts touch the world in callbacks, not at load time).

use std::time::{Duration, Instant};

use box_assets::{ImportContext, ImportError, Importer};
use box_core::asset_ref::AssetRef;
use box_core::color::Color;
use mlua::{Lua, Result as LuaResult, Table, Value};

use crate::component::{PropType, PropertyDecl, ScriptValue};
use crate::convert::{from_vector, to_color};

/// A `.luau` file: its source and what its module declares.
#[derive(Clone, Debug, PartialEq)]
pub struct ScriptAsset {
    /// Path relative to `assets/` (the chunk name in error messages).
    pub path: String,
    pub source: String,
    /// The module's declared `properties`, in declaration order.
    pub properties: Vec<PropertyDecl>,
    /// Names of the module's functions (`on_start`, `on_update`, ...),
    /// sorted.
    pub functions: Vec<String>,
    /// False when the file returns something other than a table (a value
    /// for `require`, not a script for entities).
    pub is_module: bool,
}

impl ScriptAsset {
    /// The declared property `name`.
    pub fn property(&self, name: &str) -> Option<&PropertyDecl> {
        self.properties.iter().find(|p| p.name == name)
    }
}

/// Imports `.luau` files as [`ScriptAsset`]s (importer name `luau`).
pub struct ScriptImporter;

/// The longest a script's top level may run while it is imported.
const IMPORT_BUDGET: Duration = Duration::from_secs(2);

impl Importer for ScriptImporter {
    fn name(&self) -> &str {
        "luau"
    }

    fn extensions(&self) -> &[&str] {
        &["luau"]
    }

    fn import(&self, ctx: &mut ImportContext) -> Result<(), ImportError> {
        let path = ctx.path().to_string();
        let source = String::from_utf8(ctx.bytes().to_vec())
            .map_err(|e| ImportError::Decode(format!("{path}: not UTF-8: {e}")))?;
        let asset = inspect(&path, &source, ctx)
            .map_err(|e| ImportError::Decode(crate::runtime::describe_error(&e)))?;
        ctx.add(None, asset);
        Ok(())
    }
}

/// Runs the file's top level in an import VM and reads its declarations.
fn inspect(path: &str, source: &str, ctx: &mut ImportContext) -> LuaResult<ScriptAsset> {
    let lua = Lua::new();
    crate::api::install_import(&lua)?;
    let started = Instant::now();
    lua.set_interrupt(move |_| {
        if started.elapsed() > IMPORT_BUDGET {
            Err(mlua::Error::runtime(
                "the script's top level ran for more than 2 s while importing (an endless loop?)",
            ))
        } else {
            Ok(mlua::VmState::Continue)
        }
    });
    lua.scope(|scope| {
        let require = scope.create_function_mut(|lua, name: String| {
            let caller = crate::api::caller_path(lua);
            let dep = crate::api::resolve_require(caller.as_deref(), &name)?;
            let bytes = ctx
                .read_dependency(&dep)
                .map_err(|e| mlua::Error::runtime(format!("require(\"{name}\"): {e}")))?;
            let text = String::from_utf8(bytes)
                .map_err(|_| mlua::Error::runtime(format!("{dep}: not UTF-8")))?;
            let v: Value = lua
                .load(&text)
                .set_name(format!("@{dep}"))
                .set_environment(import_env(lua)?)
                .eval()?;
            Ok(match v {
                Value::Nil => Value::Boolean(true),
                v => v,
            })
        })?;
        lua.globals().set("require", require)?;
        lua.sandbox(true)?;
        let module: Value = lua
            .load(source)
            .set_name(format!("@{path}"))
            .set_environment(import_env(&lua)?)
            .eval()?;
        describe_module(&lua, path, source, module)
    })
}

/// The import environment: the real globals, and for any other name a
/// stand-in table whose functions fail with a clear message.
fn import_env(lua: &Lua) -> LuaResult<Table> {
    let env = lua.create_table()?;
    let mt = lua.create_table()?;
    let globals = lua.globals();
    mt.set(
        "__index",
        lua.create_function(move |lua, (_, key): (Value, String)| {
            let v: Value = globals.get(key.as_str())?;
            if !v.is_nil() {
                return Ok(v);
            }
            let stub = lua.create_table()?;
            let stub_mt = lua.create_table()?;
            let lib = key.clone();
            stub_mt.set(
                "__index",
                lua.create_function(move |lua, (_, name): (Value, String)| {
                    let what = format!("{lib}.{name}");
                    lua.create_function(move |_, _: mlua::MultiValue| -> LuaResult<()> {
                        Err(mlua::Error::runtime(format!(
                            "{what} is only available while the game runs (call it from a callback, not the script's top level)"
                        )))
                    })
                })?,
            )?;
            stub.set_metatable(Some(stub_mt))?;
            Ok(Value::Table(stub))
        })?,
    )?;
    env.set_metatable(Some(mt))?;
    Ok(env)
}

/// What the importer and the runtime read from a loaded module.
pub(crate) fn describe_module(
    lua: &Lua,
    path: &str,
    source: &str,
    module: Value,
) -> LuaResult<ScriptAsset> {
    let Value::Table(table) = module else {
        return Ok(ScriptAsset {
            path: path.to_owned(),
            source: source.to_owned(),
            properties: Vec::new(),
            functions: Vec::new(),
            is_module: false,
        });
    };
    let mut functions = Vec::new();
    for pair in table.pairs::<Value, Value>() {
        let (k, v) = pair?;
        if let (Value::String(k), Value::Function(_)) = (&k, &v) {
            functions.push(k.to_str()?.to_string());
        }
    }
    functions.sort();
    let properties = parse_properties(lua, path, source, &table)?;
    Ok(ScriptAsset {
        path: path.to_owned(),
        source: source.to_owned(),
        properties,
        functions,
        is_module: true,
    })
}

fn prop_err(path: &str, name: &str, msg: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::runtime(format!("{path}: property `{name}`: {msg}"))
}

/// The module's `properties` table, in declaration order:
///
/// ```lua
/// Spinner.properties = {
///     speed = { type = "number", default = 90, min = 0, max = 720, unit = "deg/s", doc = "Turn rate" },
///     axis = { default = vector.create(0, 1, 0) },  -- type from the default
///     enabled = true,                               -- shorthand: the default alone
/// }
/// ```
pub(crate) fn parse_properties(
    _lua: &Lua,
    path: &str,
    source: &str,
    module: &Table,
) -> LuaResult<Vec<PropertyDecl>> {
    let props: Value = module.raw_get("properties")?;
    let props = match props {
        Value::Nil => return Ok(Vec::new()),
        Value::Table(t) => t,
        other => {
            return Err(mlua::Error::runtime(format!(
                "{path}: `properties` must be a table, got {}",
                other.type_name()
            )));
        }
    };
    let mut out = Vec::new();
    for pair in props.pairs::<Value, Value>() {
        let (k, v) = pair?;
        let Value::String(k) = k else {
            return Err(mlua::Error::runtime(format!(
                "{path}: property names must be strings"
            )));
        };
        let name = k.to_str()?.to_string();
        out.push(parse_decl(path, &name, v)?);
    }
    // Declaration order: where each name first appears after `properties`.
    let start = source.find("properties").unwrap_or(0);
    let position = |name: &str| -> usize {
        let hay = &source[start..];
        let mut from = 0;
        while let Some(i) = hay[from..].find(name) {
            let at = from + i;
            let before = hay[..at].chars().next_back();
            let after = hay[at + name.len()..].trim_start().chars().next();
            let ident = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
            if !ident(before) && after == Some('=') {
                return start + at;
            }
            from = at + name.len();
        }
        usize::MAX
    };
    out.sort_by(|a, b| {
        position(&a.name)
            .cmp(&position(&b.name))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(out)
}

fn parse_decl(path: &str, name: &str, v: Value) -> LuaResult<PropertyDecl> {
    let (table, default_value) = match &v {
        // `{ type = ..., default = ... }`, unless it is a colour table.
        Value::Table(t) if !is_color_table(t)? => (Some(t.clone()), t.raw_get::<Value>("default")?),
        _ => (None, v.clone()),
    };
    let declared = match &table {
        Some(t) => match t.raw_get::<Value>("type")? {
            Value::Nil => None,
            Value::String(s) => {
                let s = s.to_str()?.to_string();
                Some(PropType::parse(&s).ok_or_else(|| {
                    let names: Vec<&str> = PropType::ALL.iter().map(|t| t.name()).collect();
                    prop_err(
                        path,
                        name,
                        format!("unknown type `{s}`; types: {}", names.join(", ")),
                    )
                })?)
            }
            other => {
                return Err(prop_err(
                    path,
                    name,
                    format!("`type` must be a string, got {}", other.type_name()),
                ));
            }
        },
        None => None,
    };
    let ty = match declared {
        Some(t) => t,
        None => infer(&default_value)
            .ok_or_else(|| prop_err(path, name, "give a `type` or a `default` to infer it from"))?,
    };
    let default = if default_value.is_nil() {
        ty.zero()
    } else {
        convert_default(ty, &default_value).map_err(|m| prop_err(path, name, m))?
    };
    let num = |key: &str| -> LuaResult<Option<f64>> {
        match &table {
            Some(t) => t.raw_get::<Option<f64>>(key),
            None => Ok(None),
        }
    };
    let text = |key: &str| -> LuaResult<Option<String>> {
        match &table {
            Some(t) => t.raw_get::<Option<String>>(key),
            None => Ok(None),
        }
    };
    let range = match (num("min")?, num("max")?) {
        (Some(a), Some(b)) => Some((a, b)),
        (Some(a), None) => Some((a, f64::INFINITY)),
        (None, Some(b)) => Some((f64::NEG_INFINITY, b)),
        (None, None) => None,
    };
    Ok(PropertyDecl {
        name: name.to_owned(),
        ty,
        default,
        range,
        step: num("step")?,
        unit: text("unit")?,
        doc: text("doc")?,
    })
}

fn is_color_table(t: &Table) -> LuaResult<bool> {
    let num = |v: Value| matches!(v, Value::Number(_) | Value::Integer(_));
    Ok(num(t.raw_get("r")?) && num(t.raw_get("g")?))
}

fn infer(v: &Value) -> Option<PropType> {
    Some(match v {
        Value::Boolean(_) => PropType::Boolean,
        Value::Number(_) => PropType::Number,
        // Whole numbers read back as integers; declare `type = "integer"` for one.
        Value::Integer(_) => PropType::Number,
        Value::String(_) => PropType::String,
        Value::Vector(_) => PropType::Vector,
        Value::Table(_) => PropType::Color,
        _ => return None,
    })
}

fn convert_default(ty: PropType, v: &Value) -> Result<ScriptValue, String> {
    let wrong = || {
        format!(
            "default {} does not match type {}",
            v.type_name(),
            ty.name()
        )
    };
    Ok(match (ty, v) {
        (PropType::Number, Value::Number(n)) => ScriptValue::Number(*n),
        (PropType::Number, Value::Integer(n)) => ScriptValue::Number(*n as f64),
        (PropType::Integer, Value::Integer(n)) => ScriptValue::Integer(*n),
        (PropType::Integer, Value::Number(n)) if n.fract() == 0.0 => {
            ScriptValue::Integer(*n as i64)
        }
        (PropType::Boolean, Value::Boolean(b)) => ScriptValue::Boolean(*b),
        (PropType::String, Value::String(s)) => {
            ScriptValue::String(s.to_str().map_err(|e| e.to_string())?.to_string())
        }
        (PropType::Vector, Value::Vector(x)) => ScriptValue::Vector(from_vector(*x)),
        (PropType::Color, Value::Table(_)) => {
            ScriptValue::Color(to_color(v, Color::WHITE, "default").map_err(|e| e.to_string())?)
        }
        (PropType::Asset, Value::String(s)) => {
            ScriptValue::Asset(AssetRef::path(&s.to_str().map_err(|e| e.to_string())?))
        }
        (PropType::Entity, Value::Nil) => ScriptValue::Entity(None),
        _ => return Err(wrong()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    fn decls(src: &str) -> Vec<PropertyDecl> {
        let lua = Lua::new();
        crate::api::install_import(&lua).unwrap();
        let module: Value = lua.load(src).eval().unwrap();
        describe_module(&lua, "scripts/t.luau", src, module)
            .unwrap()
            .properties
    }

    #[test]
    fn properties_keep_declaration_order_and_infer_types() {
        let src = r#"
            local M = {}
            M.properties = {
                speed = { type = "number", default = 90, min = 0, max = 720, unit = "deg/s", doc = "Turn rate" },
                axis = { default = vector.create(0, 1, 0) },
                enabled = true,
                tint = Color.rgb(1, 0, 0),
                target = { type = "entity" },
                count = { type = "integer", default = 3 },
            }
            function M.on_update(self, dt) end
            return M
        "#;
        let d = decls(src);
        let names: Vec<&str> = d.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            ["speed", "axis", "enabled", "tint", "target", "count"]
        );
        assert_eq!(d[0].range, Some((0.0, 720.0)));
        assert_eq!(d[0].unit.as_deref(), Some("deg/s"));
        assert_eq!(d[1].ty, PropType::Vector);
        assert_eq!(d[1].default, ScriptValue::Vector(Vec3::Y));
        assert_eq!(d[2].default, ScriptValue::Boolean(true));
        assert_eq!(d[3].ty, PropType::Color);
        assert_eq!(d[4].default, ScriptValue::Entity(None));
        assert_eq!(d[5].default, ScriptValue::Integer(3));
    }

    #[test]
    fn bad_declarations_name_the_property() {
        let lua = Lua::new();
        crate::api::install_import(&lua).unwrap();
        let src = "return { properties = { speed = { type = \"numbr\" } } }";
        let module: Value = lua.load(src).eval().unwrap();
        let e = describe_module(&lua, "scripts/t.luau", src, module).unwrap_err();
        assert!(
            e.to_string()
                .contains("property `speed`: unknown type `numbr`"),
            "{e}"
        );
    }
}
