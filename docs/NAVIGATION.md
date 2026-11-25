# Navigation: enemies that find their way

Status: **N1, N2 and B2b built** (2026-11-24); B2 is complete. Part of behaviours phase B2 ([BEHAVIOURS.md](BEHAVIOURS.md), B2a built). Decision D21 accepted.

## Goal

An enemy placed with Add ▸ Gameplay ▸ Enemy chases the player around walls, up ramps and stairs, across a bridge that was just revealed, and gives up when the player is out of reach. The user sets nothing up: no bake button, no navmesh asset, no layers. Scripts and agents can ask the same question ("can something walk from here to there, and how?").

Without navigation, B2's `Chaser` walks straight at the player and sticks on the first wall, which is the first thing a newcomer would notice.

## What exists today

- Every static collider is already in `box_physics::Physics` as a parry `SharedShape` with its pose (`Slot` in `physics.rs`), kept current by change-driven `sync_physics`: primitives, `ColliderShape::Mesh` (imported levels with `"colliders": true`, generated meshes), terrain heightfields (`ColliderHint::Heights`).
- `KinematicCharacter` defines what is walkable for a character: `radius` 0.4 m, `height` 1.8 m, `max_slope` about 53°, `step_height` 0.3 m. The navmesh must agree with it, or paths lead where the character cannot go.
- `Collidable(false)` already switches revealed geometry off and on (D18); `box_engine::route::walk_route` already probes whether a character can walk a waypoint list (MCP `walk_route`).
- No navmesh, pathfinding or steering code exists.

## Library choice

| Option | What it gives | Cost | Verdict |
|---|---|---|---|
| **rerecast 1.0** | Rust port of Recast: voxelise triangles, filter by slope, climb, height and radius, regions, polygon mesh, detail mesh; tile and border settings | Depends only on glam 0.33 (our version), bitflags, slotmap, thiserror, tracing; no `unsafe`; MIT/Apache | **Use for baking** |
| landmass 0.9 | Agents, steering, local avoidance (RVO, `dodgy_2d`) on a given navmesh | Pulls glam 0.29 and 0.30 (two extra copies) and `geo`; its rerecast bridge needs Bevy | No |
| polyanya 0.18 | Optimal any-angle paths on a navmesh | Pulls `geo`, `rstar`; dependency conflicts with landmass | No |
| recastnavigation-sys | Full C++ Recast + Detour | C++ build on every platform, `unsafe` FFI (denied workspace-wide) | No |
| Own voxel grid + A* | Full control | Rewrites what Recast already gets right (ledges, low ceilings, erosion) | No |

rerecast has no Detour (queries, paths). The query side is small and ours: A* over polygon adjacency, then the funnel (string-pulling) algorithm, then heights from the detail mesh. About 500 lines with tests, deterministic, no extra dependency.

## Proposal

### Crate and module

New crate `crates/box_nav` (depends on box_core and rerecast only): `NavMesh` (polygons, adjacency, detail triangles, per-tile), `bake_tile(triangles, &NavConfig) -> NavTile`, `find_path`, `closest_point`, `raycast` (straight walkability), `random_point_near`. No world access, no physics types: it takes a triangle soup. Unit tests and benchmarks run without a GPU.

box_engine feature `navigation` (default on, needs `physics`), added to `modules::MODULES`, the CI feature matrix and generated native projects' module list. Glue in `crates/box_engine/src/nav_glue.rs`.

`box_physics` gains one read API: `Physics::static_triangles(aabb, filter) -> impl Iterator<Item = [Vec3; 3]>` (parry `to_trimesh` for primitives, heightfield and mesh triangles as stored), skipping triggers, `Collidable(false)`, characters and non-`Fixed` rigid bodies.

### Baking: automatic, tiled, off the main thread

- `NavSettings` (scene resource, saved, defaults need no change): `enabled` true, agent `radius`/`height`/`max_slope`/`step_height` (default: `KinematicCharacter`'s defaults; one agent size per scene in B2), `cell_size` 0.15 m, `tile_size` 32 m.
- On Play start and scene load, every tile overlapping static colliders is baked on worker threads. Agents that ask before their tile is ready get `NavStatus::Baking` and move straight toward the target meanwhile (one frame to a few hundred ms in a small level).
- **Change-driven rebuild (stamp and flush).** `sync_physics` already knows which slots it inserted, updated or removed. The glue stamps the old and new AABB of each into a dirty-tile set (cheap, per change). A flush at most every 0.25 s sends dirty tiles to a worker; the finished tile is swapped in at the start of a frame and paths crossing it are marked stale. A bridge revealed by `Show { collide: true }` becomes walkable within about 0.3 s.
- **Edit world.** The navmesh is also built in the editor's edit world (a change-driven `add_edit_world_system`, same throttle) so the overlay and `box check` work without Play; dragging an object rebuilds only the tiles it touched, after the drag settles.
- Exported games bake at load like Play. A cooked navmesh in `game.pak` waits until a measured load time needs it.
- Moving things (`Mover` platforms, doors on kinematic bodies, dynamic props) are not baked; agents collide with them through their own `KinematicCharacter` and repath when stuck. Jumps and off-mesh links (ladders, gaps) are out of scope.

### Agents

**`NavAgent`** (component, saved settings, runtime state): drives the entity's `KinematicCharacter` toward a destination along a path. Added automatically by `Chaser` (`on_add`), usable on its own from scripts and the inspector.

| Field | Default | Meaning |
|---|---|---|
| `speed` | 3.5 m/s | Walking speed along the path. |
| `turn_rate` | 10 rad/s | How fast the body turns to face its motion. |
| `arrive_distance` | 0.5 m | Close enough counts as arrived. |
| `destination` | `None` (`@NotSaved`) | Set by `Chaser`, scripts (`e:move_to(point)`) or `SetField`. |

Runtime `NavAgentState` (never saved): the path corners, the next corner, `NavStatus` (`Idle`, `Baking`, `Moving`, `Arrived`, `Unreachable`, `Stuck`).

Per frame, per agent: steer toward the next corner (one vector op), a separation push from agents within 1 m (a grid lookup, no RVO), write the character's horizontal velocity. Repath only when the destination moved more than 1 m, the path crosses a rebuilt tile, or the agent made no progress for 0.5 s; at most 8 repaths per frame across all agents (a queue), so 50 agents never repath in one frame.

### Queries for scripts, CLI and agents

- Luau `nav` global: `nav.path(from, to)` (array of points or `nil`), `nav.reachable(from, to)`, `nav.closest(point)`, `nav.random_near(point, radius)`; `e:move_to(point)`, `e:stop()`, `e:nav_status()` on entities with a `NavAgent`. Declared in `ScriptApi::definitions()` with descriptions; reference regenerated.
- `box nav <project> [--scene s] --from x,y,z --to x,y,z --json`: the path, its length, or `unreachable` with the closest reachable point. MCP `nav_path` (project and live).
- `box render --nav` and the live `screenshot` tool's `overlays: ["nav"]`: walkable polygons drawn over the frame, so an agent can see where enemies can go.

### Editor

- View ▸ Navigation: the navmesh drawn in the viewport on the post-tonemap `LineLayer::Display*` layers (outlines plus a translucent fill), in edit mode and Play; unreachable islands in a second colour.
- Selecting a `NavAgent` or `Chaser` in Play draws its current path.
- Scene Settings lists `NavSettings` under Add Scene Setting (absent means defaults).

### `box check`

- An enemy (`Chaser`) that cannot reach any `PlayerController` start: `nav_unreachable` warning naming both.
- A `Spawner` whose area has no navmesh under it (when its prefab has a `Chaser`).
- `NavSettings` agent size smaller than a `Chaser`'s `KinematicCharacter` (paths it cannot fit).

## B2 with navigation

B2 in [BEHAVIOURS.md](BEHAVIOURS.md) is done in four steps; N1 and B2a do not depend on each other.

| Step | Scope | Depends on |
|---|---|---|
| **B2a** | `Health` (`on_damage`, `on_zero`, invulnerability, hearts in the built-in HUD), actions `Damage`, `Heal`, `Despawn`; `Spawner` (`Transient` copies of a prefab, interval, `max_alive`, radius); `SetEnabled` widened to every behaviour. Presets Door to Scene, Key and Lock, Lever. | B1 |
| **N1** | `box_nav` (bake, tiles, path, funnel, closest, raycast); `Physics::static_triangles`; `nav_glue` (worker bake, dirty tiles, edit world); `NavSettings`; View ▸ Navigation; `box nav`, MCP `nav_path`, `--nav` overlay; module plumbing. | none |
| **N2** | `NavAgent` with repath queue and separation; Luau `nav` global and entity methods. | N1 |
| **B2b** | `Chaser` on `NavAgent`: `Idle` or patrol points → `Chase` (inside `sight` with line of sight, a ray through `Physics`) → `Search` (walk to the last seen point) → `Return` (home) beyond `give_up`; contact damage through its own `Interactable { Touch, cooldown, actions: [Damage 1 → Interactor] }`; state fed to its body's animation state machine (`speed`, `chasing`). Presets Enemy and Spawner (enemy prefab). Gizmos: `Interactable` radius, `Mover` points (draggable through `path_tool`; `Mover` on a `Spline` path), `Spawner` area, `Chaser` sight and give-up rings. `box check` rule warnings (BEHAVIOURS.md list plus the navigation ones above). | B2a, N2 |

## N1 as built

Code: `crates/box_nav` (`bake.rs`: rerecast pipeline per tile; `mesh.rs`: tiles, polygons, links across tile edges; `query.rs`: `find_poly`, `find_path` with A* and the funnel, `is_walkable_line`, `random_point_near`, `height_at`), `crates/box_engine/src/nav_glue.rs` (`NavSettings`, `Navigation`, `update_navigation`, `update_edit_world`, `settle_navigation`, `draw_navmesh`, query helpers), `Physics::triangles_in`, `Physics::take_changed_regions` and `sync_physics_quiet` (box_physics), View ▸ Navigation (`crates/box_editor/src/editor.rs`), `box nav` (`apps/cli/src/nav_cmd.rs`), MCP `nav_path`, `box render --nav` and the MCP `render` tool's `nav`. Tests: `crates/box_nav/tests/nav.rs`, `crates/box_engine/tests/navigation.rs`, `crates/box_editor/tests/navigation.rs` (captures `target/captures/navigation/editor.png`, `editor_moved.png`), `apps/cli/tests/nav.rs`. Costs: [measurements/b2-navigation.md](measurements/b2-navigation.md).

Where it differs from the proposal:

- Changed regions carry their entity; changes of colliders that are not baked (characters, movers, moving bodies) are ignored, or a moving platform would rebake its tile forever (found by the engine test).
- Flushes are throttled by frames (`FLUSH_FRAMES` = 15), not seconds, so headless runs are deterministic; `Navigation::inline()` and `settle_navigation` bake on the calling thread for tools and tests.
- Bakes run on one worker thread per flush (no pool yet); each tile is caught with `panic_guard::catch`.
- Corners on a straight line (where a path crosses a tile corner) are dropped.
- The edit world builds a navmesh only while View ▸ Navigation is on (it syncs colliders quietly for that).
- Not yet: the live `screenshot` tool's `overlays: ["nav"]`.

## N2 and B2b as built

Code: `crates/box_engine/src/nav_agent.rs` (`NavAgent`, `NavAgentState`, `NavStatus`, `drive_nav_agents`, `finished_walk_to`), `crates/box_engine/src/script_glue/nav.rs` (Luau `nav.ready/path/reachable/closest/random_near`, `e:move_to/stop/nav_status`), `crates/box_engine/src/behaviours/chaser.rs` (`Chaser`, `ChaserState`, `ChaserMode`), `crates/box_engine/src/behaviours/check.rs` (`rule_problems`, `reachability_problems`), the Enemy preset, `crates/box_editor/src/behaviour_gizmos.rs`, `box check` rule warnings (`apps/cli/src/lib.rs` `rule_problems`). Tests: `crates/box_engine/tests/navigation.rs` (`agents_walk_around_walls_and_take_turns_planning`), `crates/box_engine/tests/script_bindings.rs` (`nav_queries_and_move_to`), `crates/box_engine/tests/behaviours_b2.rs` (enemy chase, corner, search, home; patrol; enemies from a spawner; capture `enemy_chase.png`), `crates/box_editor/tests/behaviour_gizmos.rs` (capture `editor_gizmos.png`), `apps/cli/tests/nav.rs` (`box_check_reports_rules_that_cannot_work`).

Where it differs from the proposal:

- `Chaser` has `chase_speed` (patrol and search use `speed`), `line_of_sight`, `patrol` offsets with `wait`, and its own contact damage (`damage`, `attack_cooldown`, `reach`) instead of an `Interactable` on the enemy. A player without `Health` respawns on contact.
- Modes: `Patrol` (waits at home without patrol points), `Chase`, `Search`, `Return`. Line of sight is one ray from eye height; nothing is seen before the first collider sync (the first frame would otherwise see through every wall).
- A `Chaser` reads its walk as finished only when the agent's status belongs to the goal it now asks for (`finished_walk_to`): a stale "arrived" from the previous goal sent it home early (found by the test).
- A new `Navigation` resource scans every collider on its first update, so one made after the colliders were synced (a reset) still builds the whole mesh.
- Agents and chasers skip a spawner's hidden template; copies of an Enemy template chase.
- `nav.ready()` was added: scripts wait for the first bake with `wait_until(nav.ready)`.
- `box check` rule warnings: `group_empty`, `var_never_set` (skipped when the scene has scripts), `switch_scene_empty`, `spawner_empty`, `no_camera`, `nav_unreachable` (the scene's navmesh is built for this only when it has an enemy).
- Gizmos draw for the selection: `Interactable` reach, `Mover` path, `Spawner` area, `Chaser` sight, give-up and patrol. Dragging Mover points in the viewport is not built yet.

## Acceptance

- `cargo test -p box_nav`: bake of fixtures (floor, wall with a gap, ramp at 30° and 60°, stairs at 0.25 m and 0.5 m, low ceiling, narrow corridor under the agent's diameter); paths go around walls, up the ramp and stairs that the character can climb, not the others; unreachable reported; funnel paths are the shortest through the corridor; same input gives the same mesh and path bit for bit.
- `crates/box_engine/tests/navigation.rs`: a terrain hill plus imported glTF level (`cinder_keep`'s keep) bakes; a character following `nav.path` with `walk_route` reaches every waypoint (the navmesh agrees with the controller); hiding a wall with `Collidable(false)` opens a path within 0.3 s; moving one box rebuilds only its tiles; Play and edit world give the same mesh. Capture `target/captures/navigation/overlay.png`.
- `crates/box_engine/tests/behaviours.rs` (B2b): a playtest where an Enemy chases the player around an L-shaped wall and hits them (health drops by one, invulnerability holds), loses them behind a door, searches, returns home; a Spawner keeps at most `max_alive`; key and lock, lever starting a `Mover`, door to scene. Captures `chase.png`, `hearts.png`.
- `crates/box_editor/tests/navigation.rs`: View ▸ Navigation draws in edit mode and Play; dragging a wall updates the overlay after release; capture `target/captures/navigation/editor.png`.
- `apps/cli/tests`: `box nav --json` path and `unreachable`; `box check` reports `nav_unreachable`.
- **Cost** (`--release --test navigation cost`, reference Mac): full bake of a 200 × 200 m scene with terrain and 500 colliders under 400 ms of worker time and 0 ms on the main thread beyond the swap; one dirty tile under 25 ms of worker time; path query p95 under 0.1 ms; 50 agents chasing under 0.3 ms per frame on the main thread, including repaths. Recorded in `docs/measurements/b2-navigation.md`.

## Decision

### D21. Navigation (accepted 2026-11-24)

**Decision.** How characters that are not the player find paths.

**Recommendation.** A navmesh baked automatically from the static colliders `Physics` already holds, with rerecast (Recast port, same glam, no `unsafe`), in tiles rebuilt off the main thread when colliders change (stamp and throttled flush); our own A* and funnel queries in a small `box_nav` crate; a `NavAgent` component that drives `KinematicCharacter`; `Chaser` built on it. One agent size per scene, no off-mesh links, no RVO avoidance in B2.

**Alternatives.** (1) No navigation, straight-line chasers (BEHAVIOURS.md's first draft): enemies stick on walls, the first thing a newcomer notices. (2) landmass for agents: better avoidance, but three glam versions and `geo`; revisit if crowds need RVO. (3) A manual bake button and a navmesh asset (Unity's old model): one more thing to forget and to go stale. (4) C++ Recast/Detour through FFI: complete, but a C++ toolchain and `unsafe`.

**Trade-offs.** A bake in Play start and load (worker threads, agents move straight until ready) instead of a cooked file; one agent size; the query code is ours to maintain.

**Reversibility.** High: `NavSettings` and `NavAgent` are small saved types; the bake and query code sit behind them.

**Confidence.** Medium-high (rerecast's output is checked against C++ Recast by its own test suite; ours needs the controller agreement test above).

## Open questions

1. Several agent sizes (a large enemy that cannot fit through doors): a second mesh per size, or later? (Recommended: later; one size until a game needs it.)
2. Should `Mover` platforms carry agents (a dynamic link while the platform docks), or stay obstacles? (Recommended: obstacles in B2.)
3. Cook the navmesh into exported games now, or only when a measured load time asks for it? (Recommended: when measured.)
