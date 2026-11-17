# S2 findings: bevy_reflect on hecs

Question: does standalone `bevy_reflect` on top of `hecs` (through `box_core::World`) cover the inspector, scene serialization, Play-mode cloning and undo without per-type code, at an acceptable compile-time and API cost?

Run on 2026-11-15. Verified by the lead afterwards: `bevy_reflect` 0.20.1 declares `rust-version = 1.95.0` and an optional `glam ^0.33.2` dependency (crates.io API); the spike's 21 tests pass.

## Verdict

**Adopt `bevy_reflect` 0.20.1 behind `box_core::reflect`, with four conditions:**

1. **Toolchain to 1.95 or later.** bevy_reflect 0.20.1 declares `rust-version = 1.95`; the workspace pins 1.94. The spike pins 1.98.1 in its own `rust-toolchain.toml`. (0.19.1 still supports 1.87 but is built on glam 0.32, which is worse.)
2. **Pin glam to the version bevy_reflect's `glam` feature uses (0.33.x today), and upgrade the two together.** The workspace's glam 0.34.3 is a different crate to the compiler, so the feature does nothing for us. Remote wrappers work but are a tax on every component author (see below). Verified in a scratch copy: `box_core` builds unchanged against glam 0.33.2, and with the feature enabled `Vec3`/`Quat` fields need no attributes, serialize as arrays and support `Reflect::set`. `box_render` was not checked.
3. **The facade owns the semantics that bevy_reflect gets wrong or leaves open:** stable names, the `Component` bridge to hecs, a structural `replace` (bevy's `try_apply` leaves extra list elements behind), value cloning with a dynamic fallback, field-metadata types, and the scene format. Game code sees the `Reflect` derive, `#[reflect(...)]` attributes and `&dyn PartialReflect`, and nothing else.
4. **Default features off** (`default-features = false, features = ["std", "debug", "uuid", "reflect_documentation"]`). The defaults include `auto_register_inventory` (registration before `main` through `inventory`).

Everything in the brief works with about 1,000 lines of generic glue and no per-type code. A component needs only `#[derive(Reflect)] #[reflect(Component, Default, Clone)]` plus `#[type_path = "..."]`. A homegrown derive would take weeks to reach this point and would still need the same glue.

## What was built

`cargo test`: 21 tests pass, including a proptest (256 cases, up to 40 random edits each). `cargo clippy --all-targets -- -D warnings` is clean.

| File | Contents |
|---|---|
| `src/components.rs` | `Uid` (UUID newtype), `Name`, `Transform` (glam 0.34 via remote wrappers), `PointLight` (range/unit/step metadata), `Health` (a "game" type), `Npc` (enum with struct and tuple variants, `Vec`, `Option<Uid>`, `Option<String>`) |
| `src/math.rs` | `#[reflect_remote]` wrappers for glam 0.34 `Vec3`/`Quat`, serialized as arrays |
| `src/registry.rs` | `Registry` = `TypeRegistry` + stable-name index; `ReflectComponent` type data (fn pointers into hecs: contains/get/get_mut/insert/remove) |
| `src/meta.rs` | `Range`, `Unit`, `Step` attribute types; `FieldMeta` read from `NamedField` (doc comment, attributes) |
| `src/ops.rs` | Uid index, list components, `get_field`/`set_field` by `"Component.path"`, add-default/insert/remove by name, structural `replace`, hierarchy placement |
| `src/scene.rs` | Stable-ordered pretty JSON save/load, `UnknownComponents`, subtree snapshots, `clone_world` |
| `src/undo.rs` | `Edit` (SetField/Add/Remove/Spawn/Despawn/Reparent), `Transaction`, `Editor` with drag merging, undo/redo |
| `tests/inspector.rs` | Registry names, duplicate rejection, metadata, path get/set (nested, array index, enum variant field, Vec element), dynamic values, enum/Vec/Option fields, error messages |
| `tests/scene.rs` | 300-entity byte-identical round trip, hierarchy and child order, unknown-component preservation, Play clone with Uid mapping, version check |
| `tests/undo.rs` | Drag merge, subtree despawn and undo, **proptest**: N random edits, then undo all gives a file byte-identical to the original and redo all gives one byte-identical to the edited state; guard test that over half of the generated ops produce an undo step |
| `examples/bench.rs` | Serialization timings |
| `baseline/` | Same components with serde derives only, for the compile-time comparison (own workspace) |

Storage is the real `box_core::World` (hecs 0.11), used read-only via a path dependency. `box_core::Transform` is not `Reflect`, so the spike defines its own; in M2 the real one gains the derive.

### Scene format produced

```json
{
  "version": 1,
  "entities": [
    {
      "uid": "00000000-0000-0001-0000-000000000001",
      "name": "Entity 0",
      "parent": null,
      "components": {
        "Npc": {
          "behaviour": { "Patrol": { "speed": 2.5, "waypoints": [[1.0, 0.0, 0.0]] } },
          "tags": ["a"],
          "target": "00000000-0000-0000-0000-000000000002",
          "loot_table": null
        },
        "Transform": { "translation": [-26.0, 4.48, 13.800003], "rotation": [0.0, -0.53, 0.0, 0.85], "scale": [1.6, 1.6, 1.6] }
      }
    }
  ]
}
```

(Compacted here; the real output is serde_json pretty with one array element per line. A custom formatter that keeps short numeric arrays on one line would shrink diffs; cosmetic.)

- Entities are in hierarchy pre-order: roots sorted by Uid, children in `Children` order. `uid`/`name`/`parent` are hoisted out of the components. Components are sorted by name; fields follow declaration order.
- Enums are externally tagged, unit variants are bare strings, `Option` is `null` or the value, and single-field tuple structs (`Uid`) are written as their inner value.
- Loading goes through an untyped `serde_json::Value` tree (where migrations will run). `TypedReflectDeserializer` takes `&serde_json::Value` directly.
- Unknown components are kept verbatim (serde_json `preserve_order`), merged back in name order on save, and come out byte-identical.

## What worked well

- **Derive and registry.** `register::<T>()` pulls in field types. `#[reflect(Component)]` with our own `ReflectComponent: CreateTypeData<T>` bridges to hecs with one generic impl.
- **Paths.** `translation.x`, `color[1]`, `behaviour.waypoints[0][2]` (inside an enum variant) and `rotation.w` (through a remote wrapper) all work. Runtime errors are good, e.g. "Error accessing element with `.intensty` access(offset 8): The struct accessed doesn't have an `intensty` field".
- **Metadata.** `#[reflect(@Range { min: 0.0, max: 100_000.0 }, @Unit("lm"), @Step(10.0))]` is read back with `field.get_attribute::<Range>()`; doc comments come back through `field.docs()` (feature `reflect_documentation`).
- **Serde integration.** The typed serializer and deserializer slot into hand-written `Serialize` impls. Custom formats come from `#[reflect(Serialize, Deserialize)]` on a type with serde impls.
- **Dynamic values.** A runtime `DynamicStruct` (what scripts and the deserializer produce) can be applied to a component or turned into one.
- **Undo values as reflected boxes.** No component needed undo code. The proptest covers field edits, enum variant switches, Vec grow and shrink, Option flips, fixed arrays, add/remove, spawn/despawn of subtrees with sibling-index restore, and reparenting.

## What was awkward (and what the facade must absorb)

| Issue | Detail | Mitigation |
|---|---|---|
| **MSRV** | 0.20.1 needs Rust 1.95 (published 2026-10-28); workspace pins 1.94 | Bump the toolchain on adoption |
| **glam coupling** | The 0.20.1 `glam` feature uses `glam ^0.33.2`; we use 0.34.3. The orphan rule forbids implementing `Reflect` for `glam::Vec3` in our crates | Pin glam to bevy_reflect's version (verified). The alternative is remote wrappers (next row) |
| Remote wrappers (if glam 0.34 is kept) | Every glam field needs `#[reflect(remote = Vec3Def)]`. `#[type_path]` cannot be used on `#[reflect_remote]`, so the wrapper's module path leaks. Downcasting gives the remote type. **`Reflect::set` always fails on remote wrappers.** Whole-field sets must pass `Vec3Def(v)`. Default/Clone/PartialEq/serde must be written by hand | Avoid by pinning glam |
| **`try_apply` is not replace** | `list_try_apply` never shrinks: applying `["c"]` over `["a","b"]` gives `["c","b"]`, so naive undo corrupts data. The proptest catches it when truncation is disabled. Maps do drop extra keys (inconsistent) | `box_core::reflect::replace`: a generic structural walk that truncates lists, then defers leaves and variant switches to `try_apply` (~60 lines) |
| **`reflect_clone` gaps** | Not implemented for `[T; N]`; arrays get no `ReflectFromReflect` either | `clone_value`: `reflect_clone`, falling back to `to_dynamic()`. Undo values are `Box<dyn PartialReflect>` and `replace` handles both kinds |
| Attribute syntax | No `range = .., unit = ..` form; custom attributes are `@expr` values looked up by type | Ship `Range`/`Unit`/`Step`/`Tooltip` in `box_core::reflect`; update the SCENE_SYSTEM.md section 6 example |
| `#[type_path = "box"]` | "expected identifier, found keyword `box`", reported at the derive | Use `box_engine` |
| Short-name ambiguity | `get_with_short_type_path` returns None on ambiguity | Own name index; reject duplicate short names at registration (tested) |
| `PartialReflect` vs `Reflect`, dynamic vs concrete | Two traits, `try_downcast_ref` vs `downcast_ref`, `into_partial_reflect`... easy to get wrong | Facade takes `&dyn PartialReflect` and returns `Box<dyn PartialReflect>` |
| Deserialized values are dynamic | Deserializer builds Dynamic*, then FromReflect converts | Fine at measured speeds; `ReflectDeserialize` on leaf types shortcuts it |
| API churn | `FromType` became `CreateTypeData` in 0.20; the PartialReflect split came in 0.15 | Keep bevy types out of public signatures except the derive and `dyn PartialReflect` |

Derive error messages were otherwise acceptable: a missing `Reflect` on a field type says "consider annotating with `#[derive(Reflect)]`", but repeats it for each trait (~60 lines for one cause).

## Measurements

Apple M3 Ultra (28 cores), dev profile as in the workspace (opt-level 1, dependencies at 3). Other agents were compiling at the same time, so build numbers carry about ±15% noise.

### Compile time

| | bevy_reflect spike | serde-only baseline |
|---|---|---|
| Clean `cargo build` (lib) | 15.5 s, 14.5 s | 9.5 s, 9.6 s |
| Clean `cargo test --no-run` | 17.1 s | n/a |
| Build units | 77 | 38 |
| Incremental, components.rs changed (one component added) | 1.63–1.86 s | 0.30–0.33 s |
| Incremental, touch only | 0.48–0.50 s | 0.18–0.19 s |
| 20 extra 4-field components in one edit | +1.1 s (~55 ms each) | +0.2 s (~10 ms each) |

The spike crate also contains ~1,000 lines of glue, so absolute incremental numbers are not like-for-like. **The fair figure is per component: a reflected component costs about 5× a serde one, ~55 ms of dev build.** About 30 reflected engine types would add ~1.5 s to a `box_core` rebuild. Heaviest new crates: bevy_reflect 4.9 s, bevy_reflect_derive 2.6 s, derive_more-impl 2.1 s, bevy_macro_utils 1.8 s, assert_type_match 1.6 s. **Net: +5–6 s on a clean build.**

### Serialization (release; dev in parentheses)

10,000 entities with a random hierarchy. All have Uid, Name and Transform; 25% have PointLight, 33% Health, 20% Npc. The file is 6.5 MB of pretty JSON.

| Operation | 10k | 1k |
|---|---|---|
| Save (reflect to pretty JSON bytes) | 29 ms (36) | 3.0 ms |
| Parse JSON to value tree | 16 ms (16) | 1.5 ms |
| Value tree to world (reflect deserialize + FromReflect + hecs + hierarchy) | 28 ms (30) | 2.5 ms |
| Load total | 46 ms (52) | 4.7 ms |
| Play clone (save + load) | 78 ms (90) | 7.3 ms |

Play on a 10k-entity scene starts in under 0.1 s even in a dev build, so no binary snapshot format is needed for Play.

## API sketch for box_core in M2

```rust
// ---- box_core::reflect ----
pub use bevy_reflect::{FromReflect, GetTypeRegistration, PartialReflect, Reflect, TypeInfo, TypePath, Typed,
                       std_traits::ReflectDefault, reflect_remote}; // Reflect derive re-exported under our path

/// Inspector metadata: `#[reflect(@Range { min: 0.0, max: 1.0 }, @Unit("m"))]`.
#[derive(Reflect, Clone, Copy)] pub struct Range { pub min: f64, pub max: f64 }
#[derive(Reflect, Clone, Copy)] pub struct Unit(pub &'static str);
#[derive(Reflect, Clone, Copy)] pub struct Step(pub f64);
#[derive(Reflect, Clone, Copy)] pub struct Hidden;
#[derive(Reflect, Clone, Copy)] pub struct NotSaved;

#[derive(Clone)] pub struct ReflectComponent { /* fn ptrs: contains, get, get_mut, insert, remove */ }
pub type Value = Box<dyn PartialReflect>;

/// One registry for inspector, scenes, undo, scripting; shared by edit and play worlds (Arc held by World).
pub struct Registry { /* TypeRegistry + BTreeMap<&'static str, TypeId> */ }
impl Registry {
    pub fn register<T: Component + GetTypeRegistration + TypePath>(&mut self) -> Result<(), ReflectError>;
    pub fn register_resource<T: GetTypeRegistration + TypePath + 'static>(&mut self) -> Result<(), ReflectError>;
    pub fn component(&self, name: &str) -> Option<ComponentType<'_>>;
    pub fn components(&self) -> impl Iterator<Item = ComponentType<'_>>; // sorted
}
impl ComponentType<'_> {
    pub fn name(&self) -> &'static str; pub fn doc(&self) -> Option<&str>;
    pub fn fields(&self) -> impl Iterator<Item = FieldMeta>; pub fn default_value(&self) -> Option<Value>;
}
pub struct FieldMeta { pub name: &'static str, pub type_path: &'static str, pub doc: Option<&'static str>,
                       pub range: Option<Range>, pub unit: Option<&'static str>, pub step: Option<f64>, pub hidden: bool }
/// "PointLight.intensity", parsed once (component name + bevy ParsedPath).
pub struct FieldPath { /* .. */ }
impl FieldPath { pub fn parse(registry: &Registry, s: &str) -> Result<Self, ReflectError>; }
pub fn clone_value(v: &dyn PartialReflect) -> Value;
pub fn replace(target: &mut dyn PartialReflect, v: &dyn PartialReflect) -> Result<(), ReflectError>;
pub enum ReflectError { /* UnknownComponent, Missing, AlreadyPresent, Path, Shape, Duplicate, Cycle .. */ }

// ---- box_core::world::World additions ----
impl World {
    pub fn registry(&self) -> &Registry;
    pub fn spawn_saved(&mut self, uid: Uid, name: &str, bundle: impl DynamicBundle) -> Entity; // maintains Uid index
    pub fn entity_by_uid(&self, uid: Uid) -> Option<Entity>;
    pub fn component_names(&self, e: Entity) -> Vec<&'static str>;
    pub fn get_reflect(&self, e: Entity, component: &str) -> Option<impl Deref<Target = dyn Reflect> + '_>;
    pub fn get_reflect_mut(&mut self, e: Entity, component: &str) -> Option<&mut dyn Reflect>;
    pub fn get_field(&self, e: Entity, path: &FieldPath) -> Result<Value, ReflectError>;
    pub fn set_field(&mut self, e: Entity, path: &FieldPath, v: &dyn PartialReflect) -> Result<(), ReflectError>;
    pub fn insert_reflect(&mut self, e: Entity, component: &str, v: &dyn PartialReflect) -> Result<(), ReflectError>;
    pub fn insert_default(&mut self, e: Entity, component: &str) -> Result<(), ReflectError>;
    pub fn remove_reflect(&mut self, e: Entity, component: &str) -> Result<Value, ReflectError>;
    pub fn placement(&self, e: Entity) -> Placement;                              // parent Uid + sibling index
    pub fn place(&mut self, e: Entity, p: Placement) -> Result<(), ReflectError>; // no panic on cycles
}

// ---- box_core::scene ----
pub const SCENE_VERSION: u32 = 1;
pub struct UnknownComponents(pub BTreeMap<String, serde_json::Value>);
pub fn save(world: &World) -> Result<Vec<u8>, SceneError>;
pub fn load(bytes: &[u8], registry: Arc<Registry>) -> Result<World, SceneError>;
pub fn load_value(tree: &serde_json::Value, registry: Arc<Registry>) -> Result<World, SceneError>;
pub fn save_subtree(world: &World, root: Entity) -> Result<serde_json::Value, SceneError>;
pub fn instantiate(world: &mut World, records: &[serde_json::Value], at: Placement) -> Result<Vec<Entity>, SceneError>;
pub fn clone_world(world: &World) -> Result<World, SceneError>;

// ---- box_editor (M2a), as in src/undo.rs ----
pub enum Edit { SetField { uid, component: &'static str, path: String, old: Value, new: Value },
                AddComponent { uid, component, data: Value }, RemoveComponent { uid, component, data: Value },
                Spawn { snapshot: EntitySnapshot }, Despawn { snapshot: EntitySnapshot },
                Reparent { uid, old: Placement, new: Placement } }
pub struct EntitySnapshot { pub uid: Uid, pub placement: Placement, pub records: serde_json::Value }
pub struct EditorCommands { /* begin, commit, set_field, add/remove_component, spawn, despawn, reparent, undo, redo */ }
```

M2 notes:
- **Root order.** The spike orders roots by Uid; users will want to reorder roots, which needs a persisted root order (e.g. `SceneRoots`).
- **The Uid index belongs in `World`** (updated in spawn_saved/despawn/insert). hecs has no hooks, so raw `ecs_mut()` inserts of `Uid` bypass it.
- `World::set_parent` panics on cycles; editor and script paths need the non-panicking `place`.
- Spawn/Despawn undo snapshots reuse the scene serializer, so unknown components survive delete and undo.
- If `Name` is removed from an entity, the file says `"name": null`.

## Risks for M7 scripting

1. **Script-declared components do not fit bevy's registry.** It is keyed by `TypeId`, and `TypeInfo` is `&'static`. Design `ComponentType` in M2 as an enum (Rust type | script type) that the inspector, serializer and undo consume, rather than letting them use bevy `TypeInfo` directly. Script data can use the `UnknownComponents`-style named-value storage, which this spike shows round-trips byte-identically. This is the biggest design risk: cheap in M2, expensive later.
2. **Type names for `.d.luau`.** `TypeInfo` has names, types, docs and attributes, but type paths need a mapping table (`core::option::Option<box_engine::Uid>`, `alloc::vec::Vec<..>`, glam types to Luau `vector`). Remote-wrapper names would leak too.
3. **Methods.** The bevy_reflect `functions` feature was not evaluated; expect our own `#[script]` method attribute or hand-written bindings.
4. **Per-call cost.** Generic access is dyn dispatch plus path walking. Use pre-parsed `ParsedPath` and typed fast paths for Transform. Measure in S4 (10k callbacks per frame); S2 measured none of it.
5. **Luau ↔ reflect conversion.** One generic converter per `ReflectRef` kind (a few hundred lines). Script values arrive dynamic, and arrays can be neither cloned nor FromReflect'ed generically, so the `replace` path is mandatory.
6. **`Send + Sync`** is required on every reflected type, so no mlua handles or Rc in components.
7. **Churn.** The binding generator is the heaviest `TypeInfo` consumer; budget an upgrade day per Bevy bump, together with the glam pin.

## If bevy_reflect had failed: minimal own derive

A replacement would need: a derive for structs, tuple structs and enums producing `&'static TypeInfo` (names, ids, paths, docs, `@` attributes) and field access by index and name; built-in impls for primitives, `String`, `Option`, `Vec`, `[T; N]`, `Uuid` and glam types; a path parser, a dynamic value type with structural replace, and a name-keyed registry with type data; a serde bridge. Roughly 3,000–5,000 lines and 3–5 weeks, plus maintenance. The gains would be no churn, no MSRV or glam coupling, replace-first semantics and first-class script types. Not worth it now, since every blocker found is solved by the glam pin or absorbed by ~1,000 lines of facade. Revisit if the M7 script-type design (risk 1) needs deep registry changes.

## Not covered

- The egui inspector row from the D3 proof-of-concept list (the brief excluded GUI); `FieldMeta` is what a row needs.
- `box_render` on glam 0.33.
- Resources and asset settings through the registry (same mechanism, not exercised).
