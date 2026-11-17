//! Entities (methods on entity handles) and `world`.

use bevy_reflect::GetPath;
use box_core::identity::{Name, Uid};
use box_core::reflect::{Registry, did_you_mean};
use box_core::transform::{Transform, compute_global};
use box_core::visibility::{self, Transient, Visible};
use box_core::world::{Entity, World};
use glam::Vec3;
use mlua::{Function, LightUserData, Lua, MultiValue, Result as LuaResult, Table, Value, Vector};

use super::{ScriptApi, host, names_arg, with_world, with_world_mut};
use crate::component::{Script, ScriptValue};
use crate::convert::{
    LuaQuat, apply_lua, check_entity, entity_value, from_vector, reflect_to_lua, to_asset,
    to_color, to_entity, vector,
};

pub(crate) struct EntityApi;
pub(crate) struct WorldApi;

fn rt(msg: impl Into<String>) -> mlua::Error {
    mlua::Error::runtime(msg.into())
}

/// The registered component `name`, or an error with a did-you-mean.
pub(crate) fn component_name(registry: &Registry, name: &str) -> LuaResult<&'static str> {
    match registry.component(name) {
        Some(c) => Ok(c.name()),
        None => {
            let mut msg = format!("unknown component `{name}`");
            if registry.resource(name).is_some() {
                msg.push_str(" (it is a resource: use world.resource)");
            } else if let Some(g) = did_you_mean(name, registry.components().map(|c| c.name())) {
                msg.push_str(&format!(" (did you mean `{g}`?)"));
            }
            Err(rt(msg))
        }
    }
}

fn live(w: &World, e: Entity) -> LuaResult<()> {
    if w.contains(e) {
        Ok(())
    } else {
        Err(rt("the entity no longer exists"))
    }
}

/// A copy of component `path` (`"Transform"`, `"Transform.translation"`).
fn get_path(lua: &Lua, w: &World, e: Entity, path: &str) -> LuaResult<Value> {
    live(w, e)?;
    let registry = w.registry().clone();
    if !path.contains(['.', '[']) {
        // A whole component: no path to parse.
        let name = component_name(&registry, path)?;
        return match w.get_reflect(e, name) {
            Ok(root) => reflect_to_lua(lua, &registry, root.as_partial_reflect()),
            Err(box_core::reflect::ReflectError::Missing(_)) => Ok(Value::Nil),
            Err(err) => Err(rt(err.to_string())),
        };
    }
    let fp = w.field_path(path).map_err(|err| {
        let name = path.split(['.', '[']).next().unwrap_or(path);
        match component_name(&registry, name) {
            Err(e) => e,
            Ok(_) => rt(err.to_string()),
        }
    })?;
    if fp.is_resource() {
        return Err(rt(format!(
            "`{}` is a resource: use world.resource",
            fp.target()
        )));
    }
    let root = match w.get_reflect(e, fp.target()) {
        Ok(r) => r,
        Err(box_core::reflect::ReflectError::Missing(_)) => return Ok(Value::Nil),
        Err(err) => return Err(rt(err.to_string())),
    };
    if fp.is_whole() {
        return reflect_to_lua(lua, &registry, root.as_partial_reflect());
    }
    let field = (**root).reflect_path(fp.field()).map_err(|err| {
        rt(registry
            .explain_missing_field(&fp)
            .unwrap_or_else(|| format!("{path}: {err}")))
    })?;
    reflect_to_lua(lua, &registry, field)
}

/// Patches component `path` with `value`.
fn set_path(lua: &Lua, w: &mut World, e: Entity, path: &str, value: &Value) -> LuaResult<()> {
    live(w, e)?;
    let registry = w.registry().clone();
    if !path.contains(['.', '[']) && path != "Script" {
        // A whole component: no path to parse.
        let name = component_name(&registry, path)?;
        let root = w
            .get_reflect_mut(e, name)
            .map_err(|err| rt(err.to_string()))?;
        return apply_lua(lua, &registry, root.as_partial_reflect_mut(), value, path);
    }
    let fp = w.field_path(path).map_err(|err| {
        let name = path.split(['.', '[']).next().unwrap_or(path);
        match component_name(&registry, name) {
            Err(e) => e,
            Ok(_) => rt(err.to_string()),
        }
    })?;
    if fp.target() == "Script" && fp.is_whole() {
        let current = w.get::<Script>(e).map(|s| (*s).clone());
        let script = script_from_lua(value, current)?;
        return w
            .insert_one(e, script)
            .map_err(|_| rt("the entity no longer exists"));
    }
    let root = w
        .get_reflect_mut(e, fp.target())
        .map_err(|err| rt(err.to_string()))?;
    let target = if fp.is_whole() {
        root.as_partial_reflect_mut()
    } else {
        root.reflect_path_mut(fp.field())
            .map_err(|err| rt(format!("{path}: {err}")))?
    };
    apply_lua(lua, &registry, target, value, path)
}

/// A `Script` from a script value: a path string, or
/// `{ source = "scripts/x.luau", props = { speed = 2 } }`.
pub(crate) fn script_from_lua(v: &Value, current: Option<Script>) -> LuaResult<Script> {
    let mut script = current.unwrap_or_default();
    match v {
        Value::String(_) => script.source = to_asset(v, "Script")?,
        Value::Table(t) => {
            let source: Value = t.get("source")?;
            if !source.is_nil() {
                script.source = to_asset(&source, "Script.source")?;
            }
            if let Value::Table(props) = t.get::<Value>("props")? {
                for pair in props.pairs::<String, Value>() {
                    let (k, val) = pair?;
                    script.set(&k, script_value_from_lua(&val, &k)?);
                }
            }
        }
        other => {
            return Err(rt(format!(
                "Script: expected a script path or {{ source = ..., props = {{...}} }}, got {}",
                other.type_name()
            )));
        }
    }
    Ok(script)
}

/// A property value from a Luau value (its type follows the value).
pub(crate) fn script_value_from_lua(v: &Value, name: &str) -> LuaResult<ScriptValue> {
    Ok(match v {
        Value::Boolean(b) => ScriptValue::Boolean(*b),
        Value::Integer(i) => ScriptValue::Integer(*i),
        Value::Number(n) => ScriptValue::Number(*n),
        Value::String(s) => ScriptValue::String(s.to_str()?.to_string()),
        Value::Vector(v) => ScriptValue::Vector(from_vector(*v)),
        Value::Table(_) => ScriptValue::Color(to_color(v, box_core::color::Color::WHITE, name)?),
        other => {
            return Err(rt(format!(
                "property `{name}`: a {} cannot be a property value",
                other.type_name()
            )));
        }
    })
}

/// Inserts component `name` (default value, patched with `value`), or
/// patches it when present.
fn add_component(lua: &Lua, w: &mut World, e: Entity, name: &str, value: &Value) -> LuaResult<()> {
    live(w, e)?;
    let registry = w.registry().clone();
    let name = component_name(&registry, name)?;
    if name == "Script" {
        let current = w.get::<Script>(e).map(|s| (*s).clone());
        let script = script_from_lua(value, current)?;
        return w
            .insert_one(e, script)
            .map_err(|_| rt("the entity no longer exists"));
    }
    let ty = registry.component(name).ok_or_else(|| rt(name))?;
    if !ty.contained_in(w, e) {
        let mut v = ty.default_value().ok_or_else(|| {
            rt(format!(
                "`{name}` has no default value, so it cannot be added"
            ))
        })?;
        if !value.is_nil() {
            apply_lua(lua, &registry, v.as_mut(), value, name)?;
        }
        return w
            .insert_reflect(e, name, v.as_ref())
            .map_err(|err| rt(err.to_string()));
    }
    if value.is_nil() {
        return Ok(());
    }
    set_path(lua, w, e, name, value)
}

fn transform_of(w: &World, e: Entity) -> LuaResult<Transform> {
    w.get::<Transform>(e)
        .map(|t| *t)
        .ok_or_else(|| rt("the entity has no Transform"))
}

fn edit_transform(lua: &Lua, e: Entity, f: impl FnOnce(&mut Transform)) -> LuaResult<()> {
    with_world_mut(lua, |w| match w.component_mut::<Transform>(e) {
        Some(t) => {
            f(t);
            Ok(())
        }
        None => Err(rt("the entity has no Transform (or no longer exists)")),
    })?
}

/// Entities with every component in `names`.
pub(crate) fn query(w: &World, names: &[String]) -> LuaResult<Vec<Entity>> {
    let registry = w.registry().clone();
    let mut types = Vec::with_capacity(names.len());
    for n in names {
        let name = component_name(&registry, n)?;
        types.push(registry.component(name).ok_or_else(|| rt(name))?);
    }
    let Some((first, rest)) = types.split_first() else {
        return Err(rt("world.query: name at least one component"));
    };
    let mut out = first.entities(w);
    if !rest.is_empty() {
        out.retain(|e| rest.iter().all(|t| t.contained_in(w, *e)));
    }
    Ok(out)
}

fn entity_list(lua: &Lua, list: &[Entity]) -> LuaResult<Table> {
    let t = lua.create_table_with_capacity(list.len(), 0)?;
    for (i, e) in list.iter().enumerate() {
        t.raw_set(i + 1, entity_value(*e))?;
    }
    Ok(t)
}

macro_rules! method {
    ($lua:ident, $t:ident, $name:literal, $f:expr) => {
        $t.set($name, $lua.create_function($f)?)?;
    };
}

impl ScriptApi for EntityApi {
    fn name(&self) -> &str {
        "Entity"
    }

    fn install(&self, lua: &Lua, _g: &Table) -> LuaResult<()> {
        let m = lua.create_table()?;
        method!(lua, m, "is_alive", |lua, e: Value| {
            let e = check_entity(&e, "is_alive")?;
            with_world(lua, |w| w.contains(e))
        });
        method!(lua, m, "id", |_, e: Value| {
            let e = check_entity(&e, "id")?;
            Ok(format!("{}", e.id()))
        });
        method!(lua, m, "uid", |lua, e: Value| {
            let e = check_entity(&e, "uid")?;
            with_world(lua, |w| w.uid(e).map(|u| u.to_string()))
        });
        method!(lua, m, "name", |lua, e: Value| {
            let e = check_entity(&e, "name")?;
            with_world(lua, |w| w.get::<Name>(e).map(|n| n.0.clone()))
        });
        method!(lua, m, "set_name", |lua, (e, name): (Value, String)| {
            let e = check_entity(&e, "set_name")?;
            with_world_mut(lua, |w| w.insert_one(e, Name::new(name)).is_ok())
        });
        method!(lua, m, "has", |lua, (e, name): (Value, String)| {
            let e = check_entity(&e, "has")?;
            with_world(lua, |w| {
                let registry = w.registry().clone();
                let name = component_name(&registry, &name)?;
                Ok(registry
                    .component(name)
                    .is_some_and(|c| c.contained_in(w, e)))
            })?
        });
        method!(lua, m, "get", |lua, (e, path): (Value, String)| {
            let e = check_entity(&e, "get")?;
            with_world(lua, |w| get_path(lua, w, e, &path))?
        });
        method!(
            lua,
            m,
            "set",
            |lua, (e, path, value): (Value, String, Value)| {
                let e = check_entity(&e, "set")?;
                with_world_mut(lua, |w| set_path(lua, w, e, &path, &value))?
            }
        );
        method!(
            lua,
            m,
            "add",
            |lua, (e, name, value): (Value, String, Value)| {
                let e = check_entity(&e, "add")?;
                with_world_mut(lua, |w| add_component(lua, w, e, &name, &value))?
            }
        );
        method!(lua, m, "remove", |lua, (e, name): (Value, String)| {
            let e = check_entity(&e, "remove")?;
            with_world_mut(lua, |w| {
                let registry = w.registry().clone();
                let name = component_name(&registry, &name)?;
                Ok(w.remove_reflect(e, name).is_ok())
            })?
        });
        method!(lua, m, "despawn", |lua, e: Value| {
            let e = check_entity(&e, "despawn")?;
            with_world_mut(lua, |w| w.despawn(e))
        });
        method!(lua, m, "set_visible", |lua, (e, visible): (Value, bool)| {
            let e = check_entity(&e, "set_visible")?;
            with_world_mut(lua, |w| {
                live(w, e)?;
                // Showing an entity that never was hidden adds nothing.
                if visible && !w.has::<Visible>(e) {
                    return Ok(());
                }
                let _ = w.insert_one(e, Visible(visible));
                Ok(())
            })?
        });
        method!(lua, m, "is_visible", |lua, e: Value| {
            let e = check_entity(&e, "is_visible")?;
            with_world(lua, |w| {
                live(w, e)?;
                Ok(!visibility::is_hidden(w, e))
            })?
        });
        method!(lua, m, "parent", |lua, e: Value| {
            let e = check_entity(&e, "parent")?;
            with_world(lua, |w| w.parent(e).map(entity_value))
        });
        method!(lua, m, "children", |lua, e: Value| {
            let e = check_entity(&e, "children")?;
            let kids = with_world(lua, |w| w.children(e))?;
            entity_list(lua, &kids)
        });
        method!(lua, m, "set_parent", |lua, (e, parent): (Value, Value)| {
            let e = check_entity(&e, "set_parent")?;
            let parent = if parent.is_nil() {
                None
            } else {
                Some(check_entity(&parent, "set_parent")?)
            };
            with_world_mut(lua, |w| {
                w.set_parent(e, parent).map_err(|err| rt(err.to_string()))
            })?
        });
        method!(lua, m, "script", |lua, e: Value| {
            let e = check_entity(&e, "script")?;
            let host = host(lua)?;
            Ok(host.instances.borrow().get(&e).cloned())
        });
        // Transform fast paths (local transform unless named world_).
        method!(lua, m, "position", |lua, e: Value| {
            let e = check_entity(&e, "position")?;
            with_world(lua, |w| transform_of(w, e).map(|t| vector(t.translation)))?
        });
        method!(lua, m, "set_position", |lua, (e, p): (Value, Vector)| {
            let e = check_entity(&e, "set_position")?;
            edit_transform(lua, e, |t| t.translation = from_vector(p))
        });
        method!(lua, m, "translate", |lua, (e, d): (Value, Vector)| {
            let e = check_entity(&e, "translate")?;
            edit_transform(lua, e, |t| t.translation += from_vector(d))
        });
        method!(lua, m, "rotation", |lua, e: Value| {
            let e = check_entity(&e, "rotation")?;
            with_world(lua, |w| transform_of(w, e).map(|t| LuaQuat(t.rotation)))?
        });
        method!(lua, m, "set_rotation", |lua, (e, q): (Value, LuaQuat)| {
            let e = check_entity(&e, "set_rotation")?;
            edit_transform(lua, e, |t| t.rotation = q.0)
        });
        method!(lua, m, "rotate", |lua, (e, q): (Value, LuaQuat)| {
            let e = check_entity(&e, "rotate")?;
            edit_transform(lua, e, |t| t.rotation = (q.0 * t.rotation).normalize())
        });
        method!(lua, m, "rotate_y", |lua, (e, angle): (Value, f32)| {
            let e = check_entity(&e, "rotate_y")?;
            edit_transform(lua, e, |t| t.rotate_y(angle))
        });
        method!(lua, m, "scale", |lua, e: Value| {
            let e = check_entity(&e, "scale")?;
            with_world(lua, |w| transform_of(w, e).map(|t| vector(t.scale)))?
        });
        method!(lua, m, "set_scale", |lua, (e, s): (Value, Value)| {
            let e = check_entity(&e, "set_scale")?;
            let s = match s {
                Value::Number(n) => Vec3::splat(n as f32),
                Value::Integer(n) => Vec3::splat(n as f32),
                Value::Vector(v) => from_vector(v),
                other => {
                    return Err(rt(format!(
                        "set_scale: expected a number or vector, got {}",
                        other.type_name()
                    )));
                }
            };
            edit_transform(lua, e, |t| t.scale = s)
        });
        method!(lua, m, "forward", |lua, e: Value| {
            let e = check_entity(&e, "forward")?;
            with_world(lua, |w| transform_of(w, e).map(|t| vector(t.forward())))?
        });
        method!(lua, m, "right", |lua, e: Value| {
            let e = check_entity(&e, "right")?;
            with_world(lua, |w| transform_of(w, e).map(|t| vector(t.right())))?
        });
        method!(lua, m, "up", |lua, e: Value| {
            let e = check_entity(&e, "up")?;
            with_world(lua, |w| transform_of(w, e).map(|t| vector(t.up())))?
        });
        method!(lua, m, "look_at", |lua,
                                    (e, target, up): (
            Value,
            Vector,
            Option<Vector>
        )| {
            let e = check_entity(&e, "look_at")?;
            // `target` and `up` are in the world: bring them into the
            // parent's space, where the entity's Transform lives.
            let (target, up) = with_world(lua, |w| {
                let target = from_vector(target);
                let up = up.map_or(Vec3::Y, from_vector);
                match w.parent(e) {
                    Some(parent) => {
                        let to_parent = compute_global(w, parent).inverse();
                        (
                            to_parent.transform_point3(target),
                            to_parent.transform_vector3(up),
                        )
                    }
                    None => (target, up),
                }
            })?;
            edit_transform(lua, e, |t| {
                let dir = target - t.translation;
                if dir.length_squared() > 1e-8 {
                    t.look_to(dir, up);
                }
            })
        });
        method!(lua, m, "world_position", |lua, e: Value| {
            let e = check_entity(&e, "world_position")?;
            with_world(lua, |w| {
                live(w, e)?;
                Ok(vector(compute_global(w, e).translation.into()))
            })?
        });
        method!(lua, m, "world_rotation", |lua, e: Value| {
            let e = check_entity(&e, "world_rotation")?;
            with_world(lua, |w| {
                live(w, e)?;
                let (_, r, _) = compute_global(w, e).to_scale_rotation_translation();
                Ok(LuaQuat(r))
            })?
        });
        method!(lua, m, "interpolated_position", |lua, e: Value| {
            let e = check_entity(&e, "interpolated_position")?;
            with_world(lua, |w| {
                box_core::interpolate::interpolated_transform(w, e)
                    .map(|t| vector(t.translation))
                    .ok_or_else(|| rt("the entity has no Transform (or no longer exists)"))
            })?
        });
        let mt = lua.create_table()?;
        mt.set("__index", m)?;
        mt.set(
            "__tostring",
            lua.create_function(|lua, e: LightUserData| {
                let e = to_entity(&Value::LightUserData(e));
                let name = e.and_then(|e| {
                    with_world(lua, |w| w.get::<Name>(e).map(|n| n.0.clone()))
                        .ok()
                        .flatten()
                });
                Ok(match (e, name) {
                    (Some(e), Some(n)) => format!("Entity({} \"{n}\")", e.id()),
                    (Some(e), None) => format!("Entity({})", e.id()),
                    (None, _) => "Entity(invalid)".to_owned(),
                })
            })?,
        )?;
        lua.set_type_metatable::<LightUserData>(Some(mt));
        Ok(())
    }

    fn definitions(&self) -> String {
        // The typed `get`/`set` overloads per component are generated from
        // the registry (`crate::types`); these are the fixed methods.
        r#"--- A handle to an entity (an object in the scene). Handles compare by value (`==`) and work as table keys; a handle to a despawned entity stays a valid value but its methods fail ("the entity no longer exists"), so check `is_alive` when you keep handles. Position methods work on the local `Transform` (relative to the parent) unless named `world_`.
declare extern type Entity with
    --- True while the entity exists.
    function is_alive(self): boolean
    --- A runtime id for logs and debugging: unique while the game runs, not saved (use `uid` to refer to an entity across saves).
    function id(self): string
    --- The entity's persistent uid (saved in scenes and save games), or nil if it has none.
    function uid(self): string?
    --- The entity's `Name`, or nil.
    function name(self): string?
    --- Sets the entity's `Name`. Returns false if the entity no longer exists.
    function set_name(self, name: string): boolean
    --- True if the entity has the registered component `component` (fails with a did-you-mean for unknown names).
    function has(self, component: string): boolean
    --- Adds component `component` with its default values patched with `value` (a table of fields), or patches it when the entity already has it. `Script` takes a path or `{ source = path, props = { ... } }`.
    function add(self, component: string, value: any?): ()
    --- Removes component `component`. Returns true if the entity had it.
    function remove(self, component: string): boolean
    --- Removes the entity and its descendants. Returns false if it was already gone.
    function despawn(self): boolean
    --- Shows or hides this entity and its descendants (`Visible`, saved): hidden meshes and lights are not drawn and hidden particle emitters stop; physics and scripts keep running.
    function set_visible(self, visible: boolean): ()
    --- False when this entity or an ancestor is hidden (`Visible(false)`).
    function is_visible(self): boolean
    --- The parent entity, or nil for a root.
    function parent(self): Entity?
    --- The child entities, in hierarchy order.
    function children(self): { Entity }
    --- Re-parents the entity (nil makes it a root). Its local transform is kept, so it moves with its new parent.
    function set_parent(self, parent: Entity?): ()
    --- The `self` table of the script running on this entity (its fields and module functions), or nil when it has none.
    function script(self): any
    --- The local position (`Transform.translation`).
    function position(self): vector
    --- Sets the local position.
    function set_position(self, position: vector): ()
    --- Moves the entity by `offset` (local axes of the parent, metres).
    function translate(self, offset: vector): ()
    --- The local rotation.
    function rotation(self): Quat
    --- Sets the local rotation.
    function set_rotation(self, rotation: Quat): ()
    --- Applies `rotation` on top of the current one (`rotation * current`, about the parent's axes).
    function rotate(self, rotation: Quat): ()
    --- Turns the entity around the parent's Y axis by `radians` (positive turns left).
    function rotate_y(self, radians: number): ()
    --- The local scale.
    function scale(self): vector
    --- Sets the local scale: a vector, or a number for a uniform scale.
    function set_scale(self, scale: vector | number): ()
    --- The local forward direction (-Z rotated by the local rotation).
    function forward(self): vector
    --- The local right direction (+X rotated).
    function right(self): vector
    --- The local up direction (+Y rotated).
    function up(self): vector
    --- Turns the entity so its forward (-Z) points at `target`, a point in the world (also under a moved or turned parent), keeping `up` (default +Y, in the world) up.
    function look_at(self, target: vector, up: vector?): ()
    --- The position in the world (through every parent), computed now.
    function world_position(self): vector
    --- The rotation in the world (through every parent), computed now.
    function world_rotation(self): Quat
    --- Where an `Interpolated` entity is drawn this frame: between its last two fixed-step positions. Use it to follow an entity moved in `on_fixed_update` smoothly (a camera).
    function interpolated_position(self): vector
@COMPONENT_METHODS@end
"#
        .into()
    }

    fn doc(&self) -> &str {
        r#"Methods on entity handles (`e:position()`), which `world.spawn`, `world.find`, `world.query`, `self.entity` and callbacks such as `on_trigger_enter` return.

Transform shortcuts (`position`, `set_position`, `rotate_y`, ...) work on the local `Transform`, relative to the parent; `world_position` and `world_rotation` go through every parent, and `look_at` takes a point in the world. Any registered component is reachable with `get`, `set`, `add`, `remove` and `has` by name (see [Components](components.md)); the definitions file adds a typed `get`/`set` overload per component, so luau-lsp knows `e:get("Transform")` returns a `Transform`.

```lua
function Lamp.on_start(self)
    local e = self.entity
    e:set_position(e:position() + vector.create(0, 1, 0))
    local t = e:get("Transform")               -- a copy
    e:set("Transform", { scale = t.scale * 2 })
    e:set("PointLight.intensity", 40)          -- one field
    if not e:has("Collider") then
        e:add("Collider", { is_trigger = true })
    end
end
```"#
    }
}

impl ScriptApi for WorldApi {
    fn name(&self) -> &str {
        "world"
    }

    fn install(&self, lua: &Lua, g: &Table) -> LuaResult<()> {
        let t = lua.create_table()?;
        method!(lua, t, "spawn", |lua, spec: Option<Table>| spawn(lua, spec));
        method!(lua, t, "find", |lua, name: String| {
            with_world(lua, |w| w.find_by_name(&name).map(entity_value))
        });
        method!(lua, t, "find_all", |lua, name: String| {
            let found: Vec<Entity> = with_world(lua, |w| {
                w.query::<(Entity, &Name)>()
                    .iter()
                    .filter(|(_, n)| n.as_str() == name)
                    .map(|(e, _)| e)
                    .collect()
            })?;
            entity_list(lua, &found)
        });
        method!(lua, t, "entity", |lua, uid: String| {
            with_world(lua, |w| {
                Uid::parse(&uid)
                    .and_then(|u| w.entity_by_uid(u))
                    .map(entity_value)
            })
        });
        method!(lua, t, "query", |lua, names: MultiValue| {
            let names = names_arg(names)?;
            let found = with_world(lua, |w| query(w, &names))??;
            entity_list(lua, &found)
        });
        method!(lua, t, "count", |lua, names: MultiValue| {
            let names = names_arg(names)?;
            Ok(with_world(lua, |w| query(w, &names))??.len())
        });
        method!(lua, t, "each", |lua, (names, f): (Value, Function)| {
            let names = names_arg(MultiValue::from_iter([names]))?;
            let found = with_world(lua, |w| query(w, &names))??;
            for e in found {
                let mut args = MultiValue::with_capacity(names.len() + 1);
                let row = with_world(lua, |w| -> LuaResult<Option<Vec<Value>>> {
                    if !w.contains(e) {
                        return Ok(None);
                    }
                    let mut vals = Vec::with_capacity(names.len());
                    for n in &names {
                        vals.push(get_path(lua, w, e, n)?);
                    }
                    Ok(Some(vals))
                })??;
                let Some(vals) = row else { continue };
                args.push_back(entity_value(e));
                args.extend(vals);
                f.call::<()>(args)?;
            }
            Ok(())
        });
        method!(lua, t, "resource", |lua, name: String| {
            with_world(lua, |w| {
                let registry = w.registry().clone();
                let fp = w.field_path(&name).map_err(|e| rt(e.to_string()))?;
                if !fp.is_resource() {
                    return Err(rt(format!(
                        "`{}` is a component: use entity:get",
                        fp.target()
                    )));
                }
                match w.get_resource_field(&fp) {
                    Ok(v) => reflect_to_lua(lua, &registry, v.as_ref()),
                    Err(box_core::reflect::ReflectError::MissingResource(_)) => Ok(Value::Nil),
                    Err(e) => Err(rt(e.to_string())),
                }
            })?
        });
        method!(
            lua,
            t,
            "set_resource",
            |lua, (name, value): (String, Value)| {
                with_world_mut(lua, |w| {
                    let registry = w.registry().clone();
                    let fp = w.field_path(&name).map_err(|e| rt(e.to_string()))?;
                    if !fp.is_resource() {
                        return Err(rt(format!(
                            "`{}` is a component: use entity:set",
                            fp.target()
                        )));
                    }
                    let mut current = w.get_resource_field(&fp).map_err(|e| rt(e.to_string()))?;
                    apply_lua(lua, &registry, current.as_mut(), &value, &name)?;
                    w.set_resource_field(&fp, current.as_ref())
                        .map_err(|e| rt(e.to_string()))
                })?
            }
        );
        method!(lua, t, "despawn", |lua, e: Value| {
            let e = check_entity(&e, "world.despawn")?;
            with_world_mut(lua, |w| w.despawn(e))
        });
        method!(lua, t, "scripts", |lua, path: String| {
            let host = host(lua)?;
            let list: Vec<Table> = {
                let order = host.script_list.borrow();
                let instances = host.instances.borrow();
                order
                    .iter()
                    .filter(|(_, p)| **p == *path)
                    .filter_map(|(e, _)| instances.get(e).cloned())
                    .collect()
            };
            let out = lua.create_table_with_capacity(list.len(), 0)?;
            for (i, s) in list.into_iter().enumerate() {
                out.raw_set(i + 1, s)?;
            }
            Ok(out)
        });
        g.set("world", t)
    }

    fn definitions(&self) -> String {
        r#"--- The game world: spawn and find entities, query them by component, read and change resources.
declare world: {
    --- Spawns an entity with a fresh uid and returns it. `spec` keys: `name` (a string), `parent` (an entity), `transient` (true: runtime-only, never saved, removed when a save loads) and any registered component name with a table of fields (defaults patched with them), such as `{ name = "Coin", Transform = { translation = p }, Primitive = { shape = { Sphere = {} } } }`. A default `Transform` is added unless one is given. Runtime entities are removed by a scene switch unless kept (`scenes.persist`).
    spawn: (spec: { [string]: any }?) -> Entity,
    --- The first entity whose `Name` is `name`, or nil. Names need not be unique.
    find: (name: string) -> Entity?,
    --- Every entity named `name`.
    find_all: (name: string) -> { Entity },
    --- The entity with this uid (as `e:uid()` returns it), or nil.
    entity: (uid: string) -> Entity?,
    --- Every entity that has all of the named components (given as separate arguments or one array), for example `world.query("Transform", "Collider")`. Loop the result and use entity methods: the batch form for many entities.
    query: (...(string | { string })) -> { Entity },
    --- How many entities have all of the named components (separate arguments or one array).
    count: (...(string | { string })) -> number,
    --- Calls `f(entity, a, b, ...)` for every entity with the named components, passing copies of their values: convenient for reading many entities (about 0.6 µs per row).
    each: (components: string | { string }, f: (Entity, ...any) -> ()) -> (),
    --- A copy of a registered resource (`world.resource("Environment")`) or one of its fields (`"Environment.exposure"`), or nil when the world has none.
    resource: @RESOURCE_GET@,
    --- Patches a resource, or one field (`world.set_resource("Environment.exposure", 1.5)`), with the given value.
    set_resource: (name: string, value: any) -> (),
    --- Removes an entity and its descendants (like `e:despawn()`). Returns false if it was already gone.
    despawn: (entity: Entity) -> boolean,
    --- The `self` tables of every running instance of the script at `path` (assets-relative, such as "scripts/enemy.luau"), in start order: script data in bulk without copying components.
    scripts: (path: string) -> { any },
}
"#
        .into()
    }

    fn doc(&self) -> &str {
        r#"Entities and resources. Every registered component, resource and event type is reachable by name without bindings: see [Components](components.md), [Resources](resources.md) and [Event types](event-types.md). Values are copies: `e:get("Transform")` returns a fresh table, and changes go back through `e:set` or `world.set_resource`.

For many entities, query once and loop: `world.query` returns entities, `world.each` passes component copies, and `world.scripts` returns script instances' `self` tables.

```lua
function Level.on_start(self)
    world.spawn({
        name = "Marker",
        Transform = { translation = vector.create(0, 3, 0) },
        Primitive = { shape = { Sphere = { radius = 0.3 } } },
    })
    for _, e in world.query("Collider") do
        log.info(e:name(), e:world_position())
    end
    local env = world.resource("Environment")
    world.set_resource("Environment", { exposure = env.exposure * 1.2 })
end
```"#
    }
}

/// `world.spawn{ name = ..., parent = ..., transient = true?, Transform = {...}, ... }`:
/// a new entity with a fresh uid, a default `Transform` unless one is given,
/// and the named components (defaults patched with the given tables).
/// `transient = true` makes it runtime-only ([`Transient`]: never saved,
/// despawned when a save game loads).
fn spawn(lua: &Lua, spec: Option<Table>) -> LuaResult<Value> {
    let mut parent = None;
    let mut name = None;
    let mut transient = false;
    let mut parts: Vec<(String, Value)> = Vec::new();
    if let Some(spec) = &spec {
        for pair in spec.pairs::<String, Value>() {
            let (k, v) = pair?;
            match k.as_str() {
                "name" => {
                    name = Some(match &v {
                        Value::String(s) => s.to_str()?.to_string(),
                        other => {
                            return Err(rt(format!(
                                "world.spawn: name must be a string, got {}",
                                other.type_name()
                            )));
                        }
                    })
                }
                "parent" => parent = Some(check_entity(&v, "world.spawn parent")?),
                "transient" => {
                    transient = match v {
                        Value::Boolean(b) => b,
                        Value::Nil => false,
                        other => {
                            return Err(rt(format!(
                                "world.spawn: transient must be a boolean, got {}",
                                other.type_name()
                            )));
                        }
                    }
                }
                _ => parts.push((k, v)),
            }
        }
    }
    // Deterministic order whatever the table's iteration order.
    parts.sort_by(|a, b| a.0.cmp(&b.0));
    with_world_mut(lua, |w| {
        let registry = w.registry().clone();
        for (k, _) in &parts {
            component_name(&registry, k)?;
        }
        let e = w.spawn((Uid::new(),));
        if let Some(n) = name {
            let _ = w.insert_one(e, Name::new(n));
        }
        if !parts.iter().any(|(k, _)| k == "Transform") {
            let _ = w.insert_one(e, Transform::default());
        }
        if transient {
            let _ = w.insert_one(e, Transient);
        }
        for (k, v) in &parts {
            if let Err(err) = add_component(lua, w, e, k, v) {
                w.despawn(e);
                return Err(err);
            }
        }
        if let Some(p) = parent
            && let Err(err) = w.set_parent(e, Some(p))
        {
            w.despawn(e);
            return Err(rt(format!("world.spawn: {err}")));
        }
        Ok(entity_value(e))
    })?
}
