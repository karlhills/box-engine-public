//! `camera`: the active camera's projection for scripts (world points to
//! screen pixels, screen pixels and the cursor to world rays), through
//! [`crate::world_to_screen`], [`crate::screen_ray`] and [`crate::cursor_ray`].

use box_core::math::Ray;
use box_script::convert::{from_vector, vector};
use box_script::mlua::{Lua, Result as LuaResult, Table, Vector};
use box_script::{ScriptApi, with_world};
use glam::Vec2;

use crate::runner::WindowSize;

pub struct CameraApi;

fn ray_values(ray: Option<Ray>) -> (Option<Vector>, Option<Vector>) {
    match ray {
        Some(r) => (Some(vector(r.origin)), Some(vector(r.direction))),
        None => (None, None),
    }
}

impl ScriptApi for CameraApi {
    fn name(&self) -> &str {
        "camera"
    }

    fn install(&self, lua: &Lua, g: &Table) -> LuaResult<()> {
        let t = lua.create_table()?;
        t.set(
            "world_to_screen",
            lua.create_function(|lua, p: Vector| {
                with_world(lua, |w| {
                    crate::world_to_screen(w, from_vector(p))
                        .map(|s| Vector::new(s.pixels.x, s.pixels.y, s.depth))
                })
            })?,
        )?;
        t.set(
            "screen_ray",
            lua.create_function(|lua, p: Vector| {
                with_world(lua, |w| {
                    ray_values(crate::screen_ray(w, Vec2::new(p.x(), p.y())))
                })
            })?,
        )?;
        t.set(
            "cursor_ray",
            lua.create_function(|lua, ()| with_world(lua, |w| ray_values(crate::cursor_ray(w))))?,
        )?;
        t.set(
            "screen_size",
            lua.create_function(|lua, ()| {
                with_world(lua, |w| {
                    let s = w.get_resource::<WindowSize>().copied().unwrap_or_default();
                    Vector::new(s.width as f32, s.height as f32, 0.0)
                })
            })?,
        )?;
        g.set("camera", t)
    }

    fn definitions(&self) -> String {
        r#"--- The active camera's projection: world points to screen pixels and screen points to world rays (picking).
declare camera: {
    --- Where world point `position` appears through the active camera: x and y in physical pixels from the top left (as `input.cursor_position()`), z its distance in front of the camera in metres. Nil when it is behind the camera or there is no active camera. A point beside the view is returned with x or y outside `camera.screen_size()`.
    world_to_screen: (position: vector) -> vector?,
    --- The world-space ray from the active camera through screen point `position` (physical pixels from the top left; z is ignored): its origin and normalized direction, or nil, nil without an active camera. Cast it with `physics.raycast`.
    screen_ray: (position: vector) -> (vector?, vector?),
    --- The ray under the mouse cursor (`camera.screen_ray(input.cursor_position())`), or nil, nil when the cursor is outside the window.
    cursor_ray: () -> (vector?, vector?),
    --- The size of the game view in physical pixels (z is 0).
    screen_size: () -> vector,
}
"#
        .into()
    }

    fn doc(&self) -> &str {
        r#"The camera the game is drawn with is the first active `Camera` entity. `camera` converts between the world and its screen with the renderer's own projection: pick what is under the mouse with `camera.cursor_ray()` and `physics.raycast`, or find where a world point is drawn with `camera.world_to_screen`. Screen positions are physical pixels from the top left, like `input.cursor_position()`. For UI that follows an entity, anchor a node to it (`ui.anchor_to_entity`) instead.

```lua
function Pick.on_update(self, dt)
    -- A click picks what is under the cursor.
    if input.mouse_pressed("Left") then
        local origin, direction = camera.cursor_ray()
        if origin and direction then
            local hit = physics.raycast(origin, direction, 100)
            if hit then
                log.info("clicked", hit.entity:name(), "at", hit.point)
            end
        end
    end
    -- Where the ball is drawn: nil behind the camera, outside the size beside it.
    local screen = if self.ball then camera.world_to_screen(self.ball:world_position()) else nil
    local size = camera.screen_size()
    self.ball_visible = screen ~= nil
        and screen.x >= 0 and screen.x <= size.x
        and screen.y >= 0 and screen.y <= size.y
end
```"#
    }
}
