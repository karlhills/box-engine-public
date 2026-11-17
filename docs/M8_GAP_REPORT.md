# M8 gap report: what the second game needed

The second game, [examples/cinder_keep](../examples/cinder_keep) (a third-person action-platformer, Luau only, on imported glTF levels), was built against the script API. Every engine change and workaround it needed is listed here, together with what the lean native build ([examples/minimal_native](../examples/minimal_native)) turned up. Status: **fixed** (in M8), **worked around** (game-side, engine unchanged), **scheduled** (with a milestone), or **left** (by design).

Editor Play mode and the stock player ran the game without changes.

## Second game

| # | Need | What was missing | Action | Files | Status |
|---|---|---|---|---|---|
| 1 | Imported glTF levels collide | No collider shape for mesh geometry; colliders cannot be added to prefab contents (Instanced entities are not saved or edited) | `ColliderShape::Mesh` (triangles of the entity's MeshAsset via AssetLookup, cached per asset version and scale, static; on a non-Fixed body it is left out of the simulation with a warning) and the glTF import setting `colliders` (every non-skinned mesh node gets `Collider { shape: Mesh }`). Character controller, queries, triggers and rapier statics all use it | box_physics/src/{collider,physics,dynamics/sim,lib}.rs; box_assets/src/{meta,importers/gltf}.rs; box_editor/src/icons.rs; tests box_physics/tests/dynamics.rs (stairs, seams, resting bodies, late load, scale, moving body), box_engine/tests/m8_game.rs | fixed. M9: a `-nocol` node-name suffix opts a node out; `ColliderShape::ConvexHull` for dynamic props |
| 2 | (found with 1) | The dynamics mirror never retried a static collider whose shape could not be built when first seen (a mesh still loading) | Retried when its inputs change | box_physics/src/dynamics/sim.rs | fixed |
| 3 | Levels authored outside the editor | The .glb writer wrote one mesh node | `write_scene_glb`: named node hierarchy, a mesh per node, shared materials; generator `gen_cinder_keep` | box_proc/src/glb.rs, box_proc/examples/gen_cinder_keep.rs | fixed |
| 4 | Attack and hit animation | The humanoid had locomotion clips only | `humanoid::combat_character_glb` (Attack, Hit); the plain character file is byte-identical | box_proc/src/humanoid.rs | fixed |
| 5 | Pause menu "Quit" | No script access to AppExit | `app.quit()` (stops Play in the editor) | box_script/src/api/{input,mod}.rs; regenerated example definitions | fixed |
| 6 | Dialogue must freeze the hero | A modal opened by game code during Update captured input only at the next Update: one fixed step of gameplay input leaked (proven: the hero moved 0.1 m into the dialogue; unit test sees [1.0, 0.0] without the fix) | `Ui::sync_capture` also runs in PostUpdate | box_ui/src/{input,systems}.rs; box_ui/tests/ui.rs | fixed |
| 7 | Measure script cost with the wave (R24) | ScriptStats had no timings | `ScriptStats::update_time` / `fixed_time` | box_script/src/runtime.rs | fixed |
| 8 | Many enemies cheaply | (nothing missing) | One batch script with Lua arrays and typed accessors: 0.27 ms for 120 enemies, 0.46 ms for 200 | game | no change needed |
| 9 | Projectile pooling | No game-facing visibility toggle (`box_render::Hidden` is editor-only) | Pooled embers parked at y = −50 with their emitters stopped | wave.luau | fixed in M9 (`Visible` component, `e:set_visible`/`is_visible`; the ember pool hides embers) |
| 10 | Transient runtime entities and saves | `world.spawn` gives a uid, so enemies and embers would be saved; `SaveExclude` entities are left as they are on load | `save.exclude` on spawn, and the owner's `on_destroy` removes them when a load replaces it | wave.luau | fixed in M9 (`Transient`, `world.spawn{ transient = true }`: never saved, despawned on load) |
| 11 | Restoring script state on load | `on_load` runs at the instance's first wait; state set in `on_start` after a wait overwrites what was restored | Set defaults before any wait in `on_start` | game.luau, player.luau | fixed in M9 (`on_load` runs after `on_start` returns; a stuck `on_start` warns after 5 s) |
| 12 | Materials for script-spawned imported meshes | `MeshAsset` materials must be listed in the mesh's material-slot order; wrong order drew the brazier wrongly (seen in a capture) | Listed in slot order | scenes, wave.luau | fixed in M9 (empty `materials` uses the imported ones) |
| 13 | UI fades | No UI animation API | Full-screen node with a theme class; opacity set per frame from a task | game.luau | worked around; scheduled after M9 if more games need UI tweens |
| 14 | Swinging a hinged gate open | Setting a hinged body's angular velocity alone is redistributed by the joint (about a quarter for a door on its edge) | Script sets linear = ω × r as well | wave.luau | worked around; joint motors scheduled after M9 |
| 15 | Triggers from scenes, scene transitions with persistent state, checkpoint save/load, dialogue waits, gamepad menus, kinematic-to-dynamic boulders, data tables | (nothing missing) | Existing API (`scenes.persist`, `save`, `wait_until`, UI modals, `RigidBody` kind change + impulse after one step) | game | no change needed |
| 16 | `e:set` on a missing component | Fails (needs `has`/`add`) | `has`/`add` | wave.luau | left (consistent API) |

## Lean native build

| # | Need | What was missing | Action | Files | Status |
|---|---|---|---|---|---|
| 17 | A native game without dynamics and scripting | Test files assumed every module (script-only tests, a `Collider` in a procedural test, dynamics asserts in script tests) | `required-features` and `cfg` gates on tests; the library already compiled in every feature set | crates/box_engine/{Cargo.toml,tests/*} | fixed |
| 18 | Clear message for a scene using a left-out module's types | The warning suggested running the game's own player | `box_engine::modules` names the feature to enable | crates/box_engine/src/modules.rs | fixed |
| 19 | Opting out from a generated project | `box new --native` depended on box_engine with all modules | The template passes the modules through as the game's own features (`default = ["physics", "audio"]` drops rapier and Luau) | apps/cli/src/native.rs | fixed |
| 20 | `box check` and MCP on a lean project | They validate against the full engine registry plus `.box/registry.json`, so they do not flag `RigidBody` or `Script` in a lean project; only the player warns | — | — | fixed in M9 (`registry.json` `modules`, format 2; `module_disabled` in `check`/MCP) |
| 21 | Settings files of a left-out module | `assets/settings/PhysicsSettings.json` in a build without `dynamics` is ignored silently | — | — | fixed in M9 (runtime warning plus `module_disabled` in `check`) |

## Not verified

- The Windows run (an M8 acceptance criterion: the game completable on macOS and Windows).
- An interactive playthrough by a person.
