//! `events`: registry events (Rust `Events<T>` queues) and script-only
//! signals.
//!
//! - `events.send(name, payload)`: a registered event type (`PlaySound`) is
//!   built from its default patched with `payload` and sent on its queue,
//!   where Rust systems and script handlers read it. Any other name is a
//!   script signal.
//! - `events.on(name, handler)` returns an id for `events.off(id)`. Handlers
//!   run in the scripts' Update stage, before `on_update`: events of
//!   registered types sent since the last dispatch (copies), and signals
//!   sent since then (so a signal sent during `on_update` arrives next
//!   frame). A handler added by a script instance is removed with it.

use mlua::{Function, Lua, Result as LuaResult, Table, Value};

use super::{ScriptApi, host, with_world, with_world_mut};
use crate::convert::apply_lua;

pub(crate) struct EventsApi;

/// Resource: signals Rust code sends to script handlers
/// (`events.on(name, handler)`), each with a JSON payload (entities as
/// `{"$entity": uid}`, vectors as `{"$vector": [x, y, z]}`). Delivered in
/// the scripts' next Update stage, with the signals scripts send; queued
/// only while the world runs scripts. Bindings use it to turn Rust events
/// into script events (box_ui's `ui.on`, box_engine's `save.on`).
#[derive(Debug, Default)]
pub struct ScriptSignals {
    pub(crate) pending: Vec<(String, serde_json::Value)>,
}

impl ScriptSignals {
    /// Queues `name` with `payload` for the script handlers of `name`.
    pub fn send(&mut self, name: impl Into<String>, payload: serde_json::Value) {
        self.pending.push((name.into(), payload));
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

/// `events.on(name, handler)` from Rust: subscribes `handler` to the
/// registered event or signal `name`, owned by the script instance whose
/// callback is running (removed with it). Returns the id for
/// [`unsubscribe`]. For bindings that offer their own `on` (`ui.on`).
pub fn subscribe(lua: &Lua, name: &str, handler: Function) -> LuaResult<u64> {
    let host = host(lua)?;
    // A registered event type is read from now on.
    if !host.event_cursors.borrow().contains_key(name) {
        let start = with_world(lua, |w| {
            w.registry().event(name).map(|ty| {
                let mut cursor = u64::MAX;
                ty.read(w, &mut cursor);
                cursor
            })
        })?;
        if let Some(c) = start {
            host.event_cursors.borrow_mut().insert(name.to_string(), c);
        }
    }
    let id = host.next_subscription.get();
    host.next_subscription.set(id + 1);
    host.subscriptions.borrow_mut().push(Subscription {
        id,
        name: name.to_string(),
        handler,
        owner: host.current.get(),
    });
    Ok(id)
}

/// `events.off(id)` from Rust. True if the handler existed.
pub fn unsubscribe(lua: &Lua, id: u64) -> LuaResult<bool> {
    let host = host(lua)?;
    let mut subs = host.subscriptions.borrow_mut();
    let before = subs.len();
    subs.retain(|s| s.id != id);
    Ok(subs.len() != before)
}

pub(crate) struct Subscription {
    pub id: u64,
    pub name: String,
    pub handler: Function,
    pub owner: Option<u64>,
}

impl ScriptApi for EventsApi {
    fn name(&self) -> &str {
        "events"
    }

    fn install(&self, lua: &Lua, g: &Table) -> LuaResult<()> {
        let t = lua.create_table()?;
        t.set(
            "on",
            lua.create_function(|lua, (name, handler): (String, Function)| {
                subscribe(lua, &name, handler)
            })?,
        )?;
        t.set(
            "off",
            lua.create_function(|lua, id: u64| unsubscribe(lua, id))?,
        )?;
        t.set(
            "send",
            lua.create_function(|lua, (name, payload): (String, Value)| {
                let host = host(lua)?;
                let registered = with_world_mut(lua, |w| -> LuaResult<bool> {
                    let registry = w.registry().clone();
                    let Some(ty) = registry.event(&name) else {
                        return Ok(false);
                    };
                    let mut value = ty.default_value().ok_or_else(|| {
                        mlua::Error::runtime(format!(
                            "events.send: `{name}` has no default value (#[reflect(Default)])"
                        ))
                    })?;
                    if !payload.is_nil() {
                        apply_lua(lua, &registry, value.as_mut(), &payload, &name)?;
                    }
                    ty.send(w, value.as_ref())
                        .map_err(|e| mlua::Error::runtime(format!("events.send: {e}")))?;
                    Ok(true)
                })??;
                if !registered {
                    host.signals.borrow_mut().push((name, payload));
                }
                Ok(())
            })?,
        )?;
        g.set("events", t)
    }

    fn definitions(&self) -> String {
        r#"--- Events between scripts and the engine: registered event types and script-only signals.
declare events: {
    --- Calls `handler(event)` for every event or signal named `name`, in the scripts' Update stage before `on_update`. Registered event types arrive as copies (sent since the handler was added); signals with their payload. Returns an id for `events.off`. The handler belongs to the calling script instance and stops with it.
    on: (name: string, handler: (event: any) -> ()) -> number,
    --- Removes a handler. Returns true if it existed.
    off: (id: number) -> boolean,
    --- Sends an event. A registered event type (`"PlaySound"`) is built from its default patched with `payload` and sent on its queue, where native systems and script handlers read it; any other name is a script signal, delivered to `events.on` handlers with `payload` as is (a signal sent during `on_update` arrives next frame).
    send: @EVENT_SEND@,
}
"#
        .into()
    }

    fn doc(&self) -> &str {
        r#"Decoupled messages. Registered event types (see [Event types](event-types.md)) are shared with native code: `events.send("PlaySound", { clip = "audio/coin.ogg" })` plays a sound. Any other name is a script signal: a lightweight message between scripts with any payload.

The coin's script sends a signal and a sound:

```lua
function Coin.on_trigger_enter(self, other)
    events.send("coin_collected", { value = 10 })
    events.send("PlaySound", { clip = "audio/pickup.wav", position = self.entity:world_position() })
    self.entity:despawn()
end
```

The score keeper's script listens:

```lua
function Score.on_start(self)
    self.score = 0
    events.on("coin_collected", function(payload)
        self.score += payload.value
    end)
end
```"#
    }
}
