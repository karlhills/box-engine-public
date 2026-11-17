//! Spike S4: Luau binding shape for box-engine (throwaway).
//!
//! `cargo run --release` prints every measurement; FINDINGS.md has the
//! conclusions. Questions:
//! 1. How does script code reach the World (scoped functions per frame, or
//!    the world moved into a shared cell for the script stage)?
//! 2. Per-entity callbacks: Rust calls each instance, or one call into a
//!    Lua dispatcher that loops?
//! 3. Entity handles: full userdata or light userdata with a type metatable?
//! 4. Component access: generic reflection (JSON round trip or a direct
//!    walk) vs a typed fast path; copy vs live proxy.
//! 5. Batch queries over 5,000 entities: a callback per row or arrays.
//! 6. Value types: userdata vs table (Quat), native `vector` for Vec3.
//! 7. Coroutine waits, error locations, sandboxing, saved state.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use box_core::prelude::*;
use box_core::reflect::{PartialReflect, ReflectRef};
use mlua::prelude::*;
use mlua::{LightUserData, Value, Vector};

type Shared = Rc<RefCell<World>>;

fn time<R>(label: &str, iters: u32, mut f: impl FnMut() -> R) -> f64 {
    // Warm up.
    for _ in 0..iters.div_ceil(10).max(1) {
        std::hint::black_box(f());
    }
    let start = Instant::now();
    for _ in 0..iters {
        std::hint::black_box(f());
    }
    let per = start.elapsed().as_secs_f64() / f64::from(iters);
    println!("{label:<64} {:>10.3} us", per * 1e6);
    per
}

fn new_lua() -> Lua {
    let lua = Lua::new();
    lua.sandbox(true).expect("sandbox");
    lua
}

fn entity_ud(e: Entity) -> LightUserData {
    LightUserData(std::ptr::without_provenance_mut(e.to_bits().get() as usize))
}

fn ud_entity(l: LightUserData) -> Option<Entity> {
    Entity::from_bits(l.0 as usize as u64)
}

fn v3(v: Vec3) -> Vector {
    Vector::new(v.x, v.y, v.z)
}

fn from_v(v: Vector) -> Vec3 {
    Vec3::new(v.x(), v.y(), v.z())
}

#[derive(Clone, Copy)]
struct LQuat(Quat);

impl LuaUserData for LQuat {
    fn add_methods<M: LuaUserDataMethods<Self>>(m: &mut M) {
        m.add_meta_function("__mul", |lua, (a, b): (LQuat, Value)| match b {
            Value::Vector(v) => Ok(Value::Vector(v3(a.0 * from_v(v)))),
            other => {
                let b = LQuat::from_lua(other, lua)?;
                lua.create_userdata(LQuat(a.0 * b.0)).map(Value::UserData)
            }
        });
    }
    fn add_fields<F: LuaUserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("x", |_, q| Ok(q.0.x));
    }
}

impl FromLua for LQuat {
    fn from_lua(v: Value, _: &Lua) -> LuaResult<Self> {
        match v {
            Value::UserData(ud) => Ok(*ud.borrow::<LQuat>()?),
            _ => Err(LuaError::runtime("expected Quat")),
        }
    }
}

// ----- generic reflection <-> Lua -----

fn reflect_to_lua(lua: &Lua, world: &World, v: &dyn PartialReflect) -> LuaResult<Value> {
    if let Some(x) = v.try_downcast_ref::<Vec3>() {
        return Ok(Value::Vector(v3(*x)));
    }
    if let Some(q) = v.try_downcast_ref::<Quat>() {
        return lua.create_userdata(LQuat(*q)).map(Value::UserData);
    }
    if let Some(x) = v.try_downcast_ref::<f32>() {
        return Ok(Value::Number(f64::from(*x)));
    }
    if let Some(x) = v.try_downcast_ref::<bool>() {
        return Ok(Value::Boolean(*x));
    }
    match v.reflect_ref() {
        ReflectRef::Struct(s) => {
            let t = lua.create_table_with_capacity(0, s.field_len())?;
            for i in 0..s.field_len() {
                if let (Some(n), Some(f)) = (s.name_at(i), s.field_at(i)) {
                    t.raw_set(n, reflect_to_lua(lua, world, f)?)?;
                }
            }
            Ok(Value::Table(t))
        }
        _ => {
            let json = world.registry().value_to_json(v).map_err(LuaError::external)?;
            lua.to_value(&json)
        }
    }
}

/// Patch `target` with the fields of a Lua value (type-directed).
fn apply_lua(target: &mut dyn PartialReflect, v: &Value) -> LuaResult<()> {
    if let Some(x) = target.try_downcast_mut::<Vec3>() {
        if let Value::Vector(v) = v {
            *x = from_v(*v);
            return Ok(());
        }
        return Err(LuaError::runtime("expected vector"));
    }
    if let Some(x) = target.try_downcast_mut::<Quat>() {
        if let Value::UserData(ud) = v {
            *x = ud.borrow::<LQuat>()?.0;
            return Ok(());
        }
        return Err(LuaError::runtime("expected Quat"));
    }
    if let Some(x) = target.try_downcast_mut::<f32>() {
        *x = v.as_f64().ok_or_else(|| LuaError::runtime("expected number"))? as f32;
        return Ok(());
    }
    match (target.reflect_mut(), v) {
        (box_core::reflect::ReflectMut::Struct(s), Value::Table(t)) => {
            for pair in t.pairs::<LuaString, Value>() {
                let (k, val) = pair?;
                let k = k.to_str()?;
                let f = s
                    .field_mut(&k)
                    .ok_or_else(|| LuaError::runtime(format!("no field {k}")))?;
                apply_lua(f, &val)?;
            }
            Ok(())
        }
        _ => Err(LuaError::runtime("unsupported")),
    }
}

/// Generic struct conversion with field-name strings interned once per
/// type (what a registry-generated converter table would hold).
struct Keys(Vec<LuaString>);

fn reflect_to_lua_cached(lua: &Lua, keys: &Keys, v: &dyn PartialReflect) -> LuaResult<Value> {
    match v.reflect_ref() {
        ReflectRef::Struct(s) => {
            let t = lua.create_table_with_capacity(0, s.field_len())?;
            for (i, k) in keys.0.iter().enumerate() {
                if let Some(f) = s.field_at(i) {
                    let val = if let Some(x) = f.try_downcast_ref::<Vec3>() {
                        Value::Vector(v3(*x))
                    } else if let Some(q) = f.try_downcast_ref::<Quat>() {
                        Value::UserData(lua.create_userdata(LQuat(*q))?)
                    } else {
                        Value::Nil
                    };
                    t.raw_set(k, val)?;
                }
            }
            Ok(Value::Table(t))
        }
        _ => Ok(Value::Nil),
    }
}

fn spawn_world(n: usize) -> World {
    let mut w = World::new();
    for i in 0..n {
        w.spawn((
            Transform::from_xyz(i as f32, 0.0, 0.0),
            Name::new(format!("e{i}")),
        ));
    }
    w
}

/// The world-access API: `world_get(e)` style functions reading the world
/// from app data (approach B).
fn install_api(lua: &Lua) -> LuaResult<()> {
    let g = lua.globals();
    // Typed fast path: translation only.
    g.set(
        "get_pos",
        lua.create_function(|lua, e: LightUserData| {
            let w = lua.app_data_ref::<Shared>().expect("world");
            let w = w.borrow();
            let e = ud_entity(e).ok_or_else(|| LuaError::runtime("bad entity"))?;
            Ok(w.get::<Transform>(e).map(|t| v3(t.translation)))
        })?,
    )?;
    g.set(
        "set_pos",
        lua.create_function(|lua, (e, p): (LightUserData, Vector)| {
            let w = lua.app_data_ref::<Shared>().expect("world");
            let mut w = w.borrow_mut();
            let e = ud_entity(e).ok_or_else(|| LuaError::runtime("bad entity"))?;
            if let Some(t) = w.component_mut::<Transform>(e) {
                t.translation = from_v(p);
            }
            Ok(())
        })?,
    )?;
    // Generic: whole component by name through a direct reflect walk.
    g.set(
        "get_reflect",
        lua.create_function(|lua, (e, name): (LightUserData, LuaString)| {
            let w = lua.app_data_ref::<Shared>().expect("world");
            let w = w.borrow();
            let e = ud_entity(e).ok_or_else(|| LuaError::runtime("bad entity"))?;
            let c = w
                .get_reflect(e, &name.to_str()?)
                .map_err(LuaError::external)?;
            reflect_to_lua(lua, &w, c.as_partial_reflect())
        })?,
    )?;
    g.set(
        "set_reflect",
        lua.create_function(|lua, (e, name, v): (LightUserData, LuaString, Value)| {
            let w = lua.app_data_ref::<Shared>().expect("world");
            let mut w = w.borrow_mut();
            let e = ud_entity(e).ok_or_else(|| LuaError::runtime("bad entity"))?;
            let c = w
                .get_reflect_mut(e, &name.to_str()?)
                .map_err(LuaError::external)?;
            apply_lua(c.as_partial_reflect_mut(), &v)
        })?,
    )?;
    let keys = Keys(vec![
        lua.create_string("translation")?,
        lua.create_string("rotation")?,
        lua.create_string("scale")?,
    ]);
    g.set(
        "get_cached",
        lua.create_function(move |lua, (e, name): (LightUserData, LuaString)| {
            let w = lua.app_data_ref::<Shared>().expect("world");
            let w = w.borrow();
            let e = ud_entity(e).ok_or_else(|| LuaError::runtime("bad entity"))?;
            let c = w
                .get_reflect(e, &name.to_str()?)
                .map_err(LuaError::external)?;
            reflect_to_lua_cached(lua, &keys, c.as_partial_reflect())
        })?,
    )?;
    g.set(
        "query_entities",
        lua.create_function(|lua, ()| {
            let shared = lua.app_data_ref::<Shared>().expect("world").clone();
            let w = shared.borrow();
            let t = lua.create_table_with_capacity(w.count::<&Transform>(), 0)?;
            for (i, e) in w.query::<Entity>().with::<&Transform>().iter().enumerate() {
                t.raw_set(i + 1, entity_ud(e))?;
            }
            Ok(t)
        })?,
    )?;
    // Generic through JSON (value_to_json -> Lua table).
    g.set(
        "get_json",
        lua.create_function(|lua, (e, name): (LightUserData, LuaString)| {
            let w = lua.app_data_ref::<Shared>().expect("world");
            let w = w.borrow();
            let e = ud_entity(e).ok_or_else(|| LuaError::runtime("bad entity"))?;
            let c = w
                .get_reflect(e, &name.to_str()?)
                .map_err(LuaError::external)?;
            let json = w
                .registry()
                .value_to_json(c.as_partial_reflect())
                .map_err(LuaError::external)?;
            lua.to_value(&json)
        })?,
    )?;
    g.set(
        "set_json",
        lua.create_function(|lua, (e, name, v): (LightUserData, LuaString, Value)| {
            let w = lua.app_data_ref::<Shared>().expect("world");
            let mut w = w.borrow_mut();
            let e = ud_entity(e).ok_or_else(|| LuaError::runtime("bad entity"))?;
            let json: serde_json::Value = lua.from_value(v)?;
            let name = name.to_str()?;
            let path = w.field_path(&name).map_err(LuaError::external)?;
            let ty = w.registry().component(&name).expect("registered");
            let _ = ty;
            let tid = std::any::TypeId::of::<Transform>();
            let value = w
                .registry()
                .value_from_json(tid, &json)
                .map_err(LuaError::external)?;
            w.set_field(e, &path, value.as_ref())
                .map_err(LuaError::external)
        })?,
    )?;
    // Batch, callback per row.
    g.set(
        "each_transform",
        lua.create_function(|lua, f: LuaFunction| {
            let shared = lua.app_data_ref::<Shared>().expect("world").clone();
            let rows: Vec<(Entity, Vec3)> = shared
                .borrow()
                .query::<(Entity, &Transform)>()
                .iter()
                .map(|(e, t)| (e, t.translation))
                .collect();
            for (e, p) in rows {
                let out: Option<Vector> = f.call((entity_ud(e), v3(p)))?;
                if let Some(p) = out
                    && let Some(t) = shared.borrow_mut().component_mut::<Transform>(e)
                {
                    t.translation = from_v(p);
                }
            }
            Ok(())
        })?,
    )?;
    // Batch as arrays: returns (entities, positions); write back with set_all.
    g.set(
        "query_transforms",
        lua.create_function(|lua, ()| {
            let shared = lua.app_data_ref::<Shared>().expect("world").clone();
            let w = shared.borrow();
            let n = w.count::<&Transform>();
            let es = lua.create_table_with_capacity(n, 0)?;
            let ps = lua.create_table_with_capacity(n, 0)?;
            for (i, (e, t)) in w.query::<(Entity, &Transform)>().iter().enumerate() {
                es.raw_set(i + 1, entity_ud(e))?;
                ps.raw_set(i + 1, v3(t.translation))?;
            }
            Ok((es, ps))
        })?,
    )?;
    g.set(
        "write_transforms",
        lua.create_function(|lua, (es, ps): (LuaTable, LuaTable)| {
            let shared = lua.app_data_ref::<Shared>().expect("world").clone();
            let mut w = shared.borrow_mut();
            let n = es.raw_len();
            for i in 1..=n {
                let e: LightUserData = es.raw_get(i)?;
                let p: Vector = ps.raw_get(i)?;
                if let Some(e) = ud_entity(e)
                    && let Some(t) = w.component_mut::<Transform>(e)
                {
                    t.translation = from_v(p);
                }
            }
            Ok(())
        })?,
    )?;
    g.set(
        "entity_full_ud",
        lua.create_function(|lua, e: LightUserData| lua.create_any_userdata(ud_entity(e)))?,
    )?;
    g.set(
        "quat_ud",
        lua.create_function(|lua, ()| lua.create_userdata(LQuat(Quat::IDENTITY)))?,
    )?;
    g.set(
        "quat_table",
        lua.create_function(|lua, ()| {
            let t = lua.create_table_with_capacity(0, 4)?;
            t.raw_set("x", 0.0)?;
            t.raw_set("y", 0.0)?;
            t.raw_set("z", 0.0)?;
            t.raw_set("w", 1.0)?;
            Ok(t)
        })?,
    )?;
    Ok(())
}

fn main() -> LuaResult<()> {
    println!("== S4 Luau spike (mlua 0.12.2, Luau 0.740) ==\n");
    let n = 10_000;

    // ---------- 1. world access ----------
    println!("-- world access --");
    let lua = new_lua();
    install_api(&lua)?;
    let shared: Shared = Rc::new(RefCell::new(spawn_world(n)));
    lua.set_app_data(shared.clone());
    let es: Vec<Entity> = shared
        .borrow()
        .query::<Entity>()
        .with::<&Transform>()
        .iter()
        .collect();
    // Swap cost of moving a world in and out of the shared cell.
    let mut outside = spawn_world(5000);
    time("move world into the cell and back (B)", 10_000, || {
        std::mem::swap(&mut *shared.borrow_mut(), &mut outside);
        std::mem::swap(&mut *shared.borrow_mut(), &mut outside);
    });
    // A: scoped functions created every frame (say 40 API functions).
    let world_a = RefCell::new(spawn_world(100));
    time("A: lua.scope creating 40 world functions (per frame)", 2_000, || {
        lua.scope(|scope| {
            let w = &world_a;
            for _ in 0..40 {
                let f = scope.create_function(move |_, ()| Ok(w.borrow().entity_count()))?;
                std::hint::black_box(f);
            }
            Ok(())
        })
        .unwrap()
    });

    // ---------- 2. callbacks ----------
    println!("\n-- 10,000 per-entity callbacks --");
    let spinner = r#"
        local Spinner = {}
        function Spinner.on_update(self, dt)
            self.angle = self.angle + self.speed * dt
        end
        return Spinner
    "#;
    let module: LuaTable = lua
        .load(spinner)
        .set_name("=scripts/spinner.luau")
        .eval()?;
    let update: LuaFunction = module.get("on_update")?;
    let instances: Vec<LuaTable> = (0..n)
        .map(|_| {
            let t = lua.create_table()?;
            t.set("angle", 0.0)?;
            t.set("speed", 1.0)?;
            Ok(t)
        })
        .collect::<LuaResult<_>>()?;
    time("Rust calls on_update per instance (10,000 calls)", 50, || {
        for inst in &instances {
            update.call::<()>((inst, 1.0 / 60.0)).unwrap();
        }
    });
    // The same module loaded in its own environment (per-script globals).
    let env = lua.create_table()?;
    let envmt = lua.create_table()?;
    envmt.set("__index", lua.globals())?;
    env.set_metatable(Some(envmt))?;
    let module_env: LuaTable = lua
        .load(spinner)
        .set_name("=scripts/spinner_env.luau")
        .set_environment(env)
        .eval()?;
    let update_env: LuaFunction = module_env.get("on_update")?;
    time("... module in its own environment (10,000 calls)", 50, || {
        for inst in &instances {
            update_env.call::<()>((inst, 1.0 / 60.0)).unwrap();
        }
    });
    let math_src = "local M = {} function M.f(self, dt) self.angle = math.sin(self.angle + dt) end return M";
    let plain: LuaTable = lua.load(math_src).eval()?;
    let env2 = lua.create_table()?;
    let envmt2 = lua.create_table()?;
    envmt2.set("__index", lua.globals())?;
    env2.set_metatable(Some(envmt2))?;
    env2.set_safeenv(true);
    let in_env: LuaTable = lua.load(math_src).set_environment(env2).eval()?;
    let (fp, fe): (LuaFunction, LuaFunction) = (plain.get("f")?, in_env.get("f")?);
    time("math.sin callback, shared globals (10,000 calls)", 50, || {
        for inst in &instances {
            fp.call::<()>((inst, 0.01)).unwrap();
        }
    });
    time("math.sin callback, own safeenv environment (10,000 calls)", 50, || {
        for inst in &instances {
            fe.call::<()>((inst, 0.01)).unwrap();
        }
    });
    // Lua-side dispatcher: one call, Lua loops.
    let list = lua.create_sequence_from(instances.iter().cloned())?;
    let dispatch: LuaFunction = lua
        .load(
            r#"
        return function(list, f, dt)
            for i = 1, #list do
                f(list[i], dt)
            end
        end
    "#,
        )
        .set_name("=dispatch")
        .eval()?;
    time("Lua dispatcher loops 10,000 instances (1 Rust call)", 200, || {
        dispatch
            .call::<()>((&list, &update, 1.0 / 60.0))
            .unwrap()
    });
    // pcall per instance inside the dispatcher (errors isolated per instance).
    let dispatch_pcall: LuaFunction = lua
        .load(
            r#"
        return function(list, f, dt)
            for i = 1, #list do
                local ok, err = pcall(f, list[i], dt)
                if not ok then error(err) end
            end
        end
    "#,
        )
        .set_name("=dispatch_pcall")
        .eval()?;
    time("... with pcall per instance", 200, || {
        dispatch_pcall
            .call::<()>((&list, &update, 1.0 / 60.0))
            .unwrap()
    });
    // A callback that does real work: moves its entity (typed path).
    let mover: LuaFunction = lua
        .load(
            r#"
        return function(list, dt)
            for i = 1, #list do
                local e = list[i]
                local p = get_pos(e)
                set_pos(e, p + vector.create(0, dt, 0))
            end
        end
    "#,
        )
        .set_name("=mover")
        .eval()?;
    let ents = lua.create_sequence_from(es.iter().map(|e| entity_ud(*e)))?;
    time("10,000 x (get_pos + set_pos) typed, light-userdata entity", 50, || {
        mover.call::<()>((&ents, 1.0 / 60.0)).unwrap()
    });

    // ---------- 3. entity handles ----------
    println!("\n-- entity handles --");
    let make_light: LuaFunction = lua
        .load("return function(e, n) local x for i=1,n do x = e end return x end")
        .eval()?;
    let _ = make_light;
    let ud_cost: LuaFunction = lua
        .load("return function(e, n) for i=1,n do local u = entity_full_ud(e) end end")
        .eval()?;
    let e0 = entity_ud(es[0]);
    time("10,000 full userdata entity handles", 50, || {
        ud_cost.call::<()>((e0, 10_000)).unwrap()
    });
    // Light userdata with a type metatable: methods on handles.
    let mt = lua.create_table()?;
    let methods = lua.create_table()?;
    methods.set(
        "pos",
        lua.create_function(|lua, e: LightUserData| {
            let w = lua.app_data_ref::<Shared>().expect("world");
            let w = w.borrow();
            Ok(ud_entity(e)
                .and_then(|e| w.get::<Transform>(e).map(|t| v3(t.translation))))
        })?,
    )?;
    mt.set("__index", methods)?;
    lua.set_type_metatable::<LightUserData>(Some(mt));
    let m: Option<Vector> = lua
        .load("local e = ... return e:pos()")
        .call(e0)?;
    println!("light userdata method call e:pos() -> {m:?}");
    let eq: bool = lua.load("local a, b = ... return a == b").call((e0, e0))?;
    println!("light userdata equality a == b -> {eq}");

    // ---------- 4. component access ----------
    println!("\n-- component get/set (one entity, per call) --");
    let bench = |label: &str, src: &str| -> LuaResult<()> {
        let f: LuaFunction = lua.load(src).eval()?;
        let per = time(label, 20, || f.call::<()>((e0, 10_000)).unwrap()) / 10_000.0;
        println!("{:<64} {:>10.3} us/op", "", per * 1e6);
        Ok(())
    };
    bench(
        "typed get_pos x10,000",
        "return function(e,n) for i=1,n do local p = get_pos(e) end end",
    )?;
    bench(
        "typed set_pos x10,000",
        "return function(e,n) local v = vector.create(1,2,3) for i=1,n do set_pos(e, v) end end",
    )?;
    bench(
        "generic get_reflect('Transform') direct walk x10,000",
        "return function(e,n) for i=1,n do local t = get_reflect(e, 'Transform') end end",
    )?;
    bench(
        "generic set_reflect('Transform', {translation=v}) patch x10,000",
        "return function(e,n) local v = vector.create(1,2,3) for i=1,n do set_reflect(e, 'Transform', {translation = v}) end end",
    )?;
    bench(
        "generic get_cached('Transform') interned keys x10,000",
        "return function(e,n) for i=1,n do local t = get_cached(e, 'Transform') end end",
    )?;
    bench(
        "generic get_json('Transform') x10,000",
        "return function(e,n) for i=1,n do local t = get_json(e, 'Transform') end end",
    )?;
    bench(
        "generic set_json('Transform', full table) x10,000",
        "return function(e,n) local t = get_json(e, 'Transform') for i=1,n do set_json(e, 'Transform', t) end end",
    )?;

    // ---------- 5. batch queries ----------
    println!("\n-- batch over 5,000 entities --");
    *shared.borrow_mut() = spawn_world(5000);
    let per_row: LuaFunction = lua
        .load(
            r#"
        return function(dt)
            each_transform(function(e, p) return p + vector.create(0, dt, 0) end)
        end
    "#,
        )
        .eval()?;
    time("each_transform: Rust calls a Lua closure per row", 50, || {
        per_row.call::<()>(1.0 / 60.0).unwrap()
    });
    let arrays: LuaFunction = lua
        .load(
            r#"
        return function(dt)
            local es, ps = query_transforms()
            local up = vector.create(0, dt, 0)
            for i = 1, #ps do ps[i] = ps[i] + up end
            write_transforms(es, ps)
        end
    "#,
        )
        .eval()?;
    time("query_transforms arrays + Lua loop + write back", 50, || {
        arrays.call::<()>(1.0 / 60.0).unwrap()
    });
    let ents_typed: LuaFunction = lua
        .load(
            r#"
        return function(dt)
            local up = vector.create(0, dt, 0)
            for _, e in query_entities() do
                set_pos(e, get_pos(e) + up)
            end
        end
    "#,
        )
        .eval()?;
    time("query_entities array + typed get/set per entity", 50, || {
        ents_typed.call::<()>(1.0 / 60.0).unwrap()
    });
    let y = shared
        .borrow()
        .query::<&Transform>()
        .iter()
        .next()
        .map(|t| t.translation.y);
    println!("check: first entity y after the runs = {y:?}");

    // ---------- 6. value types ----------
    println!("\n-- value types --");
    bench(
        "Quat as userdata x10,000",
        "return function(e,n) for i=1,n do local q = quat_ud() end end",
    )?;
    bench(
        "Quat as table {x,y,z,w} x10,000",
        "return function(e,n) for i=1,n do local q = quat_table() end end",
    )?;
    bench(
        "Lua-side table {x,y,z,w} constructor x10,000",
        "return function(e,n) for i=1,n do local q = {x=0,y=0,z=0,w=1} end end",
    )?;
    bench(
        "native vector arithmetic (v + w) * 2 x10,000",
        "return function(e,n) local v = vector.create(1,2,3) for i=1,n do v = (v + v) * 0.5 end end",
    )?;
    bench(
        "Quat userdata * vector x10,000",
        "return function(e,n) local q = quat_ud() local v = vector.create(1,0,0) for i=1,n do v = q * v end end",
    )?;

    // ---------- 7. coroutines, errors, sandbox, state ----------
    println!("\n-- coroutines --");
    let co_src = r#"
        local function wait(s) return coroutine.yield(s) end
        return function()
            local t = 0
            for i = 1, 3 do t = t + wait(0.5) end
            return t
        end
    "#;
    let body: LuaFunction = lua.load(co_src).set_name("=scripts/waiter.luau").eval()?;
    let th = lua.create_thread(body.clone())?;
    let mut resumes = 0;
    let mut arg = Value::Nil;
    loop {
        let r: Value = th.resume(arg.clone())?;
        resumes += 1;
        if !th.is_resumable() {
            println!("coroutine finished after {resumes} resumes, result {r:?}");
            break;
        }
        // The scheduler would wait `r` seconds; pass the elapsed time back.
        arg = r;
    }
    let threads: Vec<mlua::Thread> = (0..1000)
        .map(|_| {
            lua.create_thread(
                lua.load("return function() while true do coroutine.yield(0) end end")
                    .eval::<LuaFunction>()
                    .unwrap(),
            )
            .unwrap()
        })
        .collect();
    time("resume 1,000 waiting coroutines", 200, || {
        for t in &threads {
            t.resume::<()>(()).unwrap();
        }
    });

    println!("\n-- errors --");
    let bad = "local M = {}\nfunction M.on_update(self, dt)\n    local x = nil\n    return x.y\nend\nreturn M";
    let m: LuaTable = lua.load(bad).set_name("@scripts/bad.luau").eval()?;
    let f: LuaFunction = m.get("on_update")?;
    match f.call::<()>((lua.create_table()?, 0.1)) {
        Ok(()) => println!("unexpected: no error"),
        Err(e) => println!("runtime error: {e}"),
    }
    match lua.load("local x = = 1").set_name("@scripts/syntax.luau").exec() {
        Ok(()) => println!("unexpected: no error"),
        Err(e) => println!("syntax error: {e}"),
    }
    let tb: String = lua
        .load("local function f() error('boom') end local ok, e = xpcall(f, debug.traceback) return e")
        .set_name("@scripts/trace.luau")
        .eval()?;
    println!("xpcall traceback:\n{tb}");

    println!("\n-- sandbox --");
    for probe in [
        "return io",
        "return os.execute",
        "return os.getenv",
        "return os.remove",
        "return require",
        "return loadstring ~= nil",
        "return getfenv ~= nil",
        "return package",
        "return dofile",
        "return os.clock() >= 0",
        "string.upper = nil return 'mutated stdlib'",
    ] {
        let r: LuaResult<Value> = lua.load(probe).eval();
        match r {
            Ok(v) => println!("{probe:<45} -> {v:?}"),
            Err(e) => println!("{probe:<45} -> error: {}", e.to_string().lines().next().unwrap_or("")),
        }
    }
    // Per-script environments: globals do not leak between scripts.
    let env_a = lua.create_table()?;
    let mt = lua.create_table()?;
    mt.set("__index", lua.globals())?;
    env_a.set_metatable(Some(mt.clone()))?;
    let env_b = lua.create_table()?;
    env_b.set_metatable(Some(mt))?;
    lua.load("leaked = 1").set_environment(env_a).exec()?;
    let seen: Value = lua.load("return leaked").set_environment(env_b).eval()?;
    println!("global from script A seen by script B: {seen:?}");
    // Interrupt budget for runaway loops.
    let budget = Rc::new(std::cell::Cell::new(0u64));
    let b = budget.clone();
    lua.set_interrupt(move |_| {
        b.set(b.get() + 1);
        if b.get() > 100_000 {
            Err(LuaError::runtime("script exceeded its instruction budget"))
        } else {
            Ok(mlua::VmState::Continue)
        }
    });
    let r = lua
        .load("while true do end")
        .set_name("@scripts/loop.luau")
        .exec();
    println!(
        "infinite loop with interrupt budget -> {}",
        r.err().map(|e| e.to_string()).unwrap_or_default()
    );
    lua.remove_interrupt();

    println!("\n-- saved state --");
    let st: LuaTable = lua
        .load("return { score = 12, inventory = {'key', 'gem'}, pos = {1, 2, 3}, nested = { open = true } }")
        .eval()?;
    let json: serde_json::Value = lua.from_value(Value::Table(st))?;
    println!("on_save table -> JSON: {json}");
    let back: Value = lua.to_value(&json)?;
    let score: f64 = match back {
        Value::Table(t) => t.get("score")?,
        _ => 0.0,
    };
    println!("JSON -> Lua: score = {score}");
    let v: LuaResult<serde_json::Value> = lua.from_value(Value::Vector(Vector::new(1.0, 2.0, 3.0)));
    println!("vector -> JSON through serde: {v:?}");
    println!("\nLua memory in use: {} KiB", lua.used_memory() / 1024);
    Ok(())
}
