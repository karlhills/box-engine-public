//! The engine API scripts see, and the hook other crates use to add theirs.
//!
//! Every global table scripts use comes from a [`ScriptApi`]: the built-in
//! ones ([`builtin_apis`]: values, entities, `world`, `time`, `input`,
//! `physics`, `events`, `task`, `log`) and any registered by plugins through
//! [`ScriptApis`] (box_engine adds `scenes`). Component, resource and event
//! types need no binding: `e:get("Name")`, `world.resource("Name")` and
//! `events.send("Name", ...)` reach every type in the registry, so types
//! other crates register appear in scripts automatically, and in the
//! generated type definitions ([`crate::types`]).
//!
//! A hand-written binding for a crate:
//!
//! ```
//! use box_script::{ScriptApi, ScriptApis, mlua, with_world};
//! use box_core::prelude::*;
//!
//! struct Weather;
//! impl ScriptApi for Weather {
//!     fn name(&self) -> &str { "weather" }
//!     fn install(&self, lua: &mlua::Lua, globals: &mlua::Table) -> mlua::Result<()> {
//!         let t = lua.create_table()?;
//!         t.set("frame", lua.create_function(|lua, ()| {
//!             with_world(lua, |w| w.resource::<Time>().frame)
//!         })?)?;
//!         globals.set("weather", t)
//!     }
//!     fn definitions(&self) -> String {
//!         "declare weather: { frame: () -> number }\n".into()
//!     }
//! }
//!
//! let mut app = App::new();
//! app.world.resource_or_default::<ScriptApis>().add(Weather);
//! ```

mod core;
mod events;
pub(crate) mod ground;
mod input;
mod physics;
mod world;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use box_core::world::{Entity, World};
use mlua::{Function, Lua, Result as LuaResult, Table, Thread, Value};

pub(crate) use core::{Step, caller_path, clock, module_env, resolve_require, spawn, step};
pub(crate) use events::Subscription;
pub use events::{ScriptCall, ScriptCalls, ScriptSignals, subscribe, unsubscribe};

/// A group of script globals (a table such as `physics`, or functions)
/// with its luau-lsp type declarations.
pub trait ScriptApi: 'static {
    /// A short name for logs and errors (usually the global it installs).
    fn name(&self) -> &str;

    /// Creates the API's globals in `globals` (the VM's global table, before
    /// it is sandboxed and made read-only). Functions reach the world with
    /// [`with_world`] / [`with_world_mut`].
    fn install(&self, lua: &Lua, globals: &Table) -> LuaResult<()>;

    /// Declarations for the generated `.d.luau` definitions file (luau-lsp
    /// syntax: `declare name: { ... }`, `declare function f(...): T`).
    ///
    /// Put a `--- description` line (one or more) directly above every
    /// declaration, table field, class member and type field: luau-lsp
    /// shows it on hover, and the generated reference
    /// ([`crate::docs`], `box script docs`) is built from it. Keep one
    /// field per line so each can carry its own description; the reference
    /// test fails on any item without one.
    fn definitions(&self) -> String {
        String::new()
    }

    /// Method declarations this API adds to the `Entity` class (through
    /// [`entity_methods`]), one per line in luau-lsp class syntax
    /// (`function jump(self): boolean`), each with a `---` description
    /// line above it, as in [`definitions`](Self::definitions).
    fn entity_definitions(&self) -> String {
        String::new()
    }

    /// An overview of the API for its page of the generated reference
    /// (Markdown: what it is for, how its parts fit together, a short
    /// example). Required for every API; items are described in
    /// [`definitions`](Self::definitions).
    fn doc(&self) -> &str {
        ""
    }
}

/// The method table of entity handles (`e:name()`), for APIs that add
/// entity methods in [`ScriptApi::install`] (declare them in
/// [`ScriptApi::entity_definitions`]). The built-in entity methods are
/// installed before any plugin's API.
pub fn entity_methods(lua: &Lua) -> LuaResult<Table> {
    let mt = lua
        .type_metatable::<mlua::LightUserData>()
        .ok_or_else(|| mlua::Error::runtime("entity methods are not installed yet"))?;
    mt.raw_get("__index")
}

/// Resource: script APIs added by plugins (`build`), installed in every
/// script VM of the app after the built-in ones. The VM is created on the
/// first frame that runs a script, so APIs added in any plugin's `build`
/// are present.
#[derive(Clone, Default)]
pub struct ScriptApis {
    apis: Vec<Arc<dyn ScriptApi>>,
}

impl ScriptApis {
    /// Adds an API; one with the same name replaces it.
    pub fn add(&mut self, api: impl ScriptApi) -> &mut Self {
        let name = api.name().to_owned();
        self.apis.retain(|a| a.name() != name);
        self.apis.push(Arc::new(api));
        self
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn ScriptApi>> {
        self.apis.iter()
    }

    pub fn len(&self) -> usize {
        self.apis.len()
    }

    pub fn is_empty(&self) -> bool {
        self.apis.is_empty()
    }
}

/// The built-in APIs, in install order.
pub fn builtin_apis() -> Vec<Arc<dyn ScriptApi>> {
    vec![
        Arc::new(core::CoreApi),
        Arc::new(world::EntityApi),
        Arc::new(world::WorldApi),
        Arc::new(input::TimeApi),
        Arc::new(input::AppApi),
        Arc::new(input::InputApi),
        Arc::new(physics::PhysicsApi),
        Arc::new(physics::CharacterApi),
        Arc::new(events::EventsApi),
        Arc::new(ground::GroundApi),
    ]
}

// ----- the host: what API functions share -----

/// When a waiting task resumes.
pub(crate) enum Wake {
    /// At this game time (`Time::elapsed_f64`).
    At(f64),
    /// At this frame (`Time::frame`).
    Frame(u64),
    /// When the function returns true, or at the deadline (game time).
    Until(Function, Option<f64>),
    /// Next frame (a plain `coroutine.yield` inside a task).
    Next,
}

pub(crate) struct Task {
    pub id: u64,
    pub thread: Thread,
    /// The script instance that started it (cancelled with it).
    pub owner: Option<u64>,
    pub wake: Wake,
    /// Game time when it started waiting (what `wait` returns).
    pub since: f64,
}

/// State the API functions share with the runtime, kept in the VM's app
/// data. The world lives here while scripts run (moved in and out by the
/// runtime, so no pointer to it is ever held).
pub(crate) struct Host {
    pub world: RefCell<World>,
    /// True while a script stage runs (the world above is the real one).
    pub live: Cell<bool>,
    /// The instance whose callback runs, and its script path.
    pub current: Cell<Option<u64>>,
    pub current_path: RefCell<Option<Rc<str>>>,
    pub tasks: RefCell<Vec<Task>>,
    pub next_task: Cell<u64>,
    pub subscriptions: RefCell<Vec<Subscription>>,
    pub next_subscription: Cell<u64>,
    /// Read positions of registry event queues by event name.
    pub event_cursors: RefCell<HashMap<String, u64>>,
    /// Script-only signals sent this frame: name and payload.
    pub signals: RefCell<Vec<(String, Value)>>,
    /// Instances to disable after a failure inside a task or handler:
    /// instance id and message.
    pub failures: RefCell<Vec<(u64, String)>>,
    /// Instance tables by entity (`e:script()`).
    pub instances: RefCell<HashMap<Entity, Table>>,
    /// Running instances in start order: entity and script path
    /// (`world.scripts`).
    pub script_list: RefCell<Vec<(Entity, Rc<str>)>>,
    /// `require` cache: path -> module value.
    pub modules: RefCell<HashMap<String, Value>>,
    /// `require` calls being loaded (cycle detection).
    pub loading: RefCell<Vec<String>>,
    /// Required modules per requiring script path (hot reload).
    pub deps: RefCell<HashMap<String, Vec<String>>>,
    /// The project's `assets/` folder, where `require` reads.
    pub assets_dir: RefCell<Option<PathBuf>>,
    /// Sources of loaded script assets by path (preferred over the disk).
    pub sources: RefCell<HashMap<String, String>>,
}

impl Host {
    pub fn new() -> Self {
        Self {
            world: RefCell::new(World::new()),
            live: Cell::new(false),
            current: Cell::new(None),
            current_path: RefCell::new(None),
            tasks: RefCell::new(Vec::new()),
            next_task: Cell::new(1),
            subscriptions: RefCell::new(Vec::new()),
            next_subscription: Cell::new(1),
            event_cursors: RefCell::new(HashMap::new()),
            signals: RefCell::new(Vec::new()),
            failures: RefCell::new(Vec::new()),
            instances: RefCell::new(HashMap::new()),
            script_list: RefCell::new(Vec::new()),
            modules: RefCell::new(HashMap::new()),
            loading: RefCell::new(Vec::new()),
            deps: RefCell::new(HashMap::new()),
            assets_dir: RefCell::new(None),
            sources: RefCell::new(HashMap::new()),
        }
    }
}

// ----- deprecations -----

/// Deprecated names already reported (once per name per process).
fn reported() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static REPORTED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    REPORTED.get_or_init(Default::default)
}

/// Logs, once per process for `old`, that the script API name `old` is
/// deprecated in favour of `new`, with the calling script's `file:line`
/// (a `box_script` warning: an editor toast and Console count). The policy
/// is in docs/STABILITY.md: a deprecated name keeps working, warns, and is
/// removed no earlier than two minor releases later.
pub fn deprecated(lua: &Lua, old: &str, new: &str) {
    let first = reported()
        .lock()
        .map(|mut r| r.insert(old.to_string()))
        .unwrap_or(false);
    if !first {
        return;
    }
    let at = (1..6)
        .find_map(|level| {
            lua.inspect_stack(level, |d| {
                let src = d.source().source.map(|s| s.to_string())?;
                let line = d.current_line()?;
                src.strip_prefix('@').map(|p| format!("{p}:{line}: "))
            })
            .flatten()
        })
        .unwrap_or_default();
    tracing::warn!(
        target: "box_script",
        "{at}`{old}` is deprecated: use `{new}` (the old name still works for now and will be removed in a later release)"
    );
}

/// Adds `table[old_key]` as a deprecated alias of `table[new_key]` (which
/// must be set already): it calls the new function after [`deprecated`]
/// (`module.old_key` → `module.new_key` in the message).
pub fn deprecated_alias(
    lua: &Lua,
    table: &Table,
    module: &str,
    old_key: &str,
    new_key: &str,
) -> LuaResult<()> {
    let f: Function = table.get(new_key)?;
    let old = format!("{module}.{old_key}");
    let new = format!("{module}.{new_key}");
    table.set(
        old_key,
        lua.create_function(move |lua, args: mlua::MultiValue| {
            deprecated(lua, &old, &new);
            f.call::<mlua::MultiValue>(args)
        })?,
    )
}

pub(crate) fn host(lua: &Lua) -> LuaResult<Rc<Host>> {
    lua.app_data_ref::<Rc<Host>>()
        .map(|h| h.clone())
        .ok_or_else(|| mlua::Error::runtime("only available while the game runs"))
}

/// Runs `f` with the world scripts are running against. Fails outside a
/// script stage (while a script file is imported) and when the world is
/// already borrowed mutably.
pub fn with_world<R>(lua: &Lua, f: impl FnOnce(&World) -> R) -> LuaResult<R> {
    let host = host(lua)?;
    if !host.live.get() {
        return Err(mlua::Error::runtime("only available while the game runs"));
    }
    let w = host
        .world
        .try_borrow()
        .map_err(|_| mlua::Error::runtime("the world is busy (re-entrant call)"))?;
    Ok(f(&w))
}

/// [`with_world`] with mutable access.
pub fn with_world_mut<R>(lua: &Lua, f: impl FnOnce(&mut World) -> R) -> LuaResult<R> {
    let host = host(lua)?;
    if !host.live.get() {
        return Err(mlua::Error::runtime("only available while the game runs"));
    }
    let mut w = host
        .world
        .try_borrow_mut()
        .map_err(|_| mlua::Error::runtime("the world is busy (re-entrant call)"))?;
    Ok(f(&mut w))
}

/// The script path of the running callback, for log prefixes.
pub(crate) fn current_script(lua: &Lua) -> Option<Rc<str>> {
    lua.app_data_ref::<Rc<Host>>()
        .and_then(|h| h.current_path.borrow().clone())
}

/// Installs the APIs into `lua`'s globals and the entity metatable.
pub(crate) fn install(lua: &Lua, apis: &[Arc<dyn ScriptApi>]) -> LuaResult<()> {
    let globals = lua.globals();
    for api in apis {
        api.install(lua, &globals).map_err(|e| {
            mlua::Error::runtime(format!(
                "script API `{}` failed to install: {e}",
                api.name()
            ))
        })?;
    }
    Ok(())
}

/// Installs only what a script file may use while it is imported (value
/// types, `log`), for reading its `properties` table on an import worker.
pub(crate) fn install_import(lua: &Lua) -> LuaResult<()> {
    core::CoreApi.install(lua, &lua.globals())
}

/// The definitions of every API in `apis`, in order.
pub fn api_definitions(apis: &[Arc<dyn ScriptApi>]) -> String {
    let mut out = String::new();
    for api in apis {
        let d = api.definitions();
        if !d.is_empty() {
            out.push_str(&format!("-- {}\n", api.name()));
            out.push_str(d.trim_end());
            out.push_str("\n\n");
        }
    }
    out
}

/// Converts a list of names given as varargs or one table.
pub(crate) fn names_arg(args: mlua::MultiValue) -> LuaResult<Vec<String>> {
    let mut names = Vec::new();
    for v in args {
        match v {
            Value::String(s) => names.push(s.to_str()?.to_string()),
            Value::Table(t) => {
                for item in t.sequence_values::<String>() {
                    names.push(item?);
                }
            }
            other => {
                return Err(mlua::Error::runtime(format!(
                    "expected component names, got {}",
                    other.type_name()
                )));
            }
        }
    }
    Ok(names)
}
