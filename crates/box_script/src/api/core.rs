//! Values (`Quat`, `Color`), `log`/`print`, waits and tasks, `require`.

use std::rc::Rc;

use box_core::color::Color;
use glam::{EulerRot, Quat, Vec3};
use mlua::{
    Function, Lua, MultiValue, Result as LuaResult, Table, Thread, Value, Variadic, Vector,
};

use super::{Host, ScriptApi, Task, Wake, current_script, host};
use crate::convert::{LuaQuat, color_table, from_vector};

pub(crate) struct CoreApi;

/// Waits, written in Luau: they yield a request the scheduler reads.
const PRELUDE: &str = r#"
local yield, isyieldable = coroutine.yield, coroutine.isyieldable
local function check(name)
    if not isyieldable() then
        error(name .. "() works only in on_start or in a task.spawn function", 3)
    end
end
function wait(seconds)
    check("wait")
    return yield("box.wait", seconds or 0)
end
function wait_frames(frames)
    check("wait_frames")
    return yield("box.frames", frames or 1)
end
function wait_until(condition, timeout)
    check("wait_until")
    return yield("box.until", condition, timeout)
end
"#;

impl ScriptApi for CoreApi {
    fn name(&self) -> &str {
        "core"
    }

    fn install(&self, lua: &Lua, g: &Table) -> LuaResult<()> {
        // Deterministic `math.random` (seed 1) for every VM.
        lua.load("math.randomseed(1)").set_name("=box").exec()?;
        g.set("loadstring", Value::Nil)?;
        g.set("Quat", quat_table(lua)?)?;
        g.set("Color", color_lib(lua)?)?;
        let log = lua.create_table()?;
        log.set("info", log_fn(lua, Level::Info)?)?;
        log.set("warn", log_fn(lua, Level::Warn)?)?;
        log.set("error", log_fn(lua, Level::Error)?)?;
        g.set("log", log)?;
        g.set("print", log_fn(lua, Level::Info)?)?;
        lua.load(PRELUDE).set_name("=box prelude").exec()?;
        let task = lua.create_table()?;
        task.set(
            "spawn",
            lua.create_function(|lua, (f, args): (Function, MultiValue)| {
                let host = host(lua)?;
                spawn(lua, &host, f, args, host.current.get())
            })?,
        )?;
        task.set(
            "delay",
            lua.create_function(|lua, (seconds, f, args): (f64, Function, MultiValue)| {
                let host = host(lua)?;
                let wrapper: Function = lua
                    .load("local s, f = ... return function(...) wait(s) return f(...) end")
                    .set_name("=box task.delay")
                    .call((seconds, f))?;
                spawn(lua, &host, wrapper, args, host.current.get())
            })?,
        )?;
        task.set(
            "cancel",
            lua.create_function(|lua, id: u64| {
                let host = host(lua)?;
                let mut tasks = host.tasks.borrow_mut();
                let before = tasks.len();
                tasks.retain(|t| t.id != id);
                Ok(tasks.len() != before)
            })?,
        )?;
        g.set("task", task)?;
        g.set("require", lua.create_function(require)?)?;
        Ok(())
    }

    fn definitions(&self) -> String {
        r##"--- A rotation (unit quaternion). Values are immutable: every operation returns a new Quat. `q * v` rotates a vector, `q * r` applies `r` first, then `q`. Compares with `==`.
declare extern type Quat with
    --- The x component.
    x: number
    --- The y component.
    y: number
    --- The z component.
    z: number
    --- The w (scalar) component.
    w: number
    --- The opposite rotation.
    function inverse(self): Quat
    --- The same rotation scaled back to unit length (after many multiplications).
    function normalize(self): Quat
    --- Spherical interpolation from this rotation (t = 0) to `to` (t = 1).
    function slerp(self, to: Quat, t: number): Quat
    --- The angle between two rotations, in radians.
    function angle_to(self, other: Quat): number
    --- The rotated forward direction (-Z).
    function forward(self): vector
    --- The rotated right direction (+X).
    function right(self): vector
    --- The rotated up direction (+Y).
    function up(self): vector
    --- Euler angles in radians, as `x, y, z` (pitch, yaw, roll in YXZ order), the inverse of `Quat.from_euler`.
    function to_euler(self): (number, number, number)
    --- The rotation axis (unit vector) and angle in radians.
    function to_axis_angle(self): (vector, number)
    --- `q * r`: the rotation `r` followed by `q`.
    function __mul(self, other: Quat): Quat
    --- `q * v`: the vector `v` rotated by `q`.
    function __mul(self, other: vector): vector
end

--- Constructors of rotations.
declare Quat: {
    --- No rotation.
    identity: () -> Quat,
    --- A rotation from raw components (normalized).
    new: (x: number, y: number, z: number, w: number) -> Quat,
    --- A rotation of `radians` around `axis` (normalized; +Y when zero).
    from_axis_angle: (axis: vector, radians: number) -> Quat,
    --- A rotation from Euler angles in radians: pitch `x`, yaw `y` and roll `z`, in YXZ order (yaw, then pitch, then roll, each about the already turned axes).
    from_euler: (x: number, y: number, z: number) -> Quat,
    --- A rotation of `radians` around the X axis.
    from_rotation_x: (radians: number) -> Quat,
    --- A rotation of `radians` around the Y axis (yaw; positive turns left, from -Z towards -X).
    from_rotation_y: (radians: number) -> Quat,
    --- A rotation of `radians` around the Z axis.
    from_rotation_z: (radians: number) -> Quat,
    --- The rotation whose forward (-Z) points along `direction`, with `up` (default +Y) as close to up as possible.
    look_to: (direction: vector, up: vector?) -> Quat,
}

--- A colour with linear channels from 0 to 1 (alpha 1 is opaque). Components store colours this way; build one with `Color.rgb`, `Color.srgb` or `Color.hex`, or write a table `{ r = 1, g = 0.5, b = 0, a = 1 }`.
type Color = {
    --- Red, linear.
    r: number,
    --- Green, linear.
    g: number,
    --- Blue, linear.
    b: number,
    --- Alpha (opacity).
    a: number,
}

--- Constructors of colours.
declare Color: {
    --- A colour from linear channels (alpha defaults to 1).
    rgb: (r: number, g: number, b: number, a: number?) -> Color,
    --- A colour from sRGB channels from 0 to 1, as colour pickers show them, converted to linear (alpha defaults to 1).
    srgb: (r: number, g: number, b: number, a: number?) -> Color,
    --- A colour from an sRGB hex string, "#RRGGBB" or "RRGGBB".
    hex: (hex: string) -> Color,
}

--- Messages in the editor's Console and the terminal, prefixed with the script's path. Values are converted with `tostring` and joined with spaces. `print(...)` is `log.info(...)`.
declare log: {
    --- An information message.
    info: (...any) -> (),
    --- A warning: also an editor toast and a Console badge count.
    warn: (...any) -> (),
    --- An error: also an editor toast and a Console badge count. It does not stop the script (use Luau's `error()` for that).
    error: (...any) -> (),
}

--- Suspends the running task for `seconds` of game time (default 0) and returns the seconds actually waited. Works in `on_start` and in `task.spawn` / `task.delay` functions; elsewhere it fails. Pausing or scaling time pauses or scales waits.
declare function wait(seconds: number?): number
--- Suspends the running task for `frames` frames (default 1). Same rules as `wait`.
declare function wait_frames(frames: number?): ()
--- Suspends the running task until `condition()` returns true (checked once per frame) and returns true, or returns false after `timeout` seconds of game time. Same rules as `wait`.
declare function wait_until(condition: () -> boolean, timeout: number?): boolean

--- Tasks: functions that run alongside the frame loop and may `wait`. A task belongs to the script instance that started it and stops with it (or when its file reloads with `on_reload`).
declare task: {
    --- Runs `f(...)` at once until its first wait, then resumes it when the wait ends (in Update, on game time). Returns the task id for `task.cancel`.
    spawn: <A...>(f: (A...) -> ...any, A...) -> number,
    --- Runs `f(...)` as a task after `seconds` of game time. Returns the task id.
    delay: <A...>(seconds: number, f: (A...) -> ...any, A...) -> number,
    --- Stops a waiting task. Returns true if it was still waiting.
    cancel: (id: number) -> boolean,
}
"##
        .into()
    }

    fn doc(&self) -> &str {
        r##"The basics every script has: the value types `vector`, `Quat` and `Color`, logging, waits and tasks, `require`.

**`vector`** is Luau's built-in 3-component vector (positions, directions, velocities; a Vec2 field uses z = 0). Create one with `vector.create(x, y, z)` and use `+`, `-`, `*` and `/` with numbers or other vectors. Read components with `v.x`, `v.y`, `v.z`. The `vector` library has `magnitude`, `normalize`, `dot`, `cross`, `angle`, `floor`, `ceil`, `abs`, `sign`, `clamp`, `min`, `max`, and the constants `vector.zero` and `vector.one`. The engine's axes: +Y is up, -Z is forward, units are metres and angles are radians (`math.rad(degrees)`).

**`print(...)`** logs like `log.info`. **`require(path)`** loads another module: `"./util"` and `"../shared/math"` are relative to the requiring script, other paths are relative to `assets/` (the `.luau` extension is optional). Modules are cached, may not require each other in a cycle, and reload when their file changes.

Scripts run in Luau's sandbox: no `io`, `os.execute`, `os.getenv`, `loadstring` or `dofile`, and built-in libraries are read-only. `math.random` is seeded with 1, so a scene plays back the same way every time.

```lua
function Countdown.on_start(self)
    local axis = vector.create(0, 1, 0)
    local turn = Quat.from_axis_angle(axis, math.rad(90))
    local ahead = turn * vector.create(0, 0, -1)   -- (-1, 0, 0)
    log.info("ahead is", ahead, "tint", Color.hex("#ff8800").r)

    task.spawn(function()
        for i = 3, 1, -1 do
            log.info("starting in", i)
            wait(1)
        end
        log.info("go!")
    end)
end
```"##
    }
}

fn quat_table(lua: &Lua) -> LuaResult<Table> {
    let q = lua.create_table()?;
    q.set(
        "identity",
        lua.create_function(|_, ()| Ok(LuaQuat(Quat::IDENTITY)))?,
    )?;
    q.set(
        "new",
        lua.create_function(|_, (x, y, z, w): (f32, f32, f32, f32)| {
            Ok(LuaQuat(Quat::from_xyzw(x, y, z, w).normalize()))
        })?,
    )?;
    q.set(
        "from_axis_angle",
        lua.create_function(|_, (axis, angle): (Vector, f32)| {
            Ok(LuaQuat(Quat::from_axis_angle(
                from_vector(axis).normalize_or(Vec3::Y),
                angle,
            )))
        })?,
    )?;
    q.set(
        "from_euler",
        lua.create_function(|_, (x, y, z): (f32, f32, f32)| {
            Ok(LuaQuat(Quat::from_euler(EulerRot::YXZ, y, x, z)))
        })?,
    )?;
    q.set(
        "from_rotation_x",
        lua.create_function(|_, a: f32| Ok(LuaQuat(Quat::from_rotation_x(a))))?,
    )?;
    q.set(
        "from_rotation_y",
        lua.create_function(|_, a: f32| Ok(LuaQuat(Quat::from_rotation_y(a))))?,
    )?;
    q.set(
        "from_rotation_z",
        lua.create_function(|_, a: f32| Ok(LuaQuat(Quat::from_rotation_z(a))))?,
    )?;
    q.set(
        "look_to",
        lua.create_function(|_, (dir, up): (Vector, Option<Vector>)| {
            let up = up.map_or(Vec3::Y, from_vector);
            let t = box_core::transform::Transform::default().looking_to(from_vector(dir), up);
            Ok(LuaQuat(t.rotation))
        })?,
    )?;
    Ok(q)
}

fn color_lib(lua: &Lua) -> LuaResult<Table> {
    let c = lua.create_table()?;
    c.set(
        "rgb",
        lua.create_function(|lua, (r, g, b, a): (f32, f32, f32, Option<f32>)| {
            color_table(lua, Color::linear_rgba(r, g, b, a.unwrap_or(1.0)))
        })?,
    )?;
    c.set(
        "srgb",
        lua.create_function(|lua, (r, g, b, a): (f32, f32, f32, Option<f32>)| {
            let mut c = Color::srgb(r, g, b);
            c.a = a.unwrap_or(1.0);
            color_table(lua, c)
        })?,
    )?;
    c.set(
        "hex",
        lua.create_function(|lua, s: String| {
            // `RRGGBB` or `RRGGBBAA` (as theme files write colours); alpha
            // is linear.
            let digits = s.trim_start_matches('#');
            let v = u32::from_str_radix(digits, 16)
                .ok()
                .filter(|_| digits.len() == 6 || digits.len() == 8)
                .ok_or_else(|| {
                    mlua::Error::runtime(format!("Color.hex: `{s}` is not RRGGBB or RRGGBBAA"))
                })?;
            let c = if digits.len() == 8 {
                Color::hex(v >> 8).with_alpha((v & 0xff) as f32 / 255.0)
            } else {
                Color::hex(v)
            };
            color_table(lua, c)
        })?,
    )?;
    Ok(c)
}

#[derive(Clone, Copy)]
enum Level {
    Info,
    Warn,
    Error,
}

fn log_fn(lua: &Lua, level: Level) -> LuaResult<Function> {
    lua.create_function(move |lua, args: Variadic<Value>| {
        let tostring: Function = lua.globals().get("tostring")?;
        let mut parts = Vec::with_capacity(args.len());
        for a in args.iter() {
            parts.push(tostring.call::<String>(a.clone())?);
        }
        let msg = parts.join(" ");
        let at = current_script(lua);
        let at = at.as_deref().unwrap_or("script");
        match level {
            Level::Info => tracing::info!(target: "box_script", "{at}: {msg}"),
            Level::Warn => tracing::warn!(target: "box_script", "{at}: {msg}"),
            Level::Error => tracing::error!(target: "box_script", "{at}: {msg}"),
        }
        Ok(())
    })
}

// ----- tasks -----

/// What a coroutine step ended with.
pub(crate) enum Step {
    Done,
    Waiting(Wake),
    Failed(mlua::Error),
}

/// Game time and frame now.
pub(crate) fn clock(host: &Host) -> (f64, u64) {
    host.world
        .try_borrow()
        .ok()
        .and_then(|w| {
            w.get_resource::<box_core::time::Time>()
                .map(|t| (t.elapsed_f64, t.frame))
        })
        .unwrap_or((0.0, 0))
}

/// Resumes `thread` with `args` and reads what it waits for next.
pub(crate) fn step(thread: &Thread, args: MultiValue, now: f64, frame: u64) -> Step {
    let out = match thread.resume::<MultiValue>(args) {
        Ok(out) => out,
        Err(e) => return Step::Failed(e),
    };
    if !thread.is_resumable() {
        return Step::Done;
    }
    let mut it = out.into_iter();
    let kind = match it.next() {
        Some(Value::String(s)) => s.to_str().map(|s| s.to_string()).unwrap_or_default(),
        _ => return Step::Waiting(Wake::Next),
    };
    let num = |v: Option<Value>| match v {
        Some(Value::Number(n)) => n,
        Some(Value::Integer(i)) => i as f64,
        _ => 0.0,
    };
    match kind.as_str() {
        "box.wait" => Step::Waiting(Wake::At(now + num(it.next()).max(0.0))),
        "box.frames" => Step::Waiting(Wake::Frame(frame + num(it.next()).max(1.0) as u64)),
        "box.until" => match it.next() {
            Some(Value::Function(f)) => {
                let deadline = match it.next() {
                    Some(Value::Number(t)) => Some(now + t),
                    Some(Value::Integer(t)) => Some(now + t as f64),
                    _ => None,
                };
                Step::Waiting(Wake::Until(f, deadline))
            }
            _ => Step::Failed(mlua::Error::runtime(
                "wait_until: expected a function returning true when done",
            )),
        },
        _ => Step::Waiting(Wake::Next),
    }
}

/// Starts `f(args...)` as a task owned by `owner`: runs it at once until it
/// first waits. Returns the task id.
pub(crate) fn spawn(
    lua: &Lua,
    host: &Rc<Host>,
    f: Function,
    args: MultiValue,
    owner: Option<u64>,
) -> LuaResult<u64> {
    let thread = lua.create_thread(f)?;
    let id = host.next_task.get();
    host.next_task.set(id + 1);
    let (now, frame) = clock(host);
    match step(&thread, args, now, frame) {
        Step::Done => {}
        Step::Waiting(wake) => host.tasks.borrow_mut().push(Task {
            id,
            thread,
            owner,
            wake,
            since: now,
        }),
        Step::Failed(e) => {
            let msg = crate::runtime::describe_error(&e);
            match owner {
                Some(o) => host.failures.borrow_mut().push((o, msg)),
                None => tracing::error!(target: "box_script", "task failed: {msg}"),
            }
        }
    }
    Ok(id)
}

// ----- require -----

/// A fresh environment for one script module: its own globals, reading
/// through to the shared, read-only built-ins (`safeenv`, so built-in
/// lookups keep the interpreter's fast path).
pub(crate) fn module_env(lua: &Lua) -> LuaResult<Table> {
    let env = lua.create_table()?;
    let mt = lua.create_table()?;
    mt.set("__index", lua.globals())?;
    env.set_metatable(Some(mt))?;
    env.set_safeenv(true);
    Ok(env)
}

/// The assets-relative path of the script that called the running function.
pub(crate) fn caller_path(lua: &Lua) -> Option<String> {
    for level in 1..4 {
        let src = lua.inspect_stack(level, |d| d.source().source.map(|s| s.to_string()));
        match src.flatten() {
            Some(s) if s.starts_with('@') => return Some(s[1..].to_string()),
            Some(_) | None => continue,
        }
    }
    None
}

/// `name` as an assets-relative `.luau` path: `./x` and `../x` relative to
/// the requiring script's folder, anything else relative to `assets/`.
pub(crate) fn resolve_require(caller: Option<&str>, name: &str) -> LuaResult<String> {
    let joined = if name.starts_with("./") || name.starts_with("../") {
        let dir = caller
            .and_then(|c| c.rsplit_once('/').map(|(d, _)| d))
            .unwrap_or("");
        if dir.is_empty() {
            name.to_string()
        } else {
            format!("{dir}/{name}")
        }
    } else {
        name.to_string()
    };
    let with_ext = if joined.ends_with(".luau") {
        joined
    } else {
        format!("{joined}.luau")
    };
    box_assets::normalize_rel(&with_ext).ok_or_else(|| {
        mlua::Error::runtime(format!(
            "require(\"{name}\"): scripts can only require files inside the project's assets folder"
        ))
    })
}

fn require(lua: &Lua, name: String) -> LuaResult<Value> {
    let host = host(lua)?;
    let caller = caller_path(lua);
    let path = resolve_require(caller.as_deref(), &name)?;
    if let Some(c) = &caller {
        let mut deps = host.deps.borrow_mut();
        let list = deps.entry(c.clone()).or_default();
        if !list.contains(&path) {
            list.push(path.clone());
        }
    }
    if let Some(v) = host.modules.borrow().get(&path) {
        return Ok(v.clone());
    }
    if host.loading.borrow().contains(&path) {
        return Err(mlua::Error::runtime(format!(
            "require(\"{name}\"): circular require of {path}"
        )));
    }
    let source = match host.sources.borrow().get(&path) {
        Some(s) => s.clone(),
        None => {
            let dir = host.assets_dir.borrow().clone().ok_or_else(|| {
                mlua::Error::runtime(format!(
                    "require(\"{name}\"): no project assets folder to read {path} from"
                ))
            })?;
            box_assets::vfs::read_to_string(dir.join(&path)).map_err(|e| {
                mlua::Error::runtime(format!("require(\"{name}\"): cannot read {path}: {e}"))
            })?
        }
    };
    host.loading.borrow_mut().push(path.clone());
    let result = lua
        .load(&source)
        .set_name(format!("@{path}"))
        .set_environment(module_env(lua)?)
        .eval::<Value>();
    host.loading.borrow_mut().retain(|p| p != &path);
    let value = match result? {
        Value::Nil => Value::Boolean(true),
        v => v,
    };
    host.modules.borrow_mut().insert(path, value.clone());
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn require_paths_stay_in_the_project() {
        assert_eq!(
            resolve_require(Some("scripts/a.luau"), "./util").unwrap(),
            "scripts/util.luau"
        );
        assert_eq!(
            resolve_require(Some("scripts/ai/a.luau"), "../lib/x.luau").unwrap(),
            "scripts/lib/x.luau"
        );
        assert_eq!(
            resolve_require(Some("scripts/a.luau"), "lib/m").unwrap(),
            "lib/m.luau"
        );
        assert!(resolve_require(Some("scripts/a.luau"), "../../etc/passwd").is_err());
        assert!(resolve_require(None, "/etc/passwd").is_err());
    }
}
