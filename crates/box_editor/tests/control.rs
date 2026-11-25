//! The editor as a live control host: the discovery file's life cycle, the
//! Agent Control menu, saving agent edits, and the renderer's GPU mesh cache
//! across Play/Stop and scene switches. The full method conversation is in
//! `control_socket.rs` (its own binary: it captures logs).
//!
//! Screenshots and the GPU cache need a GPU; with `BOX_SKIP_GPU_TESTS` (or no
//! adapter) the editor runs its UI without rendering and `screenshot`
//! answers `unsupported`.

mod control_harness;

use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use box_core::scene;
use box_editor::headless::{CaptureOptions, Headless};
use box_editor::log::LogBuffer;
use box_editor::{EditorOptions, build_editor};
use box_engine::control::{self, ControlClient, read_discovery};
use box_render::RenderError;
use control_harness::{Frames, SCENE, project_editor, temp_dir};
use egui::pos2;
use serde_json::{Value as Json, json};

#[test]
fn agent_control_needs_a_project() {
    let options = EditorOptions {
        control: true,
        ..Default::default()
    };
    let mut editor = build_editor(&options, LogBuffer::default()).unwrap();
    assert!(editor.state.agent_control().is_none());
    assert!(!editor.state.set_agent_control(true));
}

#[test]
fn the_discovery_file_follows_the_scene_and_goes_with_the_project() {
    let (mut editor, root) = project_editor("discovery", LogBuffer::default());
    let file = control::discovery_path(&root);
    let d = read_discovery(&root).unwrap();
    assert_eq!(d.scene.as_deref(), Some(SCENE));
    assert_eq!(d.kind, control::HostKind::Editor);

    // Save As and Open Scene rewrite the scene.
    editor
        .state
        .save_scene_as(&root.join("assets/scenes/level2.scene.json"))
        .unwrap();
    assert_eq!(
        read_discovery(&root).unwrap().scene.as_deref(),
        Some("assets/scenes/level2.scene.json")
    );
    editor.state.open_scene(&root.join(SCENE), false).unwrap();
    assert_eq!(read_discovery(&root).unwrap().scene.as_deref(), Some(SCENE));

    // Turning it off removes the file; on again writes a new token.
    editor.state.set_agent_control(false);
    assert!(!file.exists());
    assert!(editor.state.set_agent_control(true));
    assert_ne!(read_discovery(&root).unwrap().token, d.token);

    // Another project: control was allowed for this one only.
    let other = temp_dir("discovery_other");
    box_engine::Project::create(&other, "other").unwrap();
    editor.open_project(&other).unwrap();
    assert!(editor.state.agent_control().is_none());
    assert!(!file.exists());
    assert!(!control::discovery_path(&other).exists());

    // Quit stops it too.
    editor.open_project(&root).unwrap();
    assert!(editor.state.set_agent_control(true));
    editor.on_quit();
    assert!(!file.exists());
}

/// The renderer frees the GPU meshes of a dropped play world on Stop, and of
/// a replaced scene on Open Scene: the cache does not grow with Play cycles.
#[test]
fn stop_and_scene_switches_free_gpu_meshes() {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping the GPU cache check");
        return;
    }
    let (mut editor, root) = project_editor("gpu_cache", LogBuffer::default());
    let mut frames = Frames::new(&mut editor);
    let Frames::Gpu(_) = &frames else {
        eprintln!("no GPU adapter; skipping the GPU cache check");
        return;
    };
    let cached = |frames: &Frames| match frames {
        Frames::Gpu(h) => h.renderer().cached_meshes(),
        Frames::Ui { .. } => unreachable!(),
    };
    for _ in 0..2 {
        frames.frame(&mut editor);
    }
    let edit = cached(&frames);
    assert!(edit > 0);
    let mut during = Vec::new();
    for _ in 0..3 {
        editor.state.start_play().unwrap();
        for _ in 0..3 {
            frames.frame(&mut editor);
        }
        during.push(cached(&frames));
        editor.state.stop_play();
        frames.frame(&mut editor);
        assert_eq!(cached(&frames), edit, "Stop frees the play world's meshes");
    }
    eprintln!("GPU meshes: edit {edit}, during Play {during:?}, after each Stop {edit}");
    // The cache holds only the meshes of the world being drawn: during Play
    // the play world's copies replace the edit world's.
    assert!(
        during.iter().all(|&n| n == edit),
        "only the shown world's meshes are cached: {during:?}"
    );

    // A scene with no meshes: the old scene's meshes go.
    let empty = root.join("assets/scenes/empty.scene.json");
    let world = box_core::prelude::World::with_registry(editor.state.game().registry());
    std::fs::write(&empty, scene::save(&world).unwrap()).unwrap();
    editor.state.open_scene(&empty, true).unwrap();
    frames.frame(&mut editor);
    assert_eq!(
        cached(&frames),
        0,
        "Open Scene frees the old scene's meshes"
    );
    editor
        .state
        .open_scene(Path::new(&root.join(SCENE)), true)
        .unwrap();
    frames.frame(&mut editor);
    assert_eq!(cached(&frames), edit);
}

/// The File menu with Allow Agent Control on, and the toolbar's Agent
/// indicator: `target/captures/editor_agent_menu.png`.
#[test]
fn capture_the_agent_control_menu() {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping the menu capture");
        return;
    }
    let (mut editor, root) = project_editor("menu", LogBuffer::default());
    let options = CaptureOptions {
        size: [2560, 1440],
        pixels_per_point: 2.0,
        frames: 1,
    };
    let mut h = match Headless::new(&mut editor, &options) {
        Ok(h) => h,
        Err(e)
            if matches!(
                e.downcast_ref::<RenderError>(),
                Some(RenderError::NoAdapter)
            ) =>
        {
            eprintln!("no GPU adapter; skipping the menu capture");
            return;
        }
        Err(e) => panic!("{e:#}"),
    };
    h.frame(&mut editor, Vec::new());
    let file = pos2(22.0, 18.0);
    let click = |pressed| egui::Event::PointerButton {
        pos: file,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    h.frame(&mut editor, vec![egui::Event::PointerMoved(file)]);
    h.frame(&mut editor, vec![click(true), click(false)]);
    // Past the menu's fade-in.
    for _ in 0..20 {
        h.frame(&mut editor, Vec::new());
    }
    assert!(editor.state.agent_control().is_some());
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures");
    std::fs::create_dir_all(&out).unwrap();
    h.image()
        .unwrap()
        .save(out.join("editor_agent_menu.png"))
        .unwrap();

    // The menu item turns it off (and removes the discovery file).
    // Below New/Open Project, Open Example, Open Recent, New Scene, Open Scene, Save, Save As.
    let item = pos2(80.0, 258.0);
    let click = |pressed| egui::Event::PointerButton {
        pos: item,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    h.frame(&mut editor, vec![egui::Event::PointerMoved(item)]);
    h.frame(&mut editor, vec![click(true), click(false)]);
    assert!(editor.state.agent_control().is_none());
    assert!(!control::discovery_path(&root).exists());
}

/// W1: `apply_commands` with `save` writes an agent's fix to the scene file
/// once: at once when editing, at Stop when made during Play.
#[test]
fn saved_agent_edits_reach_the_scene_file() {
    let (mut editor, root) = project_editor("save", LogBuffer::default());
    let file = editor
        .state
        .scene
        .path
        .clone()
        .expect("the start scene's file");
    let mut frames = Frames::new(&mut editor);
    frames.frame(&mut editor);
    let conversation = thread::spawn(move || {
        let mut c = ControlClient::connect_project(&root, Duration::from_secs(60)).unwrap();
        let mut call = |method: &str, params: Json| {
            c.call(method, params)
                .unwrap_or_else(|e| json!({ "error": e.to_control_error().to_json() }))
        };
        let mut r = serde_json::Map::new();
        r.insert(
            "edit".into(),
            call(
                "apply_commands",
                json!({ "commands": { "op": "spawn", "name": "SavedLamp" }, "save": true }),
            ),
        );
        r.insert(
            "both".into(),
            call(
                "apply_commands",
                json!({ "commands": { "op": "spawn", "name": "X" }, "save": true, "dry_run": true }),
            ),
        );
        call("play", json!({}));
        r.insert(
            "play".into(),
            call(
                "apply_commands",
                json!({ "commands": { "op": "spawn", "name": "PlayFix" }, "save": true }),
            ),
        );
        Json::Object(r)
    });
    let r = frames.drive(&mut editor, conversation);
    assert_eq!(r["edit"]["saved"], true, "{r:#}");
    assert_eq!(r["both"]["error"]["code"], "invalid_params");
    assert_eq!(r["play"]["saved_on_stop"], true, "{r:#}");
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("SavedLamp"));
    assert!(!text.contains("PlayFix"), "saved only at Stop");
    // The play world has the fix now.
    assert!(editor.state.world.find_by_name("PlayFix").is_some());
    editor.state.stop_play();
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("PlayFix"), "saved at Stop");
    assert!(
        editor.state.world.find_by_name("PlayFix").is_some(),
        "in the edit world"
    );
    assert!(!editor.state.is_dirty());
}
