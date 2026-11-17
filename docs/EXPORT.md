# Exporting games

How a box project becomes a game that runs on a machine without box-engine
or Rust (M9, risk R16, spike S7: `spikes/s7_export/FINDINGS.md`). The
model is Godot's export templates: **a prebuilt player plus a data pack**
(docs/ASSETS.md section 7).

```
box export --project my-game --target macos   --profile release     --out my-game/export
box export --project my-game --target windows --profile development --out my-game/export
```

In the editor: **File ▸ Export…** (also **Game ▸ Project Settings ▸ Export
Settings…**) runs the same code (`box_export::export`).

## 1. What an export contains

macOS (`--target macos`):

```
<Name>.app/Contents/Info.plist                 generated: name, bundle id, version
<Name>.app/Contents/MacOS/<Name>               the player
<Name>.app/Contents/Resources/AppIcon.icns     from the project's icon (or the engine's)
<Name>.app/Contents/Resources/game.pak         the cooked project
```

The bundle is ad-hoc signed (`codesign --force --sign - --timestamp=none`,
part of macOS; no Xcode), which Apple Silicon requires. It runs on the Mac
that made it, and on other Macs after right-click ▸ Open (or `xattr -dr
com.apple.quarantine <Name>.app`): a downloaded ad-hoc app is quarantined and
Gatekeeper refuses it otherwise. Distributing without that step needs a
Developer ID signature and notarization with the developer's own Apple
credentials; the engine never signs with a real identity. Signing seals
`Info.plist` and `Resources`, so it is always the last step; editing the
bundle afterwards breaks the signature (re-sign with the command above).

Windows (`--target windows`):

```
<Name>/<Name>.exe     the player, with icon, version info and subsystem patched
<Name>/game.pak
```

Resources are patched with `editpe` from any host: the main icon (resized
from the PNG), version info (`ProductName`, `FileDescription`,
`ProductVersion`, `FileVersion`, `OriginalFilename`, `InternalName`) and the
PE subsystem: **GUI for release** (no console window), console for
development. The exe is unsigned; sign it with `signtool` (or
`osslsigncode`) if needed.

## 2. Settings (`box.project.json`)

```json
"export": { "app_name": "Cinder Keep", "bundle_id": "com.example.cinder-keep",
            "version": "1.2.0", "icon": "assets/icon.png" }
```

| Field | Default | Rules |
|---|---|---|
| `app_name` | the project name | 1-64 characters; the bundle and exe file names drop `<>:"/\|?*` |
| `bundle_id` | `com.example.<name>` | reverse DNS, letters, digits, `-` |
| `version` | `1.0.0` | one to three numbers 0-65535 |
| `icon` | the engine's icon | a square PNG inside the project (1024 px is best) |

The section is optional and left out of the file while empty, so older
engines still read projects that never set it. The editor's Export window
validates each field as it is typed and writes the file on Save Settings or
Export.

## 3. Profiles

| | release | development |
|---|---|---|
| GPU backend validation (wgpu `VALIDATION` + `DEBUG`) | off | on |
| Windows console window | none (GUI subsystem) | yes |
| Hot reload (assets, scripts, engine shaders) | off | off (a pak cannot change) |
| Logs | stderr, warnings and `box*` info | the same |

wgpu's own API validation, which `--report`/`FrameStats::validation_errors`
count, is always on. The profile is recorded in the pak's manifest and
applied by the player at startup (`box_render::set_gpu_validation`).

## 4. Cook and `game.pak`

`box_export::cook`:

1. Every asset file is imported once with the engine's importers (the
   player's own code). A file that does not import, a broken `.meta` or a
   missing start scene stops the export with the list of problems (CLI:
   error code `cook_failed` with `problems`; editor: an error toast). Missing
   `.meta` files are created in the project first, as the editor does, so
   asset ids are stable.
2. Packed: `box.project.json` and `assets/**` with their `.meta` files, plus
   `export.json` (format, name, bundle id, version, profile, engine version,
   target). Left behind: hidden files and folders (`.box/`, `.DS_Store`,
   `.vscode/`), `authoring/`, docs, `Cargo.*`, `src/`, `target/`, the export
   folder itself, everything outside `assets/`.
3. The pak is reproducible: the same project gives the same bytes (sorted
   entries, no timestamps).

**Pak format** (`box_assets::pak`, version 1; little-endian):

| bytes | field |
|---|---|
| 0..8 | magic `BOXPAK\r\n` |
| 8..12 | format version |
| 12..16 | flags (0) |
| 16..20 | entry count |
| 20..24 | reserved |
| 24..32 | index offset |
| 32..40 | index length |
| 40..56 | xxh3-128 of the index |
| 56..64 | xxh3-64 of bytes 0..56 |

Contents follow the header back to back, 16-byte aligned, uncompressed. The
index at the end holds per entry (sorted by path): `u16` path length, the
UTF-8 path, `u64` offset, `u64` length, xxh3-128 of the content. `Pak::open`
checks the header and index hashes and that the index ends the file;
`Pak::read` checks the file's hash on every read. A damaged pak is reported
as `<path>: damaged pak (index hash mismatch)` at startup, or `damaged pak
(<file>: content hash mismatch)` when that file is read. Why this format and
not zip: spike S7 (2.3x faster reads than a stored zip, 4-6x faster than
loose files, microseconds to open; deflate reads 7-40x slower).

## 5. The player and paks

The player (`box-player`, a native game's own player) plays a pak when

- it is given one: `box-player path/to/game.pak`, or
- it has no project argument and a `game.pak` sits next to the executable,
  or in `../Resources/` when the executable is in an app's
  `Contents/MacOS/` (`box_engine::packed::find_bundled_pak`). It is located
  from the executable, never the working directory: an app opened from
  Finder runs in `/`.

A native game's player bin calls `options.default_project(<its folder>)`
(the `box new --native` template does), which plays its own project folder
only when no pak is bundled; the same binary plays the pak in an export.

The pak is **mounted** (`box_assets::vfs`) at its own path, which becomes the
project root: `<...>/game.pak/box.project.json`,
`<...>/game.pak/assets/scenes/main.scene.json`. Everything that reads
project files goes through `vfs::read`/`is_file`/`is_dir`/`read_dir`
(`Project::open`, scene loads and switches, settings and
`input.actions.json`, the asset index and `.meta` files, importer
dependencies such as glTF buffers, Luau `require`), so a project folder and
a pak take the same code paths. A mounted project is read-only: the asset
server never writes `.meta` files and refuses `watch`, `write`, `create`,
`rename` and `set_settings` (`AssetError::ReadOnly`); settings files are not
created; no registry file is written; `--control` is refused (there is no
project folder for `.box/control.json`); engine shader hot reload (debug
builds) is off. Save games go to the per-user folder named after the bundle
id: `~/Library/Application Support/<bundle_id>/saves` on macOS,
`%APPDATA%\<bundle_id>\saves` on Windows (`box_engine::packed::data_dir`).

## 6. Player templates

A script project bundles a prebuilt `box-player` of the same engine version,
found in this order (`box_export::template`):

1. `$BOX_TEMPLATES/<target>/box-player[.exe]`
2. `<folder of box or box-editor>/templates/<target>/...`
3. `<folder of box-editor>/../Resources/templates/<target>/...` (inside the
   editor's `.app`)
4. for the host platform: `box-player` next to `box`/`box-editor` (a cargo
   target folder, `cargo install` of the engine's apps)

`<target>` is `macos` or `windows`. A `VERSION` file in a templates folder
must equal the engine version. `--player <exe>` overrides the lookup. The
program's folder is its real location: `box` run through a symlink (as
`install.sh` installs it into `~/.local/bin`) finds the templates of the app
it links into. Release layout and packaging: [RELEASING.md](RELEASING.md).

A native project (a `Cargo.toml` at its root) builds its own player:
`cargo build --release --bin <player> [--features ...]` in the project
folder, the executable path read from cargo's JSON messages. Only for the
host platform: a native Windows game is exported on Windows.

**Windows templates are built on Windows (CI)**, not cross-compiled: the
player links wgpu, Luau (C++ through `cc`) and audio, which need the MSVC
toolchain and SDK. An engine release ships `templates/macos/box-player` and
`templates/windows/box-player.exe` so a Mac user can export a Windows build
of a script project (docs/ASSETS.md section 8).

## 7. Texture compression

Not done at export yet; textures ship as their PNG/JPEG sources and are
decoded at load, as in the editor. `texture-compressor` 0.2.4 (MIT OR
Apache-2.0, pure Rust, deterministic BC1-BC7) is a suitable encoder. Using
it needs a compressed variant of `ImageData` (today RGBA8 rows only, a public
struct used across crates), BC uploads in the renderer with a
`TEXTURE_COMPRESSION_BC` check and an RGBA fallback, and cooked artefacts for
glTF-embedded images. The examples' textures total 1.7 MB (six files), so
there is nothing to gain or measure yet; it waits for a project whose texture
memory or load time asks for it (R22 stays open).

## 8. What needs Windows or CI

- Building the Windows `box-player.exe` template (the `windows` job of
  `.github/workflows/release.yml`, on `windows-2022`; RELEASING.md §5). The
  macOS release packaging (`scripts/release/package-macos.sh`) puts it next
  to the macOS one when it is in `target/templates/windows/`; the workflow
  puts it there. A Windows release package (editor and CLI for Windows
  users) would be built on Windows; no script builds one yet.
- Running an exported Windows game: that the patched exe starts, finds
  `game.pak`, shows its icon in Explorer and the taskbar and its version in
  Properties, and that release builds show no console window. On macOS the
  patching is tested on a real (tiny, `rust-lld`-linked) PE:
  `crates/box_export/tests/fixtures/tiny.exe`.
- Exporting a native project for Windows (cargo on Windows).

## 9. Tests

- `cargo test -p box_assets --lib --test pak`: pak round trip, listing,
  alignment, determinism, damage (content, index, header, truncation, magic,
  newer version); the asset server on a mounted pak (same ids, imports with
  glTF buffers and images from the pak, writes and watching refused, missing
  `.meta` reported).
- `cargo test -p box_player_app --test export` (this `box-player` is the
  template): the three data examples (`collector_luau`, `cinder_keep`,
  `script_demo`) exported for macOS in both profiles; pak contents and
  manifest; Info.plist, icon, `codesign --verify --strict`; the `.app` copied
  to a folder with no project, run headless (20 frames, capture, private
  HOME), its log naming the profile and the pak, no validation errors; its
  capture equal to a run from the project folder (golden tolerance). Cook
  determinism and round trip; a mounted pak read-only without a watcher even
  for a windowed, watching run; damaged paks; `--control` refused; the
  Windows layout, icon, version and subsystem per profile, re-export
  replacing the earlier one, a Mach-O refused as a Windows template, a
  stranger's folder kept; `open -W -n` (LaunchServices) launching the app.
  `--ignored`: `examples/collector` exported through `cargo build --release`.
- `cargo test -p box_editor --test export`: the Export window's settings
  validation, save, background export with progress, success and error
  toasts; a capture of the window (`target/captures/export/editor_export.png`).
- `cargo test -p box_export`: Info.plist escaping, `.icns` structure and
  determinism, names, bundle ids, settings validation.

## 10. Measurements

Apple M3 Ultra, macOS 26.6.2, release `box` and `box-player`, script
projects (template player), medians of 5 exports:

| project | packed files | assets + project file | project folder | game.pak | .app | export | import (validation) | pak write | bundle | sign |
|---|---|---|---|---|---|---|---|---|---|---|
| collector_luau | 40 | 266 KB | 356 KB | 270 KB | 37.8 MB | 304 ms | 11 ms | 7 ms | 158 ms | 121 ms |
| cinder_keep | 48 | 653 KB | 793 KB | 657 KB | 38.2 MB | 237 ms | 11 ms | 8 ms | 120 ms | 83 ms |
| script_demo | 24 | 41 KB | 46 KB | 43 KB | 37.6 MB | 173 ms | 9 ms | 5 ms | 86 ms | 68 ms |

The pak is 1 % larger than the files it holds (header, index, alignment) and
24-28 % smaller than the project folder (editor state, authoring, docs
stay behind). The bundle is the player (36.6 MB release binary) plus the
pak; bundling time is mostly copying the binary and writing the icon set.

Native export (`examples/collector`, release player already built once):
11.2 s cargo (incremental), 0.5 s cook, bundle and sign; 16 files, 155 KB pak.

Startup (process start to exit after one headless frame, 640x360, medians
of 12 runs, the same release `box-player`):

| project | from the folder | from game.pak | the .app's executable |
|---|---|---|---|
| collector_luau | 150 ms | 149 ms | 169 ms |
| cinder_keep | 128 ms | 126 ms | 156 ms |
| script_demo | 125 ms | 106 ms | 98 ms |

Folder and pak are equal within noise: startup is dominated by GPU device
and pipeline creation, and reading the assets is under a millisecond either
way (spike S7: reading every file takes 0.07-0.1 ms from the pak against
0.4-0.6 ms as loose files). The `.app` column varies by ±30 ms between runs
(a freshly copied, signed binary in a new location).
