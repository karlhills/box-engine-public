# Getting started

This guide takes you from nothing to a running game: install box-engine, create a project, place objects in the editor, write a first Luau script, press Play, and run the game in the player. It takes about fifteen minutes (plus the first build). You do not need to know Rust: games made with scripts need no Rust code at all.

## Contents

- [What you get](#what-you-get)
- [Install a release](#install-a-release)
- [Install from source](#install-from-source)
- [Create a project](#create-a-project)
- [Open the editor](#open-the-editor)
- [Add objects](#add-objects)
- [Write your first script](#write-your-first-script)
- [Play](#play)
- [Run the game in the player](#run-the-game-in-the-player)
- [Set up your code editor](#set-up-your-code-editor)
- [Without the editor: the `box` command](#without-the-editor-the-box-command)
- [Export your game](#export-your-game)
- [Where to go next](#where-to-go-next)

## What you get

box-engine is three programs:

| Program | What it does |
|---|---|
| `box-editor` | The editor: build scenes, edit components, write scripts, press Play. |
| `box-player` | Plays a project's start scene in a window (or headless, for tests). |
| `box` | The command-line tool: create projects, check them, inspect and edit scenes, render images, run games, generate script types and the API reference, and serve AI agents (`box mcp`). Every command takes `--json`. |

A **project** is a folder with a `box.project.json` file and an `assets/` folder. Scenes, scripts, models, materials, sounds, string tables and settings all live under `assets/` as ordinary files you can keep in Git.

## Install a release

A release needs no Rust or other developer tools. On a Mac with Apple Silicon:

1. Download `box-engine-<version>-macos.zip` and double-click it to unpack it (or open the `.dmg` and copy its folder out).
2. In Terminal, go into the unpacked folder and run the installer:

   ```bash
   cd ~/Downloads/box-engine-<version>-macos
   ./install.sh
   ```

   It copies **Box Editor.app** to `~/Applications` and links the commands `box`, `box-editor` and `box-player` into `~/.local/bin`. It needs no administrator password. If `~/.local/bin` is not on your `PATH` yet, it prints the line to add to `~/.zprofile`; run it, or open a new Terminal window afterwards.

3. Check the installation; all three print the same version:

   ```bash
   box --version
   box-editor --version
   box-player --version
   ```

The release folder also holds the guides (`docs/`), the script API reference and example projects (`examples/`). `./install.sh --help` lists the options: `--app-dir /Applications` installs the app for every user of the Mac (if you may write there), `--bin-dir <folder>` puts the commands elsewhere. To uninstall, run `./install.sh --uninstall` in the release folder, or delete `~/Applications/Box Editor.app` and the three links in `~/.local/bin`; your projects are never touched.

Releases are ad-hoc signed, not notarised by Apple. `install.sh` removes the download quarantine from the installed app so macOS opens it; if you copy the app by hand instead, right-click it and choose **Open** the first time.

For Windows, see the Windows guide when it is available.

## Install from source

You need:

- **Rust**, installed with [rustup](https://rustup.rs). The repository pins its toolchain (`rust-toolchain.toml`), so rustup downloads the right version the first time you build.
- **Git**, to get the source.
- macOS on Apple Silicon or Windows with a GPU that supports Metal, DirectX 12 or Vulkan.

Get the source and install the three programs. Each `cargo install` builds in release mode and copies the program to `~/.cargo/bin`, which rustup puts on your `PATH`:

```bash
git clone <repository URL> box-engine
cd box-engine
cargo install --locked --path apps/cli
cargo install --locked --path apps/editor
cargo install --locked --path apps/player
```

The first build takes several minutes; later ones reuse most of the work. Check that they are installed:

```bash
box --version
box-editor --version
box-player --version
```

To update later, pull the new source and run the same three `cargo install` commands again.

## Create a project

```bash
box new my-game
```

```text
Created project `my-game` from template `empty` in /path/to/my-game
start scene: assets/scenes/main.scene.json
README.md says what is in it
Next: box-editor my-game · box run my-game · box render my-game --out shot.png
```

The new project has one scene, `assets/scenes/main.scene.json`, with a ground plane, a cube, a sun and a camera. `box.project.json` names the project and its start scene (the one the player opens). A `.gitignore` leaves out `.box/`, the folder where the editor keeps per-machine state. This guide continues from this empty project.

**Templates.** `box new` starts from the `empty` template. Others give you more to start from; each project they create has a `README.md` that explains it:

| Template | What you get |
|---|---|
| `empty` (default) | A ground, a cube, a sun and a camera. |
| `third-person` | A small game in Luau: a skinned character on the character controller, a follow camera, platforms, pickups and a HUD. |
| `procedural` | A garden made of generators, spline sweeps, a Luau rule and scattered shrubs and rocks. |
| `native` | The empty project plus a Rust crate with your own editor and player (the same as `box new --native`; see [Native plugins](native-plugins.md)). |

```bash
box new --list-templates
box new my-platformer --template third-person
```

In the editor, **File ▸ New Project…** shows the same templates with a preview, and **File ▸ Open Example…** (also on the Welcome card) copies one of the example games into a folder of your own and opens it.

## Open the editor

```bash
box-editor my-game
```

The window has the **viewport** in the middle (your scene), the **hierarchy** on the left (every object), the **inspector** on the right (the selected object's components), and **Assets** and **Console** tabs along the bottom. The next time you start `box-editor` without a project, it reopens the last one.

Moving around the viewport:

- **Orbit**: Alt + left drag (or a two-finger scroll on a trackpad).
- **Pan**: drag with the middle mouse button (or Shift + two-finger scroll).
- **Zoom**: the mouse wheel, or pinch.
- **Frame** the selection: F.
- **Fly**: hold the right mouse button, move the mouse to look and use W A S D (Q and E go down and up).

The [editor manual](editor.md) has every pane, tool and shortcut. On macOS the shortcuts use Cmd; on Windows use Ctrl.

## Add objects

1. Click **+ Add** in the toolbar and choose **Meshes ▸ Sphere**. The sphere lands where the middle of the viewport looks, resting on the ground, and is selected.
2. Move it with the gizmo: **W** is Move, **E** Rotate, **R** Scale. Drag an arrow to move along one axis. Press **Y** to snap to steps.
3. In the inspector, open **Standard Material** and click **Base Color** to pick a colour. Every change is one undo step (Cmd+Z undoes, Cmd+Shift+Z redoes).
4. Add a light: **+ Add ▸ Lights ▸ Point Light**. Raise its **Intensity** in the inspector.
5. Rename the sphere: double-click its row in the hierarchy (or press F2) and call it `Ball`.
6. Save the scene with **Cmd+S** (File ▸ Save Scene). The title shows a dot `•` while there are unsaved changes.

## Write your first script

Scripts are Luau files under `assets/`. Create the folder `my-game/assets/scripts/` and, in it, a file `spinner.luau` with this text:

```lua
-- Turns the entity around its Y axis (getting started, script modules).
local Spinner = {}

Spinner.properties = {
    speed = { type = "number", default = 90, min = 0, max = 720, unit = "deg/s", doc = "Turn rate" },
}

function Spinner.on_update(self, dt)
    self.entity:rotate_y(math.rad(self.props.speed) * dt)
end

return Spinner
```

What it says:

- A script is a **module**: a table it returns at the end.
- `properties` declares values you can change per object in the inspector, here a turn rate in degrees per second with a slider from 0 to 720.
- `on_update(self, dt)` runs every frame. `self.entity` is the object the script is on, `self.props.speed` is this object's value of the property, and `dt` is the frame time in seconds. `rotate_y` takes radians, so the script converts with `math.rad`.

Now attach it:

1. Select the **Cube** in the hierarchy.
2. At the bottom of the inspector, click **Add Component**, type `Script` and choose **Script**.
3. In the new **Script** section, set **Source** to `scripts/spinner.luau`: drag the file from the Assets pane onto the field, or use the field's picker.

The inspector now shows a **Speed** row labelled "default". Drag it to change the speed for this cube only; ↺ resets it. Save the scene.

## Play

Press **Play** in the toolbar (Cmd+P). The game runs inside the editor from the scene as it is: the cube spins. While playing:

- The viewport has the game's keyboard, mouse and gamepad input. Escape gives input back to the editor; click the viewport to hand it back.
- The inspector shows live values. Changes you make during Play are discarded when you stop, unless you tick **Keep after Stop** on a field.
- Edit `spinner.luau` in your text editor and save: the script reloads at once, without stopping Play. Try a different speed or `rotate_y(-...)`.
- If a script fails, the Console shows the error with `file:line`, a toast appears, and only that object's script stops until you fix and save the file. The editor never crashes on a script error.

Press **Play** again (Cmd+P) to stop. The scene is back exactly as it was before Play.

To see output from a script, use `print` or `log.info`: messages appear in the Console tab. Try a second script, `assets/scripts/hello.luau`, on the Ball:

```lua
local Hello = {}

function Hello.on_start(self)
    print("hello from", self.entity:name())
end

return Hello
```

`on_start` runs once, when the object starts playing, so the Console shows `scripts/hello.luau: hello from Ball`. An object has at most one script; put more behaviour in the same file, or on another object. The [scripting guide](scripting.md) explains every callback.

## Run the game in the player

The player runs the project's start scene in its own window, the way your players will see it:

```bash
box-player my-game
```

Close the window to quit. Other ways to run it:

```bash
box run my-game                                   # the same, through the box tool
box-player my-game --capture shot.png --frames 90  # no window: render frame 90 to a PNG
box-player my-game --headless --frames 120         # no window, no image: just run 120 frames
```

The player and `box run` print script output (`print`, `log.info`) and errors to the terminal. `RUST_LOG` changes what is shown (for example `RUST_LOG=warn` hides script output).

## Set up your code editor

Any text editor works. For completion, type checking and hover documentation, use Visual Studio Code with the **Luau Language Server** extension (luau-lsp), then run:

```bash
box script types --project my-game
```

This writes `.box/types/box.d.luau`, the type definitions of every engine API and every component, and `.vscode/settings.json`, which points luau-lsp at it. Open the project folder in VS Code: typing `self.entity:` lists the entity methods, and hovering a function shows its description. Run the command again after you upgrade box-engine.

## Without the editor: the `box` command

Everything above can also be done from a terminal, which is handy for automation and for AI agents. These commands attach the spinner without the editor and check the result:

```bash
cat > spin.json <<'EOF'
[
  { "op": "add_component", "entity": "Cube", "component": "Script",
    "value": { "source": "scripts/spinner.luau", "properties": [] } }
]
EOF
box scene apply my-game/assets/scenes/main.scene.json spin.json
box check my-game
box run my-game --frames 120
box render my-game --out shot.png
```

- `box scene apply` applies editor commands (the same ones the editor uses, with validation and undo) and saves the scene. `--dry-run` only checks them; `box scene apply --help` lists the commands.
- `box check` parses every scene, imports every asset and resolves every reference; it exits with an error code when something is broken.
- `box scene show my-game` prints the scene's objects; `box registry` lists every component type with its fields.
- `box render` draws a scene to a PNG without opening a window.

## Export your game

Exporting turns the project into a game that runs on computers without box-engine: the engine's prebuilt player plus your project packed into one file, `game.pak`. A game made with scripts needs no compiler for this.

```bash
box export --project my-game --target macos --out my-game/export
```

```text
exported my-game/export/my-game.app (my-game 1.0.0, release build)
  run: my-game/export/my-game.app/Contents/MacOS/my-game
```

Double-click `my-game.app` in Finder, or run it from Terminal:

```bash
open my-game/export/my-game.app
my-game/export/my-game.app/Contents/MacOS/my-game --capture shot.png --frames 90   # no window: a screenshot
```

In the editor, **File ▸ Export…** does the same, and also edits the game's name, version, bundle id and icon (saved in `box.project.json` under `export`).

- **Profiles.** `--profile release` (the default) is the build you give to players. `--profile development` also turns on GPU validation, for finding rendering problems.
- **What goes in.** `box.project.json` and everything under `assets/`. Editor state (`.box/`), hidden files and anything outside `assets/` stay behind. Every asset is imported once while exporting: a broken file stops the export with a list of problems, so `box check my-game` first is a good habit.
- **Giving it to others.** The app is ad-hoc signed. It runs on your Mac; on another Mac, a downloaded copy opens after right-click ▸ **Open** (or `xattr -dr com.apple.quarantine my-game.app`). Opening without that step needs a Developer ID signature and notarisation with your own Apple developer account, which box-engine does not do for you. Zip the `.app` to send it (`ditto -c -k --keepParent my-game.app my-game.zip` keeps it intact).
- **Saves.** An exported game keeps its save games in `~/Library/Application Support/<bundle id>/saves`.
- **Windows.** `--target windows` makes a folder with `<Name>.exe` and `game.pak`, from a Mac too, when your box-engine includes the Windows player; see the Windows guide when it is available.

Exporting a native project (one with Rust code) builds its player with cargo, so it needs Rust. [EXPORT.md](../EXPORT.md) has the details: the pak format, signing, settings and what each platform needs.

## Where to go next

- [Editor manual](editor.md): every pane, tool, menu and shortcut.
- [Scripting guide](scripting.md): lifecycle, properties, components, events, UI, saves, physics, animation, rules, debugging and performance.
- [Script API reference](../reference/README.md): every function, method, callback and component, with descriptions.
- [Procedural guide](procedural.md): generators, splines, sweeps, rules and scatter.
- [Native plugins](native-plugins.md): writing game code in Rust.
- The example projects: in the editor, **File ▸ Open Example…** copies `collector_luau` (a complete small game in Luau), `cinder_keep` (a larger Luau game), `script_rules` (Luau rules that build geometry) or `script_demo` (a small scripted scene) into a folder of your own. They are also in the `examples/` folder of a release and of the source tree, with `examples/guide_samples` (every sample in these guides); open any of them with `box-editor <folder>`.
