# Minimal native

A native box project built without the engine's `dynamics` (rapier rigid bodies) and `script` (Luau) modules: `box_engine = { …, default-features = false, features = ["physics", "audio"] }`. An orb circles a beacon pad; the pad's trigger collider lights it and plays a chime as the orb passes. This is the M8 check that the optional modules really are optional.

```bash
cargo run -p minimal_native --bin minimal-player                 # window
cargo run -p minimal_native --bin minimal-player -- --capture target/captures/minimal_native/main.png --frames 60
cargo test -p minimal_native                                     # run alone: see below
cargo run -p minimal_native --example minimal_native_author                     # rewrite assets/scenes/main.scene.json
```

- `src/lib.rs`: the game (`Orbit`, `Beacon`) and `main_scene`, which builds the start scene with fixed uids.
- A scene that uses `RigidBody` or `Script` still plays: the data is kept and the player warns once per type, naming the feature to enable.
- Run its tests with `-p minimal_native`. In `cargo test --workspace` cargo unifies features with the other members, so box_engine there has every module and the absence test only says so.
- There is no editor binary: box_editor always links every module. Open the project in the stock editor, or add an editor bin as in `examples/collector` (it then builds with all modules).
