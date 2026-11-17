# Empty project

A blank box-engine project: one scene with a ground plane, a cube, a sun and a camera. Created with `box new <folder>` (or `--template empty`), or File ▸ New Project ▸ Empty in the editor.

## What is in it

| Path | What it is |
|---|---|
| `box.project.json` | The project's name and its start scene (the scene the player opens). |
| `assets/scenes/main.scene.json` | The start scene. Every scene, script, model, material and sound lives under `assets/`. |
| `.gitignore` | Leaves out `.box/`, where the editor keeps per-machine state (layout, recovery files, saves). |

## Next steps

```bash
box-editor .          # open it in the editor; + Add places objects, Play runs it
box run .             # play the start scene in a window
box render . --out shot.png
```

To add behaviour, write a Luau script under `assets/scripts/` and attach it with a **Script** component; the getting-started guide in the box-engine documentation (`docs/guide/getting-started.md`) walks through a first script. The `third-person` and `procedural` templates (`box new --list-templates`) are fuller starting points.
