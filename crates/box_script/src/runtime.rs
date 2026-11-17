//! The script runtime: one Luau VM per world that plays (a Play world, the
//! player), created on the first frame that has a script. An editor's edit
//! world never runs systems, so it never gets one.
//!
//! **Frame.** `FixedUpdate` (after the rigid-body step and the native input
//! systems, before `move_characters`): finish removals, start new
//! instances, `on_fixed_update(self, dt)`.
//! `Update` (after the game's native systems): removals, inspector edits
//! (a slice of the instances per frame), hot reload, starts, trigger
//! callbacks, collision callbacks (`dynamics` feature), event handlers
//! (registry events, script signals and [`ScriptSignals`] from Rust),
//! waiting tasks, `on_update(self, dt)`.
//! Instances run in the order they started; that order follows the scene's
//! entity order, so runs are deterministic.
//!
//! **World access.** While a stage runs, the world is moved into the VM's
//! shared host (a `RefCell`) and moved back afterwards (a swap, no copy and
//! no pointer); API functions borrow it for the duration of one call.
//!
//! **Errors.** A failing callback, task or handler logs the error with the
//! script's file and line (`tracing` target `box_script`, so the editor
//! shows a toast and counts it on the Console tab) and disables that
//! entity's script until its file is reloaded. A file that fails to load
//! keeps its previous version running.
//!
//! **Watchdog.** A stage whose scripts run longer than
//! [`DEFAULT_WATCHDOG`] (an endless loop) is interrupted: the running
//! callback fails with an error naming its file and line, so the editor
//! never hangs in Play.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use box_assets::{AssetEvent, AssetEventKind};
use box_core::asset_ref::AssetRef;
use box_core::assets::{AssetLookup, Assets, Handle};
use box_core::event::{EventCursor, Events};
use box_core::identity::{Name, Uid};
use box_core::world::{Entity, World};
use box_physics::{TriggerEvent, TriggerKind};
use mlua::{Function, Lua, MultiValue, Table, Value};

use crate::api::{self, Host, ScriptApis, ScriptSignals, Step, Wake};
use crate::asset::{ScriptAsset, parse_properties};
use crate::component::{PropertyDecl, Script, ScriptProperty, ScriptValue};
use crate::convert::{
    asset_value, color_table, entity_value, json_to_lua_with, lua_to_json, vector,
};

/// Seconds of game time after which an `on_start` that is still running
/// while its instance has `on_load` data waiting logs a warning.
pub const LOAD_WAIT_WARNING: f64 = 5.0;

/// Resource: entities whose `Script` was added or removed since the
/// runtime last looked (filled by `on_add` / `on_remove` hooks).
#[derive(Default)]
pub struct ScriptQueue {
    pub(crate) added: Vec<Entity>,
    pub(crate) removed: Vec<Entity>,
}

impl ScriptQueue {
    fn is_idle(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty()
    }
}

#[derive(Default)]
struct Callbacks {
    start: Option<Function>,
    update: Option<Function>,
    fixed_update: Option<Function>,
    destroy: Option<Function>,
    trigger_enter: Option<Function>,
    trigger_exit: Option<Function>,
    #[cfg(feature = "dynamics")]
    collision_enter: Option<Function>,
    #[cfg(feature = "dynamics")]
    collision_exit: Option<Function>,
    save: Option<Function>,
    load: Option<Function>,
    reload: Option<Function>,
}

impl Callbacks {
    fn read(t: &Table) -> Self {
        let f = |name: &str| t.raw_get::<Option<Function>>(name).ok().flatten();
        Self {
            start: f("on_start"),
            update: f("on_update"),
            fixed_update: f("on_fixed_update"),
            destroy: f("on_destroy"),
            trigger_enter: f("on_trigger_enter"),
            trigger_exit: f("on_trigger_exit"),
            #[cfg(feature = "dynamics")]
            collision_enter: f("on_collision_enter"),
            #[cfg(feature = "dynamics")]
            collision_exit: f("on_collision_exit"),
            save: f("on_save"),
            load: f("on_load"),
            reload: f("on_reload"),
        }
    }
}

struct Module {
    /// Path relative to `assets/` once loaded, else the reference text.
    path: Rc<str>,
    handle: Option<Handle<ScriptAsset>>,
    /// Version of the asset value the module table came from.
    version: Option<u64>,
    table: Option<Table>,
    /// Instances' metatable: `__index` is the module table, replaced in
    /// place on reload so every instance sees the new functions.
    meta: Table,
    fns: Callbacks,
    decls: Vec<PropertyDecl>,
    /// The version whose load failed (logged once).
    failed_version: Option<u64>,
}

struct Instance {
    id: u64,
    entity: Entity,
    module: usize,
    this: Table,
    source: AssetRef,
    properties: Vec<ScriptProperty>,
    started: bool,
    /// The task running `on_start` while it waits; `on_load` data for this
    /// instance waits until it returns.
    start_task: Option<u64>,
    /// Game time `on_start` began (for the stuck-`on_start` warning).
    started_at: f64,
    /// The stuck-`on_start` warning was logged.
    load_warned: bool,
    /// Why it is disabled (until its file reloads).
    disabled: Option<String>,
}

/// Counts for tests, benchmarks and tools.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScriptStats {
    /// Running script instances.
    pub instances: usize,
    /// Instances disabled by an error.
    pub disabled: usize,
    /// Loaded script files.
    pub modules: usize,
    /// Waiting tasks.
    pub tasks: usize,
    /// Callbacks run in the last stage.
    pub callbacks: u64,
    /// Successful hot reloads so far.
    pub reloads: u64,
    /// Script errors so far.
    pub errors: u64,
    /// Wall time of the last Update stage's scripts (starts, events,
    /// tasks, `on_update`), engine calls they made included.
    pub update_time: Duration,
    /// Wall time of the scripts of the fixed steps that ran before the last
    /// Update stage (`on_fixed_update`, all steps of that frame together).
    pub fixed_time: Duration,
}

/// How long one stage's scripts may run before the watchdog interrupts
/// them.
pub const DEFAULT_WATCHDOG: Duration = Duration::from_secs(5);

/// The watchdog's state: the current stage's deadline and the limit.
#[derive(Default)]
struct Watchdog {
    deadline: Cell<Option<Instant>>,
    limit: Cell<Option<Duration>>,
    ticks: Cell<u32>,
}

/// Resource: the world's Luau VM and its script instances. See the module
/// docs.
pub struct ScriptRuntime {
    lua: Lua,
    host: Rc<Host>,
    watchdog: Rc<Watchdog>,
    modules: Vec<Module>,
    by_key: HashMap<String, usize>,
    instances: Vec<Instance>,
    by_entity: HashMap<Entity, u64>,
    next_instance: u64,
    /// Script entities waiting for their file to load.
    pending: Vec<Entity>,
    /// Round-robin position of the inspector-edit scan.
    scan: usize,
    triggers: EventCursor<TriggerEvent>,
    #[cfg(feature = "dynamics")]
    collisions: EventCursor<box_physics::CollisionEvent>,
    asset_events: EventCursor<AssetEvent>,
    /// Files loaded through `require`: handle and version seen.
    required: HashMap<String, (Handle<ScriptAsset>, Option<u64>)>,
    /// `on_load` data for saved entities that have not started yet.
    pending_loads: HashMap<Uid, serde_json::Value>,
    stats: ScriptStats,
    /// Fixed-step script time since the last Update stage.
    fixed_running: Duration,
}

/// Formats a Luau error as `file:line: message` plus the script frames of
/// its traceback.
pub fn describe_error(e: &mlua::Error) -> String {
    fn walk(e: &mlua::Error, trace: &mut Option<String>) -> String {
        match e {
            mlua::Error::CallbackError { traceback, cause } => {
                if trace.is_none() {
                    *trace = Some(traceback.clone());
                }
                walk(cause, trace)
            }
            mlua::Error::WithContext { context, cause } => {
                format!("{context}: {}", walk(cause, trace))
            }
            mlua::Error::RuntimeError(s) => s.clone(),
            mlua::Error::SyntaxError { message, .. } => message.clone(),
            mlua::Error::MemoryError(s) => format!("out of memory: {s}"),
            mlua::Error::ExternalError(x) => x.to_string(),
            other => other.to_string(),
        }
    }
    let mut trace = None;
    let text = walk(e, &mut trace);
    let (first, rest) = match text.split_once("\nstack traceback:") {
        Some((a, b)) => (a.to_string(), Some(b.to_string())),
        None => (text, None),
    };
    let frames: Vec<String> = rest
        .or(trace)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| l.contains(".luau:"))
        .map(str::to_owned)
        .collect();
    // A message without a location (raised by an engine function) gets the
    // innermost script frame's.
    let located = first
        .split(':')
        .nth(1)
        .is_some_and(|n| n.trim().chars().all(|c| c.is_ascii_digit()) && !n.is_empty());
    let mut out = match (located, frames.first()) {
        (false, Some(frame)) => {
            let at = frame.split(": in ").next().unwrap_or(frame);
            format!("{at}: {first}")
        }
        _ => first,
    };
    if frames.len() > 1 {
        out.push_str("\n  traceback: ");
        out.push_str(&frames.join(" <- "));
    }
    out
}

/// The world, moved into the host for one stage and back on drop (also
/// during a panic).
struct Live<'a> {
    world: &'a mut World,
    host: Rc<Host>,
}

impl<'a> Live<'a> {
    fn new(world: &'a mut World, host: Rc<Host>) -> Self {
        std::mem::swap(world, &mut *host.world.borrow_mut());
        host.live.set(true);
        Self { world, host }
    }
}

impl Drop for Live<'_> {
    fn drop(&mut self) {
        self.host.live.set(false);
        if let Ok(mut w) = self.host.world.try_borrow_mut() {
            std::mem::swap(self.world, &mut *w);
        }
    }
}

impl ScriptRuntime {
    /// A VM with the built-in APIs and `extra` (plugins' [`ScriptApis`]).
    pub fn new(extra: &ScriptApis) -> mlua::Result<Self> {
        let lua = Lua::new();
        let host = Rc::new(Host::new());
        lua.set_app_data(host.clone());
        let mut apis = api::builtin_apis();
        apis.extend(extra.iter().cloned());
        api::install(&lua, &apis)?;
        lua.sandbox(true)?;
        let watchdog = Rc::new(Watchdog::default());
        watchdog.limit.set(Some(DEFAULT_WATCHDOG));
        let dog = watchdog.clone();
        lua.set_interrupt(move |lua| {
            // The clock is read every 1024 interrupts (loop back-edges and
            // calls), so the check costs next to nothing.
            let t = dog.ticks.get().wrapping_add(1);
            dog.ticks.set(t);
            if t % 1024 == 0
                && let Some(deadline) = dog.deadline.get()
                && Instant::now() > deadline
            {
                dog.deadline.set(None);
                let limit = dog.limit.get().unwrap_or_default().as_secs_f32();
                let at = (0..4)
                    .find_map(|level| {
                        lua.inspect_stack(level, |d| {
                            let src = d.source().source.map(|s| s.to_string())?;
                            let line = d.current_line()?;
                            src.strip_prefix('@').map(|p| format!("{p}:{line}: "))
                        })
                        .flatten()
                    })
                    .unwrap_or_default();
                return Err(mlua::Error::runtime(format!(
                    "{at}scripts ran for more than {limit} s in one frame (an endless loop?); interrupted"
                )));
            }
            Ok(mlua::VmState::Continue)
        });
        Ok(Self {
            lua,
            host,
            watchdog,
            modules: Vec::new(),
            by_key: HashMap::new(),
            instances: Vec::new(),
            by_entity: HashMap::new(),
            next_instance: 1,
            pending: Vec::new(),
            scan: 0,
            triggers: EventCursor::default(),
            #[cfg(feature = "dynamics")]
            collisions: EventCursor::default(),
            asset_events: EventCursor::default(),
            required: HashMap::new(),
            pending_loads: HashMap::new(),
            fixed_running: Duration::ZERO,
            stats: ScriptStats::default(),
        })
    }

    /// Sets how long one stage's scripts may run before they are
    /// interrupted (`None`: never, for stepping through a debugger).
    pub fn set_watchdog(&self, limit: Option<Duration>) {
        self.watchdog.limit.set(limit);
    }

    fn arm_watchdog(&self) {
        self.watchdog
            .deadline
            .set(self.watchdog.limit.get().map(|l| Instant::now() + l));
    }

    pub fn stats(&self) -> ScriptStats {
        ScriptStats {
            instances: self.instances.len(),
            disabled: self
                .instances
                .iter()
                .filter(|i| i.disabled.is_some())
                .count(),
            modules: self.modules.iter().filter(|m| m.table.is_some()).count(),
            tasks: self.host.tasks.borrow().len(),
            ..self.stats
        }
    }

    /// The Luau VM (tests and tools; game code goes through scripts).
    pub fn lua(&self) -> &Lua {
        &self.lua
    }

    /// The running instance (`self` table) of `entity`'s script.
    pub fn instance(&self, entity: Entity) -> Option<Table> {
        self.host.instances.borrow().get(&entity).cloned()
    }

    /// Why `entity`'s script is disabled, if it is.
    pub fn disabled_reason(&self, entity: Entity) -> Option<String> {
        let id = self.by_entity.get(&entity)?;
        self.instances
            .iter()
            .find(|i| i.id == *id)
            .and_then(|i| i.disabled.clone())
    }

    fn world(&self) -> std::cell::Ref<'_, World> {
        self.host.world.borrow()
    }

    // ----- stages -----

    fn begin(&mut self) {
        self.stats.callbacks = 0;
        let assets_dir = box_assets::loader(&self.world()).map(|s| s.assets_dir().to_path_buf());
        if assets_dir.is_some() {
            *self.host.assets_dir.borrow_mut() = assets_dir;
        }
        let (added, removed) = {
            let mut w = self.host.world.borrow_mut();
            let q = w.resource_or_default::<ScriptQueue>();
            (std::mem::take(&mut q.added), std::mem::take(&mut q.removed))
        };
        for e in removed {
            self.remove(e);
        }
        for e in added {
            if !self.by_entity.contains_key(&e) && !self.pending.contains(&e) {
                self.pending.push(e);
            }
        }
    }

    fn fixed_stage(&mut self) {
        self.begin();
        self.start_pending();
        let dt = self
            .world()
            .get_resource::<box_core::time::Time>()
            .map_or(1.0 / 60.0, |t| t.delta);
        self.each("on_fixed_update", |fns| fns.fixed_update.as_ref(), dt);
        self.apply_failures();
    }

    fn update_stage(&mut self) {
        self.begin();
        self.scan_edits();
        self.hot_reload();
        self.start_pending();
        self.dispatch_triggers();
        #[cfg(feature = "dynamics")]
        self.dispatch_collisions();
        self.dispatch_events();
        self.resume_tasks();
        self.flush_loads();
        let dt = self
            .world()
            .get_resource::<box_core::time::Time>()
            .map_or(1.0 / 60.0, |t| t.delta);
        self.each("on_update", |fns| fns.update.as_ref(), dt);
        self.apply_failures();
    }

    /// Calls a per-frame callback on every running instance.
    fn each(&mut self, name: &str, pick: impl Fn(&Callbacks) -> Option<&Function>, dt: f32) {
        let mut failed = Vec::new();
        for i in 0..self.instances.len() {
            let inst = &self.instances[i];
            if !inst.started || inst.disabled.is_some() {
                continue;
            }
            let module = &self.modules[inst.module];
            let Some(f) = pick(&module.fns) else { continue };
            if !self.host.world.borrow().contains(inst.entity) {
                continue;
            }
            self.host.current.set(Some(inst.id));
            *self.host.current_path.borrow_mut() = Some(module.path.clone());
            self.stats.callbacks += 1;
            if let Err(e) = f.call::<()>((&inst.this, dt)) {
                failed.push((i, e));
            }
        }
        self.host.current.set(None);
        for (i, e) in failed {
            self.fail(i, name, &e);
        }
    }

    /// Runs `f(self, args...)` for instance index `i` with error handling;
    /// returns its results.
    fn call(&mut self, i: usize, name: &str, f: &Function, args: MultiValue) -> Option<MultiValue> {
        let inst = &self.instances[i];
        let mut full = MultiValue::with_capacity(args.len() + 1);
        full.push_back(Value::Table(inst.this.clone()));
        full.extend(args);
        self.host.current.set(Some(inst.id));
        *self.host.current_path.borrow_mut() = Some(self.modules[inst.module].path.clone());
        self.stats.callbacks += 1;
        let result = f.call::<MultiValue>(full);
        self.host.current.set(None);
        match result {
            Ok(v) => Some(v),
            Err(e) => {
                self.fail(i, name, &e);
                None
            }
        }
    }

    fn entity_label(&self, e: Entity) -> String {
        self.world()
            .get::<Name>(e)
            .map(|n| format!("\"{}\"", n.0))
            .unwrap_or_else(|| format!("entity {}", e.id()))
    }

    fn fail(&mut self, i: usize, callback: &str, e: &mlua::Error) {
        let msg = describe_error(e);
        self.disable(i, callback, msg);
    }

    fn disable(&mut self, i: usize, callback: &str, msg: String) {
        let entity = self.instances[i].entity;
        let label = self.entity_label(entity);
        let path = self.modules[self.instances[i].module].path.clone();
        tracing::error!(
            target: "box_script",
            "{msg}\n  in {callback} of {label} ({path}); this entity's script is disabled until the file is reloaded"
        );
        self.stats.errors += 1;
        let id = self.instances[i].id;
        self.instances[i].disabled = Some(msg);
        self.cancel_owned(id);
    }

    /// Disables the instances whose tasks or handlers failed.
    fn apply_failures(&mut self) {
        let failures = std::mem::take(&mut *self.host.failures.borrow_mut());
        for (id, msg) in failures {
            if let Some(i) = self.instances.iter().position(|x| x.id == id)
                && self.instances[i].disabled.is_none()
            {
                self.disable(i, "a task or event handler", msg);
            }
        }
    }

    fn cancel_owned(&self, id: u64) {
        self.host.tasks.borrow_mut().retain(|t| t.owner != Some(id));
        self.host
            .subscriptions
            .borrow_mut()
            .retain(|s| s.owner != Some(id));
    }

    // ----- instances -----

    fn remove(&mut self, e: Entity) {
        self.pending.retain(|p| *p != e);
        let Some(id) = self.by_entity.remove(&e) else {
            return;
        };
        let Some(i) = self.instances.iter().position(|x| x.id == id) else {
            return;
        };
        if self.instances[i].started && self.instances[i].disabled.is_none() {
            let module = self.instances[i].module;
            if let Some(f) = self.modules[module].fns.destroy.clone() {
                self.call(i, "on_destroy", &f, MultiValue::new());
            }
        }
        let inst = self.instances.remove(i);
        self.cancel_owned(inst.id);
        let mut map = self.host.instances.borrow_mut();
        if map.get(&e).is_some_and(|t| t == &inst.this) {
            map.remove(&e);
        }
        drop(map);
        self.host.script_list.borrow_mut().retain(|(x, _)| *x != e);
    }

    /// The module for `source`, its file requested and (re)loaded when its
    /// asset value is new.
    fn module_for(&mut self, source: &AssetRef) -> Option<usize> {
        let key = source.to_string();
        let idx = match self.by_key.get(&key) {
            Some(i) => *i,
            None => {
                let meta = self.lua.create_table().ok()?;
                self.modules.push(Module {
                    path: Rc::from(source.path.clone().unwrap_or_else(|| key.clone())),
                    handle: None,
                    version: None,
                    table: None,
                    meta,
                    fns: Callbacks::default(),
                    decls: Vec::new(),
                    failed_version: None,
                });
                self.by_key.insert(key, self.modules.len() - 1);
                self.modules.len() - 1
            }
        };
        if self.modules[idx].handle.is_none() {
            let handle = {
                let w = self.world();
                w.get_resource::<AssetLookup>()
                    .and_then(|l| l.get::<ScriptAsset>(source))
                    .or_else(|| {
                        box_assets::loader(&w).and_then(|s| s.load::<ScriptAsset>(source).ok())
                    })
            };
            self.modules[idx].handle = handle;
        }
        self.refresh_module(idx);
        Some(idx)
    }

    /// Loads the module's asset value when its version changed. Returns
    /// true when a new version was loaded.
    fn refresh_module(&mut self, idx: usize) -> bool {
        let Some(handle) = self.modules[idx].handle else {
            return false;
        };
        let (asset, version) = {
            let w = self.world();
            let Some(store) = w.get_resource::<Assets<ScriptAsset>>() else {
                return false;
            };
            match (store.get(handle), store.version(handle)) {
                (Some(a), Some(v)) => (a.clone(), v),
                _ => return false,
            }
        };
        let m = &self.modules[idx];
        if m.version == Some(version) || m.failed_version == Some(version) {
            return false;
        }
        self.load_module(idx, &asset, version)
    }

    fn load_module(&mut self, idx: usize, asset: &ScriptAsset, version: u64) -> bool {
        let path: Rc<str> = Rc::from(asset.path.as_str());
        self.modules[idx].path = path.clone();
        self.host
            .sources
            .borrow_mut()
            .insert(asset.path.clone(), asset.source.clone());
        // Required modules load fresh with this version.
        self.host.modules.borrow_mut().remove(&asset.path);
        *self.host.current_path.borrow_mut() = Some(path.clone());
        let result = (|| -> mlua::Result<(Table, Vec<PropertyDecl>)> {
            let env = api::module_env(&self.lua)?;
            let value: Value = self
                .lua
                .load(&asset.source)
                .set_name(format!("@{}", asset.path))
                .set_environment(env)
                .eval()?;
            let Value::Table(t) = value else {
                return Err(mlua::Error::runtime(format!(
                    "{}: the script must return its module table (got {}); end the file with `return MyScript`",
                    asset.path,
                    value.type_name()
                )));
            };
            let decls = parse_properties(&self.lua, &asset.path, &asset.source, &t)?;
            Ok((t, decls))
        })();
        match result {
            Ok((table, decls)) => {
                let m = &mut self.modules[idx];
                let _ = m.meta.raw_set("__index", table.clone());
                m.fns = Callbacks::read(&table);
                m.table = Some(table);
                m.decls = decls;
                m.version = Some(version);
                m.failed_version = None;
                true
            }
            Err(e) => {
                let had = self.modules[idx].table.is_some();
                self.modules[idx].failed_version = Some(version);
                self.stats.errors += 1;
                let tail = if had {
                    "; the previous version keeps running"
                } else {
                    "; entities using it do not run it until it loads"
                };
                tracing::error!(target: "box_script", "{}{tail}", describe_error(&e));
                false
            }
        }
    }

    /// Starts the pending entities whose script file has loaded.
    fn start_pending(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.pending);
        let mut waiting = Vec::new();
        for e in pending {
            let script = self.world().get::<Script>(e).map(|s| (*s).clone());
            let Some(script) = script else { continue };
            if script.source.is_none() {
                continue;
            }
            match self.module_for(&script.source) {
                Some(m) if self.modules[m].table.is_some() => self.start(e, m, script),
                _ => waiting.push(e),
            }
        }
        self.pending.extend(waiting);
    }

    fn props_table(
        &self,
        decls: &[PropertyDecl],
        stored: &[ScriptProperty],
    ) -> mlua::Result<Table> {
        let t = self.lua.create_table_with_capacity(0, decls.len())?;
        let w = self.world();
        for d in decls {
            let v = d.effective(stored.iter().find(|p| p.name == d.name).map(|p| &p.value));
            let lv = match v {
                ScriptValue::Number(n) => Value::Number(n),
                ScriptValue::Integer(i) => Value::Number(i as f64),
                ScriptValue::Boolean(b) => Value::Boolean(b),
                ScriptValue::String(s) => Value::String(self.lua.create_string(&s)?),
                ScriptValue::Vector(v) => Value::Vector(vector(v)),
                ScriptValue::Color(c) => Value::Table(color_table(&self.lua, c)?),
                ScriptValue::Entity(u) => u
                    .and_then(|u| w.entity_by_uid(u))
                    .map_or(Value::Nil, entity_value),
                ScriptValue::Asset(r) => asset_value(&self.lua, &r)?,
            };
            t.raw_set(d.name.as_str(), lv)?;
        }
        Ok(t)
    }

    fn start(&mut self, e: Entity, module: usize, script: Script) {
        let made = (|| -> mlua::Result<Table> {
            let this = self.lua.create_table()?;
            this.raw_set("entity", entity_value(e))?;
            this.raw_set(
                "props",
                self.props_table(&self.modules[module].decls, &script.properties)?,
            )?;
            this.set_metatable(Some(self.modules[module].meta.clone()))?;
            Ok(this)
        })();
        let this = match made {
            Ok(t) => t,
            Err(err) => {
                tracing::error!(target: "box_script", "cannot start a script: {err}");
                return;
            }
        };
        let id = self.next_instance;
        self.next_instance += 1;
        self.by_entity.insert(e, id);
        self.host.instances.borrow_mut().insert(e, this.clone());
        self.host
            .script_list
            .borrow_mut()
            .push((e, self.modules[module].path.clone()));
        self.instances.push(Instance {
            id,
            entity: e,
            module,
            this,
            source: script.source,
            properties: script.properties,
            started: false,
            start_task: None,
            started_at: 0.0,
            load_warned: false,
            disabled: None,
        });
        let i = self.instances.len() - 1;
        self.run_start(i);
    }

    /// `on_start` as a task (it may wait), then, once it has returned, any
    /// `on_load` data ([`Self::flush_loads`] when it waited).
    fn run_start(&mut self, i: usize) {
        self.instances[i].started = true;
        self.instances[i].start_task = None;
        self.instances[i].started_at = api::clock(&self.host).0;
        let module = self.instances[i].module;
        if let Some(f) = self.modules[module].fns.start.clone() {
            let id = self.instances[i].id;
            self.host.current.set(Some(id));
            *self.host.current_path.borrow_mut() = Some(self.modules[module].path.clone());
            self.stats.callbacks += 1;
            let this = self.instances[i].this.clone();
            let r = api::spawn(
                &self.lua,
                &self.host,
                f,
                MultiValue::from_iter([Value::Table(this)]),
                Some(id),
            );
            self.host.current.set(None);
            let task = match r {
                Ok(task) => task,
                Err(e) => {
                    self.fail(i, "on_start", &e);
                    return;
                }
            };
            // A failure before its first wait.
            let failed = {
                let mut f = self.host.failures.borrow_mut();
                f.iter().position(|(o, _)| *o == id).map(|p| f.remove(p).1)
            };
            if let Some(msg) = failed {
                self.disable(i, "on_start", msg);
                return;
            }
            if self.host.tasks.borrow().iter().any(|t| t.id == task) {
                // Waiting: on_load runs when on_start returns.
                self.instances[i].start_task = Some(task);
                return;
            }
        }
        let uid = self.world().uid(self.instances[i].entity);
        if let Some(data) = uid.and_then(|u| self.pending_loads.remove(&u)) {
            self.run_load(i, &data);
        }
    }

    fn run_load(&mut self, i: usize, data: &serde_json::Value) {
        let module = self.instances[i].module;
        let Some(f) = self.modules[module].fns.load.clone() else {
            return;
        };
        let value = {
            let w = self.world();
            json_to_lua_with(&self.lua, data, &|uid| {
                Uid::parse(uid)
                    .and_then(|u| w.entity_by_uid(u))
                    .map_or(Value::Nil, entity_value)
            })
        };
        let value = match value {
            Ok(v) => v,
            Err(e) => {
                self.fail(i, "on_load", &e);
                return;
            }
        };
        self.call(i, "on_load", &f, MultiValue::from_iter([value]));
    }

    // ----- inspector edits and hot reload -----

    /// Picks up `Script` edits made while playing (inspector, control
    /// socket): a slice of the instances per frame, so every instance is
    /// checked within 16 frames at a cost that does not grow per frame.
    fn scan_edits(&mut self) {
        let n = self.instances.len();
        if n == 0 {
            return;
        }
        let slice = n.div_ceil(16);
        let mut restart = Vec::new();
        for _ in 0..slice {
            self.scan %= n;
            let i = self.scan;
            self.scan += 1;
            let e = self.instances[i].entity;
            let current = {
                let w = self.world();
                w.get::<Script>(e).map(|s| (*s).clone())
            };
            let Some(current) = current else { continue };
            if current.source != self.instances[i].source {
                restart.push(e);
            } else if current.properties != self.instances[i].properties {
                let module = self.instances[i].module;
                if let Ok(props) =
                    self.props_table(&self.modules[module].decls, &current.properties)
                {
                    let _ = self.instances[i].this.raw_set("props", props);
                }
                self.instances[i].properties = current.properties;
            }
        }
        for e in restart {
            self.remove(e);
            self.pending.push(e);
        }
    }

    fn hot_reload(&mut self) {
        // Failed re-imports (syntax errors) of running scripts.
        let failed: Vec<(String, String)> = {
            let host = self.host.clone();
            let w = host.world.borrow();
            match w.get_resource::<Events<AssetEvent>>() {
                Some(events) => self
                    .asset_events
                    .read(events)
                    .filter(|e| {
                        e.kind == AssetEventKind::Failed
                            && e.type_id == std::any::TypeId::of::<ScriptAsset>()
                    })
                    .map(|e| (e.path.clone(), e.message.clone().unwrap_or_default()))
                    .collect(),
                None => Vec::new(),
            }
        };
        for (path, message) in failed {
            let running = self
                .modules
                .iter()
                .any(|m| &*m.path == path.as_str() && m.table.is_some());
            let tail = if running {
                "the previous version keeps running"
            } else {
                "entities using it do not run it until it loads"
            };
            self.stats.errors += 1;
            tracing::error!(target: "box_script", "{message}; {tail}");
        }
        // Changed script files.
        let mut reloaded = Vec::new();
        for idx in 0..self.modules.len() {
            let had = self.modules[idx].table.is_some();
            if self.refresh_module(idx) && had {
                reloaded.push(idx);
            }
        }
        // Changed files that scripts `require`.
        let deps = self.host.deps.borrow().clone();
        for paths in deps.values() {
            for p in paths {
                if !self.required.contains_key(p) {
                    let handle = {
                        let w = self.world();
                        let r = AssetRef::path(p);
                        w.get_resource::<AssetLookup>()
                            .and_then(|l| l.get::<ScriptAsset>(&r))
                            .or_else(|| {
                                box_assets::loader(&w).and_then(|s| s.load::<ScriptAsset>(&r).ok())
                            })
                    };
                    if let Some(h) = handle {
                        let v = self
                            .world()
                            .get_resource::<Assets<ScriptAsset>>()
                            .and_then(|s| s.version(h));
                        self.required.insert(p.clone(), (h, v));
                    }
                }
            }
        }
        let mut changed_libs = Vec::new();
        for (path, (h, seen)) in self.required.iter_mut() {
            let (value, now) = {
                let w = self.host.world.borrow();
                let store = w.get_resource::<Assets<ScriptAsset>>();
                (
                    store.and_then(|s| s.get(*h).cloned()),
                    store.and_then(|s| s.version(*h)),
                )
            };
            if now.is_some() && now != *seen {
                if seen.is_some()
                    && let Some(v) = value
                {
                    changed_libs.push((path.clone(), v.source));
                }
                *seen = now;
            }
        }
        for (lib, source) in changed_libs {
            self.host.sources.borrow_mut().insert(lib.clone(), source);
            self.host.modules.borrow_mut().remove(&lib);
            // Reload every module that requires it.
            for (idx, m) in self.modules.iter().enumerate() {
                let uses = deps.get(&*m.path).is_some_and(|d| d.contains(&lib));
                if uses && m.table.is_some() && !reloaded.contains(&idx) {
                    reloaded.push(idx);
                }
            }
            for idx in &reloaded {
                let asset = self.modules[*idx].handle.and_then(|h| {
                    self.world()
                        .get_resource::<Assets<ScriptAsset>>()
                        .and_then(|s| s.get(h).cloned().zip(s.version(h)))
                });
                if let Some((asset, version)) = asset {
                    self.modules[*idx].version = None;
                    self.load_module(*idx, &asset, version);
                }
            }
        }
        for idx in reloaded {
            self.after_reload(idx);
        }
    }

    /// Instances of a reloaded module: fresh properties, disabled ones
    /// enabled again, `on_reload(self)` where the module has it.
    fn after_reload(&mut self, idx: usize) {
        self.stats.reloads += 1;
        let mut count = 0;
        for i in 0..self.instances.len() {
            if self.instances[i].module != idx {
                continue;
            }
            count += 1;
            let stored = self.instances[i].properties.clone();
            if let Ok(props) = self.props_table(&self.modules[idx].decls, &stored) {
                let _ = self.instances[i].this.raw_set("props", props);
            }
            self.instances[i].disabled = None;
            if !self.instances[i].started {
                self.run_start(i);
                continue;
            }
            if let Some(f) = self.modules[idx].fns.reload.clone() {
                let id = self.instances[i].id;
                self.cancel_owned(id);
                self.call(i, "on_reload", &f, MultiValue::new());
            }
        }
        tracing::info!(
            target: "box_script",
            "reloaded {} ({count} running)",
            self.modules[idx].path
        );
    }

    // ----- events, triggers, tasks -----

    fn instance_index(&self, e: Entity) -> Option<usize> {
        let id = self.by_entity.get(&e)?;
        self.instances.iter().position(|x| x.id == *id)
    }

    fn dispatch_triggers(&mut self) {
        let events: Vec<TriggerEvent> = {
            let host = self.host.clone();
            let w = host.world.borrow();
            match w.get_resource::<Events<TriggerEvent>>() {
                Some(ev) => self.triggers.read(ev).cloned().collect(),
                None => return,
            }
        };
        for ev in events {
            for (me, other) in [(ev.trigger, ev.other), (ev.other, ev.trigger)] {
                let Some(i) = self.instance_index(me) else {
                    continue;
                };
                if !self.instances[i].started || self.instances[i].disabled.is_some() {
                    continue;
                }
                let module = self.instances[i].module;
                let (f, name) = match ev.kind {
                    TriggerKind::Enter => (
                        self.modules[module].fns.trigger_enter.clone(),
                        "on_trigger_enter",
                    ),
                    TriggerKind::Exit => (
                        self.modules[module].fns.trigger_exit.clone(),
                        "on_trigger_exit",
                    ),
                };
                if let Some(f) = f {
                    self.call(i, name, &f, MultiValue::from_iter([entity_value(other)]));
                }
            }
        }
    }

    /// `on_collision_enter(self, other, impulse)` and
    /// `on_collision_exit(self, other)` for both entities of each
    /// `CollisionEvent` since the last frame.
    #[cfg(feature = "dynamics")]
    fn dispatch_collisions(&mut self) {
        use box_physics::{CollisionEvent, CollisionKind};
        let events: Vec<CollisionEvent> = {
            let host = self.host.clone();
            let w = host.world.borrow();
            match w.get_resource::<Events<CollisionEvent>>() {
                Some(ev) => self.collisions.read(ev).copied().collect(),
                None => return,
            }
        };
        for ev in events {
            for (me, other) in [(ev.a, ev.b), (ev.b, ev.a)] {
                let Some(i) = self.instance_index(me) else {
                    continue;
                };
                if !self.instances[i].started || self.instances[i].disabled.is_some() {
                    continue;
                }
                let module = self.instances[i].module;
                let (f, name, args) = match ev.kind {
                    CollisionKind::Started => (
                        self.modules[module].fns.collision_enter.clone(),
                        "on_collision_enter",
                        MultiValue::from_iter([
                            entity_value(other),
                            Value::Number(ev.impulse as f64),
                        ]),
                    ),
                    CollisionKind::Stopped => (
                        self.modules[module].fns.collision_exit.clone(),
                        "on_collision_exit",
                        MultiValue::from_iter([entity_value(other)]),
                    ),
                };
                if let Some(f) = f {
                    self.call(i, name, &f, args);
                }
            }
        }
    }

    fn dispatch_events(&mut self) {
        let names: Vec<String> = {
            let subs = self.host.subscriptions.borrow();
            let mut n: Vec<String> = subs.iter().map(|s| s.name.clone()).collect();
            n.sort();
            n.dedup();
            n
        };
        let mut batches: Vec<(String, Vec<Value>)> = Vec::new();
        {
            let w = self.world();
            let registry = w.registry().clone();
            let mut cursors = self.host.event_cursors.borrow_mut();
            for name in &names {
                let Some(ty) = registry.event(name) else {
                    continue;
                };
                let cursor = cursors.entry(name.clone()).or_insert_with(|| {
                    let mut c = u64::MAX;
                    ty.read(&w, &mut c);
                    c
                });
                let events = ty.read(&w, cursor);
                if events.is_empty() {
                    continue;
                }
                let mut values = Vec::with_capacity(events.len());
                for ev in events {
                    match crate::convert::reflect_to_lua(
                        &self.lua,
                        &registry,
                        ev.as_partial_reflect(),
                    ) {
                        Ok(v) => values.push(v),
                        Err(e) => tracing::error!(target: "box_script", "event {name}: {e}"),
                    }
                }
                batches.push((name.clone(), values));
            }
        }
        // Signals from Rust ([`ScriptSignals`]), then those scripts sent.
        let from_rust = {
            let mut w = self.host.world.borrow_mut();
            w.get_resource_mut::<ScriptSignals>()
                .map(|s| std::mem::take(&mut s.pending))
                .unwrap_or_default()
        };
        for (name, json) in from_rust {
            let value = {
                let w = self.world();
                json_to_lua_with(&self.lua, &json, &|uid| {
                    Uid::parse(uid)
                        .and_then(|u| w.entity_by_uid(u))
                        .map_or(Value::Nil, entity_value)
                })
            };
            match value {
                Ok(v) => batches.push((name, vec![v])),
                Err(e) => tracing::error!(target: "box_script", "signal {name}: {e}"),
            }
        }
        let signals = std::mem::take(&mut *self.host.signals.borrow_mut());
        for (name, payload) in signals {
            batches.push((name, vec![payload]));
        }
        for (name, values) in batches {
            let handlers: Vec<(Function, Option<u64>, u64)> = self
                .host
                .subscriptions
                .borrow()
                .iter()
                .filter(|s| s.name == name)
                .map(|s| (s.handler.clone(), s.owner, s.id))
                .collect();
            for value in values {
                for (f, owner, sub) in &handlers {
                    if !self
                        .host
                        .subscriptions
                        .borrow()
                        .iter()
                        .any(|s| s.id == *sub)
                    {
                        continue;
                    }
                    self.host.current.set(*owner);
                    if let Some(o) = owner
                        && let Some(i) = self.instances.iter().position(|x| x.id == *o)
                    {
                        if self.instances[i].disabled.is_some() {
                            continue;
                        }
                        *self.host.current_path.borrow_mut() =
                            Some(self.modules[self.instances[i].module].path.clone());
                    }
                    self.stats.callbacks += 1;
                    let r = f.call::<()>(value.clone());
                    self.host.current.set(None);
                    if let Err(e) = r {
                        let msg = format!("{} (handler of `{name}`)", describe_error(&e));
                        match owner {
                            Some(o) => self.host.failures.borrow_mut().push((*o, msg)),
                            None => {
                                self.stats.errors += 1;
                                tracing::error!(target: "box_script", "{msg}");
                            }
                        }
                    }
                }
            }
        }
        self.apply_failures();
    }

    fn resume_tasks(&mut self) {
        let tasks = std::mem::take(&mut *self.host.tasks.borrow_mut());
        if tasks.is_empty() {
            return;
        }
        let (now, frame) = api::clock(&self.host);
        let mut keep = Vec::with_capacity(tasks.len());
        for mut task in tasks {
            // Owners disabled or removed meanwhile.
            if let Some(o) = task.owner
                && !self
                    .instances
                    .iter()
                    .any(|x| x.id == o && x.disabled.is_none())
            {
                continue;
            }
            let args = match &task.wake {
                Wake::At(t) if now + 1e-9 >= *t => {
                    MultiValue::from_iter([Value::Number(now - task.since)])
                }
                Wake::Frame(f) if frame >= *f => MultiValue::new(),
                Wake::Next => MultiValue::new(),
                Wake::Until(cond, deadline) => {
                    self.host.current.set(task.owner);
                    let met = cond.call::<Value>(());
                    self.host.current.set(None);
                    match met {
                        Ok(v) if !matches!(v, Value::Nil | Value::Boolean(false)) => {
                            MultiValue::from_iter([Value::Boolean(true)])
                        }
                        Ok(_) if deadline.is_some_and(|d| now >= d) => {
                            MultiValue::from_iter([Value::Boolean(false)])
                        }
                        Ok(_) => {
                            keep.push(task);
                            continue;
                        }
                        Err(e) => {
                            self.task_failed(task.owner, &e);
                            continue;
                        }
                    }
                }
                _ => {
                    keep.push(task);
                    continue;
                }
            };
            self.host.current.set(task.owner);
            if let Some(o) = task.owner
                && let Some(i) = self.instances.iter().position(|x| x.id == o)
            {
                *self.host.current_path.borrow_mut() =
                    Some(self.modules[self.instances[i].module].path.clone());
            }
            self.stats.callbacks += 1;
            let step = api::step(&task.thread, args, now, frame);
            self.host.current.set(None);
            match step {
                Step::Done => {}
                Step::Waiting(wake) => {
                    task.wake = wake;
                    task.since = now;
                    keep.push(task);
                }
                Step::Failed(e) => self.task_failed(task.owner, &e),
            }
        }
        // Tasks spawned while resuming were pushed meanwhile.
        let mut tasks = self.host.tasks.borrow_mut();
        let spawned = std::mem::take(&mut *tasks);
        *tasks = keep;
        tasks.extend(spawned);
        drop(tasks);
        self.apply_failures();
    }

    /// Runs the `on_load` data of instances whose `on_start` returned since
    /// (or whose start task was cancelled), and warns once about an
    /// `on_start` that has kept `on_load` waiting for
    /// [`LOAD_WAIT_WARNING`] seconds of game time. Nothing to do (one map
    /// check) while no load is pending.
    fn flush_loads(&mut self) {
        if self.pending_loads.is_empty() {
            return;
        }
        let now = api::clock(&self.host).0;
        let uids: Vec<Uid> = self.pending_loads.keys().copied().collect();
        for uid in uids {
            let e = self.world().entity_by_uid(uid);
            let Some(i) = e.and_then(|e| self.instance_index(e)) else {
                continue;
            };
            let inst = &self.instances[i];
            if !inst.started || inst.disabled.is_some() {
                continue;
            }
            if let Some(task) = inst.start_task {
                if self.host.tasks.borrow().iter().any(|t| t.id == task) {
                    if !inst.load_warned && now - inst.started_at >= LOAD_WAIT_WARNING {
                        let path = self.modules[inst.module].path.clone();
                        let label = self.entity_label(inst.entity);
                        tracing::warn!(
                            target: "box_script",
                            "{path}: on_load of {label} is waiting for on_start to return (still running after {LOAD_WAIT_WARNING} s); on_load runs after on_start finishes, so move endless loops in on_start into task.spawn"
                        );
                        self.instances[i].load_warned = true;
                    }
                    continue;
                }
                self.instances[i].start_task = None;
            }
            if let Some(data) = self.pending_loads.remove(&uid) {
                self.run_load(i, &data);
            }
        }
        self.apply_failures();
    }

    fn task_failed(&mut self, owner: Option<u64>, e: &mlua::Error) {
        let msg = describe_error(e);
        match owner {
            Some(o) => self.host.failures.borrow_mut().push((o, msg)),
            None => {
                self.stats.errors += 1;
                tracing::error!(target: "box_script", "task failed: {msg}");
            }
        }
    }

    // ----- saved state -----

    fn save(&mut self) -> serde_json::Value {
        let mut out = serde_json::Map::new();
        // Runtime-only entities are not in the save: neither is their state.
        let transient = box_core::visibility::transient_entities(&self.world());
        for i in 0..self.instances.len() {
            if !self.instances[i].started || self.instances[i].disabled.is_some() {
                continue;
            }
            let e = self.instances[i].entity;
            if transient.contains(&e) {
                continue;
            }
            let Some(uid) = self.world().uid(e) else {
                continue;
            };
            let module = self.instances[i].module;
            let Some(f) = self.modules[module].fns.save.clone() else {
                continue;
            };
            let Some(result) = self.call(i, "on_save", &f, MultiValue::new()) else {
                continue;
            };
            let value = result.into_iter().next().unwrap_or(Value::Nil);
            let w = self.host.world.borrow();
            match lua_to_json(&value, &|e| w.uid(e).map(|u| u.to_string())) {
                Ok(json) => {
                    out.insert(uid.to_string(), json);
                }
                Err(err) => {
                    drop(w);
                    let path = self.modules[module].path.clone();
                    self.disable(
                        i,
                        "on_save",
                        format!("{path}: on_save must return plain data (numbers, strings, booleans, tables, vectors, entities): {err}"),
                    );
                }
            }
        }
        serde_json::Value::Object(out)
    }

    fn load(&mut self, state: &serde_json::Value) -> usize {
        let Some(map) = state.as_object() else {
            return 0;
        };
        let mut applied = 0;
        for (uid, data) in map {
            let Some(uid) = Uid::parse(uid) else { continue };
            let e = self.world().entity_by_uid(uid);
            match e.and_then(|e| self.instance_index(e)) {
                Some(i) if self.instances[i].started && self.instances[i].start_task.is_none() => {
                    self.run_load(i, data);
                    applied += 1;
                }
                _ => {
                    self.pending_loads.insert(uid, data.clone());
                }
            }
        }
        self.apply_failures();
        applied
    }
}

// ----- systems -----

fn ensure_runtime(world: &mut World) -> bool {
    if world.contains_resource::<ScriptRuntime>() {
        return true;
    }
    if world
        .get_resource::<ScriptQueue>()
        .is_none_or(ScriptQueue::is_idle)
    {
        return false;
    }
    let apis = world
        .get_resource::<ScriptApis>()
        .cloned()
        .unwrap_or_default();
    match ScriptRuntime::new(&apis) {
        Ok(rt) => {
            world.insert_resource(rt);
            true
        }
        Err(e) => {
            tracing::error!(target: "box_script", "the script VM could not start: {e}");
            false
        }
    }
}

fn with_runtime(world: &mut World, f: impl FnOnce(&mut ScriptRuntime)) {
    if !ensure_runtime(world) {
        return;
    }
    let Some(mut rt) = world.remove_resource::<ScriptRuntime>() else {
        return;
    };
    {
        let _live = Live::new(world, rt.host.clone());
        rt.arm_watchdog();
        f(&mut rt);
        rt.watchdog.deadline.set(None);
    }
    world.insert_resource(rt);
}

/// FixedUpdate system: `on_fixed_update` (see the module docs).
pub fn script_fixed_update(world: &mut World) {
    profiling::scope!("scripts fixed");
    with_runtime(world, |rt| {
        let start = Instant::now();
        rt.fixed_stage();
        rt.fixed_running += start.elapsed();
    });
}

/// Update system: starts, hot reload, events, tasks, `on_update`.
pub fn script_update(world: &mut World) {
    profiling::scope!("scripts update");
    with_runtime(world, |rt| {
        let start = Instant::now();
        rt.update_stage();
        rt.stats.update_time = start.elapsed();
        rt.stats.fixed_time = std::mem::take(&mut rt.fixed_running);
    });
}

/// Every running script's `on_save(self)` result by entity uid, as JSON
/// (`{"<uid>": <table>}`): the script half of a saved game. Properties are
/// not included; they are scene data (the `Script` component). Values are
/// plain data: vectors as `{"$vector": [x, y, z]}`, rotations as
/// `{"$quat": [x, y, z, w]}`, entities as `{"$entity": "<uid>"}`.
pub fn save_script_state(world: &mut World) -> serde_json::Value {
    let mut out = serde_json::Value::Object(Default::default());
    with_runtime(world, |rt| out = rt.save());
    out
}

/// Hands each entity's data from [`save_script_state`] to its script's
/// `on_load(self, data)`: at once for running instances whose `on_start`
/// has returned, else right after it returns (after all its waits).
/// Returns how many ran at once.
pub fn load_script_state(world: &mut World, state: &serde_json::Value) -> usize {
    if !world.contains_resource::<ScriptRuntime>() {
        // No VM yet: keep the data for the instances when they start.
        world.resource_or_default::<ScriptQueue>();
        let apis = world
            .get_resource::<ScriptApis>()
            .cloned()
            .unwrap_or_default();
        if let Ok(rt) = ScriptRuntime::new(&apis) {
            world.insert_resource(rt);
        }
    }
    let mut n = 0;
    with_runtime(world, |rt| n = rt.load(state));
    n
}
