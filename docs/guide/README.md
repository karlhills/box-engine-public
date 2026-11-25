# box-engine guides

Guides for making games with box-engine, written for people who did not build the engine. Start with Getting started; the others are task-oriented: look up what you want to do.

| Guide | What it covers |
|---|---|
| [Your first game](first-game.md) | A complete small game in twenty minutes without code: player, coins, moving platform, hazards, win rules, HUD, tuning and export. |
| [Getting started](getting-started.md) | Install, create a project, add objects in the editor, write a first Luau script, Play, run the game in the player. |
| [How box-engine fits together](concepts.md) | Projects, scenes, entities and components, assets and prefabs, the four ways to add gameplay, Play mode, undo, settings, game state, the command line and agents. |
| [Editor manual](editor.md) | Panes, viewport navigation, selection and gizmos, adding objects, the inspector, undo and saving, Play mode, assets, procedural tools, Project Settings, the command palette, every keyboard shortcut. |
| [Scripting guide](scripting.md) | Luau scripts: instances, properties, the lifecycle, entities and components, input and characters, physics, events, waits, UI and text, saves, data, scenes, animation, rules, modules, hot reload, debugging and performance. |
| [The Assistant](assistant.md) | Describe what to build and an AI agent (Claude Code) does it in the open project: requests, undo, what it can do, privacy and cost, other agents. |
| [Behaviours](behaviours.md) | Gameplay without code: presets (Player, Pickup, Hazard, Moving Platform, Checkpoint, Goal), the player controller and follow camera, motion, win and lose rules, the built-in HUD, pause and end screens. |
| [Interactions](interactions.md) | Props the player uses without code: the `Interactable` component (prompts, conditions on game variables, actions on targets), prefabs placed with per-copy settings, save games, scripts, CLI and MCP. |
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
