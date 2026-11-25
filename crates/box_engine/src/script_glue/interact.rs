//! `vars` and `interact`: game variables and interactables from scripts
//! (docs/INTERACTIONS.md).
//!
//! ```lua
//! vars.add("embers", 1)
//! if vars.get("embers") >= 2 then ... end
//! vars.on_change("embers", function(change) ui.set_text("count", tostring(change.value)) end)
//! interact.use(lever)          -- as if the player pressed it (conditions apply)
//! ```

use std::collections::BTreeMap;

use box_core::app::{App, Stage};
use box_core::world::World;
use box_script::convert::{check_entity, entity_value};
use box_script::mlua::{Function, Lua, Result as LuaResult, Table, Value};
use box_script::{ScriptApi, ScriptRuntime, ScriptSignals, with_world, with_world_mut};
use serde_json::json;

use crate::interact::{GameVars, InteractFocus, InteractRequests, Interactable, conditions_hold};

/// Signal prefix of `vars.on_change` (`vars:<name>`).
const VARS_SIGNAL: &str = "vars:";

pub struct VarsApi;

impl ScriptApi for VarsApi {
    fn name(&self) -> &str {
        "vars"
    }

    fn install(&self, lua: &Lua, g: &Table) -> LuaResult<()> {
        let t = lua.create_table()?;
        t.set(
            "get",
            lua.create_function(|lua, name: String| {
                with_world(lua, |w| {
                    w.get_resource::<GameVars>().map_or(0.0, |v| v.get(&name))
                })
            })?,
        )?;
        t.set(
            "set",
            lua.create_function(|lua, (name, value): (String, f64)| {
                with_world_mut(lua, |w| {
                    w.resource_or_default::<GameVars>().set(&name, value)
                })
            })?,
        )?;
        t.set(
            "add",
            lua.create_function(|lua, (name, amount): (String, f64)| {
                with_world_mut(lua, |w| {
                    w.resource_or_default::<GameVars>().add(&name, amount)
                })
            })?,
        )?;
        t.set(
            "all",
            lua.create_function(|lua, ()| {
                let values: BTreeMap<String, f64> = with_world(lua, |w| {
                    w.get_resource::<GameVars>()
                        .map(|v| v.values.clone())
                        .unwrap_or_default()
                })?;
                let t = lua.create_table()?;
                for (k, v) in values {
                    t.set(k, v)?;
                }
                Ok(t)
            })?,
        )?;
        t.set(
            "on_change",
            lua.create_function(|lua, (name, handler): (String, Function)| {
                box_script::subscribe(lua, &format!("{VARS_SIGNAL}{name}"), handler)
            })?,
        )?;
        t.set(
            "off",
            lua.create_function(|lua, id: u64| box_script::unsubscribe(lua, id))?,
        )?;
        g.set("vars", t)
    }

    fn definitions(&self) -> String {
        r#"--- A change of a game variable, as `vars.on_change` handlers receive it.
type VarChange = {
    --- The variable's name.
    name: string,
    --- The new value.
    value: number,
    --- The value before the change.
    old: number,
}
--- Game variables: named numbers for puzzle and progress state (`embers`, `gate_open`), shared with `Interactable` conditions and actions, kept across scene switches and saved in save games. Unset names read 0.
declare vars: {
    --- The value of `name` (0 when unset).
    get: (name: string) -> number,
    --- Sets `name` to `value`.
    set: (name: string, value: number) -> (),
    --- Adds `amount` (negative: subtracts) to `name` and returns the new value.
    add: (name: string, amount: number) -> number,
    --- Every variable that was set, by name.
    all: () -> { [string]: number },
    --- Calls `handler` in the next Update after `name` changes (by a script, an `Interactable` or a loaded save). Returns an id for `vars.off`; removed with the script.
    on_change: (name: string, handler: (change: VarChange) -> ()) -> number,
    --- Removes an `on_change` handler.
    off: (id: number) -> (),
}"#
        .to_string()
    }

    fn doc(&self) -> &str {
        r#"Game variables are named numbers that hold puzzle and progress state. `Interactable` conditions test them (`AtLeast`, `Below`, `Equals`) and its actions change them (`AddVar`, `SetVar`), so most puzzles need no script; scripts read and change the same values, and hear about changes:

```lua
    vars.on_change("offerings", function(change)
        self.offerings = change.value
    end)
```"#
    }
}

pub struct InteractApi;

impl ScriptApi for InteractApi {
    fn name(&self) -> &str {
        "interact"
    }

    fn install(&self, lua: &Lua, g: &Table) -> LuaResult<()> {
        let t = lua.create_table()?;
        t.set(
            "focus",
            lua.create_function(|lua, ()| {
                with_world(lua, |w| {
                    w.get_resource::<InteractFocus>()
                        .and_then(|f| f.entity)
                        .map_or(Value::Nil, entity_value)
                })
            })?,
        )?;
        t.set(
            "use",
            lua.create_function(|lua, e: Value| {
                let e = check_entity(&e, "interact.use")?;
                with_world_mut(lua, |w| {
                    w.resource_or_default::<InteractRequests>().use_entity(e);
                })
            })?,
        )?;
        t.set(
            "can_use",
            lua.create_function(|lua, e: Value| {
                let e = check_entity(&e, "interact.can_use")?;
                with_world(lua, |w| {
                    w.get::<Interactable>(e).is_some_and(|i| i.available()) && conditions_hold(w, e)
                })
            })?,
        )?;
        g.set("interact", t)
    }

    fn definitions(&self) -> String {
        r#"--- Interactables (`Interactable` components): what the player can use, and using them from scripts.
declare interact: {
    --- The interactable whose prompt shows now (the nearest one in range), or nil.
    focus: () -> Entity?,
    --- Uses `entity`'s `Interactable` in the next Update as if the player pressed its action: its conditions, cooldown and `once` apply, its distance does not.
    use: (entity: Entity) -> (),
    --- True when `entity`'s `Interactable` is enabled, not used up and its conditions hold.
    can_use: (entity: Entity) -> boolean,
}"#
        .to_string()
    }

    fn doc(&self) -> &str {
        r#"An `Interactable` component makes an entity usable: a prompt shows when the player is in range, pressing its action (`interact`: E or the gamepad's North button) checks its conditions and runs its actions. A script on the same entity hears each use in `on_interact`:

```lua
function Altar.on_interact(self, interactor: Entity)
    self.used_by = interactor:name()
    vars.add("visits", 1)
end
```

Every use also sends the `Interacted` event, which any script can handle:

```lua
    events.on("Interacted", function(e)
        local used = world.entity(e.entity)
        if used ~= nil then
            log.info(used:name() .. " was used")
        end
    end)
```

`interact.use(entity)` uses one from a script (a lever that opens a door elsewhere)."#
    }
}

/// Adds the `vars` change signals: when [`GameVars`] changed since the last
/// frame, sends `vars:<name>` for each changed value. Idle cost: one
/// revision check.
pub(super) fn install(app: &mut App) {
    let mut last: Option<(u64, BTreeMap<String, f64>)> = None;
    app.system(Stage::Update, move |w: &mut World| {
        forward_var_changes(&mut last, w)
    })
    .label("vars_signals")
    .after("interact");
}

fn forward_var_changes(last: &mut Option<(u64, BTreeMap<String, f64>)>, w: &mut World) {
    let Some(vars) = w.get_resource::<GameVars>() else {
        return;
    };
    let revision = vars.revision();
    let Some((seen, before)) = last else {
        *last = Some((revision, vars.values.clone()));
        return;
    };
    if *seen == revision {
        return;
    }
    let now = vars.values.clone();
    let mut changes = Vec::new();
    for (name, value) in &now {
        let old = before.get(name).copied().unwrap_or(0.0);
        if old != *value {
            changes.push((name.clone(), *value, old));
        }
    }
    for (name, old) in before.iter() {
        if !now.contains_key(name) && *old != 0.0 {
            changes.push((name.clone(), 0.0, *old));
        }
    }
    *last = Some((revision, now));
    if changes.is_empty() || !w.contains_resource::<ScriptRuntime>() {
        return;
    }
    let signals = w.resource_or_default::<ScriptSignals>();
    for (name, value, old) in changes {
        signals.send(
            format!("{VARS_SIGNAL}{name}"),
            json!({ "name": name, "value": value, "old": old }),
        );
    }
}
