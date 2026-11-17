# Vision

> **box-engine — Make worlds, not workflows.**
> A lightweight, open game engine for creating beautiful, interactive 3D worlds. Draw, shape, and build directly in the editor with procedural tools that understand what you're creating — then bring it to life with gameplay.

Guiding philosophy: **make beautiful 3D games without fighting the engine.**

## Ease of use is a primary design goal

**The engine gets out of the developer's way so they can focus on making games.** A developer should spend their time making a game, not figuring out how to use box-engine. This weighs as much as performance and correctness when making decisions, and applies to the editor, the CLI, the Rust API, the file formats and the error messages alike. It is not about fewer features: the aim is a powerful engine that feels effortless, especially for solo developers and small teams.

| Principle | What it means here |
|---|---|
| Discoverability | Common actions are intuitive, searchable and easy to find without documentation (menus, tooltips, a command search). |
| Immediate feedback | Changes to scenes, materials, lighting and objects appear at once whenever practical. |
| Direct manipulation | Prefer working on objects in the viewport over navigating menus or editing numbers. |
| Great defaults | A new project looks good and works well without configuration. |
| Minimal friction | Fewer clicks, setup steps, repetitive tasks and context switches. |
| Progressive complexity | Common workflows stay simple; advanced capabilities are there when needed, not in the way. |
| Clear feedback | Errors are understandable, actionable and never silent. |
| Fast iteration | Experimenting, undoing, testing gameplay and returning to editing are quick and safe. |

Godot, Unity and Unreal are references for what works and what creates friction; their interfaces are not copied blindly. The current review and improvement plan is in [DX_REVIEW.md](DX_REVIEW.md).

## What box-engine is

A Rust-based 3D engine with an integrated graphical editor, aimed at independent developers who want attractive stylized 3D worlds without the weight of a large commercial engine.

Three things define it:

1. **The editor is the product.** Most work happens in a responsive native editor: viewport, hierarchy, inspector, assets, Play. The editor is not a debug overlay bolted onto a runtime.
2. **Procedural creation is the differentiator.** Objects are described by parameters and rules (a wall along a curve, a roof over a footprint, plants scattered over a mask) and regenerate instantly as you edit. Parameters are the source of truth; meshes are derived.
3. **Good-looking by default.** A new scene has pleasant lighting, soft shadows, fog, sky and tone mapping without setup. The target is stylized environments, not photorealism.

## Who it is for

| Audience | What they need | When we serve them |
|---|---|---|
| The engine's developers (a small team with AI assistance) | A controllable engine to build their games | From M1 |
| Indie developers who do not know Rust | Install a binary, edit scenes, write scripts, export a game | M7–M9 |
| Rust developers who want an editor | A native project tier: a Cargo crate that plugs into the engine | From M3 |

## Differentiators

- **Procedural tools that are part of the scene model**, not an add-on: every primitive is a generator with editable parameters, and games can register their own generators.
- **Small and understandable.** About ten crates, one renderer, one entity model. A single developer can hold the architecture in their head.
- **No toolchain required for script projects.** A project is a folder of data and scripts; the editor ships with prebuilt player binaries for export (the Godot model).
- **Open formats only.** glTF, PNG/JPEG, OGG/MP3/WAV, TTF/OTF, JSON scenes, WGSL shaders (KTX2 only as cooked export output). No proprietary formats or services.

## The first demanding use: a construction game

The engine was first proven against a private reference game: an existing procedural building game made with another engine, used as the behavioural and visual target. It is not part of this repository. What it needs, in engine terms (see [RISKS.md](RISKS.md) and [PROCEDURAL.md](PROCEDURAL.md)):

- The world is a small **semantic document**. Everything visible is derived from it.
- Meshes are rebuilt **per entity**, only when a signature of the entity's inputs changes, and expensive passes are throttled.
- Physics is used **only for queries** (raycasts, overlaps).
- Custom shaders, heavy transparency, depth and opaque scene textures for water, one cascaded sun shadow, many unshadowed point lights, and an HDR post stack.
- Rigid-part animation by transforms rather than skinning.

box-engine supports these needs through **general** features. Architectural construction lives in an optional plugin (`box_arch`) above the general procedural layer; game-specific rules, solvers and tools live in the game project.

## Non-goals

- Matching the feature lists of Unity, Godot or Unreal.
- Photorealism, ray tracing, virtualized geometry, large open-world streaming.
- Consoles, mobile, web and VR in the first release.
- A visual scripting editor, a node-graph material editor, or a node-graph procedural editor in the first release.
- Building our own windowing library, physics solver, audio mixer, UI toolkit or ECS from scratch.
- A runtime shaped around one game. A second, substantially different game (M8) is a release gate.
- Multiplayer networking, in the first release.

## Principles used to make decisions

0. **Ease of use is weighed with performance and correctness** (see above). A feature is not done until its common use is easy to discover and its failures are clear.
1. **Prefer proven libraries for infrastructure; own the architecture and the renderer.**
2. **No abstraction without a near-term user.** A feature enters the engine when a milestone needs it.
3. **Hard-to-reverse decisions get a proof of concept first.** Easy-to-reverse decisions are made quickly.
4. **Everything visual is verified by a rendered capture.** The engine has a headless capture path from M1.
5. **Rust does not make the engine fast by itself.** Performance comes from measured budgets, not from the language.
6. **Expensive work never runs in the per-frame path.** Input is accumulated cheaply; rebuilds are throttled and run as jobs.
7. **AI-friendly by design.** Everything an AI assistant needs to work on the engine or on a game is text, deterministic and observable from the command line: readable file formats, headless captures, machine-readable reports, precise errors and a maintained `AGENTS.md`. See [ARCHITECTURE.md](ARCHITECTURE.md) section 12.
