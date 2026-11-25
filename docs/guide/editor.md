# The editor

The box-engine editor (`box-editor`) is where you build scenes: place objects, tune their components, import assets, shape procedural geometry and press Play to try the game without leaving the window. Everything you do in it is an ordinary scene edit with undo, saved as JSON under your project's `assets/` folder, so the same scenes also work with the `box` CLI, scripts and agents. This manual is task-oriented: find the thing you want to do in the contents and read that section. If you have not made a project yet, start with [Getting started](getting-started.md).

## Contents

- [Opening the editor](#opening-the-editor)
- [The window](#the-window)
- [Moving around the viewport](#moving-around-the-viewport)
- [Selecting objects](#selecting-objects)
- [Moving, rotating and scaling](#moving-rotating-and-scaling)
- [Adding objects](#adding-objects)
- [The hierarchy](#the-hierarchy)
- [Editing components in the inspector](#editing-components-in-the-inspector)
- [Hiding and locking](#hiding-and-locking)
- [Undo and redo](#undo-and-redo)
- [Saving scenes](#saving-scenes)
- [Play mode](#play-mode)
- [Sound: the Mixer and audio ranges](#sound-the-mixer-and-audio-ranges)
- [Assets](#assets)
- [Editing scripts](#editing-scripts)
- [Procedural tools](#procedural-tools)
- [Project Settings](#project-settings)
- [The command palette](#the-command-palette)
- [Keyboard shortcuts](#keyboard-shortcuts)
- [The Console, toasts and errors](#the-console-toasts-and-errors)
- [Rebuild and Restart (native projects)](#rebuild-and-restart-native-projects)
- [The Assistant](#the-assistant)
- [Allow Agent Control](#allow-agent-control)
- [Editor files](#editor-files)

On macOS the shortcuts below use Cmd and Alt (Option). On Windows and Linux, Cmd is Ctrl.

## Opening the editor

Make a project and open it:

```bash
box new my-game
box-editor my-game
```

From a source checkout of the engine, without installing anything:

```bash
cargo run -p box_cli -- new my-game
cargo run -p box_editor_app -- my-game
```

`box-editor` takes the project folder (the one holding `box.project.json`, or any folder inside it). Without one it opens the project the working folder is in, else the last project you had open, else a sample scene under a **Welcome** card with New Project…, Open Project…, the example projects, your recent projects and "Explore the sample scene". Help ▸ Welcome Screen shows the card again.

**New Project…** shows the project templates with a preview each: **Empty**, **Third-person game**, **Procedural scene** and **Native (Rust)**. Pick one, choose a folder (created if missing; its name becomes the project name) and press Create: the project opens, and its `README.md` explains what is in it. A native project is a Rust crate, so the dialog shows the `box new --native` command to run in a terminal instead (Copy Command). **File ▸ Open Example…** (or an example on the Welcome card) copies one of the example projects (Collector, Cinder Keep, Script Rules, Script Demo, Guide Samples) into a new or empty folder you choose and opens the copy; the example itself is never changed.

A project reopens where you left it: the scene, the camera and the selection. Pass `--no-restore` to start fresh. If the project or its start scene cannot be opened, the editor still starts and shows a dialog with the cause.

All options:

```bash
box-editor --help
```

| Option | What it does |
|---|---|
| `--play` | Start in Play mode |
| `--control` | Allow Agent Control from the start (needs a project) |
| `--no-restore` | Do not reopen the last project or session |
| `--capture <png>` | Render the whole editor headlessly to a PNG and exit (2560x1440 at 2x) |
| `--restore` | With `--capture`: restore the project's last session |
| `--size <W>x<H>` | Window size in points, or capture size in pixels |
| `--frames <n>` | Frames before capturing; in a window, quit after n frames |
| `--write-registry` | Write the game's types to `<project>/.box/registry.json` and exit |

A headless capture is handy for bug reports and for checking a scene from a script:

```bash
box-editor my-game --capture editor.png
box-editor my-game --play --capture editor-play.png
```

## The window

```
+------------------------------------------------------------------------+
| Menus  + Add  Tools  World  Snap      > || >|        my-game / main    |
+------------+--------------------------------------+--------------------+
| Hierarchy  | Viewport                             | Inspector          |
|            |                                      |                    |
+------------+--------------------------------------+                    |
| Assets | Console                                  |                    |
+------------------------------------------------------------------------+
```

- **Toolbar** (one row): the menus File, Edit, View, Game and Help; **+ Add**; the tool group (Move, Rotate, Scale, then viewport tools such as Place, Spline, Paint, and Path when the project has script rules); **World**/**Local** gizmo orientation; the snap toggle and its step; **Pivot** (only with two or more selected); Play, Pause and Step in the middle; and on the right the status: Agent (when agent control is on), Playing or Paused, `project / scene` (a dot `•` means unsaved changes), draw calls and fps. Every button has a tooltip with its shortcut.
- **Hierarchy** (left): the scene's entities.
- **Viewport** (centre): the 3D view.
- **Inspector** (right): the components of the selection, the scene's settings, an asset, or a project settings file.
- **Assets** and **Console** (bottom, tabbed): the asset browser and the log. Assets is in front when a project is open.

**Rearranging panes.** Every pane is a tab: drag a tab to dock it somewhere else, or drag the dividers. The layout is saved per project in `.box/layout.json` when you quit, or right away with View ▸ Save Layout. View ▸ Reset Layout restores the default. The command palette has Show Viewport, Show Hierarchy, Show Inspector, Show Console and Show Assets to bring a tab to the front.

## Moving around the viewport

| Do this | How |
|---|---|
| Orbit | Alt + left drag, or two-finger scroll |
| Pan | Middle drag, or Shift + two-finger scroll |
| Zoom | Mouse wheel (toward the cursor), Alt + right drag, or pinch |
| Fly | Hold the right button: move the mouse to look, W A S D to move, Q / E down and up, Shift faster, wheel sets the speed |
| Frame the selection | F (frames the whole scene when nothing is selected); or double-click a hierarchy row's icon |
| Look along an axis | Click an axis of the axis gizmo (top right); click it again for the opposite side |
| Reset the camera | View ▸ Reset Camera |

While you hold the right button over the viewport, the single-key shortcuts (W, E, R, Y, F) are off, so W A S D fly. The fly speed (0.1 to 500 m/s) shows briefly when you change it with the wheel. A hint with the basic controls sits at the bottom of the viewport until you click its ×.

**View menu.** Click the pill at the top left of the viewport ("Perspective · Lit · Grid · Icons") for:

- **Grid** and **Icons** (lights, cameras and empties get a clickable icon).
- **Collider wireframes**: Selected, All or None.
- **Contact points**: where rigid bodies touch, while playing.
- **Particle preview**: Selected, All or None. Previewed emitters simulate while you edit, so you can tune them without Play. Nothing about the preview is saved.
- **View from**: Top, Bottom, Front, Back, Right, Left.

These view settings last for the session. The editor has a perspective, lit view only.

A selected camera shows its frustum, a light its direction, range or cone, a particle emitter its emission shape.

## Selecting objects

| Do this | How |
|---|---|
| Select | Click it in the viewport or the hierarchy |
| Add to the selection | Shift-click |
| Toggle one in or out | Cmd-click |
| Select everything in a rectangle | Left drag on empty space in the viewport (Shift adds, Cmd toggles) |
| Make another selected entity the primary | Shift-click it |
| Select a range of rows | Shift-click in the hierarchy (Cmd+Shift adds the range) |
| Select nothing | Escape, or Edit ▸ Select None |

The marquee selects everything whose bounds meet the rectangle, hidden behind other objects or not. Flat objects such as a ground plane are only selected when entirely inside it, so dragging across the floor does not grab it. Lights, cameras and empties are hit by their origin or icon. Escape during a marquee drag restores the previous selection.

The **primary** selection (outlined in the accent colour) is the one the gizmo and the inspector header follow. The viewport's bottom-left chip shows its name and how many are selected.

Clicking a part of a procedural object, a glTF model or a scatter selects the object that owns it. To look at one generated part, Cmd+Alt+click it: it is shown read-only in the inspector (see [Procedural tools](#procedural-tools)).

## Moving, rotating and scaling

Press **W** (Move), **E** (Rotate) or **R** (Scale), or click the tool in the toolbar, then drag a handle of the gizmo. Each drag is one undo step. While you drag, a readout next to the cursor shows the change (`X  Δ 1.50 m`, `Y  45.0°`, `×1.25`) and whether snapping is on.

- **World / Local**: the toolbar button switches the gizmo between world axes and the object's own axes. Scaling always uses local axes.
- **Several objects**: the gizmo moves, turns and scales all of them together. A child whose parent is also selected moves once, with the parent. With two or more selected, the **Pivot** button chooses where the gizmo sits and turns: Primary (the primary's origin) or Centre (the middle of the selection).
- **Snapping**: press **Y** or click the magnet button to turn it on or off. Click the step value next to it (for example `0.5 m`) to set the Move (m), Rotate (°) and Scale steps. Hold Cmd while dragging a gizmo to invert snapping for that drag. Steps and the on/off state are kept per project.
- **Place on Surface**: choose **Place** in the toolbar, then click a surface: the selection drops onto it, turned to the surface (hold Shift to keep its rotation). Esc or Enter returns to Move.
- **Cancel a drag**: press Escape before releasing; the values go back.

You can also type exact values in the inspector's Transform section.

## Adding objects

Click **+ Add** in the toolbar (also Edit ▸ Add, or right-click empty space in the hierarchy). The menu has:

| Group | Entries |
|---|---|
| Meshes | Cube, Sphere, Cylinder, Capsule, Plane |
| Procedural | One submenu per generator category: **Shapes** (Extrude, Lathe, Spline) and any added by your game or plugins (for example **Architecture**) |
| Gameplay | Ready-made objects that work without code: Player (with a follow camera), Pickup, Hazard, Moving Platform, Checkpoint, Goal ([Behaviours](behaviours.md)) |
| Empty | Empty (a transform only, for grouping) |
| Lights | Directional Light, Point Light, Spot Light, Ambient Volume |
| Other | Camera, Particle Emitter |

A new object appears where the centre of the viewport looks: resting on the surface there (or on the ground plane), moved sideways so it does not overlap another object. A point light goes 2.5 m above the surface, a spot light 3 m above it pointing down. The new object is selected, and adding it is one undo step.

To add a child, right-click a hierarchy row and use **Add Child**; children start at the parent's origin.

To add a model or prefab, drag it from the Assets pane into the viewport (see [Assets](#assets)).

Cmd+D duplicates the selection; Delete or Backspace deletes it.

## The hierarchy

- The pinned **Scene** row at the top selects nothing and shows the scene's settings in the inspector: the Environment (sky, fog, exposure, tone mapping, post-processing, wind), and the game's rules and HUD when the scene has them. **Add Scene Setting** at the bottom adds Game Rules or Hud Settings ([Behaviours](behaviours.md)). View ▸ Scene Settings does the same.
- **Rename**: double-click the name, press F2, or right-click ▸ Rename.
- **Reparent**: drag a row onto the middle of another row. Drag onto a row's top or bottom edge to reorder instead. The object keeps its position in the world. Dragging near the list's top or bottom edge scrolls it.
- **Search**: the field at the top filters the rows.
- **Right-click a row** for Add Child, Rename, Hide in Viewport / Show in Viewport, Lock in Viewport / Unlock, Duplicate and Delete.
- **Marks**: a gear marks an object that generates parts (hover for the count); a warning mark means an asset it uses failed to load or a rule reported a problem (hover for the reason).
- Drop a material from the Assets pane onto a row to assign it.

## Editing components in the inspector

Select an entity and the inspector lists its components as sections (Transform, Primitive, Standard Material, and so on). The entity's name is at the top; click it to rename.

- **Change a value**: drag a number field sideways, or click it and type. Fields with a range have a slider; colours open a picker; asset fields have a picker, a clear button and accept a dragged asset. Each drag or typed edit is one undo step. Hover a label for its description.
- **Folded groups**: nested settings (Bloom, Wind, the items of a list) are headings that open on click. A check box on a heading is that group's on/off switch.
- **Lists** show "n items" with **+** (adds a copy of the last item) and **−** (removes the last one).
- **Changed values** have a dot before their label. Hover the row and click ↺ to reset it.
- **Right-click a field's label** for Reset to Default, Copy Value and Paste Value. Paste only accepts a value of the same type. The clipboard is plain JSON, so values paste between editor windows.
- **The ⋯ menu of a section** has Reset, Copy and Paste for the whole component, and Remove.
- **Add Component** is pinned at the bottom of the inspector: type to search, Enter adds the first match. Components are grouped as Rendering, Lights, Physics, Assets & Prefabs, Gameplay (your game's own types) and Other. After copying a component, "Paste <name> as New Component" adds it to an entity that lacks it.
- **Scripts**: a Script component shows the properties the `.luau` file declares, with their defaults. A property you have not changed is labelled "default"; ↺ removes your override. See [Scripting](scripting.md).
- **Linked materials**: a Material Ref section has **Edit** (show the material file in the inspector) and **Unlink** (copy its values into the entity's own material).

**Several entities selected**: the inspector shows the components they all have, and the header shows how many are selected. A value that differs between them shows "—". Editing a field sets it on all of them as one undo step; for a vector, only the lane you edit. Add Component adds to every entity that lacks it, Remove removes from all.

**Unknown components**: a component that this editor does not know (for example from a native game opened in the plain `box-editor`) shows as a collapsed, read-only section "… — not registered (data kept)". Its data is kept and saved back unchanged. Open the project with the game's own editor to edit it.

## Hiding and locking

Hiding and locking help you work in a busy scene. They are editor-only: they are not saved in the scene, are not undo steps, do not affect Play, and last for the session.

- **Hide**: H hides the selection in the viewport (H again shows it). Alt+H (Edit ▸ Show All Hidden) shows everything. Each hierarchy row also has an eye on hover. Hiding affects meshes only: a hidden light still lights.
- **Lock**: click the lock on a hierarchy row, or right-click ▸ Lock in Viewport. A locked entity cannot be clicked, marquee-selected or moved in the viewport; clicks go through it. You can still select it in the hierarchy.

Children follow their parent; a dimmed eye or lock means it is inherited.

## Undo and redo

- **Undo** Cmd+Z, **Redo** Cmd+Shift+Z, also in the Edit menu, which names the step ("Undo Cube: Translation").
- **Edit ▸ History** lists every step, most recent first; click one to go back (or forward) to it.
- A drag (gizmo, slider, colour picker) is one step. Undo restores the selection of that moment too.
- While a material file is shown in the inspector, undo and redo step that file's own history, not the scene's.
- Undo is off during Play; the undo history survives Play.
- Opening another scene clears the history.

## Saving scenes

| Do this | How |
|---|---|
| Save | Cmd+S (File ▸ Save Scene) |
| Save under a new name | Cmd+Shift+S (File ▸ Save Scene As…) |
| New scene | File ▸ New Scene: a sun, ground and camera, unsaved; Save asks where |
| Open another scene | File ▸ Open Scene ▸ (every `*.scene.json` under `assets/`), or double-click it in Assets |
| Another project | File ▸ New Project…, Open Project…, Open Example…, Open Recent |

Scenes are saved as `.scene.json` files under `assets/` (the start scene of a new project is `assets/scenes/main.scene.json`). Writes are atomic.

**Unsaved changes.** Opening another scene or project, New Scene, New Project and Quit ask **Save / Don't Save / Cancel** when the scene has unsaved changes. Undoing back to the saved state counts as saved.

**Recovery.** While a scene has unsaved changes, the editor writes a recovery copy under `<project>/.box/recovery/` (at most every 30 s, and before Play). If the editor stops without saving, the next time you open that scene it offers **Restore** (load the changes, still unsaved) or **Discard**.

## Play mode

Press **Play** (Cmd+P) to run the game inside the editor, from the scene as it is now. Press it again (Stop, Cmd+P) to return to editing. Your scene comes back exactly as it was.

| Control | Shortcut | What it does |
|---|---|---|
| Play / Stop | Cmd+P | Start, or stop and return to the edit scene |
| Pause / Resume | Cmd+Shift+P | Freeze time; the inspector still shows live values |
| Step | Cmd+Alt+P | Run one frame while paused |
| Free Camera | (toolbar, during Play) | Look at the running game through the editor camera |

**While playing:**

- The viewport shows the game's camera (the first active Camera), with a coloured border: accent while running, amber while paused. Free Camera brings back the editor camera and its overlays.
- Every pane shows the **play world**. You can select and edit things to experiment, but those edits are discarded on Stop. The inspector says so in a banner: "Playing — changes are discarded when you stop".
- **Keep a change**: right-click a field's label (or open a section's ⋯ menu) and tick **Keep after Stop**. Kept rows get an accent bar; on Stop their values are applied to the edit scene as one undo step, "Apply Play changes". Objects created during Play cannot be kept.
- Undo, Redo, History and Save are off. Opening another scene or project, or quitting, stops Play first.
- Project Settings edits apply to the running game at once and are kept (they are written to their file).
- Asset and script files hot-reload into the running game.

**Game input.** When Play starts, the viewport takes keyboard, mouse and gamepad input for the game, and the viewport tab bar reads "Playing · Esc gives input back to the editor". Escape, a click elsewhere, Pause or switching to another window gives input back to the editor ("Click to give the game input"); click the viewport to hand it back to the game. While the game has input, editor shortcuts other than Play, Pause and Step are off. If the game locks the mouse cursor, the editor hides and captures it until you press Escape.

**When the game fails.** If Play cannot start, or game code panics during Play, Play stops with an error toast and a Console record naming the phase, the location and the message. The edit scene and its recovery copy are untouched. Script errors are caught and logged with `file:line`; they cannot crash the editor.

## Sound: the Mixer and audio ranges

**View ▸ Mixer** shows the game's sound while it plays. Use it to tune a mix: play the same stretch again and again, and change levels while you listen.

- **Bus faders**: Master, Effects, Ambience and Music are the project's Audio Settings. Moving a fader changes the running game at once and writes `assets/settings/AudioSettings.json`, so the change stays after Stop. Each bus shows the combined level of its sounds.
- **Sources**: during Play, every sound that is playing, loudest first. Each row shows its bus, its distance from the listener, how much the distance turns it down (Atten.), where it is heard from (Pan, L or R), and a meter of how loud it is at the listener after distance, panning, bus and master volume. A one-shot (`PlaySound`) is named by its clip.
- **Volume**: an `AudioSource`'s row has a volume slider. It is an ordinary Play edit, so the inspector shows it too. Press **Keep** to apply the value to the scene at Stop.
- **Solo and mute**: **M** mutes a row and **S** solos it. While anything is soloed, only soloed sounds are heard. A bus row mutes or solos the whole bus. **Solo selected** and **Mute selected** apply to the selected entities and every sound below them in the hierarchy, such as an Orchard group with a source on each tree. A muted sound keeps playing silently, and its meter still shows its level. Solo and mute never change the scene, and they end at Stop.

Editing `AudioSettings.json` in another program, or a source's Volume in the inspector, also changes the running game at once.

**View ▸ Audio Ranges** (also in the viewport's view menu) draws two rings around each positional sound: inside the inner ring it plays at full volume, beyond the outer ring it is silent. During Play, each playing sound also gets a line to the listener (the `AudioListener` entity, else the game camera), coloured by its level: green, yellow above -12 dB, red above -3 dB. The line is dark grey when the sound is silent, out of range, muted or soloed out. The rings show over the game's own view too.

**View ▸ Navigation** draws the navmesh: where enemies and other characters the game moves can walk. Bright lines outline each walkable area (they keep a character's radius from walls and edges); faint lines are the edges between its pieces. It is built from the scene's colliders and follows your edits a moment after you stop moving something. Moving things (platforms with a `Mover`, characters, rigid bodies) and triggers are not part of it. `NavSettings` in Scene Settings sets the agent size; `box nav` and `box render --nav` show the same from the command line.

Selected behaviours draw their gizmos: an `Interactable`'s reach (green ring and cross at the interaction point), a `Mover`'s path (yellow), a `Spawner`'s area (violet), and a `Chaser`'s sight (orange ring), give-up distance (faint orange) and patrol (yellow).

Levels are measured on the sound output. When no sound device can be opened, the output is silent, and the Mixer shows no levels.

**For agents.** The same information and controls are live MCP tools: `audio_levels` (every voice's level, distance, attenuation and pan as JSON; `target` filters to an entity and the sources below it, a bus or a clip), `audio_solo` and `audio_mute` (`{"target": "Orchard"}`, `{"target": "bus:music"}`, `{"clear": true}`). To check a mix without a window, `box run` measures it over a headless run:

```bash
box run my-game --frames 600 --audio-report audio.json
```

The report lists each source's RMS and peak level in dBFS, how long it was heard, and how long it was muted, together with the mixed output and the buses. Headless runs mix the sound in memory, so nothing is heard. `box-player --audio-report` does the same.

## Assets

The Assets pane shows the project's `assets/` folder: a folder tree on the left, the files of the selected folder on the right, a breadcrumb, **Search assets** (across all folders) and **New** (Folder, Material, Scene).

**Importing.** Drop files from your file manager onto the editor window: they are copied into the folder shown (originals stay where they were). You can also copy files into `assets/` with any tool; the editor notices new and changed files. Supported: glTF/GLB models, PNG and JPEG images, `.material.json` materials, `.scene.json` scenes, `.wgsl` shaders, `.luau` scripts, audio (OGG, MP3, WAV, FLAC), fonts and JSON data. FBX is not supported; convert it to glTF.

**`.meta` files.** Every asset gets a small `<file>.meta` next to it holding its id and import settings. References in scenes use the id, so you can move and rename assets freely. Commit `.meta` files with the assets. Rename and move inside the editor (or move the `.meta` along with the file) so the id stays the same.

**Working with files:**

- Images show a thumbnail; other files a type badge. Amber means importing, a red "!" means the import failed (hover for the error).
- Double-click a folder to open it, a scene to open it, a glTF to see its meshes, materials and textures.
- Rename with F2, a double-click on the name, or right-click ▸ Rename. Move a file by dragging it onto a folder.
- Right-click ▸ Reimport, or ▸ Move to Trash… (also Delete). Trashed files go to `<project>/.box/trash/` with their `.meta`; move them back to restore the asset with its id.

**Using assets in the scene:**

- Drag a glTF model or a `.scene.json` into the viewport: it becomes a prefab instance standing on the surface under the pointer (a terrain, a floor, another object; the ground plane where there is nothing): the lowest point of its meshes touches the surface, whether or not the model's origin is at its base. Drag one of its meshes to get just that mesh. To put something back on the terrain later, select it and press End (Edit ▸ Snap to Ground).
- Drag a material onto an object in the viewport or the hierarchy to assign it.
- Drag any asset onto a matching field in the inspector.

**The asset inspector.** Click an asset to show it in the inspector:

- Images and models show their **Import Settings** (images: Color space, Mipmaps; models: Scale, Tangents). **Apply** writes them to the `.meta` and re-imports; **Revert** drops your edits. A glTF's `colliders` setting (static mesh colliders for level geometry) is set in its `.meta` file.
- A `.material.json` is edited like a component and saved to its file as you edit, with its own undo history and **Revert**. Every object using it updates at once. Materials inside a glTF are read-only.
- A shader material shows the parameters its `.wgsl` file declares, and the shader's compile error if it has one.

**Hot reload.** Change an asset file outside the editor (save a texture in your paint program, edit a script) and the editor re-imports it within a moment, in the edit scene and during Play.

## Editing scripts

Luau scripts open in the **Script** tab, next to the Viewport:

- Double-click a `.luau` file in the Assets pane (or right-click ▸ Edit Script).
- Press **Edit** on a Script component in the inspector.
- Double-click a script error in the Console, or press **Edit Script** on its toast: the file opens at that line.

Each open file is a tab at the top of the pane; a dot after the name means unsaved edits. **Cmd+S** saves the file while you are typing in it (elsewhere it saves the scene). Saving reloads every script that uses the file at once, during Play too. **Cmd+F** finds text; Enter jumps to the next match.

Lines named by script errors and warnings in the log are red in the margin; hover the number to read the message. If a file changes on disk (another program, the Assistant), it reloads; if you have unsaved edits, a bar offers **Reload** or **Keep Mine**. Unsaved scripts are part of the "Save changes?" prompt when you quit or open another project.

**Open in Code Editor** (in the Script tab, the inspector and the Assets right-click menu) opens the file in VS Code, at the cursor's line, when it is installed, otherwise in your default app. For completion and type checking there, see [Set up your code editor](getting-started.md#set-up-your-code-editor).

## Procedural tools

Procedural objects store their parameters, not their mesh: change a value and the shape rebuilds. This section covers the editor side; [Procedural modelling](procedural.md) explains generators, rules and scatter in depth.

- **Generators**: Add ▸ Shapes ▸ Extrude, Lathe or Spline (a tube along an editable curve). Their parameters are in the inspector; material slots (for example Sides, Caps) are set per slot.
- **Edit Spline**: select a spline (or a tube that follows one) and choose **Spline** in the toolbar. Drag points and their handles (Shift: up and down only), Cmd-click the curve to insert a point, Delete or Backspace removes the selected point. Enter or Escape finishes. Each drag, insert or delete is one undo step.
- **Draw Path** (toolbar **Path**, shown when the project has script rules): click to place points on surfaces (they snap to other paths), Enter or a click on the last point finishes, a click on the first point closes the path, Backspace removes the last point, Esc cancels. Select an existing path first: the new path copies its rule and settings. With a path selected, drag its points to move them.
- **Paint Scatter** (toolbar **Paint**): select an entity with a Scatter component and drag to paint where it places instances. Shift-drag or X erases, `[` and `]` change the radius, `-` and `=` the strength. One stroke is one undo step.
- **Generated parts**: rules generate parts (fence posts, columns) for an object. A click on a part selects its owner. Cmd+Alt+click inspects the part read-only, with "Generated by" (rule and owner). Editing one of its fields saves an **override** on the owner, marked like a changed value with ↺ to reset it. Edit ▸ Hide Generated Part removes the inspected part. The owner's inspector lists its parts, overrides, hidden parts and rule warnings.
- **View ▸ Rules** opens the rule list: each rule's intents, parts, last run time and warnings, with a switch to turn a rule off for this session (nothing is saved).
- **Bake to Mesh** (Edit menu): writes the selected procedural object's mesh to a `.glb` under `assets/baked/` and replaces the generator with that mesh. Undo brings the generator back (the file stays).

## Project Settings

Game ▸ Project Settings lists the project's settings resources: values shared by every scene, such as Audio Settings and Physics Settings, plus any your game registers. Choosing one shows it in the inspector.

- Each one is a file, `assets/settings/<Type>.json`. The editor creates missing ones with the defaults when it opens a project; commit them.
- Edits are written to the file as you go. They have no undo and do not mark the scene unsaved.
- During Play they apply to the running game immediately and are kept after Stop.
- Editing the file outside the editor hot-reloads it.

## The command palette

Press **Cmd+K** (or F1, or Help ▸ Command Palette…) and type. The palette searches every menu action, plus Add ▸ (objects, generators and gameplay presets), Add Component (for the selection), Open Scene ▸ (the project's scenes), the project's assets (choosing one selects it in the Assets pane), Open Recent ▸ projects and the undo History.

- Matching is forgiving: each word can match part of a label or letters in order.
- Recently used actions come first.
- Rows show the shortcut, On/Off for toggles, or why an action is unavailable right now.
- Up and Down move, Enter runs, Esc closes.

Viewport tools are in it too ("Place on Surface Tool", "Edit Spline Tool", "Paint Scatter Tool", "Draw Path Tool"), as are actions that games and plugins add.

## Keyboard shortcuts

This is the editor's shortcut table (`crates/box_editor/src/shortcuts.rs`); Help ▸ Keyboard Shortcuts… shows the same list, with the viewport navigation. Single-key shortcuts work when no text field has focus.

| Group | Keys | Action |
|---|---|---|
| General | Cmd+K / F1 | Command palette: search every action, scene, asset and recent project |
| Tools | W | Move tool |
| Tools | E | Rotate tool |
| Tools | R | Scale tool |
| Tools | Enter | Finish a viewport tool (Edit Spline, Place): back to the transform tools |
| Tools | Y | Snapping on or off (hold Cmd/Ctrl while dragging a gizmo to invert it) |
| View | F | Frame the selection (the scene when nothing is selected) |
| Selection | F2 | Rename the selected entity, or the asset after a click in Assets |
| Selection | Delete / Backspace | Delete the selection; in Assets, move the selected file to the trash; in Edit Spline, the selected point |
| Selection | Cmd+D | Duplicate the selection |
| Selection | Escape | Select nothing; cancel a drag, marquee or asset drag |
| Selection | H | Hide the selection in the viewport, or show it when it is hidden |
| Selection | Alt+H | Show every hidden entity |
| Edit | Cmd+Z | Undo |
| Edit | Cmd+Shift+Z | Redo |
| Edit | Cmd+S | Save the scene |
| Edit | Cmd+Shift+S | Save the scene as a new file |
| Play | Cmd+P | Play, or Stop while playing |
| Play | Cmd+Shift+P | Pause or resume |
| Play | Cmd+Alt+P | Step one frame while paused |
| Game | Cmd+Shift+B | Rebuild the game's code and restart the editor (native projects) |
| Paint Scatter | [ | Smaller brush (Paint Scatter tool) |
| Paint Scatter | ] | Larger brush (Paint Scatter tool) |
| Paint Scatter | - | Weaker brush (Paint Scatter tool) |
| Paint Scatter | = | Stronger brush (Paint Scatter tool) |
| Paint Scatter | X | Switch between adding and erasing (Paint Scatter tool; Shift while dragging erases) |

Modifiers must match exactly: Cmd+Shift+S never triggers Cmd+S, and Shift+W does not select the Move tool. Delete and Duplicate act on entities only while the Assets pane is not the active pane; in the Assets pane, Delete moves the selected file to the trash (after asking).

## The Console, toasts and errors

Nothing fails silently in the editor.

- **Toasts** appear at the bottom right of the viewport for warnings and errors, and short notes for your actions ("Saved main.scene.json", "Undo: Delete Cube"). Repeats merge ("×3"). Hover a toast to keep it, × closes it, a click opens the Console at that record.
- **The Console** tab lists log records. The chips **Info**, **Warnings** and **Errors** show counts and filter by level; **Filter** searches messages and sources; **Clear** empties the list. Hover a row for the whole message. Click, Cmd-click and Shift-click select rows; Cmd+C or right-click ▸ Copy / Copy All copies them.
- **The Console tab badge** counts warnings (amber) and errors (red) logged since you last looked at the Console.
- Script errors show with `file:line`. Asset load failures mark the affected hierarchy rows and inspector sections.
- The editor also logs to the terminal it was started from; the `RUST_LOG` environment variable sets the level.

## Rebuild and Restart (native projects)

A native project (made with `box new my-game --native`) has its own Rust code and its own editor binary, started with `cargo editor` in the project folder. In that editor, **Game ▸ Rebuild and Restart** (Cmd+Shift+B) builds the game's code in the background (the toolbar shows "Building…", the output goes to the Console). Compiler errors appear as `file:line` toasts and the editor keeps running. When the build succeeds, the editor restarts into the new binary with the same scene, camera, selection and unsaved changes.

In the plain `box-editor` the item is disabled, and its tooltip says why. See [Native plugins](native-plugins.md).

## The Assistant

**View ▸ Assistant** (or Cmd+K, "assistant") opens a window where you describe what to build or change, such as "add eight coins and a goal that wins once they are all collected". An AI agent then does it in the open project while you watch, and the window lists what it did in plain words ("Placed pickup ×8", "Checked the project") and what it says.

- It uses **Claude Code**, installed and signed in on this computer (`claude` in a terminal). The editor stores no key and sends nothing until you press Send. The agent can only use the project's `box` tools: no shell, and files only under `assets/`.
- **One request is one undo step**, labelled "Assistant: …" (Cmd+Z undoes it). Your own edits while it works stay separate steps. **Undo Turn** under a request also puts back the files it wrote; it is available while that request's changes are the latest.
- **Stop** ends a request at once; what it already changed stays, as one step.
- Each message tells the agent the open scene, the selection and where the camera looks, so "put a platform here" and "make this bigger" work.
- The agent is told to build with [presets and behaviours](behaviours.md) first, so what it makes can be changed in the inspector.

If Claude Code is not installed or not signed in, the window says so.

## Allow Agent Control

**File ▸ Allow Agent Control** (or `box-editor my-game --control`) lets local tools such as `box mcp` query, edit, screenshot and play the running editor. It needs an open project. While it is on, the toolbar shows **Agent** (hover for the port and connections), and the editor writes `<project>/.box/control.json` (port and access token, readable only by you), which it removes when control is turned off, the project changes or the editor quits. It listens only on 127.0.0.1.

Agent edits made while editing are undo steps labelled "Agent edit" and mark the scene unsaved; edits made during Play are discarded on Stop like yours. Start the MCP server for an agent with:

```bash
box mcp --project my-game
```

## Editor files

| File | What it holds |
|---|---|
| `<project>/.box/layout.json` | The pane layout (saved on quit and by View ▸ Save Layout) |
| `<project>/.box/editor.json` | Snap steps and state, the dismissed navigation hint, and the last session (scene, camera, selection) |
| `<project>/.box/recovery/` | Recovery copies of unsaved scenes |
| `<project>/.box/trash/` | Asset files moved to the trash from the Assets pane |
| `<project>/.box/control.json` | Agent control port and token, while it is on |
| `recent.json` in the user config folder | Recent projects (at most 10; File ▸ Open Recent, Clear Recent Projects) |

The user config folder is `~/Library/Application Support/box-editor` on macOS, `%APPDATA%\box-editor` on Windows, and `$XDG_CONFIG_HOME/box-editor` (or `~/.config/box-editor`) on Linux; `BOX_EDITOR_CONFIG_DIR` overrides it. `.box/` is local to your machine and listed in the project's `.gitignore`; everything under `assets/`, including `.meta` and settings files, belongs in version control.

On macOS, an editor started from Finder writes its log to `~/Library/Logs/box-editor.log`.

## See also

- [Getting started](getting-started.md): install, first project, first game.
- [Scripting](scripting.md): Luau scripts, properties and the script API.
- [Native plugins](native-plugins.md): Rust game code and the native editor.
- [Procedural modelling](procedural.md): generators, rules, splines and scatter.
- [API reference](../reference/README.md).
