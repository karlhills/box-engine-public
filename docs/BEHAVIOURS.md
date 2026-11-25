# Behaviours: a first game without code

Status: **B1 and B2 built** (2026-11-24; B2 as B2a, N1, N2, B2b in [NAVIGATION.md](NAVIGATION.md#b2-with-navigation)); **B3 built** (2026-11-25) except the user test and script versions of the player, camera and enemy. D19 below is not yet confirmed. Builds on [INTERACTIONS.md](INTERACTIONS.md) (D18). User guide: [guide/behaviours.md](guide/behaviours.md). Follow-up: [AI_PANEL.md](AI_PANEL.md).

## Goal

Someone who has never made a game opens the editor, picks a template, and finishes a small 3D game (run, jump, collect, avoid, reach the goal, win or lose, pause, export) **without opening a script**. Every piece of gameplay is a component or a preset dragged into the scene and tuned in the inspector. When they want more, one click turns any behaviour into the Luau script that does the same thing, so scripting starts from working code instead of a blank file.

The same parts make the agent path reliable: an agent (`box mcp`, the [AI panel](AI_PANEL.md)) places and tunes behaviours through commands instead of writing Luau, so its results are smaller, checkable by `box check`, and editable by the user in the inspector.

## What exists today

- `Interactable` + `GameVars` + `Collidable` (D18) cover press and touch interactions with conditions and an ordered list of actions. `examples/ember_bridge` is a puzzle with no gameplay Luau.
- Everything else a first game needs is a Luau script that every project copies: the third-person template has `player.luau` (74 lines), `camera.luau` (62), `pickup.luau` (54), `hud.luau` (45), `util.luau` (37). `examples/cinder_keep` adds `checkpoint.luau`, `door.luau`, health, win and pause in `game.luau` (435 lines).
- The engine pieces under those scripts are already there: `KinematicCharacter` + `CharacterPlugin`, `Interpolated`, `AnimationStateMachine`, `Timer`/`Tween`, `ParticleEmitter::burst`, `PlaySound`, `Scenes::switch`, `SaveGames`, `box_ui` modals and themes, string tables, `Transient`, `Visible`/`Collidable`.
- Add Component has a Gameplay category (`Script`, `Interactable`, `Interactor`; `crates/box_editor/src/inspect.rs`).

So nothing here needs a new subsystem. The work is packaging what the scripts do as engine components, sharing one action vocabulary, and making it discoverable.

## Principles

1. **One action vocabulary.** Every "when X happens, do Y" uses `InteractAction` lists (D18). New behaviours add triggers, not a second effect system. New effects are new `InteractAction` variants, usable everywhere.
2. **Presets over components.** When an existing component plus a few fields already describes the thing, it ships as a **preset** (a named recipe of components), not a new type. A pickup is an `Interactable { trigger: Touch, once, actions: [...] }` plus a `Spinner`. The user sees "Pickup"; the scene holds ordinary components.
3. **Plain names, good defaults.** Every behaviour works when added with no fields changed. Field docs (the inspector tooltips) are written for someone who does not know the engine.
4. **Eject, don't trap.** Every behaviour component has a Luau equivalent the user can generate and keep editing (see [Eject to script](#eject-to-script)).
5. **Cheap per frame.** Behaviours are change-driven or simple per-entity updates over a query. No per-frame allocation, no mesh rebuilds, HUD text set only when the value changes.

## Proposal

Code lives in `crates/box_engine/src/behaviours/` (one file per behaviour, `BehavioursPlugin` added by `app_with_world` next to `InteractPlugin`), registered in `engine_registry()`. Behaviours that need the character controller are behind the `physics` feature and listed in `modules::MODULES`.

### New components

| Component | What it does | Main fields (all with defaults) |
|---|---|---|
| `PlayerController` | Moves a `KinematicCharacter` from the `move`/`jump`/`run` actions relative to the camera; turns and animates its body; respawns below `fall_limit` (at the last checkpoint). Adds `KinematicCharacter`, a capsule `Collider` on the player layer and `Interpolated` when missing (`on_add`). It is the `Interactor`. | `speed` 6 m/s, `run_multiplier` 1.6, `jump` on, `turn_rate`, `fall_limit` −20 m, `camera: Option<Uid>`, `body: Option<Uid>` (feeds `speed`, `grounded`, `jump` to its state machine when those parameters exist) |
| `FollowCamera` | Third-person orbit camera with mouse/stick look, cursor lock on click, pull-in against walls; or a fixed-angle mode for top-down and side-on games. | `target: Option<Uid>` (default: the player), `mode` (`Orbit`, `Fixed`), `distance`, `height`, `pitch`, `smoothing`, `look_speed`, `mouse_sensitivity` |
| `Spinner` | Rotates around an axis. | `axis` Y, `speed` 2 rad/s |
| `Bobber` | Moves up and down around its start. | `height` 0.15 m, `period` 3 s, phase from position (so copies are out of step) |
| `Mover` | Moves along waypoints or a `Spline`, carrying characters standing on it (the controller already rides platforms). | `points: Vec<Vec3>` or `path: Option<Uid>`, `speed`, `mode` (`Loop`, `PingPong`, `Once`), `wait` s at each end, `start_on` (`Always`, `Signal(name)`) |
| `Health` | Hit points with a short invulnerability after each hit. Runs `on_damage` and `on_zero` actions. On the player, `on_zero` defaults to `[Respawn]` when lives remain, else `[Lose]`. | `max` 3, `current` (saved), `invulnerable` 0.8 s, `on_damage`, `on_zero` |
| `Spawner` | Spawns copies of a prefab (`Transient`) at an interval inside an area, up to a limit. | `prefab`, `interval` 2 s, `max_alive` 5, `radius` 0 m, `enabled` |
| `Chaser` | A simple enemy: idles or walks patrol points, chases the player inside `sight` (with line of sight) along the navmesh on its own `KinematicCharacter` through a `NavAgent`, searches where it last saw them, returns home beyond `give_up` ([NAVIGATION.md](NAVIGATION.md)). | `speed`, `sight` 8 m, `give_up` 14 m, `patrol: Vec<Vec3>` |

Every component has `enabled: bool`. `InteractAction::SetEnabled` is widened from `Interactable` to every behaviour on the target, so a lever can start a `Mover` or stop a `Spawner`.

### Game rules and state

**`GameRules`** (scene resource, saved in the scene like `Environment`): what winning and losing mean for this scene. Shown in Scene Settings.

| Field | Default | Meaning |
|---|---|---|
| `win` | `[]` | `Vec<Condition>`: the scene is won when all hold (checked when a var or group changes). Empty: only a `Win` action wins. |
| `lose` | `[]` | `Vec<Condition>`: lost when any holds. |
| `time_limit` | 0 s | Counts down when > 0; `Lose` at zero. |
| `lives` | 0 | Player respawns before losing; 0 means unlimited respawns (health still applies). |
| `on_win` / `on_lose` | built-in screen | `EndScreen::Default` (themed "You win" / "Try again" with Restart, Next, Quit) or `EndScreen::Scene(name)`. |
| `next_scene` | `""` | What Next opens. |

**`GameState`** (runtime resource): `Playing`, `Paused`, `Won`, `Lost`; the elapsed time; lives left. Pausing scales time to 0 and opens the built-in pause menu (Resume, Restart, Quit) on the `pause` action, which the engine merges into every action map like `interact`. A project that builds its own menus sets `GameRules.builtin_menus = false`.

New `Condition` variants: `GroupDone { group }` (every `Interactable` in `group` has been used), `TimeAbove { seconds }`.

New `Interactable` field: `group: String` (default `""`). The Pickup preset sets `group: "pickups"`, so "collect everything" is `win: [GroupDone { group: "pickups" }]` with no counting by hand, and the HUD can show `3 / 10`.

### New actions and targets

`InteractAction` gains: `Damage { amount }`, `Heal { amount }`, `Win`, `Lose`, `Respawn`, `SetCheckpoint`, `SwitchScene { scene }`, `Burst { preset }` (a one-shot particle burst from a small built-in set: `Sparkle`, `Dust`, `Flame`), `Despawn { target }` (only `Transient` targets; others use `Hide`). `Target` gains `Interactor` (whoever triggered it: the player taking damage from a hazard).

New triggers for `Interactable`: `Enter` and `Exit` (in a trigger collider or radius; `Touch` stays the same as `Enter` with `once` semantics for compatibility) and `Timer { every }`.

### Presets

A preset is a commands file (the `box_core::edit` JSON format) with an `$at` placeholder, stored in `crates/box_engine/presets/*.commands.json` and compiled in. The editor's Add ▸ Gameplay menu, the command palette, `box scene apply` (a new command `{ "add_preset": { "preset": "pickup", "at": [x, y, z] } }`) and MCP all use the same files, so there is one definition.

| Preset | Components |
|---|---|
| Player | `PlayerController` on a capsule, the template humanoid as `Body`, a `FollowCamera` (created if the scene has no camera) |
| Pickup | Coin mesh, `Spinner`, `Bobber`, `Interactable { Touch, once, group: "pickups", actions: [AddVar score 1, PlaySound, Burst Sparkle, Hide] }` |
| Hazard | Spikes mesh, `Interactable { Enter, cooldown 0.8, actions: [Damage 1 → Interactor] }` |
| Moving Platform | Box, `Mover` with two points 4 m apart, `PingPong` |
| Checkpoint | Flag mesh, `Interactable { Enter, once, actions: [SetCheckpoint, Burst Flame] }` |
| Goal | Flag on a pad, `Interactable { Enter, actions: [Win] }` (optionally `requires: [GroupDone pickups]` with a blocked prompt) |
| Door to Scene | Arch, `Interactable { Press, prompt "Enter", actions: [SwitchScene] }` |
| Key and Lock | Key: `Touch` pickup that adds `keys`; Lock: `Press`, `requires keys ≥ 1`, `actions: [AddVar keys −1, Hide door]` |
| Lever | `Press`, `Toggle` on a target and `SetEnabled` |
| Enemy | Humanoid, `Chaser`, `Health { max 1 }`, `Hazard`-style contact damage |
| Spawner | Empty with `Spawner` and an area gizmo |

Preset meshes are procedural (`Primitive`, `Lathe`, `Extrude`) so they need no asset files and look consistent with the starter scene.

### Built-in HUD

**`HudSettings`** (scene resource): which vars to show (`[{ var: "score", label: "Score" }]`, defaults to every var a preset in the scene writes), group progress (`3 / 10`), the player's health as hearts or a bar, the timer when `time_limit > 0`. One retained `box_ui` panel styled by the theme (classes `hud`, `hud_value`, `hud_hearts`), updated only when a var revision, health or the displayed second changes. `HudSettings.enabled = false` for games that draw their own.

### Save games

Behaviour state that matters is in saved fields (`Health.current`, `Interactable.uses`, `Mover` progress) or `GameVars`/`GameState` save hooks, so checkpoints and save games work without game code. `Spawner` spawns are `Transient` and are not saved.

### Eject to script

Inspector header menu ▸ **Convert to Script** (and the `eject_behaviour` command): writes `assets/scripts/<behaviour>.luau` with the behaviour's logic and its current values as `properties`, attaches it as a `Script` and removes the component, as one undo step. The Luau sources live next to the Rust (`behaviours/luau/*.luau`) and are the same code the template used to ship. A test plays each acceptance scene twice, once with components and once fully ejected, and compares playtest fingerprints so the two never drift.

### Editor

- Add ▸ Gameplay lists the presets with an icon and a one-line description; the palette finds them by name ("add coin" finds Pickup).
- Viewport gizmos: radius spheres for `Interactable`, `Mover` paths with draggable points (through `path_tool`), `Spawner` areas, `Chaser` sight rings, shown for the selection.
- Scene Settings gets a Rules section (`GameRules`, `HudSettings`).
- A behaviour's inspector heading has a "?" that opens its section of the guide.

### CLI and MCP

The types go through the registry, so `box registry`, `scene apply`, `scene show` and the live tools cover them with no per-type code. New: `add_preset`, `eject_behaviour` commands; `box preset list --json`; MCP `list_presets`. `box check` warns about rules that cannot be met (a `GroupDone` group with no members, a `Goal` that requires a var nothing writes, a scene with `PlayerController` but no camera).

## Acceptance

- **Template.** The third-person template is rebuilt from presets with no `.luau` files and the same feel (its playtest still passes). The scripted version moves to the examples gallery (`third-person-scripted`).
- **First game example** (`examples/first_game`, no Luau): player, camera, 12 coins, two moving platforms, spike hazards, a checkpoint, a chaser enemy, a goal requiring all coins, a 3-minute limit, 3 lives, built-in HUD, pause and end screens. Scenes from `authoring/*.commands.json` using only `add_preset` and `set`, which is the proof that an agent can build it.
- `crates/box_engine/tests/behaviours.rs`: a playtest finishes the game (win), another dies three times (lose), another uses a checkpoint and a save/load; each repeats with every behaviour ejected and must give the same fingerprints. Captures in `target/captures/behaviours/` (`hud.png`, `pause.png`, `win.png`, `lose.png`).
- `crates/box_editor/tests/behaviours.rs`: Add ▸ Gameplay ▸ Pickup places one on the surface, Play collects it, Convert to Script is one undo step; capture of the gizmos.
- Cost: 1,000 pickups, 100 movers, 50 chasers under 0.5 ms of behaviour time on the reference Mac (`--release --test behaviours cost`).
- **User test:** someone who has not used box-engine follows the new guide (`docs/guide/first-game.md`) from install to export without help. Record the time and every point where they got stuck in `docs/measurements/`.

## B1 as built

Code: `crates/box_engine/src/behaviours/`, `crates/box_engine/presets/`, `crates/box_core/src/preset.rs`, the `add_preset` command and `EditStack::ensure_resource` in `crates/box_core/src/edit/`, Add ▸ Gameplay and Add Scene Setting in the editor (`crates/box_editor/src/presets.rs`). Test: `crates/box_engine/tests/behaviours.rs`; the third-person template test plays the converted template.

Where it differs from the proposal above:

- `PlayerController` has an absolute `run_speed` (10 m/s) instead of `run_multiplier`.
- No new triggers: `Touch` already fires on entering. `Exit` and `Timer` wait for a game that needs them.
- `Damage`, `Heal` and `Despawn` come with `Health` and `Spawner` in B2. The Hazard preset uses `Respawn` (costing a life when `GameRules.lives` is set).
- The Pickup preset hides itself (`Hide`) instead of despawning, so save games and Restart bring it back without extra state.
- `Mover` follows `points` only; a `Spline` path comes later.
- `GameRules` has `menus` (on/off), `win_title` and `lose_title` instead of `EndScreen`; a custom end scene is a `SwitchScene` action. Restart restores an in-memory save of the scene taken when it started.
- `HudSettings.vars` empty shows every game variable that has a value.
- Resources: `set_resource` adds a missing registered resource with its defaults (one undo step), which is how presets' companions (`GameRules`, `HudSettings`) get into a scene from commands; the editor's Scene Settings has Add Scene Setting.
- Behaviour types have common names that games also use (`PlayerController`, `FollowCamera`, `Mover`, `GameState`; the collector example and the native guide define their own). They register with `Registry::register_yielding`: a game type with the same short name takes the name over instead of failing to register, and presets that use the engine type are left out for that game.
- The third-person template's Luau version is `examples/third_person_scripted` (Open Example ▸ Third-person (Luau)).

## B2a as built

Code: `crates/box_engine/src/behaviours/{health,spawner}.rs`, the `Damage`, `Heal` and `Despawn` actions and `InteractRequests::run_actions` (action lists run for an owner, used by `Health.on_damage`/`on_zero`) in `crates/box_engine/src/interact.rs`, presets `door_to_scene`, `key_and_lock`, `lever`, `spawner`; `Health` on the Player preset. Test: `crates/box_engine/tests/behaviours_b2.rs` (captures `health_hud.png`, `spawner.png`).

- `Health.on_zero` left empty respawns the player (refilled; a life with `GameRules.lives`) and removes anything else (`Despawn`: spawned copies despawn, saved entities hide). `Damage` on a player without `Health` respawns it, so hazards made with `Damage` work on old scenes.
- `Spawner` copies its first child (or `template`) with fresh uids as `Transient` scene roots; the template gets a runtime `SpawnTemplate` marker and is hidden and non-collidable while playing. Copy spots come from a hash of the spawner's uid and count, so a scene replays the same. `total` and `lifetime` were added.
- Interactable groups count only saved interactables (no spawned copies, no template); the HUD leaves out groups with nothing to count.
- The HUD shows the player's health (`HudSettings.health`).
- Door to Scene's `SwitchScene` starts empty; using it before a scene is typed warns. `box check` warnings come with B2b.

## B3 as built

Code: `Command::EjectBehaviour` and `Applied::files` (box_core `edit`), `BehaviourScript` and `Registry::register_behaviour_script` (box_core `preset`), `crates/box_engine/src/behaviours/scripts.rs` and `behaviours/luau/{spinner,bobber,mover}.luau`, `write_missing_files` (used by `box scene apply`, `control::apply_to_world` for MCP and the editor), the inspector's ⋯ ▸ Convert to Script (`EditorState::convert_to_script`). `examples/first_game` (presets and `set` only; in the Open Example gallery). Tests: `crates/box_engine/tests/behaviours_b3.rs`, `crates/box_engine/tests/first_game.rs` (capture `target/captures/first_game/won.png`), `crates/box_editor/tests/convert_to_script.rs` (capture `editor_converted.png`).

Where it differs from the proposal:

- One shared file per behaviour (`scripts/behaviours/<name>.luau`), written only when missing; each converted copy keeps its values as properties. Undo restores the component and leaves the file.
- Script properties hold no lists, so a Mover's points become `point_1` … `point_8` with `point_count`; a Mover with more points is refused with the reason.
- Script versions exist for Spinner, Bobber and Mover. Player, FollowCamera, Chaser, Health and Spawner have none yet (their Luau equivalents need APIs scripts lack: health damage with invulnerability, the game rules' respawn); `examples/third_person_scripted` remains the Luau player and camera.
- Equivalence is checked by playing each behaviour next to its converted copy (largest difference: one frame of motion, because a script starts a frame later), not by playtest fingerprints, which round to millimetres.
- `box render` and `box walk` add the default game actions, so scenes with a player no longer warn about unknown actions there.
- Not done: the user test with someone new to the engine (needs a person), and recording it in `docs/measurements/`.

## Phases

| Phase | Scope |
|---|---|
| B1 (built) | `PlayerController`, `FollowCamera`, `Spinner`, `Bobber`, `Mover`; new actions and targets; `group` and `GroupDone`; `GameRules`/`GameState`, pause and end screens; built-in HUD; presets Player, Pickup, Hazard, Moving Platform, Checkpoint, Goal; `add_preset`. Template rebuilt. |
| B2 | `Health`, `Spawner`, `Chaser` on navigation (D21); presets Enemy, Spawner, Door to Scene, Key and Lock, Lever; gizmos; `box check` rule warnings. Split into B2a, N1, N2, B2b in [NAVIGATION.md](NAVIGATION.md#b2-with-navigation). |
| B3 | Eject to script with the fingerprint tests; `examples/first_game`; first-game guide; project `AGENTS.md` section on presets; the user test. |

## Decision

### D19. Behaviours and presets (proposed)

**Decision.** How a newcomer makes a complete game without code.

**Recommendation.** Engine components for the behaviours every small 3D game rewrites (player, camera, motion, health, spawner, chaser on a navmesh), scene-level `GameRules` with built-in HUD, pause and end screens, all wired through D18's `Condition`/`InteractAction` vocabulary. Presets are compiled-in commands files shared by the editor, CLI and MCP. Each component can be ejected into an equivalent Luau script, verified by playtest fingerprints.

**Alternatives.** (1) Ship better scripts in templates: what exists today; every scene carries copies, agents still write Luau, and changing a speed means reading code. (2) A visual scripting graph: a non-goal for the first release and a much larger editor investment. (3) A separate `box_gameplay` crate: cleaner boundary, but `InteractPlugin` already lives in box_engine and the behaviours need its types; revisit if the folder grows past a few thousand lines.

**Trade-offs.** A fixed set of behaviours suits a genre (third-person action and collect games) better than others; first-person, top-down and side-on are covered by `FollowCamera` modes, anything further is scripts. More saved types to keep stable (docs/STABILITY.md).

**Reversibility.** Medium (scene files hold the components; eject gives an exit).

**Confidence.** Medium-high.

## Open questions

1. Does the third-person template switch to behaviours (recommended), or keep scripts with a separate `first-game` template?
2. Is `Chaser` in scope before navigation exists, or does the first release stop at hazards and moving obstacles? **Proposed answer: navigation comes with B2 (D21, [NAVIGATION.md](NAVIGATION.md)), so `Chaser` paths around walls.**
3. Should presets also be user-definable (a project's `presets/` folder of commands files), or engine-only at first? (Recommended: engine-only in B1–B3, user presets later, since `.scene.json` prefabs already cover reusable props.)
