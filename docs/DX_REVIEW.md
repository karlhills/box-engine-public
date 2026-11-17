# Developer Experience Review

Ease of use is a primary design goal ([VISION.md](VISION.md)): the engine should get out of the developer's way so they can focus on making games. This document records the first full review (2026-11-15, after M4, at commit 2adce51) and the improvement plan that follows from it. It is kept current: the **Status** column is updated as items land.

## How the review was done

Three independent audits, each evidence-based (code references, headless captures, timed commands, throwaway programs; no tracked files changed):

- **Editor** (E-findings): a newcomer's tasks walked through the editor, with captures and a scripted probe driving the editor with egui events.
- **Workflows and defaults** (W-findings): clone to first pixel, new-project defaults, a tiny native game made from the docs alone, import and hot reload, the edit-play loop, CLI and MCP ergonomics, and deliberately broken inputs to read the error messages.
- **Programming model** (A-findings): what a game author writes for common features, measured against the collector sample; API consistency; errors a game author meets; iteration speed.

Godot, Unity, Unreal and Bevy were used as references for what works and what creates friction.

## What already works well (keep)

- **Undo and document safety.** Every edit is one undo step; drags merge; Escape cancels a drag; recovery files, Save / Don't Save / Cancel on every switch, atomic writes.
- **Play.** An isolated snapshot, caught game panics with phase and location, a clear visual state, Pause/Step, Free Camera.
- **Assets.** Drop targets explain what will happen; failed imports carry a badge and the error; hot reload works in the editor and in a running player within about a second; bad files keep the last good version.
- **Inspector from reflection.** Units, ranges and doc-comment tooltips with no per-type code; mixed values for multi-selection.
- **Headless and machine-readable.** Every command runs in well under a second (`box render` 0.13 s, editor capture 0.10 s, `box run --frames 60` 0.18 s); `--json` everywhere with stable error codes; precise scene and version errors.
- **Programming model.** One `Game` value drives editor, player, Play and tests; systems are plain functions; transforms, meshes and colliders are change-driven, so game code can just mutate; gameplay tests run without a GPU.

## Main problems

1. **Silent failures.** Warnings and errors reach only the Console, which sits behind the Assets tab without a badge (E1). Unknown components (a typo, or opening a native project with the stock editor or player) load with no message anywhere (W2, W18, A3). Game types are silently not registered when a plugin is added with `add_plugin` instead of `Game::with_plugin` (A1). JSON results report success while assets failed (W6); `box check` checks only scenes (W7); `scene apply` accepts dangling references (W8). A failed shader import draws white instead of the error material (W9). Events read in FixedUpdate are dropped at high frame rates, so triggers are missed on 120/144 Hz displays (A2).
2. **First steps are rough.** No quick start in the README (W3); the native template's editor opens the scratch scene instead of the game (W1); no start screen or recent projects (E6, W11); the editor exits before showing a window when the start scene is broken (W5); new objects spawn inside the starter cube, so Add appears to do nothing (E3); the starter scene shows the ground's edge and a grey sky (W10).
3. **Hard to discover.** No command search, Help menu or shortcut list; tool buttons lack tooltips (E5); snapping steps cannot be edited (E10); scene lighting is only reachable by deselecting everything (E23); Point Light cannot be added (E15); Add Component is an unsearchable list of type names (E16); CLI help does not show the command schema or MCP setup (W17, W22); `box-editor`/`box-player` have no `--help` and accept typos (W16).
4. **Indirect manipulation.** Lights, cameras and empties are invisible and unclickable in the viewport (E2); no fly navigation although the docs promise it (E8); F frames only with the pointer over the viewport (E9).
5. **Ceremony in game code.** About 28% of the collector sample is ceremony: an exact-version `bevy_reflect` dependency (A4), `#[type_path]` that games do not need (A23), reflect attribute types missing from the prelude (A24), lazy-init passes without an "on added" hook (A5), event cursors and copies (A6), collect-then-apply loops (A7), no find-by-name or multi-resource access (A17, A19), `Time::delta` that is wrong inside FixedUpdate (A8).
6. **Slow loops for native games.** Rust changes need a manual editor restart that forgets the scene, camera and selection (W12, A15); the CLI and MCP cannot edit game component types (W4); tunables live in constants (A13).

## Improvement plan

Priorities: **P0** fixes silent failures and first-impression breakage, **P1** removes everyday friction, **P2** is larger investment. Effort: S ≤ half a day, M ≤ 2 days, L more. IDs refer to the audit findings summarized above.

### Batch 1: quick wins (P0/P1, mostly S)

| Area | Items | Status |
|---|---|---|
| Editor feedback | Toasts for warnings and errors (click opens the Console) and a count badge on the Console tab (E1, E31); Play tint on panels and an inspector banner "changes are discarded on Stop" (E14); undo/redo toast and selection restored with each step (E20); readable undo labels (E21); unknown components shown read-only in the inspector with a hint (A3, W2); error marks on hierarchy rows for failed prefabs and meshes (W19); Console text selectable and copyable (E25); Console in front when there is no project (E29) | Done (43f4e2e) |
| Editor interaction | New objects placed on the surface at the viewport centre (E3); Delete and Duplicate scoped to the focused pane (E4); tooltips with shortcuts and Help ▸ Keyboard Shortcuts (E5a); F frames from anywhere (E9); editable snap steps and hold-to-snap (E10); File ▸ New Scene and Assets ▸ New ▸ Scene (E7); Point Light in the Add menu (E15); searchable Add Component with friendly names and categories (E16); hierarchy background menu and empty-scene hints (E18); Scene Settings entry for the environment (E23); slider precision from the step (E22, W27); acronym-aware labels and full-label tooltips (E28); `--help` and graceful startup errors (E27, W5 basic) | Done (8d3435f) |
| Workflows and CLI | README quick start with install commands (W3); better starter scene: 200 m ground, a sky that matches its swatches, framing on the objects (W10); stock player and editor warn about native projects and say what to run (W2); `box run` runs a native project's player (W15); `box new --native` on an existing project (W14); `--help` and unknown-flag errors for player and editor (W16); command schema in `box scene apply --help` and MCP setup in `box mcp --help` and its instructions (W17, W22); next steps after `box new` (W24); `asset_not_found` warnings from `scene apply` (W8); failed shader imports draw the error material (W9); a `problems` list in JSON results and `box check` that imports assets and resolves references (W6, W7); ORM/normal textures imported linear automatically (W20); clearer import and path errors (W23, W26, W31); quieter default logs without ANSI when piped (W30); info line for successful hot reloads (W25); control socket announced after the first frame (W21); friendly registry text (W29) | Done (610fb36, 40fee78) |
| Game API | Warn when a plugin's types are missing from the registry (A1); event queues advance only on frames with a fixed step (A2); unknown-component warning with did-you-mean at scene load (A3, W18); prelude re-exports `bevy_reflect`, reflect attributes and `info!`/`warn!`/`error!` so games need no extra dependencies (A4, A24); `#[type_path]` dropped for game types (A23); `Time::delta` is the fixed step inside FixedUpdate (A8); `find_by_name`, `first_with`, `resources_mut`, `component_mut` (A11, A17, A19); `Primitive` and `StandardMaterial` shorthand constructors (A20); named systems in profiling and panic messages (A18); `register_all` and surfaced registration errors (A25); hierarchy fields guarded and a warning for a bare `Parent` (A12); `Game::run` and a documented single entry point (A16); consistent naming, template plugin renamed (A26); native template opens its own project and builds the player without egui (W1, A34); `Physics::ray` with a default filter and documented collider timing (A31); `CursorOptions::lock/release` (A27); `App::run_frames` for tests (A28); range-checked commands and friendlier path errors (A29, A30) | Done (eca1995): collector lost 85 lines of ceremony; the rest is Batch 2 (on_add, on_event, deferral) |

### Batch 2: bigger investments (P1/P2, M–L)

| Item | Findings | Status |
|---|---|---|
| Viewport icons, picking and gizmos for lights, cameras, empties and colliders | E2 | Done (6c28d83) |
| Fly navigation (hold right button: look, WASDQE, wheel for speed) | E8 | Done (6c28d83) |
| Command palette (Cmd+K) over every action, Add, Add Component, scenes, assets, recent projects | E5b, W33 | Done (2df5c69) |
| Start screen with recent projects; reopen the last project; restore scene, camera and selection after a restart; open with an error instead of exiting | E6, W5, W11, A15, W32 | Done (2df5c69); opening `box.project.json` from Finder needs an open-file handler winit 0.30 lacks |
| System ergonomics: `on_add`, `on_event`, deferred structural changes, ordering by name | A5, A6, A7, A18 | Done (9a8f81a) |
| Fixed-step correctness: transform interpolation | A9 | Done (9a8f81a) |
| Native iteration: Rebuild and Restart in the editor; the game's registry visible to `box` and MCP | W12, W4 | Done (c2c148e) |
| Live tuning: project settings resources with hot reload, data-driven action maps, apply Play changes to the edit scene | A13, A21, A14 | Done (c2c148e, d07c23d); no editor UI for action bindings yet (edit the JSON file) |
| Linked materials on primitives and undo for asset edits | E11, E12 | Done (d07c23d) |
| Inspector reset/copy/paste, hide/lock in the hierarchy, view menu and clickable axis gizmo, undo history | E13, E17, E19, E20 | Done (d07c23d, 6c28d83, 2df5c69); no orthographic or unlit view yet |
| Gameplay kit: `move_and_slide` character movement, `Timer`, `Tween` over reflected fields, scene-switch persistence policy | A22, A32, A10 | Done (9a8f81a); collector 592 → 446 lines |
| Inspector progressive disclosure (folding nested structs, pinned Add Component), visible check boxes, capped generated outlines | M6 usability | Done (3f5da0f) |
| M9 error-reporting pass: actionable messages for common failures (docs/TROUBLESHOOTING.md), crash reports kept on the machine with a next-start prompt, Copy/Show File on toasts, `shader_invalid` in `box check`, `build_failed` with compile errors in `box run` | M9 | Done (8425845) |

## Measured baseline (for comparison after the improvements)

| Measurement | Value |
|---|---|
| Clean build of editor, CLI and player (M3 Ultra, 28 cores) | 34.9 s wall, 672 s CPU |
| Native project clean build / edit-and-rebuild | 37.9 s / 0.6–1.0 s |
| Clone to first editor pixel (data project) | 3 steps, about 40 s, no written path to follow |
| Tiny native game from the docs | 8 steps, 2 confusing moments (scratch scene opens; CLI cannot use game types) |
| Collector sample code that is ceremony | about 180 of 646 lines (28%) |
| Native code edit to Play | about 1 s build plus a manual editor relaunch and navigation |
| Scene edit to Play | instant (Play start 0.8 ms) |
