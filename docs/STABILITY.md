# Stability statement

This statement says what box-engine promises to people who build games with it. It confirms decision D10 ([DECISIONS.md](DECISIONS.md)): there are three surfaces, each versioned on its own terms. It holds from the first release after M9 (0.2) and applies to every later 0.x release. At 1.0 the 0.x rules below become semantic versioning (see "From 1.0"). The review behind it is in [API_REVIEW.md](API_REVIEW.md).

## Summary

| Surface | Status in 0.x | What may change in a 0.y release |
|---|---|---|
| Script API (Luau): globals, entity methods, callbacks, value types | **Stable**, except the parts marked experimental | Additions. Renames and removals only through a deprecation (below) |
| Component, resource and event types scripts reach by name (`e:get("RigidBody")`) | **Stable**, with the script API: a type's name and field names are API | Additions (new types, new fields with defaults); renames only with a deprecation and a file migration |
| File formats: scenes, projects, `.meta`, save games | **Stable**, versioned with automatic migration | New versions that older engines refuse; this engine opens every older version |
| Other data files (materials, settings, input actions, data tables, strings, themes, animation graphs, UI documents) | **Stable in content**, unversioned until their first breaking change | Additions; a breaking change adds `"version"` and a migration |
| `box` CLI `--json` output and problem `code`s, `box mcp` tools | **Stable** keys and codes | New keys, codes and tools; MCP versions are negotiated at `initialize` |
| Native Rust API (`box_engine::prelude`, `Plugin`, `Game`) | **Unstable** | Anything; listed in the release notes |
| Everything else (inner crates, editor UI, `.box/` contents, log wording, pixel output) | Not covered | Anything |

## Script API

**Stable in 0.x**

**Globals**

- Value types: `vector`, `Quat`, `Color`.
- Logging, tasks and modules: `log`/`print`, `wait`/`wait_frames`/`wait_until`, `task`, `require`.
- `world`, `time`, `app`, `input`, `physics` (queries, `move_and_slide`, forces and impulses), `character`, `events`.
- `scenes`, `save`, `data`, `particles`, `animation`, `strings`.

**Entity methods**

- Component access by name: `get`, `set`, `add`, `remove`, `has`.
- Identity and hierarchy.
- The transform fast paths.
- `set_visible`/`is_visible`.

**Lifecycle callbacks and their order** (SCRIPTING.md §13)

- `on_start`, `on_update`, `on_fixed_update`, `on_destroy`, `on_trigger_enter`/`exit`, `on_collision_enter`/`exit`, `on_save`, `on_load`, `on_reload`.
- **`on_load` runs once `on_start` has returned**, waits included.

**Script property declarations** (`type`, `default`, `min`, `max`, `step`, `unit`, `doc`).

**Conventions every stable function keeps**

- **Units and space.** Radians, seconds, metres. +Y up, −Z forward. Colours are linear (`Color.srgb`/`Color.hex` convert from sRGB).
- **Names.**
  - Accessors are a noun and `set_` plus the noun, with no `get_` prefix.
  - Boolean queries start with `is_`.
  - Module functions take the entity first, and optional arguments come last.
- **Values are copies.** `set` patches the fields it is given.
- **Errors.**
  - Commands and data access raise with the calling `file:line`: a missing component, a despawned entity, a wrong type.
  - Identity queries (`e:name()`, `e:has()`, `e:is_alive()`) never raise.
- **Subscriptions.** `on` returns an id for `off`. A subscription belongs to the instance that made it and stops with it.
- **Engine types follow their reflected shape.** A component's script form is its scene-JSON form (newtype as its field, unit variant as a string, data variant as `{ Variant = payload }`). Renaming an engine component or field is an API change.
- **Quaternion order.** `Quat.from_euler(x, y, z)` applies Y, then X, then Z.

**Experimental: may change in any 0.y release, without a deprecation period**

- The script rule API: `rule(intents, out)`, `out:part`, `out:warn`, `scope`, `RuleIntent`.
- The `ui` API and UI document, theme and string-table formats. The UI layer is still settling (D11).
- `animation.layers`, `animation.joint` and the animation graph format.
- `ScriptStats`, the watchdog and other runtime tuning.
- Anything a release note calls experimental, and every API added in a release until the next minor release confirms it.

**Semver for the script API in 0.x**

- A **0.y.z patch** fixes bugs only. Behaviour that contradicts the documentation is a bug.
- A **0.y minor release** may add functions, fields, callbacks and types, and may deprecate.
- A minor release never removes or renames a stable item without a deprecation. It never changes an item's meaning silently, either.

## Deprecations

1. **Mark.**
   - **Script API:** the old name stays as an alias of the new one (`box_script::deprecated_alias`). Its declaration in the generated `box.d.luau` gets a `--- Deprecated: use …` line.
   - **Rust:** `#[deprecated(note = "…")]`.
   - **File formats:** the migration rewrites the old form; it does not need an alias.
2. **Warn.** The first use of a deprecated script name logs one warning per name, naming the calling `file:line` and the replacement. It appears as an editor toast and on the Console tab, and as `problems` in CLI JSON.
3. **List.** Deprecations are listed in the release notes and in API_REVIEW.md.
4. **Remove.** In 0.x, no earlier than **two minor releases** after the release that deprecated the name. A name deprecated in 0.2 can go in 0.4. From 1.0, only in the next major release.

The M9 deprecations are listed in API_REVIEW.md: `time.paused`, `input.cursor_locked`, `animation.finished`, `ui.checked`, `scenes.load` and `save.keep`. They can be removed in 0.4.

## File formats

**Versioned with automatic migration**

| Format | Current version |
|---|---|
| `*.scene.json` | `version` 1 |
| `box.project.json` | `version` 1, plus `engine_version` |
| `*.meta` | `version` 1 |
| Save games | engine `format` 1, plus the game's own `version` with `SaveConfig::migrate` |

- **Opening older files.** This engine opens every older version of these formats, migrating on load through the scene and project migration mechanism (scene versioning in box_core and the project upgrade command).
- **Writing.** Files are always written at the current version.
- **What migration keeps.** Data the engine does not understand is kept, not dropped: unknown components and resources are preserved byte for byte.
- **Newer files.** A file from a newer engine is refused with a message to update box-engine, before anything is changed.

**Unversioned data files**

- The files: `.material.json`, settings files (`assets/settings/<Type>.json`), `input.actions.json`, data tables, string tables, themes, animation graphs, UI documents.
- They read as version 1.
- Fields added later always have defaults, so an older file stays valid.
- The first breaking change adds a `"version"` field and a migration.

**Tool-internal files are not formats**

- `.box/registry.json` (format 2): a mismatch reads as stale, and the hint says how to rewrite it.
- `.box/control.json`, `.box/layout.json`, `.box/editor.json`, `.box/types/`.
- They are rewritten by the tools and may change in any release.

**Rules every format keeps**

- Stable-ordered text JSON.
- Uids as strings.
- Asset references as `{id, path}` or a path string with an optional `#Label`.
- Linear colours as `[r, g, b, a]`, except sRGB `#rrggbbaa` strings in themes.
- Reflected values use Rust variant names (`"Dynamic"`). Configuration and API strings use snake_case (`"keep_all"`).

## Native Rust API

Unstable through 0.x:

- **Intended surface.** Games should use `box_engine::prelude`, `Plugin`, `Game`, the reflect derive pattern (AGENTS.md "Conventions") and `ScriptApi`.
- **Most stable in practice.** Those items are the most stable, but a minor release may change any of them. The release notes list every change that breaks a game and how to update.
- **Inner crates.** `box_core`, `box_render` and the others carry no promise even then.
- **Pinned dependencies.** `bevy_reflect`, `glam` and `hecs` query types are re-exported at the engine's pinned versions. Upgrading them is a breaking change of the Rust API, not of scripts or files.

## From 1.0

At 1.0 these rules become semantic versioning for the script API, the stable data formats and the CLI/MCP surface:

- **Major release:** removals (after deprecation) and breaking changes.
- **Minor release:** additions and deprecations.
- **Patch release:** fixes.

The native Rust API gets its own decision before 1.0, either the same semver or "stable prelude, unstable rest". Until then it stays unstable.
