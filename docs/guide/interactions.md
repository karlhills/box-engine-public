# Interactions

Make props the player can use without writing code: a brazier that needs an ember, a lever that opens a gate, a pickup that counts. You configure them with the `Interactable` component, in the inspector, with `box scene apply` or through MCP. Scripts are there for what the built-in actions do not cover.

The worked example is [`examples/ember_bridge`](../../examples/ember_bridge): two embers, two braziers and two hidden bridges, with no puzzle code. Play it with `box run examples/ember_bridge` (WASD to move, E to use).

## Contents

- [How it works](#how-it-works)
- [Make a prop usable in the editor](#make-a-prop-usable-in-the-editor)
- [The fields](#the-fields)
- [Conditions and game variables](#conditions-and-game-variables)
- [Actions](#actions)
- [Targets and prefabs](#targets-and-prefabs)
- [Hidden at the start](#hidden-at-the-start)
- [Who the player is](#who-the-player-is)
- [Save games](#save-games)
- [Scripts](#scripts)
- [From the command line and MCP](#from-the-command-line-and-mcp)
- [Limits](#limits)

## How it works

An entity with an `Interactable` component is something the player can use. When the player comes within its `radius`, a prompt such as **[E] Light the brazier** appears above it (only for the nearest one). Pressing the `interact` action (E, or the gamepad's North button) checks its `requires` conditions; if they all hold, its `actions` run in order. A `Touch` interactable fires as soon as the player walks into range, with no prompt: use it for pickups.

Conditions and some actions use **game variables**: named numbers such as `embers` or `gate_open`. They start at 0, are kept across scene switches and are saved in save games.

## Make a prop usable in the editor

1. Place the prop: drag a glTF model or a prefab scene (`.scene.json`) from the Assets pane into the viewport. The entity you get is the prefab's root.
2. With it selected, choose **Add Component ▸ Gameplay ▸ Interactable**.
3. Set **Prompt** ("Light the brazier") and, if it needs something, **Blocked prompt** ("Needs an ember").
4. Under **Requires**, press **+** and pick `AtLeast` with **Var** `embers` and **Value** 1.
5. Under **Actions**, press **+** for each step and pick its kind: `AddVar` (`embers`, −1), `StartParticles` (target `Child` `Flame`), `Show` (target `Entity`, then pick the bridge in the list).
6. Press Play, walk up to it and press E.

To place another one, duplicate it (Ctrl+D) and pick a different bridge in the copy's `Show` action. Each placed copy has its own settings.

## The fields

| Field | Default | Meaning |
|---|---|---|
| `enabled` | true | Off: no prompt, cannot be used. Another interactable's `SetEnabled` action switches it. |
| `trigger` | `Press` | `Press`: the player presses `action` in range. `Touch`: fires when the player comes within `radius`. |
| `action` | `interact` | The input action to press. `interact` exists in every project (E, gamepad North); others come from `assets/settings/input.actions.json`. |
| `radius` | 2 m | How close the player must be to the interaction point. |
| `offset` | (0, 1, 0) | The interaction point and prompt position: the entity's origin plus this, in world axes. |
| `prompt`, `prompt_key` | `Use` | The prompt text, or a string-table key (wins when set; see the scripting guide's UI and text section). |
| `blocked_prompt`, `blocked_prompt_key` | empty | Shown while a condition fails. Empty: no prompt while blocked. |
| `requires` | none | Conditions that must all hold. |
| `actions` | none | What happens, in order. |
| `once` | false | Usable once (pickups, one-shot puzzles). |
| `cooldown` | 0.25 s | Minimum time between uses. |
| `uses` | 0 | How many times it was used. This is its state; you do not usually set it. |

The prompt's look comes from the UI theme: the styles `prompt` (the box), `prompt_key` (the key chip) and `prompt_text`. Override them in your theme file.

## Conditions and game variables

| Condition | Holds when |
|---|---|
| `AtLeast { var, value }` | the variable is at least `value` |
| `Below { var, value }` | the variable is less than `value` |
| `Equals { var, value }` | the variable equals `value` |
| `GroupDone { group }` | every `Interactable` whose `group` field is this name was used (and there is at least one): "collect all the pickups" |
| `TimeAbove { seconds }` | more than `seconds` of game time passed since the scene started |

Variables are changed by the `AddVar` and `SetVar` actions and by scripts (`vars.set`, `vars.add`). A puzzle with steps is a variable that counts them: a lever does `SetVar { var: "gate_open", value: 1 }`, and the gate's interactable requires `Equals { var: "gate_open", value: 1 }`.

## Actions

| Action | What it does |
|---|---|
| `Show { target, collide }` | Makes the target visible; with `collide` (on by default) also solid. |
| `Hide { target, collide }` | Hides the target; with `collide`, the player walks through it. |
| `Toggle { target, collide }` | Flips visibility (and collision): levers, doors. |
| `StartParticles { target }`, `StopParticles { target }` | Starts or stops the target's `ParticleEmitter`. |
| `SetLight { target, intensity }` | Sets the target's `PointLight` or `SpotLight` intensity. |
| `AddVar { var, amount }`, `SetVar { var, value }` | Changes a game variable. Spending an ember is `AddVar` with −1. |
| `SetEnabled { target, enabled }` | Switches another `Interactable`, or a behaviour (`Mover`, `Spinner`, `Bobber`, `FollowCamera`, `PlayerController`), on or off. |
| `PlaySound { clip, volume }` | Plays a sound once at the interaction point. |
| `Signal { name }` | Sends a script signal `name` with `{ entity, interactor }`, for `events.on(name, ...)`. |
| `CallScript { target, function }` | Calls `function(self, interactor)` on the target's script. |
| `SetField { target, path, value }` | Sets any component field: `path` as in scene commands (`PointLight.color`), `value` as JSON text (`[1, 0.5, 0.2, 1]`). |
| `Win`, `Lose` | Wins or loses the game: the scene's `GameRules` show the win or lose screen ([Behaviours](behaviours.md)). |
| `Respawn` | Sends the player back to the last checkpoint (or where it started); with `GameRules.lives`, it costs a life. Hazards use it. |
| `SetCheckpoint` | Makes the interaction point the place the player respawns. |
| `SwitchScene { scene }` | Switches to another scene (`level2`, or a path). |
| `Burst { style, count }` | A one-shot particle burst at the interaction point: `Sparkle`, `Dust` or `Flame`. |
| `Damage { target, amount }` | Lowers the target's `Health` (default target: whoever used it); ignored while it is invulnerable after a hit. A player without `Health` respawns instead. |
| `Heal { target, amount }` | Raises the target's `Health`, up to its maximum. |
| `Despawn { target }` | Removes a spawned copy (a `Spawner`'s, or a script's `transient` spawn); anything saved in the scene is hidden instead, with its colliders off. |

The ember_bridge braziers also swap their coals from ash to glowing embers with `SetField` on `MeshAsset.materials`.

## Targets and prefabs

Every action that changes an entity has a `target`:

- `This`: the interactable itself.
- `Child` with a name: a descendant with that `Name`, including the contents of a prefab. The brazier prefab (`assets/prefabs/brazier.scene.json`) has children `Flame` and `Glow`, so `Child` `Flame` works for every placed brazier.
- `Entity`: any saved entity in the scene, picked in the inspector's list (`{ "Entity": "<uid>" }` in files). Use it for what differs per copy: the bridge each brazier reveals.
- `Interactor`: whoever used it, the player.

The `group` field puts interactables in a named group: the `GroupDone` condition and the built-in HUD (`Pickups 3 / 12`) count its members.

Put the `Interactable` on the placed prefab's root, not inside the prefab. The root is saved with the scene, so every copy keeps its own settings, and `Child` targets still reach into the prefab's contents.

## Hidden at the start

Something revealed later (a bridge, a secret door) starts with **Visible** off and **Collidable** off. `Visible(false)` only hides drawing; `Collidable(false)` switches off the colliders of the entity and everything under it, including a glTF prefab's mesh colliders. `Show` turns both on.

## Who the player is

The entity with an `Interactor` component. Without one, the entity with a `PlayerController`, else the `KinematicCharacter` named `Player` (or the only character). If a scene has several characters and none is marked, the first is used and a warning says to add `Interactor`.

## Save games

Save games keep each interactable's `uses` and `enabled`, the game variables, and the visibility and collision of saved entities (the bridges). Changes to prefab contents (the brazier's flame and light) are not stored; when a save loads, every interactable that was used applies its `Show`, `Hide`, `Toggle`, particle, light, `SetEnabled` and `SetField` actions again to targets inside prefabs. Variables, sounds, signals and script calls are not repeated.

## Scripts

A script on the same entity hears each use in `on_interact`, after the built-in actions ran (from `examples/guide_samples/assets/scripts/altar.luau`):

```lua
function Altar.on_interact(self, interactor: Entity)
    self.used_by = interactor:name()
    vars.add("visits", 1)
end
```

Every use also sends the `Interacted` event to any script:

```lua
    events.on("Interacted", function(e)
        local used = world.entity(e.entity)
        if used ~= nil then
            log.info(used:name() .. " was used")
        end
    end)
```

A HUD follows a variable with `vars.on_change`:

```lua
    vars.on_change("offerings", function(change)
        self.offerings = change.value
    end)
```

`interact.use(entity)` uses an interactable from a script (its conditions apply, its distance does not), `interact.can_use(entity)` tells whether it would work, and `interact.focus()` returns the one whose prompt shows. See the reference pages for [`interact`](../reference/interact.md) and [`vars`](../reference/vars.md).

A custom condition is a script that sets a variable the interactable requires.

## From the command line and MCP

`Interactable` is an ordinary saved component, so `box registry --json` lists its fields and `box scene apply` (MCP `scene_apply`, or `apply_commands` on a running game) edits it. The ember_bridge scene is made this way, from `examples/ember_bridge/authoring/main.commands.json`. A brazier:

```json
{
  "op": "add_component", "entity": "Brazier West", "component": "Interactable",
  "value": {
    "radius": 2.4, "offset": [0, 1.7, 0],
    "prompt": "Light the brazier", "blocked_prompt": "Needs an ember",
    "requires": [{ "AtLeast": { "var": "embers", "value": 1 } }],
    "actions": [
      { "AddVar": { "var": "embers", "amount": -1 } },
      { "StartParticles": { "target": { "Child": "Flame" } } },
      { "SetLight": { "target": { "Child": "Glow" }, "intensity": 40 } },
      { "Show": { "target": { "Entity": "eb000000-0000-4000-8000-00000000001f" } } }
    ],
    "once": true
  }
}
```

Point a placed copy at another bridge with one `set`:

```json
{ "op": "set", "entity": "Brazier Middle", "path": "Interactable.actions[3]", "value": { "Show": { "target": { "Entity": "eb000000-0000-4000-8000-000000000020" } } } }
```

In a running game started with `--control`, `send_input` with `{"action": "interact"}` presses the action, and `world_query` with `include_resources` shows `GameVars`.

## Limits

- One prompt at a time, for the nearest interactable in range; there is no facing check.
- No timed sequences (`Wait`) and no dialogue choices: use a script (`CallScript`, `Signal`, `on_interact`).
- An `Interactable` placed inside a prefab's source scene works, but cannot be changed per copy. Put it on the placed root instead.
