//! `ground`: the height and normal of the terrain under a point, in
//! scripts (the world's `box_proc::Ground`) and in rules (the same ground,
//! handed to the run; a rule that samples it re-runs where the terrain
//! changes).

use box_proc::ground::{Ground, GroundSample};
use mlua::{Lua, Result as LuaResult, Table};

use super::{ScriptApi, with_world};
use crate::convert::vector;

pub(crate) struct GroundApi;

/// Adds `height` and `at` to `t`, sampling with `sample`.
pub(crate) fn install_functions(
    lua: &Lua,
    t: &Table,
    sample: fn(&Lua, f32, f32) -> LuaResult<Option<GroundSample>>,
) -> LuaResult<()> {
    t.set(
        "height",
        lua.create_function(move |lua, (x, z): (f32, f32)| {
            Ok(sample(lua, x, z)?.map(|s| s.height))
        })?,
    )?;
    t.set(
        "at",
        lua.create_function(move |lua, (x, z): (f32, f32)| {
            sample(lua, x, z)?
                .map(|s| {
                    let out = lua.create_table_with_capacity(0, 3)?;
                    out.raw_set("height", s.height)?;
                    out.raw_set("normal", vector(s.normal))?;
                    out.raw_set("position", vector(glam::Vec3::new(x, s.height, z)))?;
                    Ok(out)
                })
                .transpose()
        })?,
    )?;
    Ok(())
}

fn world_sample(lua: &Lua, x: f32, z: f32) -> LuaResult<Option<GroundSample>> {
    with_world(lua, |w| {
        w.get_resource::<Ground>().and_then(|g| g.sample(x, z))
    })
}

impl ScriptApi for GroundApi {
    fn name(&self) -> &str {
        "ground"
    }

    fn install(&self, lua: &Lua, g: &Table) -> LuaResult<()> {
        let t = lua.create_table()?;
        install_functions(lua, &t, world_sample)?;
        g.set("ground", t)
    }

    fn definitions(&self) -> String {
        r#"--- The ground at a point.
type GroundHit = {
    --- Height of the ground in metres.
    height: number,
    --- The surface normal (unit length, pointing up out of the ground).
    normal: vector,
    --- The point on the ground: `x`, `height`, `z`.
    position: vector,
}
--- The terrain under a point: heights of the drawn and walked surface of every `Terrain` (the highest where they overlap). Also available in rules, where sampling it makes the rule re-run where the terrain changes.
declare ground: {
    --- The ground height at world `x`, `z` in metres, or nil where there is no terrain.
    height: (x: number, z: number) -> number?,
    --- Height, normal and point of the ground at world `x`, `z`, or nil where there is no terrain.
    at: (x: number, z: number) -> GroundHit?,
}
"#
        .to_owned()
    }

    fn doc(&self) -> &str {
        r#"Heights come from the world's `Terrain` entities: exactly the surface that is drawn and that characters walk on, so whatever a script or rule places at `ground.height(x, z)` meets the ground. Where there is no terrain the functions return nil. To keep an entity on the ground without a script, give it a `FollowGround` component (with an `offset`); a `Sweep` with `follow_ground` lays a path on it.

In a rule, `ground` reads the terrain as it is when the rule runs. A rule that samples it re-runs when the terrain under its intents changes: a rule with a `scope` only for the intents whose points are in the changed area, a rule without one for all of them.

```lua
function Lamps.rule(intents, out)
    for _, it in intents do
        for i, p in it.points do
            local y = ground.height(p.x, p.z) or p.y
            out:part(it, `lamp{i}`, { Primitive = { shape = "Cylinder" } }, { position = vector.create(p.x, y + 1, p.z) })
        end
    end
end
```
"#
    }
}
