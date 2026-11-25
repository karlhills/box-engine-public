//! Convert to Script in the editor (docs/BEHAVIOURS.md, B3): a Moving
//! Platform's Mover becomes `scripts/behaviours/mover.luau` with its values
//! as properties, written into the project, as one undo step; in Play the
//! converted platform moves. Capture (GPU):
//! `target/captures/behaviours/editor_converted.png` (the Script section).

use std::path::PathBuf;

use box_core::prelude::Transform;
use box_editor::headless::{CaptureOptions, Headless};
use box_editor::log::LogBuffer;
use box_editor::{EditorOptions, build_editor, layout};
use box_engine::behaviours::Mover;
use box_engine::box_script::Script;

fn project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("box_convert_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    box_engine::Project::create(&dir, name).unwrap();
    dir
}

#[test]
fn convert_to_script_writes_the_script_and_undoes_in_one_step() {
    let dir = project("mover");
    let mut editor = build_editor(
        &EditorOptions {
            project: Some(dir.clone()),
            ..Default::default()
        },
        LogBuffer::default(),
    )
    .unwrap();
    editor.tree = layout::default_layout();
    let platform = editor.state.add_preset("moving_platform").expect("placed");
    let undo_before = editor.state.edits.undo_len();
    let path = editor
        .state
        .convert_to_script(&[platform], "Mover")
        .expect("converted");
    assert_eq!(path, "scripts/behaviours/mover.luau");
    assert_eq!(editor.state.edits.undo_len(), undo_before + 1);
    let file = dir.join("assets").join(&path);
    let text = std::fs::read_to_string(&file).expect("script written");
    assert!(text.contains("function Mover.on_fixed_update"), "{text}");
    let e = editor.state.world.entity_by_uid(platform).unwrap();
    assert!(!editor.state.world.has::<Mover>(e));
    assert!(editor.state.world.has::<Script>(e));

    // Undo brings the Mover back (the file stays, for other copies).
    editor.state.undo_scene();
    let e = editor.state.world.entity_by_uid(platform).unwrap();
    assert!(editor.state.world.has::<Mover>(e));
    assert!(file.is_file());
    editor.state.redo_scene();

    // In Play the converted platform moves.
    let options = CaptureOptions {
        size: [1280, 720],
        pixels_per_point: 1.0,
        frames: 1,
    };
    let headless = if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        None
    } else {
        Headless::new(&mut editor, &options).ok()
    };
    if let Some(mut h) = headless {
        editor.state.select(Some(platform));
        for _ in 0..4 {
            h.frame(&mut editor, Vec::new());
        }
        let out =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/behaviours");
        std::fs::create_dir_all(&out).unwrap();
        h.image()
            .unwrap()
            .save(out.join("editor_converted.png"))
            .unwrap();
    }
    let start = {
        let w = &editor.state.world;
        w.get::<Transform>(w.entity_by_uid(platform).unwrap())
            .unwrap()
            .translation
    };
    editor.state.start_play().expect("play");
    let mut moved = 0.0f32;
    for _ in 0..600 {
        // As the windowed editor does every frame: serves the play world's
        // forwarded loads (the new script) from the edit world's server.
        editor.state.update_assets();
        editor.state.update_play(1.0 / 60.0).expect("play frame");
        std::thread::sleep(std::time::Duration::from_millis(2));
        let w = &editor.state.world;
        if let Some(e) = w.entity_by_uid(platform) {
            let at = w.get::<Transform>(e).unwrap().translation;
            moved = moved.max(at.distance(start));
        }
    }
    editor.state.stop_play();
    assert!(moved > 1.0, "the converted platform moved {moved} m");
    let _ = std::fs::remove_dir_all(&dir);
}
