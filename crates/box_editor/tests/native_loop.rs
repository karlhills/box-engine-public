//! Fast iteration for native games (docs/DX_REVIEW.md, Batch 2): Game ▸
//! Rebuild and Restart (with a stand-in build command and restart hook),
//! Game ▸ Project Settings (settings resources edited in their files, hot
//! reload, Play), and captures of both. The last test needs a GPU.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use box_core::identity::Uid;
use box_core::prelude::*;
use box_core::reflect::{Reflect, ReflectDefault, ReflectResource};
use box_editor::log::LogBuffer;
use box_editor::rebuild::{AFTER_REBUILD_FLAG, RebuildTarget, RestartCommand, shell_command};
use box_editor::{Editor, EditorOptions, NewEntity, build_game_editor};
use box_engine::Game;
use egui::accesskit::TreeUpdate;
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, pos2};
use tracing_subscriber::layer::SubscriberExt;

/// Character tunables: a project settings resource.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Resource, Default, Clone)]
#[type_path = "iteration_test"]
struct Tuning {
    /// Walking speed.
    #[reflect(@box_core::reflect::Range { min: 0.0, max: 30.0 }, @box_core::reflect::Unit("m/s"))]
    speed: f32,
    /// Jump height.
    #[reflect(@box_core::reflect::Range { min: 0.0, max: 5.0 }, @box_core::reflect::Unit("m"))]
    jump: f32,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            speed: 6.0,
            jump: 1.2,
        }
    }
}

struct TuningGame;
impl Plugin for TuningGame {
    fn register(&self, registry: &mut Registry) {
        registry.register_settings::<Tuning>().unwrap();
    }
    fn build(&self, _app: &mut App) {}
}

fn game() -> Game {
    Game::new().with_plugin(TuningGame)
}

fn temp_project(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("box_iteration_{name}_{}", Uid::new()));
    let dir = base.join(name);
    box_engine::Project::create(&dir, name).unwrap();
    dir.canonicalize().unwrap()
}

fn editor(project: &Path, log: &LogBuffer, after_rebuild: bool) -> Editor {
    let options = EditorOptions {
        project: Some(project.to_path_buf()),
        after_rebuild,
        ..Default::default()
    };
    build_game_editor(&options, log.clone(), game()).unwrap()
}

/// A build that prints `stdout` (cargo JSON lines) and exits with `code`.
fn stand_in(project: &Path, stdout: &[&str], code: i32) -> RebuildTarget {
    let mut script = String::new();
    for line in stdout {
        script.push_str(&format!("printf '%s\\n' '{line}'; "));
    }
    script.push_str(&format!("echo '   Compiling game v0.1.0' >&2; exit {code}"));
    RebuildTarget {
        dir: project.to_path_buf(),
        bin: "editor".into(),
        command: shell_command(&script),
        args: vec![project.display().to_string(), AFTER_REBUILD_FLAG.into()],
    }
}

/// Polls the build until it ends.
fn finish(editor: &mut Editor) {
    let start = Instant::now();
    while editor.poll_rebuild() {
        assert!(start.elapsed() < Duration::from_secs(20), "build hangs");
        std::thread::sleep(Duration::from_millis(10));
    }
}

type Restarts = Arc<Mutex<Vec<RestartCommand>>>;

fn recording_hook(editor: &mut Editor) -> Restarts {
    let restarts: Restarts = Arc::default();
    let r = restarts.clone();
    editor.set_restart_hook(Box::new(move |cmd| {
        r.lock().unwrap().push(cmd.clone());
        std::io::Error::other("test: not restarting")
    }));
    restarts
}

const ERROR_JSON: &str = r#"{"reason":"compiler-message","message":{"level":"error","message":"mismatched types","spans":[{"file_name":"src/lib.rs","line_start":3,"column_start":5,"is_primary":true}],"rendered":"error[E0308]: mismatched types\n --> src/lib.rs:3:5"}}"#;

#[test]
fn rebuild_and_restart_keeps_the_scene_camera_selection_and_unsaved_changes() {
    if cfg!(windows) {
        return; // the stand-in build is a POSIX shell script
    }
    let project = temp_project("rebuild_ok");
    let log = LogBuffer::default();
    // tracing caches a callsite's interest when it is first hit: without a
    // subscriber here, the build-line callsite would read as disabled for the
    // failed-rebuild test running at the same time.
    let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(log.layer()));
    let mut ed = editor(&project, &log, false);
    // Not this project's editor binary: unavailable, with the reason.
    let reason = ed
        .state
        .actions()
        .get("game.rebuild")
        .unwrap()
        .availability(&ed.state)
        .unwrap_err();
    assert!(reason.contains("no Rust crate"), "{reason}");

    let sphere = ed.state.add_entity(NewEntity::Sphere, None).unwrap();
    ed.state.select(Some(sphere));
    ed.state.camera.orbit.yaw = 1.25;
    ed.state.camera.orbit.distance = 7.5;
    assert!(ed.state.is_dirty());
    let exe = project.join("target/debug/editor");
    let artifact = format!(
        r#"{{"reason":"compiler-artifact","target":{{"name":"editor"}},"executable":"{}"}}"#,
        exe.display()
    );
    ed.state
        .set_rebuild_target(stand_in(&project, &[&artifact], 0));
    assert!(
        ed.state
            .actions()
            .get("game.rebuild")
            .unwrap()
            .availability(&ed.state)
            .is_ok()
    );
    let restarts = recording_hook(&mut ed);
    ed.state.run_action("game.rebuild").unwrap();
    ed.begin_rebuild();
    assert!(ed.state.rebuild_running());
    finish(&mut ed);
    let restarts = restarts.lock().unwrap().clone();
    assert_eq!(restarts.len(), 1, "restarted once");
    assert_eq!(restarts[0].exe, exe, "into the binary cargo built");
    assert_eq!(restarts[0].dir, project);
    assert!(restarts[0].args.contains(&AFTER_REBUILD_FLAG.to_string()));
    assert!(
        ed.state.recovery_candidate().is_some(),
        "unsaved changes kept"
    );

    // The restarted editor: same scene, camera and selection, and the
    // unsaved changes without a recovery prompt.
    let ed2 = editor(&project, &log, true);
    assert!(
        ed2.dialog().is_none(),
        "no recovery prompt: {:?}",
        ed2.dialog()
    );
    assert!(ed2.state.is_dirty());
    assert!(ed2.state.world.entity_by_uid(sphere).is_some());
    assert_eq!(ed2.state.selection, vec![sphere]);
    assert_eq!(ed2.state.camera.orbit.yaw, 1.25);
    assert_eq!(ed2.state.camera.orbit.distance, 7.5);
    // A normal start asks about the recovery file instead.
    let ed3 = editor(&project, &log, false);
    assert!(matches!(
        ed3.dialog(),
        Some(box_editor::Dialog::Recovery { .. })
    ));
    std::fs::remove_dir_all(project.parent().unwrap()).unwrap();
}

#[test]
fn a_failed_rebuild_toasts_the_compiler_errors_and_keeps_editing() {
    if cfg!(windows) {
        return;
    }
    let project = temp_project("rebuild_fail");
    let log = LogBuffer::default();
    let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(log.layer()));
    let mut ed = editor(&project, &log, false);
    ed.state
        .set_rebuild_target(stand_in(&project, &[ERROR_JSON], 101));
    let restarts = recording_hook(&mut ed);
    ed.begin_rebuild();
    finish(&mut ed);
    ed.state.poll_log();
    assert!(restarts.lock().unwrap().is_empty(), "no restart");
    assert!(!ed.state.rebuild_running());
    let toasts: Vec<String> = ed
        .state
        .toasts()
        .items()
        .iter()
        .map(|t| t.text.clone())
        .collect();
    assert!(
        toasts
            .iter()
            .any(|t| t == "src/lib.rs:3:5: mismatched types"),
        "{toasts:?}"
    );
    assert!(
        toasts
            .iter()
            .any(|t| t.starts_with("Rebuild failed: 1 compiler error")),
        "{toasts:?}"
    );
    // The full diagnostic and cargo's progress are in the Console.
    let console: Vec<String> = log
        .lock()
        .records()
        .iter()
        .map(|r| r.message.clone())
        .collect();
    assert!(
        console.iter().any(|m| m.contains("error[E0308]")),
        "{console:?}"
    );
    assert!(
        console.iter().any(|m| m.contains("Compiling game")),
        "{console:?}"
    );
    // Still editing.
    assert!(ed.state.add_entity(NewEntity::Cube, None).is_some());
    std::fs::remove_dir_all(project.parent().unwrap()).unwrap();
}

#[test]
fn project_settings_are_created_edited_in_their_file_and_hot_reload() {
    let project = temp_project("settings");
    let log = LogBuffer::default();
    let mut ed = editor(&project, &log, false);
    let file = project.join("assets/settings/Tuning.json");
    // The editor writes the defaults for a settings type without a file.
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("\"speed\": 6.0"), "{text}");
    assert_eq!(*ed.state.world.resource::<Tuning>(), Tuning::default());
    // Not in the scene, nor in Scene Settings.
    let bytes = box_core::scene::save(&ed.state.world).unwrap();
    assert!(!String::from_utf8(bytes).unwrap().contains("Tuning"));
    let sections = box_editor::inspect::resource_sections(&ed.state.world);
    assert!(sections.iter().all(|s| s.component != "Tuning"));

    // Game ▸ Project Settings ▸ Tuning (next to the engine's audio volumes
    // and physics settings).
    assert_eq!(
        ed.state.settings_types(),
        ["AudioSettings", "PhysicsSettings", "Tuning"]
    );
    ed.state.run_action("settings.Tuning").unwrap();
    assert_eq!(ed.state.project_settings_shown(), Some("Tuning"));
    assert!(ed.state.edit_settings("Tuning.speed", &12.0f32, 0.0));
    assert_eq!(ed.state.world.resource::<Tuning>().speed, 12.0);
    ed.state.flush_settings_edit(0.1, false);
    assert!(
        std::fs::read_to_string(&file)
            .unwrap()
            .contains("\"speed\": 6.0"),
        "not written during a drag"
    );
    ed.state.flush_settings_edit(1.0, true);
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("\"speed\": 12.0"), "{text}");
    assert!(!ed.state.is_dirty(), "a file edit, not a scene edit");
    assert!(!ed.state.edits.can_undo());

    // Play reads the file; an edit during Play reaches both worlds.
    ed.state.start_play().unwrap();
    assert_eq!(ed.state.world.resource::<Tuning>().speed, 12.0);
    assert!(ed.state.edit_settings("Tuning.jump", &2.5f32, 2.0));
    ed.state.flush_settings_edit(3.0, true);
    ed.state.stop_play();
    assert_eq!(
        ed.state.world.resource::<Tuning>().jump,
        2.5,
        "kept after Stop"
    );

    // Changed outside the editor: hot reload.
    std::fs::write(&file, r#"{ "speed": 3.0, "jump": 0.5 }"#).unwrap();
    let start = Instant::now();
    while ed.state.world.resource::<Tuning>().speed != 3.0 {
        assert!(start.elapsed() < Duration::from_secs(10), "not reloaded");
        ed.state.update_assets();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(ed.state.world.resource::<Tuning>().jump, 0.5);
    // Selecting an entity goes back to the entity inspector.
    let cube = ed.state.add_entity(NewEntity::Cube, None).unwrap();
    ed.state.select(Some(cube));
    assert_eq!(ed.state.project_settings_shown(), None);
    std::fs::remove_dir_all(project.parent().unwrap()).unwrap();
}

#[test]
fn write_registry_writes_the_games_types() {
    let project = temp_project("registry");
    let options = EditorOptions {
        project: Some(project.clone()),
        write_registry: true,
        ..Default::default()
    };
    let path = box_editor::write_registry(&options, &game()).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let tuning = json["resources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "Tuning")
        .unwrap();
    assert_eq!(tuning["settings"], true);
    assert_eq!(tuning["file"], "assets/settings/Tuning.json");
    assert_eq!(json["engine_version"], box_engine::project::ENGINE_VERSION);
    std::fs::remove_dir_all(project.parent().unwrap()).unwrap();
}

// ----- captures -----

fn texts(update: Option<&TreeUpdate>) -> Vec<(String, Rect)> {
    let Some(u) = update else {
        return Vec::new();
    };
    u.nodes
        .iter()
        .filter_map(|(_, n)| {
            let b = n.bounds()?;
            Some((
                n.label().or(n.value()).unwrap_or_default().to_string(),
                Rect::from_min_max(
                    pos2(b.x0 as f32, b.y0 as f32),
                    pos2(b.x1 as f32, b.y1 as f32),
                ),
            ))
        })
        .collect()
}

/// Captures for review in `target/captures/iteration/`: the Game menu with
/// Rebuild and Restart, a build error toast, and the Project Settings
/// inspector.
#[test]
fn iteration_captures() {
    use box_editor::headless::{CaptureOptions, Headless};
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() || cfg!(windows) {
        eprintln!("skipping captures");
        return;
    }
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/iteration");
    std::fs::create_dir_all(&out).unwrap();
    let project = temp_project("Meadow");
    let log = LogBuffer::default();
    let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(log.layer()));
    let mut ed = editor(&project, &log, false);
    ed.tree = box_editor::layout::default_layout();
    ed.state
        .set_rebuild_target(stand_in(&project, &[ERROR_JSON], 101));
    let options = CaptureOptions {
        size: [2560, 1600],
        pixels_per_point: 2.0,
        frames: 1,
    };
    let mut h = match Headless::new(&mut ed, &options) {
        Ok(h) => h,
        Err(e)
            if matches!(
                e.downcast_ref::<box_render::RenderError>(),
                Some(box_render::RenderError::NoAdapter)
            ) =>
        {
            eprintln!("no GPU adapter; skipping captures");
            return;
        }
        Err(e) => panic!("{e:#}"),
    };
    h.enable_accesskit();
    let run = |h: &mut Headless, ed: &mut Editor, events: Vec<Event>| {
        h.frame(ed, events);
        texts(h.accesskit_update())
    };
    let find = |nodes: &[(String, Rect)], text: &str| -> Pos2 {
        nodes
            .iter()
            .rev()
            .find(|(t, _)| t == text || t.starts_with(&format!("{text} ")))
            .unwrap_or_else(|| panic!("no `{text}`"))
            .1
            .center()
    };
    let click = |h: &mut Headless, ed: &mut Editor, pos: Pos2| {
        let press = |pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        run(h, ed, vec![Event::PointerMoved(pos)]);
        run(h, ed, vec![press(true), press(false)]);
        let mut n = Vec::new();
        for _ in 0..10 {
            n = run(h, ed, Vec::new());
        }
        n
    };
    let save = |h: &Headless, name: &str| h.image().unwrap().save(out.join(name)).unwrap();
    let mut nodes = Vec::new();
    for _ in 0..3 {
        nodes = run(&mut h, &mut ed, Vec::new());
    }
    // Game menu, then its Project Settings submenu.
    let n = click(&mut h, &mut ed, find(&nodes, "Game"));
    let n = click(&mut h, &mut ed, find(&n, "Project Settings"));
    let _ = n;
    save(&h, "game_menu.png");
    let esc = |pressed| Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: Modifiers::NONE,
    };
    run(&mut h, &mut ed, vec![esc(true), esc(false)]);
    for _ in 0..3 {
        run(&mut h, &mut ed, Vec::new());
    }

    // Project Settings in the inspector.
    ed.state.run_action("settings.Tuning").unwrap();
    for _ in 0..3 {
        run(&mut h, &mut ed, Vec::new());
    }
    save(&h, "project_settings.png");

    // A failed rebuild: the error toast.
    ed.begin_rebuild();
    let start = Instant::now();
    while ed.state.rebuild_running() {
        assert!(start.elapsed() < Duration::from_secs(20));
        run(&mut h, &mut ed, Vec::new());
        std::thread::sleep(Duration::from_millis(10));
    }
    for _ in 0..3 {
        run(&mut h, &mut ed, Vec::new());
    }
    save(&h, "build_error.png");
    eprintln!("captures in {}", out.display());
    std::fs::remove_dir_all(project.parent().unwrap()).unwrap();
}
