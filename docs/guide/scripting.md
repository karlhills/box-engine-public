# Scripting guide

Scripts make a box-engine game behave: move the player, collect coins, open menus, switch levels. They are written in [Luau](https://luau.org), a small, fast, gradually typed language derived from Lua, and they run in both the editor's Play mode and the player, with no Rust and no build step. Save a script and it reloads at once, even while the game runs.

This guide explains how scripts work and how to do common things. Every function is described in the [script API reference](../reference/README.md); this guide links to it rather than repeating it. Every code sample here is a part of `examples/guide_samples` (open it with `box-editor examples/guide_samples` and press Play), which the engine's tests play.

## Contents

- [Scripts and instances](#scripts-and-instances)
- [Properties](#properties)
- [The lifecycle](#the-lifecycle)
- [Entities and components](#entities-and-components)
- [Spawning, finding and many entities](#spawning-finding-and-many-entities)
- [Input and characters](#input-and-characters)
- [Physics](#physics)
- [Events and signals](#events-and-signals)
- [Waits and tasks](#waits-and-tasks)
- [UI and text](#ui-and-text)
- [Saving the game](#saving-the-game)
- [Data, scenes, animation, particles and sound](#data-scenes-animation-particles-and-sound)
- [Script rules](#script-rules)
- [Modules and `require`](#modules-and-require)
- [Hot reload](#hot-reload)
- [Errors and debugging](#errors-and-debugging)
- [Types and editor support](#types-and-editor-support)
- [Performance](#performance)

## Scripts and instances

A script is a `.luau` file under `assets/` that returns a **module** table. Attach it to an entity with a **Script** component (inspector ▸ Add Component ▸ Script, then pick the file as its Source). Every entity with that Script gets its own **instance**: a table called `self` that the engine passes to each callback.

```lua
-- Turns the entity around its Y axis (getting started, script modules).
local Spinner = {}

Spinner.properties = {
    speed = { type = "number", default = 90, min = 0, max = 720, unit = "deg/s", doc = "Turn rate" },
}

function Spinner.on_update(self, dt)
    self.entity:rotate_y(math.rad(self.props.speed) * dt)
end

return Spinner
```

- `self.entity` is the entity the script is on.
- `self.props` holds the property values of this entity (see below).
- Any other field you set on `self` (`self.score = 0`) is the instance's own state. It lives while the game runs; it is not saved with the scene (see [Saving the game](#saving-the-game)).
- The module's functions are shared by all instances. `self`'s metatable is the module, so you can call your own helpers as methods: `self:reset()`.
- An entity has at most one Script. To combine behaviours, write them in one file, or put scripts on separate (child) entities.

The full list of what a module may define is in [Script modules](../reference/script-modules.md).

## Properties

`properties` declares values a designer changes per entity in the inspector, without touching the code. They are saved in the scene.

```lua
Player.properties = {
    speed = { type = "number", default = 5, min = 0, max = 20, unit = "m/s", doc = "Walking speed" },
}
```

- Types: `number`, `integer`, `boolean`, `string`, `vector`, `color`, `entity` and `asset`. Leave out `type` and it is inferred from `default`; a number infers `number`, so write `type = "integer"` for whole numbers.
- `min`, `max`, `step` and `unit` shape the inspector field; `doc` is its tooltip.
- A bare value is shorthand for a default: `enabled = true`.
- In the inspector a property shows "default" until you change it; ↺ goes back to the default. Changing a default in the code updates every entity that has not overridden it.
- An `entity` property is a reference to another object in the scene; in the script it arrives as an entity handle (or nil). The Hero sample uses one to reach its model:

```lua
Hero.properties = {
    body = { type = "entity", doc = "The character model (with an AnimationStateMachine)" },
}
```

## The lifecycle

The engine calls these functions when the module defines them ([reference](../reference/script-modules.md#scriptmodule)):

| Callback | When |
|---|---|
| `on_start(self)` | Once, when the instance starts (the entity has its Script and the file has loaded). It may `wait`. |
| `on_update(self, dt)` | Every frame, after the game's native systems. `dt` is the frame's game time in seconds. |
| `on_fixed_update(self, dt)` | Every fixed step (60 per second by default), before characters move. Use it for movement and physics. |
| `on_destroy(self)` | After the entity or its Script is gone. |
| `on_trigger_enter(self, other)` / `on_trigger_exit(self, other)` | A trigger collider starts or stops overlapping another collider. Both entities are told. |
| `on_collision_enter(self, other, impulse)` / `on_collision_exit(self, other)` | A rigid body or character starts or stops touching another collider. |
| `on_save(self)` / `on_load(self, data)` | A save game is written or loaded. |
| `on_reload(self)` | The file was saved and reloaded during Play. |

Each frame runs the fixed steps first (each: physics, scripts' `on_fixed_update`, then characters move), then Update: new instances start, trigger and collision callbacks, event handlers, waiting tasks, then `on_update`. Instances run in scene order, and `math.random` starts from the same seed, so a scene plays the same way every time.

`dt` is scaled by `time.set_scale` and is 0 while `time.set_paused(true)`: use it for everything that should slow down or stop with the game. A pause menu keeps working because `on_update` and UI events still run while paused:

```lua
function Menu.on_update(self, dt)
    if input.pressed("pause") then
        time.set_paused(not time.is_paused())
    end
end
```

## Entities and components

Entities are handles with methods ([reference](../reference/entity.md)). Transform shortcuts work on the local transform, relative to the parent; `world_position` and `world_rotation` go through every parent. Positions are `vector`s (`vector.create(x, y, z)`), rotations `Quat`s, colours `Color` tables. +Y is up, -Z is forward, units are metres and radians.

Every registered component is reachable by name, with no extra code ([all components](../reference/components.md)):

- `e:get("Transform")` returns a **copy** of the component as a table. Changing the copy changes nothing.
- `e:set("Transform", { scale = ... })` changes only the fields you give. A path changes one field: `e:set("PointLight.intensity", 40)`.
- `e:add("Collider", { is_trigger = true })` adds a component (its defaults patched with your fields); `e:remove(name)` removes one; `e:has(name)` checks.
- Unknown names and fields fail with a suggestion: `unknown component 'Transfrom' (did you mean 'Transform'?)`.

```lua
function Lamp.on_start(self)
    local e = self.entity
    e:set_position(e:position() + vector.create(0, 1, 0))
    local t = e:get("Transform")               -- a copy
    e:set("Transform", { scale = t.scale * 2 })
    e:set("PointLight.intensity", 40)          -- one field
    if not e:has("Collider") then
        e:add("Collider", { is_trigger = true })
    end
end
```

Resources (one value per world, such as the `Environment` with sky, fog and exposure) work the same way with `world.resource(name)` and `world.set_resource(name, patch)` ([all resources](../reference/resources.md)).

## Spawning, finding and many entities

`world.spawn` creates an entity from a table of components; `world.find(name)` and `world.entity(uid)` find one; `world.query(...)` returns every entity that has the named components ([reference](../reference/world.md)).

```lua
function Level.on_start(self)
    world.spawn({
        name = "Marker",
        Transform = { translation = vector.create(0, 3, 0) },
        Primitive = { shape = { Sphere = { radius = 0.3 } } },
    })
    for _, e in world.query("Collider") do
        log.info(e:name(), e:world_position())
    end
    local env = world.resource("Environment")
    world.set_resource("Environment", { exposure = env.exposure * 1.2 })
end
```

When one script manages many entities, keep their handles and loop over them, instead of giving each its own script. This swarm spawns a ring of orbs under its own entity and moves them all every frame:

```lua
function Swarm.on_start(self)
    self.orbs = {}
    for i = 1, self.props.count do
        local orb = world.spawn({
            name = "Orb",
            parent = self.entity,
            Transform = { translation = util.ring(self.props.count, self.props.radius, i) },
            Primitive = { shape = { Sphere = { radius = 0.1 } } },
        })
        table.insert(self.orbs, orb)
    end
end

function Swarm.on_update(self, dt)
    local t = time.elapsed()
    for i, orb in self.orbs do
        local p = util.ring(self.props.count, self.props.radius, i)
        orb:set_position(p + vector.create(0, math.sin(t * 2 + i) * 0.3, 0))
    end
end
```

Entities spawned by scripts are runtime entities: they are not saved with the scene, and with `scenes.set_runtime_entities("despawn_unless_persistent")` a scene switch removes them unless you mark them with `scenes.persist(e)`. `world.scripts("scripts/enemy.luau")` returns the `self` tables of every instance of a script, to reach their state in one call.

## Input and characters

Read input through **actions**, named in `assets/settings/input.actions.json`, so the same script works with keyboard, mouse and gamepad ([reference](../reference/input.md)):

```json
{
  "actions": {
    "move": {
      "kind": "axis2",
      "bindings": [
        { "buttons": { "up": { "key": "W" }, "down": { "key": "S" }, "left": { "key": "A" }, "right": { "key": "D" } } },
        { "stick": "Left" }
      ]
    },
    "look": {
      "kind": "axis2",
      "bindings": [{ "stick": "Right" }]
    },
    "jump": {
      "kind": "button",
      "bindings": [{ "key": "Space" }, { "gamepad": "South" }]
    },
    "pause": {
      "kind": "button",
      "bindings": [{ "key": "P" }, { "gamepad": "Start" }]
    }
  }
}
```

`input.pressed(name)` is true in the frame the action went down, `input.held(name)` while it is down, `input.axis2(name)` gives a stick or WASD as a vector (x right, y forward). Raw keys and the mouse are there too (`input.key_pressed("F5")`, `input.mouse_delta()`, `input.lock_cursor()`).

A walking, jumping player is an entity with a **KinematicCharacter** component (capsule size, gravity, jump speed, slope and step limits) and a script that sets its velocity every fixed step ([reference](../reference/character.md)):

```lua
function Player.on_fixed_update(self, dt)
    local move = input.axis2("move")
    character.move(self.entity, vector.create(move.x, 0, -move.y) * self.props.speed)
    if input.pressed("jump") and character.jump(self.entity) then
        events.send("jumped")
    end
end
```

Use `on_fixed_update` for this: it runs just before the engine moves characters, so input applies in the same step at any frame rate. A camera that follows a moving entity should read `e:interpolated_position()` in `on_update`, which is smooth between fixed steps (give the followed entity an `Interpolated` component).

## Physics

Give an entity a **Collider** to make it solid; tick **Is Trigger** to make it a trigger area that reports overlaps instead of blocking. A **RigidBody** (Dynamic) makes it fall, bounce and get pushed. Scripts react through callbacks and ask questions through `physics` ([reference](../reference/physics.md)):

```lua
function Coin.on_trigger_enter(self, other)
    events.send("coin_collected", { value = 10 })
    events.send("PlaySound", { clip = "audio/pickup.wav", position = self.entity:world_position() })
    self.entity:despawn()
end
```

```lua
function Lift.on_start(self)
    wait(0.5)   -- queries see colliders from the next frame on
    local here = self.entity:world_position()
    local hit = physics.raycast(here, vector.create(0, -1, 0), 10, { exclude = self.entity })
    if hit then
        log.info("above", hit.entity:name(), "at", hit.distance, "m")
    end
    for _, e in physics.overlap_sphere(here, 3) do
        if e:has("RigidBody") then
            physics.apply_impulse(e, vector.create(0, 50, 0))   -- N·s: 5 m/s for 10 kg
        end
    end
end
```

- Queries see colliders as of the end of the previous frame: something spawned this frame is found from the next one.
- Pass `{ exclude = self.entity }` so a ray from inside an object does not hit the object itself.
- Collision layers (`Collider.layers` and `mask`) decide what touches what; a query's `mask` option picks which layers it sees.

## Events and signals

`events.send(name, payload)` and `events.on(name, handler)` connect scripts without them knowing each other ([reference](../reference/events.md)). A name that is a registered event type (such as `PlaySound`, see [event types](../reference/event-types.md)) reaches native code too; any other name is a **signal** between scripts. The coin above sends `coin_collected`; the score keeper listens:

```lua
function Score.on_start(self)
    self.score = 0
    events.on("coin_collected", function(payload)
        self.score += payload.value
    end)
end
```

Handlers run in the Update stage before `on_update`; a signal sent during `on_update` arrives in the next frame. A handler belongs to the instance that added it and stops when that instance does, so there is nothing to clean up.

## Waits and tasks

`wait(seconds)`, `wait_frames(n)` and `wait_until(fn, timeout)` pause the running code without blocking the game, so sequences read top to bottom instead of as state machines ([reference](../reference/core.md)). They work in `on_start` and in tasks started with `task.spawn` or `task.delay`:

```lua
function Countdown.on_start(self)
    local axis = vector.create(0, 1, 0)
    local turn = Quat.from_axis_angle(axis, math.rad(90))
    local ahead = turn * vector.create(0, 0, -1)   -- (-1, 0, 0)
    log.info("ahead is", ahead, "tint", Color.hex("#ff8800").r)

    task.spawn(function()
        for i = 3, 1, -1 do
            log.info("starting in", i)
            wait(1)
        end
        log.info("go!")
    end)
end
```

Waits count game time: pausing or slowing time pauses or slows them. Calling `wait` inside `on_update` or an event handler fails, because those must finish within the frame; start a task there instead. `task.cancel(id)` stops a task; tasks also stop with their instance.

## UI and text

The game UI is built from documents (tables, or the same as JSON) and changed through setters when something happens, not redrawn every frame ([reference](../reference/ui.md)):

```lua
function Hud.on_start(self)
    ui.add({
        kind = "panel", id = "hud", anchor = { 0, 0 }, offset = { 24, 24 },
        children = {
            { kind = "text", id = "score", key = "hud.score", args = { score = 0 } },
        },
    })
    ui.set_arg("score", "score", 10)

    local menu = ui.add({
        kind = "panel", id = "pause", modal = true, hidden = true, anchor = { 0.5, 0.5 },
        children = {
            { kind = "button", id = "resume", text = "Resume" },
            { kind = "button", id = "quit", text = "Quit" },
        },
    })
    ui.on("click", "resume", function()
        ui.set_visible(menu, false)
        time.set_paused(false)
    end)
end
```

- Nodes have kinds (`panel`, `row`, `column`, `text`, `button`, `slider`, `toggle`, `image`, `progress`, `list` and more), an optional `id` you use to find them, and roots are placed with `anchor` (0 to 1 across the screen) and `offset`.
- Any function that takes a node also takes its `id`: `ui.set_arg("score", "score", 10)`.
- A `modal` root takes keyboard and gamepad focus and keeps gameplay input from the game while it is visible.
- `ui.on(kind, id, handler)` reacts to `click`, `change` (sliders), `toggle`, `focus`, `blur` and `back`.
- Text comes from **string tables** when a node has a `key`: `assets/strings/en.strings.json` holds `{ "hud": { "score": "Score {score}" } }`, and `{score}` is filled from `args`. `strings.set_language("de")` switches every keyed text to `de.strings.json` ([reference](../reference/strings.md)).
- `ui.set_theme("ui/game.theme.json")` loads colours and styles; theme files hot-reload.

The score keeper shows its score in the HUD's text node. Setters change nothing (and cost little) when the value is the same, so this is cheap:

```lua
function Score.on_update(self, dt)
    -- A setter changes nothing (and costs little) when the value is the same.
    if ui.contains("score") then
        ui.set_arg("score", "score", self.score)
    end
end
```

## Saving the game

`save.save(slot)` writes the running world to `<project>/.box/saves/<slot>.save.json`, `save.load(slot)` restores it ([reference](../reference/save.md)). Entities from scenes are saved with their components automatically, and so are properties. A script's own state in `self` is saved only if the script returns it from `on_save`:

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
```

After a load, each instance starts again and `on_load` runs once its `on_start` has returned (waits included), so set defaults in `on_start` and restore them in `on_load`; keep endless loops in `task.spawn`. Entities spawned by scripts are saved only when marked with `save.include(e)`; `save.exclude(e)` leaves something out, and `world.spawn{ transient = true }` makes runtime-only entities that are never saved and are removed when a save loads.

## Data, scenes, animation, particles and sound

- **Data tables**: put tuning numbers, items and dialogue in `assets/**/*.data.json` and read them with `data.table(path)` (nil until imported, so wait for it in `on_start`). They hot-reload. ([reference](../reference/data.md))

```lua
function Spawner.on_start(self)
    local enemies = nil
    wait_until(function()
        enemies = data.table("data/enemies.data.json")
        return enemies ~= nil
    end)
    log.info("slime hp", enemies.slime.hp)
end
```

- **Scenes**: `scenes.switch("level2")` replaces the level at the start of the next frame; `scenes.load_additive(name)` adds one next to it ([reference](../reference/scenes.md)).

```lua
function Goal.on_trigger_enter(self, other)
    if other:name() == "Player" then
        scenes.switch("level2")
    end
end
```

- **Animation**: a glTF character with an **AnimationStateMachine** (`*.animgraph.json`) is steered by parameters; the graph picks clips and crossfades ([reference](../reference/animation.md)).

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
```

- **Particles**: `particles.burst(position, count, options)` for one-off effects; emitters placed in the editor can be started and stopped ([reference](../reference/particles.md)).
- **Sound**: send a `PlaySound` event (as the coin does) or give an entity an **AudioSource**. Volumes per bus are in the `AudioSettings` resource.

## Script rules

Scripts can also generate geometry while you edit: a **rule** file turns intents (entities with a `ScriptIntent` naming the file) into parts such as fence posts or columns. Rules run in the editor as you change their inputs, are pure functions with no access to `world` or `input`, and have their own API ([reference](../reference/rules.md)). The [procedural guide](procedural.md) explains them; the smallest rule makes a row of posts:

```lua
local Row = {}
Row.properties = { count = { type = "integer", default = 3, min = 1, max = 20 } }

function Row.rule(intents: { RuleIntent }, out: RuleOut)
    for _, it in intents do
        for i = 1, it.props.count do
            out:part(it, `post{i}`, { Primitive = {} }, { position = vector.create(i * 2, 0.5, 0) })
        end
    end
end

return Row
```

## Modules and `require`

Share code between scripts with `require`. A path starting with `./` or `../` is relative to the requiring file; any other path is relative to `assets/`. The `.luau` extension is optional.

```lua
-- Shared helpers, loaded with `require("./util")` (scripting guide).
local util = {}

-- The point `i` of `count` points spread evenly on a circle.
function util.ring(count: number, radius: number, i: number): vector
    local a = (i / count) * math.pi * 2
    return vector.create(math.cos(a) * radius, 0, math.sin(a) * radius)
end

return util
```

```lua
local util = require("./util")
```

A module is loaded once and shared; editing it reloads every script that requires it. Scripts can only require files inside `assets/`, and two modules may not require each other.

## Hot reload

Save a `.luau` file while the game runs (in the editor's Play mode, or in the player) and it reloads:

- Instances keep `self` and their properties; the new functions apply from the next call.
- An instance that had failed comes back.
- If the module defines `on_reload(self)`, the instance's tasks and handlers are cancelled first and `on_reload` runs, so restart what you need there. Without it, running tasks finish with their old code.
- A file that does not load (a syntax error) keeps the previous version running, with the error in the Console.

Data tables, string tables, themes, materials, textures and models hot-reload too.

## Errors and debugging

When a callback fails, the error is logged with the file and line, the entity's script is disabled until you save the file again, and everything else keeps running. In the editor the error shows as a toast and in the Console; the player prints it:

```text
ERROR box_script: scripts/bad.luau:3: unknown component `Transfrom` (did you mean `Transform`?)
  in on_start of "Broken" (scripts/bad.luau); this entity's script is disabled until the file is reloaded
```

- Errors raised by engine functions point at your calling line, and name what is wrong (an unknown component or field, a missing component such as "the entity has no KinematicCharacter", an entity that no longer exists).
- `print(...)` and `log.info/warn/error(...)` write to the Console (and the player's terminal), prefixed with the script's path. `log.warn` and `log.error` also show a toast in the editor.
- During Play, the inspector shows live component values and script properties, and you can pause (Cmd+Shift+P) and step frames (Cmd+Alt+P).
- A script stage that runs longer than 5 seconds (an endless loop) is interrupted, so a mistake cannot hang the editor.
- Scripts run in a sandbox: no file, network or process access (`io`, `os.execute`, `loadstring` are not there). Use `data`, `save` and `require` for files.
- There is no breakpoint debugger yet.

## Types and editor support

`box script types --project <dir>` writes `.box/types/box.d.luau`, the type definitions of every API and component, and points VS Code's luau-lsp extension at it. You then get completion, hover documentation (the same text as the reference) and type checking. Type annotations are optional; add `--!strict` at the top of a file for full checking. `e:get("Transform")` is typed per component, so mistakes in field names show up while you type.

## Performance

Scripts decide what happens; the engine does the heavy work. Measured on an Apple M3 Ultra (release build): 10,000 `on_update` callbacks take about 0.6 ms per frame, a loop over 5,000 queried entities writing a position each about 0.5 ms, a transform method call about 0.07 µs and a whole-component copy (`e:get`) about 0.5 µs.

- Prefer the transform methods (`position`, `set_position`, `rotate_y`) to `get`/`set` of a whole `Transform`, and `set` with a path or a few fields to copying whole components.
- For many similar objects, one script that loops over a list (or `world.query`) beats a script per object.
- Find entities once (in `on_start`) and keep the handles, instead of `world.find` every frame.
- Change UI with setters only when something changes.
- Put work that does not need every frame on a timer: `task.spawn` with `wait(0.5)` in a loop.
- Mesh generation, pathfinding and other heavy loops belong in native code ([native plugins](native-plugins.md)), exposed to scripts as an API.
