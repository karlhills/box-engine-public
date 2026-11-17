//! `data`: data tables (`*.data.json`) as Luau tables.
//!
//! ```lua
//! local enemies = data.table("data/enemies.data.json")   -- nil until imported
//! if enemies then print(enemies.slime.hp) end
//! ```
//!
//! The table is converted once per file version (hot reload makes a new
//! one) and is read-only: copy what you want to change.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use box_script::convert::json_to_lua;
use box_script::mlua::{Lua, Result as LuaResult, Table, Value};
use box_script::{ScriptApi, with_world_mut};

use crate::data::{data_table, data_version};

pub struct DataApi;

/// Freezes a converted table and its nested tables.
fn freeze(v: &Value) {
    if let Value::Table(t) = v {
        for (_, inner) in t.pairs::<Value, Value>().flatten() {
            freeze(&inner);
        }
        t.set_readonly(true);
    }
}

impl ScriptApi for DataApi {
    fn name(&self) -> &str {
        "data"
    }

    fn install(&self, lua: &Lua, g: &Table) -> LuaResult<()> {
        let t = lua.create_table()?;
        // Converted tables by path, with the file version they came from.
        let cache: Rc<RefCell<HashMap<String, (u64, Value)>>> = Rc::default();
        t.set(
            "table",
            lua.create_function(move |lua, path: String| {
                let found = with_world_mut(lua, |w| {
                    let version = data_version(w, &path)?;
                    if let Some((v, value)) = cache.borrow().get(&path)
                        && *v == version
                    {
                        return Some(Ok(value.clone()));
                    }
                    let json = data_table(w, &path)?.json().clone();
                    Some(json_to_lua(lua, &json).inspect(|value| {
                        freeze(value);
                        cache
                            .borrow_mut()
                            .insert(path.clone(), (version, value.clone()));
                    }))
                })?;
                found.transpose().map(|v| v.unwrap_or(Value::Nil))
            })?,
        )?;
        t.set(
            "version",
            lua.create_function(|lua, path: String| {
                with_world_mut(lua, |w| data_version(w, &path))
            })?,
        )?;
        g.set("data", t)
    }

    fn definitions(&self) -> String {
        r#"--- Data tables: JSON files (`*.data.json`) as read-only Luau tables.
declare data: {
    --- The data table at `path` (assets-relative, such as "data/enemies.data.json") as a read-only Luau table, or nil until the file is imported (the first call requests it, so check for nil in the first frames). The same table is returned until the file changes; copy what you want to change.
    table: (path: string) -> any,
    --- The table's version: it changes on every hot reload, so a script can rebuild what it derived from the table only when needed. Nil until imported.
    version: (path: string) -> number?,
}
"#
        .into()
    }

    fn doc(&self) -> &str {
        r#"Tunable numbers, items, dialogue and other data live in `assets/**/*.data.json` files, edited by hand or by tools and hot-reloaded during Play. Scripts read them as Luau tables.

```json
{ "slime": { "hp": 3, "speed": 1.5 }, "bat": { "hp": 1, "speed": 4 } }
```

```lua
function Spawner.on_start(self)
    local enemies = nil
    wait_until(function()
        enemies = data.table("data/enemies.data.json")
        return enemies ~= nil
    end)
    log.info("slime hp", enemies.slime.hp)
end
```"#
    }
}
