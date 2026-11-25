# How box-engine fits together

A short map of the ideas behind box-engine: what a project is, what a scene holds, and the four ways to add gameplay. Read it once, then use the other guides as references.

## Contents

- [Projects are folders](#projects-are-folders)
- [Scenes, entities and components](#scenes-entities-and-components)
- [The hierarchy and transforms](#the-hierarchy-and-transforms)
- [Assets](#assets)
- [Prefabs: reusing objects](#prefabs-reusing-objects)
- [Four ways to add gameplay](#four-ways-to-add-gameplay)
- [Edit mode and Play mode](#edit-mode-and-play-mode)
- [Undo and saving](#undo-and-saving)
- [Settings and input](#settings-and-input)
- [Game state that lasts](#game-state-that-lasts)
- [Procedural objects](#procedural-objects)
- [Tools outside the editor](#tools-outside-the-editor)
- [Shipping a game](#shipping-a-game)

## Projects are folders

A project is an ordinary folder you can keep in Git:

```text
my-game/
├── box.project.json        name, start scene, export settings
├── assets/                 everything the game uses
│   ├── scenes/main.scene.json
│   ├── scripts/            .luau files
│   ├── models/             .glb / .gltf
│   ├── textures/, audio/, materials/, strings/, data/
│   └── settings/           input.actions.json, settings resources
├── AGENTS.md, CLAUDE.md    notes for AI agents working on the project
└── .box/                   editor state: layout, recent session, backups (not game content)
```

Every file under `assets/` gets a small `.meta` file beside it with a permanent id, so renaming or moving a file in the editor does not break the scenes that use it. Commit the `.meta` files with their assets.

All formats are open: JSON scenes, glTF models, PNG and JPEG images, OGG, WAV, MP3 and FLAC sounds, TTF and OTF fonts, WGSL shaders, Luau scripts.

## Scenes, entities and components

A **scene** (`*.scene.json`) is a level, a menu, or a reusable group of objects. It holds:

- **entities**: the things in the world. An entity has a name and a permanent id, and nothing else by itself.
- **components** on each entity: the data that makes it something. A `Transform` places it, a `Primitive` gives it a shape, a `StandardMaterial` colours it, a `Collider` makes it solid, a `PointLight` makes it glow, a `PlayerController` lets you steer it.
- **scene settings** (resources): the sky, fog and exposure (`Environment`), the rules of the game (`GameRules`), the HUD (`HudSettings`).

Select an entity and the Inspector lists its components; select the **Scene** row at the top of the Hierarchy and it lists the scene settings. **Add Component** adds more. Every field has a tooltip.

Units are metres, +Y is up, and −Z is forward.

## The hierarchy and transforms

Entities form a tree. Drag one onto another in the Hierarchy to make it a child: a child moves, turns and scales with its parent, and its `Transform` is relative to the parent. A character's model is a child of the character, a lamp's flame a child of the lamp.

## Assets

The **Assets** pane shows the `assets/` folder. Drag a model into the viewport to place it, a material onto an object to paint it, a file onto a matching field in the Inspector to use it. Files you drop in from Finder or Explorer are copied into the project.

Assets reload while you work: save a texture, a model or a script in another program and the editor picks it up.

## Prefabs: reusing objects

A **prefab** is a model or scene placed as one object (a `PrefabInstance`). A `.glb` brazier, or a `.scene.json` lantern you built from parts, can be placed many times; change the source file and every copy changes. Settings on the placed object itself (its position, an `Interactable` on it) belong to that copy.

## Four ways to add gameplay

From quickest to most powerful. Use the first one that can do the job; they work together in one project.

| Way | What it is | Good for | Guide |
|---|---|---|---|
| **Presets and behaviours** | Ready-made objects (Player, Pickup, Hazard, Moving Platform, Checkpoint, Goal) and components (`PlayerController`, `FollowCamera`, `Spinner`, `Bobber`, `Mover`), plus the scene's `GameRules` and `HudSettings` | A complete small game with no code | [Behaviours](behaviours.md), [Your first game](first-game.md) |
| **Interactions** | An `Interactable` on any object: when the player presses a key nearby or touches it, check conditions and run actions (show, hide, light up, count, win, respawn, play a sound) | Pickups, switches, doors, puzzles | [Interactions](interactions.md) |
| **Luau scripts** | A `Script` component running a `.luau` file: code that reacts to updates, input, collisions and events | Anything specific to your game | [Scripting](scripting.md) |
| **Rust plugins** | A native project: your own components and systems compiled into the engine | Large or performance-critical games | [Native plugins](native-plugins.md) |

The [Assistant](assistant.md) can do the first three for you from a description.

## Edit mode and Play mode

**Play** runs the game inside the editor on a copy of the scene. You can change things while it runs to try them out; when you press **Stop**, the copy is thrown away and the scene is back as it was. Tick **Keep after Stop** on a field to carry a tuned value back.

Scripts, physics, audio and the game's rules run only during Play. Edit mode shows the scene and keeps procedural objects up to date.

## Undo and saving

Every change in the editor is an undo step (Cmd+Z, Shift+Cmd+Z). The Edit menu shows what will be undone. A request to the Assistant is one step however much it changes.

The scene is saved with **Cmd+S**. If the editor stops unexpectedly, it offers to restore your unsaved work the next time you open the project.

## Settings and input

- **Input actions.** Games read named actions (`move`, `jump`, `interact`, `pause`), not keys. The defaults cover keyboard, mouse and gamepad; change them in `assets/settings/input.actions.json`.
- **Project settings.** Tunables shared by every scene, such as audio volumes, live in `assets/settings/` and appear under **Game ▸ Project Settings**. They reload when the file changes.

## Game state that lasts

- **Game variables** (`GameVars`) are named numbers, like `score`, `keys` or `gate_open`. Pickups and switches change them, conditions test them, the HUD shows them, and they survive a switch to the next scene.
- **Save games** store every saved object and the game variables. Scripts call `save.save("slot")` and `save.load("slot")`; checkpoints and pickups come back as they were.

## Procedural objects

Many objects are described by parameters rather than modelled by hand: a wall along a curve, a lathed column, a terrain with hills and paths, plants scattered by slope. Change a number or drag a point and the mesh rebuilds at once. See [Procedural creation](procedural.md).

## Tools outside the editor

- **`box`** is the command-line tool: create projects, check them, edit scenes with JSON commands, render images, run games, export. Every command takes `--json`.
- **`box mcp`** serves the same abilities to AI agents (MCP). With **File ▸ Allow Agent Control** on, an agent can also see and edit the open editor, play the game and take screenshots.

## Shipping a game

**File ▸ Export** (or `box export`) builds a standalone game: a macOS app, or a Windows folder. Players need nothing else installed. See [Exporting games](../EXPORT.md).
