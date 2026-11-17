# Native plugins

A native box-engine project keeps its game code in a Rust crate next to its scenes and assets. The crate builds the game's own player and editor, so your components show in the inspector, your systems run in Play mode and the player, and your tools appear in the editor. This guide shows how to write that code: plugins, saved components, systems, events, input, scenes, script APIs and editor actions, and how to test it all headlessly.

Every Rust sample below is copied from `examples/guide_native` in the engine repository, a small native project (a player who walks over glowing pickups) that builds and is tested. Open it next to this guide; `cargo run -p guide_native --bin guide-player` plays it.

If you have not made a project yet, start with [Getting started](getting-started.md). For scenes and the editor itself, see [The editor](editor.md).

## Contents

- [Native or Luau?](#native-or-luau)
- [Create a native project](#create-a-native-project)
- [Run the player and the editor](#run-the-player-and-the-editor)
- [The game plugin](#the-game-plugin)
- [Saved components](#saved-components)
- [Resources and project settings](#resources-and-project-settings)
- [Systems and stages](#systems-and-stages)
- [Hooks: reacting to new and removed components](#hooks-reacting-to-new-and-removed-components)
- [Events](#events)
- [Input actions](#input-actions)
- [Physics, characters, timers and tweens](#physics-characters-timers-and-tweens)
- [Scenes at runtime](#scenes-at-runtime)
- [Scripts in a native project](#scripts-in-a-native-project)
- [A script API in Rust](#a-script-api-in-rust)
- [Editor actions and tools](#editor-actions-and-tools)
- [Generators and rules](#generators-and-rules)
- [How `box` and agents see your types](#how-box-and-agents-see-your-types)
- [Rebuild and Restart](#rebuild-and-restart)
- [Optional engine modules](#optional-engine-modules)
- [Testing headlessly](#testing-headlessly)
- [Where next](#where-next)

## Native or Luau?

box-engine has two ways to write game logic, and one project can use both.

| | Native (Rust) | Luau scripts |
|---|---|---|
| Code lives in | A Cargo crate with a `Plugin` | `.luau` files under `assets/` |
| Needs | A Rust toolchain and a box-engine checkout | Nothing: the stock `box-editor` and `box-player` run it |
| Runs in | The project's own editor and player binaries | Any editor or player |
| After a change | Rebuild and restart (the editor does it for you) | Hot reload on save, even during Play |
| Good for | Heavy computation, generators and rules, editor tools, new engine-level features | Gameplay rules, interaction, UI flow, small games |

Choose native when you need Rust's speed or the whole engine API, want custom editor tools, or are writing a reusable module. Choose Luau ([Scripting](scripting.md)) when fast iteration matters most and the script API covers what you need.

Mixing is normal: a native project plays scenes with `Script` components like any other project, and Rust code can add script APIs, events and component types that scripts use by name. A common split is "systems and data types in Rust, level-specific behaviour in Luau". See [Scripts in a native project](#scripts-in-a-native-project).

## Create a native project

```bash
box new my-game --native
```

From an engine checkout without installing `box`, run `cargo run -p box_cli -- new my-game --native`. The crate depends on the engine by path: on the checkout `box` was built from, or on the one you name with `--engine-path ../box-engine`. On an existing project, `box new <dir> --native` adds only the crate.

Besides the usual `box.project.json` and `assets/scenes/main.scene.json`, it writes:

| File | What it is |
|---|---|
| `Cargo.toml` | The game crate: path dependencies on `box_engine` and (optional) `box_editor`, the engine module features, an `editor` feature, two binaries and the engine's build profile. It is a workspace of its own. |
| `src/lib.rs` | The game plugin: a `Spin` component, a system that turns entities with it, an `on_add` hook, an `on_event` handler for `SceneEvent`, the `MyGamePlugin` plugin and `pub fn game() -> Game`. |
| `src/bin/player.rs` | The player: plays this project's start scene (or the project given) with `game()`. |
| `src/bin/editor.rs` | The editor (needs the `editor` feature): box-editor with this game's types and systems. |
| `.cargo/config.toml` | The aliases `cargo player` and `cargo editor`. |
| `rust-toolchain.toml`, `Cargo.lock` | Copied from the engine, so the game builds with the same Rust and dependency versions. |

`target/` is added to `.gitignore` (which already ignores `.box/`, the folder of local and generated files).

The plugin type is named after the project: `my-game` gives `MyGamePlugin`, `Space Race` gives `SpaceRacePlugin`.

## Run the player and the editor

In the project folder:

```bash
cargo player                      # play the start scene in a window
cargo player -- --headless --frames 60   # 60 frames, no window
cargo editor                      # open this project in the game's editor
```

`cargo player` is `cargo run --bin player` and `cargo editor` is `cargo run --features editor --bin editor`. The `editor` feature is off by default, so the player builds without the editor's dependencies. The first build compiles the engine and takes a few minutes; later builds are incremental.

`box run` works too: for a native project it runs the game's player through cargo, so your types and systems are included:

```bash
box run my-game                   # a window
box run my-game --frames 10       # headless
box run my-game --no-cargo        # print the cargo command instead of running it
```

The player takes the usual runner flags: `--scene <name>`, `--capture <png>`, `--frames <n>`, `--headless`, `--size <W>x<H>`, `--report <json>`, `--control`. Run it with `--help` for the list.

The stock `box-player` refuses a native project and prints the command that runs the game's player (`--force-engine-only` plays it with the engine's types only). The stock `box-editor` opens it, keeps the data of your components without running them, and warns that they belong to the game's code: use `cargo editor` instead.

## The game plugin

A game is one or more plugins. A plugin has two parts:

- `register` adds the types that scenes save and the inspector shows. It runs once, when the registry is built.
- `build` adds resources, events, systems and hooks. It runs for every app the game runs in: the player, each Play session in the editor, each test app.

The sample's plugin, which the rest of this guide takes apart:

```rust
/// The gameplay plugin.
pub struct GameplayPlugin;

impl Plugin for GameplayPlugin {
    fn register(&self, registry: &mut Registry) {
        // Errors (a clashing name, a missing attribute) are logged by
        // `Game::with_plugin` with this plugin's name.
        registry.register_all::<(Mover, Pickup)>();
        let _ = registry.register_settings::<Tuning>();
        let _ = registry.register_event::<PickedUp>();
    }

    fn build(&self, app: &mut App) {
        app.world.resource_or_default::<Score>();
        // `assets/settings/input.actions.json` overrides these per action.
        app.world
            .resource_or_default::<ActionMap>()
            .merge_missing(&default_actions());
        app.add_event::<PickedUp>();
        app.system(Stage::FixedUpdate, move_player)
            .label("move_player");
        app.system(Stage::FixedUpdate, collect_pickups)
            .after("move_player");
        app.on_event(Stage::Update, add_score);
        app.on_add::<Mover>(|world, e| {
            // Moved in FixedUpdate: drawn between steps.
            let _ = world.insert_one(e, Interpolated::default());
        });
        app.on_add::<Pickup>(add_bob);
    }
}
```

`Game` collects the plugins. The player, the editor and tests all start from the same `game()` function:

```rust
/// The game as the player, the editor and tests run it.
pub fn game() -> Game {
    Game::new()
        .with_plugin(gameplay::GameplayPlugin)
        .with_plugin(script_api::ScoreApiPlugin)
        .with_plugin(procedural::ProceduralSamplesPlugin)
}
```

Plugins build in the order they are added, after the engine's own plugins (input, scenes, assets, physics, audio, UI and so on), and before the Luau script runtime. Split a game into several plugins when it grows; each can register its own types.

Everything a game needs comes from one import, `use box_engine::prelude::*;`. It includes `bevy_reflect` for `#[derive(Reflect)]` and the log macros `info!`, `warn!` and `error!`, so the crate needs no other dependencies. Everything you log appears in the editor's Console, and warnings and errors are counted on its tab. The editor turns warnings and errors into toasts only for log targets starting with `box` (the engine's crates); give a message such a target to make it a toast: `warn!(target: "box_game", "...")`. In a terminal only warnings and errors from your crate are shown by default; `RUST_LOG=info` shows the rest.

The binaries are a few lines each. The sample's player:

```rust
fn play() -> anyhow::Result<()> {
    let mut options = box_engine::player::PlayerOptions::from_args(std::env::args().skip(1))?;
    if options.project.is_none() {
        options.project = Some(guide_native::project_dir());
    }
    box_engine::player::run_game(options, guide_native::game())?;
    Ok(())
}
```

`Game::run("title")` runs a code-only game (no project or scene files) in a window, or headless with the runner flags.

## Saved components

A component that scenes save and the inspector edits is a reflected Rust type, registered in `register`:

```rust
/// The player: walks on the ground with the `move` action.
#[derive(Reflect, Clone)]
#[reflect(Component, Default, Clone)]
pub struct Mover {
    /// Walking speed.
    #[reflect(@Range { min: 0.0, max: 20.0 }, @Unit("m/s"))]
    pub speed: f32,
}

impl Default for Mover {
    fn default() -> Self {
        Self { speed: 5.0 }
    }
}
```

- `#[derive(Reflect, Clone)]` plus `#[reflect(Component, Default, Clone)]` is required, and the type needs a `Default`: the value **Add Component** inserts. Write it by hand when zero is not a sensible default, or derive it.
- Doc comments become tooltips: the one on the type in **Add Component**, the ones on fields in the inspector.
- `registry.register_all::<(Mover, Pickup)>()` registers several types. Scene files and the inspector address types by their short name (`Mover`), so names must be unique across the engine and your game. Errors are kept and logged once, naming the plugin, so the result can be ignored.

Field attributes shape the inspector:

```rust
/// Something the player collects by walking into it.
#[derive(Reflect, Default, Clone)]
#[reflect(Component, Default, Clone)]
pub struct Pickup {
    /// Score for collecting it.
    #[reflect(@Range { min: 0.0, max: 100.0 }, @Step(1.0))]
    pub points: u32,
    /// How far it bobs up and down.
    #[reflect(@Range { min: 0.0, max: 1.0 }, @Unit("m"))]
    pub bob_height: f32,
    /// A note for level designers: saved, but not shown in the inspector.
    #[reflect(@Hidden)]
    pub note: String,
}
```

| Attribute | Effect |
|---|---|
| `@Range { min, max }` | Slider limits |
| `@Unit("m")` | Unit shown after the value |
| `@Step(0.1)` | Drag and snap step |
| `@Logarithmic` | Logarithmic slider (with `@Range`) |
| `@Hidden` | On a field: saved but not shown. On a type: hidden from the inspector and **Add Component** |
| `@NotSaved` | On a type: never written to scene files |

Fields can be numbers, `bool`, `String`, vectors and quaternions, `Color`, `AssetRef` (a reference to an asset file), enums and nested reflected structs, among others; the engine's own components (`Primitive`, `StandardMaterial`) are good models.

Keep runtime state out of saved components. Data that only exists while the game runs (a cache, a target entity, an AI state) belongs in a plain component that you add in an [`on_add` hook](#hooks-reacting-to-new-and-removed-components); unregistered components are never saved.

## Resources and project settings

A resource is a single value in the world, read with `world.resource::<T>()` and changed with `world.resource_mut::<T>()`. There are three kinds:

**Runtime resources** are any Rust type, inserted in `build`. They are never saved and survive scene switches, which makes them right for a score:

```rust
/// The score: a plain runtime resource (not registered, so never saved),
/// which survives scene switches.
#[derive(Default)]
pub struct Score {
    pub points: u32,
    /// Levels cleared (every pickup collected).
    pub levels: u32,
}
```

`app.world.resource_or_default::<Score>()` in `build` inserts it if it is missing.

**Scene resources** are saved in a scene file's `"resources"` object, like the engine's `Environment`. Derive `Reflect` with `#[reflect(Resource, Default, Clone)]` and register with `registry.register_resource::<T>()`.

**Settings resources** are tunables shared by every scene. Each lives in its own file, `assets/settings/<Type>.json`:

```rust
/// Tunables shared by every scene, in `assets/settings/Tuning.json`.
#[derive(Reflect, Clone)]
#[reflect(Resource, Default, Clone)]
pub struct Tuning {
    /// How close the player must come to collect a pickup.
    #[reflect(@Range { min: 0.1, max: 5.0 }, @Unit("m"))]
    pub pickup_radius: f32,
}

impl Default for Tuning {
    fn default() -> Self {
        Self { pickup_radius: 1.0 }
    }
}
```

Register it with `registry.register_settings::<Tuning>()` (it needs `#[reflect(Default)]`). Then:

- The player and Play mode load the file before your plugins build, so `build` and every system can read `world.resource::<Tuning>()`. Fields missing from the file take their defaults; without a file (or without a project, as in `game().new_app()`) the whole default is used.
- The editor lists the type under **Game ▸ Project Settings** and creates the file with the defaults when it is missing.
- Changes to the file hot-reload, during Play too. A file that does not parse keeps the last good value and logs a warning.

The sample's file:

```json
{
  "pickup_radius": 1.2
}
```

## Systems and stages

A system is a function (or closure) taking `&mut World`. Add it to a stage in `build`:

| Stage | Runs | Use for |
|---|---|---|
| `Stage::FixedUpdate` | At a fixed 60 Hz, zero or more times per frame, before `Update`. `Time::delta` is the step. | Movement, physics-related gameplay, anything that must not depend on the frame rate |
| `Stage::Update` | Once per frame | Gameplay that follows the display: cameras, UI, effects |
| `Stage::PostUpdate` | Once per frame, after transform propagation | Work that needs final world transforms |

```rust
/// Moves every player with the `move` action (+Y is forward, along -Z).
fn move_player(world: &mut World) {
    let dt = world.resource::<Time>().delta;
    let input = world.resource::<Actions>().axis2("move");
    let step = Vec3::new(input.x, 0.0, -input.y);
    for (transform, mover) in world.query_mut::<(&mut Transform, &Mover)>() {
        transform.translation += step * mover.speed * dt;
    }
}
```

Things to know:

- **Queries.** `world.query_mut::<(&mut Transform, &Mover)>()` iterates the matching entities. Query iterators yield only what you ask for: ask for `Entity` explicitly, as in `world.query::<(Entity, &Transform, &Pickup)>()`. Helpers: `world.find_by_name("Player")`, `world.first_with::<T>()`, `world.count::<&T>()`, `world.resources_mut::<(A, B)>()` for two resources at once.
- **Transforms.** Change `Transform` freely; world transforms (`GlobalTransform`) are computed at the start of `PostUpdate`. Never write `GlobalTransform`, and parent entities with `world.set_parent`. Coordinates are right-handed, +Y up, -Z forward, in metres.
- **Order.** `app.add_system(stage, f)` runs systems in the order they were added. To order by name, use `app.system(stage, f)` and chain `.label("name")`, `.after("name")` or `.before("name")`. A function system is also known by its own name, so `.after("move_player")` finds `move_player` without a label. `app.check_order()` returns an error for a cycle or a name that matches nothing; call it in a test.
- **Structural changes while iterating.** Spawning, inserting, removing and despawning need `&mut World`, which a running query holds. Queue them with `world.defer()`; the app applies them right after the system:

```rust
/// Collects the pickups near a player: despawns each one and sends a
/// `PickedUp`. The despawn is deferred because the query is still iterating.
fn collect_pickups(world: &mut World) {
    let radius = world.resource::<Tuning>().pickup_radius;
    let players: Vec<Vec3> = world
        .query::<(&Transform, &Mover)>()
        .iter()
        .map(|(t, _)| t.translation)
        .collect();
    let mut collected = Vec::new();
    for (e, transform, pickup) in world.query::<(Entity, &Transform, &Pickup)>().iter() {
        if players
            .iter()
            .any(|p| p.distance(transform.translation) < radius)
        {
            world.defer().despawn(e);
            collected.push(PickedUp {
                points: pickup.points,
            });
        }
    }
    world
        .resource_mut::<Events<PickedUp>>()
        .send_batch(collected);
}
```

`world.defer()` also has `spawn`, `insert`, `insert_one`, `remove`, `set_parent` and `run(|world| ...)`.

- **Cost.** Systems run every frame (or step). Expensive work, such as rebuilding meshes or scanning every entity, should be change-driven or throttled.

## Hooks: reacting to new and removed components

`app.on_add::<T>(f)` calls `f(world, entity)` once for every entity that gets a `T`: spawned with it, given it, or loaded from a scene, including after a scene switch. It is the place to add runtime state to saved data. The sample gives each pickup a bobbing tween, which is never saved:

```rust
/// A new pickup bobs up and down: a runtime `Tween`, never saved.
fn add_bob(world: &mut World, e: Entity) {
    let (Some(y), Some(height)) = (
        world.get::<Transform>(e).map(|t| t.translation.y),
        world.get::<Pickup>(e).map(|p| p.bob_height),
    ) else {
        return;
    };
    let bob = Tween::to("Transform.translation.y", y + height)
        .from(y - height)
        .over(FRAC_PI_2)
        .ease(Ease::SineInOut)
        .repeat(Repeat::PingPong);
    let _ = world.insert_one(e, bob);
}
```

`app.on_remove::<T>(f)` runs after an entity loses its `T`, by removal or despawn (the entity may already be gone): use it to clean up state kept elsewhere. Both run at the start of each stage and after every system, so a system never sees a new component whose `on_add` has not run yet.

## Events

Events carry "something happened" from one system to another. The sample sends `PickedUp` when a pickup is collected:

```rust
/// Sent when a pickup is collected. Registered, so scripts can send it and
/// handle it by name too.
#[derive(Reflect, Default, Clone, Debug)]
#[reflect(Default, Clone)]
pub struct PickedUp {
    pub points: u32,
}
```

- `app.add_event::<PickedUp>()` in `build` creates the queue.
- Send with `world.send(event)`, or through the queue: `world.resource_mut::<Events<PickedUp>>().send(event)` (`send_batch` for several, as `collect_pickups` does).
- Handle with `app.on_event(stage, f)`. It calls `f(world, &event)` once per event with full `&mut World` access, owns the read position and adds the queue if needed:

```rust
/// Adds a collected pickup's points to the score; when none are left,
/// counts the level and plays it again.
fn add_score(world: &mut World, event: &PickedUp) {
    let left = world.count::<&Pickup>();
    let (score, scenes) = world.resources_mut::<(Score, Scenes)>();
    score.points += event.points;
    info!("+{} points: {} ({left} left)", event.points, score.points);
    if left == 0 && !scenes.has_pending() {
        score.levels += 1;
        scenes.switch("main");
    }
}
```

For the low-level form, keep an `EventCursor<T>` in a system's closure and call `cursor.read(world.resource::<Events<T>>())`; each reader sees each event once. Events live for two frames, and queues advance only on frames where a fixed step ran, so `FixedUpdate` readers see every event at any frame rate. A `FixedUpdate` handler sees an event at the first fixed step after it was sent; `Update` and `PostUpdate` handlers at the first frame.

`registry.register_event::<PickedUp>()` (in `register`) makes the event visible to scripts and tools by its short name: `events.send("PickedUp", { points = 1 })` in Luau builds a `PickedUp` from its default plus the given fields and puts it on the same queue your Rust handler reads, and `events.on("PickedUp", fn)` lets a script handle the ones Rust sends. A registered event needs `#[derive(Reflect, Clone)]`, and `#[reflect(Default)]` so a script can send it with only some fields. Engine events you handle with `on_event` the same way include `TriggerEvent` (physics) and `SceneEvent` (scene loads and unloads); `PlaySound` is one you send.

## Input actions

Games read named actions, not keys, so bindings can change without code changes. Define the defaults in code:

```rust
/// The actions the game reads, unless the world's `ActionMap` (or the
/// project's action file) already defines them.
pub fn default_actions() -> ActionMap {
    ActionMap::new().axis2(
        "move",
        [
            Axis2Binding::wasd(),
            Axis2Binding::arrows(),
            Axis2Binding::Stick(GamepadStick::Left),
        ],
    )
}
```

In `build`, merge them into the world's `ActionMap` with `merge_missing`, as the plugin above does, so actions someone else defined are kept. Then read the `Actions` resource in systems:

- `actions.axis2("move")` returns a `Vec2` (+Y is forward), `axis1` a single axis.
- `actions.pressed("jump")` and `released` are true once per press; `held` while it is down. In `FixedUpdate` they fire once per fixed step, so a press is never lost or doubled.
- An action name that no map defines reads as zero and is logged once.

Other builders: `.button(name, [Binding::Key(KeyCode::Space), Binding::Gamepad(GamepadButton::South)])`, `.axis1(name, ...)`, `Axis2Binding::dpad()`, `Axis2Binding::keys(up, down, left, right)` and `.with_dead_zone(name, 0.2)`.

Players and designers can rebind without rebuilding: `assets/settings/input.actions.json` replaces the bindings of the actions it lists and hot-reloads. The sample's file adds IJKL to `move`:

```json
{
  "actions": {
    "move": {
      "kind": "axis2",
      "bindings": [
        { "buttons": { "up": { "key": "W" }, "down": { "key": "S" }, "left": { "key": "A" }, "right": { "key": "D" } } },
        { "buttons": { "up": { "key": "I" }, "down": { "key": "K" }, "left": { "key": "J" }, "right": { "key": "L" } } },
        { "stick": "Left" }
      ]
    }
  }
}
```

Raw input is still there when you need it: the `Input` resource has `key_down`, `key_pressed`, `button_pressed` and `mouse_delta`. For mouse look, lock the cursor with `world.resource_mut::<CursorOptions>().lock()` (and `release()`).

To pick with the mouse, `cursor_ray(world)` gives the world ray under the cursor through the active camera; cast it with `world.resource::<Physics>().raycast(&ray, 100.0, &QueryFilter::default())`. `world_to_screen(world, point)` gives where a world point is drawn (pixels, depth; `None` behind the camera) and `screen_ray(world, pixel)` the ray through any pixel. They use the renderer's own projection; for an explicit camera use `Camera::world_to_viewport` and `Camera::viewport_ray`.

## Physics, characters, timers and tweens

With the default `physics` module (see [Optional engine modules](#optional-engine-modules)):

- **Colliders and queries.** Add a `Collider` component in the editor; set `is_trigger` for an area that reports overlaps instead of blocking. The `Physics` resource answers `raycast`, `raycast_all`, `shapecast`, `overlap` and `contains_point`; exclude the asking entity with `QueryFilter::new().excluding(e)`. Queries see colliders as of the previous frame's `PostUpdate`, so a collider spawned this frame is found from the next one.
- **Triggers.** Handle `TriggerEvent` with `app.on_event(Stage::Update, ...)`: it has the `trigger` and `other` entities and a `kind` (`TriggerKind::Enter` or `Exit`).
- **Characters.** Give a player a `KinematicCharacter` (in an `on_add` hook, as runtime state) and set its `velocity` and call `jump()` from a `FixedUpdate` system ordered `.before("move_characters")`, so the input applies in the same step. `examples/collector/src/lib.rs` is a complete third-person controller.
- **Rigid bodies.** With the `dynamics` module, entities with a `RigidBody` (and `Velocity`, joints) are simulated by the engine; the simulation starts when a scene has a body.
- **Interpolation.** Entities moved in `FixedUpdate` look smoother with an `Interpolated` component (the sample adds one to the player); read `interpolated_transform(world, e)` to follow one with a camera.

Timed behaviour uses `Time::delta`, so time scale and pause apply:

- `Timer::once(2.0)` and `Timer::repeating(0.5)` count game time: call `timer.tick(dt)`, which returns true when it finishes. Keep timers in your own components or resources.
- `Tween` animates any reflected field by path, as `add_bob` above does: `Tween::to(path, value)` or `Tween::by(path, delta)`, then `.from`, `.over(seconds)`, `.ease(Ease::...)` and `.repeat(Repeat::...)`. Insert it on the entity; the engine ticks it.

## Scenes at runtime

The player starts with the project's start scene. Change scenes through the `Scenes` resource:

- `scenes.switch("level2")` replaces every loaded scene (a name means `assets/scenes/level2.scene.json`; a path relative to the project works too).
- `scenes.load_additive("hud")` loads a scene next to the others and returns its id; `scenes.unload(id)` removes it.
- Requests apply at the start of the next `Update`, so no system ever sees half of each scene. `scenes.has_pending()` tells whether one is waiting; `add_score` above uses it so that two pickups collected in the same step restart the level only once.

Entities loaded from a scene belong to it. Entities you spawn at runtime survive a switch unless they are parented under a scene entity. New games should call `scenes.set_runtime_entities(RuntimeEntities::DespawnUnlessPersistent)` in `build` and mark the runtime roots they want to keep with `Persistent`. Resources survive switches, which is why the sample's `Score` keeps counting across restarts. `SceneEvent` reports loads and unloads.

## Scripts in a native project

A native project runs Luau scripts like any other project (with the default `script` module): put `.luau` files under `assets/` and add `Script` components in the editor. The sample's `Bonus` entity runs `assets/scripts/bonus.luau`, which calls the Rust `score` API (next section) and sends the Rust `PickedUp` event by name.

How the two meet:

- **Order.** In `Update`, scripts run after your native systems. In `FixedUpdate`, a script's `on_fixed_update` runs after your systems ordered `.before("move_characters")`, then characters move, then your other fixed systems run.
- **Types.** Every registered component, resource and event is available to scripts by name (`e:get("Pickup")`, `world.resource("Tuning")`, `events.send("PickedUp", ...)`) with no binding code.
- **Editor completion.** The game's player and editor write `.box/types/box.d.luau` at startup, with your types and script APIs, so luau-lsp completes them.
- **Errors.** A failing script is disabled and logged; it never stops your Rust systems.

See [Scripting](scripting.md) for the script side.

## A script API in Rust

When scripts need something registered types do not give them (a function, a query, a service of your game), add a global table in Rust. Implement `ScriptApi`:

- `name`: the global's name, for logs.
- `install`: create the globals. Functions reach the world with `with_world` (read) and `with_world_mut` (write); they fail with a clear message outside a running game.
- `definitions`: luau-lsp declarations, with a `---` description line above every declaration and field. They give script authors completion and become the reference text.
- `doc`: an overview paragraph for the API's reference page.

```rust
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
```

The imports, all through `box_engine` (no direct `mlua` dependency):

```rust
use box_engine::box_script::mlua::{self, Lua, Table};
use box_engine::box_script::{with_world, with_world_mut};
use box_engine::prelude::*;
```

Add it in a plugin's `build` with `app.add_script_api`:

```rust
/// Adds the `score` API to every app the game runs in.
pub struct ScoreApiPlugin;

impl Plugin for ScoreApiPlugin {
    fn build(&self, app: &mut App) {
        app.add_script_api(ScoreApi);
    }
}
```

Scripts can now call `score.add(5)` and `score.get()`. Values cross the boundary as copies; keep calls coarse (one call per action, not per vertex), and leave heavy work in Rust. `box_engine::box_script::convert` has conversions for engine value types, and `box_script::entity_methods` adds methods to entities (declare them in `ScriptApi::entity_definitions`).

`box script docs --project` builds the reference from `.box/registry.json`, so it lists your types but not your Rust script APIs. To include them, build the reference in your own code, as the sample's test does:

```rust
    let game = game();
    let apis: Vec<_> = game.script_apis().iter().cloned().collect();
    let reference = docs::reference(&game.registry(), &apis);
```

`reference.write(dir)` writes the Markdown pages (one per API, such as `score.md`), and `reference.undocumented()` lists items without a description.

## Editor actions and tools

Editor-only code lives behind the `editor` feature, so the player never links the editor. Implement `EditorPlugin` and register palette actions or viewport tools:

```rust
impl EditorPlugin for GuideEditor {
    fn build(&self, editor: &mut EditorState) {
        editor.register_action(
            Action::new("guide.reset_score", "Game", "Reset Score", |state| {
                // During Play, `state.world` is the play world.
                if let Some(score) = state.world.get_resource_mut::<Score>() {
                    score.points = 0;
                }
                state.notify("Score reset");
            })
            .tooltip("Sets the running game's score back to zero")
            .keywords("points")
            .enabled_when(|state| {
                if state.is_playing() {
                    Ok(())
                } else {
                    Err("Start Play first".into())
                }
            }),
        );
    }
}
```

- `Action::new(id, category, label, run)` takes a stable id, the category shown before the label in the command palette (Cmd+K or F1), and what to do with the editor state.
- `.enabled_when` returns `Err(reason)` to disable the action; the palette shows the reason. `.tooltip` is shown under the selected palette row, `.keywords` adds search words, `.checked_when` makes a toggle and `.shortcut` shows a keyboard shortcut.
- `state.notify(text)` shows a toast.
- During Play, `state.world` is the play world and the edited scene is parked until Stop. Changes to a play world are thrown away on Stop; edits to the scene itself should go through the editor's edit commands so they can be undone.

Viewport tools register with `editor.register_tool(...)`; the engine's path tool (`box_editor::path_tool::PathTool`) draws point lists into a component of your choice. `examples/walls/src/editor.rs` adds Draw Wall and Draw Fence this way.

Pass the plugins to `run_game_with` in the editor binary. The sample's `src/bin/guide-editor.rs`:

```rust
fn main() -> anyhow::Result<()> {
    let mut options = box_editor::EditorOptions::from_args(std::env::args().skip(1))?;
    if options.project.is_none() {
        options.project = Some(guide_native::project_dir());
    }
    box_editor::run_game_with(options, guide_native::game(), &[&GuideEditor])
}
```

The generated `src/bin/editor.rs` calls `box_editor::run_game(options, my_game::game())`; change it to `run_game_with` when you add an `EditorPlugin`. Declare the module with `#[cfg(feature = "editor")] pub mod editor;` in `lib.rs`, and use `box_editor` only inside it.

## Generators and rules

Procedural geometry is written in Rust too:

- A **generator** is a reflected component that builds a mesh from its fields: `impl Generator` (`VERSION`, `generate`, optional `MATERIAL_SLOTS`, `CATEGORY`), registered with `registry.register_generator::<T>()` in `register`. It then appears under **Add ▸ <category>** and regenerates whenever its fields change.
- A **rule** turns saved "intent" components into derived parts (walls from wall paths, fences that join at shared posts): `app.add_rule(name, f)` in `build`, from `box_engine::box_proc::rule::RuleAppExt`.

Both run in the editor while you edit, not only in Play. See [Procedural geometry](procedural.md) for both, with samples from the same crate.

## How `box` and agents see your types

The `box` CLI and `box mcp` do not link your game's code. They learn its types from `<project>/.box/registry.json`, which the game's player and editor write at startup (rewritten only when the types change). With your types known, `box registry`, `box check`, `box scene show` and `box scene apply` validate game components like engine ones:

```bash
box registry --project my-game --json     # your types are marked "game": true
box check my-game                         # every scene, asset and reference
```

`box check` warns when the file cannot be trusted:

- `registry_missing`: no file yet; game types are unknown to `box`.
- `registry_stale`: the game's sources (`Cargo.toml` or a `.rs` file under `src/`) are newer than the file, or another engine version wrote it.

Both warnings include the command that fixes them. Run the player with `--write-registry`, which writes the file and exits, or just open the game's editor:

```bash
cargo player -- --write-registry
```

## Rebuild and Restart

In the game's own editor (`cargo editor`), **Game ▸ Rebuild and Restart** (Cmd+Shift+B) rebuilds the editor binary in the background, with the build output in the Console and compiler errors as `file:line` toasts. If the build fails, the editor keeps running. If it succeeds, the editor restarts into the new binary with the same scene, camera, selection and unsaved changes. This is the edit-compile-test loop for native code.

It is available only when the running editor is the project's own editor binary (the stock `box-editor` explains why it is disabled). The build is `cargo build --bin <editor>` with the editor binary's required features, plus `--release` when the editor was a release build. Set `BOX_REBUILD_COMMAND` to a shell line to use a different command.

To keep rebuilds rare, put tunable numbers in reflected data (component fields, settings resources) that you can change live in the editor.

## Optional engine modules

The engine's optional modules are `box_engine` Cargo features, all on by default:

| Feature | Adds |
|---|---|
| `physics` | Colliders, physics queries, triggers, kinematic characters |
| `dynamics` | Rigid bodies, joints and collision events (rapier); needs `physics` |
| `audio` | `AudioSource`, `PlaySound`, `AudioSettings` |
| `script` | Luau scripts and script rules; needs `physics` |

The generated `Cargo.toml` turns off `box_engine`'s defaults and lists the modules in the game's own features, so you can drop what you do not use and compile less:

```toml
[features]
default = ["physics", "dynamics", "audio", "script"]
physics = ["box_engine/physics"]
dynamics = ["box_engine/dynamics"]
audio = ["box_engine/audio"]
script = ["box_engine/script"]
editor = ["dep:box_editor"]
```

For example `default = ["physics", "audio"]` leaves out rapier and Luau. A scene that uses a left-out module's types still loads; the player keeps the data and warns once per type, naming the feature to enable. The editor binary always links every module. `examples/minimal_native` is a game built this way.

## Testing headlessly

`game().new_app()` builds the same app the player runs, with every engine plugin and yours, around an empty world, with no window and no GPU. Spawn entities, drive frames with `app.run_frames(n, dt)` and assert:

```rust
#[test]
fn pickups_near_the_player_are_collected_and_scored() {
    let mut app = game().new_app();
    app.world.spawn((Transform::default(), Mover::default()));
    let near = pickup(&mut app, 0.5, 10);
    let far = pickup(&mut app, 8.0, 20);
    app.run_frames(2, DT);
    assert!(!app.world.contains(near), "collected and despawned");
    assert!(app.world.contains(far));
    assert_eq!(app.world.resource::<Score>().points, 10);
    assert_eq!(app.world.resource::<Score>().levels, 0);
}
```

- Feed input with `app.world.resource_mut::<Input>().press_key(KeyCode::D)` (and `release_key`, `press_button`, `set_gamepad_axis`).
- `app.check_order()` fails on a cycle or an unknown system name; `app.system_names(stage)` lists the run order.
- `game().registry()` checks registration: `registry.component("Pickup")` and its field metadata, `registry.event("PickedUp")`, `game().registration_errors()`.

To test the project as the player runs it, scenes, settings files, scripts and all, build the player's app and let the asset server finish its imports between frames:

```rust
fn frames(app: &mut App, n: usize) {
    for _ in 0..n {
        if let Some(s) = app.world.get_resource::<AssetServer>().cloned() {
            s.update_until_idle(&mut app.world, Duration::from_millis(20));
        }
        app.run_frames(1, DT);
    }
}

fn play() -> App {
    let options = PlayerOptions::from_args([
        project_dir().display().to_string(),
        "--headless".into(),
        "--no-watch".into(),
    ])
    .unwrap();
    let (app, _project) = build_app(&game(), &options).unwrap();
    app
}
```

The sample's `tests/gameplay.rs` uses these to check that the settings file is loaded, that `bonus.luau` reaches the Rust API and event, and that clearing the level restarts it. Editor plugins are tested the same way with `box_editor::build_game_editor_with` and `editor.state.run_action(id)`; see `tests/editor.rs`, which also captures the palette with `box_editor::headless::capture`.

For a picture of the real thing, run the player headless with a capture:

```bash
cargo player -- --frames 60 --capture shot.png
```

## Where next

- [Getting started](getting-started.md): projects, scenes and the first run.
- [The editor](editor.md): the inspector your components appear in, Play mode, Project Settings.
- [Scripting](scripting.md): the Luau side of a mixed project.
- [Procedural geometry](procedural.md): generators and rules in Rust.
- [Script API reference](../reference/README.md): every script global, including how registered types appear.
- Example games in the engine repository: `examples/collector` (a complete native game: character, camera, triggers, sound, scene switching), `examples/walls` (rules, generators and editor tools), `examples/minimal_native` (a game without the optional modules).
