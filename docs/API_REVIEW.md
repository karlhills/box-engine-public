# M9 API review: script API and file formats

The M9 review covered the script API (globals, entity methods, lifecycle callbacks, the script rule API, value types) and the main file formats. It checked naming, argument order, nil-versus-error behaviour, units, coordinate conventions, events and signals, and duplicates. It fed [STABILITY.md](STABILITY.md).

**Sources.** Read from the code, not the docs:

- every `ScriptApi::install` and its `definitions()`: crates/box_script/src/api, crates/box_engine/src/script_glue, crates/box_ui/src/script_api.rs;
- the runtime callbacks: crates/box_script/src/runtime.rs;
- the rule API: crates/box_script/src/rules.rs;
- the conversions: crates/box_script/src/convert.rs;
- the file readers and writers.

**Method.** The example projects (`script_demo`, `script_rules`, `collector_luau`, `cinder_keep`) were read as users of the API.

**Deprecation.** Every rename below keeps the old name as a deprecated alias:

- The helper is `box_script::deprecated_alias`.
- Each old name logs one `box_script` warning per process, with the calling `file:line`.
- In the generated `box.d.luau`, the old name carries a `--- Deprecated: use …` doc line.
- The examples and tests now use the new names.
- `crates/box_engine/tests/api_review.rs` checks the new names, the aliases, and the one-warning rule.

## Conventions confirmed

These hold across the whole API. STABILITY.md promises them.

**Names**

- Every global, function, method and table field is snake_case.
- Types are PascalCase: `Quat`, `Color`, `Entity`, `UiNode`, component names.
- Accessors are a noun and `set_` plus the noun, with no `get_` prefix: `position`/`set_position`, `time.scale`/`set_scale`, `strings.language`/`set_language`, `animation.float`/`set_float`.
- Boolean queries start with `is_` (after this review, see the fixes below).
- Callbacks are `on_` plus snake_case, with `_enter`/`_exit` pairs.
  - `self` comes first.
  - Both update callbacks get `dt`.

**Arguments**

- Module functions that act on an entity take it first: `character.*`, `animation.*`, `particles.*`, physics dynamics, `save.include`/`exclude`, `scenes.persist`.
- Optional arguments come last. `ui.on` was the exception; it is fixed below.

**Units and space**

- Angles are radians everywhere (`@Unit("rad")`, `rotate_y`, `Quat.*`, `max_slope`).
- Durations are seconds. Frame counts are named as such (`wait_frames`, `time.frame`).
- +Y is up and −Z is forward (`e:forward()`, `Quat:forward`).
- Lengths are metres.
- Colours are linear in scripts and scene data. `Color.srgb`/`Color.hex` convert.
- Entity methods act on the local transform unless named `world_*`.

**Values and data**

- Values are copies. `e:set` patches type-directed, with a did-you-mean for unknown fields. Paths are dotted (`"Transform.scale"`).
- Uids are strings everywhere.
- Quaternions are `[x, y, z, w]` in both `Quat.new` and JSON.
- Save hooks and `ScriptSignals` share the tagged JSON forms `$entity`, `$vector` and `$quat`.

**Queries and subscriptions**

- Physics queries share one options table (`exclude`, `mask`, `triggers`).
- Ray and sphere casts normalise the direction and default `max_distance` to 1000 m.
- Box sizes are full extents in both the collider and `overlap_box`.
- Subscriptions (`events.on`, `ui.on`, `save.on`) return a numeric id, which `off` removes, and belong to the calling instance, which stops them when it stops. Tasks are cancelled with `task.cancel(id)`.

**File formats**

- Scene JSON hoists `uid`, `name` and `parent` in lowercase. Component keys are the PascalCase type name and fields are snake_case.
- `world.spawn{ name, parent, Component = {...} }` uses the same shape.
- The versioned formats (scene, project, `.meta`, save) refuse newer versions with an "update box-engine" message before parsing their shape. `.box/registry.json` from another format or engine version reads as stale (`registry_stale`).

## Fixed in M9

| Finding | Change |
|---|---|
| Boolean getters without `is_` | `time.paused` → `time.is_paused`, `input.cursor_locked` → `input.is_cursor_locked`, `animation.finished` → `animation.is_finished`, `ui.checked` → `ui.is_checked` (old names deprecated) |
| `load` meant three things (`scenes.load`, `save.load`, `on_load`) | `scenes.load` → `scenes.load_additive` (the Rust name; deprecated alias) |
| `save.keep` had no off switch and did not pair with `save.exclude` | `save.include(e, included?)`; `save.keep` deprecated |
| `save.on` had no `off` | `save.off(id)` |
| `ui.on(kind, id?, fn)` had an optional argument in the middle (`ui.on("back", nil, fn)`) | `ui.on(kind, fn)` also accepted; declared as an overload |
| `input.mouse_down`/`mouse_pressed` had no `mouse_released` | `input.mouse_released` |
| `particles.is_finished` returned false for a non-emitter (a `wait_until` on it hung), and `count` returned 0 | Both fail on an entity without an emitter, as `start`/`stop` do. A despawned one-shot burst counts as finished with 0 particles |
| `Color.hex` took only `RRGGBB`, while theme files write `#RRGGBBAA` | 8 digits accepted (alpha linear) |
| `world.query`/`count` were declared `...string` but accept name lists | Declarations widened |
| `on_load` ran at `on_start`'s first wait (M8 #11) | Runs when `on_start` returns. A stuck `on_start` warns after 5 s |
| No game-facing visibility (M8 #9), no runtime-only spawns (M8 #10) | `Visible`, `e:set_visible`/`is_visible`; `Transient`, `world.spawn{ transient = true }` |

## Deferred (listed for a later release)

Each item says why it waits.

1. **Naming**
   - `input.key_down` versus `input.held` for actions.
   - `lock_cursor`/`release_cursor` instead of `set_cursor_locked`.
   - Cosmetic; aliases can come with the input rebinding work.
2. **`character.move(e, v)` sets a horizontal velocity.** `e:translate` takes an offset and `physics.move_and_slide` a displacement. The name is documented. A rename (`set_move_velocity`) needs a broader look at character control.
3. **`physics.velocity(e)` returns `(linear, angular)`, while `character.velocity(e)` returns one vector.** Planned: `physics.linear_velocity`/`angular_velocity`, decided with joint motors (M8 #14).
4. **Methods say `position`, data says `translation`** (the Transform field and scene JSON). Deliberate: methods are convenience, data mirrors the Rust field. It is documented.
5. **`self.props` in scripts versus `properties` in the `Script` component** (and `e:set("Script", { props = … })`). Accepting both keys is cheap; the rename of `props` needs a decision first.
6. **nil versus error.**
   - Identity and hierarchy queries on a despawned entity are silent (`name`, `uid`, `parent` → nil; `has` → false). Data access raises (`get`, `set`, transform methods).
   - Missing resources: `time` raises, while `input` and physics queries return defaults.
   - The rule to write down: queries of identity are silent; data access and commands raise.
   - Making `input` and physics strict needs the headless test worlds to carry those resources.
7. **Success booleans.** Some mutators return a bool (`set_name`, `remove`, `despawn`, `ui.set_*`); others return nothing and raise (`set`, `add`, `particles.start`). Rule to adopt: a mutator that can be a no-op returns a bool.
8. **`physics.spherecast(origin, radius, direction, …)` puts radius before direction**, unlike `raycast`. Unity's order; documented. Changing it would break callers silently, because both are vectors and numbers.
9. ~~**`e:look_at(target)` compares a world target with the local translation**~~ **Fixed (M9):** the target and `up` are world-space and converted into the parent's space (test `look_at_aims_at_a_world_point_under_a_turned_parent`).
10. **`scenes.current()` returns the resolved file path**, not the name `switch` accepts. To return the assets-relative path, the callers in the examples need checking.
11. **`world.despawn(e)` duplicates `e:despawn()`.** A harmless alias, kept.
12. **Script signal names are not reserved.** `"ui"` and `"save"` are used internally, so `events.send("ui", …)` reaches `ui.on` handlers. To decide: reserve a `box.` prefix in the next minor release.
13. **`ui.rect` returns physical pixels**, while documents use points. Planned: `ui.rect` in points.
14. **`Quat.from_euler(x, y, z)` applies YXZ order.** Documented in STABILITY.md; no change.
15. **Scripts cannot observe scene loads** (no `scenes.on`). Engine events (`SceneEvent`, `TriggerEvent`) are not script events. Next minor release, on the `save.on` pattern.
16. **File format versions.**
    - Unversioned: `.material.json`, `.animgraph.json`, `input.actions.json`, `.strings.json`, `.theme.json`, UI documents, settings files.
    - Rule (STABILITY.md): a missing `version` means 1, and the first breaking change adds it, with a migration in the migration mechanism.
17. **Save files** call the engine format `format` and the game's version `version`, while elsewhere `version` is the engine format. Documented; a rename needs a save migration.
18. **Path roots differ.** `box.project.json` `start_scene` and save-file scene paths are project-relative (`assets/scenes/…`); asset references are assets-relative. Accepting both forms everywhere is planned.
19. **Enum string casing differs.** Reflected data uses Rust variant names (`"Dynamic"`, `"KeepAll"`); API strings and serde configs are snake_case (`"keep_all"`, `"click"`). Rule recorded in STABILITY.md: reflected data uses Rust variant names, API strings use snake_case.
20. **Settings files are named `<TypeName>.json`**, not `name.kind.json` like other data files. Kept; renaming would orphan every project's settings.
