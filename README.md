# box-engine

**Make worlds, not workflows.**

A lightweight, open game engine for creating beautiful, interactive 3D worlds. Draw, shape, and build directly in the editor with procedural tools that understand what you're creating — then bring it to life with gameplay.

> **Status: Milestone 9 (external developer readiness) built and verified headlessly on macOS: `box export` and File ▸ Export make a macOS `.app` or a Windows folder with the game in one pak; project templates and an examples gallery; user guides and a generated script API reference; project upgrades with backups; crash reports and actionable errors; a macOS release package tested on a clean machine. M8 brought a second game in Luau only, M7 scripting, dynamics, animation, game UI and saves, M5–M6 procedural generators and architecture. Windows runs pending.** See [AGENTS.md](AGENTS.md) for how to build, run and verify, and for the `box` CLI that agents can use to create, edit and render scenes.

## What it will be

- A **Rust** engine with a custom **wgpu** renderer, tuned for stylized environments and good default lighting.
- An **integrated native editor**: viewport, hierarchy, inspector, assets, console, Play and Stop.
- **Procedural creation as a core feature**: objects are parameters plus generators and regenerate instantly as you edit.
- **Two ways to write games**: Rust plugins, or Luau scripts with no toolchain required.
- **macOS first** (Apple Silicon, Metal), **Windows second** (DX12). Open formats only.

The engine was first proven on a construction vertical slice built against a private reference game (a procedural building game). A second, different game, [examples/cinder_keep](examples/cinder_keep), validates that the engine is general.

## Architecture at a glance

| Area | Choice |
|---|---|
| Renderer | Custom forward renderer on wgpu; 4x MSAA; HDR; cascaded shadows; WGSL shaders |
| Entity model | Entities with reflected components in a parent/child tree; `hecs` storage used through our own `World` |
| Editor UI | egui with `egui_tiles` docking, sharing the engine's GPU device |
| Play mode | Isolated play world inside the editor process, built from a serialized snapshot |
| Game logic | Rust plugins first (M3); Luau scripting through `mlua` (M7) |
| Scenes and assets | Versioned JSON scenes; UUID asset identity in `.meta` sidecars |
| Procedural | Parameters are the source of truth; typed generator components; meshes are a cache |
| Physics | Optional; `parry3d` queries first, `rapier3d` dynamics when a game needs them |
| Distribution | Prebuilt editor and player binaries; a project is a data folder |

## Planning documents

| Document | Contents |
|---|---|
| [docs/VISION.md](docs/VISION.md) | Product vision, target developers, differentiators, non-goals |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Layers, crates, boundaries, frame loop, performance philosophy, developer experience |
| [docs/TECH_STACK.md](docs/TECH_STACK.md) | Researched dependencies, versions, rejected options, references |
| [docs/RENDERER.md](docs/RENDERER.md) | Rendering architecture and shader strategy |
| [docs/EDITOR.md](docs/EDITOR.md) | Editor UX and technical design |
| [docs/SCENE_SYSTEM.md](docs/SCENE_SYSTEM.md) | Entities, components, hierarchy, serialization, prefabs |
| [docs/SCRIPTING.md](docs/SCRIPTING.md) | Game logic and scripting strategy |
| [docs/ASSETS.md](docs/ASSETS.md) | Asset pipeline, project layout, packaging |
| [docs/PROCEDURAL.md](docs/PROCEDURAL.md) | Procedural modeling and world creation |
| [docs/ROADMAP.md](docs/ROADMAP.md) | Milestones M0–M9 with acceptance criteria |
| [docs/DECISIONS.md](docs/DECISIONS.md) | Decision records, trade-offs, confidence, review outcomes, open questions |
| [docs/RISKS.md](docs/RISKS.md) | Risks, validation experiments, mitigations |
| [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md) | Step-by-step plan for Milestone 1 |
| [AGENTS.md](AGENTS.md) | Commands, conventions and definition of done for AI agents and contributors |

## Roadmap summary

| Milestone | Result |
|---|---|
| M0 | Architecture and decisions (this phase) |
| M1 | Engine bootstrap: window, renderer, lighting, shadows, headless capture |
| — | Spikes: junction generation (S6), editor shell (S1), reflection (S2) |
| M2 | Minimal editor: viewport, hierarchy, inspector, gizmos, undo, save and load |
| M3 | Game runtime: Play and Stop, input, Rust game plugins, physics queries, a small sample game |
| M4 | Assets and scenes: glTF, textured materials, asset browser, hot reload, instanced scenes |
| M5 | Procedural geometry: generators, splines, non-destructive regeneration |
| M6 | Construction vertical slice: procedural architecture, look, placement tools (built against a private reference game) |
| M7 | Scripting and gameplay foundations: Luau, dynamics, skinned animation, particles, game UI, save games |
| M8 | Second game validation |
| M9 | External developer readiness: export, documentation, releases |

M6 and the M7–M8 track are independent after M4 and M5 and can be done in either order.

## Key decisions

Decided on 2026-11-14. Reasoning is in [docs/DECISIONS.md](docs/DECISIONS.md).

| # | Decision | Outcome | Maps to |
|---|---|---|---|
| 1 | Editor UI framework | egui, subject to spike S1 meeting the visual bar | D2, Q1 |
| 2 | Reflection layer | `bevy_reflect` re-exported, subject to spike S2 | D3, Q2 |
| 3 | Scene file format | JSON | D6, Q9 |
| 4 | Scripting language and timing | Luau, built in M7 | D4 |
| 5 | Second game | A short third-person action-platformer | Q5 |
| 6 | Track order after M5 | Milestone order as numbered: construction slice (M6), then scripting and the second game (M7–M8). Can be revisited before M6. | Q7 |
| 7 | In-game UI | egui directly for native games now; script-facing UI designed in M7 | D11, Q3 |
| 8 | Construction slice scope | M6 is a vertical slice; a full game is a later, separate project | — |
| 9 | Licence | MIT | Q6 |

## Licence

MIT. See [LICENSE](LICENSE).

## Quick start

Prerequisites: Rust through [rustup](https://rustup.rs) (the repository's `rust-toolchain.toml` picks the version) and a GPU with Metal (macOS) or DX12/Vulkan.

```bash
git clone https://github.com/karlhills/box-engine-public.git box-engine && cd box-engine
cargo install --locked --path apps/cli      # box
cargo install --locked --path apps/editor   # box-editor
cargo install --locked --path apps/player   # box-player
```

```bash
box new my-game                      # a project: box.project.json, a starter scene, assets/
box-editor my-game                   # edit it
box run my-game                      # play it (box-player my-game does the same)
box render my-game --out shot.png    # a headless screenshot
box check my-game                    # validate scenes and assets
```

- On macOS, `apps/editor/macos/bundle.sh --open` builds and opens `target/release/Box Editor.app`; `scripts/release/package-macos.sh` builds a full release (app, CLI, player template, installer) into `target/release-dist/` ([docs/RELEASING.md](docs/RELEASING.md)).
- A game with Rust code: `box new my-game --native`, then `cd my-game && cargo editor` / `cargo player` (or `box run my-game`).
- AI agents: `box mcp --help` shows how to register the MCP server.
- More: [AGENTS.md](AGENTS.md) (every command and convention) and [docs/](docs/).
