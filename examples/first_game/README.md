# First Game

A whole small game made only from presets: no scripts and no code. Collect all twelve coins (two ride on moving platforms, two are up the ramp on the stone ledge), avoid the spikes and the guard who chases you when he sees you, then step on the goal before the three minutes run out. You have three lives and three health.

**Controls.** WASD, the arrow keys or the left stick move; Space or gamepad South jumps; Shift runs. Click the game view for mouse look; Escape releases it; P pauses.

## Run it

```bash
box-editor .                 # open it; press Play (Cmd+P / Ctrl+P)
box run .                    # play it in a window
```

## How it is put together

Everything is a preset placed with `add_preset` and tuned with `set` (docs/guide/behaviours.md):

| Entity | Preset | Tuned |
|---|---|---|
| `Player`, `Player Camera` | Player | — |
| `Coin 1` … `Coin 12` | Pickup | `Coin 11` and `Coin 12` are children of the platforms, so they ride along |
| `Platform West`, `Platform South` | Moving Platform | `Platform South` slides 10 m along x |
| `Spikes 1` … `Spikes 3` | Hazard | — |
| `Checkpoint` | Checkpoint | — |
| `Guard` | Enemy | `Chaser.patrol`: walks 8 m up and down behind the wall |
| `Goal` | Goal | `Interactable.requires`: `GroupDone pickups`, so it waits for every coin |
| `Ledge`, `Ramp`, `Wall`, `Ground`, `Sun` | plain shapes with colliders | — |

`GameRules` sets a 180 s time limit, 3 lives and the win title; `HudSettings` shows coins, health, lives and time.

The scene is generated from commands, the way an agent builds one:

```bash
cp authoring/empty.scene.json assets/scenes/main.scene.json
box scene apply assets/scenes/main.scene.json authoring/main.commands.json
box check .                  # no warnings: every rule can be met, the guard can reach you
```

Select a behaviour in the inspector and choose ⋯ ▸ Convert to Script to see it as Luau (Spinner, Bobber and Mover have script versions).
