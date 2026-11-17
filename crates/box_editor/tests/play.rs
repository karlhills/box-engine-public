//! Play mode, run headlessly: the round trip (Play, mutate, Stop leaves the
//! edit scene byte-identical with its undo history), fault injection (a game
//! panic stops Play, the edit world and autosave are intact), pause and step,
//! game types through the editor's registry, game input forwarding, and a
//! capture of the editor in Play mode.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use box_core::app::FixedTime;
use box_core::identity::{Name, Uid};
use box_core::input::{Input, KeyCode};
use box_core::prelude::*;
use box_core::reflect::{Reflect, ReflectComponent, ReflectDefault};
use box_core::scene;
use box_editor::headless::{CaptureOptions, Headless};
use box_editor::log::LogBuffer;
use box_editor::{
    DocAction, Editor, EditorOptions, GamePanic, NewEntity, PlayError, PlayPhase, PlayState,
    TransformTool, build_game_editor, layout,
};
use box_engine::Game;
use box_engine::scenes::Scenes;
use box_render::RenderError;
use egui::{Event, Key, Modifiers, PointerButton, RawInput, Rect, pos2, vec2};
use tracing_subscriber::layer::SubscriberExt;

const DT: f32 = 1.0 / 60.0;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("box_editor_play_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A game component, registered by a plugin.
#[derive(Reflect, Default, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone)]
#[type_path = "play_test"]
struct Coin {
    value: u32,
}

/// Moves every transform, spawns a saved entity per frame, despawns the
/// first root and renames another: plenty for Stop to undo.
struct Mutator;
impl Plugin for Mutator {
    fn register(&self, registry: &mut box_core::reflect::Registry) {
        registry.register::<Coin>().unwrap();
    }
    fn build(&self, app: &mut App) {
        app.add_system(Stage::Update, |w| {
            let dt = w.resource::<Time>().delta;
            for t in w.query_mut::<&mut Transform>() {
                t.translation.y += dt;
            }
            let frame = w.resource::<Time>().frame;
            w.spawn_saved(
                Uid::new(),
                &format!("Spawned {frame}"),
                (Coin { value: 1 },),
            )
            .unwrap();
            if frame == 2 {
                let first = w.roots()[0];
                w.despawn(first);
            }
            if let Some(n) = w.query_mut::<&mut Name>().into_iter().next() {
                n.0 = "Renamed by the game".into();
            }
        });
    }
}

/// Lifts the glazed ball a little each frame (for the capture).
struct Lift;
impl Plugin for Lift {
    fn build(&self, app: &mut App) {
        app.add_system(Stage::Update, |w| {
            let dt = w.resource::<Time>().delta;
            for (n, t) in w.query_mut::<(&Name, &mut Transform)>() {
                if n.as_str() == "Glazed Ball" {
                    t.translation.y += 30.0 * dt;
                }
            }
        });
    }
}

/// Panics in `Update` on frame `0`.
struct Bomb(u64);
impl Plugin for Bomb {
    fn build(&self, app: &mut App) {
        let at = self.0;
        app.add_system(Stage::Update, move |w| {
            if w.resource::<Time>().frame == at {
                panic!("boom at frame {at}");
            }
        });
    }
}

/// Panics while the play app is built.
struct BadBuild;
impl Plugin for BadBuild {
    fn build(&self, _app: &mut App) {
        panic!("cannot build");
    }
}

#[derive(Default)]
struct FixedSteps(u32);

/// Counts fixed steps and records the keys and buttons it saw.
#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<String>>>);
impl Plugin for Recorder {
    fn build(&self, app: &mut App) {
        app.world.insert_resource(FixedSteps::default());
        app.add_system(Stage::FixedUpdate, |w| {
            w.resource_mut::<FixedSteps>().0 += 1
        });
        let seen = self.0.clone();
        app.add_system(Stage::Update, move |w| {
            let input = w.resource::<Input>();
            let mut seen = seen.lock().unwrap();
            for k in [KeyCode::W, KeyCode::E, KeyCode::Space] {
                if input.key_pressed(k) {
                    seen.push(format!("press {k:?}"));
                }
            }
            if input.button_pressed(box_core::input::MouseButton::Left) {
                seen.push("click".into());
            }
        });
    }
}

fn editor_for(game: Game, project: Option<&Path>, autosave: &Path) -> Editor {
    let options = EditorOptions {
        project: project.map(Path::to_path_buf),
        ..Default::default()
    };
    let mut editor = build_game_editor(&options, LogBuffer::default(), game).unwrap();
    editor.state.autosave_dir = autosave.to_path_buf();
    editor
}

fn snapshot(editor: &Editor) -> Vec<u8> {
    scene::save(&editor.state.world).unwrap()
}

#[test]
fn play_round_trip_leaves_the_edit_scene_and_history_unchanged() {
    let dir = temp_dir("round_trip");
    let mut editor = editor_for(Game::new().with_plugin(Mutator), None, &dir);
    let cube = editor.state.add_entity(NewEntity::Cube, None).unwrap();
    let placed = {
        let e = editor.state.world.entity_by_uid(cube).unwrap();
        editor.state.world.get::<Transform>(e).unwrap().translation
    };
    editor
        .state
        .set_field(cube, "Transform.translation", &Vec3::new(1.0, 2.0, 3.0));
    let before = snapshot(&editor);
    let (undo, redo) = (editor.state.edits.undo_len(), editor.state.edits.redo_len());
    let label = editor.state.edits.undo_label().map(str::to_owned);
    assert!(editor.state.is_dirty());

    editor.state.start_play().unwrap();
    assert_eq!(editor.state.play_state(), PlayState::Playing);
    // The scratch scene has no file: its autosave is in the autosave folder.
    let autosave = editor.state.play_autosave().unwrap().to_path_buf();
    assert_eq!(autosave, dir.join("scratch.scene.json"));
    assert_eq!(std::fs::read(&autosave).unwrap(), before);
    let count = editor.state.world.entity_count();
    for _ in 0..10 {
        assert!(editor.state.update_play(DT).unwrap());
    }
    assert_eq!(editor.state.play_session().unwrap().frames(), 10);
    // The panes see the play world, which the game changed.
    assert_ne!(snapshot(&editor), before);
    assert_eq!(editor.state.world.entity_count(), count + 10 - 1);
    // An edit during Play changes the play world only, with no undo step.
    assert!(!editor.state.edits.can_undo());
    editor.state.rename(cube, "Changed during Play");
    editor.state.delete_selection();
    // Still dirty: that is the edit scene's state, not the play world's.
    assert!(editor.state.is_dirty());
    assert!(editor.title().ends_with("Playing"), "{}", editor.title());

    editor.state.stop_play();
    assert_eq!(editor.state.play_state(), PlayState::Editing);
    assert_eq!(
        snapshot(&editor),
        before,
        "the edit scene must be byte-identical"
    );
    assert_eq!(editor.state.edits.undo_len(), undo);
    assert_eq!(editor.state.edits.redo_len(), redo);
    assert_eq!(editor.state.edits.undo_label().map(str::to_owned), label);
    assert!(editor.state.is_dirty());
    // The edit world's undo history still works on the edit world.
    editor.state.undo();
    let e = editor.state.world.entity_by_uid(cube).unwrap();
    assert_eq!(
        editor.state.world.get::<Transform>(e).unwrap().translation,
        placed
    );
    assert!(
        !autosave.exists(),
        "a scene without a project drops its autosave on Stop"
    );
}

#[test]
fn play_keeps_the_edit_scenes_dirty_state_and_undo_to_saved_is_clean_after() {
    let dir = temp_dir("dirty_across_play");
    let mut editor = editor_for(Game::new().with_plugin(Mutator), None, &dir);
    assert!(!editor.state.is_dirty());

    // Clean scene: edits made during Play never dirty it.
    editor.state.start_play().unwrap();
    let cube = editor.state.add_entity(NewEntity::Cube, None).unwrap();
    editor.state.rename(cube, "Play only");
    editor.state.update_play(DT).unwrap();
    assert!(!editor.state.is_dirty());
    editor.state.stop_play();
    assert!(!editor.state.is_dirty());

    // Dirty scene stays dirty through Play, and undoing back after Stop is
    // clean again (the parked undo history still matches the saved state).
    let cube = editor.state.add_entity(NewEntity::Cube, None).unwrap();
    assert!(editor.state.is_dirty());
    editor.state.start_play().unwrap();
    editor.state.rename(cube, "Play only");
    assert!(editor.state.is_dirty());
    editor.state.stop_play();
    assert!(editor.state.is_dirty());
    editor.state.undo();
    assert!(!editor.state.is_dirty());
}

#[test]
fn a_game_panic_stops_play_and_keeps_the_edit_world_and_autosave() {
    let dir = temp_dir("fault");
    let project = dir.join("game");
    box_engine::Project::create(&project, "game").unwrap();
    let log = LogBuffer::default();
    let layer = log.layer();
    let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(layer));
    let options = EditorOptions {
        project: Some(project.clone()),
        ..Default::default()
    };
    let game = Game::new().with_plugin(Mutator).with_plugin(Bomb(5));
    let mut editor = build_game_editor(&options, log.clone(), game).unwrap();
    editor.state.add_entity(NewEntity::Sphere, None).unwrap();
    let before = snapshot(&editor);
    let undo = editor.state.edits.undo_len();

    editor.state.start_play().unwrap();
    let autosave = editor.state.play_autosave().unwrap().to_path_buf();
    assert_eq!(Some(&autosave), editor.state.recovery_path().as_ref());
    let mut error = None;
    for _ in 0..10 {
        match editor.state.update_play(DT) {
            Ok(_) => {}
            Err(e) => {
                error = Some(e);
                break;
            }
        }
    }
    match error {
        Some(PlayError::Panic(
            ref panic @ GamePanic {
                phase,
                ref message,
                ref location,
                ref system,
                ref plugins,
            },
        )) => {
            assert_eq!(phase, PlayPhase::Frame(5));
            assert_eq!(message, "boom at frame 5");
            assert!(location.as_ref().unwrap().contains("play.rs"));
            assert!(plugins.iter().any(|p| p.ends_with("Bomb")), "{plugins:?}");
            // The system that panicked: the closure Bomb's build added.
            let system = system.as_deref().unwrap_or_default();
            assert!(
                system.contains("Bomb") && system.contains("build"),
                "{system}"
            );
            let text = panic.to_string();
            assert!(text.contains(&format!("in system `{system}`")), "{text}");
        }
        other => panic!("expected a game panic, got {other:?}"),
    }
    assert_eq!(editor.state.play_state(), PlayState::Editing);
    assert_eq!(snapshot(&editor), before);
    assert_eq!(editor.state.edits.undo_len(), undo);
    // The autosave is intact and loads to the pre-Play scene.
    let saved = std::fs::read(&autosave).unwrap();
    assert_eq!(saved, before);
    let reloaded = scene::load(&saved, editor.state.game().registry()).unwrap();
    assert_eq!(scene::save(&reloaded).unwrap(), before);
    // Reported once, in the console.
    let errors: Vec<String> = log
        .lock()
        .records()
        .iter()
        .filter(|r| r.level == tracing::Level::ERROR)
        .map(|r| r.message.clone())
        .collect();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("Play stopped") && errors[0].contains("boom"));

    // The editor keeps working, and Play can start again.
    editor.state.start_play().unwrap();
    assert!(editor.state.update_play(DT).unwrap());
    editor.state.stop_play();
    assert_eq!(snapshot(&editor), before);
}

/// Switches the play world to the project's `arena2` scene on frame 3.
struct Switcher;
impl Plugin for Switcher {
    fn build(&self, app: &mut App) {
        app.add_system(Stage::Update, |w| {
            if w.resource::<Time>().frame == 3 {
                w.resource_mut::<Scenes>().switch("arena2");
            }
        });
    }
}

fn names(world: &World) -> Vec<String> {
    world.query::<&Name>().iter().map(|n| n.0.clone()).collect()
}

#[test]
fn a_runtime_scene_switch_during_play_resolves_in_the_project() {
    let dir = temp_dir("scene_switch");
    let project = dir.join("game");
    box_engine::Project::create(&project, "game").unwrap();
    let game = Game::new().with_plugin(Switcher);
    let arena = project.join("assets/scenes/arena2.scene.json");
    let mut world = World::with_registry(game.registry());
    world
        .spawn_saved(Uid::new(), "Arena Marker", (Transform::default(),))
        .unwrap();
    std::fs::write(&arena, scene::save(&world).unwrap()).unwrap();

    let mut editor = editor_for(game, Some(&project), &dir);
    editor.state.add_entity(NewEntity::Cube, None).unwrap();
    let before = snapshot(&editor);
    let undo = editor.state.edits.undo_len();
    let edit_names = names(&editor.state.world);
    let main = editor.state.scene.path.clone().unwrap();

    editor.state.start_play().unwrap();
    assert!(editor.state.update_play(DT).unwrap());
    {
        let scenes = editor.state.world.resource::<Scenes>();
        assert_eq!(scenes.project_root(), Some(project.as_path()));
        let start = scenes.main().unwrap();
        assert_eq!(scenes.path(start), Some(main.as_path()));
    }
    for _ in 0..4 {
        assert!(editor.state.update_play(DT).unwrap());
    }
    let w = &editor.state.world;
    let scenes = w.resource::<Scenes>();
    assert_eq!(scenes.path(scenes.main().unwrap()), Some(arena.as_path()));
    assert_eq!(
        names(w),
        vec!["Arena Marker".to_string()],
        "the switch replaced the scene"
    );

    editor.state.stop_play();
    assert_eq!(
        snapshot(&editor),
        before,
        "the edit scene must be byte-identical"
    );
    assert_eq!(names(&editor.state.world), edit_names);
    assert_eq!(editor.state.edits.undo_len(), undo);
    assert!(editor.state.is_dirty());
}

#[test]
fn a_panic_while_building_the_game_leaves_the_editor_editing() {
    let dir = temp_dir("bad_build");
    let mut editor = editor_for(Game::new().with_plugin(BadBuild), None, &dir);
    let before = snapshot(&editor);
    match editor.state.start_play() {
        Err(PlayError::Panic(p)) => {
            assert_eq!(p.phase, PlayPhase::Start);
            assert_eq!(p.message, "cannot build");
            assert_eq!(p.system, None, "no system runs while building");
        }
        other => panic!("expected a build panic, got {:?}", other.err()),
    }
    assert_eq!(editor.state.play_state(), PlayState::Editing);
    assert_eq!(snapshot(&editor), before);
}

#[test]
fn pause_stops_time_and_step_runs_exactly_one_frame() {
    let dir = temp_dir("pause");
    let mut editor = editor_for(Game::new().with_plugin(Recorder::default()), None, &dir);
    editor.state.start_play().unwrap();
    let s = &mut editor.state;
    assert!(s.update_play(DT).unwrap());
    let read = |s: &box_editor::EditorState| {
        let w = &s.world;
        (
            *w.resource::<Time>(),
            w.resource::<FixedTime>().tick,
            w.resource::<FixedSteps>().0,
        )
    };
    let (t0, tick0, steps0) = read(s);
    assert_eq!((t0.frame, tick0, steps0), (1, 1, 1));

    s.set_play_paused(true);
    assert_eq!(s.play_state(), PlayState::Paused);
    for _ in 0..5 {
        assert!(!s.update_play(0.1).unwrap());
    }
    let (t1, tick1, steps1) = read(s);
    assert_eq!(t1.frame, t0.frame);
    assert_eq!(t1.elapsed_f64, t0.elapsed_f64);
    assert_eq!((tick1, steps1), (tick0, steps0));

    // Step: one frame of one fixed step, even with a large dt; still paused.
    s.step_play();
    assert!(s.update_play(0.5).unwrap());
    let (t2, tick2, steps2) = read(s);
    assert_eq!(t2.frame, t0.frame + 1);
    assert!((t2.delta - 1.0 / 60.0).abs() < 1e-6);
    assert_eq!((tick2, steps2), (tick0 + 1, steps0 + 1));
    assert_eq!(s.play_state(), PlayState::Paused);
    assert!(!s.update_play(DT).unwrap());

    s.toggle_pause();
    assert_eq!(s.play_state(), PlayState::Playing);
    assert!(s.update_play(DT).unwrap());
    assert_eq!(read(s).0.frame, t0.frame + 2);
}

#[test]
fn game_types_are_in_the_editor_registry_and_round_trip() {
    let dir = temp_dir("types");
    let project = dir.join("coins");
    box_engine::Project::create(&project, "coins").unwrap();
    let game = Game::new().with_plugin(Mutator);
    // The scratch scene uses the game registry too.
    let scratch = editor_for(game.clone(), None, &dir);
    assert!(scratch.state.world.registry().component("Coin").is_some());

    let mut editor = editor_for(game, Some(&project), &dir);
    assert!(editor.state.world.registry().component("Coin").is_some());
    let uid = editor.state.add_entity(NewEntity::Empty, None).unwrap();
    editor.state.add_component(uid, "Coin");
    assert!(editor.state.set_field(uid, "Coin.value", &7u32));
    let path = editor.state.save_scene().unwrap();
    assert!(
        String::from_utf8(std::fs::read(&path).unwrap())
            .unwrap()
            .contains("Coin")
    );

    editor.state.load_scene(&path).unwrap();
    let e = editor.state.world.entity_by_uid(uid).unwrap();
    assert_eq!(editor.state.world.get::<Coin>(e).unwrap().value, 7);
    // And the play world gets it from the snapshot.
    editor.state.start_play().unwrap();
    let e = editor.state.world.entity_by_uid(uid).unwrap();
    assert_eq!(editor.state.world.get::<Coin>(e).unwrap().value, 7);
    editor.state.stop_play();
}

#[test]
fn saving_and_document_actions_never_touch_the_play_world() {
    let dir = temp_dir("documents");
    let project = dir.join("docs");
    box_engine::Project::create(&project, "docs").unwrap();
    let mut editor = editor_for(Game::new().with_plugin(Mutator), Some(&project), &dir);
    let scene_path = editor.state.scene.path.clone().unwrap();
    let on_disk = std::fs::read(&scene_path).unwrap();
    let before = snapshot(&editor);
    // A clean scene with a file needs no autosave: the file is the copy.
    editor.state.start_play().unwrap();
    assert_eq!(editor.state.play_autosave(), None);
    editor.state.update_play(DT).unwrap();
    assert!(editor.state.save_scene().is_err());
    assert_eq!(std::fs::read(&scene_path).unwrap(), on_disk);
    // Quit (like every document action) stops Play first.
    editor.begin(DocAction::Quit);
    assert_eq!(editor.state.play_state(), PlayState::Editing);
    assert!(editor.quit_requested());
    assert_eq!(snapshot(&editor), before);
    assert!(editor.state.recovery_path().is_some_and(|p| !p.exists()));
}

// ----- the editor UI, headless without a GPU -----

fn raw(frame: u32, events: Vec<Event>) -> RawInput {
    RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 720.0))),
        time: Some(frame as f64 / 60.0),
        focused: true,
        events,
        ..Default::default()
    }
}

fn key(key: Key, pressed: bool, modifiers: Modifiers) -> Event {
    Event::Key {
        key,
        physical_key: Some(key),
        pressed,
        repeat: false,
        modifiers,
    }
}

#[test]
fn game_input_reaches_the_game_and_not_the_editor() {
    let dir = temp_dir("input");
    let recorder = Recorder::default();
    let seen = recorder.0.clone();
    let mut editor = editor_for(Game::new().with_plugin(recorder), None, &dir);
    editor.tree = layout::default_layout();
    let ctx = egui::Context::default();
    editor.apply_theme(&ctx);
    let mut frame = 0;
    let mut run = |editor: &mut Editor, events: Vec<Event>| {
        let _ = ctx.run_ui(raw(frame, events), |ui| editor.ui(ui));
        frame += 1;
    };
    run(&mut editor, vec![]);
    let viewport = editor.state.viewport.rect.expect("viewport laid out");

    // Cmd+P plays; the game has input straight away.
    run(&mut editor, vec![key(Key::P, true, Modifiers::COMMAND)]);
    assert_eq!(editor.state.play_state(), PlayState::Playing);
    assert!(editor.state.game_focus());
    // E is a game key now, not the Rotate tool.
    run(&mut editor, vec![key(Key::E, true, Modifiers::NONE)]);
    assert_eq!(editor.state.tool, TransformTool::Translate);
    assert_eq!(*seen.lock().unwrap(), vec!["press E".to_string()]);
    // Escape gives input back; E is the editor's again.
    run(&mut editor, vec![key(Key::Escape, true, Modifiers::NONE)]);
    assert!(!editor.state.game_focus());
    run(&mut editor, vec![key(Key::E, true, Modifiers::NONE)]);
    assert_eq!(editor.state.tool, TransformTool::Rotate);
    // A click in the viewport gives input to the game again (that click is
    // not the game's); the next one is.
    let inside = viewport.center();
    let click = |pressed| Event::PointerButton {
        pos: inside,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    run(
        &mut editor,
        vec![Event::PointerMoved(inside), click(true), click(false)],
    );
    assert!(editor.state.game_focus());
    run(&mut editor, vec![click(true)]);
    run(
        &mut editor,
        vec![click(false), key(Key::Space, true, Modifiers::NONE)],
    );
    assert!(
        seen.lock()
            .unwrap()
            .ends_with(&["click".into(), "press Space".into()])
    );
    // Cmd+Shift+P pauses (and releases the game's keys); Cmd+P stops.
    run(
        &mut editor,
        vec![key(Key::P, true, Modifiers::COMMAND.plus(Modifiers::SHIFT))],
    );
    assert_eq!(editor.state.play_state(), PlayState::Paused);
    assert!(
        !editor
            .state
            .world
            .resource::<Input>()
            .key_down(KeyCode::Space)
    );
    assert!(editor.title().ends_with("Paused"));
    run(&mut editor, vec![key(Key::P, true, Modifiers::COMMAND)]);
    assert_eq!(editor.state.play_state(), PlayState::Editing);
}

/// Records the mouse motion the game sees each frame.
#[derive(Clone, Default)]
struct MotionRecorder(Arc<Mutex<Vec<Vec2>>>);
impl Plugin for MotionRecorder {
    fn build(&self, app: &mut App) {
        let seen = self.0.clone();
        app.add_system(Stage::Update, move |w| {
            seen.lock().unwrap().push(w.resource::<Input>().mouse_delta);
        });
    }
}

/// Pointer motion reaches the game until it grabs the cursor; then only raw
/// device motion does (what the windowed runner forwards), never both.
#[test]
fn a_grabbed_cursor_takes_motion_from_device_events_only() {
    use box_core::input::{CursorGrab, CursorOptions};
    let dir = temp_dir("motion");
    let recorder = MotionRecorder::default();
    let seen = recorder.0.clone();
    let mut editor = editor_for(Game::new().with_plugin(recorder), None, &dir);
    editor.tree = layout::default_layout();
    let ctx = egui::Context::default();
    let mut frame = 0;
    let mut run = |editor: &mut Editor, events: Vec<Event>| {
        let _ = ctx.run_ui(raw(frame, events), |ui| editor.ui(ui));
        frame += 1;
        seen.lock().unwrap().last().copied().unwrap_or_default()
    };
    run(&mut editor, vec![]);
    let c = editor.state.viewport.rect.unwrap().center();
    editor.state.start_play().unwrap();
    assert!(editor.state.game_focus() && !editor.state.game_cursor_grabbed());
    run(&mut editor, vec![Event::PointerMoved(c)]);
    let moved = run(&mut editor, vec![Event::PointerMoved(c + vec2(10.0, 4.0))]);
    assert_eq!(moved, Vec2::new(10.0, 4.0));

    // The runner grabbed the cursor for the game.
    editor
        .state
        .world
        .resource_mut::<CursorOptions>()
        .active_grab = CursorGrab::Locked;
    assert!(editor.state.game_cursor_grabbed());
    let pointer = run(&mut editor, vec![Event::PointerMoved(c + vec2(30.0, 0.0))]);
    assert_eq!(pointer, Vec2::ZERO, "pointer motion is not counted twice");
    editor
        .state
        .world
        .resource_mut::<Input>()
        .add_mouse_motion(Vec2::new(-3.0, 7.0));
    assert_eq!(run(&mut editor, vec![]), Vec2::new(-3.0, 7.0));

    // Escape gives input back to the editor, and with it the cursor.
    run(&mut editor, vec![key(Key::Escape, true, Modifiers::NONE)]);
    assert!(!editor.state.game_focus() && !editor.state.game_cursor_grabbed());
}

// ----- capture -----

#[test]
fn capture_the_editor_in_play_mode() {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping the Play capture");
        return;
    }
    let dir = temp_dir("capture");
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures");
    std::fs::create_dir_all(&out).unwrap();
    let options = CaptureOptions {
        size: [1280, 720],
        pixels_per_point: 1.0,
        frames: 4,
    };
    for (name, game_camera) in [("editor_play", false), ("editor_play_game_camera", true)] {
        let mut editor = editor_for(Game::new().with_plugin(Lift), None, &dir);
        editor.tree = layout::default_layout();
        box_editor::headless::select_sample_entity(&mut editor);
        if game_camera {
            editor.state.add_entity(NewEntity::Camera, None).unwrap();
        }
        editor.state.start_play().unwrap();
        let mut h = match Headless::new(&mut editor, &options) {
            Ok(h) => h,
            Err(e)
                if matches!(
                    e.downcast_ref::<RenderError>(),
                    Some(RenderError::NoAdapter)
                ) =>
            {
                eprintln!("no GPU adapter; skipping the Play capture");
                return;
            }
            Err(e) => panic!("{e:#}"),
        };
        for _ in 0..options.frames {
            h.frame(&mut editor, Vec::new());
        }
        assert_eq!(editor.state.play_state(), PlayState::Playing);
        assert_eq!(editor.state.shows_game_camera(), game_camera);
        // Each editor frame ran one play frame of real (here: fixed) time.
        let session = editor.state.play_session().unwrap();
        assert_eq!(session.frames(), options.frames as u64);
        let w = &editor.state.world;
        let y = w
            .query::<(&Name, &Transform)>()
            .iter()
            .find(|(n, _)| n.as_str() == "Glazed Ball")
            .map(|(_, t)| t.translation.y)
            .unwrap();
        let elapsed = w.resource::<Time>().elapsed;
        assert!(elapsed > 0.04, "{elapsed}");
        assert!((y - (0.8 + 30.0 * elapsed)).abs() < 1e-3, "{y}");
        let image = h.image().unwrap();
        image.save(out.join(format!("{name}.png"))).unwrap();
    }
}
