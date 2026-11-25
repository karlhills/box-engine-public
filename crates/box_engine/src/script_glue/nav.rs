//! `nav`: navmesh queries for scripts, and the `move_to`, `stop` and
//! `nav_status` entity methods that drive a [`NavAgent`]
//! (docs/NAVIGATION.md; `navigation` feature).

use box_physics::KinematicCharacter;
use box_script::convert::{check_entity, from_vector, vector};
use box_script::mlua::{Lua, Result as LuaResult, Table, Value, Vector};
use box_script::{ScriptApi, entity_methods, with_world, with_world_mut};

use crate::nav_agent::{NavAgent, nav_status};
use crate::nav_glue::{nav_closest, nav_path, nav_random_near, nav_reachable};

pub struct NavApi;

impl ScriptApi for NavApi {
    fn name(&self) -> &str {
        "nav"
    }

    fn install(&self, lua: &Lua, g: &Table) -> LuaResult<()> {
        let t = lua.create_table()?;
        t.set(
            "path",
            lua.create_function(|lua, (from, to): (Vector, Vector)| {
                let path = with_world(lua, |w| nav_path(w, from_vector(from), from_vector(to)))?;
                match path.filter(|p| p.is_complete()) {
                    Some(p) => {
                        let out = lua.create_table()?;
                        for (i, q) in p.points.iter().enumerate() {
                            out.set(i + 1, vector(*q))?;
                        }
                        Ok(Value::Table(out))
                    }
                    None => Ok(Value::Nil),
                }
            })?,
        )?;
        t.set(
            "ready",
            lua.create_function(|lua, ()| {
                with_world(lua, |w| {
                    w.get_resource::<crate::nav_glue::Navigation>()
                        .is_some_and(|n| !n.mesh().is_empty() && !n.is_baking())
                })
            })?,
        )?;
        t.set(
            "reachable",
            lua.create_function(|lua, (from, to): (Vector, Vector)| {
                with_world(lua, |w| {
                    nav_reachable(w, from_vector(from), from_vector(to))
                })
            })?,
        )?;
        t.set(
            "closest",
            lua.create_function(|lua, p: Vector| {
                with_world(lua, |w| nav_closest(w, from_vector(p)).map(vector))
            })?,
        )?;
        t.set(
            "random_near",
            lua.create_function(|lua, (center, radius, seed): (Vector, f32, Option<f64>)| {
                with_world(lua, |w| {
                    // Without a seed: one from game time, so replays repeat.
                    let seed = seed.map_or_else(
                        || {
                            w.get_resource::<box_core::time::Time>()
                                .map_or(0, |t| t.elapsed_f64.to_bits())
                        },
                        |s| s as u64,
                    );
                    nav_random_near(w, from_vector(center), radius.max(0.0), seed).map(vector)
                })
            })?,
        )?;
        g.set("nav", t)?;

        let m = entity_methods(lua)?;
        m.set(
            "move_to",
            lua.create_function(|lua, (e, p): (Value, Vector)| {
                let e = check_entity(&e, "move_to")?;
                with_world_mut(lua, |w| {
                    if !w.contains(e) {
                        return Err(box_script::mlua::Error::runtime("the entity no longer exists"));
                    }
                    if !w.has::<KinematicCharacter>(e) {
                        return Err(box_script::mlua::Error::runtime(
                            "move_to needs a KinematicCharacter on the entity (add one, or a NavAgent, in the editor)",
                        ));
                    }
                    if let Some(a) = w.component_mut::<NavAgent>(e) {
                        a.destination = Some(from_vector(p));
                    } else {
                        let _ = w.insert_one(
                            e,
                            NavAgent {
                                destination: Some(from_vector(p)),
                                ..Default::default()
                            },
                        );
                    }
                    Ok(())
                })?
            })?,
        )?;
        m.set(
            "stop",
            lua.create_function(|lua, e: Value| {
                let e = check_entity(&e, "stop")?;
                with_world_mut(lua, |w| {
                    if let Some(a) = w.component_mut::<NavAgent>(e) {
                        a.destination = None;
                    }
                })
            })?,
        )?;
        m.set(
            "nav_status",
            lua.create_function(|lua, e: Value| {
                let e = check_entity(&e, "nav_status")?;
                with_world(lua, |w| nav_status(w, e).name())
            })?,
        )?;
        Ok(())
    }

    fn definitions(&self) -> String {
        r#"--- Where characters the game moves can walk: the navmesh, built from the scene's static colliders.
declare nav: {
    --- True once the navmesh is built and no part of it is being rebuilt (right after a scene starts, or after colliders change, it is not: paths may be missing or change soon).
    ready: () -> boolean,
    --- The corners of the shortest walkable path from `from` to `to` (feet positions), start first; nil when `to` cannot be reached or there is no navmesh yet.
    path: (from: vector, to: vector) -> { vector }?,
    --- Whether something can walk from `from` to `to`.
    reachable: (from: vector, to: vector) -> boolean,
    --- The nearest walkable point to `position` (within 2 m sideways and 3 m up or down), or nil.
    closest: (position: vector) -> vector?,
    --- A random walkable point within `radius` metres of `center`, reachable from it (wander targets, spawn spots). The same `seed` gives the same point; without one it changes every frame.
    random_near: (center: vector, radius: number, seed: number?) -> vector?,
}
"#
        .into()
    }

    fn entity_definitions(&self) -> String {
        r#"    --- Walks this character to `position` along the navmesh, around walls, through doors and up stairs (adds a `NavAgent` if it has none; needs a `KinematicCharacter`). Call it again to change the goal: a goal that moved less than a metre does not replan.
    function move_to(self, position: vector): ()
    --- Stops walking (clears the `NavAgent` destination).
    function stop(self): ()
    --- Where its walk stands: "idle", "baking" (the navmesh is not ready: it walks straight), "moving", "arrived", "unreachable" (it walks as close as it can) or "off_mesh".
    function nav_status(self): string
"#
        .into()
    }

    fn doc(&self) -> &str {
        r#"Characters the game moves (enemies, villagers, pets) walk on the navmesh: the walkable area of the scene, built automatically from its static colliders and kept up to date when they change (View ▸ Navigation shows it in the editor). Give a character a destination with `e:move_to(point)` and it walks there around obstacles; `e:nav_status()` says when it has arrived or cannot get there. `nav` answers questions about the walkable area without moving anything.

```lua
-- A guard that patrols between two points and chases the player when close.
function Guard.on_start(self)
    self.home = self.entity:world_position()
    self.post = self.home + vector.create(8, 0, 0)
    self.entity:move_to(self.post)
end

function Guard.on_update(self, dt)
    local player = world.find("Player")
    if player and vector.magnitude(player:world_position() - self.entity:world_position()) < 6 then
        self.entity:move_to(player:world_position())
    elseif self.entity:nav_status() == "arrived" then
        -- Turn back at each end.
        self.home, self.post = self.post, self.home
        self.entity:move_to(self.post)
    end
end
```"#
    }
}
