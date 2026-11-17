# Scene System

How box-engine represents entities, components, hierarchy, prefabs and saved scenes, and how the editor and runtime share that data. Decision records: D3 (entity model) and D6 (formats) in [DECISIONS.md](DECISIONS.md).

## 1. Model

**Entities with typed components, arranged in a parent/child tree, with reflection on every component.**

This is deliberately between a full ECS and a Godot-style node tree:

- Like an ECS: data lives in plain component structs; behaviour lives in systems (functions) or scripts; a game adds its own components.
- Like a node tree: every scene entity has a name, a transform and a parent. Users think in a hierarchy, and the editor shows one.
- Unlike Bevy-style ECS: there is no parallel scheduler, no archetype-level API exposed to users, and no system parameter magic. A system is `fn(&mut World, ...)`.

### Why not a full ECS framework

The demanding requirements here are **dynamic access by name, reflection, stable IDs and undo**, not iteration speed over a million entities. Those are engine-owned concerns whichever storage is used. `bevy_ecs` would bring high churn and would leak Bevy concepts into our plugin and script APIs. A custom ECS would be effort with no user-visible return.

### Storage

`hecs` stores entities and components. `box_core::World` wraps spawning, hierarchy, reflection and serialization, and re-exports `hecs` query types under `box_core` names. Query types are not re-implemented; wrapping them fully would be insulation against a storage swap that nobody plans.

**Known limit.** Systems are `fn(&mut World)` with no declared data access, so they cannot be run in parallel later without changing every system. This is accepted: the target is thousands of active entities, not hundreds of thousands. It is recorded as risk R24.

```rust
// Illustrative API.
let e = world.spawn((Name::new("Lamp"), Transform::from_xyz(0.0, 2.0, 0.0), PointLight::default()));
world.set_parent(e, Some(root));

for (entity, (transform, spin)) in world.query::<(&mut Transform, &Spin)>() {
    transform.rotate_y(spin.speed * dt);
}

// Dynamic access by path, used by the inspector, the CLI, scripts and undo:
let path = world.field_path("PointLight.intensity")?;
let value = world.get_field(e, &path)?;
```

## 2. Identity

| ID | Type | Lifetime | Used for |
|---|---|---|---|
| `Entity` | Generational index | One run of one world | Fast runtime references |
| `Uid` | UUID stored as a component | Persistent across save, load, Play and undo | Scene files, undo, prefab overrides, editor selection, cross-entity references in components |
| `AssetId` | UUID stored in the asset's `.meta` file | Persistent | References from scenes and components to assets |

**Runtime entities are exempt.** `Uid`, `Name` and a place in the hierarchy are required only for entities that are saved in a scene or shown in the editor. Entities spawned during play (projectiles, particles, pickups) may carry only the components they need, which keeps them cheap.

A component field that refers to another entity is an `EntityRef`, which serializes as a `Uid` and resolves to an `Entity` on load. The Play snapshot preserves `Uid`s, so the editor can map between edit-world and play-world entities.

## 3. Built-in components

| Component | Purpose |
|---|---|
| `Name`, `Uid` | Identity |
| `Transform` | Local translation, rotation, scale |
| `GlobalTransform` | Derived world matrix (not serialized) |
| `Parent` / `Children` | Hierarchy (maintained by the world, not edited directly) |
| `Visibility` | Self and inherited visibility |
| `MeshRenderer` | Mesh handle plus material handles per group. Runtime data: derived, never saved. From M2 it is maintained from `Primitive` (and later generators and imported models) by `sync_scene_meshes` in PostUpdate |
| `StandardMaterial` | As a component: the inline material parameters of a `Primitive` entity, drawn when it has no `MaterialRef` |
| `MaterialRef` | A link from a `Primitive` entity to a material asset (`.material.json`, standard or shader, or a glTF material). Saved; edits to the file show through hot reload |
| `Interpolated`, `KinematicCharacter` | Smooth drawing of entities moved in FixedUpdate; the kinematic character controller's settings (`box_physics`) |
| `Camera` | Projection, clear, target |
| `DirectionalLight`, `PointLight` | Lights |
| `Primitive` | A built-in shape (cuboid, sphere, cylinder, capsule, plane) with parameters and `cast_shadows`, in `box_core` from M2. Since M5 a generator (`box_proc::builtin`): drawn through the generator runtime; saved form unchanged |
| `Procedural` + a generator component | Procedural objects, from `box_proc` (M5); see [PROCEDURAL.md](PROCEDURAL.md) |
| `Collider`, later `RigidBody`, `CharacterController` | From `box_physics` (M3; dynamics in M7) |
| `AudioSource`, `AudioListener` | From `box_audio` (M6) |
| `Script` | From `box_script` (M7) |
| `SkinnedMesh`, `AnimationPlayer` | Skinning and clip playback (M7) |
| `PrefabInstance` | Marks the root of an instantiated scene (M4) |

Scene-wide settings (sky, fog, ambient, tone mapping) are an `Environment` resource saved with the scene, under `"resources"`. Any type registered with `register_resource` and present in the world is saved the same way.

**Game-specific components** are ordinary Rust structs deriving `Reflect` in a native project, or script-declared property tables in a script project. Both appear in the inspector and are saved in scenes the same way.

## 4. Transform hierarchy

- `Transform` is local. `GlobalTransform` is computed in PostUpdate only for subtrees whose `Transform` or `Parent` changed since the last propagation. A cached copy per entity is compared, so raw `&mut Transform` writes need no dirty marking. `GlobalTransform` is derived: never write it; use `transform::compute_global` for a fresh world transform between propagations. A parent without a `Transform` counts as identity.
- Reparenting through `EditStack::reparent` (editor drag, `box scene apply`) keeps world position, as one undo step (`Reparent` plus a `Transform` `SetField`). `Keep::Local`, or `"keep": "local"` in JSON, keeps the local transform. With non-uniform parent scale and a rotated child only the position is exact (no shear). Entities without a `Transform`, and singular parents, keep their local transform. `World::set_parent` and `World::place` are low-level and keep the local transform.
- Fixed-timestep simulation writes transforms; rendering interpolates entities marked as simulated.
- Rigid-part animation (birds and creatures in a stylized world) is plain transform animation over child entities. Skinned animation is deferred; see section 10.

## 5. Meshes, materials and instancing

- A `Primitive` entity's material: `MaterialRef` (when it names an asset) > inline `ShaderMaterial` > inline `StandardMaterial` > the default. `sync_scene_meshes` resolves the link through `AssetLookup` and the asset server and uses the asset's own handle; it draws the inline material while the asset loads and the placeholder (one error) when it cannot be loaded. The link is resolved again only when the reference, the lookup's generation or the server's finished imports change.

- A `MeshRenderer` references a `Handle<Mesh>` and one material handle per material group. A `Mesh` asset is a `MeshData` plus its bounds. Handles are reference-counted and point into asset stores in the world.
- Meshes come from three sources through the same handle type: imported assets, generators, or game code that builds `MeshData` directly.
- From renderer stage RS3, entities sharing the same mesh and material are drawn as one instanced draw automatically.
- For very large counts (scattered vegetation), one entity can hold an array of instance transforms, so that thousands of plants are not thousands of entities. This arrives with scatter in M6.

## 6. Reflection

Components, resources and asset settings derive `Reflect` (from `bevy_reflect`, re-exported as `box_core::reflect`) and are registered in a `TypeRegistry`:

```rust
#[derive(Reflect, Default, Clone)]
#[reflect(Component, Default, Clone)]
#[type_path = "box_engine"]
pub struct PointLight {
    pub color: Color,
    /// Luminous power.
    #[reflect(@Range { min: 0.0, max: 100_000.0 }, @Unit("lm"))]
    pub intensity: f32,
    pub radius: f32,
}
```

One registry drives five things: the inspector, scene serialization, undo values, script bindings, and generated script type definitions. This is the main reason reflection is worth its dependency.

Game and editor code import reflection only through `box_core`, so replacing the underlying crate would be confined to core and the derive re-export.

## 7. Scene files

**Format: JSON text**, one file per scene, with the extension `.scene.json`.

```json
{
  "version": 1,
  "resources": { "Environment": { "fog_density": 0.02, "exposure": 0.55 } },
  "entities": [
    {
      "uid": "0193a4f2-...",
      "name": "Tower",
      "parent": null,
      "components": {
        "Transform": { "translation": [0, 0, 0], "rotation": [0, 0, 0, 1], "scale": [1, 1, 1] },
        "Primitive": { "shape": { "Cylinder": { "radius": 1.5, "height": 12.0, "segments": 32 } }, "cast_shadows": true },
        "StandardMaterial": { "base_color": [0.8, 0.78, 0.75, 1.0], "roughness": 0.7, "metallic": 0.0 }
      }
    }
  ]
}
```

Rules:

- **Entities are written in hierarchy pre-order and keys in a stable order** (components sorted by name, fields in declaration order, numeric arrays of up to four on one line), so diffs are small and merges are tractable. Root order is the file order, so roots can be reordered.
- **Derived data is never saved**: no `GlobalTransform`, no generated meshes, no GPU state.
- **Unknown components and fields are preserved** as raw values and written back, so opening a scene without a plugin does not destroy its data. This is built when plugins make it possible to hit the case (M3 at the earliest, M9 at the latest).
- Vectors and quaternions are arrays; colours are linear `[r, g, b, a]` (loading also accepts `[r, g, b]` and `{r, g, b, a}`).
- Assets are referenced by `AssetId`, with the path stored alongside as a human-readable hint and a repair aid.
- Why JSON: universal tooling, readable by scripts and AI assistants, and existing procedural games already save JSON documents. RON handles Rust enums more naturally but offers little else here. The choice is easy to change because loading goes through an untyped value tree.

### Versioning and migration

Built in M9 (`box_core::migrate`): envelope format chains plus per-type steps carried as `"$version"`; unknown fields of known types are kept too (`UnknownFields`). No shipped format needed a real step through M8 (every change was additive, checked against every committed file). Project upgrade is explicit, with a backup (`box upgrade`, the editor's Upgrade prompt). See [FORMATS.md](FORMATS.md).

1. Read the file into an untyped value tree.
2. If `version` is older than current, run the ordered chain of migration functions (`v1 -> v2 -> v3`), each of which edits the tree.
3. Deserialize the tree into typed components through the registry.

Each component type may also carry its own version and migration for plugin-defined data. A scene written by a newer engine is refused with a clear error. Migrations are tested by keeping one fixture scene per historical version.

A compact binary "cooked" scene format is deferred to M9 packaging and will be generated from the text format, never edited.

## 8. Prefabs

A prefab is a scene file (`.prefab.json`) intended for instancing.

**First version (M4): instanced scenes without overrides.** A `PrefabInstance` entity references a scene asset; its contents are instantiated beneath it and follow the source scene when it changes. The instance root's own transform and name are per-instance. This covers reusable props and imported glTF models.

**Later, when a project needs it: overrides.** The design below is a sketch for that stage, not a commitment.

- An instance stores a reference to the prefab asset plus an **override list**: `(uid path, component, field path, value)`, along with added and removed entities or components.
- On load, the prefab is instantiated and overrides are applied. Entities inside an instance get `Uid`s derived deterministically from the instance `Uid` and the prefab-local `Uid`, so overrides and references stay stable.
- Nested prefabs are allowed; the override path includes the chain of instance `Uid`s.
- "Apply to prefab" and "Revert" are editor commands over the override list.

Overrides, structural overrides and prefab variants are all after M9 unless a game forces them earlier.

**M4 as built.** A `PrefabInstance` entity gets its scene asset's entities instantiated beneath it by the asset glue; the source can be a `.scene.json` or a glTF `#Scene` (scene resources are ignored). Every instanced entity carries the runtime marker `prefab::Instanced { instance, depth }`: it is never saved (`save`/`save_subtree` skip it) and not editable (`box_core::edit` refuses edits to it and placements under it). Uids are `instanced_uid(root uid, local uid)`, deterministic across reloads, loads and Play. Contents are rebuilt when `PrefabInstance.scene` changes (an ordinary undoable edit) or the scene asset's version changes (hot reload). Nested prefabs instantiate down to `MAX_PREFAB_DEPTH = 8`; a prefab that contains itself is refused with one error. The editor hides instanced entities in the hierarchy, and picking or marquee on them selects the instance root (`prefab::editable_ancestor`).

**Generated entities (M5).** Entities marked `Derived` (rule output, `box_core::derived`) or `Instanced` (prefab contents) are "generated": `prefab::is_generated`, filtered in queries with `prefab::NotGenerated<Q>`. Generated entities are never saved (their saved descendants are written under the nearest saved ancestor), `box_core::edit` refuses to edit them or to place entities under them (the error names the rule and the owner), and they never make a name ambiguous. Deleting an owner deletes its parts; undo restores only the owner, and its rule rebuilds the parts with the same uids.

## 8a. Runtime scenes and saved games

- **Runtime scene API (M3):** load a scene, unload it, switch scenes, and load a scene additively beneath an entity. Entities can be marked persistent so that they survive a scene switch.
- **M3 as built** (`box_engine::scenes`): a `Scenes` resource takes `switch(path_or_name)`, `load_additive(path_or_name)` and `unload(id)` requests; `ScenePlugin` (added by `app_with_world`) applies them at the start of the next `Update`, so a request made in Update or PostUpdate takes effect next frame, after that frame's FixedUpdate steps, and the new entities are propagated, meshed and drawn in the same frame. Results arrive as `SceneEvent::{Loaded, Unloaded, Failed}`; nothing panics.
  - *Switch:* the file is resolved, parsed and loaded into a scratch world first; on failure the world is unchanged. Then every instance (additive ones too) is despawned with all descendants. Each resource saved in the new file replaces the current one; resources it does not mention keep their values. Additive loads ignore the file's resources. If a uid in the file is already live (the same scene loaded twice), the whole instance gets fresh uids and references inside components are rewritten.
  - *Runtime entities:* loaded entities carry a runtime-only `SceneInstance(id)` marker (never saved). Entities without it survive a switch, except those parented under a scene entity, which go with it. Inserting `SceneInstance(id)` ties a runtime entity to an instance. Saved entities already in the world when the app is built become the main instance.
  - *Switch policy (DX Batch 2):* `Scenes::runtime_entities` is `RuntimeEntities::KeepAll` (the default, for compatibility: runtime-spawned projectiles and effects survive into the next scene) or `DespawnUnlessPersistent` (recommended, as in Unity and Godot): after the old instances are unloaded, every runtime root entity without the `Persistent` marker is despawned with its descendants. Additive loads and unloads never touch runtime entities. The default will change to `DespawnUnlessPersistent` in a later release.
  - *Paths:* absolute paths as given; otherwise `<root>/assets/<p>`, then `<root>/<p>`, each with or without `.scene.json`; then a scene name (file name, or path under `assets/`, without extension; ambiguous names are an error). Without a project root, relative paths resolve against the working directory.
  - Not yet: loading additively *beneath an entity*, and a persistent marker beyond the "no `SceneInstance`" rule.
- **Saved games (M7):** a save is a serialized snapshot of the play world, using the same serializer as scenes, limited to entities and components marked as saveable. Scripts take part through save and load hooks.
- **Script state model (M7):** script properties live in the `Script` component and are saved automatically. Other per-instance script state is saved only through the hooks. Defining this in M7 keeps Play snapshots, hot reload and saved games consistent with each other.

## 9. How the editor and runtime share scene data

- There is **one** representation: the `World` plus the registry. The editor edits a live `World` through `EditorCommands`; the runtime runs a `World`.
- The scene file is the serialized form of that world. The editor saves it; the player loads it; Play mode round-trips it in memory.
- Editor-only state (selection, camera bookmarks, panel state) is stored outside the scene, in `.box/` under the project, keyed by `Uid`.
- Editor-only helper components, stripped at export, are a possible later addition.

## 10. Deferred

- Skinned meshes, animation clips (including glTF node animation) and a clip player are scheduled for M7. Neither the construction slice nor the editor needs them, but most other games do. `MeshData` reserves joint and weight channels from M1 so the format does not change.
- Scene streaming and additive multi-scene editing beyond prefabs.
- A parallel scheduler and change-detection queries. Dirty flags are explicit where needed (transforms, procedural objects).
- Relations beyond parent/child.
