# Troubleshooting

Common failures, the message you see, what causes it and how to fix it. Messages follow one pattern: **what happened, where (file, line, entity), what to do**. In `--json` output every failure has a stable `code`; `box check` lists problems with the same codes. In the editor, warnings and errors appear as toasts (Copy copies the whole message; Show File reveals the file it names) and in the Console.

Paths below are examples.

## Projects and scenes

### Project folder does not exist

```
error: could not open the project: my-gam does not exist; check the path (was the project moved or renamed?), or create a project there with `box new my-gam`
```
`box --json` code: `not_found`.

**Cause.** A typo in the path, or the project was moved or renamed. The editor reopens the last project at start; if it moved, the Welcome card is shown with the error.
**Fix.** Pass the right folder, or open it with File ▸ Open Project. `box new <dir>` creates a project.

### Folder is not a project

```
no box.project.json in /Users/me/games or any parent directory; open the folder that holds box.project.json, or create a project with `box new <dir>`
```
Code: `not_a_project`.

**Fix.** Point at the folder containing `box.project.json`.

### Malformed box.project.json

```
my-game/box.project.json: invalid project file: trailing comma at line 5 column 1
```
Code: `project_invalid`.

**Fix.** Edit the JSON at that line (a text editor; JSON allows no trailing commas or comments).

### Start scene missing

```
the start scene `assets/scenes/main.scene.json` does not exist (set by `start_scene` in my-game/box.project.json); create that scene, or point `start_scene` at an existing one; available scenes: level (assets/scenes/level.scene.json)
```
Code: `start_scene_missing` (`box run`, `box check`).

**Cause.** The scene was renamed, moved or deleted outside the editor.
**Fix.** Change `start_scene` in `box.project.json`, or recreate the file. The editor opens the project with an empty scene; File ▸ Save Scene saves it under the expected name.

### Malformed scene JSON

```
could not load the scene my-game/assets/scenes/main.scene.json: invalid JSON: key must be a string at line 13 column 23
```
Code: `scene_parse`. Other scene codes: `scene_version` (saved by a newer engine), `scene_invalid` (a component value of the wrong type; `entity` and `component` say which).

**Fix.** Edit the file at that line and column, or restore it from version control. The editor opens an empty, unsaved scene and leaves the file untouched.

### Unknown component

```
scene `assets/scenes/main.scene.json` uses unknown component `Helth` on 1 entity (data kept); did you mean `Health`? If it is a game type, run the game's own player or editor (a native project's `cargo run --bin player` / `--bin editor`)
```
Code: `unknown_component` (warning, with `suggestion`).

**Cause.** A typo in a hand-edited scene, or a native project opened with the stock `box-editor`/`box-player`, which do not contain the game's types.
**Fix.** Correct the name, or run the project's own editor/player (`cargo editor`, `box run`). The data is kept and saved back unchanged.

## Assets

### Missing asset referenced by a scene

```
mesh models/tower.glb#Mesh/Roof: no such file in assets/ (moved, renamed or deleted?); fix the reference in the inspector or put the file back; drawing the placeholder
```
Code: `asset_not_found` (with `entity_name`, `component`). The hierarchy row gets a warning mark.

**Fix.** Put the file back, or pick another asset in the inspector. Moving files inside the editor's asset browser keeps references working (they hold ids).

### Broken glTF, PNG, JPEG, audio or font file

```
models/broken.glb: import failed: invalid glTF: could not completely read the object; the file is damaged, incomplete or not really glTF: export or download it again
audio/hit.wav: import failed: unexpected end of file; the file is damaged, incomplete or not really WAV: export or download it again
```
Code: `asset_import_failed`. Meshes draw a placeholder cube, textures a checker, sounds are silent.

**Cause.** A truncated download or export, or a file renamed to the wrong extension. glTF files using Draco or meshopt compression get their own message (re-export without compression).
**Fix.** Export or download again. Hot reload picks up the fixed file.

### Shader compile error

```
material shader shaders/water.wgsl failed to compile; drawing the error material
shader shaders/water.wgsl failed to parse (at shaders/water.wgsl:4):
error: no definition in scope for identifier: `undefined_thing`
```
Codes: `shader_invalid` (`box check` validates material shaders without a GPU), `shader_header` (a bad `@material` header).

**Fix.** Fix the line named. A shader that compiled before keeps its last good version while you edit.

### Script syntax or runtime error

```
scripts/door.luau:2: Expected '(' when parsing function, got 'update'; entities using it do not run it until it loads
scripts/door.luau:5: attempt to index nil with 'open'
  in on_update of "Door" (scripts/door.luau); this entity's script is disabled until the file is reloaded
```
Code: `asset_import_failed` for syntax errors in `box check`.

**Fix.** Fix the line; saving the file reloads the script and re-enables it.

## GPU, audio and system

### No GPU adapter

```
no GPU adapter found: box-engine needs a GPU with Metal (macOS 11 or later). Update the graphics driver; in a virtual machine or remote session, enable GPU acceleration
no GPU adapter found for WGPU_BACKEND=vulkan; box-engine needs Metal (macOS 11 or later). Unset WGPU_BACKEND to use the default backend
```
Code: `no_gpu` (`box render`).

**Fix.** Unset `WGPU_BACKEND`, update the driver, or enable GPU acceleration. On Windows box-engine needs Direct3D 12 or Vulkan.

### GPU device lost

```
the GPU device was lost (<driver reason>). The graphics driver reset or the GPU stopped responding; restart the app, and update the graphics driver if it happens again
the GPU device was lost (...); a crash report was written to ~/Library/Logs/box-engine/box-editor-crash-20261116-205122-4242.txt
```
The app stops drawing, writes a crash report and exits instead of hanging. The editor keeps unsaved scene changes in the recovery file first; it is offered at the next start.

### GPU out of memory

```
wgpu error: Out of Memory. The GPU ran out of memory: use a smaller window or render size, fewer or smaller textures and shadow maps, or close other apps that use the GPU
```

### No audio device

```
no audio output device could be opened (...); sound is off. Connect or enable an output device in the system sound settings and restart the game
```
The game runs silently.

### Save file corrupt

```
save slot `slot1`: not a save file: .box/saves/slot1.json: EOF while parsing an object at line 1 column 21 (the save file is damaged and there is no backup (.box/saves/slot1.json.bak); delete it to start over)
```
A damaged slot with a `.bak` loads the backup instead (with a warning). Saves of another game or a newer version are refused, never silently replaced.

### Permission denied or disk full

```
could not save the scene: writing assets/scenes/main.scene.tmp: Permission denied (os error 13); no permission to write here: check the file's and its folder's permissions (or whether another program locks it), or choose another location
... No space left on device (os error 28); the disk is full: free some space and try again
```
`box --json`: code `io` with a `hint`. Writes are atomic: the old file stays intact when a write fails.

### Two apps with live control on one project

```
another Editor (pid 4242) already serves live control for this project; `box mcp` now reaches this Player instead. Quit the other one, or start only one app with --control
```
The control socket always binds a free port on 127.0.0.1, so "port in use" cannot happen; the discovery file `.box/control.json` names one app at a time.

## Native projects

### The game's code does not compile (`box run`)

```
error[E0308]: mismatched types
 --> src/lib.rs:12:22
...
error[build_failed]: the game's code does not compile (1 error; first: src/lib.rs:12:22: mismatched types); fix it and run again
```
Code: `build_failed`; `problems` lists each compiler error as `compile_error` with `file`, `line`, `column`. A player that builds but then fails is `run_failed` with its exit code.

### Rebuild and Restart fails (editor)

```
Rebuild failed: 1 compiler error; the editor keeps running (Console has the output)
```
Each compiler error is a toast with `file:line:column` (Show File reveals it). The editor keeps running with the old code; fix and press ⇧⌘B again.

## Crashes

When an app panics outside the code paths that catch and report errors (Play-mode game code, generators, importers), it prints the usual panic message and then:

```
box-editor crashed. A crash report was written to:
  /Users/me/Library/Logs/box-engine/box-editor-crash-20261116-205122-4242.txt
It stays on this computer; attach it when you report the problem.
```

Reports are written to `~/Library/Logs/box-engine/` (macOS), `%LOCALAPPDATA%\box-engine\logs\` (Windows) or `~/.local/state/box-engine/logs/` (Linux); `BOX_CRASH_DIR` overrides the folder. A report holds the message and location, a backtrace, the engine version, OS, GPU adapter, command line and the last 200 log lines. Nothing is uploaded. The next editor start shows "The editor crashed last time" with Open Crash Report and Dismiss; unsaved scene changes are offered from the recovery file as usual. A panic in a background thread is reported ("hit an internal error") but is not a crash. In Play mode a game panic stops Play and is shown in the editor as before; it does not write a crash report.
