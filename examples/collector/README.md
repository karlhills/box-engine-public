# Collector

The M3 sample game. Walk a capsule around an arena and collect every gem. When the arena is empty the game switches to the next one (`arena` → `arena2` → `arena` …) and keeps your score, which is written to the log.

**Controls.** WASD, arrows, d-pad or left stick move relative to the camera. Space or gamepad South jumps. The right stick turns the camera. Click to lock the mouse for mouse look; Escape releases it.

**Run.**

```bash
cargo run -p collector --bin collector-player                  # play in a window
cargo run -p collector --bin collector-player -- --scene arena2
cargo run -p collector --bin collector-player -- --capture target/captures/collector/arena.png --frames 30
cargo run -p collector --features editor --bin collector-editor                # the editor with the game's components; press Play
```

The editor shows the game's components (`PlayerController`, `Collectible`, `FollowCamera`, `ArenaExit`) in the inspector.

**Scenes are generated.** Edit `authoring/<scene>.commands.json` (the `box scene apply` command format; game components allowed) and run `cargo run -p collector --example collector_author`. `cargo test -p collector` checks the scene files are up to date and runs the gameplay headlessly.

**How it works.** The character is the engine's kinematic character controller (`KinematicCharacter` plus `CharacterPlugin`: collide-and-slide with `Physics::move_and_slide`, gravity, jumps, ground snapping), drawn with `Interpolated` so it is smooth above 60 Hz; its shared tunables are the `CharacterSettings` project settings file (`assets/settings/CharacterSettings.json`, hot-reloaded). Runtime state is added by `on_add` hooks. Gems are trigger colliders bobbing with a `Tween`; an `on_event::<TriggerEvent>` handler collects them. When the last gem goes, `Scenes::switch` loads the arena named by `ArenaExit`; runtime entities are despawned (`DespawnUnlessPersistent`).
