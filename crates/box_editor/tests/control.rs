//! The editor as a live control host: a headless editor with Allow Agent
//! Control on, driven by a `ControlClient` on another thread while the main
//! thread keeps running editor frames. Also the discovery file's life cycle
//! and the renderer's GPU mesh cache across Play/Stop and scene switches.
//!
//! Screenshots and the GPU cache need a GPU; with `BOX_SKIP_GPU_TESTS` (or no
//! adapter) the editor runs its UI without rendering and `screenshot`
//! answers `unsupported`.

use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use box_core::identity::Uid;
use box_core::scene;
use box_editor::headless::{CaptureOptions, Headless};
use box_editor::log::LogBuffer;
use box_editor::{Editor, EditorOptions, PlayState, build_editor, layout};
use box_engine::control::{self, ControlClient, read_discovery};
use box_render::RenderError;
use egui::{RawInput, Rect, pos2, vec2};
use serde_json::{Value as Json, json};
use tracing_subscriber::layer::SubscriberExt;

const SCENE: &str = "assets/scenes/main.scene.json";

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("box_editor_ctl_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn project_editor(name: &str, log: LogBuffer) -> (Editor, PathBuf) {
    let dir = temp_dir(name);
    box_engine::Project::create(&dir, name).unwrap();
    let options = EditorOptions {
        project: Some(dir.clone()),
        control: true,
        ..Default::default()
    };
    let mut editor = build_editor(&options, log).unwrap();
    editor.tree = layout::default_layout();
    editor.state.autosave_dir = dir.join("autosave");
    let root = editor.state.project.as_ref().unwrap().root().to_path_buf();
    (editor, root)
}

/// Runs editor frames on this thread: rendered with a GPU, UI only without.
enum Frames {
    Gpu(Box<Headless>),
    Ui { ctx: egui::Context, frame: u32 },
}

impl Frames {
    fn new(editor: &mut Editor) -> Self {
        if std::env::var_os("BOX_SKIP_GPU_TESTS").is_none() {
            let options = CaptureOptions {
                size: [1280, 720],
                pixels_per_point: 1.0,
                frames: 1,
            };
            match Headless::new(editor, &options) {
                Ok(h) => return Self::Gpu(Box::new(h)),
                Err(e)
                    if matches!(
                        e.downcast_ref::<RenderError>(),
                        Some(RenderError::NoAdapter)
                    ) => {}
                Err(e) => panic!("{e:#}"),
            }
        }
        eprintln!("no GPU (or BOX_SKIP_GPU_TESTS): running the editor UI without rendering");
        let ctx = egui::Context::default();
        editor.apply_theme(&ctx);
        Self::Ui { ctx, frame: 0 }
    }

    fn gpu(&self) -> bool {
        matches!(self, Self::Gpu(_))
    }

    fn frame(&mut self, editor: &mut Editor) {
        match self {
            Self::Gpu(h) => {
                h.frame(editor, Vec::new());
            }
            Self::Ui { ctx, frame } => {
                editor.poll_control(None);
                let raw = RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 720.0))),
                    time: Some(*frame as f64 / 60.0),
                    focused: true,
                    ..Default::default()
                };
                let _ = ctx.run_ui(raw, |ui| editor.ui(ui));
                *frame += 1;
            }
        }
    }

    /// Runs frames until `conversation` finishes; returns its result.
    fn drive<T>(&mut self, editor: &mut Editor, conversation: thread::JoinHandle<T>) -> T {
        let start = Instant::now();
        while !conversation.is_finished() {
            self.frame(editor);
            thread::sleep(Duration::from_millis(1));
            assert!(
                start.elapsed() < Duration::from_secs(120),
                "conversation hung"
            );
        }
        conversation.join().unwrap()
    }
}

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

/// Every method over the socket, with the editor's bookkeeping checked after.
#[test]
fn a_client_queries_edits_screenshots_and_plays_the_editor() {
    let log = LogBuffer::default();
    let _subscriber =
        tracing::subscriber::set_default(tracing_subscriber::registry().with(log.layer()));
    let (mut editor, root) = project_editor("socket", log);
    let initial = scene::save(&editor.state.world).unwrap();
    let initial_count = editor.state.world.entity_count();
    let mut frames = Frames::new(&mut editor);
    let gpu = frames.gpu();
    // Lay the viewport out so screenshots have a current size.
    frames.frame(&mut editor);

    let conversation = thread::spawn(move || {
        let mut c = ControlClient::connect_project(&root, Duration::from_secs(60)).unwrap();
        let mut r = serde_json::Map::new();
        let mut call = |name: &str, method: &str, params: Json| {
            let v = match c.call(method, params) {
                Ok(v) => v,
                Err(e) => json!({ "error": e.to_control_error().to_json() }),
            };
            r.insert(name.into(), v);
        };
        call("status", "status", json!({}));
        call(
            "query",
            "world_query",
            json!({ "include_components": false }),
        );
        call(
            "apply",
            "apply_commands",
            json!({ "commands": [
                { "op": "spawn", "name": "Lamp", "components": {
                    "Primitive": { "shape": { "Sphere": { "radius": 0.3, "sectors": 16, "stacks": 8 } } } } },
                { "op": "set", "entity": "Lamp", "path": "Transform.translation.y", "value": 2.0 }
            ] }),
        );
        call(
            "dry",
            "apply_commands",
            json!({ "commands": { "op": "despawn", "entity": "Lamp" }, "dry_run": true }),
        );
        call(
            "apply_err",
            "apply_commands",
            json!({ "commands": { "op": "rename", "entity": "Nobody", "name": "x" } }),
        );
        call("lamp", "world_query", json!({ "name": "lamp" }));
        call("shot", "screenshot", json!({ "width": 320, "height": 180 }));
        call("shot_default", "screenshot", json!({}));
        call("pause_edit", "pause", json!({}));
        call("play", "play", json!({}));
        call("play_query", "world_query", json!({ "name": "lamp" }));
        call(
            "play_apply",
            "apply_commands",
            json!({ "commands": { "op": "rename", "entity": "Lamp", "name": "Play Lamp" } }),
        );
        call("pause", "pause", json!({}));
        call("step", "step_frames", json!({ "frames": 5 }));
        call("paused", "status", json!({}));
        call(
            "play_shot",
            "screenshot",
            json!({ "width": 64, "height": 48 }),
        );
        call("stats", "frame_stats", json!({}));
        call("stop", "stop", json!({}));
        call("log", "read_log", json!({ "since": 0, "limit": 10000 }));
        r
    });
    let r = frames.drive(&mut editor, conversation);
    let shot_size = |v: &Json| {
        let png = control::base64_decode(v["png_base64"].as_str().unwrap()).unwrap();
        let img = image::load_from_memory(&png).unwrap();
        assert_eq!(
            (img.width() as u64, img.height() as u64),
            (v["width"].as_u64().unwrap(), v["height"].as_u64().unwrap())
        );
        (img.width(), img.height())
    };

    assert_eq!(r["status"]["kind"], "editor");
    assert_eq!(r["status"]["mode"], "edit");
    assert_eq!(r["status"]["scene"], SCENE);
    assert_eq!(r["status"]["undo"], 0);
    assert_eq!(r["query"]["world"], "edit");
    assert_eq!(r["query"]["total"], initial_count);

    assert_eq!(r["apply"]["world"], "edit");
    assert_eq!(r["apply"]["undoable"], true);
    assert_eq!(r["apply"]["created"].as_array().unwrap().len(), 1);
    assert_eq!(r["dry"]["dry_run"], true);
    assert_eq!(r["apply_err"]["error"]["code"], "entity_not_found");
    assert_eq!(r["lamp"]["total"], 1, "the dry run changed nothing");
    assert_eq!(
        r["lamp"]["entities"][0]["components"]["Transform"]["translation"][1],
        2.0
    );

    if gpu {
        assert_eq!(shot_size(&r["shot"]), (320, 180));
        // No size: the viewport at its current size.
        let viewport = editor.state.viewport.target_px;
        assert!(viewport[0] > 0);
        assert_eq!(shot_size(&r["shot_default"]), (viewport[0], viewport[1]));
        assert_eq!(shot_size(&r["play_shot"]), (64, 48));
        let png = control::base64_decode(r["shot"]["png_base64"].as_str().unwrap()).unwrap();
        let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("editor_control_screenshot.png"), png).unwrap();
        assert_eq!(r["stats"]["renderer"]["validation_errors"], 0);
    } else {
        assert_eq!(r["shot"]["error"]["code"], "unsupported");
        assert_eq!(r["stats"]["renderer"], Json::Null);
    }

    assert_eq!(r["pause_edit"]["error"]["code"], "not_playing");
    assert_eq!(r["play"]["mode"], "play");
    assert_eq!(r["play_query"]["world"], "play");
    assert_eq!(r["play_query"]["total"], 1, "the play world has the edit");
    assert_eq!(r["play_apply"]["world"], "play");
    assert_eq!(r["play_apply"]["undoable"], false);
    assert_eq!(r["play_apply"]["discarded_on_stop"], true);
    assert_eq!(r["pause"]["mode"], "paused");
    let before_step = r["pause"]["frame"].as_u64().unwrap();
    assert_eq!(r["step"]["frames"], 5);
    assert_eq!(r["step"]["frame"].as_u64().unwrap(), before_step + 5);
    assert_eq!(r["paused"]["mode"], "paused", "stepping keeps the pause");
    assert_eq!(r["paused"]["frame"], r["step"]["frame"]);
    assert_eq!(r["paused"]["undo"], 1, "the edit world's history");
    assert_eq!(r["stats"]["world"], "play");
    assert_eq!(r["stop"]["mode"], "edit");
    let messages: Vec<&str> = r["log"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["message"].as_str().unwrap())
        .collect();
    assert!(
        messages
            .iter()
            .any(|m| m.contains("agent applied 2 command(s) to the edit world")),
        "{messages:?}"
    );
    assert!(messages.iter().any(|m| m.contains("agent pressed Play")));
    assert!(r["log"]["next"].as_u64().unwrap() >= messages.len() as u64);

    // Editing again, with the agent's edit as one undo step on the edit world.
    assert_eq!(editor.state.play_state(), PlayState::Editing);
    assert_eq!(editor.state.edits.undo_len(), 1);
    assert_eq!(editor.state.edits.undo_label(), Some("Agent edit"));
    assert!(editor.state.is_dirty());
    assert!(editor.title().contains(" •"), "{}", editor.title());
    let lamp = box_core::edit::resolve_entity(&editor.state.world, "Lamp").unwrap();
    assert!(
        box_core::edit::resolve_entity(&editor.state.world, "Play Lamp").is_err(),
        "the Play rename was discarded"
    );
    frames.frame(&mut editor);
    assert!(
        hierarchy_has(&editor, lamp),
        "the hierarchy shows the agent's entity"
    );
    // Cmd+Z undoes it: back to the scene as loaded.
    editor.state.undo();
    assert_eq!(scene::save(&editor.state.world).unwrap(), initial);
}

fn hierarchy_has(editor: &Editor, uid: Uid) -> bool {
    editor.state.hierarchy.rows.iter().any(|r| r.uid == uid)
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
