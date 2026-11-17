//! `save`: save games from scripts, and script state in save games.
//!
//! ```lua
//! save.save("slot1")                 -- applies at the start of the next Update
//! save.load("slot1")
//! save.on(function(ev)               -- ev.kind: "saved" | "loaded" | "failed"
//!     log.info(ev.kind .. " " .. ev.slot)
//! end)
//! save.include(world.spawn{ ... })   -- a runtime entity that saves (Saveable)
//! save.exclude(self.entity)          -- leave out of saves (SaveExclude)
//! ```
//!
//! Script state: [`install`] adds the `scripts` [`SaveHooks`] entry
//! (`box_script::save_script_state` / `load_script_state`), so each running
//! script's `on_save(self)` result is saved and handed to its
//! `on_load(self, data)` when the save is loaded: after the loaded entity's
//! `on_start` has returned (all its waits included), so `on_start` sets
//! defaults and `on_load` overrides them.

use box_core::app::{App, Stage};
use box_core::event::{EventCursor, Events};
use box_core::world::World;
use box_script::convert::check_entity;
use box_script::mlua::{self, Function, Lua, Result as LuaResult, Table, Value};
use box_script::{
    ScriptApi, ScriptRuntime, ScriptSignals, load_script_state, save_script_state, with_world,
    with_world_mut,
};
use serde_json::json;

use crate::savegame::{
    SaveEvent, SaveExclude, SaveGames, SaveHooks, Saveable, list_slots, slot_path,
};

/// The script signal save results are sent as (`save.on`).
pub const SAVE_SIGNAL: &str = "save";

pub struct SaveApi;

fn rt(msg: impl Into<String>) -> mlua::Error {
    mlua::Error::runtime(msg.into())
}

fn request(lua: &Lua, slot: &str, f: impl FnOnce(&mut SaveGames, &str)) -> LuaResult<()> {
    with_world_mut(lua, |w| {
        slot_path(w, slot).map_err(|e| rt(e.to_string()))?;
        match w.get_resource_mut::<SaveGames>() {
            Some(g) => {
                f(g, slot);
                Ok(())
            }
            None => Err(rt("this world has no save games (SavePlugin)")),
        }
    })?
}

impl ScriptApi for SaveApi {
    fn name(&self) -> &str {
        "save"
    }

    fn install(&self, lua: &Lua, g: &Table) -> LuaResult<()> {
        let t = lua.create_table()?;
        t.set(
            "save",
            lua.create_function(|lua, slot: String| request(lua, &slot, |g, s| g.save(s)))?,
        )?;
        t.set(
            "load",
            lua.create_function(|lua, slot: String| request(lua, &slot, |g, s| g.load(s)))?,
        )?;
        t.set(
            "slots",
            lua.create_function(|lua, ()| {
                let slots = with_world(lua, list_slots)?;
                let out = lua.create_table_with_capacity(slots.len(), 0)?;
                for (i, s) in slots.iter().enumerate() {
                    let row = lua.create_table_with_capacity(0, 4)?;
                    row.raw_set("slot", s.slot.as_str())?;
                    row.raw_set("saved_at", s.saved_at)?;
                    row.raw_set("version", s.version)?;
                    row.raw_set("scene", s.scene.as_deref())?;
                    out.raw_set(i + 1, row)?;
                }
                Ok(out)
            })?,
        )?;
        t.set(
            "include",
            lua.create_function(|lua, (e, included): (Value, Option<bool>)| {
                let e = check_entity(&e, "save.include")?;
                with_world_mut(lua, |w| {
                    if included.unwrap_or(true) {
                        w.insert_one(e, Saveable).is_ok()
                    } else {
                        w.remove_one::<Saveable>(e).is_ok()
                    }
                })
            })?,
        )?;
        box_script::deprecated_alias(lua, &t, "save", "keep", "include")?;
        t.set(
            "exclude",
            lua.create_function(|lua, (e, excluded): (Value, Option<bool>)| {
                let e = check_entity(&e, "save.exclude")?;
                with_world_mut(lua, |w| {
                    if excluded.unwrap_or(true) {
                        w.insert_one(e, SaveExclude).is_ok()
                    } else {
                        w.remove_one::<SaveExclude>(e).is_ok()
                    }
                })
            })?,
        )?;
        t.set(
            "on",
            lua.create_function(|lua, handler: Function| {
                box_script::subscribe(lua, SAVE_SIGNAL, handler)
            })?,
        )?;
        t.set(
            "off",
            lua.create_function(|lua, id: u64| box_script::unsubscribe(lua, id))?,
        )?;
        g.set("save", t)
    }

    fn definitions(&self) -> String {
        r#"--- A save file, as `save.slots` lists it.
type SaveSlot = {
    --- The slot name.
    slot: string,
    --- When it was written, in Unix seconds.
    saved_at: number,
    --- The game's save version when it was written.
    version: number,
    --- The main scene's path when it was saved.
    scene: string?,
}
--- The outcome of a save or load request, as `save.on` handlers receive it.
type SaveResult = {
    --- What happened.
    kind: "saved" | "loaded" | "failed",
    --- The slot of the request.
    slot: string,
    --- The save file's path.
    path: string?,
    --- How many entities were saved or loaded.
    entities: number?,
    --- Why it failed (`failed` only).
    message: string?,
}

--- Save games: snapshots of the running world in named slots.
declare save: {
    --- Saves the game to `slot` (letters, digits, `-` and `_`, at most 64) at the start of the next Update. The result arrives at `save.on` handlers.
    save: (slot: string) -> (),
    --- Loads `slot` at the start of the next Update: the world is replaced by the saved one and scripts get their `on_load`. The result arrives at `save.on` handlers.
    load: (slot: string) -> (),
    --- The save files, newest first.
    slots: () -> { SaveSlot },
    --- Makes a runtime entity (spawned by a script) part of save games (`Saveable`); `included = false` takes that back. Entities loaded from scenes are saved anyway. Returns false if the entity is gone.
    include: (entity: Entity, included: boolean?) -> boolean,
    --- Deprecated: use `include`.
    keep: (entity: Entity) -> boolean,
    --- Leaves an entity and its descendants out of save games (`excluded` false puts it back). Returns false if the entity is gone.
    exclude: (entity: Entity, excluded: boolean?) -> boolean,
    --- Calls `handler(result)` after each save or load request finishes. Returns an id for `events.off`. The handler belongs to the calling script instance.
    on: (handler: (result: SaveResult) -> ()) -> number,
    --- Removes a `save.on` handler by the id it returned.
    off: (id: number) -> boolean,
}
"#
        .into()
    }

    fn doc(&self) -> &str {
        r#"A save game holds what a scene file holds, taken from the running world (every entity with a uid and its saved components, and saved resources), plus each running script's `on_save` result, handed back to its `on_load` after a load. Files are `<project>/.box/saves/<slot>.save.json`, written atomically with a backup. Requests apply at the start of the next Update, so a save never sees a half-updated frame.

```lua
function Game.on_start(self)
    self.coins = 0                 -- defaults first: on_load runs after this
    events.on("coin_collected", function()
        self.coins += 1
    end)
    save.on(function(result)
        log.info(result.kind, result.slot, result.message or "")
    end)
end

function Game.on_update(self, dt)
    if input.key_pressed("F5") then save.save("quick") end
    if input.key_pressed("F9") then save.load("quick") end
end

function Game.on_save(self)
    return { coins = self.coins }
end

function Game.on_load(self, data)
    self.coins = data.coins
end
```"#
    }
}

/// The `scripts` save hook and the `save` signal forwarder.
pub(super) fn install(app: &mut App) {
    app.world.resource_or_default::<SaveHooks>().add_mut(
        "scripts",
        save_script_state,
        |w: &mut World, state: &serde_json::Value| {
            // A world that never ran a script has nothing to restore.
            if state.as_object().is_some_and(|m| !m.is_empty()) {
                load_script_state(w, state);
            }
        },
    );
    let mut cursor = EventCursor::<SaveEvent>::default();
    app.system(Stage::Update, move |w: &mut World| {
        forward_save_events(&mut cursor, w)
    })
    .label("save_script_events")
    .after("save_requests");
}

/// Sends each [`SaveEvent`] to scripts as the `save` signal while the
/// world runs scripts.
fn forward_save_events(cursor: &mut EventCursor<SaveEvent>, world: &mut World) {
    let Some(events) = world.get_resource::<Events<SaveEvent>>() else {
        return;
    };
    let list: Vec<SaveEvent> = cursor.read(events).cloned().collect();
    if list.is_empty() || !world.contains_resource::<ScriptRuntime>() {
        return;
    }
    let signals = world.resource_or_default::<ScriptSignals>();
    for ev in list {
        let payload = match ev {
            SaveEvent::Saved { slot, path } => {
                json!({ "kind": "saved", "slot": slot, "path": path.to_string_lossy() })
            }
            SaveEvent::Loaded {
                slot,
                path,
                entities,
            } => json!({
                "kind": "loaded", "slot": slot, "path": path.to_string_lossy(), "entities": entities,
            }),
            SaveEvent::Failed { slot, message } => {
                json!({ "kind": "failed", "slot": slot, "message": message })
            }
        };
        signals.send(SAVE_SIGNAL, payload);
    }
}
