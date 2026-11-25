//! Behaviours in the editor (docs/BEHAVIOURS.md): Add ▸ Gameplay places
//! presets on the surface as one undo step and selects them; the palette
//! lists them; Scene Settings adds `GameRules` and `HudSettings`. Captures
//! in `target/captures/behaviours/` (`editor_preset.png`: a placed player and
//! pickup with the pickup's inspector; `editor_scene_rules.png`: the scene
//! settings with the rules added).

use std::path::PathBuf;

use box_core::identity::Name;
use box_core::prelude::Transform;
use box_editor::headless::{CaptureOptions, Headless};
use box_editor::log::LogBuffer;
use box_editor::{EditorOptions, build_editor, layout};

#[test]
fn gameplay_presets_and_scene_rules() {
    let mut editor = build_editor(&EditorOptions::default(), LogBuffer::default()).expect("editor");
    editor.tree = layout::default_layout();

    // The menu and the palette list the presets.
    let presets = editor.state.preset_entries();
    let titles: Vec<&str> = presets.iter().map(|p| p.title.as_str()).collect();
    for t in [
        "Player",
        "Pickup",
        "Hazard",
        "Moving Platform",
        "Checkpoint",
        "Goal",
    ] {
        assert!(titles.contains(&t), "{t} in {titles:?}");
    }
    assert!(presets.iter().all(|p| p.category == "Gameplay"));
    let palette = box_editor::actions::context_actions(&mut editor.state);
    assert!(palette.iter().any(|a| a.id() == "add.preset:pickup"));

    // A player: its root and its camera, one undo step.
    let undo_before = editor.state.edits.undo_len();
    let player = editor.state.add_preset("player").expect("player placed");
    assert_eq!(editor.state.edits.undo_len(), undo_before + 1);
    assert_eq!(editor.state.primary(), Some(player));
    let w = &editor.state.world;
    let e = w.entity_by_uid(player).unwrap();
    assert_eq!(w.get::<Name>(e).unwrap().as_str(), "Player");
    assert!(w.find_by_name("Player Camera").is_some());
    editor.state.undo_scene();
    assert!(editor.state.world.find_by_name("Player Camera").is_none());
    editor.state.redo_scene();
    assert!(editor.state.world.find_by_name("Player Camera").is_some());

    // A pickup rests on what is under the view centre (here the player),
    // lifted by its own 0.8 m.
    let pickup = editor.state.add_preset("pickup").expect("pickup placed");
    let w = &editor.state.world;
    let t = w
        .get::<Transform>(w.entity_by_uid(pickup).unwrap())
        .unwrap()
        .translation;
    assert!(t.y > 0.5, "{t}");

    // Scene settings: the rules and the HUD can be added, runtime state not.
    let missing = editor.state.missing_scene_resources();
    assert!(
        missing.contains(&"GameRules") && missing.contains(&"HudSettings"),
        "{missing:?}"
    );
    assert!(!missing.contains(&"GameState") && !missing.contains(&"GameVars"));

    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        assert!(editor.state.add_scene_resource("GameRules"));
        return;
    }
    let options = CaptureOptions {
        size: [1440, 900],
        pixels_per_point: 1.0,
        frames: 1,
    };
    let mut h = match Headless::new(&mut editor, &options) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("skipped (no GPU): {e:#}");
            return;
        }
    };
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/behaviours");
    std::fs::create_dir_all(&out).unwrap();
    for _ in 0..4 {
        h.frame(&mut editor, Vec::new());
    }
    h.image()
        .unwrap()
        .save(out.join("editor_preset.png"))
        .unwrap();

    editor.state.select(None);
    assert!(editor.state.add_scene_resource("GameRules"));
    assert!(editor.state.world.get_resource_reflect("GameRules").is_ok());
    assert!(
        !editor
            .state
            .missing_scene_resources()
            .contains(&"GameRules")
    );
    for _ in 0..4 {
        h.frame(&mut editor, Vec::new());
    }
    h.image()
        .unwrap()
        .save(out.join("editor_scene_rules.png"))
        .unwrap();
    // Adding it is one undo step.
    editor.state.undo_scene();
    assert!(
        editor
            .state
            .world
            .get_resource_reflect("GameRules")
            .is_err()
    );
}
