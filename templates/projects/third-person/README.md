# Third-person game

A small third-person game made only of a scene and assets, with no code at all: walk the skinned humanoid around, jump onto the platforms and collect the five pickups to win. Every piece of gameplay is a *behaviour*, a component you tune in the inspector. Created with `box new <folder> --template third-person`, or File ▸ New Project ▸ Third-person game in the editor.

**Controls.** WASD, the arrow keys, the d-pad or the left stick move relative to the camera; hold Shift (gamepad West) to run. Space or the gamepad's South button jumps. The right stick turns the camera; click the game view to capture the mouse for mouse look, Escape releases it. P, Escape or the gamepad's Start button pauses.

## Run it

```bash
box-editor .                 # open it; press Play (Cmd+P / Ctrl+P)
box run .                    # play it in a window
box run . --frames 60        # headless, no window
```

## How it is put together

The scene is `assets/scenes/main.scene.json`. Select an entity in the editor's hierarchy to see its components in the inspector; hover a field for what it does.

| Entity | Components | What it does |
|---|---|---|
| **Player** | `PlayerController`, `KinematicCharacter`, `Collider` (capsule), `Interpolated` | Walks and runs with the `move` and `run` actions relative to the camera, jumps with `jump`, climbs steps and slides along walls. Falling off the world puts it back where it started. |
| **Body** (child of Player) | `PrefabInstance` (`models/character.glb`), `AnimationStateMachine` (`animation/locomotion.animgraph.json`) | The visible humanoid. The player controller turns it and sets the `speed`, `grounded` and `jump` parameters, so Idle, Walk, Run and Jump blend. |
| **Camera** | `Camera`, `FollowCamera` | Follows the player and orbits with the right stick or the mouse; pulls in when a wall is in the way. |
| **Pickup 1–5** | `Primitive`, `Spinner`, `Bobber`, `Interactable` (Touch, group `pickups`) | Spin and bob; when the player walks into one it adds 1 to `score`, plays `audio/pickup.wav`, sparkles and disappears. |
| **Ground**, **Platform 1–3**, **Low Wall**, **Crate**, **Pillar** | `Primitive`, `StandardMaterial`, `Collider` | Things to stand on and bump into. |

Scene Settings holds two more pieces: `GameRules` (the game is won when the `pickups` group is done; the pause menu and the win screen) and `HudSettings` (the score and pickup count in the top-left corner).

| File | What it is |
|---|---|
| `assets/settings/input.actions.json` | The named input actions (`move`, `jump`, `look`) and their keys, buttons and sticks. `run` and `pause` have built-in defaults. |
| `assets/animation/locomotion.animgraph.json` | The animation state machine: states, clips and transitions. |
| `assets/models/character.glb` | The skinned humanoid with its Idle, Walk, Run and Jump clips. |
| `assets/audio/pickup.wav` | The pickup sound. |

## Things to try

- **Tune the feel.** Select Player and change the `PlayerController`'s **Speed** and **Run Speed**, or the `KinematicCharacter`'s **Jump Speed** and **Gravity**. During Play, tick **Keep after Stop** on a field to keep a change.
- **Add more.** Add ▸ Gameplay places a Pickup, a Hazard, a Moving Platform, a Checkpoint or a Goal at the centre of the view. A new pickup counts toward the win by itself.
- **Change the rules.** In Scene Settings, give `GameRules` a **Time Limit** or **Lives**; the HUD shows them.
- **See it as code.** The editor's Open Example ▸ Third-person (Luau) is this game written as commented Luau scripts.
