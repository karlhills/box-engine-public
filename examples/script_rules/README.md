# Script Rules

A data project (no Rust): two Luau rules build its geometry while you edit. `assets/rules/fence.luau` turns a `ScriptIntent`'s points into posts (`Primitive`) and swept rails (`Spline` + `Sweep`); where two fences' ends meet, the fence with the lower uid builds the shared post. `assets/rules/colonnade.luau` places turned columns (`Lathe`) and beams.

Open it with `cargo run -p box_editor_app -- examples/script_rules`: select a fence and edit its properties in the inspector, drag its points with the Path tool, or draw another (Path copies the selected fence's rule and settings); View ▸ Rules lists both rules. Edit a rule file while the editor runs, or during Play, and the parts update. `box render examples/script_rules --out shot.png` renders them. The scene is generated from `authoring/main.commands.json` with `box scene apply`.
