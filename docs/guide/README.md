# box-engine guides

Guides for making games with box-engine, written for people who did not build the engine. Start with Getting started; the others are task-oriented: look up what you want to do.

| Guide | What it covers |
|---|---|
| [Getting started](getting-started.md) | Install, create a project, add objects in the editor, write a first Luau script, Play, run the game in the player. |
| [Editor manual](editor.md) | Panes, viewport navigation, selection and gizmos, adding objects, the inspector, undo and saving, Play mode, assets, procedural tools, Project Settings, the command palette, every keyboard shortcut. |
| [Scripting guide](scripting.md) | Luau scripts: instances, properties, the lifecycle, entities and components, input and characters, physics, events, waits, UI and text, saves, data, scenes, animation, rules, modules, hot reload, debugging and performance. |
| [Materials](materials.md) | Standard materials and material shaders, alpha modes (including `Multiply2x`), draw order and depth (sort bias, depth write and test, ghost pre-write), reading the scene depth from a shader, culling-bounds padding. |
| [Procedural guide](procedural.md) | Generators, splines and sweeps, rules in Luau and Rust, overrides, scatter, bake to mesh, `box_arch`. |
| [Native plugins](native-plugins.md) | Game code in Rust: plugins, saved components, systems, events, script APIs, editor actions, `.box/registry.json`, Rebuild and Restart, optional engine modules, headless tests. |
| [Script API reference](../reference/README.md) | Every global, function, method, callback, component, resource and event a script can use. Generated from the engine source (`box script docs`). |

## The samples

Every code sample in these guides is tested:

- Luau samples come from `examples/guide_samples`, a data project. Open it with `box-editor examples/guide_samples` and press Play; `cargo test -p box_engine --test guide_samples` plays its scenes and checks that each Luau block in the guides and the reference is copied from it.
- Rust samples come from `examples/guide_native`, a native project (`cargo run -p guide_native --bin guide-player`; `cargo test -p guide_native`).

Commands shown in the guides were run as written when the guides were last updated. If one does not work for you, that is a bug in the guide: please report it.

## Not covered yet

- **Installing a release** and **exporting a game** as a standalone application are being built; [Getting started](getting-started.md) has placeholders for both.
- Design documents for engine developers (architecture, decisions, the roadmap) are in [`docs/`](../), not here.
