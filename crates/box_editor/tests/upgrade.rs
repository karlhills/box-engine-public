//! Opening a project whose files are older than the engine (an older
//! `engine_version` and a game component at an older type version): the
//! upgrade prompt and its three answers, run headlessly through the dialog
//! methods, and a capture of the prompt (needs a GPU).

use std::path::{Path, PathBuf};

use box_core::identity::Uid;
use box_core::migrate::Step;
use box_core::prelude::*;
use box_core::reflect::{Reflect, ReflectComponent, ReflectDefault};
use box_editor::headless::{CaptureOptions, capture};
use box_editor::log::LogBuffer;
use box_editor::{Dialog, Editor, EditorOptions, UpgradeChoice, build_game_editor, layout};
use box_engine::Game;
use box_engine::project::ENGINE_VERSION;
use box_render::RenderError;
use serde_json::{Value as Json, json};

/// A game component at version 2: version 1 called `rate` `speed`.
#[derive(Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default, Clone)]
#[type_path = "upgrade_test"]
struct Spinner {
    rate: f32,
}

struct SpinnerGame;
impl Plugin for SpinnerGame {
    fn register(&self, registry: &mut Registry) {
        registry.register::<Spinner>().unwrap();
        registry.add_migration(
            "Spinner",
            Step::new("speed is now rate").rename_field("speed", "rate"),
        );
    }
    fn build(&self, _app: &mut App) {}
}

fn game() -> Game {
    Game::new().with_plugin(SpinnerGame)
}

fn scene_path(root: &Path) -> PathBuf {
    root.join("assets/scenes/main.scene.json")
}

fn read_json(path: &Path) -> Json {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

/// A project made by an older engine, with a version 1 `Spinner`.
fn old_project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("box_upgrade_{name}_{}", Uid::new()));
    box_engine::Project::create(&dir, name).unwrap();
    let dir = dir.canonicalize().unwrap();
    let mut scene = read_json(&scene_path(&dir));
    scene["entities"].as_array_mut().unwrap().push(json!({
        "uid": "00000000-0000-4000-8000-00000000beef",
        "name": "Fan",
        "parent": null,
        "components": { "Spinner": { "speed": 3.0 } }
    }));
    std::fs::write(scene_path(&dir), serde_json::to_vec_pretty(&scene).unwrap()).unwrap();
    let project_file = dir.join("box.project.json");
    let mut pf = read_json(&project_file);
    pf["engine_version"] = json!("0.0.9");
    std::fs::write(&project_file, serde_json::to_vec_pretty(&pf).unwrap()).unwrap();
    dir
}

fn editor_for(project: Option<PathBuf>) -> Editor {
    let options = EditorOptions {
        project,
        ..Default::default()
    };
    build_game_editor(&options, LogBuffer::default(), game()).unwrap()
}

fn fan_rate(editor: &Editor) -> f32 {
    let w = &editor.state.world;
    w.get::<Spinner>(w.find_by_name("Fan").unwrap())
        .unwrap()
        .rate
}

#[test]
fn opening_asks_and_cancel_leaves_everything_alone() {
    let root = old_project("cancel");
    let scene_before = std::fs::read(scene_path(&root)).unwrap();
    let mut editor = editor_for(Some(root.clone()));
    match editor.dialog() {
        Some(Dialog::Upgrade {
            root: r,
            summary,
            can_upgrade,
            ..
        }) => {
            assert_eq!(r, &root);
            assert!(*can_upgrade);
            assert!(
                summary.contains("`Spinner` (1x): speed is now rate"),
                "{summary}"
            );
            assert!(summary.contains("engine_version 0.0.9 -> "), "{summary}");
        }
        other => panic!("expected the upgrade prompt, got {other:?}"),
    }
    assert!(
        editor.state.project.is_none(),
        "not opened before an answer"
    );
    editor.answer_upgrade(UpgradeChoice::Cancel);
    assert_eq!(editor.dialog(), None);
    assert!(editor.state.project.is_none());
    assert_eq!(std::fs::read(scene_path(&root)).unwrap(), scene_before);
    assert!(!root.join(".box/backups").exists());
}

#[test]
fn read_only_opens_migrated_in_memory_and_refuses_to_write() {
    let root = old_project("read_only");
    let scene_before = std::fs::read(scene_path(&root)).unwrap();
    let mut editor = editor_for(None);
    editor.open_project(&root).unwrap();
    assert!(matches!(editor.dialog(), Some(Dialog::Upgrade { .. })));
    editor.answer_upgrade(UpgradeChoice::ReadOnly);
    assert_eq!(editor.dialog(), None);
    assert!(editor.state.read_only);
    assert_eq!(editor.state.project.as_ref().unwrap().root(), root);
    assert_eq!(fan_rate(&editor), 3.0, "migrated in memory");
    assert!(editor.title().contains("(read-only)"), "{}", editor.title());

    // Saving is refused, with the way out in the message.
    editor.save();
    match editor.dialog() {
        Some(Dialog::Error { message, .. }) => {
            assert!(message.contains("read-only"), "{message}")
        }
        other => panic!("expected an error, got {other:?}"),
    }
    editor.dismiss_error();
    assert!(editor.state.save_scene().is_err());
    assert!(editor.state.new_material_asset("").is_err());
    assert_eq!(std::fs::read(scene_path(&root)).unwrap(), scene_before);
    assert!(
        !root.join("assets/settings").exists(),
        "no settings files are created"
    );
    assert!(!root.join(".box/backups").exists());
}

#[test]
fn upgrade_backs_up_migrates_and_opens_for_editing() {
    let root = old_project("upgrade");
    let scene_before = std::fs::read(scene_path(&root)).unwrap();
    let mut editor = editor_for(Some(root.clone()));
    editor.answer_upgrade(UpgradeChoice::Upgrade);
    assert_eq!(editor.dialog(), None);
    assert!(!editor.state.read_only);
    assert_eq!(fan_rate(&editor), 3.0);
    assert!(!editor.title().contains("read-only"));

    let scene = read_json(&scene_path(&root));
    let fan = scene["entities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "Fan")
        .unwrap();
    assert_eq!(
        fan["components"]["Spinner"],
        json!({ "$version": 2, "rate": 3.0 })
    );
    assert_eq!(
        read_json(&root.join("box.project.json"))["engine_version"],
        ENGINE_VERSION
    );
    let backups: Vec<PathBuf> = std::fs::read_dir(root.join(".box/backups"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(backups.len(), 1);
    assert_eq!(
        std::fs::read(backups[0].join("assets/scenes/main.scene.json")).unwrap(),
        scene_before
    );
    // Editable now; opening it again asks nothing.
    editor.state.save_scene().unwrap();
    editor.open_project(&root).unwrap();
    assert_eq!(editor.dialog(), None);
}

#[test]
fn a_project_from_a_newer_engine_is_refused() {
    let root = old_project("newer");
    let project_file = root.join("box.project.json");
    let mut pf = read_json(&project_file);
    pf["engine_version"] = json!("99.0.0");
    std::fs::write(&project_file, serde_json::to_vec_pretty(&pf).unwrap()).unwrap();
    let mut editor = editor_for(None);
    let err = editor.open_project(&root).unwrap_err();
    assert!(format!("{err:#}").contains("update box-engine"), "{err:#}");
    assert!(editor.state.project.is_none());
    // At start: an error dialog over the scratch scene.
    let editor = editor_for(Some(root));
    assert!(matches!(editor.dialog(), Some(Dialog::Error { .. })));
}

/// Renders the upgrade prompt to `target/captures/editor_upgrade_prompt.png`.
/// Skipped with `BOX_SKIP_GPU_TESTS` or without an adapter.
#[test]
fn upgrade_prompt_capture() {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        return;
    }
    let root = old_project("capture");
    let log = LogBuffer::default();
    log.set_manual_time(0.0);
    let options = EditorOptions {
        project: Some(root),
        ..Default::default()
    };
    let mut editor = build_game_editor(&options, log, game()).unwrap();
    editor.tree = layout::default_layout();
    let options = CaptureOptions {
        size: [1600, 1000],
        pixels_per_point: 2.0,
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
    assert!(matches!(editor.dialog(), Some(Dialog::Upgrade { .. })));
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures");
    std::fs::create_dir_all(&out).unwrap();
    image.save(out.join("editor_upgrade_prompt.png")).unwrap();
}
