# Behaviours: gameplay without code

Behaviours are components for the gameplay most small 3D games need: a player you control, a camera that follows it, things that spin, bob and move, and rules for winning and losing with a HUD, a pause menu and end screens. You add them in the editor and tune them in the inspector; no script is involved. When you want something they do not cover, a Luau script can do anything a behaviour does (the [scripting guide](scripting.md)), and both work side by side.

The `third-person` template is built this way: it has no scripts. `examples/third_person_scripted` is the same game written in Luau, if you want to see how each behaviour would look as code.

## Contents

- [Presets: ready-made objects](#presets-ready-made-objects)
- [The player](#the-player)
- [The camera](#the-camera)
- [Motion: Spinner, Bobber, Mover](#motion-spinner-bobber-mover)
- [Health](#health)
- [Spawner](#spawner)
- [Enemies: Chaser](#enemies-chaser)
- [Convert to Script](#convert-to-script)
- [Rules: winning and losing](#rules-winning-and-losing)
- [The HUD](#the-hud)
- [Input actions](#input-actions)
- [Save games](#save-games)
- [From the command line and MCP](#from-the-command-line-and-mcp)
- [Limits](#limits)

## Presets: ready-made objects

**Add ▸ Gameplay** places a ready-made object at the centre of the view, on the ground, as one undo step. The command palette (Cmd+K / Ctrl+K) finds them too: type "add pickup".

| Preset | What you get |
|---|---|
| Player | A capsule character with `PlayerController` and 3 `Health`, and a camera with `FollowCamera` aimed at it. |
| Pickup | A spinning, bobbing coin. Walking into it adds 1 to the `score` variable, sparkles and hides it. It is in the `pickups` group. |
| Hazard | Spikes. Touching them sends the player back to the last checkpoint. |
| Moving Platform | A platform sliding back and forth 6 m; the player rides on it. |
| Checkpoint | A flag; walking past it moves the respawn point here and lights the flag. |
| Goal | A glowing pad that wins the game when the player steps on it. |
| Door to Scene | A doorway; pressing E there opens another scene. Type the scene's name (`level2`) into its `SwitchScene` action. |
| Key and Lock | A locked door and its key. The key adds 1 to `keys`; pressing E at the door uses one and opens it. Without a key the door says "Needs a key". |
| Lever | A lever and a gate; each press opens or closes the gate. Point its first `Toggle` action at anything else to switch that instead. |
| Enemy | A red creature that chases the player when it sees them, around walls along the navmesh, and costs them 1 health on contact. It has 1 health itself. |
| Spawner | A pad that makes a spike ball every 3 s (at most 3, each for 10 s) within 3 m of itself. Touching a ball costs the player 1 health. Replace the ball (the spawner's child) to spawn something else. |

A preset is ordinary components: after placing one, select it and change anything. The pickup's behaviour, for example, is an `Interactable` (see [Interactions](interactions.md)) with the actions `AddVar`, `Burst` and `Hide`.

## The player

`PlayerController` moves the entity's `KinematicCharacter` (added with a capsule `Collider` when missing):

| Field | Default | What it does |
|---|---|---|
| `speed` | 6 m/s | Walking speed at full input. |
| `run_speed` | 10 m/s | Speed while `run` is held (Shift, gamepad West). |
| `jump` | on | Space or gamepad South jumps (height: the character's `jump_speed`). |
| `turn_rate` | 12 rad/s | How fast the body turns toward where it moves. |
| `fall_limit` | −20 m | Falling below this height respawns the player. |
| `camera` | none | The camera movement is relative to; none: the active camera. |
| `body` | none | The model that turns and animates; none: the first child with an `AnimationStateMachine`, else the player itself. |

The body's state machine gets the parameters `speed` (m/s), `grounded` (bool) and `jump` (trigger), so a locomotion graph like the template's `locomotion.animgraph.json` blends Idle, Walk, Run and Jump by itself.

The player is also the one that uses interactables, so you do not need an `Interactor` component.

## The camera

`FollowCamera` goes on the entity with the `Camera`.

| Field | Default | What it does |
|---|---|---|
| `target` | none | What to follow; none: the player. |
| `mode` | `Orbit` | `Orbit`: the right stick or the mouse turns it, and it pulls in when a wall is in the way. `Fixed`: keeps the direction of its saved rotation (top-down and side-on games). |
| `distance`, `height`, `pitch` | 7 m, 1.2 m, 0.35 rad | Where it sits: `distance` from a point `height` above the target, looking down by `pitch`. |
| `smoothing` | 10 | How quickly it catches up. |
| `look_speed`, `mouse_sensitivity` | 2.5 rad/s, 0.004 | Turn rates for the stick and the mouse. |
| `capture_mouse` | on | Clicking the game captures the mouse for looking; Escape lets it go. |

Movement is relative to the camera: pushing forward walks away from it. In Play a follow camera becomes the active camera and switches the others off, so a scene's starting camera can stay where it is.

## Motion: Spinner, Bobber, Mover

These move the entity from where it is when Play starts, so moving the entity in the editor moves its whole motion.

- **`Spinner`**: turns around `axis` (in the parent's axes) at `speed` rad/s.
- **`Bobber`**: floats up and down by `height` metres every `period` seconds. Copies in different places bob out of step.
- **`Mover`**: moves through `points` (offsets from its start) at `speed` m/s. `mode` is `PingPong` (back and forth), `Loop` (round and round) or `Once` (stops at the end); `wait` pauses at each end. A character standing on a mover rides with it. A lever can start one: give it `enabled: false` and use `SetEnabled` on it.

Every behaviour has `enabled`. The `SetEnabled` action switches it.

## Health

`Health` gives anything hit points: `max` (3) and `current`. The `Damage` action lowers it (the spike balls, a hazard you make with `Damage` instead of `Respawn`); `Heal` raises it up to `max`. After a hit nothing can hurt it for `invulnerable` seconds (0.8).

- `on_damage`: actions run with every hit (a sound, a burst; `Interactor` there is whoever hit it).
- `on_zero`: actions run when it reaches 0. Left empty, the player goes back to the last checkpoint with full health (costing a life when the rules count lives), and anything else disappears: a spawned copy is removed, anything saved in the scene is hidden, so Restart and save games bring it back.

Scripts and the inspector can set `current` directly; reaching 0 that way runs `on_zero` too. The HUD shows the player's health (`Health 2 / 3`).

## Spawner

`Spawner` makes copies of its first child (or of the entity in `template`) every `interval` seconds while fewer than `max_alive` exist, at random spots within `radius` metres. `total` stops it after that many copies (0: never); `lifetime` removes each copy after that many seconds (0: never). While playing, the child it copies is hidden and cannot be touched; edit it in the editor like any entity.

Copies are not saved: loading a save game or Restart starts the spawner again. They are not counted in interactable groups, so spawned coins do not stop "collect them all" from finishing. A `Despawn` action removes a copy.

## Enemies: Chaser

`Chaser` is a simple enemy. It waits at home, or walks between its `patrol` points (offsets from where it starts, with a `wait` at each), until it sees the player within `sight` metres with nothing in between (`line_of_sight`). Then it chases them at `chase_speed`, around walls and up ramps and stairs, and touching them costs `damage` health at most once every `attack_cooldown` seconds. When the player gets out of sight it goes to where it saw them last and looks around; beyond `give_up` metres, or when the search finds nothing, it walks home.

It walks on the navmesh, the walkable area of the scene, which is built from your colliders by itself: View ▸ Navigation shows it. `NavSettings` (Scene Settings) sets the size of the characters it is built for. Selecting an enemy shows its sight (orange ring), give-up distance (faint ring) and patrol (yellow).

To spawn enemies, put an Enemy under a Spawner in place of its spike ball. A script can walk any character somewhere with `e:move_to(point)` (it uses a `NavAgent`, which `Chaser` uses too); see the `nav` reference.

## Convert to Script

When a behaviour almost does what you want, turn it into a script and change the script. In the inspector, open the ⋯ menu of its section and choose **Convert to Script**. The component is replaced by a `Script` running `scripts/behaviours/<name>.luau`, which does the same thing; the values you set become the script's properties, so it carries on exactly as before. It is one undo step.

Every converted copy shares the one script file, each with its own property values. The file is written the first time and then left alone, so your edits stay when you convert more copies. Spinner, Bobber and Mover can be converted (a Mover's points become `point_1` … `point_8`, so at most eight). The `eject_behaviour` command does the same from `box scene apply` and MCP: `{ "op": "eject_behaviour", "entity": "Lift", "component": "Mover" }`.

## Rules: winning and losing

`GameRules` is part of the scene (Scene Settings). Without it, nothing below happens, so games that make their own menus are not affected.

| Field | Default | What it does |
|---|---|---|
| `win` | none | Conditions; the game is won as soon as all hold. `GroupDone { group: "pickups" }` is "collect them all". A `Win` action wins too. |
| `lose` | none | Conditions; the game is lost as soon as any holds. A `Lose` action loses too. |
| `time_limit` | 0 (none) | Seconds to win in. |
| `lives` | 0 (unlimited) | Respawns before the game is lost. |
| `menus` | on | The pause menu (`pause`: Escape, P, gamepad Start) and the win and lose screens. |
| `next_scene` | none | The win screen's Next button opens this scene. |
| `win_title`, `lose_title` | "You win!", "Game over" | The screens' titles. |

The pause menu has Resume, Restart and Quit; the win screen Next (with `next_scene`), Play again and Quit; the lose screen Try again and Quit. Game time stops while a menu is open. Restart puts the scene back the way it was when it started, game variables included. Quit stops Play in the editor and closes an exported game.

Checkpoints and hazards use the `SetCheckpoint` and `Respawn` actions; falling below the player's `fall_limit` respawns too. Where the game stands (playing, paused, won or lost, time, lives, checkpoint) is the `GameState` resource: scripts read it like any resource, and `world_query` shows it.

## The HUD

`HudSettings` (Scene Settings) shows a panel in the top-left corner:

- the game variables listed in `vars` (each with a `label`), or every variable that has a value when the list is empty;
- each interactable group's progress (`Pickups 3 / 12`), with `groups` on;
- the player's health, when it has a `Health` (`health`);
- lives left and the time left, when the rules set them.

It changes only when a value changes. Its look is the theme's `hud` style.

## Input actions

Behaviours read named actions. Projects get these when their action map lacks them; redefine any in `assets/settings/input.actions.json`.

| Action | Default bindings |
|---|---|
| `move` | WASD, arrow keys, d-pad, left stick |
| `look` | right stick |
| `jump` | Space, gamepad South |
| `run` | Left Shift, gamepad West |
| `pause` | Escape, P, gamepad Start |

In the editor's Play view Escape returns input to the editor, so use P or the gamepad to pause there.

## Save games

Behaviours keep their state in save games without any setup: the player's position and components, where every mover, spinner and bobber is (the `motion` save hook), and the game state (time, lives, checkpoint; the `game` hook). Game variables are saved as before (the `vars` hook).

## From the command line and MCP

Everything here is components and resources in the registry, so `box scene apply`, `box scene show`, `box registry` and the MCP tools cover it. Two command forms help:

```json
[
  { "op": "add_preset", "preset": "pickup", "at": [3.0, 0.0, -4.0] },
  { "op": "set_resource", "path": "GameRules", "value": { "win": [{ "GroupDone": { "group": "pickups" } }], "time_limit": 120 } },
  { "op": "set_resource", "path": "HudSettings", "value": {} }
]
```

`add_preset` places a preset (`box registry` lists them under `presets`) with fresh uids, and takes `name` and `parent`. `set_resource` adds a scene resource the scene does not have yet.

`box check` warns about rules that cannot work: a "collect them all" group nothing belongs to, a game variable nothing sets, a door with no scene, an empty spawner, a player with no camera, and an enemy that cannot walk to the player. `box nav . --from x,y,z --to x,y,z` shows where an enemy can walk.

## Limits

- One player per scene; several `PlayerController`s all read the same input.
- Enemies do not jump, climb ladders or ride moving platforms; one character size per scene for the navmesh.
- `Mover` follows straight lines between points; splines come later.
- Convert to Script covers Spinner, Bobber and Mover; the player, camera, enemy, health and spawner do not have script versions yet (`examples/third_person_scripted` shows a player and camera in Luau).
