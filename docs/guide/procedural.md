# Procedural modelling

box-engine builds geometry from parameters. A procedural object stores a few numbers (a height, a radius, a list of points) and the engine turns them into a mesh whenever they change. On top of that, **rules** turn simple saved intent (a path, a row, an outline) into many generated parts: a path becomes a gravel strip, stepping stones and lanterns; a footprint becomes walls and a roof. This guide explains how that works, how to use the built-in pieces in the editor, and how to write your own rules in Luau and your own generators and rules in Rust.

If you are new to the engine, read [Getting started](getting-started.md) first. The editor side (menus, tools, shortcuts) is covered in [The editor](editor.md#procedural-tools); script basics are in [Scripting](scripting.md); Rust game crates are in [Native plugins](native-plugins.md).

## Contents

- [How procedural objects work](#how-procedural-objects-work)
- [The built-in generators](#the-built-in-generators)
- [Using generators in the editor](#using-generators-in-the-editor)
- [Splines and sweeps](#splines-and-sweeps)
- [Rules: from intent to parts](#rules-from-intent-to-parts)
- [Script rules in Luau](#script-rules-in-luau)
- [Overrides: changing one generated part](#overrides-changing-one-generated-part)
- [Generators in Rust](#generators-in-rust)
- [Rules in Rust](#rules-in-rust)
- [Terrain](#terrain)
- [Terrain paths](#terrain-paths)
- [Scatter](#scatter)
- [Bake to Mesh](#bake-to-mesh)
- [Architecture: box_arch](#architecture-box_arch)
- [Performance notes](#performance-notes)

The samples in this guide are real, tested files: the Luau rule is `examples/guide_samples/assets/rules/walk.luau` (its scene is `assets/scenes/procedural.scene.json` in the same project), and the Rust samples are in `examples/guide_native/src/procedural.rs`.

## How procedural objects work

- **A generator is a component.** `Lathe`, `Extrude`, `Sweep` and even the plain `Primitive` (Cube, Sphere, ...) are components on an entity. They hold parameters only, and they appear in the inspector, scene files, undo, the CLI and scripts like any other component.
- **Parameters are saved; meshes are not.** A scene file stores the generator's fields. The mesh is a cache, rebuilt when the scene loads and whenever a parameter changes. Edit a field (in the inspector, from a script, with `box scene apply`) and the shape updates, usually in the same frame.
- **Generators are pure.** The same parameters and seed always give the same mesh. Randomness comes only from the seed.
- **Problems are warnings, not crashes.** A generator given impossible values (a negative size, too many segments) clamps them and reports a warning. Each new warning is logged once: the editor shows it as a toast and in the Console.

### The Procedural component

Every generator entity can carry an optional `Procedural` component with settings shared by all generators:

| Field | What it does |
|---|---|
| `seed` | The seed of the generator's random choices. Change it for another variation of the same parameters. Generators without random choices ignore it. |
| `materials` | Material assets by **material slot**, in the generator's slot order. An empty entry uses the entity's own material. |
| `cast_shadows` | Whether the generated mesh casts shadows (default on). |

Without a `Procedural`, the seed is 0, shadows are on, and every slot uses the entity's own material.

### Materials by slot

A generator divides its mesh into named **material slots**: a `Lathe` has Surface and Cap, an `Extrude` has Sides, Caps and Bevel. Slot `i` draws `Procedural.materials[i]` when that names a material asset. Otherwise it draws the entity's own material, with the same precedence as a primitive: a `MaterialRef`, else an inline `ShaderMaterial`, else an inline `StandardMaterial`, else the default material. So a generator with a `StandardMaterial` next to it looks like a coloured primitive, and you can override slots one at a time.

## The built-in generators

| Generator | What it makes | Main parameters | Material slots |
|---|---|---|---|
| `Primitive` | Cube, sphere, cylinder, capsule, plane | `shape` (`Cuboid`, `Sphere`, `Cylinder`, `Capsule`, `Plane`), `cast_shadows` | Surface |
| `Lathe` | A profile turned around the entity's Y axis: vases, columns, domes, towers | `shape`: `Vase`, `Column` (height, radius), `Dome` (radius, rings) or `Custom` (radius, height points); `segments`, `start_angle`, `sweep_angle` (a partial turn), `caps`, `smooth_angle` | Surface, Cap |
| `Extrude` | A flat outline pushed up along Y into a solid: slabs, plaques, stars, frames | `outline`: `Rectangle` (rounded corners), `Circle`, `Polygon`, `Star` or `Custom`; `hole`: `None`, `Inset`, `Circle`, `Rectangle`, `Custom`; `depth`, `bevel_width`, `bevel_segments`, `caps`, `smooth_angle` | Sides, Caps, Bevel |
| `Sweep` | A profile carried along a `Spline`: tubes, rails, kerbs, paths | `spline` (another entity, or none for this one's), `profile` (`Rect`, `Circle`, `Custom`), `tolerance`, `orientation` (`Up` or `RotationMinimizing`), `caps` | Surface, Caps |

Presets keep the common case to one dropdown; pick `Custom` to type your own points. Lathe and Extrude shapes stand on the entity's origin (y = 0). The [component reference](../reference/components.md) lists every field with its range and unit.

The sample scene has one of each (a column, a star and a tube) next to two garden walks made by a script rule. Render it headless and open the PNG:

```bash
cargo run -p box_cli -- render examples/guide_samples/assets/scenes/procedural.scene.json --out target/captures/guide/procedural.png
```

## Using generators in the editor

- **Add one:** **+ Add ▸ Shapes ▸ Extrude**, **Lathe** or **Spline** (a `Sweep` with a `Spline` on the same entity: a tube to start from). Generators your game or plugins register appear under their own category, for example **+ Add ▸ Architecture**. The new object rests on the surface under the viewport centre, with a `Procedural` and a `StandardMaterial`.
- **Shape it** in the inspector. Fields have ranges, units and tooltips; drag a slider and the mesh follows while you drag.
- **Change one slot's material:** set the entry for that slot in **Procedural ▸ Materials**.
- **Try another variation:** change **Procedural ▸ Seed**.
- **Read its warnings** in the toasts and the Console.

Everything is an ordinary scene edit: undo works, and the scene file stores only the parameters. The same object written as scene data, as `box scene apply` takes it (from the sample's `authoring/procedural.commands.json`):

```json
{
  "op": "spawn", "uid": "9a0c0000-0000-4000-8000-000000000004", "name": "Column",
  "components": {
    "Transform": { "translation": [-3.0, 0.0, -3.0], "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [1.0, 1.0, 1.0] },
    "Lathe": { "shape": { "Column": { "height": 2.4, "radius": 0.25 } } },
    "StandardMaterial": { "base_color": [0.86, 0.82, 0.74, 1.0], "roughness": 0.7 }
  }
}
```

Fields you leave out keep their defaults.

## Splines and sweeps

A `Spline` is a saved component: a list of control points in its entity's local space, a mode and a `closed` flag.

| Field | What it does |
|---|---|
| `points[].position` | The point, in metres, in the entity's local space |
| `points[].in_tangent`, `out_tangent` | Bézier handles (Bézier mode only; empty means mirrored or automatic) |
| `points[].roll` | Turns the swept profile about the path at this point (radians) |
| `points[].scale` | Scales the swept profile at this point (x across, y up) |
| `mode` | `CatmullRom` (smooth, through every point; the default), `Bezier` (smooth, with handles) or `Polyline` (straight segments with sharp corners) |
| `closed` | The last point joins the first |

A `Sweep` carries its `profile` along a spline:

- With `spline` empty it reads the `Spline` on its own entity. With `spline` set to another entity's uid it reads that one, and stays on it when either entity moves.
- `orientation`: `Up` keeps the profile upright (walls, roads, fences); `RotationMinimizing` avoids twisting (tubes and cables in 3D).
- `tolerance` is how far the sampled path may stray from the curve: smaller is smoother and heavier.
- `caps` closes the ends of an open spline (closed profiles only).
- Editing the spline regenerates exactly the sweeps that read it. A sweep whose spline is missing draws nothing and warns.

In the editor:

- **Edit Spline** (toolbar **Spline**): select a spline, or a tube that follows one. Drag points and handles (Shift: up and down only), Cmd-click the curve to insert a point, Delete or Backspace removes the selected point, Enter or Escape finishes. Each drag, insert or delete is one undo step.
- **Draw Path** (toolbar **Path**, shown once the scene has a script rule intent): draws the points of script rule paths, described [below](#drawing-intents-with-the-path-tool).

## Rules: from intent to parts

A generator makes one mesh from its own parameters. Often you want more: a path that becomes a fence with posts and rails, a room that becomes four walls and a roof, where walls that meet must agree about their corner. That is what **rules** do.

- **Intent** is what you save: a small component that says what you want (`ScriptIntent` with a path's points, or your own Rust component such as a `CairnRow`).
- **A rule** is a pure function from every intent of its kind to a list of **parts**.
- **Parts** are generated child entities of the intent's entity (the **owner**), usually carrying a generator. They are never saved: like meshes, they are rebuilt from the intent.

What the engine does for every rule, Luau or Rust:

- **Runs it only when something changed:** an intent's value, uid or world transform, its overrides, or (for a script rule) the rule file. An idle rule costs one comparison per intent.
- **Keys every part** `<owner uid>/<role>`, for example `9a0c…0002/leg1/stone0`. Roles must be semantic and stable: `post3`, `rail_left`, `leg1/stone0`, never a bare number such as `3`. A part with the same key is updated in place on the next run; a key that disappears despawns its part.
- **Writes only what changed.** A part's component is written only when its value differs, so unchanged parts are not regenerated.
- **Records provenance.** Each part carries `Derived { owner, key, rule }`. In the editor a click on a part selects its owner; Cmd+Alt+click shows the part read-only with "Generated by".
- **Shows warnings on the owner.** Rule warnings (`out:warn` in Luau, `RuleOutput::warn` and `Part::warn` in Rust) are kept on the owner (its `GeneratedWarnings`), shown in its inspector and with a badge in the viewport, and logged to the Console once. A part's own generator warnings are logged to the Console like any generator's.
- **Lists the rule.** **View ▸ Rules** shows each rule with its intents, parts, last run time and warnings, and a switch that turns it off for this session (nothing is saved).

Rules run in the editor while you edit, in Play, in the player and in `box render`.

## Script rules in Luau

A script rule is a `.luau` file under `assets/`. It needs no Rust, so it works in a data project. An entity becomes an intent of the rule by carrying a `ScriptIntent` component:

| `ScriptIntent` field | What it holds |
|---|---|
| `rule` | The rule file, relative to `assets/` (`rules/walk.luau`) |
| `points` | Points in the owner's local space: a path, an outline or positions. The rule decides what they mean. |
| `properties` | Values of the rule's declared properties that differ from their defaults |

The sample rule, `walk.luau`, turns a path into a garden walk. It starts by declaring its properties. They work like a script's: each becomes an inspector row on every intent of the rule, with the declared default, range, unit and tooltip.

```lua
local Walk = {}

Walk.properties = {
    width = { type = "number", default = 1.2, min = 0.4, max = 4, unit = "m", doc = "Width of the gravel strip" },
    spacing = { type = "number", default = 0.8, min = 0.3, max = 5, unit = "m", doc = "Distance between two stones" },
    lanterns = { type = "boolean", default = true, doc = "A lantern beside every point" },
    stone = { type = "color", default = Color.srgb(0.62, 0.6, 0.56), doc = "Stone colour" },
}
```

The module's `rule(intents, out)` function receives every intent naming the file, in uid order. Each intent `it` has `it.uid`, `it.points`, `it.props` (the effective property values), the owner's world `it.position`, `it.rotation` and `it.scale`, and `it:to_world(p)` / `it:to_local(p)`. A rule that cannot do anything useful says so with `out:warn` and moves on:

```lua
function Walk.rule(intents: { RuleIntent }, out: RuleOut)
    for _, it in intents do
        local p = it.props
        local pts = it.points
        if #pts < 2 then
            out:warn(it, "a walk needs at least two points")
            continue
        end
```

`out:part(owner, role, components, transform?)` adds a part. `components` names registered components; each is a patch over the component's default values, so you write only the fields you care about. Here the gravel strip is a `Sweep` along a `Spline` built from the intent's points:

```lua
        -- The gravel: one sweep along the whole path.
        local path = {}
        for i, q in pts do
            path[i] = { position = q }
        end
        out:part(it, "gravel", {
            Spline = { points = path, mode = "Polyline" },
            Sweep = { profile = { Rect = { width = p.width, height = 0.04 } }, orientation = "Up" },
            StandardMaterial = { base_color = Color.srgb(0.78, 0.74, 0.66), roughness = 1 },
        })
```

The optional fourth argument places the part relative to its owner: `position`, `rotation` and `scale` (a vector, or a number for a uniform scale). The stepping stones are `Extrude` hexagons, one key per stone, spaced along each stretch:

```lua
        for i = 1, #pts - 1 do
            local d = pts[i + 1] - pts[i]
            local count = math.max(1, math.floor(vector.magnitude(d) / p.spacing))
            for k = 0, count - 1 do
                out:part(it, `leg{i}/stone{k}`, stone, {
                    position = pts[i] + d * ((k + 0.5) / count) + UP * 0.02,
                    rotation = Quat.from_rotation_y(k * 0.9),
                })
            end
        end
```

The lanterns are `Lathe` vases beside every point (see the whole file for the rest). `out:part` accepts any registered component except `Uid`, `Name`, `Derived`, `Parent`, `Children` and `GlobalTransform`: generators, materials, colliders, lights, particle emitters, even a `Script`. The [rule reference](../reference/rules.md) lists them all with their types.

### Placing an intent

The sample scene has two walks. This is the second one as scene data: an ordinary entity with a `Transform` and a `ScriptIntent` whose `properties` turn the lanterns off and narrow the strip:

```json
{
  "op": "spawn", "uid": "9a0c0000-0000-4000-8000-000000000003", "name": "Side Walk",
  "components": {
    "Transform": { "translation": [2.0, 0.0, 3.0], "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [1.0, 1.0, 1.0] },
    "ScriptIntent": {
      "rule": "rules/walk.luau",
      "points": [[0.0, 0.0, 0.0], [2.5, 0.0, -2.0]],
      "properties": [
        { "name": "lanterns", "value": { "Boolean": false } },
        { "name": "width", "value": { "Number": 0.8 } }
      ]
    }
  }
}
```

Its one 3.2 m stretch gives four stones and the gravel: five parts, keyed `gravel` and `leg1/stone0` to `leg1/stone3`. The first walk, with four points and lanterns, has fourteen. `crates/box_engine/tests/guide_procedural.rs` plays the scene and checks exactly these parts, their transforms and the overrides below.

To play the scene, or render it:

```bash
cargo run -p box_player_app -- examples/guide_samples --scene procedural
cargo run -p box_cli -- render examples/guide_samples/assets/scenes/procedural.scene.json --out target/captures/guide/procedural.png
```

### Drawing intents with the Path tool

In the editor, make the first intent by adding a `ScriptIntent` to an entity (inspector ▸ Add Component) and setting its **Rule** to your `.luau` file; its property rows then appear below. Once the scene has a script rule intent, the toolbar shows **Path** (Draw Path):

- Select an existing path first: new paths copy its rule and settings.
- Click to place points on surfaces (they snap to other paths); Enter or a click on the last point finishes; a click on the first point closes the path; Backspace removes the last point; Esc cancels.
- With a path selected, drag its points to move them.

While you draw, the preview is the rule's real output.

### Editing and errors

- **Hot reload:** save the `.luau` file while the editor runs, or during Play, and the rule runs again with the new code.
- **Errors keep the last good result.** A rule that raises an error keeps the parts of its last good run; the error (`file:line: message`) is logged once and shown on every owner. A file that fails to compile keeps its previous version running.
- **Warnings name the line.** `out:warn` messages are prefixed with the rule's file and line. Give the Side Walk a single point and its inspector shows `rules/walk.luau:25: a walk needs at least two points`, and it has no parts.
- **Types:** `cargo run -p box_cli -- script types --project <dir>` writes the luau-lsp definitions, including `RuleIntent`, `RuleOut`, `RuleTransform` and `RuleComponents`, so your editor can check rule files.

### Purity

Rules are pure functions of their intents and their file:

- They have value types (`vector`, `Quat`, `Color`), `math`, `string`, `table` and `log`, but no `world`, `input`, `physics`, `events`, `scenes`, `task`, `wait` or `require`: calling them fails with a message.
- Each run executes the file afresh, so nothing survives between runs.
- `math.random` is reseeded with 1 for every run. Prefer values derived from the intent (the sample turns stones by `k * 0.9`), because in a rule with a narrowed scope (below) `math.random` fails.
- A run longer than 2 s is interrupted.

### Scope: how far a run looks

By default every run sees every intent naming the file, so a rule can compute any neighbour fact (where two fences meet, which end owns a shared post). The cost is that one edit re-runs the rule for all of them. A rule file can promise to need less:

```lua
-- A walk's parts depend only on its own points and properties, so an edit
-- re-runs the rule for the edited walk alone.
Walk.scope = "intent"
```

| `scope` | Meaning | An edit re-runs |
|---|---|---|
| `"file"` (default) | Parts may depend on any intent | Every intent |
| `"intent"` | An intent's parts depend only on itself | The changed intents |
| `{ neighbours = r }` | Parts depend on the intent and intents within `r` metres | The changed intents and those within `r` of their old or new place |

The engine cannot check the promise: a rule that reads farther than its scope gets results that depend on edit history. The first run, a run after the file changed or failed, and an edit touching more than a quarter of the intents still cover every intent.

When to narrow it: measured with a fence rule (about six parts per intent), one edit cost 2.5 ms at 100 intents, 25 ms at 1,000 and 128 ms at 5,000 without a scope, and 0.19 to 0.34 ms with `{ neighbours = ... }`. A rule with a few dozen intents does not need a scope; a rule you drag points of with hundreds of intents does. `examples/script_rules/assets/rules/fence.luau` is a full neighbour-scoped example: fences whose ends meet share one corner post, built by the fence with the lower uid.

## Overrides: changing one generated part

Rules decide every part, but sometimes one detail should differ: this stone red, that lantern gone. An **override** is saved on the owner, in an `Overrides` component, and applied after the rule runs:

- In the editor, Cmd+Alt+click a part and edit one of its fields: the change is saved as an override on the owner, marked like a changed value, with ↺ to reset it. **Edit ▸ Hide Generated Part** hides the inspected part. The owner's inspector lists its parts, overrides, hidden parts and warnings.
- As data, each entry names the rule, the part's role, `hidden`, and field values as `Component.field` paths with the value as JSON text. The first sample walk hides its second lantern and recolours its first stone:

```json
"Overrides": {
  "parts": [
    { "rule": "rules/walk.luau", "part": "lantern2", "hidden": true, "fields": [] },
    {
      "rule": "rules/walk.luau", "part": "leg1/stone0", "hidden": false,
      "fields": [{ "path": "StandardMaterial.base_color", "value": "[0.6, 0.25, 0.2, 1.0]" }]
    }
  ]
}
```

A script rule's name is its file path; a Rust rule's is the name it was added with. An override whose part or field no longer exists is kept and shown as a warning on the owner, never dropped silently. Overrides apply after the rule, so neighbours do not react to them; to change how parts relate, change the intent.

## Generators in Rust

A native game (see [Native plugins](native-plugins.md)) can add its own generator types. A generator is a reflected component with a `Default` and `PartialEq`, plus an `impl Generator`. The sample is a cairn, a stack of stones shifted and turned at random:

```rust
/// A cairn: a stack of stones, each smaller than the one below, shifted
/// and turned at random (seeded by `Procedural::seed`). Material slots:
/// Stone, and Top for the last stone.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone)]
pub struct Cairn {
    /// Number of stones.
    #[reflect(@Range { min: 1.0, max: 32.0 })]
    pub stones: u32,
    /// Width of the bottom stone.
    #[reflect(@Range { min: 0.05, max: 5.0 }, @Unit("m"), @Step(0.01))]
    pub size: f32,
    /// How far a stone may shift sideways, as a fraction of its width.
    #[reflect(@Range { min: 0.0, max: 0.5 }, @Step(0.01))]
    pub jitter: f32,
}
```

Doc comments become tooltips and `@Range`, `@Unit` and `@Step` drive the inspector, as for any component. The `impl Generator` sets three constants:

```rust
impl Generator for Cairn {
    const VERSION: u32 = 1;
    const MATERIAL_SLOTS: &'static [&'static str] = &["Stone", "Top"];
    const CATEGORY: &'static str = "Garden";
```

- `VERSION`: **bump it whenever the output changes for the same parameters and seed** (a fix, a new UV layout). It is part of the cache key, and the golden test below fails until you do.
- `MATERIAL_SLOTS`: the slot names shown next to `Procedural.materials`; mesh material groups index them. Default: one slot, Surface.
- `CATEGORY`: the Add menu: this one is **+ Add ▸ Garden ▸ Cairn**. Default: Procedural.

`generate(&self, ctx)` returns a `GenOutput`: a mesh (or none), warnings and a collider hint. It validates its parameters first (a size that is not above 0 becomes 0.6 with a warning, more than 32 stones builds 32 with a warning), then builds the stack. The only randomness is `ctx.rng()`, seeded from `Procedural::seed`:

```rust
        let rng = ctx.rng();
        let mut mesh = MeshBuilder::new();
        let mut y = 0.0;
        for i in 0..stones {
            // One random stream per stone: adding a stone keeps the others.
            let mut r = rng.fork(u64::from(i));
            let width = size * (1.0 - 0.6 * i as f32 / stones as f32);
            let height = width * r.range(0.35, 0.5);
            let shift = Vec3::new(r.range(-jitter, jitter), 0.0, r.range(-jitter, jitter)) * width;
            let turn = Quat::from_rotation_y(r.range(0.0, std::f32::consts::TAU));
            let slot = if i + 1 == stones { 1 } else { 0 };
            let stone = slab(Vec3::new(width, height, width * 0.8), slot);
            let at = shift + Vec3::new(0.0, y + height / 2.0, 0.0);
            mesh.append(&stone, Affine3A::from_rotation_translation(turn, at));
            y += height;
        }
        let mut out = GenOutput::mesh(mesh.finish());
        out.warnings = warnings;
        out
```

`slab` is a `MeshData::cuboid` whose one material group draws the given slot, so the top stone uses slot 1 (Top) and the rest slot 0 (Stone). `MeshBuilder` (in `box_engine::box_proc::mesh_builder`) also has triangles, quads, polygons, strips and grids by slot, and drops degenerate or non-finite triangles with a warning; `box_proc` has the 2D polygon kernel, `extrude`, `lathe`, `sweep` and `split` (divide a length into bays) for bigger generators.

Rules for generators:

- **Pure:** no world access, no globals, no time, no iteration over hash maps, no randomness except `ctx.rng()`. Give each element its own stream (`rng.fork(i)`) so adding an element does not reshuffle the others.
- **Degrade, never panic.** A panic in `generate` is caught and becomes a warning, and the object draws nothing.
- **Inputs from other entities** are declared, not read: `fn inputs(&self, inputs: &mut Inputs)` declares components of other entities by uid (`inputs.component::<Spline>(uid)`), and `generate` reads them with `ctx.component::<Spline>(uid)`. The runtime regenerates the object when one changes. `Sweep` works this way.
- **Runtime state** (caches, timers) never goes in the generator: the runtime keeps its own (`ProcState`).

Register it in your plugin's `register` (shown with the rule [below](#rules-in-rust)): `registry.register_generator::<Cairn>()`. That is all; it then regenerates in the editor, in Play, in the player and in tests, and appears in the Add menu.

### Golden fixtures

A golden test pins each generator's output to its `VERSION`, so a change that would silently alter saved worlds fails:

```rust
// Re-record after a VERSION bump:
// BOX_UPDATE_GOLDEN=1 cargo test -p guide_native --test procedural
#[test]
fn cairn_golden_hashes() {
    assert_golden::<Cairn>(
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden"),
        &[
            GoldenCase::new("default", Cairn::default(), 0),
            GoldenCase::new(
                "tall",
                Cairn {
                    stones: 12,
                    ..Cairn::default()
                },
                3,
            ),
        ],
    );
}
```

The fixture `tests/golden/Cairn.golden.json` stores each case's parameters, seed and a hash of the quantized mesh. If the output changes while `VERSION` stays the same, the test fails even with `BOX_UPDATE_GOLDEN=1`: bump `VERSION`, then re-record and review. A new case or a new `VERSION` fails until re-recorded.

## Rules in Rust

A Rust rule is a plain function from `&RuleInput<Intent>` to `RuleOutput`, where the intent is any component of yours. The sample intent `CairnRow { count, spacing, size }` asks for a row of cairns along the entity's +X axis:

```rust
/// The `cairn_rows` rule: a pure function from every `CairnRow` to parts.
/// Each part is a `Cairn` with its own seed, keyed `cairn<i>`.
pub fn cairn_rows(input: &RuleInput<CairnRow>) -> RuleOutput {
    let mut out = RuleOutput::default();
    for row in input.intents {
        let r = &row.value;
        if r.spacing < r.size {
            out.warn(
                row.uid,
                "the cairns overlap: spacing is less than their size",
            );
        }
        for i in 0..r.count.min(100) {
            let cairn = Cairn {
                size: r.size,
                ..Cairn::default()
            };
            out.push(
                Part::new(row.key(format!("cairn{i}")))
                    .at(Transform::from_xyz(i as f32 * r.spacing, 0.0, 0.0))
                    .with(cairn)
                    .with(Procedural::with_seed(u64::from(i)))
                    .with(StandardMaterial::color(Color::srgb(0.55, 0.53, 0.5))),
            );
        }
    }
    out
}
```

- `input.intents` holds every `CairnRow` with its `entity`, `uid`, `value` and world `transform`.
- `row.key("cairn0")` makes the key `<uid>/cairn0`. `Part::new(key)` takes `.at(transform)` (relative to the owner), `.with(component)` (any component that is `PartialEq + Clone`; compare parameters only, never runtime caches) and `.warn(message)`.
- `out.warn(uid, message)` adds a warning to the owner.

Register the intent and the generator in `register`, and add the rule in `build`:

```rust
impl Plugin for ProceduralSamplesPlugin {
    fn register(&self, registry: &mut Registry) {
        registry.register_all::<(CairnRow,)>();
        // Errors are kept and logged by `Game` with the plugin's name.
        let _ = registry.register_generator::<Cairn>();
    }

    fn build(&self, app: &mut App) {
        app.add_rule("cairn_rows", cairn_rows);
    }
}
```

`add_rule` comes from `box_engine::box_proc::rule::RuleAppExt`; `RuleInput`, `RuleOutput` are in the same module and `Part` in `box_engine::box_proc::derive`. Rules added in `build` also run in the editor's edit world, so parts appear while you edit; a plain system that spawns entities would not. Rules run in `PostUpdate` in the system labelled `box_proc::rule::RULES_SYSTEM`.

`examples/guide_native/tests/procedural.rs` checks this rule: a row of three gives parts `cairn0` to `cairn2`, 1.5 m apart, each with its own seed and a mesh; changing the row to two cairns 0.4 m apart despawns `cairn2` and warns on the row.

More in Rust:

- **Reading another intent type:** `app.rule_reads::<WallPath>("fences")` after `add_rule` lets the `fences` rule read every `WallPath` with `input.read::<WallPath>()` (and re-run when one changes). `examples/walls` uses it so fences butt against walls.
- **A hand-written system** can call `box_proc::derive::reconcile(world, owner, rule, parts)` directly; it gets the same keys, overrides and warnings, but no change gate, and it does not run in the editor's edit world.
- **Neighbour facts are parameters.** Generators never look at their neighbours. A rule that sees every intent computes facts such as "this end meets that wall" and writes them into the parts' generator parameters. `examples/walls` (mitred and butted wall junctions, fences with shared corner posts) is the worked example.

## Terrain

`Terrain` is ground shaped by stamps: hills, ridges, basins, flats, walkable strips and noise, applied in order over an area centred on its entity. Add it with Add Component, or with a command. The drawn ground and its collision come from the same surface, so what looks walkable plays walkable.

| Field | What it does |
|---|---|
| `size`, `cell_size` | The area on X and Z, and the distance between height samples (smaller is smoother and heavier) |
| `base_height` | The height before any stamp |
| `stamps` | The shaping operations, in order (below) |
| `layers` | Colours of grass, dirt (under walkable strips) and rock (on slopes steeper than `rock_slope`), and a brightness `variation` |
| `skirt` | Depth of the wall hanging from the edges, so the world does not end in a paper-thin edge |
| `collide` | Give the ground colliders (on by default) |
| `show_walkability`, `walk_slope` | Tint ground steeper than `walk_slope` red and ground within 5° of it orange |

| Stamp | What it does |
|---|---|
| `Hill { center, radius, height }` | A round hill (a negative height makes a dip) |
| `Ridge { points, width, height }` | A ridge (or valley) along a line of points |
| `Basin { center, radius, depth }` | A hollow for a pond |
| `Flatten { center, radius, height, bank }` | A flat round pad at a height, blending into the ground over `bank` |
| `FlattenPath { points, width, bank }` | A walkable strip through points `[x, height, z]`: the points' heights are its side profile. Paths, ramps, the rise up to a shrine. Painted with the dirt layer |
| `Noise { seed, scale, amplitude, octaves }` | Rolling unevenness over everything |

For example, a valley with a hill, a pond and a path that rises 1.4 m to a flat shrine pad:

```json
{ "op": "spawn", "name": "Valley", "components": { "Terrain": {
  "size": [96, 96],
  "stamps": [
    { "Noise": { "seed": 7, "scale": 18, "amplitude": 0.6, "octaves": 3 } },
    { "Hill": { "center": [-30, -28], "radius": 22, "height": 9 } },
    { "Basin": { "center": [18, 6], "radius": 9, "depth": 1.6 } },
    { "Flatten": { "center": [0, -20], "radius": 6, "height": 1.4, "bank": 5 } },
    { "FlattenPath": { "points": [[0, 0.1, 30], [-2, 0.67, 2], [0, 1.4, -16]], "width": 3, "bank": 2 } }
  ] } } }
```

The ground is drawn and collided in chunks of 32 × 32 cells, rebuilt from the stamps: editing one stamp regenerates only the chunks it reaches. The collision of each chunk is a heightfield of exactly the drawn surface. Check routes with `box walk` (it reports steps, falls and where a character gets stuck) and turn on `show_walkability` to see slopes a character cannot climb.

### Things that follow the ground

Raise a hill and what stands on it rises with it:

- **`FollowGround { offset }`** on an entity keeps its origin on the ground plus `offset` (lamps, landmarks, a stair's foot, a shrine). Move it on X and Z freely; its height belongs to the ground. It follows in the editor and in Play.
- **`Sweep` with `follow_ground`** (and `ground_offset`) lays a path, kerb or rope on the ground: every sample's height comes from the terrain under it, at least once per terrain cell.
- **Rules and scripts** read the ground with `ground.height(x, z)` and `ground.at(x, z)` (height, normal, point; nil off the terrain). A rule that samples it re-runs when the terrain under its intents changes; with a `scope`, only for the intents in the changed area.

```lua
function Lamps.rule(intents, out)
    for _, it in intents do
        for i, p in it.points do
            local y = ground.height(p.x, p.z) or p.y
            out:part(it, `lamp{i}`, { Primitive = { shape = "Cylinder" } }, { position = vector.create(p.x, y + 1, p.z) })
        end
    end
end
```

Heights are those of the drawn surface, the one characters walk on, so whatever follows it meets the ground exactly.

To put something on the ground once, without it following later edits, use **Edit ▸ Snap to Ground** (End) on the selection: each selected entity moves straight down or up until the lowest point of its meshes touches the terrain; its rotation stays. Entities outside every terrain stay where they are. It is one undo step. Agents and command files use the `snap_to_ground` command, which places the entity's **origin**, `offset` metres above the ground (give half the height for a shape centred on its origin); `"align": true` also tilts its up axis to the slope:

```json
{ "op": "snap_to_ground", "entity": "Shrine", "offset": 0.0, "align": true }
```

A command list that snaps an entity off the terrain fails as a whole and changes nothing.

Not yet: sculpting brushes, painted layers and holes (the stamps are the way to shape it for now), and `Scatter` on a terrain. A model dragged from Assets onto a terrain already stands on it.

### Terrain paths

Use **Add ▸ Terrain Path** to create a `TerrainPath` with a `Spline`. Select it in the hierarchy and use **Edit Spline** to shape the lane. Set its full `width` in metres. In a scene with several terrains, set `terrain` to the target terrain entity.

Lanes on the same terrain merge where their footprints overlap, including crossings and shared endpoints. The engine fits the merged surface to the terrain triangles across the full width of the lane and colours it with `Terrain.layers.dirt`. Terrain still supplies collision. Use terrain stamps such as `FlattenPath` to grade a climb, and use `box walk` to check it.

Scene commands can place a path in the same way. This example uses the scene's sole terrain.

```json
{ "op": "spawn", "name": "Shrine Lane", "components": {
  "TerrainPath": { "width": 2.4 },
  "Spline": { "mode": "Polyline", "closed": false, "points": [
    { "position": [0, 0, 8] }, { "position": [2, 0, 3] }, { "position": [2, 0, -3] }
  ] }
} }
```

The lane's elevation comes from the terrain. It updates when you edit the spline or raise the ground. Keep the terrain upright. Paths are clipped to its edges and hidden while `show_walkability` is on so the slope tints stay visible. When migrating a lane made from a `Sweep`, retain its `Spline` and replace the `Sweep` with `TerrainPath`.

## Scatter

`Scatter` places many instances of meshes or prefabs over an area, deterministically from a seed. Add it to an entity with Add Component in the inspector.

| Field | What it does |
|---|---|
| `species` | What to place, each with a `source` (`Mesh { mesh, materials }` or `Prefab { scene }`), a relative `weight`, a `scale` range, `random_yaw`, `align_to_normal` (0 upright, 1 leaning with the ground), `tint_jitter` and `cast_shadows` |
| `density` | Instances per square metre where the area or paint is full |
| `min_distance` | No two instances closer than this |
| `max_slope`, `min_height`, `max_height` | Ground an instance may stand on |
| `area` | An outline on the owner's XZ plane (owner-local x, z); empty means only painted regions |
| `surface` | The entity whose mesh the instances stand on; none means the owner's XZ plane |
| `seed` | Another arrangement with the same settings |

For example, trees and bushes over a 20 m square (the model paths are placeholders for your own imported models):

```json
{
  "op": "add_component", "entity": "Ground", "component": "Scatter",
  "value": {
    "species": [
      { "source": { "Prefab": { "scene": "models/tree.glb#Scene" } }, "weight": 1.0, "scale": [0.8, 1.3] },
      { "source": { "Mesh": { "mesh": "models/bush.glb#Mesh/Bush", "materials": [] } }, "weight": 3.0 }
    ],
    "density": 0.5,
    "min_distance": 1.5,
    "area": [[-10.0, -10.0], [10.0, -10.0], [10.0, 10.0], [-10.0, 10.0]],
    "seed": 7
  }
}
```

**Paint Scatter** (toolbar **Paint**, with a scatter selected) paints where it places instances: drag to paint, Shift-drag or X to erase, `[` and `]` for the radius, `-` and `=` for the strength. A stroke is one undo step.

The instances are runtime-only, like rule parts: never saved, rebuilt from the settings. Placement works in 8 m chunks, so an edit, a brush stroke or a change to the ground re-places only the chunks it touches. Instances of a species share meshes and up to four tint variants, so they draw in instanced batches; tens of thousands are fine.

## Bake to Mesh

**Edit ▸ Bake to Mesh** freezes the selected procedural object:

- It writes the generated mesh to `assets/baked/<Name>.glb` (one glTF primitive per used material slot; an inline `StandardMaterial` becomes a glTF material).
- As one undo step, it replaces the generator (and its `Procedural`) with a `MeshAsset` drawing that file, with the same materials. Undo brings the generator back; the file stays.
- Any generator bakes, `Primitive` included. A baked primitive keeps its shadow setting, and a `Collider` that followed the primitive's shape keeps it.
- An inline `ShaderMaterial` cannot be written to glTF: those slots draw the default material, with a warning.

Bake when you want to hand-edit a shape in another tool, or stop it from changing when a generator's `VERSION` changes. The result is an ordinary model asset.

## Architecture: box_arch

`box_arch` (in `plugins/box_arch`) is an optional plugin of architectural generators, listed under **+ Add ▸ Architecture**:

| Type | What it makes |
|---|---|
| `Wall` | A solid wall along a polyline with arcs or a `Spline`, with explicit end junctions (`Cap`, `Open`, `Extend`, `Abut`, `Mitre`, `Butt`), openings (windows, doors, passages with flat, round or pointed heads) cut with reveals and sills, plinth, cornice and coping, and a top that can follow a roof's underside |
| `Roof` | Gable, hip and shed roofs over rectangles; cone and dome roofs over circles; slab, soffit, boards and ridge rolls |
| `Platform` | A slab from a footprint (rectangle, circle, polygon) with a lip or trim |
| `Stair` | A straight flight between two points, solid or open, with an optional landing; warns when the slope is impossible |
| `Anchor` | Keeps a decoration on a wall face, wall top, opening or roof ridge as the target changes |

All of them bake ambient occlusion into vertex colours (`ao` strength, 0 disables it) and emit UV set 0 in metres. `box_arch::junction` computes wall end parameters for mitres, butts and T junctions.

`box_arch` is generic: it has no rooms and decides nothing about which walls join or who owns a corner. Your game's rules decide that and write the results into the parts' parameters. The `walls` example and the `box_arch` gallery show the pattern.

It is not in the engine's default plugins, so it needs a native project. Add it to the game crate's `Cargo.toml`, under `[dependencies]` next to `box_engine`, from the same engine checkout:

```toml
box_arch = { path = "../box-engine/plugins/box_arch" }
```

Then add it to the game in `src/lib.rs`: `Game::new().with_plugin(box_arch::ArchPlugin).with_plugin(MyGamePlugin)`. Its generators then appear in your editor's Add menu, and script rules in that project can make them as parts (`out:part(it, "wall", { Wall = { ... } })`). To see what it can do:

```bash
cargo run -p box_arch --example arch_gallery -- --capture target/captures/arch/gallery.png
```

## Performance notes

- **Stamp, then flush.** Every frame each generator is compared with a cached copy of its parameters (cheap: about 2.75 µs per frame for 1,000 idle generators). Changes are queued; a flush regenerates them in `PostUpdate`.
- **Throttling.** The first change after a pause shows in the same frame. During a continuous edit (a slider drag) a flush runs at most every 30 ms, and the last value always lands. Each flush spends at most about 6 ms generating; objects over that budget wait for the next frame. Both are in the `ProcSettings` resource.
- **Cache keys.** A flush hashes the generator type, `VERSION`, the parameters, the seed and the declared inputs; an unchanged key is skipped, and an output equal to the current mesh is not uploaded again.
- **Rules are change-gated.** An idle rule costs one comparison per intent. A changed rule rewrites only the parts whose components differ, and only those regenerate.
- **Narrow script rule scopes** when you drag intents of a rule with hundreds of them (see [Scope](#scope-how-far-a-run-looks)).
- **Golden fixtures and `VERSION`.** Saved scenes store parameters, not meshes, so a generator change alters every saved world that uses it. Bump `VERSION` and re-record its golden fixtures, as [above](#golden-fixtures), so the change is deliberate and reviewed.
- **Collider hints:** a generator's `GenOutput` can carry a `ColliderHint` (cheap boxes standing in for the mesh). It is stored on the entity but not used by physics yet.

## See also

- [The editor](editor.md#procedural-tools): the procedural tools in the editor.
- [Scripting](scripting.md): Luau basics, properties, hot reload.
- [Native plugins](native-plugins.md): game crates, components, `Plugin::register` and `build`.
- [Script rule reference](../reference/rules.md): `RuleIntent`, `RuleOut`, `RuleTransform`, and every component a part can carry.
- [Component reference](../reference/components.md): every field of `Lathe`, `Extrude`, `Sweep`, `Spline`, `Procedural`, `Overrides`, `Scatter` and `ScriptIntent`.
- Examples: `examples/script_rules` (fence and colonnade rules, no Rust), `examples/walls` (neighbour-aware walls and fences in Rust) and `plugins/box_arch`.
