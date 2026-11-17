# Project templates and the examples gallery

`box new --template <name>` and the editor's **New Project** dialog create projects from the folders in `projects/`; the editor's **Open Example** copies a project listed in `examples/examples.json`. The code is `box_engine::templates` (`crates/box_engine/src/templates.rs`).

| Folder | What it is |
|---|---|
| `projects/<name>/` | A template: a complete project (`box.project.json`, `assets/`, `README.md`, `.gitignore`) plus `template.json` (`title`, `description`, `order`, optional `native` and `base`) and `preview.png` (480×270), which are not copied. |
| `examples/examples.json` | The gallery: `{"examples": [{"name", "title", "description"}]}`, `name` being a folder of the repository's `examples/`. |
| `examples/<name>.png` | An example's preview (480×270). |
| `authoring/` | How the template scenes are made: `box scene apply` of `<template>.commands.json` onto `empty.scene.json` (not shipped). |

Templates: `empty` (the starter scene, the same as the built-in fallback `Project::create`), `third-person`, `procedural`, `native` (`"base": "empty"` plus the crate `box new --native` writes; only the CLI creates it).

## Lookup

Searched in this order, each for `projects/` and `examples/examples.json` (templates are merged by name, the first found wins; the gallery comes from the first folder that has one):

1. `$BOX_TEMPLATES`
2. `<folder of the executable>/../Resources/templates` — inside the app: `Box Editor.app/Contents/Resources/templates/`
3. `<folder of the executable>/templates`
4. the source checkout's `templates/` (compiled-in path, used when it exists)

The executable's path has symlinks resolved first, so `box` symlinked from `~/.local/bin` into the app finds the app's templates. A gallery example's project is found at `<root>/examples/<name>/`, else `<root>/../examples/<name>/` (the source checkout).

## Packaging

- Templates: copy `templates/projects/` to `Box Editor.app/Contents/Resources/templates/projects/` (or `<dir of box>/templates/projects/` in a zip). It is self-contained.
- Gallery: copy `templates/examples/` (the JSON and the PNGs) to `.../templates/examples/`, and each project named in `examples.json` from `examples/<name>/` to `.../templates/examples/<name>/`, leaving out `.box/` (except `.box/types/`). Without them the editor shows no examples; the templates still work.

## Regenerating

```bash
# a template scene (here third-person) from its authoring commands
cp templates/authoring/empty.scene.json templates/projects/third-person/assets/scenes/main.scene.json
cargo run -p box_cli -- scene apply templates/projects/third-person/assets/scenes/main.scene.json templates/authoring/third-person.commands.json
# the procedural template's scatter model
cargo run -p box_proc --example gen_template_props
# a preview: capture at 1280x720, then scale to 480x270 (on a scratch copy, so no .box/ lands here)
cargo run -p box_player_app -- <copy> --capture shot.png --frames 90 --size 1280x720 && sips -z 270 480 shot.png --out templates/projects/<name>/preview.png
```

Tests: `cargo test -p box_engine --lib templates` (lookup, the app-bundle layout, the empty template equals the starter scene), `cargo test -p box_engine --test templates` (the templates play), `cargo test -p box_cli --test templates` (`box new`, check, run, captures, every gallery example), `cargo test -p box_editor --test templates` (the dialogs).
