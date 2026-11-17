//! `animation`: skeletal animation from scripts. The entity passed is the
//! one with the `AnimationStateMachine` / `AnimationPlayer` (usually the
//! `PrefabInstance` of a glTF character).
//!
//! ```lua
//! animation.set_float(body, "speed", speed)
//! if jumped then animation.trigger(body, "jump") end
//! if animation.state(body) == "Jump" then ... end
//! local hand = animation.joint(body, "LowerArm.R")
//! ```

use box_core::animation::{
    AnimationPlayer, AnimationPlayerState, AnimationStateMachine, AnimationStateMachineState,
};
use box_core::identity::Name;
use box_core::world::{Entity, World};
use box_script::convert::{check_entity, entity_value, to_asset};
use box_script::mlua::{self, Lua, Result as LuaResult, Table, Value};
use box_script::{ScriptApi, with_world, with_world_mut};

pub struct AnimationApi;

fn rt(msg: impl Into<String>) -> mlua::Error {
    mlua::Error::runtime(msg.into())
}

fn missing(w: &World, e: Entity, what: &str) -> mlua::Error {
    if w.contains(e) {
        rt(format!("the entity has no {what}"))
    } else {
        rt("the entity no longer exists")
    }
}

fn machine_mut<R>(
    lua: &Lua,
    e: Entity,
    f: impl FnOnce(&mut AnimationStateMachine) -> R,
) -> LuaResult<R> {
    with_world_mut(lua, |w| match w.component_mut::<AnimationStateMachine>(e) {
        Some(m) => Ok(f(m)),
        None => Err(missing(w, e, "AnimationStateMachine")),
    })?
}

fn machine<R>(lua: &Lua, e: Entity, f: impl FnOnce(&AnimationStateMachine) -> R) -> LuaResult<R> {
    with_world(lua, |w| match w.get::<AnimationStateMachine>(e) {
        Some(m) => Ok(f(&m)),
        None => Err(missing(w, e, "AnimationStateMachine")),
    })?
}

/// The playback state of `e`'s player (`None` before its first frame).
fn playing<R>(
    lua: &Lua,
    e: Entity,
    f: impl FnOnce(&AnimationPlayerState) -> R,
) -> LuaResult<Option<R>> {
    with_world(lua, |w| {
        if !w.has::<AnimationPlayer>(e) {
            return Err(missing(w, e, "AnimationPlayer"));
        }
        Ok(w.get::<AnimationPlayerState>(e).map(|s| f(&s)))
    })?
}

/// The first descendant of `root` named `name` (breadth first).
pub fn find_joint(w: &World, root: Entity, name: &str) -> Option<Entity> {
    let mut queue = std::collections::VecDeque::from(w.children(root));
    while let Some(e) = queue.pop_front() {
        if w.get::<Name>(e).is_some_and(|n| n.as_str() == name) {
            return Some(e);
        }
        queue.extend(w.children(e));
    }
    None
}

impl ScriptApi for AnimationApi {
    fn name(&self) -> &str {
        "animation"
    }

    fn install(&self, lua: &Lua, g: &Table) -> LuaResult<()> {
        let t = lua.create_table()?;
        t.set(
            "set_float",
            lua.create_function(|lua, (e, name, v): (Value, String, f32)| {
                let e = check_entity(&e, "animation.set_float")?;
                machine_mut(lua, e, |m| m.set_float(&name, v))
            })?,
        )?;
        t.set(
            "set_bool",
            lua.create_function(|lua, (e, name, v): (Value, String, bool)| {
                let e = check_entity(&e, "animation.set_bool")?;
                machine_mut(lua, e, |m| m.set_bool(&name, v))
            })?,
        )?;
        t.set(
            "trigger",
            lua.create_function(|lua, (e, name): (Value, String)| {
                let e = check_entity(&e, "animation.trigger")?;
                machine_mut(lua, e, |m| m.trigger(&name))
            })?,
        )?;
        t.set(
            "float",
            lua.create_function(|lua, (e, name): (Value, String)| {
                let e = check_entity(&e, "animation.float")?;
                machine(lua, e, |m| m.float(&name))
            })?,
        )?;
        t.set(
            "bool",
            lua.create_function(|lua, (e, name): (Value, String)| {
                let e = check_entity(&e, "animation.bool")?;
                machine(lua, e, |m| m.bool(&name))
            })?,
        )?;
        t.set(
            "state",
            lua.create_function(|lua, e: Value| {
                let e = check_entity(&e, "animation.state")?;
                with_world(lua, |w| {
                    if !w.has::<AnimationStateMachine>(e) {
                        return Err(missing(w, e, "AnimationStateMachine"));
                    }
                    Ok(w.get::<AnimationStateMachineState>(e)
                        .map(|s| s.current().to_string())
                        .filter(|s| !s.is_empty()))
                })?
            })?,
        )?;
        t.set(
            "transitions",
            lua.create_function(|lua, e: Value| {
                let e = check_entity(&e, "animation.transitions")?;
                with_world(lua, |w| {
                    w.get::<AnimationStateMachineState>(e)
                        .map_or(0, |s| s.transitions())
                })
            })?,
        )?;
        t.set(
            "play",
            lua.create_function(|lua, (e, clip, fade): (Value, Value, Option<f32>)| {
                let e = check_entity(&e, "animation.play")?;
                let clip = to_asset(&clip, "animation.play")?;
                with_world_mut(lua, |w| match w.component_mut::<AnimationPlayer>(e) {
                    Some(p) => {
                        let fade = fade.unwrap_or(p.crossfade);
                        p.crossfade_to(clip, fade);
                        Ok(())
                    }
                    None => Err(missing(w, e, "AnimationPlayer")),
                })?
            })?,
        )?;
        t.set(
            "time",
            lua.create_function(|lua, e: Value| {
                let e = check_entity(&e, "animation.time")?;
                Ok(playing(lua, e, AnimationPlayerState::time)?.unwrap_or(0.0))
            })?,
        )?;
        t.set(
            "normalized_time",
            lua.create_function(|lua, e: Value| {
                let e = check_entity(&e, "animation.normalized_time")?;
                Ok(playing(lua, e, AnimationPlayerState::normalized_time)?.unwrap_or(0.0))
            })?,
        )?;
        t.set(
            "is_finished",
            lua.create_function(|lua, e: Value| {
                let e = check_entity(&e, "animation.is_finished")?;
                Ok(playing(lua, e, AnimationPlayerState::finished)?.unwrap_or(false))
            })?,
        )?;
        box_script::deprecated_alias(lua, &t, "animation", "finished", "is_finished")?;
        t.set(
            "layers",
            lua.create_function(|lua, e: Value| {
                let e = check_entity(&e, "animation.layers")?;
                let layers = playing(lua, e, AnimationPlayerState::layers)?.unwrap_or_default();
                let out = lua.create_table_with_capacity(layers.len(), 0)?;
                for (i, l) in layers.iter().enumerate() {
                    let row = lua.create_table_with_capacity(0, 3)?;
                    row.raw_set("clip", l.clip.to_string())?;
                    row.raw_set("time", l.time)?;
                    row.raw_set("weight", l.weight)?;
                    out.raw_set(i + 1, row)?;
                }
                Ok(out)
            })?,
        )?;
        t.set(
            "joint",
            lua.create_function(|lua, (e, name): (Value, String)| {
                let e = check_entity(&e, "animation.joint")?;
                with_world(lua, |w| find_joint(w, e, &name).map(entity_value))
            })?,
        )?;
        g.set("animation", t)
    }

    fn definitions(&self) -> String {
        r#"--- One clip playing in a blend (during a crossfade there are two).
type AnimationLayer = {
    --- The clip's asset path.
    clip: string,
    --- The playback time in seconds.
    time: number,
    --- The blend weight from 0 to 1.
    weight: number,
}

--- Skeletal animation: state machine parameters, direct clip playback, playback state and joints.
declare animation: {
    --- Sets a number parameter of the entity's `AnimationStateMachine` (transitions test it, such as "speed" > 0.1).
    set_float: (entity: Entity, parameter: string, value: number) -> (),
    --- Sets a boolean parameter of the state machine.
    set_bool: (entity: Entity, parameter: string, value: boolean) -> (),
    --- Sets a trigger parameter: it stays set until a transition that tests it fires (and clears it).
    trigger: (entity: Entity, parameter: string) -> (),
    --- A number parameter's value (0 when unset).
    float: (entity: Entity, parameter: string) -> number,
    --- A boolean or trigger parameter's value (false when unset).
    bool: (entity: Entity, parameter: string) -> boolean,
    --- The state machine's current state name, or nil before its first frame.
    state: (entity: Entity) -> string?,
    --- How many transitions the state machine has taken (to notice a change of state).
    transitions: (entity: Entity) -> number,
    --- Plays a clip (asset path, such as "models/hero.glb#Animation/Run") on the entity's `AnimationPlayer`, crossfading over `crossfade` seconds (default the player's own crossfade).
    play: (entity: Entity, clip: string, crossfade: number?) -> (),
    --- The current clip's playback time in seconds.
    time: (entity: Entity) -> number,
    --- The current clip's playback position from 0 to 1.
    normalized_time: (entity: Entity) -> number,
    --- True when a non-looping clip has reached its end.
    is_finished: (entity: Entity) -> boolean,
    --- Deprecated: use `is_finished`.
    finished: (entity: Entity) -> boolean,
    --- The clips playing now with their times and blend weights.
    layers: (entity: Entity) -> { AnimationLayer },
    --- The first descendant named `name` (a joint of the skeleton, breadth first), or nil: attach a sword to a hand by parenting it to the joint.
    joint: (entity: Entity, name: string) -> Entity?,
}
"#
        .into()
    }

    fn doc(&self) -> &str {
        r#"Skeletal animation of glTF characters. The entity passed is the one with the `AnimationStateMachine` or `AnimationPlayer` (usually the `PrefabInstance` of the character's `.glb`). With a state machine (`*.animgraph.json`), scripts only set parameters and the graph picks clips and crossfades; without one, `animation.play` drives the player directly.

```lua
function Hero.on_update(self, dt)
    local body = self.props.body
    local v = character.velocity(self.entity)
    animation.set_float(body, "speed", vector.magnitude(vector.create(v.x, 0, v.z)))
    animation.set_bool(body, "grounded", character.is_grounded(self.entity))
    if input.pressed("jump") then
        animation.trigger(body, "jump")
    end
    if animation.state(body) == "Jump" and animation.is_finished(body) then
        log.info("jump clip done")
    end
end
```"#
    }
}
