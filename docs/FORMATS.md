# File formats, versions and migration

Every file box-engine saves, where its version lives, what happens to older
and newer files, and how data the engine does not understand is kept. Then
the migration mechanism (`box_core::migrate`), the project upgrade
(`box_engine::upgrade`, `box upgrade`, the editor's prompt) and a guide for
plugin authors who change their saved types.

Decisions: D6 in [DECISIONS.md](DECISIONS.md). Scene file layout:
[SCENE_SYSTEM.md](SCENE_SYSTEM.md) section 7. Project layout:
[ASSETS.md](ASSETS.md) section 2.

## 1. Format and version table (M9)

| Format | File | Version lives in | Current | Older file | Newer file | Unknown data |
|---|---|---|---|---|---|---|
| Project file | `box.project.json` | `"version"` (format), `"engine_version"` | 1, engine 0.1.0 | format chain (`project::format_chain`, empty); an older `engine_version` makes the project need an upgrade | refused: "update box-engine" | unknown fields refused (no silent drop) |
| Scene, prefab | `*.scene.json`, `*.prefab.json` | `"version"` (envelope) + per value `"$version"` | 1 | envelope chain (`scene::format_chain`, empty) then type steps, in memory on every load | refused (`NewerVersion`; a value newer than its type: `ComponentMigration`/`Newer`) | unknown components and resources kept raw (`UnknownComponents`, `UnknownResources`); unknown fields of known struct types kept (`UnknownFields`, `UnknownResourceFields`) and written back |
| Material | `*.material.json` | `"$version"` of `StandardMaterial` / `ShaderMaterial` | 1 (unversioned types) | type steps at import and in the editor | refused (import error naming the type) | unknown fields ignored with a warning at import; the editor writes them back when it saves the material |
| Animation graph | `*.animgraph.json` | `"$version"` of `AnimationGraph` | 1 | type steps at import | refused | ignored with a warning (kept in the file) |
| Settings | `assets/settings/<Type>.json` | `"$version"` of the settings type | 1 | type steps on load and hot reload | refused (keeps the defaults, with a warning) | ignored with a warning; Project Settings writes them back |
| Asset sidecar | `*.meta` | `"version"` | 1 | format chain (`box_assets::meta::format_chain`, empty) | refused for that asset | unknown top-level fields refused; unknown importer settings kept in the file |
| Save game | `.box/saves/<slot>.save.json` | `"format"` (engine), `"version"` (game, `SaveConfig::version`) + per value `"$version"` | format 1 | `SaveConfig::migrate` for the game version, then type steps for every component and resource | refused | unknown components kept as in scenes; unknown resource fields kept |
| Game registry dump | `.box/registry.json` | `"format"` | 1 | rewritten by the game at startup | ignored (stale) | n/a (generated) |
| Editor layout | `.box/layout.json` | `"version"` | 1 | default layout | default layout | n/a (local state) |
| Editor prefs, recovery | `.box/editor.json`, `.box/recovery/*` | recovery files are scenes | — | as scenes | as scenes | as scenes |
| Control socket | `.box/control.json` | `"version"` (protocol) | 1 | — | — | n/a (runtime) |
| Input action map | `assets/settings/input.actions.json` | none | — | additive changes only | — | unknown actions kept and ignored |
| String table, theme, data table | `*.strings.json`, `*.theme.json`, `*.data.json` | none (game-owned content) | — | — | — | the game's business |
| Edit commands | `*.commands.json` | none (input, not a stored format) | — | — | — | refused (validated) |

Not files, but versioned: generator output (`Generator::VERSION`, golden
hashes) and scatter placement (`SCATTER_VERSION`) version generated results,
not stored data; changing them never needs a file migration.

No version was bumped by M9: every file the engine writes today is byte for
byte what it wrote before. `"$version"` appears only on values of types that
have migration steps, and no engine type has one yet. When the first engine
type gets a step, bump `SCENE_VERSION` with a no-op envelope step at the same
time, so engines from before M9 refuse such files with the "newer" message
instead of "unknown field `$version`".

## 2. Inventory before M9 (what existed, what was dropped)

- Versions existed for scenes, the project file, `.meta`, save games (plus
  `SaveConfig::migrate` for game versions), the registry dump, the layout and
  the control protocol. Every one of them refused a newer file. Scenes and
  the project file also refused an *older* one ("not supported"): there was
  no migration path at all.
- Materials, animation graphs, settings and action maps had no version.
- Unknown components and resources in scenes were already kept and written
  back byte for byte.
- Unknown *fields* of a registered component or resource made the whole
  scene fail to load ("unknown field"). Fixed: kept and written back.
- Settings files: unknown fields were dropped with a warning on load and
  then **lost** when the editor's Project Settings wrote the file. Fixed.
- Material files: an unknown field failed the import. Fixed: ignored with a
  warning, and the editor's material inspector writes it back.
- Save games: engine component changes had no path (only the game version
  hook). Fixed: type steps run on save files too.

History check: every version of every `*.scene.json`, `*.material.json`,
`*.animgraph.json` and `*.actions.json` ever committed (101 files across 13
commits, M3 to M8) loads with today's engine, and no field or type name that
any of them used has disappeared since. Every change so far was additive
(new fields take their defaults through `#[reflect(Default)]`). So **no
shipped format needed a migration step yet**; the mechanism is exercised by
test-only steps against the vendored historical projects (section 6).

## 3. The mechanism (`box_core::migrate`)

Files are read into a `serde_json::Value` tree before anything is
deserialized; migrations are pure functions on that tree.

- **Format chains** (`Chain`): one per envelope format (scene, project file,
  `.meta`). Step *i* upgrades version *i + 1* to *i + 2*; the current version
  is `steps + 1`. A file newer than the chain is refused.
- **Type chains**: registered on the `Registry` per type name with
  `registry.add_migration("Type", Step)`, in version order. They apply wherever
  a value of that type is stored: scene components and resources, prefabs,
  save games, material, animation graph and settings files. A value of a type
  above version 1 carries `"$version": n` as its first key; no key means 1.
  Values of types the registry does not know are left exactly as written,
  `"$version"` included.
- **Renames**: `registry.rename_type("Old", "New")`. Scene and save keys are
  read under the new name (in place, keeping their position) and written with
  it; having both names on one entity is an error. A renamed type keeps its
  version and steps (register them under the new name).
- **Steps** (`Step`) are a description plus operations on an object:
  `rename_field`, `move_field` (dotted paths, parents created),
  `default_field` (only when missing), `remove_field`, `map_field` (a value
  transform), `custom` (anything else). A failing step names the type, the
  version and the step.
- **Schema-only types** (a native game's types as `box` and `box mcp` see
  them through `.box/registry.json`): the dump records each type's
  `"version"` and the `"renames"`, so tools detect old data and keep
  `"$version"` on raw values they write, but cannot run the game's steps:
  such data reports `NeedsGameCode` ("upgrade with the game's player:
  `--upgrade`").
- **Unknown data**: unknown components/resources stay raw; unknown top-level
  fields of known struct types are split off on load
  (`split_unknown_fields`), kept per component (`UnknownFields`,
  `UnknownResourceFields`) and appended after the known fields on save. They
  stay with the entity while it lives (removing and re-adding the component
  keeps them, so undo is exact). Nested unknown fields (inside a field's own
  struct) are not split: they still fail the load, as before.

Loading never writes. A player, `box`, Play and the editor all migrate in
memory on every load.

## 4. Project upgrade (`box_engine::upgrade`)

1. **Detect** (`upgrade::plan(root, registry)`, writes nothing): the project
   file (`version`, `engine_version` older than the engine), every scene,
   prefab, material, animation graph, settings file and `.meta` under
   `assets/`, each upgraded on its tree. Scenes are also loaded once with the
   registry, so a result that would not load is reported instead of written.
   Results: files to rewrite with one line per change, files from a newer
   engine or game (`newer`, blocks the upgrade), files that cannot be
   migrated (`errors`, blocks it too).
2. **Summarise**: `UpgradePlan::summary()` (prompt and terminal),
   `to_json()` (`box upgrade --json`).
3. **Back up**: `upgrade::apply` copies every file it is about to change to
   `<project>/.box/backups/<UTC yyyymmddThhmmssZ>/` with the same relative
   paths, plus `upgrade.json` (the plan).
4. **Migrate**: each file is written atomically, the project file last, its
   `engine_version` set to this engine's when it was older. Text is written
   the way the engine writes it (scenes, materials and graphs with compact
   numeric arrays), keeping key order, so diffs show only the change.
5. **Entry points**, all explicit:
   - Editor: opening a project that needs an upgrade (at start, Open
     Project, Open Recent) shows *Upgrade “name” to this version of
     box-engine?* with the summary: **Upgrade** (backup, migrate, open),
     **Open Read-Only** (open with in-memory migration; saving scenes,
     materials, settings and asset-browser file operations are refused with
     a message; the title says "(read-only)"; no settings files are
     created), **Cancel**. A project with newer files is refused with an
     error dialog. Headless API: `Editor::answer_upgrade(UpgradeChoice)`.
   - CLI: `box upgrade [--project <dir>] [--dry-run] [--json]`; exit 1 when
     blocked. `box check` reports `upgrade_needed` (warning), `newer_format`
     and `upgrade_failed` (errors).
   - Player: `<player> --upgrade [--dry-run]` (a native project's game steps
     run only here); a normal run refuses newer files and warns once when
     files are older.

## 5. Changing a saved type (plugin authors)

Adding a field is free: give it a default (`#[reflect(Default)]` on the
type, which every saved type has) and old files load with that default. No
step, no version.

A field added to an enum variant (or left out of one in a hand-written
file, a command or a script: `{ "Sphere": { "radius": 0.5 } }`, or just
`"Sphere"`) is filled from the enum's per-variant defaults when it has them
(`#[reflect(VariantDefaults)]` plus `impl VariantDefaults`, see
`box_core::reflect::VariantDefaults`), else with the field type's default
(0, false, empty), which scene loading and edit commands warn about. Give
an enum `VariantDefaults` whenever a zero would be wrong (sizes, segment
counts); `Registry::variant_default_problems` checks them in tests. The
inspector uses the same defaults when switching a variant.

Anything else needs a step, registered next to the type
(`MigrationStep` is `box_core::migrate::Step`, in `box_engine::prelude`):

```rust
impl Plugin for MyGame {
    fn register(&self, registry: &mut Registry) {
        registry.register::<Spinner>().unwrap();
        // v1 -> v2: `speed` (radians per second) became `turns_per_second`.
        registry.add_migration(
            "Spinner",
            MigrationStep::new("speed (rad/s) is now turns_per_second")
                .rename_field("speed", "turns_per_second")
                .map_field("turns_per_second", |v| {
                    let rad = v.as_f64().ok_or("not a number")?;
                    Ok(serde_json::json!(rad / std::f64::consts::TAU))
                }),
        );
        // v2 -> v3: the axis moved into a sub-struct.
        registry.add_migration(
            "Spinner",
            MigrationStep::new("axis is now motion.axis").move_field("axis", "motion.axis"),
        );
        // The type was called `Rotator` before.
        registry.rename_type("Rotator", "Spinner");
    }
    fn build(&self, app: &mut App) { /* ... */ }
}
```

Rules:

- Steps are append-only: never edit, reorder or remove a step that shipped;
  fix a mistake with a new step.
- Steps are pure JSON → JSON: no world, no files, no globals. Test them on
  literal JSON (`Step::apply`, `migrate::upgrade_value`).
- Keep one fixture per historical version (a scene saved by that version)
  and test that it loads and plays (see `examples/collector/tests/upgrade.rs`).
- Steps change values of the type only. Other references to the old shape
  are not rewritten: `Overrides` field paths (`"Spinner.speed"`), script
  code reading the field, animation tracks. Add a `custom` step on the type
  that holds such a reference (e.g. on `Overrides`) when you rename a field
  that has them.
- Removing a field: remove it in a step (`remove_field`) if its data should
  go; otherwise it is kept as an unknown field and written back.
- After changing steps, a native game's editor and player rewrite
  `.box/registry.json` (its hash includes versions and renames), so `box`
  sees the new versions.
- Users then run `box upgrade` (data projects), the game's player with
  `--upgrade` (native projects), or answer the editor's prompt.

## 6. Tests and fixtures

- `crates/box_core/src/migrate.rs` (unit): every operation, chains, refusal
  of newer versions, rename cycles.
- `crates/box_core/tests/migration.rs`: type steps and renames on load,
  `"$version"` on save, idempotent `upgrade_tree`, newer values refused,
  failing steps named, unknown fields of components and resources through
  load, save, Play clone and copy records, schema-only types and
  `NeedsGameCode`, single-value files.
- `crates/box_engine/tests/upgrade.rs` with vendored fixtures in
  `crates/box_engine/tests/fixtures/upgrade/`: `collector_luau_m7` (bdb076c)
  and `cinder_keep_m8` (065bb18), every JSON file and `.meta` byte for byte.
  They are current (no upgrade needed), and every value they state survives
  loading. A test-only `Script` v2 step plus an `engine_version` of 0.0.9
  upgrades Cinder Keep: backup contents, manifest, untouched files, the
  upgraded worlds equal the in-memory ones, a second plan is empty, and an
  engine without the step refuses the result (planner, loader, player).
  Also newer files blocking the upgrade, save games running type steps and
  settings keeping unknown fields.
- `examples/collector/tests/upgrade.rs` with `fixtures/upgrade/collector_m3`
  (967840e, the first Collector): the M3 arenas play exactly like today's
  (walk through Gem 1, collect both arenas: 75 then 160 points, the arena
  switch and loop), the gameplay data is today's, and a test-only
  `Collectible` v2 upgrade backs up, rewrites and still plays the same.
- `examples/walls/tests/upgrade.rs` with `fixtures/upgrade/walls_m5`
  (5320096): the M5 scenes generate today's walls and fences (same part keys
  and mesh hashes).
- `crates/box_editor/tests/upgrade.rs`: the prompt at start and on Open
  Project; Cancel, Open Read-Only (in-memory migration, writes refused, no
  settings files) and Upgrade (backup, migrated files, editable); newer
  projects refused; a capture of the prompt
  (`target/captures/editor_upgrade_prompt.png`).
- `apps/cli/tests/upgrade.rs`: `box check` warnings, `box upgrade
  --dry-run --json`, apply with backup, idempotence, newer scene blocking
  (exit 1), newer engine refused.

## 7. Not done (later)

- Nested unknown fields (inside a field's struct) are not preserved.
- `box mcp` has no upgrade tool; agents use `box upgrade`.
- `box upgrade` on a native project cannot run the game's steps; it points
  to `cargo run --bin <player> -- --upgrade`.
- Read-only mode does not stop the asset server from creating missing
  `.meta` files (identity, not migration).
- No engine type has a migration step yet; the first one should bump
  `SCENE_VERSION` (section 1).
