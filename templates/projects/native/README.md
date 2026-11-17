# Native project

A box-engine project with game code in Rust: the empty template's scene plus a Cargo crate holding the game plugin and its own editor and player. Created with `box new <folder> --native` (the same as `--template native`). It needs Rust ([rustup](https://rustup.rs)) and a box-engine source checkout; `--engine-path <checkout>` picks one other than the checkout `box` was built from.

## What is in it

| Path | What it is |
|---|---|
| `box.project.json`, `assets/scenes/main.scene.json` | The project and its start scene, as in the empty template. |
| `Cargo.toml` | The game crate. Its features choose the engine modules the player builds with; `editor` builds the editor binary. |
| `src/lib.rs` | The game plugin: a `Spin` component (shown in the editor's Add Component list) and the system that turns entities that have it. `game()` is what the editor, the player and tests run. |
| `src/bin/editor.rs`, `src/bin/player.rs` | The game's editor (box-editor plus the game's types and systems) and player. |
| `.cargo/config.toml` | The `cargo editor` and `cargo player` shortcuts. |
| `rust-toolchain.toml`, `Cargo.lock` | The engine's toolchain and dependency versions. |

## Run it

```bash
cargo editor        # the editor with this game's types; Game ▸ Rebuild and Restart after code changes
cargo player        # play the start scene
box run .           # the same, through the box tool
```

Add `Spin` to the cube in the editor (Add Component ▸ Spin) and press Play. The native plugins guide (`docs/guide/native-plugins.md` in box-engine) explains plugins, components, systems, hooks and events; `examples/minimal_native` and `examples/collector` are native examples.
