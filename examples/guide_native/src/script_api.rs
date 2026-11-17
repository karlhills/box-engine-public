//! A script API written in Rust: the `score` global that Luau scripts call
//! (`assets/scripts/bonus.luau` uses it).

use box_engine::box_script::mlua::{self, Lua, Table};
use box_engine::box_script::{with_world, with_world_mut};
use box_engine::prelude::*;

use crate::gameplay::Score;

/// The `score` script global: reads and adds to the [`Score`] resource.
pub struct ScoreApi;

impl ScriptApi for ScoreApi {
    fn name(&self) -> &str {
        "score"
    }

    fn install(&self, lua: &Lua, globals: &Table) -> mlua::Result<()> {
        let score = lua.create_table()?;
        score.set(
            "get",
            lua.create_function(|lua, ()| {
                with_world(lua, |w| w.get_resource::<Score>().map_or(0, |s| s.points))
            })?,
        )?;
        score.set(
            "add",
            lua.create_function(|lua, points: u32| {
                with_world_mut(lua, |w| w.resource_or_default::<Score>().points += points)
            })?,
        )?;
        globals.set("score", score)
    }

    fn definitions(&self) -> String {
        r#"--- The score of the game.
declare score: {
    --- The points collected so far.
    get: () -> number,
    --- Adds `points` to the score.
    add: (points: number) -> (),
}
"#
        .into()
    }

    fn doc(&self) -> &str {
        "The game's score, kept by Rust code. `score.add(5)` gives five points; \
         `score.get()` reads the total."
    }
}

/// Adds the `score` API to every app the game runs in.
pub struct ScoreApiPlugin;

impl Plugin for ScoreApiPlugin {
    fn build(&self, app: &mut App) {
        app.add_script_api(ScoreApi);
    }
}
