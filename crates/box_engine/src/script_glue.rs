//! Scripts in engine apps: box_engine's script APIs (hand-written bindings
//! of types box_engine owns, added through the same [`ScriptApis`] hook a
//! game would use) and the place [`Game::app`](crate::Game::app) adds the
//! script runtime.
//!
//! - `scenes`: runtime scenes ([`Scenes`]).
//! - `animation` ([`animation`]): state machine parameters, the current
//!   state, clip playback and crossfades, joints by name.
//! - `save` ([`save`]): save games, slots, save events, `Saveable` /
//!   `SaveExclude`; plus the `scripts` [`SaveHooks`](crate::savegame::SaveHooks)
//!   entry, so a save restores script state (`on_save` / `on_load`).
//! - `data` ([`data`]): data tables as read-only Luau tables.
//! - `particles` ([`particles`]): bursts, start, stop, restart.
//! - `camera` ([`camera`]): world to screen, screen and cursor rays.
//!
//! box_script adds `physics` and `character`; box_ui adds `ui` and
//! `strings`.

pub mod animation;
pub mod camera;
pub mod data;
pub mod particles;
pub mod save;

use box_core::app::App;
use box_script::convert::check_entity;
use box_script::mlua::{Lua, Result as LuaResult, Table, Value};
use box_script::{ScriptApi, ScriptApis, ScriptPlugin, with_world, with_world_mut};

use crate::scenes::{Persistent, RuntimeEntities, SceneInstanceId, Scenes};

/// `scenes.switch(name)`, `scenes.load_additive(name)` (`scenes.load`,
/// deprecated), `scenes.unload(id)`,
/// `scenes.current()`, `scenes.is_loaded(id)`, `scenes.persist(entity)`,
/// `scenes.set_runtime_entities(mode)`.
pub struct ScenesApi;

fn no_scenes() -> box_script::mlua::Error {
    box_script::mlua::Error::runtime("this world has no runtime scenes (Scenes resource)")
}

impl ScriptApi for ScenesApi {
    fn name(&self) -> &str {
        "scenes"
    }

    fn install(&self, lua: &Lua, g: &Table) -> LuaResult<()> {
        let t = lua.create_table()?;
        t.set(
            "switch",
            lua.create_function(|lua, name: String| {
                with_world_mut(lua, |w| {
                    w.get_resource_mut::<Scenes>()
                        .map(|s| s.switch(&name).0)
                        .ok_or_else(no_scenes)
                })?
            })?,
        )?;
        t.set(
            "load_additive",
            lua.create_function(|lua, name: String| {
                with_world_mut(lua, |w| {
                    w.get_resource_mut::<Scenes>()
                        .map(|s| s.load_additive(&name).0)
                        .ok_or_else(no_scenes)
                })?
            })?,
        )?;
        box_script::deprecated_alias(lua, &t, "scenes", "load", "load_additive")?;
        t.set(
            "unload",
            lua.create_function(|lua, id: u64| {
                with_world_mut(lua, |w| {
                    w.get_resource_mut::<Scenes>()
                        .map(|s| s.unload(SceneInstanceId(id)))
                        .ok_or_else(no_scenes)
                })?
            })?,
        )?;
        t.set(
            "current",
            lua.create_function(|lua, ()| {
                with_world(lua, |w| {
                    let s = w.get_resource::<Scenes>()?;
                    let p = s.path(s.main()?)?;
                    Some(p.to_string_lossy().into_owned())
                })
            })?,
        )?;
        t.set(
            "is_loaded",
            lua.create_function(|lua, id: u64| {
                with_world(lua, |w| {
                    w.get_resource::<Scenes>()
                        .is_some_and(|s| s.is_loaded(SceneInstanceId(id)))
                })
            })?,
        )?;
        t.set(
            "persist",
            lua.create_function(|lua, (e, keep): (Value, Option<bool>)| {
                let e = check_entity(&e, "scenes.persist")?;
                with_world_mut(lua, |w| {
                    if keep.unwrap_or(true) {
                        w.insert_one(e, Persistent).is_ok()
                    } else {
                        w.remove_one::<Persistent>(e).is_ok()
                    }
                })
            })?,
        )?;
        t.set(
            "set_runtime_entities",
            lua.create_function(|lua, mode: String| {
                let policy = match mode.as_str() {
                    "keep_all" => RuntimeEntities::KeepAll,
                    "despawn_unless_persistent" => RuntimeEntities::DespawnUnlessPersistent,
                    other => {
                        return Err(box_script::mlua::Error::runtime(format!(
                            "scenes.set_runtime_entities: `{other}` is not \"keep_all\" or \"despawn_unless_persistent\""
                        )));
                    }
                };
                with_world_mut(lua, |w| {
                    w.resource_or_default::<Scenes>()
                        .set_runtime_entities(policy)
                })
            })?,
        )?;
        g.set("scenes", t)
    }

    fn definitions(&self) -> String {
        r#"--- Runtime scenes: switch levels, load and unload scenes additively.
declare scenes: {
    --- Replaces every loaded scene with `scene` (a name such as "level2", for `assets/scenes/level2.scene.json`, or an assets-relative path) at the start of the next Update. The file is validated first; on failure nothing changes. Returns the id the new scene instance will have.
    switch: (scene: string) -> number,
    --- Loads `scene` next to the loaded ones (a room, a UI scene) at the start of the next Update. Returns its instance id for `scenes.unload`.
    load_additive: (scene: string) -> number,
    --- Deprecated: use `load_additive`.
    load: (scene: string) -> number,
    --- Removes a loaded scene instance and its entities.
    unload: (id: number) -> (),
    --- The path of the main scene (the last switch, or the start scene), or nil.
    current: () -> string?,
    --- True while the scene instance `id` is loaded.
    is_loaded: (id: number) -> boolean,
    --- Marks a runtime entity (spawned by a script, not loaded from a scene) to survive scene switches (`keep` false unmarks it). Returns false if the entity is gone.
    persist: (entity: Entity, keep: boolean?) -> boolean,
    --- What a scene switch does with runtime entities: "keep_all" (the default: they all survive) or "despawn_unless_persistent" (recommended: only root entities marked with `scenes.persist` survive).
    set_runtime_entities: (mode: "keep_all" | "despawn_unless_persistent") -> (),
}
"#
        .into()
    }

    fn doc(&self) -> &str {
        r#"Scene files are levels and rooms (`assets/scenes/*.scene.json`). The player starts with the project's start scene; scripts switch or add scenes. Requests apply at the start of the next Update, so every script sees either the old or the new scene, never half of each. Entities loaded from a scene belong to it; entities spawned at runtime survive a switch unless parented under a scene entity or, with "despawn_unless_persistent", not marked with `scenes.persist`.

```lua
function Goal.on_trigger_enter(self, other)
    if other:name() == "Player" then
        scenes.switch("level2")
    end
end
```"#
    }
}

/// Adds the engine's script APIs, the `scripts` save hook, the script
/// runtime and the script rule host ([`box_script::rules`]) to `app` (after
/// the game's plugins, so scripts run after its native systems and script
/// rules after its Rust rules).
pub fn add_scripts(app: &mut App) {
    box_script::add_script_rules(app);
    app.world
        .resource_or_default::<ScriptApis>()
        .add(ScenesApi)
        .add(animation::AnimationApi)
        .add(save::SaveApi)
        .add(data::DataApi)
        .add(particles::ParticlesApi)
        .add(camera::CameraApi);
    save::install(app);
    app.add_plugin(&ScriptPlugin);
}
