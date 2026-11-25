# AGENTS.md

How to work on this box-engine project, for AI coding agents (and people). `box new` wrote this file; edit it to add your game's own conventions.

## The project

- `box.project.json`: the name and the start scene. Everything else lives under `assets/`: scenes (`*.scene.json`), Luau scripts (`*.luau`), rules (Luau files with `rule(intents, out)`), models (`.glb`), materials, sounds, settings.
- Every asset has a `.meta` file next to it (its id). Commit them; never edit or copy ids by hand: refer to assets by path (`"scripts/door.luau"`, `"models/tree.glb#Mesh/Trunk"`).
- Scenes are JSON, but edit them with commands, not by hand: they validate every value, fill in ids and keep the file canonical.

## Commands

Every command takes `--json` (machine-readable result with a `problems` list). `box <command> --help` documents it.

```bash
box check .                                   # parse every scene, import every asset, resolve every reference
box registry --json                           # every component and resource: fields, ranges, units, docs
box scene show . --json                       # the start scene's entities and values
box scene apply assets/scenes/main.scene.json cmds.json   # edit with commands (--dry-run to check); box scene apply --help
box render . --out shot.png --eye 12,6,14 --look-at 0,1,0  # a view without changing the scene; --overview; --views views.json
box walk . --route route.json                 # can a character walk these waypoints? steps, falls, slopes, where it gets stuck
box nav . --from x,y,z --to x,y,z             # can enemies get from here to there? the navmesh path, or the nearest reachable point
box check .                                   # also warns about rules that cannot work (empty groups, unset vars, enemies that cannot reach the player)
box run . --frames 120 --json                 # play headless; box run . plays in a window
box playtest .                                # replay every playtests/*.playtest.json headless; exit 1 if a check fails
box mcp                                       # the same tools over MCP, plus live control of a running editor or player
```

Command values: colours as `"#rrggbb"` or `{"srgb": [r, g, b]}`; rotations as `{"euler_deg": [x, y, z]}` or, in a whole Transform with its translation, `{"look_at": [x, y, z]}`. +Y is up, -Z is forward, units are metres.

## Verify what you change

1. `box check .` after every edit: zero errors.
2. Look at the result: `box render` from several poses (`--views` renders many in one run), not only the start camera.
3. For anything the player walks: `box walk` along the routes that matter.
4. Play it: `box run . --frames 300 --json`, or live with `box run . --control` (or `box-editor . --control`) and the MCP tools `play`, `send_input` (press or move a named action from the game's ActionMap), `teleport`, `step_frames`, `screenshot` (with `eye`/`look_at`), `world_query`, `read_log`.
5. In a running editor, `apply_commands` with `"save": true` writes a fix to the scene file once.
6. Keep the important runs as playtests and replay them after every change (`box playtest .`, MCP `playtest`): compare `passed`, each check's values and the `fingerprint`. Record one live with MCP `record_start` (it restarts the game from its scene), drive it with `send_input` (named actions, so the camera's turn is part of the input), then `record_stop` with `save: "<name>"`; add checks to `playtests/<name>.playtest.json` (`entity_near`, `entity_gone`, `component_equals`, `event_seen`, `log_contains`). Replays are deterministic (fixed 1/60 s step, fresh scene load); never drive a game with timed OS key presses.
7. Sound: `box run . --frames 600 --audio-report audio.json` measures each source's level at the listener (RMS and peak in dBFS) without a sound device. Live, `audio_levels` shows every playing sound's level, distance and pan, and `audio_solo` and `audio_mute` isolate a source, a group (with the sources below it) or a bus until Play stops. Tune `AudioSettings` (bus volumes) and `AudioSource.volume`: both apply to the running game at once.

## Building worlds

- Ground is a `Terrain` with stamps (`Hill`, `Ridge`, `Basin`, `Flatten`, `FlattenPath` for walkable paths and rises, `Noise`); it collides as drawn. Turn on `show_walkability` to see slopes a character cannot climb.
- For joined lanes, put `TerrainPath { width, terrain? }` and `Spline` on the same entity. Lanes on the same terrain merge at intersections and follow terrain edits across their full width. Use a `FlattenPath` stamp to grade the climb.
- Put things on the ground so they follow it: `FollowGround { offset }` for props and landmarks, `Sweep { follow_ground: true }` for features such as ropes, `ground.height(x, z)` in rules and scripts. Then a terrain edit moves them with it. To place something on the ground once, use the `snap_to_ground` command (`{entity, offset?, align?}`: its origin lands on the terrain, `offset` metres up).
- Shape things with generators and rules, not piles of cuboids: `Primitive`, `Lathe`, `Extrude`, `Sweep` along a `Spline`, and Luau rules (a `ScriptIntent` names the rule file; the rule emits parts). `Scatter` places copies of a mesh or prefab over a surface.
- Reuse: a `.scene.json` can be placed many times with `PrefabInstance`.
- Check before you claim a change works: `check`, a `screenshot` or `render`, and `walk_route` or a playtest for anything the player must reach. In the editor, `editor_context` says what is selected and where the camera looks; `read_asset`/`write_asset` read and write text files under `assets/`.
- Prefer behaviours to scripts for common gameplay: `add_preset` places a `player` (PlayerController + FollowCamera), `pickup`, `hazard`, `moving_platform`, `checkpoint`, `goal`, `enemy` (Chaser: chases the player along the navmesh), `spawner`, `door_to_scene`, `key_and_lock` or `lever` (`box registry` lists presets); tune them with `set` (`Chaser.patrol`, `Interactable.requires`, `Health.max`); `set_resource` `GameRules` (win/lose conditions such as `GroupDone`, time limit, lives, built-in pause and end screens) and `HudSettings` (built-in HUD). Write Luau only for what these cannot express; to change how a Spinner, Bobber or Mover works, `eject_behaviour` it into a script first. After building, run `box check` (it warns about rules that cannot work and enemies that cannot reach the player). Guide: docs/guide/behaviours.md in the engine repository.
- Things the player uses (pickups, switches, braziers, doors) are an `Interactable` on the placed prop's root: a prompt, `requires` conditions on game variables (`GameVars`) and `actions` (Show/Hide with collision, particles, lights, vars, sounds, script calls) aimed at `This`, a `Child` by name inside the prefab, or an `Entity` by uid. No script needed; things revealed later start with `Visible` false and `Collidable` false. Guide: docs/guide/interactions.md in the engine repository.
- Every surface the player walks on needs a `Collider`. Generated ground (a `Primitive`, `Sweep` or rule part) collides as it is drawn with `Collider { shape: "Mesh" }`.
- Scoped rules (`scope = "intent"`) must not use `math.random`: derive variation from the intent (its uid or points).

## Known limits (this engine version)

- `Terrain` is shaped with stamps only (hills, ridges, basins, flats, walkable strips, noise): no sculpting brushes, painted layers or holes yet. `Scatter` does not sample a terrain yet.
- `Scatter` species are meshes or prefabs, not generators.
- The architecture generators (walls, roofs, stairs) are not available to data projects yet.
- No "save selection as prefab" command: write the prefab `.scene.json` with commands like any scene.
- Aerial haze depends only on the distance from the camera: an overview from far away is washed out by a dense haze that looks right at eye level. Check both views.
