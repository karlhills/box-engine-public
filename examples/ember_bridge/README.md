# Ember Bridge

A small puzzle made without puzzle code: carry each ember to a brazier to raise a bridge across the water. Everything the player uses is an `Interactable` component configured in the scene; the only scripts are the player, the follow camera and a HUD that shows how many embers you carry.

**Controls.** WASD, the arrow keys or the left stick move; Space or South jumps; **E** (or the gamepad's North button) uses the brazier in front of you. Click the game view for mouse look, Escape releases it.

## Run it

```bash
box-editor .                 # open it; press Play (Cmd+P / Ctrl+P)
box run .                    # play it in a window
```

## How it is put together

| Entity | What it does |
|---|---|
| `Ember West`, `Ember Middle` | The `props.glb` ember mesh with an `Interactable` (`trigger: Touch`, `once`): walking into it adds 1 to the `embers` game variable, hides the ember and plays a sound. |
| `Brazier West`, `Brazier Middle` | A `PrefabInstance` of `prefabs/brazier.scene.json` (the `props.glb` brazier, a `Flame` particle emitter and a `Glow` light, both off) with an `Interactable` on the placed root: it requires `embers >= 1`, spends the ember, starts `Child` `Flame`, lights `Child` `Glow`, swaps the ash coals for glowing ones (`SetField`) and shows its bridge, picked as an `Entity` target. Both braziers use the same prefab; only the bridge differs. |
| `Bridge West`, `Bridge East` | Wooden slabs that start with `Visible` and `Collidable` off; `Show` turns both on, so the bridge appears and can be walked on. |
| `HUD` | `scripts/hud.luau`: shows the `embers` variable (`vars.on_change`) and reacts to the `Interacted` event. |
| `Player`, `Body`, `Camera` | From the third-person template; `Player` has an `Interactor` component. |

The scene files are generated from commands, the way an agent builds a scene:

```bash
cp authoring/empty.scene.json assets/prefabs/brazier.scene.json
box scene apply assets/prefabs/brazier.scene.json authoring/brazier.commands.json
cp authoring/empty.scene.json assets/scenes/main.scene.json
box scene apply assets/scenes/main.scene.json authoring/main.commands.json
```

The interactions guide (`docs/guide/interactions.md` in the engine repository) explains every field and action.
