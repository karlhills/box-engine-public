# Cinder Keep

The M8 game: a short third-person action-platformer written only in Luau, as a data project (no Rust), on levels modelled outside the editor and imported as glTF. Talk to the old warden, cross the Ember Fields, survive the cinderling wave in the courtyard, go through the gate and take the throne of the keep. It is played by the stock `box-player` and by the editor's Play mode.

**Controls.** Gamepad: left stick or d-pad moves (relative to the camera), South jumps, West or the right trigger swings the sword, the right stick turns the camera, Start pauses. Keyboard and mouse: WASD or arrows, Space jumps, J, F or the left mouse button swings (click once to lock the mouse for mouse look), Escape or P pauses. In menus and dialogue: d-pad or arrows move, South, Enter or Space confirm, East or Escape close.

**Run.**

```bash
cargo run -p box_player_app -- examples/cinder_keep
cargo run -p box_player_app -- examples/cinder_keep --scene keep
cargo run -p box_player_app -- examples/cinder_keep --capture target/captures/m8/start.png --frames 90
cargo run -p box_editor_app -- examples/cinder_keep     # press Play
```

**The game.**
- The Ember Fields (`assets/scenes/outdoor.scene.json`): the warden speaks (four lines from the string tables, one per confirm); a checkpoint brazier; three crates to shove; a ramp to the upper ledge, where a trigger releases three boulders down the rock slope; the lava gap to jump (falling in kills: back to the checkpoint); a second checkpoint; the courtyard, where 120 cinderlings come from every side (every tenth one a spitter that keeps its distance and throws embers). The sword sweeps all around the hero, kills what it reaches and scatters embers. When the last cinderling dies the gate's rigid body turns dynamic and swings open on its hinge.
- Cinder Keep (`assets/scenes/keep.scene.json`): the hall, lit by torches, braziers and a shadowed spot light over the throne, no sky light inside (an ambient volume). Health and score carry over. Climb the stairs to the throne to win.
- HUD: health bar, score, cinderlings left. Pause menu: Resume, Restart from checkpoint, Language (English / German), Quit. Area titles, toasts, a fade between scenes and on death, the win screen (Play again, Quit).

**Checkpoints** are the save slot `checkpoint` (`.box/saves/checkpoint.save.json`): entering an area and touching a checkpoint brazier save it; dying and "Restart from checkpoint" load it. A save holds every saved entity (crates, boulders, the gate, the hero) and each script's `on_save` data (score and health, the warden's dialogue, the rockfall, the wave's state).

**Scripts** (`assets/scripts`):
- `game.luau`: the persistent game: health, score, HUD, pause menu, dialogue, checkpoints, death, the scene transition with fades, the win screen. Signals it reads and sends are listed at its top.
- `level.luau`: one per scene: starts the game if needed, announces the area (title and checkpoint).
- `player.luau`: character movement, jump, the sword sweep, the animation graph, falling into the lava.
- `camera.luau`: the follow camera (from the Luau collector).
- `wave.luau`: the whole wave as one batch script: Lua arrays, typed transform writes, a neighbour grid for spacing, a pool of 24 ember projectiles with particle trails, kills with bursts of sparks, the gate.
- `warden.luau`, `checkpoint.luau`, `rockfall.luau`, `boulder.luau`, `door.luau` (also the throne: `victory`), `util.luau`.

**Data.** `assets/data/tuning.data.json` (hero, cinderlings, spitters, embers, boulders; hot-reloaded), `assets/animation/hero.animgraph.json` (Idle, Walk, Run, Jump, Attack, Hit), `assets/strings/{en,de}.strings.json`, `assets/ui/cinder.theme.json`, `assets/settings/input.actions.json`.

**Models are generated** outside the editor: `cargo run -p box_proc --example gen_cinder_keep` writes `assets/models/outdoor.glb` and `keep.glb` (named nodes, several materials; their `.meta` sets `"colliders": true`, so every level mesh gets a triangle-mesh collider on import), `props.glb` (cinderling, spitter, ember, crate, boulder, gate, brazier) and `character.glb` (the procedural humanoid with Attack and Hit clips). The files are deterministic; commit them after regenerating. Coordinates the scenes rely on: meadow floor y 0 (flat path |x| < 3.5) from z 14 to -20; ramp x -3..3 from z -12 to -20 up to the upper ledge (y 2, z -20..-28); rock slope x 4..12; lava gap z -28..-31; courtyard y 2, z -31..-62 (arena centre (0, 2, -44)); facade z -57..-58 with the doorway x -1.8..1.8, y 2..5.5. Keep: hall x -8..8, z 0..-30, floor y 0, doorway x -1.8..1.8 in the south wall, steps z -18..-22 up to the dais (y 2).

**Scenes are generated.** Edit `authoring/<scene>.commands.json`, then:

```bash
cp examples/cinder_keep/authoring/empty.scene.json examples/cinder_keep/assets/scenes/outdoor.scene.json
cargo run -p box_cli -- scene apply examples/cinder_keep/assets/scenes/outdoor.scene.json examples/cinder_keep/authoring/outdoor.commands.json
cargo run -p box_cli -- check examples/cinder_keep
```

**Editor completion.** `.box/types/box.d.luau` comes from `cargo run -p box_cli -- script types --project examples/cinder_keep` (`m8_game` checks it is current; `luau_definitions` runs luau-lsp over the scripts).

**Tests.** `cargo test -p box_engine --test m8_game` (levels and colliders, the whole game played with a gamepad, menus, the wave's cost: `cargo test --release -p box_engine --test m8_game wave -- --nocapture`), `cargo test -p box_editor --test m8_cinder_keep` (the same playthrough in the editor's Play mode). Captures go to `target/captures/m8/`.
