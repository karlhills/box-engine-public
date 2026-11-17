# Third-person game

A small third-person game made only of a scene, assets and Luau scripts (no Rust): walk the skinned humanoid around, jump onto the platforms and collect the five pickups. Created with `box new <folder> --template third-person`, or File ▸ New Project ▸ Third-person game in the editor.

**Controls.** WASD, the arrow keys, the d-pad or the left stick move relative to the camera (a light push walks, full input runs). Space or the gamepad's South button jumps. The right stick turns the camera; click the game view to lock the mouse for mouse look, Escape releases it.

## Run it

```bash
box-editor .                 # open it; press Play (Cmd+P / Ctrl+P)
box run .                    # play it in a window
box run . --frames 60        # headless, no window
```

## How it is put together

The scene is `assets/scenes/main.scene.json`. Select an entity in the editor's hierarchy to see its components in the inspector.

| Entity | Components | What it does |
|---|---|---|
| **Player** | `KinematicCharacter`, `Collider` (capsule, layer 2), `Interpolated`, `Script` (`scripts/player.luau`) | The character controller: walks, jumps, climbs steps, slides along walls. The script turns the `move` and `jump` actions into `character.move` / `character.jump`. |
| **Body** (child of Player) | `PrefabInstance` (`models/character.glb`), `AnimationStateMachine` (`animation/locomotion.animgraph.json`) | The visible humanoid. Idle, Walk, Run and Jump blend from the `speed`, `grounded` and `jump` parameters the player script sets. |
| **Camera** | `Camera`, `Script` (`scripts/camera.luau`) | Follows the player and orbits with the right stick or the mouse; pulls in when a wall is in the way. |
| **Pickup 1–5** | `Primitive`, `Collider` (trigger on layer 4, mask 2), `Script` (`scripts/pickup.luau`) | Spin and bob; when the player touches one it plays `audio/pickup.wav`, bursts into particles, sends `picked_up` and despawns. |
| **HUD** | `Script` (`scripts/hud.luau`) | The score panel in the top-left corner. |
| **Ground**, **Platform 1–3**, **Low Wall**, **Crate**, **Pillar** | `Primitive`, `StandardMaterial`, `Collider` | Things to stand on and bump into. |

| File | What it is |
|---|---|
| `assets/scripts/*.luau` | The four scripts above plus `util.luau`, shared helpers loaded with `require("./util")`. Every script is commented. |
| `assets/settings/input.actions.json` | The named input actions (`move`, `jump`, `look`) and their keys, buttons and sticks. |
| `assets/animation/locomotion.animgraph.json` | The animation state machine: states, clips and transitions. |
| `assets/models/character.glb` | The skinned humanoid with its Idle, Walk, Run and Jump clips. |
| `assets/audio/pickup.wav` | The pickup sound. |

## Things to try

- **Tune the feel.** Select Player and change the script's **Speed**, or the `KinematicCharacter`'s **Jump Speed** and **Gravity**. During Play, tick **Keep after Stop** on a field to keep a change.
- **Add pickups.** Select a pickup and duplicate it (Cmd+D / Ctrl+D), then move the copy. The HUD counts them by itself.
- **Edit a script while playing.** Save a `.luau` file and it reloads at once; errors show in the Console with `file:line`.
- **Code completion.** Run `box script types` in this folder and open it in VS Code with the Luau Language Server extension.

The scripting guide (`docs/guide/scripting.md` in box-engine) explains every callback and API used here; `examples/collector_luau` is a fuller version of this game with a pause menu, saves, two arenas and two languages.
