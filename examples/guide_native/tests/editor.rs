//! The guide's editor plugin (feature `editor`): the Reset Score palette
//! action. `cargo test -p guide_native --features editor --test editor`;
//! the capture (GPU) is `target/captures/guide_native/palette.png`.
#![cfg(feature = "editor")]

use box_editor::headless::{CaptureOptions, capture};
use box_editor::log::LogBuffer;
use box_editor::{Editor, EditorOptions, build_game_editor_with};
use guide_native::editor::GuideEditor;
use guide_native::gameplay::Score;
use guide_native::{game, project_dir};

fn open() -> Editor {
    let options = EditorOptions {
        project: Some(project_dir()),
        no_restore: true,
        ..Default::default()
    };
    build_game_editor_with(&options, LogBuffer::default(), game(), &[&GuideEditor]).unwrap()
}

fn save_capture(editor: &mut Editor, file: &str) {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        return;
    }
    let image = capture(
        editor,
        &CaptureOptions {
            size: [1600, 900],
            pixels_per_point: 1.0,
            frames: 4,
        },
    )
    .unwrap();
    let out = project_dir()
        .join("../../target/captures/guide_native")
        .join(file);
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    image.save(&out).unwrap();
}

/// The inspector shows a game component with its units, ranges and
/// tooltips (`inspector.png`).
#[test]
fn the_inspector_shows_game_components() {
    let mut editor = open();
    // Game ▸ Project Settings lists the settings resource.
    assert!(editor.state.settings_types().contains(&"Tuning"));
    let world = &editor.state.world;
    let pickup = world.find_by_name("Pickup 1").unwrap();
    let uid = world.uid(pickup);
    editor.state.select(uid);
    save_capture(&mut editor, "inspector.png");
}

#[test]
fn reset_score_runs_only_during_play_and_is_in_the_palette() {
    let mut editor = open();
    let state = &mut editor.state;
    assert_eq!(
        state.run_action("guide.reset_score"),
        Err("Start Play first".to_string())
    );

    state.start_play().unwrap();
    state.world.resource_mut::<Score>().points = 42;
    state.run_action("guide.reset_score").unwrap();
    assert_eq!(state.world.resource::<Score>().points, 0);

    state.open_palette();
    state.set_palette_query("reset score");
    let rows = state.palette_results();
    assert!(
        rows.first()
            .is_some_and(|r| r.label == "Reset Score" && r.enabled.is_ok()),
        "{:?}",
        rows.iter().map(|r| &r.label).collect::<Vec<_>>()
    );
    save_capture(&mut editor, "palette.png");
}
