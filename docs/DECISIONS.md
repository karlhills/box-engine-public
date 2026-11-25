# Architectural Decision Records

The most consequential decisions for box-engine, with alternatives, trade-offs, reversibility, confidence and any proof of concept required. Sources for factual claims are listed in [TECH_STACK.md](TECH_STACK.md) section 10.

**Status values:** *Proposed* (recommended, awaiting owner confirmation or a spike), *Accepted*, *Open*.
**Reversibility:** how costly it would be to change the decision after code depends on it.

## Summary

| # | Decision | Recommendation | Reversibility | Confidence | Proof of concept |
|---|---|---|---|---|---|
| D1 | Renderer architecture | Custom forward renderer on wgpu, MSAA, HDR, ordered pass list | Medium | High | S3 upload measurement in M1 |
| D2 | Editor UI framework | egui + egui_tiles, own event loop | **Hard** | Medium-high | **S1 before M2** |
| D3 | Entity model and reflection | Entities + components + tree on `hecs`; `bevy_reflect` for reflection | **Hard** | Medium | **S2 before M2** |
| D4 | Scripting | Rust plugins first (M3); Luau as the scripting language (M7) | Medium | High | Built in M7 (S4 done) |
| D5 | Editor/runtime separation | Isolated play world, same process | Medium | High | Round-trip test in M3 |
| D6 | Asset identity and file formats | UUID `.meta` sidecars; versioned JSON scenes | ID scheme **hard**; format medium | High | None |
| D7 | Procedural representation | Parameters as truth; typed generator components; cached meshes | Medium | High | **S6 right after M1** |
| D8 | Hot reload | Assets, shaders and scripts reload; Rust restarts | Easy | High | None |
| D9 | Cross-platform builds | macOS first; Windows built and tested on CI, smoke-run per milestone; DX12 | Easy | Medium-high | CI in M1 |
| D10 | Public API design | Three surfaces; unstable until M9 | Medium | Medium | Second game in M8 |
| D11 | In-game UI | Retained box_ui (own tree, layout, focus, events; epaint painter; overlay pass in box_render); egui types never public | Medium | Medium-high | Decided in M7 |
| D12 | Physics | parry queries (M3); rapier dynamics behind a feature (M7) | Easy | High | None |
| D13 | AI-friendly by design | `AGENTS.md`, headless captures, JSON reports, text formats; `box` CLI (M2) and MCP server (M3) over one command layer | Easy | High | None |
| D14 | M2 data-model placement | Command layer and undo in `box_core::edit`; `MeshRenderer` derived from saved `Primitive` + `StandardMaterial` components; editor overlays after tone mapping | Easy | High | M2 tests |
| D16 | M4 asset runtime | Asset data in core resolved through `AssetLookup`; one `.material.json` kind switch; runtime-only instanced prefab entities; Play copies loaded assets and forwards reloads | Medium | High | M4 tests |
| D15 | MCP server implementation | Synchronous JSON-RPC loop over stdio, no SDK; MCP 2026-07-28 plus 2025-11-25/2025-06-18 via initialize; live control over token-authenticated localhost TCP polled per frame | Easy | High | M3 tests |
| D18 | Data-configured interactions | `Interactable` component on the prefab instance root (conditions, ordered actions, `Uid`/child targets), `GameVars` counters, inherited `Collidable(bool)`, one retained prompt | Medium | Medium-high | Ember bridge test |

## How the reviews changed the plan

Three independent reviews were run on the first draft: one for unnecessary complexity, one for generality beyond the reference game, and one for cross-document consistency. Their recommendations pulled in different directions (cut scope; add missing game features), and were reconciled as follows.

| Finding | Source | Resolution |
|---|---|---|
| Luau in M3 has no user until the second game | Complexity | Scripting moved to M7. The registry it is generated from is still built in M2. |
| The central procedural risk (junctions) was first tested at M6 | Complexity | Spike S6 moved to right after M1. |
| M4 and M6 each hid several milestones | Complexity | M4 trimmed (no prefab overrides, import cache, audio or compression). M6 split into look (M6a) and construction (M6b), with renderer items only the construction slice needs built on demand. |
| M1 built machinery with no M1 user | Complexity | Plugin trait, fixed timestep and events moved to M3; pooled mesh allocator, instancing and GPU profiler deferred until measured need. |
| Background generation pipeline built before anything needs it | Complexity | M5 ships a throttled synchronous flush; jobs are added when the latency benchmark fails or in M6. |
| Three early dependencies cost more than they return | Complexity | WESL deferred to an M4 evaluation; `wgpu-profiler` and `nv-flip` dropped from the initial stack. |
| Windows golden images on CI are unlikely to be dependable | Complexity | Goldens run locally on the Mac until M9; Windows is built and tested on CI and smoke-run by hand. |
| Skinned animation, dynamics, particles, local lights, game UI, save games and gamepad were missing or vague | Generality | Collected into a new milestone, M7, ahead of the second game (M8). Gamepad and cursor lock moved into M3. `MeshData` reserves skinning channels from M1. |
| Mandatory `Uid`/`Name` on every entity makes runtime entities expensive | Generality | Required only for saved or editor-visible entities. |
| A single outdoor sun was baked into shader globals | Generality | The primary light is optional; local lights use a counted buffer from RS2. |
| `box_arch` looked like part of the engine | Generality | Moved to `plugins/`, outside the default plugin set. |
| Reflection: one research pass recommended `bevy_reflect`, another a custom registry | Research | `bevy_reflect` behind a re-export, gated by spike S2. See D3. |
| Picking: GPU ID buffer versus CPU raycast | Research | CPU raycast first, since queries are needed anyway; ID buffer deferred. |
| Crate ownership and milestone inconsistencies | Consistency | Corrected across documents; renderer stages renamed RS1–RS5 to stop clashing with risk numbers. |

Not adopted: moving rapier to M3 (no user before M7), and adding navigation and terrain to the first release (deferred; not precluded).

---

## D1. Renderer architecture

**Decision.** How to render: adopt an existing renderer or write one; forward or deferred; how passes are organized.

**Alternatives.**
- *Adopt `bevy_render`.* Mature features, but it requires `bevy_ecs` and Bevy's extraction model and release cadence.
- *Adopt `rend3`.* Archived in 2025.
- *Custom deferred renderer.* Many lights cheaply, but no MSAA, awkward transparency, and harder per-material stylized shading.
- *Custom forward renderer* (clustered later).
- *Render graph from the start* versus an ordered list of passes.

**Recommendation.** A custom forward renderer on wgpu with 4x MSAA, an HDR target, cascaded shadows and an ordered pass list. Add a depth and normal prepass when the first effect needs it (RS3). Shaders in plain WGSL with a small include step; WESL is evaluated at M4 for user-authored materials.

**Trade-offs.** We write and maintain every feature ourselves, and many small lights are limited until clustering is added. In return we control the look, keep the code small, and avoid another project's architecture.

**Reversibility.** Medium. Forward to clustered forward is incremental. A move to deferred would be a rewrite of the lighting path, but materials and scene data would survive.

**Proof of concept.** Spike S3 in M1 measures mesh upload spikes and draw-call capacity on Metal. Shader hot reload is exercised in M1.

**M1 outcome (2026-11-15).** Implemented as recommended. Tone mapper: AgX by default (analytic), ACES-fitted switchable. Per-mesh GPU buffers kept: no reallocation spikes were measured, and uploads cost about 0.6 ms per MB per frame ([measurements](measurements/m1-upload-stress.md)). HDR target is `Rg11b10Ufloat` on Apple Silicon with an `Rgba16Float` fallback.

**RS3 outcome (2026-11-16).** Prepass single-sample and separate from the MSAA pass (S5); AO applied per fragment to ambient light; bloom energy-conserving without threshold; grading in the tone-mapping pass; post-processing settings live in `Environment` and are on by default (existing scenes included, through field defaults); automatic instancing with persistent storage-buffer slots. Per-pass GPU timestamps were added early for measurement.

**R10 (2026-11-17).** Bloom gains an optional threshold (default 0, off, so existing scenes and goldens are unchanged) that keeps it energy-conserving by swapping only the above-threshold part of each pixel for its blur. The threshold is on exposed luminance, so animating exposure does not change what blooms. A fourth tone mapper, Khronos PBR Neutral, for authored colours that should stay saturated in highlights; AgX stays the default.

**Confidence.** High.


**M4 outcome (shaders).** WESL 0.6.0 was not adopted: it needs rustc 1.97.1 (above our 1.95 MSRV), brings about 12 dependencies including a second WGSL parser next to naga, mangles identifiers (which degrades the naga errors we map back to file:line), and is pre-1.0 and churning. The include step gained `import box::<module>;` instead. Revisit when WESL reaches 1.0 or conditional compilation beyond pipeline constants is needed.

**ShaderMaterial data model (M4).** Saved data in `box_core::shader_material` (scene component and material asset content): `shader: AssetRef`, parameters by name (`Vec<ShaderParam>`), `alpha_mode`, `double_sided`. Parameter types and defaults come from the shader's `@material` header, so files stay valid when the header is reordered and the inspector needs no GPU; the renderer takes uniform offsets from naga's layout. Shader text is the `ShaderSource` asset, resolved through `AssetLookup`.
---

## D2. Editor UI framework

**Decision.** Which UI technology the editor is built on.

**Alternatives.**

| Option | For | Against |
|---|---|---|
| **egui + egui_tiles** | Shares our wgpu device; exact version alignment today; immediate mode suits a reflection-driven inspector; proven by Rerun | Default look is utilitarian; add-on crates lag releases; text and IME depend on winit |
| gpui + gpui-component | Best native look, text and docking | Metal-only on macOS with no shared wgpu device; driven by Zed's priorities; viewport interop unofficial |
| Iced | Elm-style architecture, custom shader widget | Three wgpu majors behind; no docking; slow releases |
| Slint | Polished declarative UI | Licensing; no docking; wgpu embedding behind unstable flags |
| Webview (Tauri) | Web design freedom | Cannot share the GPU device; Graphite abandoned it for its viewport |
| Own UI toolkit | Total control | Years of work; Bevy's experience shows the cost |

**Recommendation.** egui with `egui-winit` and `egui-wgpu` directly, without `eframe`, plus `egui_tiles` for docking and a custom theme and widget set.

**Trade-offs.** Reaching a refined look takes deliberate design work on top of egui. We accept dependence on egui's release rhythm.

**Reversibility.** **Hard.** Every panel is written against the UI toolkit. Mitigation: keep editor logic (commands, undo, selection, play sessions) free of UI types so that only the view layer would be rewritten.

**Proof of concept (S1, before M2).** A themed, docked layout with a live wgpu viewport, a 10,000-row virtualized hierarchy and a property panel, judged for look and for frame time on a HiDPI display at high refresh rate. If the visual bar cannot be met, evaluate gpui.

**Confidence.** Medium-high.

**S1 outcome (2026-11-15): proceed with egui.** A themed shell with egui 0.37, egui-winit, egui-wgpu and egui_tiles 0.18 reached a restrained, professional look after several capture-and-fix passes (captures in `spikes/s1_editor_shell/captures/`). CPU cost with a 10,000-row virtualized hierarchy at 2560×1440, 2× scale: egui run 0.3–0.5 ms mean, under 1.3 ms max; tessellation about 0.1 ms; upload and encode about 0.2 ms. Lessons for M2:
- egui 0.37 and egui_tiles 0.18 need Rust 1.95 or later (same as bevy_reflect; see D3).
- egui ships no fonts unless asked; the editor bundles Inter and Source Code Pro (both OFL). Later, box_ui bundled DM Sans (OFL, `crates/box_ui/assets/fonts`) for the startup splash's wordmark.
- egui's theming is coarse (one fill per interaction state for all widgets), so the editor uses its own small widget kit (about 400 lines: property row, Vec3 field, slider, checkbox, segmented control, section header).
- egui treats a native viewport texture as sRGB-encoded; the engine's viewport output must match.
- Saved layouts are keyed through our own pane enum and validated on load, falling back to the default.

Not yet verified: frame pacing in a real window, because the display was unavailable. The owner should run `cargo run --release -- --seconds 30` in `spikes/s1_editor_shell` on the HiDPI display. Full report: [spikes/s1_editor_shell/FINDINGS.md](../spikes/s1_editor_shell/FINDINGS.md).

---

## D3. Entity model and reflection

**Decision.** How entities and components are represented, and how the engine reflects over them.

**Alternatives for the entity model.**
- *`bevy_ecs` standalone.* Rich (relations, observers, schedules), but high churn, heavy compile cost, and Bevy concepts would shape our API.
- *Custom ECS.* Effort without user-visible benefit.
- *Pure node tree with inheritance (Godot style).* Familiar, but awkward in Rust and poor for data-oriented systems.
- *Entities with typed components in a simple store, plus a hierarchy.* Enough for the requirements.

**Alternatives for reflection.**
- *`bevy_reflect` standalone.* Mature derive, path access, dynamic values, serde integration; few dependencies. Breaks with each Bevy release.
- *Own reflection derive.* Full control and a smaller API; a significant proc-macro project to build and maintain.
- *`facet`.* Promising but explicitly unstable.
- *serde only.* Cannot edit fields in place or carry UI metadata.

**Recommendation.** Entities, typed components and a parent/child tree, stored in `hecs` and used through `box_core::World`, with `hecs` query types re-exported. Reflection through `bevy_reflect`, re-exported as `box_core::reflect`. No parallel scheduler. Persistent identity (`Uid`, `Name`) is required only for saved or editor-visible entities.

The two research passes disagreed on reflection (one favoured `bevy_reflect`, the other a custom registry). The resolution: reflection is the foundation of the inspector, serialization, undo and scripting, and writing a robust derive is the single largest avoidable cost in the engine. Using the mature crate behind a re-export gets the engine moving; the re-export keeps replacement possible.

**Trade-offs.** `bevy_reflect` types appear in the native plugin API, and its upgrades are our breaking changes. `hecs` has no built-in change detection or relations; we add explicit dirty flags and a hierarchy. Systems are `fn(&mut World)` with no declared access, so parallel execution cannot be added later without changing every system; the engine targets thousands of active entities, not hundreds of thousands (risk R24).

**Reversibility.** **Hard** for the model as a whole. Replacing storage would touch query call sites (medium to hard). Reflection is swappable with effort because all access goes through `box_core`.

**Proof of concept (S2, before M2).** Three components with `Reflect`; a registry; get and set by name and path; a JSON round trip; a world clone through serialization; an undo transaction with drag merging; a reflected inspector row in egui. Record compile-time impact. If `bevy_reflect` is a poor fit, fall back to a minimal own derive.

**Confidence.** Medium.

**S2 outcome (2026-11-15): adopt, with conditions.** `bevy_reflect` 0.20.1 on hecs covered the inspector, path get/set, scene JSON (byte-identical round trip, unknown components preserved), Play cloning and undo with about 1,000 lines of generic glue and no per-type code; a proptest of random edits undone and redone round-trips byte for byte. Costs: about 5–6 s more on a clean build and about 55 ms per reflected component. 10k entities save in 29 ms, load in 46 ms, clone for Play in 78 ms (release). Conditions applied at the start of M2:
1. Toolchain to Rust 1.95 or later (bevy_reflect's MSRV).
2. Pin glam to the version bevy_reflect's `glam` feature uses (`^0.33.2`), and upgrade the two together.
3. `box_core::reflect` owns stable names, the hecs bridge, a structural `replace` (bevy's `try_apply` never shrinks lists), value cloning with a dynamic fallback, field metadata and the scene format.
4. Default features off.

Design consequence for M7: script-declared components cannot live in bevy's `TypeId`-keyed registry, so M2 makes the engine's `ComponentType` a Rust-or-script enum that the inspector, scenes and undo consume. Full report: [spikes/s2_reflection/FINDINGS.md](../spikes/s2_reflection/FINDINGS.md).

---

## D4. Scripting

**Decision.** How games add logic, particularly without Rust.

**Alternatives.** Rust static plugins; Rust dylib hot reload; Lua 5.4 or LuaJIT; Luau; Rhai; WebAssembly; C# through .NET hosting; JavaScript. Compared in [SCRIPTING.md](SCRIPTING.md) section 2.

**Recommendation.** Two tiers on one registry-generated API. Rust plugins (static) first, from M3, used by the construction slice. Luau through `mlua` as the scripting language, built in M7 and proven by the second game in M8. WebAssembly and C# deferred; visual scripting not in the first release.

The first draft placed a thin Luau slice in M3. The complexity review pointed out that nothing uses Luau before the second game, so it moved to M7. The cost is that the engine API is shaped by Rust use alone for several milestones; the mitigation is that the registry scripting is generated from exists from M2, and spike S4 tests the binding shape before M7 builds it.

**Trade-offs.** Luau is dynamically typed at run time (with a gradual type checker) and slower than native code, so heavy computation stays in Rust. Rejecting C# rules out reusing existing C# game code.

**Reversibility.** Medium. The registry is language-neutral, so adding or replacing a language affects `box_script` and user scripts, not the engine.

**Proof of concept.** Spike S4 at the start of M7: a script with `on_update` changing a transform by name; a batch query; an exported property in the inspector; hot reload preserving properties; generated `.d.luau` giving completion in `luau-lsp`; a benchmark of 10,000 callbacks per frame.

**Confidence.** Medium-high.

**Outcome (M7).** Spike S4 measured the binding options and `box_script` implements Luau through mlua 0.12.2 as decided. Shape: the world moves into the VM for each script stage; entities are light userdata; component values are copies patched back type-directed (generated from the registry, no per-type bindings), with typed transform fast paths; Vec3 is the native Luau `vector`. 10,000 callbacks cost 0.6 ms per frame and a batch over 5,000 entities 0.53 ms (release). Hand-written bindings are `ScriptApi`s added per crate (`scenes` from box_engine is the first); luau-lsp definitions are generated from the registry. Reflected event types were added to the registry (`register_event`). Status: implemented; API unstable until M9. Confidence: high.

---

## D5. Editor and runtime separation (Play mode)

**Decision.** Where the game runs when the user presses Play.

**Alternatives.** Same world with restore; isolated world in the editor process; separate process with IPC and shared textures. Compared in [EDITOR.md](EDITOR.md) section 7. For reference: Unity runs in-process; Godot and Fyrox use a separate process; Unreal offers both.

**Recommendation.** An isolated play world in the editor process, created from a serialized snapshot. Autosave before Play. The play world is only ever built from serialized data and is accessed through a narrow `PlaySession` interface.

**Trade-offs.** A hard crash in native game code can take the editor down; autosave limits the damage. Script projects are protected because script errors are caught.

**Reversibility.** Medium. The serialized-snapshot rule means moving to a separate process changes the transport and the viewport sharing (IOSurface on macOS, shared handles on DX12), not the scene model.

**Proof of concept.** The play round-trip test in M3 (serialize, play, mutate, stop, compare). A cross-process texture spike only if and when out-of-process play is scheduled.

**Confidence.** High.

**M3 as built.** The play world lives in `EditorState::world` during Play (the edit world, undo stack and dirty state are parked), so every pane shows it unchanged; the session is still created only from scene bytes. Live edits during Play are allowed and discarded on Stop. The autosave is written only for unsaved changes; without a project it goes to the user data folder.

---

## D6. Asset identity and serialization formats

**Decision.** How assets are identified and how scenes and settings are stored.

**Alternatives.**
- Identity: paths (Bevy), UUID in sidecar files (Unity; Godot uses UIDs with paths; Fyrox moved to UUIDs), content hashes.
- Scene format: JSON, RON, TOML, YAML, or binary.
- Versioning: none, serde defaults only, or explicit migrations.

**Recommendation.** UUID `AssetId` in `.meta` sidecars with import settings. Saved entities carry a persistent `Uid`. Scenes, prefabs, materials and the project file are JSON with an integer `version`. The identity scheme and the version field are in place from the start because they are hard to add later. The surrounding machinery is staged: a migration chain over an untyped value tree at the first breaking change, preservation of unknown data when plugins make it necessary, a hashed import cache when load times call for it, and a cooked binary form only for export.

**Trade-offs.** Sidecar files add clutter and must travel with their assets. JSON is verbose and has no comments, which matters little for machine-written files.

**Reversibility.** The ID scheme is **hard** to change once projects exist. The text format is medium: the migration pipeline works on a value tree, so switching JSON for RON is mechanical.

**Proof of concept.** None needed; the design is conventional. Covered by rename and migration tests.

**Confidence.** High.

**M9 outcome.** The migration chain runs over the value tree (format chains plus registry type steps, `"$version"`), unknown data is preserved, and project upgrade is explicit with a backup. No shipped format needed a step through M8. See [FORMATS.md](FORMATS.md).

---

## D7. Procedural geometry representation

**Decision.** What a procedural object is, and what is stored.

**Alternatives.** Store meshes; store parameters; store both. Organize generation as modifier stacks, node graphs, or typed generators. Use triangle lists or half-edge meshes. Use 3D CSG or 2D rule-based cut-outs.

**Recommendation.** Parameters are the source of truth and the only thing saved; meshes are a cache. Generators are typed, reflected components with a pure `generate` function and declared inputs. Regeneration is stamped, throttled and hashed; it runs synchronously within a time budget at first and moves to background jobs with an upload budget when measurement calls for it. Output is an indexed triangle list. Openings use 2D booleans in elevation space. Details in [PROCEDURAL.md](PROCEDURAL.md).

**Trade-offs.** No visual graph and no general mesh editing in the first release. Every generator is hand-written Rust.

**Reversibility.** Medium. A modifier list or node graph can be built on top of generators. Changing what scenes store would require migrations.

**Proof of concept.** Spike S6, right after M1: generate a wall with openings, a corner junction and a roof, then map a small part of a construction game's world document onto generator components and confirm that cross-element rules are comfortable to express. Its findings feed the `Generator` trait before M5.

**Confidence.** High for the model; medium-high for architectural junctions after S6.

**S6 outcome (2026-11-15).** Walls with openings, mitred and butted corners at 60–120°, gable and hip roofs, a roof abutting a taller wall and a wall meeting a round tower all render cleanly with eleven explicit junction rules. Per-entity generators plus a game rule system map the reference game's document without a document-level generator: rule systems write neighbour facts into generator parameters, derived entities are reconciled by key and never saved, and the parameter hash replaces dependency tracking. Generators run in 9–176 µs each; a three-room cluster in 0.46 ms. Trait changes recorded in [PROCEDURAL.md](PROCEDURAL.md) section 2. **Engine gap found:** the renderer ignores `MeshData` material groups; `MeshRenderer` gains a material list indexed by group slot in M4. (The spike's code and full report are not published.)


**M5 as built.** Material slots: slot `i` uses `Procedural.materials[i]` if set, else the entity's own material with primitive precedence, so one-slot generators behave like primitives. Registration is one call, `register_generator` in `Plugin::register`, attaching `ReflectGenerator` type data; the runtime and editor discover generators from the registry, so no `build` step is needed. The flush lives in `sync_scene_meshes` because the editor's edit world has no `App`. Bake writes a minimal glTF so baked meshes go through the normal import.
---

## D8. Hot reload

**Decision.** What reloads live.

**Alternatives.** Reload everything including Rust code (dylib reloading or hot patching); reload data only; no reload.

**Recommendation.** Assets, shaders and Luau scripts reload on save. Rust code requires a rebuild and an editor restart, made tolerable by fast incremental builds and by the editor reopening the last project and scene.

**Trade-offs.** Slower iteration for native-tier gameplay code. This avoids a class of ABI and state-corruption bugs that would be costly for a small team to chase.

**Reversibility.** Easy. Hot patching can be added later as a developer convenience without changing the architecture.

**Confidence.** High.

**Implemented (DX Batch 2).** Game ▸ Rebuild and Restart (⇧⌘B) in a native project's own editor builds the editor binary in the background (`cargo build --message-format=json`), shows compiler errors as toasts, and on success saves the recovery file and session and execs the new binary with `--after-rebuild`, which restores scene, camera, selection and unsaved changes without a prompt. A failed build leaves the editor running. Data that should change without a rebuild lives in reflected settings files (`assets/settings/<Type>.json`) and `assets/settings/input.actions.json`, both hot-reloaded.

---

## D9. Cross-platform builds

**Decision.** How macOS and Windows are both kept working.

**Alternatives.** macOS only until late; cross-compile Windows from macOS (`cargo-xwin`); build and test Windows on CI; DX12 or Vulkan as the Windows backend.

**Recommendation.** Develop on macOS with Metal. From M1, CI builds and runs non-GPU tests on a Windows runner. Windows uses DX12 with statically linked DXC, with Vulkan as a fallback. Each milestone ends with a manual smoke run on the NVIDIA PC. Golden-image tests run locally on the Mac; Windows references and a full parity pass come in M9. Platform code (winit, surface handling) is confined to `box_engine`, `box_render` and `box_editor`. Linux later.

**Trade-offs.** Windows rendering regressions can go unnoticed between smoke runs. This is accepted to keep every milestone from carrying two platforms' worth of verification.

**Reversibility.** Easy.

**Proof of concept.** CI building and testing on both platforms in M1, plus the first manual Windows run.

**Confidence.** Medium-high.

**Update (2026-11-17).** Game developers on a Mac must be able to ship Windows builds without a Windows PC, so `box export --target windows` cross-compiles a native project's player with `cargo-xwin` (MSVC target, ATL for the static DXC, static CRT). The engine's own development, tests and release templates stay on the Windows runner and PC as above; a cross-built exe is verified there before it is trusted (docs/EXPORT.md sections 6 and 8).

---

## D10. Public engine API design

**Decision.** What the engine promises to its users, and when.

**Alternatives.** Stabilize early; expose internal crates directly; a single facade.

**Recommendation.** Three surfaces, each versioned on its own terms:
1. The Rust plugin API, exposed only through `box_engine::prelude`.
2. The script API, generated from the registry.
3. File formats, each with a version and migrations.

Everything is explicitly unstable until M9, and the stability rules themselves are confirmed then. `wgpu` and `mlua` are not exposed in the prelude. `bevy_reflect`, `glam` and `hecs` query types are intentional exceptions, re-exported under our names. Native games may use `egui` directly for UI until D11 is settled.

**Trade-offs.** Wrapping costs some effort and hides some power. Advanced users can still depend on inner crates without stability guarantees.

**Reversibility.** Medium before external users exist; hard afterward.

**Proof of concept.** The second game (M8) is written only against the prelude and the script API. Anything it needs from engine internals is an API gap to fix.

**Confidence.** Medium.

---

## D11. In-game UI

**Decision.** What games use for menus and HUDs.

**Alternatives.** egui for games as well; a retained game UI toolkit of our own; an existing retained toolkit rendered through wgpu; HTML/CSS-style layout.

**Recommendation (provisional).** Two steps:

1. **Through M6:** native games, including the construction slice, use egui directly with their own theme. No engine UI abstraction is built.
2. **In M7:** a script-facing UI API is designed against the second game's needs. Its requirements: retained (scripts describe UI and respond to events; they do not issue draw calls every frame), gamepad and keyboard focus navigation, styled text, world-anchored labels, and text drawn from string tables. Whether it is implemented on egui or on something else is an internal choice that scripts cannot see.

**Trade-offs.** The slice's UI is written against egui and may need rework if the later API replaces it. Building a game UI abstraction before a second game exists would be guesswork.

**Reversibility.** Medium, provided egui types are never exposed to scripts.

**Decided (M7).** A retained UI of our own, `box_ui`, painted with epaint (egui's tessellator and text engine) and drawn by an overlay pass in `box_render`. Native games may keep egui and migrate later.

- **Model.** The `Ui` resource holds a tree. Node kinds: panel, column, row, stack, text, image, button, slider, toggle, list (scrolls to keep focus visible), progress, spacer. Any root can follow a world position (`world`: an entity by uid or name, or a point, plus an offset; hidden behind the camera, beyond `max_distance`, or when the entity is gone): that is the world-anchored label. Nodes are described by `UiNode`, in Rust builders or JSON with the same fields (unknown fields are errors). Game code adds a document once, keeps `NodeId`s or uses `find(id)`, changes nodes with setters (`set_text`, `set_key`, `set_arg`, `set_visible`, `set_enabled`, `set_value`, `set_checked`, `set_class`, `set_style`, `set_image`, `focus`, `anchor_to_entity`, `remove`), and reads `UiEvent`s. Layout and paint run in PostUpdate only when something changed.
- **Layout.** Flex-like, in reference points. Sizes are auto, points, a percentage, or fill (sharing space by `grow`). Containers have padding and gap; `justify` acts on the main axis; `align`/`align_self` act across it, with stretch as the default. Roots sit at `anchor` (also their pivot) plus `offset`. Text wraps at the available width.
- **Styling.** A theme `*.theme.json`, merged over the built-in theme, holds font roles, a palette (`$name`), the focus ring and style classes. A node's style is resolved from its kind's class, then its `class` list, then its inline `style`, then state variants (hover/focus/pressed/disabled/checked). Text accepts inline tags `[color=…]`, `[size=…]`, `[font=…]`, `[i]`, `[u]`.
- **Events.** `UiEvent { node, id, action, kind }`, with kinds click, change(value), toggle(checked), focus, blur, back. They are sent at the start of Update, so game systems see them the same frame.
- **Focus and input.** Actions `ui_navigate` (arrows, D-pad, left stick), `ui_confirm` (Enter, Space, South), `ui_back` (Escape, Backspace, East) and `ui_next`/`ui_prev` (Tab, shoulders) are merged into the `ActionMap` when missing. Navigation works in the topmost visible `modal` root. Moves are spatial (overlap preferred, wrapping); next/prev follow document order; held directions repeat in real time, so they work while the game is paused; focus returns to the node a menu last had. A visible modal root captures every other action (`Actions::capture`; `passthrough_actions` exempts some), and the press that closes a menu never reaches gameplay. Outside modal roots, mouse-bound actions read idle while the pointer is over an interactive node.
- **Text.** A node's `key` is looked up in the `Strings` string tables (active language, then fallback, then the key itself, warned once), with `{name}` placeholders from its `args`; tables hot-reload.
- **Scaling.** `ScaleMode::Reference{1920×1080}` by default; `ScaleMode::Dpi(f)` keeps physical text size.
- **Rendering.** The result is the `Overlay` resource (pixel triangles plus texture updates), drawn after tone mapping in every target (window, capture and the editor's game view) and blended in sRGB space. Buffers are rewritten only on change.
- **Scripts.** Documents are JSON, handles are opaque, setters take plain values, and events are records with string ids and kinds, so binding is mechanical.

**Why not egui directly:** it is immediate mode, so scripts would make per-frame calls, and it has no gamepad navigation or modal capture. **Why not our own text renderer:** shaping, atlas and anti-aliasing would cost weeks for no gain. **Trade-off:** epaint's API changes with egui releases; this is contained in `paint.rs`. **Reversibility:** high. **Status:** decided and built in M7. **Confidence:** medium-high.

---

## D12. Physics

**Decision.** How collision, queries and dynamics are provided.

**Alternatives.** Rapier for everything; parry for queries with rapier optional; Jolt bindings; a custom solution.

**Recommendation.** `box_physics` is optional. It starts in M3 with engine-owned `Collider` components and spatial queries (raycast, shapecast, overlap, simple triggers) on `parry3d`, which is all the construction slice and the editor need. Rigid bodies and the kinematic character controller come from `rapier3d` behind a `dynamics` cargo feature in M7. Native games that need no physics do not compile it; the prebuilt player includes it but does not initialize it unless a scene uses it.

The generality review suggested bringing rapier forward to M3. That was not adopted, because nothing would use it before M7; the M3 sample moves its character with kinematic queries.

**Trade-offs.** Two stages of integration work. Rapier releases break often, so it is pinned.

**Reversibility.** Easy, because components and the query API are engine-owned.

**Confidence.** High.

**M3 as built.** `box_engine` depends on `box_physics` through a default-on `physics` feature (prelude re-export, `Collider` in `engine_registry()`), and `app_with_world` adds `PhysicsPlugin`, so every engine app, Play session and player has queries and trigger events. parry3d 0.31.1's `glamx` re-exports glam `^0.33.7`, which unifies with our 0.33.12: parry math types are our glam types.

**M7 as built.** `box_physics` has a `dynamics` feature on `rapier3d =0.36.1`; `box_engine` re-exports it behind a default-on `dynamics` feature (editor, player, CLI and native games agree on the registry; games opt out with `default-features = false`). rapier 0.36.1 depends on parry3d 0.31.1 and glamx 0.3.1, the query layer's versions (`cargo tree`: one parry3d, one glam 0.33.12), so shapes and math types are shared. The parry query world stays the source of truth for queries, triggers, picking and characters; rapier mirrors solid colliders (bodies, the static level, kinematic characters as kinematic capsules) in a `Dynamics` resource created only while a `RigidBody` exists. `step_dynamics` is the first `FixedUpdate` system (`physics_step`); it writes bodies back to `Transform`/`Velocity` and moves their colliders in the query world at once. Components (`RigidBody`, `Velocity`, `Joint`, `PhysicsSettings`, Collider friction/restitution) and the force API on `Physics` are engine-owned. `KinematicCharacter` stays on `move_and_slide` (works without rapier), extended with steps, edge-aware ground, platforms and pushing; rapier's controller was not used because it needs rapier's query pipeline.

---

## Confirmed decisions (2026-11-14)

| Item | Outcome |
|---|---|
| D2 editor UI | egui accepted, subject to spike S1 |
| D3 reflection | `bevy_reflect` accepted, subject to spike S2 |
| D4 scripting | Luau, built in M7: accepted |
| D6 / Q9 scene format | JSON: accepted |
| D11 / Q3 in-game UI | egui directly for native games; script-facing UI designed in M7: accepted as the plan. The M7 design itself is still to be done. |
| Q5 second game | A short third-person action-platformer: accepted. M7 keeps its full scope. |
| Q6 licence | MIT |
| Q7 track order | Milestones in numbered order (M6 before M7–M8). May be revisited before M6. |
| Construction slice scope | M6 is a vertical slice; a full game is a separate later project: confirmed |

## D13. AI-friendly by design

**Decision.** Whether to treat ease of use by AI coding agents as a design requirement.

**Context.** The owner asked on 2026-11-14 that the engine be AI-friendly from the beginning. The engine is developed with AI assistance, and many of its users will work the same way.

**Recommendation.** Yes. Concretely: a maintained `AGENTS.md`; every example renders headlessly and deterministically with `--capture`; `--report` writes JSON statistics; GPU validation errors fail capture runs; all data formats are stable-ordered text; editor operations are commands that can be run without a mouse. The owner clarified the goal: the engine should be easy to use and build with through an agent, by CLI or MCP. So: a `box` CLI in M2 (create projects, inspect and edit scenes through commands, list registered types, render headlessly, all with JSON output) and an MCP server in M3 (`box mcp`) exposing the same operations plus live control of a running editor or player over an opt-in local socket. Both are thin front ends over the command layer. Details in [ARCHITECTURE.md](ARCHITECTURE.md) section 12.

**Why CLI first.** Every agent can run shell commands, CI can use it, and it needs no protocol dependency. MCP adds typed tools and, more importantly, live access to a running editor, which needs Play mode and the command layer (M2–M3) to exist first.

**Trade-offs.** A small amount of extra work per feature (a flag, a JSON field, keeping `AGENTS.md` current). It overlaps almost entirely with what testing and CI need anyway.

**Reversibility.** Easy.

**Confidence.** High.

---

## D14. M2 data-model placement

**Decision.** Where the command layer lives, how meshes and materials appear in M2 scenes (before assets exist), and how editor overlays are drawn.

**Choices (2026-11-15).**

1. **Command layer and undo in `box_core::edit`**, not in `box_editor`. `EditStack` takes `&mut World` per call. The `box` CLI, the editor and the M3 MCP server share it without the CLI linking egui. JSON `Command`s are the text form of the same edits; a command list is one all-or-nothing transaction.
2. **Scenes save parameters, not meshes.** A `Primitive` component (shape + parameters) and an optional `StandardMaterial` component are saved; `MeshRenderer` (asset handles) is runtime data, maintained by `box_engine::sync_scene_meshes` in PostUpdate. It compares against a cached copy, so an unchanged scene costs one query per frame and no mesh work. In M4, material and mesh asset references replace the component form.
3. **One registry entry point:** `box_engine::engine_registry()` (core + render types). Games add their types and give the world the `Arc<Registry>`.
4. **Editor overlays are drawn after tone mapping** (`LineLayer::Display`, `DisplayOverlay`), so grid, gizmo and selection colours are exact whatever the scene exposure. In-HDR debug lines remain for in-world debugging.
5. **Viewport texture:** `Rgba8Unorm` storage with an `Rgba8UnormSrgb` view for the tonemap pass, so egui-wgpu samples sRGB-encoded bytes without a blit. Window surfaces use the non-sRGB format with an sRGB view format.

**Trade-offs.** `box_core` grows (reflection adds ~11 s to a clean build and ~1.4 s to an incremental one, measured under load). Per-entity mesh and material assets are not shared until RS3 instancing.

**Reversibility.** Easy for 2–5; the command layer could move crates without changing its API.

**Confidence.** High.

---

## D15. MCP server implementation

**Decision.** How `box mcp` speaks MCP and reaches a running editor or player.

**Choices (2026-11-15).**
1. No MCP SDK and no async runtime: `apps/cli/src/mcp.rs` is a synchronous, newline-delimited JSON-RPC 2.0 loop over stdio; tools call the CLI library, so there is one implementation.
2. Protocol: MCP **2026-07-28** (stateless: per-request `_meta` protocol version and client capabilities, `server/discover`, `resultType`), checked against modelcontextprotocol.io/specification on 2026-11-15; plus the handshake revisions **2025-11-25** and **2025-06-18** through `initialize`, because most clients still use them.
3. The tool list is fixed. Live tools return `not_running` instead of appearing and disappearing (2026-07-28 forbids per-connection tool lists, and agents can discover the tools before starting the editor).
4. Live control: `box_engine::control`, TCP on 127.0.0.1 (cross-platform, std only), token in `<project>/.box/control.json` (0600 on Unix), polled once per frame on the main thread, opt-in only.

**Why.** An SDK would pull in tokio and track a fast-moving specification; the protocol surface needed (discover/initialize, tools/list, tools/call) is small and tested end to end.

**Trade-offs.** Specification revisions are followed by hand. No resources, prompts, subscriptions or progress notifications yet. TCP plus token is weaker than a Unix socket with file permissions: anyone who can read `.box/` can drive the editor. On Windows the discovery file has default permissions.

**Reversibility.** Easy; the tool layer is independent of the transport.

**Confidence.** High.

---

## D16. M4 asset runtime

**Choices (2026-11-15).**
1. **Asset data in core.** `AssetId`/`AssetRef`, `ImageData`, `StandardMaterial`, `ShaderMaterial`, `MeshAsset`, `PrefabInstance` and the `AssetLookup` resource live in `box_core`, so importers (`box_assets`) and the renderer never depend on each other; the renderer resolves texture and shader references through `AssetLookup`.
2. **Material file kinds:** one `.material.json` extension; a `"shader"` key means `ShaderMaterial`.
3. **Prefab instancing:** a runtime `Instanced` marker on spawned entities with derived uids, so saving, editing, the hierarchy and picking can all tell; no per-instance overrides.
4. **Play-world assets:** copy the edit world's loaded asset entries (same handle ids and versions), give the play world an `AssetMirror`, and forward edit-world asset events into it. Chosen over a second server (re-import cost; one server per world) and over moving the stores (no isolation; play-created assets would leak into the edit world).
5. **Handles stay plain ids** (not reference counted); nothing unloads yet.

**Trade-offs.** Custom asset types a game registers are not forwarded into Play. A game that mutates an imported asset during Play while the same asset hot-reloads could, in a rare case, leave a stale GPU copy.

**Reversibility.** Medium (scene files hold `AssetRef`s; the runtime choices are internal).

**Confidence.** High.

---

## D17. Portable math for generated content

**Choice (2026-11-16).** Generated geometry, colours and animation clips are bit-identical on every platform. Transcendental functions (`sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `atan2`, `powf`, `exp`, `ln`) in `box_core`, `box_proc` and `box_arch` come from `box_core::pmath`, which wraps the pure-Rust `libm` crate; glam is built with its `libm` feature so its rotation constructors and `slerp` use the same code. `clippy.toml` in those three crates disallows the `f32`/`f64` platform methods.

**Why.** The platform C libraries (Apple libm, the Windows UCRT, glibc) differ in the last bit. The first Windows run (docs/WINDOWS_ISSUES.md, 7a) showed a committed material colour, the committed animation test character and a `Wall` golden hash differing from the macOS recordings. Saved worlds regenerate from intents on load and golden hashes guard against silent changes, so generator output depending on the C library breaks that guarantee across platforms. Comparing with a tolerance would keep the files loadable but the hashes could then only be checked on one platform.

**Trade-offs.** libm is somewhat slower than the platform libraries; generation is change-driven, and glam's trigonometry is not on a per-frame hot path. Generators whose output changed bumped `VERSION`, and the committed outputs were re-recorded once. Game crates and examples may still use the platform methods; their generated output is portable only through `pmath`. Luau scripts (`math.sin`) still use the C library, so script rules with trigonometry are not covered.

**Reversibility.** High (one module and a glam feature).

**Confidence.** High.

---

## D18. Data-configured interactions

**Status.** Accepted (2026-11-21) and built. Design: [INTERACTIONS.md](INTERACTIONS.md); guide: [guide/interactions.md](guide/interactions.md).

**Decision.** How a placed prop becomes something the player uses without game code.

**Recommendation.** An `Interactable` component on the entity the player uses (usually a `PrefabInstance` root): trigger (press an action or touch), radius, prompt (text or string-table key), `requires` conditions on named `GameVars` numbers, and an ordered list of built-in actions (show/hide with collision, particles, light, vars, enable another interactable, sound, script signal or call, any reflected field). Targets are `This`, `Child(name)` (inside the prefab's contents) or `Entity(uid)`. Per-instance configuration is the component on the instance root, which is already saved per instance, so no prefab override system is added (D16 stands). A saved, inherited `Collidable(bool)` (box_physics) switches a subtree's colliders off so revealed geometry, glTF mesh colliders on prefab contents included, becomes walkable; a per-collider `enabled` field could not reach instanced children. One retained `box_ui` prompt. Luau hooks: `on_interact`, the `Interacted` event, `vars` and `interact` globals.

**Alternatives.** (1) Keep writing interaction scripts per game: flexible but every game rewrites pickups, prompts and saving, and agents must write Luau for simple puzzles. (2) A general prefab override system (property patches on instanced contents): needed eventually for other cases, much larger, and not required here because the instance root holds the per-instance data. (3) A visual logic graph: a non-goal for the first release.

**Trade-offs.** A fixed action vocabulary will not cover everything; `SetField`, `Signal` and `CallScript` are the escape hatches. Effects on prefab contents are not saved directly and are re-applied on load from `uses`.

**Reversibility.** Medium (scene files hold the components).

**Confidence.** Medium-high.

---

## D19. Behaviours and presets

**Status.** Proposed (2026-11-24), not confirmed. Full text, alternatives and trade-offs: [BEHAVIOURS.md](BEHAVIOURS.md#d19-behaviours-and-presets-proposed).

**Recommendation.** Engine components for the gameplay every small 3D game rewrites (player, camera, motion, health, spawner, simple chaser), scene-level `GameRules` with a built-in HUD, pause and end screens, all wired through D18's conditions and actions; presets as compiled-in commands files shared by the editor, CLI and MCP; every behaviour can be ejected into an equivalent Luau script.

---

## D20. In-editor assistant panel

**Status.** Proposed (2026-11-24), not confirmed; phase A1 built (2026-11-25). Full text: [AI_PANEL.md](AI_PANEL.md#d20-in-editor-assistant-panel-proposed).

**Recommendation.** An Assistant pane that runs an agent backend against `box mcp` and the editor's control socket, one undo group per turn; first backend an installed external agent (Claude Code, headless streaming), later a built-in Messages API loop. Off by default; nothing in the engine, player or exported games depends on it.

---

## D21. Navigation

**Status.** Accepted (2026-11-24); N1 built. Full text: [NAVIGATION.md](NAVIGATION.md#d21-navigation-accepted-2026-11-24).

**Recommendation.** A navmesh baked automatically from the static colliders in `Physics` with rerecast (a Recast port on our glam, no `unsafe`), in tiles rebuilt on worker threads when colliders change; A* and funnel queries in a small `box_nav` crate; a `NavAgent` component driving `KinematicCharacter`, used by B2's `Chaser`, Luau (`nav`), `box nav` and MCP `nav_path`. One agent size per scene, no off-mesh links or RVO avoidance at first.

---

## Open questions

Q5, Q6, Q7 and Q9 are answered above and kept here for reference.

| # | Question | Needed by |
|---|---|---|
| Q1 | Does the egui spike (S1) meet the visual bar? **Answered: yes, proceed (owner started M2 on 2026-11-15).** | Done |
| Q2 | Does `bevy_reflect` fit (S2), and is its compile cost acceptable? **Answered: adopt with conditions (S2 findings).** | Done |
| Q3 | Script-facing in-game UI approach (D11) | Before M7 |
| Q4 | Does a construction game's document map onto per-entity generators, or need a document-level generator? **Answered by S6: per-entity generators plus a game rule system.** | Done |
| Q5 | What is the second game? It sets the scope of M7. Recommended: a short third-person action-platformer. | Before M7 |
| Q6 | Licence for the engine (for example MIT/Apache-2.0 dual licence) | Before M9, ideally before the repository is public |
| Q7 | Order of the two tracks after M5: construction slice (M6) first, or scripting and the second game (M7–M8) first? | Before M6 |
| Q8 | Pak format and signing workflow details | M9 |
| Q9 | Scene file format: JSON (recommended) or RON | Before M2 |

Confirmed decisions are listed in the [README](../README.md) with the question each maps to.
