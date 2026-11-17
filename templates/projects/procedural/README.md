# Procedural scene

A small garden in which almost every object is generated from parameters instead of modelled: change a number in the inspector and the geometry follows at once, in the editor and in the game. Created with `box new <folder> --template procedural`, or File ▸ New Project ▸ Procedural scene in the editor.

## Open it

```bash
box-editor .                 # select an object and change its fields in the inspector
box render . --out shot.png  # the scene as a PNG, without a window
box run .                    # play it
```

## What is in the scene

| Entity | Components | What it shows |
|---|---|---|
| **Column 1**, **Column 2** | `Lathe` (Column preset) | A **generator**: a profile turned around the Y axis. Try the Vase or Dome preset, or fewer **Segments**. |
| **Vase** | `Lathe` (Vase preset) | The same generator with another preset. |
| **Planter** | `Extrude` (hexagon with an inset hole) | An outline pushed up into a solid, with a bevel on its edges. |
| **Star** | `Extrude` (Star outline) | Change **Points** or the radii. |
| **Garden Wall** | `Spline` + `Sweep` (Rect profile, Up orientation) | A **spline sweep**: a rectangle swept along a curve, kept upright. Select it and use the **Path** tool to drag, add or delete points. |
| **Arch** | `Spline` + `Sweep` (Circle profile) | A tube swept along a curve without twisting. |
| **Garden Path** | `ScriptIntent` (`rules/garden_path.luau`) | A **script rule**: a Luau file turns the intent's points and properties into parts (gravel, stepping stones, lanterns). Edit its **Width**, **Spacing** or **Lanterns**, drag its points with the Path tool, or edit the rule file while the editor is open. View ▸ Rules lists the project's rules. |
| **Ground** | `Primitive` (plane) + `Scatter` | **Scatter**: shrubs and rocks from `models/garden.glb`, placed over a 24 m square, deterministically from **Seed**. Change **Density**, **Min Distance** or **Seed**, or use **Paint** in the toolbar to paint where they grow. |

Generated geometry (generator meshes, rule parts, scattered instances) is never saved: the scene stores only the parameters, so it stays small and every change is one undo step. **Edit ▸ Bake to Mesh** freezes a generated object into a plain mesh when you want to model it by hand.

| File | What it is |
|---|---|
| `assets/scenes/main.scene.json` | The scene. |
| `assets/rules/garden_path.luau` | The script rule, commented line by line. |
| `assets/models/garden.glb` | The scattered shrub and rock meshes and their materials. |

The procedural guide (`docs/guide/procedural.md` in box-engine) covers every generator, splines, sweeps, rules and scatter; `examples/script_rules` has two more rules (fences with shared corner posts, a colonnade).
