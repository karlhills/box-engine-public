//! Scene documents, run headlessly (never a native dialog): Save As, Open
//! Scene, the unsaved-changes prompt, the recovery prompt, hierarchy drag
//! auto-scroll, and a capture of the unsaved-changes modal.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use box_core::scene;
use box_editor::headless::{CaptureOptions, capture};
use box_editor::log::LogBuffer;
use box_editor::panes::hierarchy::autoscroll_speed;
use box_editor::{
    Dialog, DocAction, Editor, EditorOptions, NewEntity, UnsavedChoice, build_editor, layout,
    recovery_is_newer,
};
use box_render::RenderError;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("box_editor_doc_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn project_editor(name: &str) -> (Editor, PathBuf) {
    let dir = temp_dir(name);
    box_engine::Project::create(&dir, name).unwrap();
    let options = EditorOptions {
        project: Some(dir.clone()),
        ..Default::default()
    };
    let editor = build_editor(&options, LogBuffer::default()).unwrap();
    let root = editor.state.project.as_ref().unwrap().root().to_path_buf();
    (editor, root)
}

fn main_scene(root: &Path) -> PathBuf {
    root.join("assets/scenes/main.scene.json")
}

fn set_mtime(path: &Path, t: SystemTime) {
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(t)
        .unwrap();
}

#[test]
fn save_as_switches_the_scene_and_writes_its_bytes() {
    let (mut editor, root) = project_editor("save_as");
    let main_before = std::fs::read(main_scene(&root)).unwrap();
    editor.state.add_entity(NewEntity::Cube, None).unwrap();
    assert!(editor.state.is_dirty());

    editor.begin_save_as(None);
    match editor.dialog() {
        Some(Dialog::SaveAs { path, then, .. }) => {
            assert_eq!(
                path, "assets/scenes/main.scene.json",
                "defaults next to the scene"
            );
            assert_eq!(*then, None);
        }
        other => panic!("expected Save As, got {other:?}"),
    }
    // Relative to the project; the extension is added.
    editor.answer_save_as(Some("assets/scenes/level2"));
    assert_eq!(editor.dialog(), None);
    let level2 = root.join("assets/scenes/level2.scene.json");
    assert_eq!(editor.state.scene.path.as_deref(), Some(level2.as_path()));
    assert_eq!(editor.state.scene.name, "level2");
    assert!(!editor.state.is_dirty());
    assert_eq!(
        std::fs::read(&level2).unwrap(),
        scene::save(&editor.state.world).unwrap()
    );
    assert_eq!(
        std::fs::read(main_scene(&root)).unwrap(),
        main_before,
        "the old scene file is untouched"
    );
    assert!(editor.title().contains("level2"));

    // Save now writes the new file.
    editor.state.add_entity(NewEntity::Sphere, None).unwrap();
    editor.save();
    assert_eq!(editor.dialog(), None);
    assert_eq!(
        std::fs::read(&level2).unwrap(),
        scene::save(&editor.state.world).unwrap()
    );

    // Saving over another existing file asks for a second confirmation.
    editor.begin_save_as(None);
    editor.answer_save_as(Some("assets/scenes/main.scene.json"));
    match editor.dialog() {
        Some(Dialog::SaveAs { error, replace, .. }) => {
            assert!(error.as_deref().unwrap().contains("already exists"));
            assert_eq!(replace.as_deref(), Some(main_scene(&root).as_path()));
        }
        other => panic!("expected the replace warning, got {other:?}"),
    }
    assert_eq!(std::fs::read(main_scene(&root)).unwrap(), main_before);
    editor.answer_save_as(Some("assets/scenes/main.scene.json"));
    assert_eq!(editor.dialog(), None);
    assert_eq!(editor.state.scene.name, "main");
    assert_ne!(std::fs::read(main_scene(&root)).unwrap(), main_before);

    // Blank input is an error, Cancel closes without saving.
    editor.begin_save_as(None);
    editor.answer_save_as(Some("  "));
    assert!(matches!(
        editor.dialog(),
        Some(Dialog::SaveAs { error: Some(_), .. })
    ));
    editor.answer_save_as(None);
    assert_eq!(editor.dialog(), None);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn scratch_scene_saves_anywhere_through_save_as() {
    let dir = temp_dir("scratch");
    std::fs::create_dir_all(&dir).unwrap();
    let mut editor = build_editor(&EditorOptions::default(), LogBuffer::default()).unwrap();
    assert!(editor.state.project.is_none());
    editor.save();
    assert!(
        matches!(editor.dialog(), Some(Dialog::SaveAs { .. })),
        "Save without a file goes to Save As"
    );
    let target = dir.join("courtyard.json");
    editor.answer_save_as(Some(target.to_str().unwrap()));
    let saved = dir.join("courtyard.scene.json");
    assert_eq!(editor.dialog(), None);
    assert_eq!(editor.state.scene.path.as_deref(), Some(saved.as_path()));
    assert_eq!(editor.state.scene.name, "courtyard");
    assert_eq!(
        std::fs::read(&saved).unwrap(),
        scene::save(&editor.state.world).unwrap()
    );
    assert!(editor.state.project.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unsaved_prompt_save_dont_save_and_cancel() {
    let (mut editor, root) = project_editor("unsaved");
    let main = main_scene(&root);
    let level2 = root.join("assets/scenes/level2.scene.json");
    editor.state.save_scene_as(&level2).unwrap();
    assert_eq!(
        editor
            .state
            .project
            .as_ref()
            .unwrap()
            .scene_files()
            .unwrap(),
        vec![level2.clone(), main.clone()]
    );

    // Not dirty: opens at once.
    editor.begin(DocAction::OpenScene(main.clone()));
    assert_eq!(editor.dialog(), None);
    assert_eq!(editor.state.scene.name, "main");

    // Dirty: asks first. Cancel keeps everything.
    let main_before = std::fs::read(&main).unwrap();
    let cube = editor.state.add_entity(NewEntity::Cube, None).unwrap();
    editor.begin(DocAction::OpenScene(level2.clone()));
    assert!(matches!(
        editor.dialog(),
        Some(Dialog::Unsaved {
            action: DocAction::OpenScene(_),
            ..
        })
    ));
    editor.answer_unsaved(UnsavedChoice::Cancel);
    assert_eq!(editor.dialog(), None);
    assert_eq!(editor.state.scene.name, "main");
    assert!(editor.state.is_dirty());

    // Don't Save: switches, the file and recovery stay untouched.
    editor.begin(DocAction::OpenScene(level2.clone()));
    editor.answer_unsaved(UnsavedChoice::DontSave);
    assert_eq!(editor.dialog(), None);
    assert_eq!(editor.state.scene.name, "level2");
    assert!(!editor.state.is_dirty());
    assert_eq!(std::fs::read(&main).unwrap(), main_before);
    assert!(
        !root
            .join(".box/recovery/project/assets/scenes/main.scene.json")
            .exists()
    );

    // Save: writes, then switches.
    editor.state.add_entity(NewEntity::Sphere, None).unwrap();
    let expected = scene::save(&editor.state.world).unwrap();
    editor.begin(DocAction::OpenScene(main.clone()));
    editor.answer_unsaved(UnsavedChoice::Save);
    assert_eq!(editor.dialog(), None);
    assert_eq!(editor.state.scene.name, "main");
    assert_eq!(std::fs::read(&level2).unwrap(), expected);
    assert!(editor.state.world.entity_by_uid(cube).is_none());

    // Quit: clean quits at once; dirty asks, Don't Save leaves no recovery.
    assert!(editor.request_quit());
    let mut editor = build_editor(
        &EditorOptions {
            project: Some(root.clone()),
            ..Default::default()
        },
        LogBuffer::default(),
    )
    .unwrap();
    editor.state.add_entity(NewEntity::Cube, None).unwrap();
    assert!(!editor.request_quit());
    assert!(!editor.quit_requested());
    editor.answer_unsaved(UnsavedChoice::DontSave);
    assert!(editor.quit_requested());
    editor.on_quit();
    assert!(
        !root
            .join(".box/recovery/project/assets/scenes/main.scene.json")
            .exists()
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn unsaved_prompt_before_project_switch_discards_only_when_it_happens() {
    let (mut editor, root) = project_editor("switch");
    let other = temp_dir("switch_other");
    box_engine::Project::create(&other, "Other").unwrap();
    let cube = editor.state.add_entity(NewEntity::Cube, None).unwrap();

    editor.begin(DocAction::OpenProject);
    editor.answer_unsaved(UnsavedChoice::DontSave);
    assert!(matches!(
        editor.dialog(),
        Some(Dialog::Project { discard: true, .. })
    ));
    // Cancelling the folder entry keeps the unsaved changes.
    editor.answer_project(None);
    assert!(editor.state.is_dirty());
    assert!(editor.state.world.entity_by_uid(cube).is_some());

    editor.begin(DocAction::OpenProject);
    editor.answer_unsaved(UnsavedChoice::DontSave);
    // A bad folder keeps the modal open with an error, and changes intact.
    let missing = temp_dir("switch_missing");
    editor.answer_project(Some(missing.to_str().unwrap()));
    assert!(matches!(
        editor.dialog(),
        Some(Dialog::Project { error: Some(_), .. })
    ));
    assert!(editor.state.is_dirty());
    editor.answer_project(Some(other.to_str().unwrap()));
    assert_eq!(editor.dialog(), None);
    assert_eq!(editor.state.project.as_ref().unwrap().name(), "Other");
    assert!(
        !root
            .join(".box/recovery/project/assets/scenes/main.scene.json")
            .exists()
    );

    // Save on a scene without a file asks for one, then runs the action.
    let mut scratch = build_editor(&EditorOptions::default(), LogBuffer::default()).unwrap();
    scratch.state.add_entity(NewEntity::Cube, None).unwrap();
    assert!(!scratch.request_quit());
    scratch.answer_unsaved(UnsavedChoice::Save);
    assert!(matches!(
        scratch.dialog(),
        Some(Dialog::SaveAs {
            then: Some(DocAction::Quit),
            ..
        })
    ));
    let file = other.join("from_scratch.scene.json");
    scratch.answer_save_as(Some(file.to_str().unwrap()));
    assert!(scratch.quit_requested());
    assert!(file.exists());
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&other);
}

#[test]
fn recovery_is_offered_only_when_newer() {
    let dir = temp_dir("newer");
    std::fs::create_dir_all(&dir).unwrap();
    let scene = dir.join("a.scene.json");
    let recovery = dir.join("a.recovery.json");
    std::fs::write(&scene, "{}").unwrap();
    assert!(!recovery_is_newer(&scene, &recovery), "no recovery file");
    std::fs::write(&recovery, "{}").unwrap();
    let t0 = SystemTime::now() - Duration::from_secs(3600);
    set_mtime(&scene, t0);
    set_mtime(&recovery, t0 + Duration::from_secs(60));
    assert!(recovery_is_newer(&scene, &recovery));
    set_mtime(&scene, t0 + Duration::from_secs(120));
    assert!(
        !recovery_is_newer(&scene, &recovery),
        "saved after the crash"
    );
    std::fs::remove_file(&scene).unwrap();
    assert!(recovery_is_newer(&scene, &recovery), "scene missing");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn recovery_prompt_restores_or_discards() {
    let (mut editor, root) = project_editor("recovery_prompt");
    let options = EditorOptions {
        project: Some(root.clone()),
        ..Default::default()
    };
    let cube = editor.state.add_entity(NewEntity::Cube, None).unwrap();
    editor.on_quit();
    let recovery = root.join(".box/recovery/project/assets/scenes/main.scene.json");
    let kept = std::fs::read(&recovery).unwrap();
    // The scene file is older than the recovery copy.
    set_mtime(
        &main_scene(&root),
        SystemTime::now() - Duration::from_secs(3600),
    );

    let mut editor = build_editor(&options, LogBuffer::default()).unwrap();
    assert_eq!(
        editor.dialog(),
        Some(&Dialog::Recovery {
            recovery: recovery.clone()
        })
    );
    assert!(editor.state.world.entity_by_uid(cube).is_none());
    editor.answer_recovery(true);
    assert_eq!(editor.dialog(), None);
    assert!(editor.state.is_dirty(), "restored changes are unsaved");
    assert!(editor.state.world.entity_by_uid(cube).is_some());
    assert_eq!(scene::save(&editor.state.world).unwrap(), kept);
    assert!(recovery.exists(), "kept until saved or discarded");
    drop(editor);

    let mut editor = build_editor(&options, LogBuffer::default()).unwrap();
    assert!(matches!(editor.dialog(), Some(Dialog::Recovery { .. })));
    editor.answer_recovery(false);
    assert!(!recovery.exists());
    assert!(!editor.state.is_dirty());
    assert!(editor.state.world.entity_by_uid(cube).is_none());
    drop(editor);

    // Discarded: no prompt next time.
    let editor = build_editor(&options, LogBuffer::default()).unwrap();
    assert_eq!(editor.dialog(), None);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn drag_autoscroll_speed_ramps_at_the_edges() {
    let (top, bottom, band, max) = (100.0, 500.0, 40.0, 600.0);
    let s = |y| autoscroll_speed(y, top, bottom, band, max);
    assert_eq!(s(300.0), 0.0, "middle");
    assert_eq!(s(140.0), 0.0, "band edge");
    assert_eq!(s(460.0), 0.0, "band edge");
    assert!((s(120.0) + 300.0).abs() < 1e-3, "halfway into the top band");
    assert!(
        (s(480.0) - 300.0).abs() < 1e-3,
        "halfway into the bottom band"
    );
    assert_eq!(s(100.0), -max);
    assert_eq!(s(20.0), -max, "beyond the top edge");
    assert_eq!(s(900.0), max, "beyond the bottom edge");
    // A short list shrinks the band so its middle never scrolls.
    assert_eq!(autoscroll_speed(50.0, 0.0, 100.0, 40.0, max), 0.0);
    assert!(autoscroll_speed(10.0, 0.0, 100.0, 40.0, max) < 0.0);
}

/// Renders the unsaved-changes modal over the editor to
/// `target/captures/editor_unsaved_prompt.png`. Skipped with
/// `BOX_SKIP_GPU_TESTS` or without an adapter.
#[test]
fn unsaved_prompt_capture() {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        return;
    }
    let log = LogBuffer::default();
    log.set_manual_time(0.0);
    let mut editor = build_editor(&EditorOptions::default(), log).unwrap();
    editor.tree = layout::default_layout();
    editor.state.add_entity(NewEntity::Cube, None).unwrap();
    assert!(!editor.request_quit());
    let options = CaptureOptions {
        size: [1600, 1000],
        pixels_per_point: 2.0,
        // Past the modal's fade-in.
        frames: 30,
    };
    let image = match capture(&mut editor, &options) {
        Ok(image) => image,
        Err(e)
            if matches!(
                e.downcast_ref::<RenderError>(),
                Some(RenderError::NoAdapter)
            ) =>
        {
            return;
        }
        Err(e) => panic!("{e:#}"),
    };
    assert!(
        matches!(editor.dialog(), Some(Dialog::Unsaved { .. })),
        "the prompt stays open without an answer"
    );
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures");
    std::fs::create_dir_all(&out).unwrap();
    image.save(out.join("editor_unsaved_prompt.png")).unwrap();
}
