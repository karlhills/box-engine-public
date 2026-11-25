# Interactions

Status: **accepted and built** (D18 in [DECISIONS.md](DECISIONS.md), confirmed 2026-11-21). User guide: [guide/interactions.md](guide/interactions.md). Code: `crates/box_engine/src/interact.rs`, `script_glue/interact.rs`; example `examples/ember_bridge`.

## Goal

A short puzzle or exploration game should be "import art, connect a few reusable interactions, make the puzzles good". Any prop, a glTF prefab or a `.scene.json` used as a `PrefabInstance`, becomes something the player can use, configured by data, with no game code. Every field is reachable from the inspector, `box scene apply`, `box registry` and MCP, so an agent can build and revise interactions from a prompt.

The acceptance case, from building the Ember Hollow test game: a brazier from an asset pack is placed in a scene. Pressing E near it (1) checks that the player has an ember, (2) spends the ember, (3) lights its flame (a particle emitter and a light inside the prefab turn on) and (4) reveals a bridge (another entity becomes visible and collidable). A second brazier placed elsewhere picks a different bridge in the inspector.

## What exists today

- Pickups, checkpoints and doors are hand-written Luau scripts (`examples/cinder_keep/assets/scripts/checkpoint.luau`, `door.luau`, the Ember Hollow `pickup.luau`) driven by `on_trigger_enter`. Counters (score, embers) live in a game script's `self` table and are saved through `on_save`. There is no engine counter, no `interact` action and no prompt.
- `PrefabInstance` has no per-instance overrides (D16). But the instance root is an ordinary saved entity: any component on it is saved per instance. Its instanced contents are runtime-only and rebuilt from the source scene.
- `Visible(false)` hides drawing only (meshes, lights, particles, picking); colliders keep working. `Collider` has no on/off switch.
- The inspector shows an `Option<Uid>` field as a None/Some combo with a read-only uuid; switching to Some fails. The only entity picker is the one for script `entity` properties (`script_props.rs`).
- A Rust system cannot call a Luau function synchronously (the runtime is removed while stages run). Rust reaches scripts through `ScriptSignals` (`events.on`) and registered events.

## Proposal

### Components and resources (box_engine, always available)

**`Interactable`** (component, saved) on the entity the player uses, normally a `PrefabInstance` root:

| Field | Type | Default | Meaning |
|---|---|---|---|
| `enabled` | bool | true | Off: ignored (no prompt, no use). Actions can switch it. |
| `trigger` | `Press` \| `Touch` | `Press` | `Press`: the player presses `action` while in range. `Touch`: fires when the player comes within `radius` (pickups); no prompt. |
| `action` | string | `"interact"` | Input action name. The engine adds `interact` (E, gamepad North) to every action map with `merge_missing`, so projects work without settings. |
| `radius` | f32 (m) | 2.0 | Distance from the player to the entity's origin plus `offset`. |
| `offset` | Vec3 (m) | (0, 1, 0) | Interaction point and prompt anchor, relative to the entity. |
| `prompt` / `prompt_key` | string | `"Use"` / `""` | Prompt text, or a string-table key (wins when set). Shown as `[E] Light the brazier`; the key label is the action's first keyboard binding (else its first gamepad button). |
| `blocked_prompt` / `blocked_prompt_key` | string | `""` | Shown instead when `requires` fails ("Needs an ember"). Empty: no prompt while blocked. |
| `requires` | `Vec<Condition>` | [] | All must hold. |
| `actions` | `Vec<InteractAction>` | [] | Run in order when used. |
| `once` | bool | false | Disable after the first use (pickups, one-shot puzzles). |
| `cooldown` | f32 (s) | 0.25 | Minimum time between uses. |
| `uses` | u32 | 0 | State: times used. Saved, so save games keep it. |

**`Interactor`** (component, saved, no fields) marks the player. Without one, the `KinematicCharacter` named `Player` (else the first one) is the interactor, so the third-person template and existing games work unchanged; several characters with none marked or named `Player` log a warning once.

**`GameVars`** (resource, runtime, saved in save games through a `SaveHooks` entry, not in scene files): named numbers (`BTreeMap<String, f64>`), 0 when unset. They carry across scene switches. The puzzle state lives here: `embers`, `gate_open`. Live tools can read them (`world_query` resources) and scripts use the `vars` global.

**`Condition`** (enum): `AtLeast { var, value }`, `Below { var, value }`, `Equals { var, value }`.

**`InteractAction`** (enum, ordered):

| Variant | Effect |
|---|---|
| `Show { target, collide: true }` / `Hide { target, collide: true }` | `Visible` on or off, and with `collide` also `Collidable` (below), so the target's subtree collides or not. |
| `Toggle { target, collide: true }` | Flip visible (and collision). Levers, doors. |
| `StartParticles { target }` / `StopParticles { target }` | `ParticleEmitter.playing`. |
| `SetLight { target, intensity }` | `PointLight` / `SpotLight` intensity. |
| `AddVar { var, amount }` / `SetVar { var, value }` | `GameVars`. Spending an ember is `AddVar { var: "embers", amount: -1 }`. |
| `SetEnabled { target, enabled }` | Another `Interactable` on or off (a lever arms a door). |
| `PlaySound { clip, volume }` | One-shot at the interaction point (audio module; warns once without it). |
| `Signal { name }` | Script signal `name` with `{ entity, interactor }`, for `events.on(name, ...)`. |
| `CallScript { target, function }` | Calls `function(self, interactor)` on the target's script at the next script update (script module). |
| `SetField { target, path, value }` | Escape hatch: any reflected field through `box_core::edit` paths, `value` as JSON text (`"PointLight.color"`, `"[1, 0.5, 0.2]"`). |

**`Target`** (enum): `This` (the interactable), `Child(name)` (a descendant by `Name`, including prefab contents: the brazier's own `Flame`), `Entity(uid)` (any saved entity: the bridge). `Child` makes a prefab self-contained; `Entity` is what changes per placed instance.

Every use also sends a registered event **`Interacted { entity, interactor }`** (scripts: `events.on("Interacted", ...)`) and calls **`on_interact(self, interactor)`** on scripts attached to the interactable. These are the Luau hooks for anything custom: a script can veto nothing synchronously, so custom conditions are written as a script that sets a var.

### Per-instance configuration

The `Interactable` lives on the `PrefabInstance` root, which is saved per instance. Placing the same prefab twice gives two components; duplicating a configured brazier (Ctrl+D, or the `duplicate` command) copies its configuration, and only the `Entity` target needs changing. `Child` targets reach into the prefab's contents, so the flame and light inside the brazier prefab need no per-instance data. No general prefab override system is needed for this, so D16's "no per-instance overrides" stands.

An `Interactable` inside a prefab's source scene also works (its instanced copy runs), but cannot be changed per instance. Binding named slots on the instance root (`Target::Slot("bridge")`) is left for later, when a game needs it.

### Collision on and off

A new saved component, `Collidable(bool)` (box_physics, default true), is the physics counterpart of `Visible`: `false` switches off the colliders of the entity and all its descendants (queries, triggers, characters, rigid bodies), whatever theirs say. It is inherited because a glTF prefab's mesh colliders sit on its instanced children, which cannot be edited; a per-collider `enabled` field (the first proposal) could not hide them. `sync_physics` and the dynamics sync compute the switched-off set once per sync (one query over `Collidable` when none is off). `Show`/`Hide` set it on the target. `Visible` keeps its meaning (drawing only).

### Prompt

One retained `box_ui` text node (key chip and text), created once and anchored with `anchor_to_entity` to the focused interactable's `offset`. It is updated only when the focus or its text changes. The focus is the nearest enabled `Press` interactable within its radius (ties: the one most in front of the camera). Styled by the theme (`class: "interact-prompt"`), so games restyle it in their theme file. Hidden while a modal UI is open.

### Save games

`Interactable.uses` and `enabled` are components on saved entities, and `Show`/`Hide`/`SetLight` changes on saved targets are saved with them. `GameVars` saves through a hook. Changes made to prefab contents (the brazier's flame) are not saved, because those entities are rebuilt; on load, every interactable with `uses > 0` re-applies its state actions (`Show`, `Hide`, `Toggle` an odd number of times, particles, lights, `SetField`) to targets inside prefab contents, once the contents exist. Vars, sounds, signals and script calls are not repeated.

### Scripts

- `vars.get(name)`, `vars.set(name, value)`, `vars.add(name, amount)`, `vars.all()`, `vars.on_change(name, fn)` (a `vars:<name>` signal sent when the revision changed), `vars.off(id)`.
- `interact.focus()` (the focused interactable or nil), `interact.use(entity)` (run it as if the player pressed, conditions included), `interact.can_use(entity)`.
- `on_interact(self, interactor)` entity callback; `Interacted` event. Both `on_interact` and `CallScript` go through `box_script::ScriptCalls`, a queue of `function(self, args...)` calls delivered in the scripts' next Update (Rust cannot call into the VM while a stage runs).

Generated definitions and `docs/reference` are regenerated; committed `.box/types/box.d.luau` files too.

### Editor

- An `Option<Uid>` field (`Target::Entity`) is an entity picker: None plus named saved entities (generated ones left out), like script entity properties. This also fixes `Sweep::spline`, `Anchor::target` and `Joint::connected`. One-field tuple variants label their field with the variant name (`Child`, `Entity`) instead of `0`.
- `Interactable` and `Interactor` are in the Gameplay category of Add Component.
- Play mode uses the same plugin, so the prompt appears in the Play view.

### CLI and MCP

The types are registered in `engine_registry()`, so `box registry`, `box scene apply` (`add_component`, `set` with paths such as `Interactable.actions[3]`), `scene_show`, `world_query` and the live `apply_commands` cover them with no per-type code. `send_input { action: "interact" }` presses the action in a running game. The guide shows the brazier as a commands file.

### Not in this step

Dialogue choices, inventories of distinct items with icons, timed sequences (`Wait`), save-point semantics, per-instance slot bindings for prefab-embedded interactions, a visual node editor.

## Acceptance

- `examples/ember_bridge`: a data project (no Luau for the puzzle; a HUD script shows the count) with two embers (`Touch` pickups), two braziers (`.scene.json` prefab around the `props.glb` brazier mesh, with a `Flame` emitter, a `Glow` light and ash coals swapped by `SetField`) and two hidden bridges, scenes generated from `authoring/*.commands.json`.
- `crates/box_engine/tests/interact.rs` plays it with `Actions::inject` (walk, press `interact`): blocked prompt without an ember, ember picked up, ember spent, flame playing, glow lit, bridge visible and solid, the player walks across both bridges; a save loads back with the prefab flame relit. A second test presses `interact` through the control socket's `send_input`.
- Captures in `target/captures/interact/`: `1_blocked.png`, `2_prompt.png`, `3_bridge.png`, `editor_inspector.png`.
- `crates/box_editor/tests/interact.rs`: the entity picker row, an undoable pick of the other bridge, the brazier in editor Play. `examples/guide_samples` has an `Altar` (Touch interactable with a script) for the guide's Luau samples.
