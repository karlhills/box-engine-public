//! Conversions between engine values and Luau values (decided in spike S4).
//!
//! | Engine | Luau |
//! |---|---|
//! | `Entity` | light userdata with the entity methods (no allocation, compares by value, usable as a table key) |
//! | `Vec3`, `Vec3A` | native `vector` |
//! | `Vec2` | `vector` with `z = 0` |
//! | `Quat` | `Quat` userdata (`q * v`, `q * q`, methods) |
//! | `Color` | table `{ r, g, b, a }` (linear) |
//! | numbers, `bool`, `String` | number, boolean, string |
//! | `AssetRef` | string (path, `#label`; an id when there is no path), `nil` for none |
//! | `Uid` | string |
//! | struct | table of fields |
//! | newtype (`Name(String)`) | its field |
//! | `Option<T>` | `nil` or the value |
//! | unit enum variant | string (`"Opaque"`) |
//! | enum variant with data | `{ Variant = payload }` |
//! | list, array, tuple | array table |
//!
//! Values are **copies**: `e:get("Transform")` returns a fresh table, and
//! writes go back through `e:set(...)`, which **patches** the target with the
//! fields the table has (type-directed, so `{ translation = v }` changes only
//! the translation). Types the walk does not know go through their scene JSON
//! form.

use std::any::TypeId;

use bevy_reflect::enums::{DynamicEnum, DynamicVariant, VariantInfo};
use bevy_reflect::structs::DynamicStruct;
use bevy_reflect::tuple::DynamicTuple;
use bevy_reflect::{PartialReflect, ReflectMut, ReflectRef, TypeInfo};
use box_core::asset_ref::{AssetId, AssetRef};
use box_core::color::Color;
use box_core::identity::Uid;
use box_core::reflect::{Registry, did_you_mean};
use box_core::world::Entity;
use glam::{Quat, Vec2, Vec3, Vec3A};
use mlua::{
    FromLua, LightUserData, Lua, MetaMethod, Result as LuaResult, Table, UserData, UserDataFields,
    UserDataMethods, Value, Vector,
};

// ----- entities -----

/// An entity as a Luau value: a light userdata holding its bits.
pub fn entity_value(e: Entity) -> Value {
    Value::LightUserData(LightUserData(std::ptr::without_provenance_mut(
        e.to_bits().get() as usize,
    )))
}

/// The entity a Luau value holds, if it is one.
pub fn to_entity(v: &Value) -> Option<Entity> {
    match v {
        Value::LightUserData(l) => Entity::from_bits(l.0 as usize as u64),
        _ => None,
    }
}

/// [`to_entity`] or a Lua error naming what was passed.
pub fn check_entity(v: &Value, what: &str) -> LuaResult<Entity> {
    to_entity(v).ok_or_else(|| {
        mlua::Error::runtime(format!("{what}: expected an Entity, got {}", v.type_name()))
    })
}

// ----- vectors -----

pub fn vector(v: Vec3) -> Vector {
    Vector::new(v.x, v.y, v.z)
}

pub fn from_vector(v: Vector) -> Vec3 {
    Vec3::new(v.x(), v.y(), v.z())
}

/// A `vector`, a table with `x`, `y`, `z`, or a list of up to three
/// numbers `{x, y, z}` (missing components from `like`).
pub fn to_vec3(v: &Value, like: Vec3, what: &str) -> LuaResult<Vec3> {
    match v {
        Value::Vector(v) => Ok(from_vector(*v)),
        Value::Table(t) if t.raw_len() > 0 => {
            let n = t.raw_len();
            if n > 3 {
                return Err(mlua::Error::runtime(format!(
                    "{what}: expected a vector or at most 3 numbers, got a list of {n}"
                )));
            }
            let mut out = like;
            for i in 0..n {
                out[i] = t.raw_get::<f32>(i + 1).map_err(|_| {
                    mlua::Error::runtime(format!("{what}[{}]: expected a number", i + 1))
                })?;
            }
            Ok(out)
        }
        Value::Table(t) => Ok(Vec3::new(
            t.get::<Option<f32>>("x")?.unwrap_or(like.x),
            t.get::<Option<f32>>("y")?.unwrap_or(like.y),
            t.get::<Option<f32>>("z")?.unwrap_or(like.z),
        )),
        other => Err(mlua::Error::runtime(format!(
            "{what}: expected a vector, got {}",
            other.type_name()
        ))),
    }
}

// ----- quaternions -----

/// A rotation: `Quat` in scripts. `q * v` rotates a vector, `q * r`
/// composes (r first); fields `x`, `y`, `z`, `w`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LuaQuat(pub Quat);

impl FromLua for LuaQuat {
    fn from_lua(v: Value, _: &Lua) -> LuaResult<Self> {
        to_quat(&v, Quat::IDENTITY, "Quat").map(LuaQuat)
    }
}

/// A `Quat`, or a table with `x`, `y`, `z`, `w`.
pub fn to_quat(v: &Value, like: Quat, what: &str) -> LuaResult<Quat> {
    match v {
        Value::UserData(ud) => ud
            .borrow::<LuaQuat>()
            .map(|q| q.0)
            .map_err(|_| mlua::Error::runtime(format!("{what}: expected a Quat"))),
        Value::Table(t) => Ok(Quat::from_xyzw(
            t.get::<Option<f32>>("x")?.unwrap_or(like.x),
            t.get::<Option<f32>>("y")?.unwrap_or(like.y),
            t.get::<Option<f32>>("z")?.unwrap_or(like.z),
            t.get::<Option<f32>>("w")?.unwrap_or(like.w),
        )
        .normalize()),
        other => Err(mlua::Error::runtime(format!(
            "{what}: expected a Quat, got {}",
            other.type_name()
        ))),
    }
}

impl UserData for LuaQuat {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("x", |_, q| Ok(q.0.x));
        f.add_field_method_get("y", |_, q| Ok(q.0.y));
        f.add_field_method_get("z", |_, q| Ok(q.0.z));
        f.add_field_method_get("w", |_, q| Ok(q.0.w));
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_meta_function(MetaMethod::Mul, |lua, (a, b): (LuaQuat, Value)| match b {
            Value::Vector(v) => Ok(Value::Vector(vector(a.0 * from_vector(v)))),
            other => {
                let b = to_quat(&other, Quat::IDENTITY, "Quat * ?")?;
                lua.create_userdata(LuaQuat((a.0 * b).normalize()))
                    .map(Value::UserData)
            }
        });
        m.add_meta_method(MetaMethod::Eq, |_, a, b: LuaQuat| Ok(a.0 == b.0));
        m.add_meta_method(MetaMethod::ToString, |_, q, ()| {
            Ok(format!(
                "Quat({:.4}, {:.4}, {:.4}, {:.4})",
                q.0.x, q.0.y, q.0.z, q.0.w
            ))
        });
        m.add_method("inverse", |_, q, ()| Ok(LuaQuat(q.0.inverse())));
        m.add_method("normalize", |_, q, ()| Ok(LuaQuat(q.0.normalize())));
        m.add_method("slerp", |_, q, (to, t): (LuaQuat, f32)| {
            Ok(LuaQuat(q.0.slerp(to.0, t)))
        });
        m.add_method("angle_to", |_, q, to: LuaQuat| Ok(q.0.angle_between(to.0)));
        m.add_method("forward", |_, q, ()| Ok(vector(q.0 * Vec3::NEG_Z)));
        m.add_method("right", |_, q, ()| Ok(vector(q.0 * Vec3::X)));
        m.add_method("up", |_, q, ()| Ok(vector(q.0 * Vec3::Y)));
        m.add_method("to_euler", |_, q, ()| {
            let (y, x, z) = q.0.to_euler(glam::EulerRot::YXZ);
            Ok((x, y, z))
        });
        m.add_method("to_axis_angle", |_, q, ()| {
            let (axis, angle) = q.0.to_axis_angle();
            Ok((vector(axis), angle))
        });
    }
}

/// A Lua `Quat`.
pub fn quat_value(lua: &Lua, q: Quat) -> LuaResult<Value> {
    lua.create_userdata(LuaQuat(q)).map(Value::UserData)
}

// ----- colours -----

/// A colour table `{ r, g, b, a }` (linear).
pub fn color_table(lua: &Lua, c: Color) -> LuaResult<Table> {
    let t = lua.create_table_with_capacity(0, 4)?;
    t.raw_set("r", c.r)?;
    t.raw_set("g", c.g)?;
    t.raw_set("b", c.b)?;
    t.raw_set("a", c.a)?;
    Ok(t)
}

/// A colour table (missing channels from `like`).
pub fn to_color(v: &Value, like: Color, what: &str) -> LuaResult<Color> {
    match v {
        Value::Table(t) => Ok(Color {
            r: t.get::<Option<f32>>("r")?.unwrap_or(like.r),
            g: t.get::<Option<f32>>("g")?.unwrap_or(like.g),
            b: t.get::<Option<f32>>("b")?.unwrap_or(like.b),
            a: t.get::<Option<f32>>("a")?.unwrap_or(like.a),
        }),
        other => Err(mlua::Error::runtime(format!(
            "{what}: expected a Color table {{ r, g, b, a }}, got {}",
            other.type_name()
        ))),
    }
}

// ----- asset references -----

/// An asset reference as a script string; `nil` for none.
pub fn asset_value(lua: &Lua, r: &AssetRef) -> LuaResult<Value> {
    if r.is_none() {
        Ok(Value::Nil)
    } else {
        lua.create_string(r.to_string()).map(Value::String)
    }
}

/// A script string (a path, `path#label`, or an asset id) or `nil`.
pub fn to_asset(v: &Value, what: &str) -> LuaResult<AssetRef> {
    match v {
        Value::Nil => Ok(AssetRef::NONE),
        Value::String(s) => {
            let s = s.to_str()?;
            if let Some(id) = AssetId::parse(&s) {
                return Ok(AssetRef::id(id));
            }
            serde_json::from_value(serde_json::Value::String(s.to_string()))
                .map_err(|e| mlua::Error::runtime(format!("{what}: {e}")))
        }
        other => Err(mlua::Error::runtime(format!(
            "{what}: expected an asset path string, got {}",
            other.type_name()
        ))),
    }
}

// ----- reflection -> Luau -----

fn number(lua_v: f64) -> Value {
    Value::Number(lua_v)
}

/// Scalars and the engine's value types; `None` for anything else.
fn scalar_to_lua(lua: &Lua, v: &dyn PartialReflect) -> LuaResult<Option<Value>> {
    // One virtual call for the type id, then plain comparisons.
    let Some(any) = v.try_as_reflect().map(|r| r.as_any()) else {
        return Ok(None);
    };
    let id = any.type_id();
    macro_rules! num {
        ($($t:ty),*) => {
            $(if id == TypeId::of::<$t>() {
                return Ok(any.downcast_ref::<$t>().map(|x| number(*x as f64)));
            })*
        };
    }
    num!(f32, f64, i32, u32, i64, u64, usize, i8, i16, isize, u8, u16);
    macro_rules! cast {
        ($t:ty, $x:ident => $e:expr) => {
            if id == TypeId::of::<$t>() {
                return match any.downcast_ref::<$t>() {
                    Some($x) => $e.map(Some),
                    None => Ok(None),
                };
            }
        };
    }
    cast!(Vec3, x => LuaResult::Ok(Value::Vector(vector(*x))));
    cast!(Quat, x => quat_value(lua, *x));
    cast!(bool, x => LuaResult::Ok(Value::Boolean(*x)));
    cast!(String, x => lua.create_string(x).map(Value::String));
    cast!(Color, x => color_table(lua, *x).map(Value::Table));
    cast!(Vec2, x => LuaResult::Ok(Value::Vector(Vector::new(x.x, x.y, 0.0))));
    cast!(Vec3A, x => LuaResult::Ok(Value::Vector(vector((*x).into()))));
    cast!(AssetRef, x => asset_value(lua, x));
    cast!(Uid, x => lua.create_string(x.to_string()).map(Value::String));
    Ok(None)
}

fn is_option(type_path: &str) -> bool {
    type_path.starts_with("core::option::Option")
}

/// A copy of a reflected value as a Luau value (see the module table).
pub fn reflect_to_lua(lua: &Lua, registry: &Registry, v: &dyn PartialReflect) -> LuaResult<Value> {
    if let Some(s) = scalar_to_lua(lua, v)? {
        return Ok(s);
    }
    let array = |items: &mut dyn Iterator<Item = &dyn PartialReflect>| -> LuaResult<Value> {
        let t = lua.create_table()?;
        for (i, item) in items.enumerate() {
            t.raw_set(i + 1, reflect_to_lua(lua, registry, item)?)?;
        }
        Ok(Value::Table(t))
    };
    match v.reflect_ref() {
        ReflectRef::Struct(s) => {
            let t = lua.create_table_with_capacity(0, s.field_len())?;
            for i in 0..s.field_len() {
                if let (Some(name), Some(field)) = (s.name_at(i), s.field_at(i)) {
                    t.raw_set(name, reflect_to_lua(lua, registry, field)?)?;
                }
            }
            Ok(Value::Table(t))
        }
        ReflectRef::TupleStruct(t) if t.field_len() == 1 => match t.field(0) {
            Some(f) => reflect_to_lua(lua, registry, f),
            None => Ok(Value::Nil),
        },
        ReflectRef::TupleStruct(t) => array(&mut t.iter_fields()),
        ReflectRef::Tuple(t) => array(&mut t.iter_fields()),
        ReflectRef::List(l) => array(&mut l.iter()),
        ReflectRef::Array(a) => array(&mut a.iter()),
        ReflectRef::Set(s) => array(&mut s.iter()),
        ReflectRef::Map(m) => {
            let t = lua.create_table()?;
            for (k, val) in m.iter() {
                t.raw_set(
                    reflect_to_lua(lua, registry, k)?,
                    reflect_to_lua(lua, registry, val)?,
                )?;
            }
            Ok(Value::Table(t))
        }
        ReflectRef::Enum(e) => {
            if is_option(e.reflect_type_path()) {
                return match e.field_at(0) {
                    Some(inner) => reflect_to_lua(lua, registry, inner),
                    None => Ok(Value::Nil),
                };
            }
            let name = e.variant_name();
            let payload = match e.variant_type() {
                bevy_reflect::enums::VariantType::Unit => {
                    return lua.create_string(name).map(Value::String);
                }
                bevy_reflect::enums::VariantType::Tuple if e.field_len() == 1 => {
                    match e.field_at(0) {
                        Some(f) => reflect_to_lua(lua, registry, f)?,
                        None => Value::Nil,
                    }
                }
                bevy_reflect::enums::VariantType::Tuple => {
                    let t = lua.create_table()?;
                    for i in 0..e.field_len() {
                        if let Some(f) = e.field_at(i) {
                            t.raw_set(i + 1, reflect_to_lua(lua, registry, f)?)?;
                        }
                    }
                    Value::Table(t)
                }
                bevy_reflect::enums::VariantType::Struct => {
                    let t = lua.create_table()?;
                    for i in 0..e.field_len() {
                        if let (Some(n), Some(f)) = (e.name_at(i), e.field_at(i)) {
                            t.raw_set(n, reflect_to_lua(lua, registry, f)?)?;
                        }
                    }
                    Value::Table(t)
                }
            };
            let t = lua.create_table_with_capacity(0, 1)?;
            t.raw_set(name, payload)?;
            Ok(Value::Table(t))
        }
        _ => {
            let json = registry
                .value_to_json(v)
                .map_err(|e| mlua::Error::runtime(e.to_string()))?;
            json_to_lua(lua, &json)
        }
    }
}

// ----- Luau -> reflection (patch) -----

/// A default value of the registered type `type_id`
/// ([`Registry::default_value`]: its `#[reflect(Default)]`, else an empty
/// container or `None`).
pub fn default_of(registry: &Registry, type_id: TypeId) -> Option<Box<dyn PartialReflect>> {
    registry.default_value(type_id)
}

fn err(path: &str, message: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::runtime(format!("{path}: {message}"))
}

fn as_f64(v: &Value, path: &str) -> LuaResult<f64> {
    match v {
        Value::Number(n) => Ok(*n),
        Value::Integer(i) => Ok(*i as f64),
        other => Err(err(
            path,
            format!("expected a number, got {}", other.type_name()),
        )),
    }
}

/// Patches scalars and engine value types; `None` when `target` is not one.
fn apply_scalar(target: &mut dyn PartialReflect, v: &Value, path: &str) -> Option<LuaResult<()>> {
    // Structs and lists (most targets) leave after one type id check.
    let id = target.try_as_reflect().map(|r| r.as_any().type_id())?;
    let scalars = [
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
        TypeId::of::<bool>(),
        TypeId::of::<String>(),
        TypeId::of::<Vec3>(),
        TypeId::of::<Vec3A>(),
        TypeId::of::<Vec2>(),
        TypeId::of::<Quat>(),
        TypeId::of::<Color>(),
        TypeId::of::<AssetRef>(),
        TypeId::of::<Uid>(),
    ];
    if !scalars.contains(&id) {
        return None;
    }
    macro_rules! float {
        ($($t:ty),*) => {
            $(if let Some(x) = target.try_downcast_mut::<$t>() {
                return Some(as_f64(v, path).map(|n| *x = n as $t));
            })*
        };
    }
    macro_rules! int {
        ($($t:ty),*) => {
            $(if let Some(x) = target.try_downcast_mut::<$t>() {
                return Some(as_f64(v, path).and_then(|n| {
                    if n.fract() != 0.0 || n < <$t>::MIN as f64 || n > <$t>::MAX as f64 {
                        return Err(err(path, format!(
                            "{n} is not a whole number in {}..{}",
                            <$t>::MIN,
                            <$t>::MAX
                        )));
                    }
                    *x = n as $t;
                    Ok(())
                }));
            })*
        };
    }
    float!(f32, f64);
    int!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);
    if let Some(x) = target.try_downcast_mut::<bool>() {
        return Some(match v {
            Value::Boolean(b) => {
                *x = *b;
                Ok(())
            }
            other => Err(err(
                path,
                format!("expected a boolean, got {}", other.type_name()),
            )),
        });
    }
    if let Some(x) = target.try_downcast_mut::<String>() {
        return Some(match v {
            Value::String(s) => s.to_str().map(|s| *x = s.to_string()),
            other => Err(err(
                path,
                format!("expected a string, got {}", other.type_name()),
            )),
        });
    }
    if let Some(x) = target.try_downcast_mut::<Vec3>() {
        return Some(to_vec3(v, *x, path).map(|n| *x = n));
    }
    if let Some(x) = target.try_downcast_mut::<Vec3A>() {
        return Some(to_vec3(v, (*x).into(), path).map(|n| *x = n.into()));
    }
    if let Some(x) = target.try_downcast_mut::<Vec2>() {
        return Some(to_vec3(v, x.extend(0.0), path).map(|n| *x = n.truncate()));
    }
    if let Some(x) = target.try_downcast_mut::<Quat>() {
        return Some(to_quat(v, *x, path).map(|n| *x = n));
    }
    if let Some(x) = target.try_downcast_mut::<Color>() {
        return Some(to_color(v, *x, path).map(|n| *x = n));
    }
    if let Some(x) = target.try_downcast_mut::<AssetRef>() {
        return Some(to_asset(v, path).map(|n| *x = n));
    }
    if let Some(x) = target.try_downcast_mut::<Uid>() {
        return Some(match v {
            Value::String(s) => s
                .to_str()
                .ok()
                .and_then(|s| Uid::parse(&s))
                .map(|u| *x = u)
                .ok_or_else(|| err(path, "expected a uid string")),
            other => Err(err(
                path,
                format!("expected a uid string, got {}", other.type_name()),
            )),
        });
    }
    None
}

fn type_id_of(info: Option<&'static TypeInfo>) -> Option<TypeId> {
    info.map(TypeInfo::type_id)
}

/// A fresh value of the type `info` describes: its default, patched with `v`.
fn construct(
    lua: &Lua,
    registry: &Registry,
    type_id: TypeId,
    v: &Value,
    path: &str,
) -> LuaResult<Box<dyn PartialReflect>> {
    let mut value = default_of(registry, type_id).ok_or_else(|| {
        err(
            path,
            "this type has no default value, so it cannot be built from a script",
        )
    })?;
    apply_lua(lua, registry, value.as_mut(), v, path)?;
    Ok(value)
}

/// The fields of a new variant of the enum `enum_type`: each field's
/// default (the enum's `VariantDefaults` for that variant, else the field
/// type's), patched from `payload`.
fn build_variant(
    lua: &Lua,
    registry: &Registry,
    enum_type: TypeId,
    variant: &VariantInfo,
    payload: &Value,
    path: &str,
) -> LuaResult<DynamicVariant> {
    match variant {
        VariantInfo::Unit(_) => Ok(DynamicVariant::Unit),
        VariantInfo::Tuple(t) => {
            let mut tuple = DynamicTuple::default();
            for i in 0..t.field_len() {
                let Some(field) = t.field_at(i) else { continue };
                let item = if t.field_len() == 1 {
                    payload.clone()
                } else {
                    match payload {
                        Value::Table(tab) => tab.raw_get::<Value>(i + 1)?,
                        _ => Value::Nil,
                    }
                };
                let value = if item.is_nil() {
                    default_of(registry, field.type_id())
                        .ok_or_else(|| err(path, "a variant field has no default"))?
                } else {
                    construct(lua, registry, field.type_id(), &item, path)?
                };
                tuple.insert_boxed(value);
            }
            Ok(DynamicVariant::Tuple(tuple))
        }
        VariantInfo::Struct(s) => {
            let mut fields = DynamicStruct::default();
            let table = match payload {
                Value::Table(t) => Some(t),
                Value::Nil => None,
                other => {
                    return Err(err(
                        path,
                        format!("expected a table of fields, got {}", other.type_name()),
                    ));
                }
            };
            for i in 0..s.field_len() {
                let Some(field) = s.field_at(i) else { continue };
                let item = match table {
                    Some(t) => t.raw_get::<Value>(field.name())?,
                    None => Value::Nil,
                };
                let sub = format!("{path}.{}", field.name());
                let value = if item.is_nil() {
                    registry
                        .variant_field_default(enum_type, s.name(), field.name())
                        .or_else(|| default_of(registry, field.type_id()))
                        .ok_or_else(|| err(&sub, "this field has no default"))?
                } else {
                    construct(lua, registry, field.type_id(), &item, &sub)?
                };
                fields.insert_boxed(field.name(), value);
            }
            Ok(DynamicVariant::Struct(fields))
        }
    }
}

/// Patches `target` with a Luau value (see the module docs): scalars are
/// replaced, tables patch the fields they name, lists take the table's
/// length, enum variants switch. `path` names the target in errors
/// (`Transform.translation`).
pub fn apply_lua(
    lua: &Lua,
    registry: &Registry,
    target: &mut dyn PartialReflect,
    v: &Value,
    path: &str,
) -> LuaResult<()> {
    if let Some(done) = apply_scalar(target, v, path) {
        return done;
    }
    let represented = target.get_represented_type_info();
    match (target.reflect_mut(), v) {
        (ReflectMut::Struct(s), Value::Table(t)) => {
            for pair in t.pairs::<Value, Value>() {
                let (k, val) = pair?;
                let Value::String(k) = k else {
                    return Err(err(path, "field names must be strings"));
                };
                let k = k.to_str()?;
                if s.field(&k).is_none() {
                    let names: Vec<String> = (0..s.field_len())
                        .filter_map(|i| s.name_at(i).map(str::to_owned))
                        .collect();
                    let mut msg = format!("no field `{}`; fields: {}", &*k, names.join(", "));
                    if let Some(g) = did_you_mean(&k, names.iter().map(String::as_str)) {
                        msg.push_str(&format!(" (did you mean `{g}`?)"));
                    }
                    return Err(err(path, msg));
                }
                let sub = format!("{path}.{}", &*k);
                if let Some(field) = s.field_mut(&k) {
                    apply_lua(lua, registry, field, &val, &sub)?;
                }
            }
            Ok(())
        }
        (ReflectMut::TupleStruct(t), _) if t.field_len() == 1 => match t.field_mut(0) {
            Some(f) => apply_lua(lua, registry, f, v, path),
            None => Ok(()),
        },
        (ReflectMut::TupleStruct(t), Value::Table(tab)) => {
            for i in 0..t.field_len() {
                let item: Value = tab.raw_get(i + 1)?;
                if let (false, Some(f)) = (item.is_nil(), t.field_mut(i)) {
                    apply_lua(lua, registry, f, &item, &format!("{path}[{}]", i + 1))?;
                }
            }
            Ok(())
        }
        (ReflectMut::Tuple(t), Value::Table(tab)) => {
            for i in 0..t.field_len() {
                let item: Value = tab.raw_get(i + 1)?;
                if let (false, Some(f)) = (item.is_nil(), t.field_mut(i)) {
                    apply_lua(lua, registry, f, &item, &format!("{path}[{}]", i + 1))?;
                }
            }
            Ok(())
        }
        (ReflectMut::Array(a), Value::Table(tab)) => {
            let n = tab.raw_len();
            if n != a.len() {
                return Err(err(path, format!("expected {} items, got {n}", a.len())));
            }
            for i in 0..n {
                let item: Value = tab.raw_get(i + 1)?;
                if let Some(f) = a.get_mut(i) {
                    apply_lua(lua, registry, f, &item, &format!("{path}[{}]", i + 1))?;
                }
            }
            Ok(())
        }
        (ReflectMut::List(l), Value::Table(tab)) => {
            let n = tab.raw_len();
            while l.len() > n {
                l.pop();
            }
            let item_type = match represented {
                Some(TypeInfo::List(info)) => Some(info.item_ty().id()),
                _ => None,
            };
            for i in 0..n {
                let item: Value = tab.raw_get(i + 1)?;
                let sub = format!("{path}[{}]", i + 1);
                if let Some(f) = l.get_mut(i) {
                    apply_lua(lua, registry, f, &item, &sub)?;
                } else {
                    let ty = item_type.ok_or_else(|| err(&sub, "unknown list item type"))?;
                    l.push(construct(lua, registry, ty, &item, &sub)?);
                }
            }
            Ok(())
        }
        (ReflectMut::Enum(e), _) => {
            let info = match represented {
                Some(TypeInfo::Enum(info)) => info,
                _ => return Err(err(path, "unknown enum type")),
            };
            if is_option(info.type_path()) {
                if v.is_nil() {
                    let none = DynamicEnum::new("None", DynamicVariant::Unit);
                    return e.try_apply(&none).map_err(|x| err(path, x.to_string()));
                }
                if e.variant_name() == "Some"
                    && let Some(inner) = e.field_at_mut(0)
                {
                    return apply_lua(lua, registry, inner, v, path);
                }
                let some = info
                    .variant("Some")
                    .ok_or_else(|| err(path, "Option without Some"))?;
                let variant = build_variant(lua, registry, info.type_id(), some, v, path)?;
                return e
                    .try_apply(&DynamicEnum::new("Some", variant))
                    .map_err(|x| err(path, x.to_string()));
            }
            let (name, payload) = match v {
                Value::String(s) => (s.to_str()?.to_string(), Value::Nil),
                Value::Table(t) => {
                    let mut pairs = t.pairs::<String, Value>();
                    match (pairs.next(), pairs.next()) {
                        (Some(Ok((k, p))), None) => (k, p),
                        _ => {
                            return Err(err(
                                path,
                                "an enum value is a variant name or a one-key table { Variant = value }",
                            ));
                        }
                    }
                }
                other => {
                    return Err(err(
                        path,
                        format!(
                            "expected a variant name or {{ Variant = value }}, got {}",
                            other.type_name()
                        ),
                    ));
                }
            };
            let Some(variant) = info.variant(&name) else {
                let names: Vec<&str> = info.variant_names().to_vec();
                let mut msg = format!("no variant `{name}`; variants: {}", names.join(", "));
                if let Some(g) = did_you_mean(&name, names.iter().copied()) {
                    msg.push_str(&format!(" (did you mean `{g}`?)"));
                }
                return Err(err(path, msg));
            };
            if e.variant_name() == name {
                // Same variant: patch its fields.
                return match variant {
                    VariantInfo::Unit(_) => Ok(()),
                    VariantInfo::Tuple(t) if t.field_len() == 1 => match e.field_at_mut(0) {
                        Some(f) if !payload.is_nil() => apply_lua(lua, registry, f, &payload, path),
                        _ => Ok(()),
                    },
                    VariantInfo::Tuple(_) | VariantInfo::Struct(_) => {
                        let Value::Table(tab) = &payload else {
                            return Ok(());
                        };
                        for i in 0..e.field_len() {
                            let key: Value = match e.name_at(i) {
                                Some(n) => Value::String(lua.create_string(n)?),
                                None => Value::Integer((i + 1) as i64),
                            };
                            let item: Value = tab.raw_get(key)?;
                            let sub = match e.name_at(i) {
                                Some(n) => format!("{path}.{n}"),
                                None => format!("{path}[{}]", i + 1),
                            };
                            if let (false, Some(f)) = (item.is_nil(), e.field_at_mut(i)) {
                                apply_lua(lua, registry, f, &item, &sub)?;
                            }
                        }
                        Ok(())
                    }
                };
            }
            let built = build_variant(lua, registry, info.type_id(), variant, &payload, path)?;
            e.try_apply(&DynamicEnum::new(name, built))
                .map_err(|x| err(path, x.to_string()))
        }
        (_, value) => {
            // Opaque and map types: through their scene JSON form.
            let type_id = type_id_of(represented)
                .ok_or_else(|| err(path, "a value of unknown type cannot be set"))?;
            let json = lua_to_json(value, &|_| None)
                .map_err(|e| err(path, format!("cannot convert to JSON: {e}")))?;
            let parsed = registry
                .value_from_json(type_id, &json)
                .map_err(|e| err(path, e))?;
            box_core::reflect::replace(target, parsed.as_ref()).map_err(|e| err(path, e))
        }
    }
}

// ----- JSON -----

/// A JSON value as a Luau value. The tagged forms written by
/// [`lua_to_json`] come back as vectors and quaternions; `{"$entity": uid}`
/// comes back as the uid string (see [`json_to_lua_with`]).
pub fn json_to_lua(lua: &Lua, json: &serde_json::Value) -> LuaResult<Value> {
    json_to_lua_with(lua, json, &|uid| match lua.create_string(uid) {
        Ok(s) => Value::String(s),
        Err(_) => Value::Nil,
    })
}

/// [`json_to_lua`] with `entity` turning each `{"$entity": uid}` into a
/// value (an entity handle, or `nil` when the uid names no entity).
pub fn json_to_lua_with(
    lua: &Lua,
    json: &serde_json::Value,
    entity: &dyn Fn(&str) -> Value,
) -> LuaResult<Value> {
    use serde_json::Value as J;
    Ok(match json {
        J::Null => Value::Nil,
        J::Bool(b) => Value::Boolean(*b),
        J::Number(n) => Value::Number(n.as_f64().unwrap_or(0.0)),
        J::String(s) => Value::String(lua.create_string(s)?),
        J::Array(items) => {
            let t = lua.create_table_with_capacity(items.len(), 0)?;
            for (i, item) in items.iter().enumerate() {
                t.raw_set(i + 1, json_to_lua_with(lua, item, entity)?)?;
            }
            Value::Table(t)
        }
        J::Object(map) => {
            if map.len() == 1
                && let Some((tag, inner)) = map.iter().next()
            {
                let floats = |n: usize| -> Option<Vec<f32>> {
                    let a = inner.as_array()?;
                    (a.len() == n)
                        .then(|| a.iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect())
                };
                match tag.as_str() {
                    "$vector" => {
                        if let Some(f) = floats(3) {
                            return Ok(Value::Vector(Vector::new(f[0], f[1], f[2])));
                        }
                    }
                    "$quat" => {
                        if let Some(f) = floats(4) {
                            return quat_value(lua, Quat::from_xyzw(f[0], f[1], f[2], f[3]));
                        }
                    }
                    "$entity" => {
                        return Ok(match inner.as_str() {
                            Some(uid) => entity(uid),
                            None => Value::Nil,
                        });
                    }
                    _ => {}
                }
            }
            let t = lua.create_table_with_capacity(0, map.len())?;
            for (k, v) in map {
                t.raw_set(k.as_str(), json_to_lua_with(lua, v, entity)?)?;
            }
            Value::Table(t)
        }
    })
}

/// A Luau value as JSON: arrays for sequences, objects for other tables,
/// `{"$vector": [x, y, z]}`, `{"$quat": [x, y, z, w]}` and
/// `{"$entity": uid}` (through `entity_uid`; `null` for an entity without
/// one). Functions, threads and other userdata cannot be converted.
pub fn lua_to_json(
    v: &Value,
    entity_uid: &dyn Fn(Entity) -> Option<String>,
) -> Result<serde_json::Value, String> {
    to_json_depth(v, entity_uid, 0)
}

fn to_json_depth(
    v: &Value,
    entity_uid: &dyn Fn(Entity) -> Option<String>,
    depth: usize,
) -> Result<serde_json::Value, String> {
    use serde_json::{Map, Number, Value as J};
    if depth > 64 {
        return Err("nested more than 64 levels (a cycle?)".into());
    }
    Ok(match v {
        Value::Nil => J::Null,
        Value::Boolean(b) => J::Bool(*b),
        Value::Integer(i) => J::Number((*i).into()),
        Value::Number(n) => {
            if n.fract() == 0.0 && n.abs() < 9.0e15 {
                J::Number((*n as i64).into())
            } else {
                J::Number(Number::from_f64(*n).ok_or("not a finite number")?)
            }
        }
        Value::String(s) => J::String(s.to_string_lossy()),
        Value::Vector(vec) => {
            let mut m = Map::new();
            m.insert(
                "$vector".into(),
                J::Array(
                    [vec.x(), vec.y(), vec.z()]
                        .iter()
                        .map(|f| Number::from_f64(f64::from(*f)).map_or(J::Null, J::Number))
                        .collect(),
                ),
            );
            J::Object(m)
        }
        Value::LightUserData(_) => {
            let e = to_entity(v).ok_or("bad entity")?;
            let mut m = Map::new();
            m.insert("$entity".into(), entity_uid(e).map_or(J::Null, J::String));
            J::Object(m)
        }
        Value::UserData(ud) => {
            let q = ud
                .borrow::<LuaQuat>()
                .map_err(|_| "a userdata other than Quat".to_string())?;
            let mut m = Map::new();
            m.insert(
                "$quat".into(),
                J::Array(
                    [q.0.x, q.0.y, q.0.z, q.0.w]
                        .iter()
                        .map(|f| Number::from_f64(f64::from(*f)).map_or(J::Null, J::Number))
                        .collect(),
                ),
            );
            J::Object(m)
        }
        Value::Table(t) => {
            let n = t.raw_len();
            let mut count = 0usize;
            for pair in t.pairs::<Value, Value>() {
                pair.map_err(|e| e.to_string())?;
                count += 1;
            }
            if n > 0 && count == n {
                let mut items = Vec::with_capacity(n);
                for i in 1..=n {
                    let item: Value = t.raw_get(i).map_err(|e| e.to_string())?;
                    items.push(to_json_depth(&item, entity_uid, depth + 1)?);
                }
                J::Array(items)
            } else {
                let mut m = Map::new();
                for pair in t.pairs::<Value, Value>() {
                    let (k, val) = pair.map_err(|e| e.to_string())?;
                    let key = match k {
                        Value::String(s) => s.to_string_lossy(),
                        Value::Integer(i) => i.to_string(),
                        Value::Number(n) => n.to_string(),
                        Value::Boolean(b) => b.to_string(),
                        other => return Err(format!("a table key of type {}", other.type_name())),
                    };
                    m.insert(key, to_json_depth(&val, entity_uid, depth + 1)?);
                }
                J::Object(m)
            }
        }
        other => return Err(format!("a {} cannot be saved", other.type_name())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use box_core::transform::Transform;

    fn lua() -> Lua {
        Lua::new()
    }

    #[test]
    fn transforms_round_trip_and_patch() {
        let lua = lua();
        let registry = Registry::with_core_types();
        let t = Transform::from_xyz(1.0, 2.0, 3.0);
        let v = reflect_to_lua(&lua, &registry, &t).unwrap();
        let Value::Table(tab) = &v else { panic!() };
        let p: Vector = tab.get("translation").unwrap();
        assert_eq!(from_vector(p), Vec3::new(1.0, 2.0, 3.0));
        let mut target = Transform::default();
        apply_lua(&lua, &registry, &mut target, &v, "Transform").unwrap();
        assert_eq!(target, t);
        let patch: Value = lua
            .load("return { translation = vector.create(5, 0, 0) }")
            .eval()
            .unwrap();
        apply_lua(&lua, &registry, &mut target, &patch, "Transform").unwrap();
        assert_eq!(target.translation, Vec3::new(5.0, 0.0, 0.0));
        let bad: Value = lua.load("return { transation = 1 }").eval().unwrap();
        let e = apply_lua(&lua, &registry, &mut target, &bad, "Transform").unwrap_err();
        assert!(e.to_string().contains("did you mean `translation`"), "{e}");
    }

    #[test]
    fn enums_options_and_lists_convert() {
        let lua = lua();
        let registry = Registry::with_core_types();
        use box_core::primitive::Primitive;
        let p = Primitive::cuboid(Vec3::ONE);
        let v = reflect_to_lua(&lua, &registry, &p).unwrap();
        let mut target = Primitive::sphere(1.0);
        apply_lua(&lua, &registry, &mut target, &v, "Primitive").unwrap();
        assert_eq!(target, p);
        let sphere: Value = lua
            .load("return { shape = { Sphere = { radius = 2 } } }")
            .eval()
            .unwrap();
        apply_lua(&lua, &registry, &mut target, &sphere, "Primitive").unwrap();
        // Fields left out of a new variant take its `VariantDefaults`.
        assert_eq!(target, Primitive::sphere(2.0));
        let capsule: Value = lua.load("return { shape = 'Capsule' }").eval().unwrap();
        apply_lua(&lua, &registry, &mut target, &capsule, "Primitive").unwrap();
        assert_eq!(target.shape, Primitive::capsule(0.4, 0.4).shape);
        let mut list = vec![1.0f32, 2.0, 3.0];
        let short: Value = lua.load("return { 9 }").eval().unwrap();
        // Lists need registered item types; f32 is.
        let mut reg = Registry::new();
        reg.register_value::<Vec<f32>>();
        apply_lua(&lua, &reg, &mut list, &short, "list").unwrap();
        assert_eq!(list, vec![9.0]);
        let long: Value = lua.load("return { 1, 2, 3, 4 }").eval().unwrap();
        apply_lua(&lua, &reg, &mut list, &long, "list").unwrap();
        assert_eq!(list, vec![1.0, 2.0, 3.0, 4.0]);
        let mut opt: Option<Vec3> = None;
        reg.register_value::<Option<Vec3>>();
        let some: Value = lua.load("return vector.create(1, 2, 3)").eval().unwrap();
        apply_lua(&lua, &reg, &mut opt, &some, "opt").unwrap();
        assert_eq!(opt, Some(Vec3::new(1.0, 2.0, 3.0)));
        apply_lua(&lua, &reg, &mut opt, &Value::Nil, "opt").unwrap();
        assert_eq!(opt, None);
    }

    #[test]
    fn save_tables_round_trip_through_json() {
        let lua = lua();
        let v: Value = lua
            .load("return { score = 3, tags = { 'a', 'b' }, at = vector.create(1, 2, 3), nested = { ok = true, half = 0.5 } }")
            .eval()
            .unwrap();
        let json = lua_to_json(&v, &|_| None).unwrap();
        assert_eq!(json["score"], 3);
        assert_eq!(json["at"]["$vector"][1], 2.0);
        let back = json_to_lua(&lua, &json).unwrap();
        let again = lua_to_json(&back, &|_| None).unwrap();
        assert_eq!(json, again);
        let f: Value = lua.load("return { f = function() end }").eval().unwrap();
        assert!(lua_to_json(&f, &|_| None).is_err());
    }
}
