//! `time`, `app` and `input`.

use box_core::action::Actions;
use box_core::app::{AppExit, FixedTime};
use box_core::input::{CursorOptions, Input, KeyCode, MouseButton};
use box_core::time::Time;
use mlua::{Lua, Result as LuaResult, Table, Vector};

use super::{ScriptApi, with_world, with_world_mut};
use crate::convert::vector;

pub(crate) struct TimeApi;
pub(crate) struct InputApi;

fn time<R>(lua: &Lua, f: impl FnOnce(&Time) -> R) -> LuaResult<R> {
    with_world(lua, |w| w.get_resource::<Time>().map(f))?
        .ok_or_else(|| mlua::Error::runtime("the world has no Time"))
}

impl ScriptApi for TimeApi {
    fn name(&self) -> &str {
        "time"
    }

    fn install(&self, lua: &Lua, g: &Table) -> LuaResult<()> {
        let t = lua.create_table()?;
        t.set(
            "delta",
            lua.create_function(|lua, ()| time(lua, |t| t.delta))?,
        )?;
        t.set(
            "elapsed",
            lua.create_function(|lua, ()| time(lua, |t| t.elapsed_f64))?,
        )?;
        t.set(
            "frame",
            lua.create_function(|lua, ()| time(lua, |t| t.frame))?,
        )?;
        t.set(
            "scale",
            lua.create_function(|lua, ()| time(lua, |t| t.scale))?,
        )?;
        t.set(
            "set_scale",
            lua.create_function(|lua, s: f32| {
                with_world_mut(lua, |w| {
                    if let Some(t) = w.get_resource_mut::<Time>() {
                        t.scale = s.max(0.0);
                    }
                })
            })?,
        )?;
        t.set(
            "is_paused",
            lua.create_function(|lua, ()| time(lua, |t| t.paused))?,
        )?;
        crate::api::deprecated_alias(lua, &t, "time", "paused", "is_paused")?;
        t.set(
            "set_paused",
            lua.create_function(|lua, p: bool| {
                with_world_mut(lua, |w| {
                    if let Some(t) = w.get_resource_mut::<Time>() {
                        t.paused = p;
                    }
                })
            })?,
        )?;
        t.set(
            "fixed_step",
            lua.create_function(|lua, ()| {
                with_world(lua, |w| {
                    w.get_resource::<FixedTime>().map_or(1.0 / 60.0, |f| f.step)
                })
            })?,
        )?;
        g.set("time", t)
    }

    fn definitions(&self) -> String {
        r#"--- Game time: the frame's delta, elapsed time, time scale and pause.
declare time: {
    --- Seconds of game time this frame (scaled; 0 while paused). Inside `on_fixed_update` it is the fixed step.
    delta: () -> number,
    --- Seconds of game time since the game started (scaled, stops while paused).
    elapsed: () -> number,
    --- The number of frames since the game started.
    frame: () -> number,
    --- The time scale: 1 is normal speed, 0.5 half speed.
    scale: () -> number,
    --- Sets the time scale (clamped at 0). Slows or speeds up `dt`, waits, physics and animation.
    set_scale: (scale: number) -> (),
    --- True while game time is paused.
    is_paused: () -> boolean,
    --- Deprecated: use `is_paused`.
    paused: () -> boolean,
    --- Pauses or resumes game time: `dt` becomes 0, fixed steps and waits stop, `on_update` still runs (for menus).
    set_paused: (paused: boolean) -> (),
    --- The fixed step in seconds (1/60 by default): the `dt` of `on_fixed_update`.
    fixed_step: () -> number,
}
"#
        .into()
    }

    fn doc(&self) -> &str {
        r#"Game time. Gameplay should use the `dt` its callback receives (or `time.delta()`), so `time.set_scale` and `time.set_paused` slow down, speed up or stop it; a pause menu keeps working because `on_update` and UI events still run.

```lua
function Menu.on_update(self, dt)
    if input.pressed("pause") then
        time.set_paused(not time.is_paused())
    end
end
```"#
    }
}

/// `app.quit()`: asks the runner to quit after this frame (`AppExit`); in
/// the editor it stops Play.
pub(crate) struct AppApi;

impl ScriptApi for AppApi {
    fn name(&self) -> &str {
        "app"
    }

    fn install(&self, lua: &Lua, g: &Table) -> LuaResult<()> {
        let t = lua.create_table()?;
        t.set(
            "quit",
            lua.create_function(|lua, ()| {
                with_world_mut(lua, |w| w.insert_resource(AppExit(true)))
            })?,
        )?;
        g.set("app", t)
    }

    fn definitions(&self) -> String {
        r#"--- The running application.
declare app: {
    --- Asks the player to exit after this frame. In the editor it stops Play.
    quit: () -> (),
}
"#
        .into()
    }

    fn doc(&self) -> &str {
        "The application that runs the game: the player, or Play mode in the editor.\n\n```lua\nfunction Menu.on_start(self)\n    ui.on(\"click\", \"quit\", function() app.quit() end)\nend\n```"
    }
}

fn actions<R>(lua: &Lua, f: impl FnOnce(&Actions) -> R) -> LuaResult<Option<R>> {
    with_world(lua, |w| w.get_resource::<Actions>().map(f))
}

fn key(name: &str) -> LuaResult<KeyCode> {
    serde_json::from_value(serde_json::Value::String(name.to_owned())).map_err(|_| {
        mlua::Error::runtime(format!(
            "unknown key `{name}` (KeyCode names: \"Space\", \"W\", \"Escape\", \"Up\")"
        ))
    })
}

fn button(name: &str) -> LuaResult<MouseButton> {
    serde_json::from_value(serde_json::Value::String(name.to_owned())).map_err(|_| {
        mlua::Error::runtime(format!(
            "unknown mouse button `{name}` (\"Left\", \"Right\", \"Middle\")"
        ))
    })
}

fn input<R>(lua: &Lua, f: impl FnOnce(&Input) -> R) -> LuaResult<Option<R>> {
    with_world(lua, |w| w.get_resource::<Input>().map(f))
}

impl ScriptApi for InputApi {
    fn name(&self) -> &str {
        "input"
    }

    fn install(&self, lua: &Lua, g: &Table) -> LuaResult<()> {
        let t = lua.create_table()?;
        t.set(
            "pressed",
            lua.create_function(|lua, a: String| {
                Ok(actions(lua, |x| x.pressed(&a))?.unwrap_or(false))
            })?,
        )?;
        t.set(
            "held",
            lua.create_function(|lua, a: String| {
                Ok(actions(lua, |x| x.held(&a))?.unwrap_or(false))
            })?,
        )?;
        t.set(
            "released",
            lua.create_function(|lua, a: String| {
                Ok(actions(lua, |x| x.released(&a))?.unwrap_or(false))
            })?,
        )?;
        t.set(
            "axis1",
            lua.create_function(
                |lua, a: String| Ok(actions(lua, |x| x.axis1(&a))?.unwrap_or(0.0)),
            )?,
        )?;
        t.set(
            "axis2",
            lua.create_function(|lua, a: String| {
                let v = actions(lua, |x| x.axis2(&a))?.unwrap_or_default();
                Ok(Vector::new(v.x, v.y, 0.0))
            })?,
        )?;
        t.set(
            "key_down",
            lua.create_function(|lua, k: String| {
                let k = key(&k)?;
                Ok(input(lua, |i| i.key_down(k))?.unwrap_or(false))
            })?,
        )?;
        t.set(
            "key_pressed",
            lua.create_function(|lua, k: String| {
                let k = key(&k)?;
                Ok(input(lua, |i| i.key_pressed(k))?.unwrap_or(false))
            })?,
        )?;
        t.set(
            "key_released",
            lua.create_function(|lua, k: String| {
                let k = key(&k)?;
                Ok(input(lua, |i| i.key_released(k))?.unwrap_or(false))
            })?,
        )?;
        t.set(
            "mouse_down",
            lua.create_function(|lua, b: String| {
                let b = button(&b)?;
                Ok(input(lua, |i| i.button_down(b))?.unwrap_or(false))
            })?,
        )?;
        t.set(
            "mouse_pressed",
            lua.create_function(|lua, b: String| {
                let b = button(&b)?;
                Ok(input(lua, |i| i.button_pressed(b))?.unwrap_or(false))
            })?,
        )?;
        t.set(
            "mouse_released",
            lua.create_function(|lua, b: String| {
                let b = button(&b)?;
                Ok(input(lua, |i| i.button_released(b))?.unwrap_or(false))
            })?,
        )?;
        t.set(
            "mouse_delta",
            lua.create_function(|lua, ()| {
                let d = input(lua, |i| i.mouse_delta)?.unwrap_or_default();
                Ok(Vector::new(d.x, d.y, 0.0))
            })?,
        )?;
        t.set(
            "cursor_position",
            lua.create_function(|lua, ()| {
                Ok(input(lua, |i| i.cursor_position)?
                    .flatten()
                    .map(|p| vector(p.extend(0.0))))
            })?,
        )?;
        t.set(
            "lock_cursor",
            lua.create_function(|lua, ()| {
                with_world_mut(lua, |w| w.resource_or_default::<CursorOptions>().lock())
            })?,
        )?;
        t.set(
            "release_cursor",
            lua.create_function(|lua, ()| {
                with_world_mut(lua, |w| w.resource_or_default::<CursorOptions>().release())
            })?,
        )?;
        t.set(
            "is_cursor_locked",
            lua.create_function(|lua, ()| {
                with_world(lua, |w| {
                    w.get_resource::<CursorOptions>()
                        .is_some_and(CursorOptions::is_locked)
                })
            })?,
        )?;
        crate::api::deprecated_alias(lua, &t, "input", "cursor_locked", "is_cursor_locked")?;
        g.set("input", t)
    }

    fn definitions(&self) -> String {
        r#"--- Player input: named actions (keyboard, mouse and gamepad bindings), raw keys and mouse, cursor lock.
declare input: {
    --- True in the frame (or fixed step, inside `on_fixed_update`) where the action became held. Unknown action names read as not pressed and are logged once.
    pressed: (action: string) -> boolean,
    --- True while any binding of the action is held (an axis action: while it is not zero).
    held: (action: string) -> boolean,
    --- True in the frame (or fixed step) where the action stopped being held.
    released: (action: string) -> boolean,
    --- A one-axis action's value from -1 to 1 (a button action reads 1 while held).
    axis1: (action: string) -> number,
    --- A two-axis action (movement, look) as a vector with length at most 1: x is right, y is up or forward, z is 0.
    axis2: (action: string) -> vector,
    --- True while the key is held. Key names: "A" to "Z", "Digit0" to "Digit9", "Space", "Enter", "Escape", "Tab", "Up", "Down", "Left", "Right", "ShiftLeft", "ControlLeft", "F1" and so on.
    key_down: (key: string) -> boolean,
    --- True in the frame the key went down.
    key_pressed: (key: string) -> boolean,
    --- True in the frame the key went up.
    key_released: (key: string) -> boolean,
    --- True while the mouse button ("Left", "Right" or "Middle") is held.
    mouse_down: (button: string) -> boolean,
    --- True in the frame the mouse button went down.
    mouse_pressed: (button: string) -> boolean,
    --- True in the frame the mouse button was let go.
    mouse_released: (button: string) -> boolean,
    --- The raw mouse movement this frame (device units, x right, y down), for mouse look; it keeps reporting while the cursor is locked.
    mouse_delta: () -> vector,
    --- The cursor position in the window in physical pixels (from the top left; z is 0), or nil when the cursor is outside it.
    cursor_position: () -> vector?,
    --- Hides the cursor and locks it to the window (for mouse look).
    lock_cursor: () -> (),
    --- Shows and frees the cursor again.
    release_cursor: () -> (),
    --- True while the cursor is locked.
    is_cursor_locked: () -> boolean,
    --- Deprecated: use `is_cursor_locked`.
    cursor_locked: () -> boolean,
}
"#
        .into()
    }

    fn doc(&self) -> &str {
        r#"Input through named actions, so a script works with any keyboard, mouse or gamepad binding. Actions are defined in `assets/settings/input.actions.json` (or by a native plugin's `ActionMap`); an action is a `button`, an `axis1` or an `axis2`:

```json
{
  "actions": {
    "move": { "kind": "axis2", "bindings": [
      { "buttons": { "up": { "key": "W" }, "down": { "key": "S" }, "left": { "key": "A" }, "right": { "key": "D" } } },
      { "stick": "Left" } ] },
    "jump": { "kind": "button", "bindings": [{ "key": "Space" }, { "gamepad": "South" }] }
  }
}
```

`pressed` and `released` fire once per frame in `on_update` and once per fixed step in `on_fixed_update` (a press in a frame without a fixed step reaches the next step), so read them where you act on them.

```lua
function Look.on_update(self, dt)
    local look = input.axis2("look")             -- x right, y up
    self.entity:rotate_y(-look.x * 2 * dt)
    if input.mouse_pressed("Left") then
        input.lock_cursor()
    elseif input.key_pressed("Escape") then
        input.release_cursor()
    end
end
```"#
    }
}
