# Scripting and Game Logic

How games add behaviour to box-engine, now and later. Decision record: D4 in [DECISIONS.md](DECISIONS.md).

Code in this document is illustrative. The script API in particular is a sketch that spike S4 and the second game will reshape.

## 1. Goal

Developers should eventually make a complete game **without modifying the engine and without a Rust toolchain**, while developers who want Rust can use it fully.

This leads to two tiers that share one API model.

| | Native tier (Rust) | Script tier (Luau) |
|---|---|---|
| Game logic lives in | A Cargo crate implementing `Plugin` | `.luau` files in the project |
| Needs a toolchain | Yes | No |
| Runs in | A project-specific editor and player binary | The prebuilt editor and player |
| Iteration | Rebuild and restart the editor | Hot reload on save |
| Best for | Engine-adjacent work, custom generators, heavy computation, construction games | Gameplay rules, interaction, UI flow, small games |
| Available from | M3 | M7 |

A native project can also use scripts. The tiers are not exclusive.

**Why scripting waits until M7.** The construction slice is a native project, so nothing before the second game uses Luau. Building the host, bindings and tooling in M3 would be work with no user for four milestones. What M3 does provide is the type registry and reflection that scripting will be generated from, so the engine API is shaped with scripting in mind. Spike S4 at the start of M7 settles the API shape before it is built.

## 2. Options evaluated

| Option | Performance | Developer experience | Safety | Hot reload | Binary-distributed engine | Verdict |
|---|---|---|---|---|---|---|
| **Rust, statically linked** | Best | Best tooling; slow iteration | Full access | No (restart) | Requires toolchain | **Tier 1, first** |
| Rust dylib hot reload | Best | Fragile | Unsafe ABI | Partially | Requires toolchain | Rejected for now |
| **Luau via `mlua`** | Good interpreter | Gradual types, active LSP, small language | Sandboxed | Trivial | Ships inside the binary | **Tier 2, first scripting language** |
| Lua 5.4 / LuaJIT | Good / best | No types; LuaJIT needs a JIT entitlement on Apple Silicon and has no JIT on iOS | Weaker sandbox | Trivial | Fine | Luau preferred |
| Rhai | Slow | Stale LSP | Sandboxed | Trivial | Fine | Rejected |
| WebAssembly (wasmtime) | Very good | Per-language toolchain, weaker debugging | Best sandbox | Module reload | Fine, larger runtime | Deferred extension |
| C# via .NET hosting | Very good | Best IDE and debugger; would allow reusing existing C# code | Managed | Possible | Large runtime, large binding surface | Deferred |
| JavaScript / TypeScript | Mixed | Excellent tooling | Sandboxed | Trivial | Engine size or speed trade-off | Not pursued |
| Visual scripting | — | — | — | — | — | Possible future feature, not in the first release |

Why Luau specifically: it has a real type checker and language server that accept custom definition files, a sandbox mode, a fast interpreter that needs no JIT permissions, and it is proven at scale for user-authored game logic.

## 3. One API, generated from the registry

The engine's type registry (see [SCENE_SYSTEM.md](SCENE_SYSTEM.md) section 6) is the single source of truth. Types and functions are marked once in Rust:

```rust
#[derive(Reflect)]
#[reflect(Component, Script)]
pub struct Transform { pub translation: Vec3, pub rotation: Quat, pub scale: Vec3 }

#[script_api]
impl Physics {
    /// Cast a ray and return the nearest hit.
    pub fn raycast(&self, origin: Vec3, dir: Vec3, max: f32) -> Option<RayHit> { /* ... */ }
}
```

From the registry the engine generates:

1. Luau bindings (field access by name, method calls).
2. Inspector UI.
3. The serialization schema.
4. `.d.luau` type definition files and reference documentation, which give script authors completion and type checking through `luau-lsp`.

Nothing is bound by hand per type. The registry is language-neutral, so a WebAssembly (WIT) or C# binding generator can be added later without redesigning the engine API.

## 4. What a script looks like

```lua
-- scripts/spinner.luau (illustrative)
local Spinner = {}

Spinner.properties = {
    speed = { type = "number", default = 90, range = { 0, 720 }, unit = "deg/s" },
    axis  = { type = "Vec3", default = Vec3.Y },
}

function Spinner.on_start(self) end

function Spinner.on_update(self, dt: number)
    local t = self.entity:get("Transform")
    t:rotate_axis(self.props.axis, math.rad(self.props.speed) * dt)
end

function Spinner.on_trigger_enter(self, other) end

return Spinner
```

- A `Script` component on an entity references a script asset and stores its property values. Properties declared by the script appear in the inspector and are saved in the scene.
- Lifecycle callbacks: `on_start`, `on_update`, `on_fixed_update`, `on_destroy`, plus event callbacks (`on_trigger_enter`, input actions, named signals).
- Scripts run in the Update and FixedUpdate phases, after native systems of the same phase.

### Requirements the M7 design must meet

These came out of the generality review. They are requirements on the design, not yet designs.

| Requirement | Why |
|---|---|
| **Batch form as well as per-entity callbacks.** A script can register a system that queries many entities in one call. | Per-entity callbacks with by-name access are too slow and too awkward for hundreds of enemies or projectiles. |
| **Clear value semantics.** Whether `entity:get` returns a live proxy or a copy is decided once and documented. | This is hard to change after scripts exist. |
| **Coroutine-style waits.** A script can wait for time, a signal or an animation without a hand-written state machine. | Dialogue, cutscenes and sequenced gameplay. |
| **A script state model.** Properties are saved automatically; other state is saved through `on_save` and `on_load` hooks. | Saved games, scene transitions and hot reload must agree. |
| **Data access.** Scripts can read JSON data tables from the project. | Dialogue, items, string tables. |
| **UI that is not per-frame immediate-mode calls.** | Boundary cost, and an API that could not be changed later. See D11. |

## 5. Native tier

```rust
pub struct MyGamePlugin;

impl Plugin for MyGamePlugin {
    fn register(&self, registry: &mut Registry) {
        registry.register_all::<(Health,)>();
    }
    fn build(&self, app: &mut App) {
        app.add_system(Stage::Update, regenerate_health);
        // generators (M5+) register here too
    }
}
pub fn game() -> Game { Game::new().with_plugin(MyGamePlugin) }

// src/bin/editor.rs (feature `editor`)
fn main() -> anyhow::Result<()> {
    box_editor::run_game(box_editor::EditorOptions::from_args(std::env::args().skip(1))?, my_game::game())
}
// src/bin/player.rs
fn main() -> anyhow::Result<()> {
    box_engine::player::run_game(box_engine::player::PlayerOptions::from_args(std::env::args().skip(1))?, my_game::game())?;
    Ok(())
}
```

- Engine modules and game modules use the **same** `Plugin` trait. `box_arch` is a plugin; a construction game is a plugin that depends on it.
- There are **no dynamic Rust plugins**. Rust has no stable ABI, and Fyrox's own documentation describes dylib hot reload as built on unsafe functionality. Rust plugins are linked statically; scripts are the dynamic layer.
- Fast iteration for native projects relies on quick incremental builds and on moving tunable values into reflected data that the editor can change live.

## 6. Hot reload

| What | Behaviour | From |
|---|---|---|
| Shaders | Reloaded on save; the last good pipeline is kept on error. | M1 |
| Assets | Reloaded on save through the asset server. | M4 |
| Luau scripts | Reloaded on save. Property values persist (they live in the component). Other script state follows the script state model. | M7 |
| Rust code | Rebuild and restart. The editor reopens the last project and scene. | — |

## 7. Safety and errors

- Luau runs in sandbox mode: no file system, network or process access. File and save APIs are provided by the engine and limited to the project's data and save directories.
- Every call into a script is wrapped. An error stops that script instance, logs a traceback with file and line to the console, and does not crash the editor or player.
- A guard against infinite loops in the editor (for example an instruction budget) is added if it proves necessary.

## 8. Performance guidance

- Scripts decide *what* happens; heavy computation stays in Rust. Mesh generation, solvers and pathfinding are native code exposed through the API.
- Crossing the boundary is the main cost. APIs are designed for coarse calls (query a set, set a field) and not for per-vertex work.
- M7's tests include a benchmark of 10,000 script callbacks per frame and of batch queries over 5,000 entities, so the cost is known before the second game depends on it.

## 9. Debugging

- Tracebacks with source locations in the console, with click-to-open.
- `print` and a structured `log` API routed to the console.
- Live inspection of script properties in the inspector during Play.
- A breakpoint debugger is deferred until after M9.

## 10. What this means for construction games

The construction slice is built in the **native tier**. Its document model, rules, solvers and mesh builders are computation-heavy and belong in Rust (in `box_arch` where reusable, and in the game crate where specific). It does not need Luau.

The script tier is validated by rewriting the M3 sample game in Luau (M7) and then, substantially, by the second game (M8).

## 11. Long-term extension strategy

1. Luau as the supported scripting language.
2. Keep the registry language-neutral and the script API versioned (additive within a minor version after M9).
3. Add a WebAssembly component host when there is demand for other languages or third-party plugins that need sandboxing.
4. Consider visual scripting only as a layer that emits calls to the same registry API.

## 12. Risks

- **Maintaining the binding layer** is the largest ongoing cost (R11). Mitigation: generate from reflection; keep the API small.
- **Freezing the script API too early.** Mitigation: mark it unstable until M9; let the second game shape it first.
- **Script performance with many entities** (R24). Mitigation: batch queries; native code for hot loops.
- **Reflection crate coupling** (`bevy_reflect` types appear in the native API). Mitigation: re-export through `box_core`; see D3.

## 13. M7 as built

Spike S4 (`spikes/s4_luau`, FINDINGS.md) chose the binding shape from measurements; `box_script` builds it. The API is unstable until M9.

**Where scripts run.** `box_engine::Game::app` adds `box_script::ScriptPlugin` after the game's plugins (default `script` feature). In Update, scripts run after the game's native systems (label `scripts_update`). In FixedUpdate, scripts (label `scripts_fixed_update`) run after the rigid-body step (`physics_step`) and the native systems ordered `.before("move_characters")`, then characters move, then the game's other fixed systems run: script input moves characters in the step it is given, as native input does (`ScriptPlugin` adds `CharacterPlugin` for this). One Luau VM per world that plays (player, Play mode), created on the first frame with a `Script`; an editor's edit world never runs systems and never gets one. Native and data projects both run scripts; a data project needs no Rust.

**Script component.** `Script { source: AssetRef, properties: Vec<ScriptProperty> }`. `properties` holds overrides of the script's declared defaults (`ScriptValue`: Number, Integer, Boolean, String, Vector, Color, Entity(uid), Asset). The inspector shows one row per declared property ("(default)" until overridden, ↺ resets); values for undeclared names are kept.

**A script** is a `.luau` file under `assets/` returning a module table:

```lua
local Spinner = {}
Spinner.properties = {
    speed = { type = "number", default = 90, min = 0, max = 720, unit = "deg/s", doc = "Turn rate" },
    axis = { default = vector.create(0, 1, 0) },   -- type inferred from the default
    enabled = true,                                -- shorthand
}
function Spinner.on_update(self, dt)
    self.entity:rotate(Quat.from_axis_angle(self.props.axis, math.rad(self.props.speed) * dt))
end
return Spinner
```

Property types: number, integer, boolean, string, vector, color, entity, asset. Order in the inspector is declaration order. The importer reads declarations by running the top level once in a throwaway sandboxed VM (2 s budget); only value types, `log` and `require` work there.

**Lifecycle and order.** Per frame: FixedUpdate (each step: rigid-body step, native input systems, scripts, `move_characters`, other native fixed systems) — removals, starts, `on_fixed_update(self, dt)`. `on_fixed_update` runs before the character move, like a native input system: `character.move`/`jump` apply in this step, and the character's position, velocity and `is_grounded` are what the previous step left (bodies are already stepped; forces added here apply at the next step). Read where the character ended up in `on_update`. Update — removals, inspector edits (a sixteenth of the instances per frame), hot reload, starts, `on_trigger_enter/exit(self, other)` (both entities of a contact), event handlers, waiting tasks, `on_update(self, dt)`. `on_start(self)` runs as a task (it may wait) when the file has loaded; `on_destroy(self)` after the entity or component is gone. Instances run in start order (scene order): deterministic; `math.random` is seeded 1.

**`self`.** A table per instance: `self.entity`, `self.props` (effective property values; entity properties resolve to handles), plus any fields the script sets; its metatable is the module.

**Value semantics.** Copies. `e:get("Transform")` returns a fresh table; `e:set("Transform", { translation = v })` patches only the given fields, type-directed (paths too: `e:set("Transform.scale", v)`). Unknown fields fail with a did-you-mean. Mapping: entity = light userdata with methods (compares by value, table key); Vec3/Vec3A = native `vector`; Vec2 = `vector` with z 0; Quat = `Quat` userdata (`q * v`, `q * q`); Color = `{r,g,b,a}` linear; numbers/bool/String direct; AssetRef = string or nil; Uid = string; struct = table; newtype = its field; Option = value or nil; unit variant = string; data variant = `{ Variant = payload }`; list = array. Other opaque types go through their scene JSON form.

**Engine API.** Built-in `ScriptApi`s: `Quat`, `Color`, `log.info`/`warn`/`error` and `print`, `wait`/`wait_frames`/`wait_until`, `task.spawn`/`delay`/`cancel`, `require`; entity methods (`get`, `set`, `add`, `remove`, `has`, `name`, `set_name`, `uid`, `despawn`, `parent`, `children`, `set_parent`, `script`, `position`, `set_position`, `translate`, `rotation`, `set_rotation`, `rotate`, `rotate_y`, `scale`, `set_scale`, `forward`/`right`/`up`, `look_at`, `world_position`, `world_rotation`); `world` (`spawn{...}`, `find`, `find_all`, `entity(uid)`, `query(...)`, `count(...)`, `each(names, fn)`, `resource`, `set_resource`, `despawn`, `scripts(path)`); `time`; `input` (actions, keys, mouse, cursor lock); `physics` (`raycast`, `raycast_all`, `spherecast`, `overlap_sphere`, `overlap_box`, `contains_point`); `events` (`on`, `off`, `send`). box_engine adds `scenes` (`switch`, `load`, `unload`, `current`, `is_loaded`). Components, resources and events need no bindings: any registered type works by name (`Registry::register_event` makes an event sendable and readable; `PlaySound` is one).

**Adding bindings (other crates).** Implement `box_script::ScriptApi` (`name`, `install(lua, globals)`, `definitions()`) and add it in `Plugin::build` with `app.add_script_api(api)` (resource `ScriptApis`); functions reach the world with `box_script::with_world` / `with_world_mut`; `box_script::convert` has the value conversions.

**Gameplay APIs (M7 integration).**
- `character` (box_script): `move(e, v)` sets the horizontal velocity, `set_velocity`, `velocity`, `jump` (true if grounded), `is_grounded`, `ground_entity`, `ground_normal`, `teleport` (also resets interpolation). Call `move`/`jump` from `on_fixed_update`: it runs before `move_characters`, so they apply in the same fixed step (as for a native system ordered `.before("move_characters")`); from `on_update` they apply at the next fixed step, as for a native Update system.
- `physics.move_and_slide(position, motion, options)` for a capsule. With the `dynamics` feature: `apply_impulse`, `apply_impulse_at_point`, `apply_torque_impulse`, `add_force`, `add_torque` (next step), `velocity(e) -> (linear, angular)`, `is_sleeping`, `joint_position`, and the callbacks `on_collision_enter(self, other, impulse)` / `on_collision_exit(self, other)` (both entities of each `CollisionEvent`, dispatched after triggers).
- `animation` (box_engine): `set_float`, `set_bool`, `trigger`, `float`, `bool`, `state`, `transitions`, `play(e, clip, crossfade?)`, `time`, `normalized_time`, `finished`, `layers` (clip, time, weight), `joint(e, name)`.
- `ui` (box_ui): `add(document, parent?)`, where a document is a JSON string or a table with the same fields (`args` values may be numbers); `find`, `remove`, `contains`; every setter and getter of `Ui`; `focus`, `focused`, `active_modal`, `captures_input`; `set_theme`, `set_passthrough_actions`; `on(kind, id?, fn)` (`click`, `change`, `toggle`, `focus`, `blur`, `back`, `any`; the event is `{node, id, action, kind, value?, checked?}`) and `off`. Nodes are `UiNode` handles; any node argument may be an id string.
- `strings` (box_ui): `get`, `format(key, args)`, `has`, `language`, `set_language`.
- `save` (box_engine): `save(slot)`, `load(slot)` (start of the next Update), `slots()`, `include(e, on?)` (Saveable; `keep` is a deprecated alias), `exclude(e)` (SaveExclude), `on(fn)` (`{kind = "saved"|"loaded"|"failed", slot, path?, entities?, message?}`). Every `Game::app` registers the `scripts` save hook, so saves carry each script's `on_save` result. On load, `on_load` runs once the respawned instance's `on_start` has returned (waits included); set defaults in `on_start`, restore in `on_load`. Keep endless loops in `task.spawn` (an `on_start` still running after 5 s with load data waiting logs a warning).
- `data` (box_engine): `table(path)` returns the data table as a read-only Luau table (nil until imported, the same table until the file changes); `version(path)`.
- `particles` (box_engine): `burst(position, count, patch?)` returns the one-shot emitter entity; `start`, `stop`, `restart`, `count`, `is_finished`.
- `scenes`: also `persist(e)` (Persistent) and `set_runtime_entities("keep_all" | "despawn_unless_persistent")`.
- Entities: `e:interpolated_position()` (where an `Interpolated` entity is drawn this frame).

- `app.quit()` asks the runner to exit (stops Play in the editor). `ScriptStats::update_time` / `fixed_time` give the script share of a frame.

- M9 additions: entity methods `set_visible(bool)` / `is_visible()` (`Visible`, inherited by the subtree; hides meshes, local lights and particles, physics and scripts keep running); `world.spawn{ ..., transient = true }` (`Transient`: runtime-only, never saved, removed on load); `save.off`, `input.mouse_released`, `ui.on(kind, fn)` without an id, `Color.hex` with `RRGGBBAA`. Renamed in the API review (old names still work and warn once with the caller's line; see [STABILITY.md](STABILITY.md)): `time.paused` → `time.is_paused`, `input.cursor_locked` → `input.is_cursor_locked`, `animation.finished` → `animation.is_finished`, `ui.checked` → `ui.is_checked`, `scenes.load` → `scenes.load_additive`, `save.keep` → `save.include`.

**Bindings that need more than globals.** `box_script::entity_methods(lua)` returns the entity method table (declare additions in `ScriptApi::entity_definitions`). Rust events become script handlers through `ScriptSignals::send(name, json)` (delivered in the next script Update, entities as `{"$entity": uid}`) and `box_script::subscribe(lua, name, fn)` (owned by the calling instance); box_ui's `ui.on` and box_engine's `save.on` are built this way. `examples/collector_luau` (the M3 game in Luau) uses every API above.

**Batch form.** `world.query("A", "B")` returns an entity array (typed per-component queries in Rust); loop it with the typed accessors. `world.scripts(path)` returns the `self` tables of a script's instances (script data with no boundary cost). `world.each(names, fn)` passes component copies (convenient, about 0.6 µs per row).

**Waits.** `wait(s)` (returns seconds waited), `wait_frames(n)`, `wait_until(fn, timeout?)` (true, or false on timeout) yield the running task; they work in `on_start` and `task.spawn` functions and fail elsewhere. Tasks resume in Update on game time.

**Events.** `events.send(name, payload)`: a registered type is built from its default patched with the payload and sent on its queue (Rust systems and script handlers read it); any other name is a script signal. `events.on(name, fn)` handlers run in the Update stage before `on_update`; signals sent during `on_update` arrive next frame. Handlers and tasks belong to the instance that created them and stop with it.

**State model.** Properties are scene data (saved with the scene and copied into save games with it). Everything else lives in `self` and is runtime-only; to save it, return plain data from `on_save(self)` and restore it in `on_load(self, data)`. `box_script::save_script_state(&mut World)` returns `{ "<uid>": data }` (vectors `{"$vector": [..]}`, rotations `{"$quat": [..]}`, entities `{"$entity": uid}`); `load_script_state` calls `on_load` at once or right after the instance's `on_start`.

**Hot reload.** Saving a `.luau` during Play re-runs the module: instances keep `self`, the new functions apply at once (shared metatable), `self.props` is refreshed, disabled instances come back (and start if they never did). If the module defines `on_reload(self)`, the instance's tasks and handlers are cancelled first and `on_reload` runs; otherwise running tasks finish their old code. Modules that `require` a changed file reload too. A file that fails to load keeps the previous version running.

**Errors.** A failing callback, task or handler logs `file:line: message` (+ script frames) under the `box_script` target (editor toast and Console count) and disables that entity's script until its file reloads; errors raised by engine functions take the calling line. A stage running longer than 5 s is interrupted by a watchdog (`ScriptRuntime::set_watchdog`), so an endless loop cannot hang the editor.

**Sandbox.** Luau sandbox mode: no `io`, `os.execute`, `os.getenv`, `loadstring`, `dofile`; built-ins read-only; per-script globals. `require` takes `./x`, `../x` (relative to the script) or an assets-relative path, only inside `assets/`.

**Type definitions.** `box script types --project <dir>` writes `.box/types/box.d.luau` (types and patch types of every component, resource and event, `Entity:get`/`set` overloads per component, every API's declarations) and, when missing, `.vscode/settings.json` pointing luau-lsp at it. Native games write the file at startup with their own types and APIs (`registry_file::write_at_startup`); `Game::script_definitions()` returns it. Checked with luau-lsp 1.70.1 (`crates/box_engine/tests/luau_definitions.rs` runs `luau-lsp analyze` on every example project and a `--!strict` probe when the tool is on `PATH`): classes are written `declare extern type X with … end` (the older `declare class` no longer parses), type aliases are `export type` (plain `type` aliases in a definitions file are invisible to scripts), and fields named after Luau keywords are quoted (`["end"]: number`). Before that check the file failed to load in luau-lsp, so editors had no completion.

**Reference documentation (M9).** Every declaration in a `ScriptApi::definitions()` carries a `--- description` line directly above it (globals, table fields, class members, type fields), and every API has a Markdown overview (`ScriptApi::doc`). luau-lsp shows the descriptions on hover (checked with 1.70.1: declared globals, fields, functions, methods and classes). `box_script::docs` parses the same declarations, plus the registry's types with their doc comments, ranges, units and defaults, into Markdown: `box script docs --out docs/reference` (the engine's reference, committed) or `box script docs --project <dir>` (into `<project>/.box/reference`, with a native game's schema types). `crates/box_engine/tests/script_reference.rs` fails when an item has no description, when the committed reference is stale, or when a definitions-file line has no `---` above it. The user guides are in [guide/](guide/); their Luau samples come from `examples/guide_samples` and their Rust samples from `examples/guide_native` (`crates/box_engine/tests/guide_samples.rs` checks both and plays the samples).

**Costs** (release, Apple M3 Ultra; [measurements/m7-script.md](measurements/m7-script.md)): 10,000 `on_update` callbacks 0.6 ms; batch query over 5,000 entities with a typed write each 0.53 ms; typed transform access 0.07 µs; generic component copy 0.53 µs.

**Example.** `examples/script_demo` (data project): spinning (`spinner.luau`), a trigger (`gem.luau`), input-driven movement with a ground ray (`player.luau`), a follow camera, a wait loop (`beacon.luau`), signals, `PlaySound`, a batch spawn and save hooks (`game.luau`), `require("./util")`.

**Limits.** One `Script` per entity. No debugger, no click-to-open tracebacks yet.

**Script rules.** A data project authors procedural rules in Luau (`box_script::rules`). Saved intent is a `ScriptIntent { rule, points, properties }` component: `rule` names a `.luau` file under `assets/` that defines `rule(intents, out)`; `points` are owner-local points (a path, an outline or positions: the rule decides; drawn and edited with the editor's Draw Path tool); `properties` overrides the file's declared `properties` (same declarations and inspector rows as a script's). The rule receives every intent naming the file (or those its `scope` covers), in uid order: `it.uid`, `it.points`, `it.props` (effective values; entities as uid strings, assets as paths), the owner's world `it.position`/`it.rotation`/`it.scale`, and `it:to_world(p)`/`it:to_local(p)`. It returns parts with `out:part(owner, role, { Component = patch, ... }, { position, rotation, scale }?)`: any registered component by name, patched over its default through reflection (`Primitive`, `Sweep` + `Spline`, `Lathe`, `Extrude`, `StandardMaterial`, box_arch generators when that plugin is present); `Uid`, `Name`, `Derived`, `Parent`, `Children` and `GlobalTransform` are refused. `out:warn(owner, message, role?)` adds a warning on the owner prefixed with `file:line`. Parts go through `reconcile` like a Rust rule's: keys `<owner uid>/<role>` (no bare numbers), Overrides, warnings on the owner, the rule list (one rule per file, named by its path, intent type `ScriptIntent`, found when the first intent names it; the on/off switch works and is remembered before the rule is found).

**Script rule host and purity.** `ScriptRules` (a `box_proc::rule::RuleSource`) is one small Luau VM per world for rules only, separate from the gameplay VM: it runs in the editor's edit world (`Game::rules`), in Play and the player (`Game::app`), and in `box render`. Rules have value types, `math`, `string`, `table` and `log`; `world`, `input`, `physics`, `events`, `scenes`, `task`, `wait*` and `require` fail with a message. Each run executes the compiled file afresh (no state survives between runs), `math.random` is reseeded with 1, and a run longer than 2 s is interrupted. A rule runs only when one of its intents (value, uid, world transform, `Overrides`) or its file changed. A failing rule keeps the parts of its last good run; the error (`file:line: message`) is logged once (target `box_script`) and shown on every owner. Saving the file re-runs the rule in the edit world and in Play; a file that fails to import keeps its previous version running, with the error on the owners.

**Script rule scope.** A run covers every intent naming the file unless the module declares less: `Rule.scope = "intent"` (an intent's parts and warnings depend only on itself) or `Rule.scope = { neighbours = r }` (on itself and the intents whose bounds, its world-space points and origin, come within `r` metres). An edit then rebuilds the changed intents and every intent within `r` of a change's old or new bounds; the rule sees those plus the intents within `r` of them, and the extra intents' parts are dropped before conversion. A scope is a promise the host cannot check: a rule that reads farther gets parts that depend on edit history. `math.random` fails in a scoped rule. The first run, a run after the file changed or a run failed, and a change touching more than a quarter of the intents cover every intent. `examples/script_rules/assets/rules/fence.luau` declares `{ neighbours = SNAP }` (a fence skips its end post when a lower-uid fence ends within SNAP); the colonnade keeps the default. Whole-file runs stay exact and are cheaper: converted components are cached per file by content and shared, and reconcile skips owners whose parts, warnings and overrides did not change.

**Script rule costs** (release, fence rule, rows of fences sharing end posts, about 6 parts per intent): idle pass 1.5 µs at 100 intents, 9.3 µs at 1,000, 47 µs at 5,000. With the fence's scope, one edit costs 0.19, 0.19 and 0.34 ms at 100, 1,000 and 5,000 intents, and a 60 Hz point drag 0.16–0.33 ms per pass. Without a scope, every edit runs the rule over every intent: 2.5, 25 and 128 ms (was 4.5, 38 and 194 ms), so drag-heavy rules over hundreds of intents should declare a scope. Types: `box script types` declares `RuleIntent`, `RuleOut`, `RuleTransform` and `RuleComponents`. Example: `examples/script_rules`.
