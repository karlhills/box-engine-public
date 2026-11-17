//! Clear feedback, driven without a GPU: toasts from warning records and
//! from a game panic, the Console badge, toast clicks opening the Console,
//! selection restored by undo and redo, readable undo labels, unknown
//! components, and hierarchy error marks for missing assets.

use std::path::PathBuf;

use box_core::asset_ref::AssetRef;
use box_core::identity::Uid;
use box_core::prelude::*;
use box_core::reflect::{Reflect, ReflectComponent, ReflectDefault};
use box_core::scene;
use box_editor::feedback::{self, MAX_VISIBLE};
use box_editor::layout::Pane;
use box_editor::log::{LogBuffer, Severity};
use box_editor::{
    Editor, EditorOptions, EditorState, NewEntity, PlayError, build_editor, build_game_editor,
};
use box_engine::Game;
use egui::{Event, PointerButton, Pos2, Rect, vec2};
use tracing_subscriber::layer::SubscriberExt;

fn temp_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("box_editor_feedback_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Feeds the editor UI one frame per call, without rendering.
struct Driver {
    ctx: egui::Context,
    time: f64,
}

impl Driver {
    fn new(editor: &Editor) -> Self {
        let ctx = egui::Context::default();
        editor.apply_theme(&ctx);
        Self { ctx, time: 0.0 }
    }

    fn frame(&mut self, editor: &mut Editor, events: Vec<Event>) {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0))),
            time: Some(self.time),
            predicted_dt: 1.0 / 60.0,
            focused: true,
            events,
            ..Default::default()
        };
        let _ = self.ctx.run_ui(raw, |ui| editor.ui(ui));
        self.time += 1.0 / 60.0;
    }

    fn click(&mut self, editor: &mut Editor, pos: Pos2) {
        let press = |pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        self.frame(editor, vec![Event::PointerMoved(pos)]);
        self.frame(editor, vec![press(true)]);
        self.frame(editor, vec![press(false)]);
    }
}

fn active_panes(editor: &Editor) -> Vec<Pane> {
    editor
        .tree
        .active_tiles()
        .into_iter()
        .filter_map(|id| editor.tree.tiles.get_pane(&id).copied())
        .collect()
}

fn project_editor(name: &str, log: LogBuffer, game: Game) -> Editor {
    let dir = temp_dir(name);
    box_engine::Project::create(&dir, name).unwrap();
    let options = EditorOptions {
        project: Some(dir),
        ..Default::default()
    };
    build_game_editor(&options, log, game).unwrap()
}

#[test]
fn warnings_of_engine_targets_become_coalesced_toasts_and_count_on_the_badge() {
    let log = LogBuffer::default();
    let mut state = EditorState::new(World::new(), log.clone());
    log.push(
        tracing::Level::WARN,
        "box_editor::assets",
        "stone.material.json: not loaded yet; try again".into(),
    );
    log.push(tracing::Level::WARN, "wgpu_core::device", "noise".into());
    log.push(tracing::Level::INFO, "box_editor::state", "fine".into());
    state.poll_log();
    let toasts = state.toasts().items();
    assert_eq!(toasts.len(), 1, "{toasts:?}");
    assert_eq!(toasts[0].severity, Severity::Warn);
    assert_eq!(
        toasts[0].text,
        "stone.material.json: not loaded yet; try again"
    );
    assert_eq!(toasts[0].seq, Some(0));
    // Every warning counts on the badge, dependency ones too.
    assert_eq!(state.console.unseen(), [2, 0]);

    // A repeat is one toast with a count; polling again reads nothing new.
    log.push(
        tracing::Level::WARN,
        "box_editor::assets",
        "stone.material.json: not loaded yet; try again".into(),
    );
    state.poll_log();
    state.poll_log();
    let toasts = state.toasts().items();
    assert_eq!((toasts.len(), toasts[0].count), (1, 2));
    assert_eq!(state.console.unseen(), [3, 0]);

    // A burst keeps at most MAX_VISIBLE, errors first, and counts the rest.
    log.push(tracing::Level::ERROR, "box_editor", "bad".into());
    for i in 0..10 {
        log.push(tracing::Level::WARN, "box_assets", format!("warning {i}"));
    }
    state.poll_log();
    let toasts = state.toasts().items();
    assert_eq!(toasts.len(), MAX_VISIBLE);
    assert!(toasts.iter().any(|t| t.severity == Severity::Error));
    // Pushed out: the coalesced warning (2 reports) and 7 of the 10 new ones.
    assert_eq!(state.toasts().overflow(), 9);
    assert_eq!(state.console.unseen(), [13, 1]);
}

#[test]
fn the_badge_clears_when_the_console_is_shown() {
    let log = LogBuffer::default();
    let mut state = EditorState::new(World::new(), log.clone());
    log.push(tracing::Level::WARN, "box_editor", "w".into());
    log.push(tracing::Level::ERROR, "box_editor", "e".into());
    state.poll_log();
    assert_eq!(state.console.unseen(), [1, 1]);
    let ctx = egui::Context::default();
    let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
        box_editor::panes::console_ui(ui, &mut state)
    });
    assert_eq!(state.console.unseen(), [0, 0]);
    assert_eq!(state.toasts().overflow(), 0);
}

/// Panics in the third played frame.
struct Bomb;
impl Plugin for Bomb {
    fn build(&self, app: &mut App) {
        app.add_system(Stage::Update, |w| {
            if w.resource::<Time>().frame == 3 {
                panic!("boom");
            }
        });
    }
}

#[test]
fn a_game_panic_shows_an_error_toast_and_a_click_opens_the_console_at_it() {
    let log = LogBuffer::default();
    let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(log.layer()));
    let mut editor = project_editor("panic", log.clone(), Game::new().with_plugin(Bomb));
    let mut d = Driver::new(&editor);
    d.frame(&mut editor, Vec::new());
    assert!(
        active_panes(&editor).contains(&Pane::Assets),
        "with a project"
    );
    editor.state.start_play().unwrap();
    let mut error = None;
    for _ in 0..5 {
        if let Err(e) = editor.state.update_play(1.0 / 60.0) {
            error = Some(e);
            break;
        }
    }
    assert!(matches!(error, Some(PlayError::Panic(_))));
    d.frame(&mut editor, Vec::new());
    let toast = editor
        .state
        .toasts()
        .items()
        .iter()
        .find(|t| t.severity == Severity::Error)
        .expect("an error toast")
        .clone();
    assert!(toast.text.starts_with("Play stopped"), "{}", toast.text);
    assert!(toast.text.contains("boom"), "{}", toast.text);
    let seq = toast.seq.expect("from a console record");
    assert!(
        editor.state.console.unseen()[1] >= 1,
        "badge counts the error"
    );

    // The newest toast is at the bottom right of the viewport.
    let view = editor.state.viewport.rect.unwrap();
    d.click(&mut editor, view.right_bottom() + vec2(-120.0, -20.0));
    assert!(active_panes(&editor).contains(&Pane::Console));
    assert!(editor.state.console.selected().contains(&seq));
    d.frame(&mut editor, Vec::new());
    assert_eq!(
        editor.state.console.unseen(),
        [0, 0],
        "shown: badge cleared"
    );
    assert!(
        !editor
            .state
            .toasts()
            .items()
            .iter()
            .any(|t| t.seq == Some(seq)),
        "a clicked toast is dismissed"
    );
}

#[test]
fn toasts_expire_after_their_time() {
    let log = LogBuffer::default();
    let mut editor = build_editor(&EditorOptions::default(), log.clone()).unwrap();
    let mut d = Driver::new(&editor);
    log.push(tracing::Level::WARN, "box_editor", "short-lived".into());
    d.frame(&mut editor, Vec::new());
    assert_eq!(editor.state.toasts().items().len(), 1);
    d.time += feedback::WARN_S + 0.1;
    d.frame(&mut editor, Vec::new());
    assert!(editor.state.toasts().items().is_empty());
}

#[test]
fn no_project_puts_the_console_in_front() {
    let mut editor = build_editor(&EditorOptions::default(), LogBuffer::default()).unwrap();
    let mut d = Driver::new(&editor);
    d.frame(&mut editor, Vec::new());
    assert!(active_panes(&editor).contains(&Pane::Console));
    assert!(!active_panes(&editor).contains(&Pane::Assets));
}

#[test]
fn undo_and_redo_restore_the_selection_and_toast_readable_labels() {
    let mut s = EditorState::new(
        World::with_registry(box_engine::shared_engine_registry()),
        LogBuffer::default(),
    );
    let cube = s.add_entity(NewEntity::Cube, None).unwrap();
    let sphere = s.add_entity(NewEntity::Sphere, None).unwrap();
    s.select(Some(cube));
    s.delete_selection();
    assert!(s.selection.is_empty());
    s.undo();
    assert_eq!(s.selection, vec![cube], "undo brings the selection back");
    assert!(
        s.toasts()
            .items()
            .iter()
            .any(|t| t.text == "Undo: Delete Cube"),
        "{:?}",
        s.toasts().items()
    );
    s.redo();
    assert!(s.selection.is_empty(), "the deleted cube is gone again");

    // Duplicate selects the copy; undo goes back to the original.
    s.select(Some(sphere));
    s.duplicate_selection();
    let copy = s.selection.clone();
    assert_ne!(copy, vec![sphere]);
    s.undo();
    assert_eq!(s.selection, vec![sphere]);
    s.redo();
    assert_eq!(s.selection, copy, "redo selects the copy again");
}

/// A game component the stock editor does not know.
#[derive(Reflect, Default, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone)]
#[type_path = "feedback_test"]
struct PlayerController {
    speed: f32,
}

struct GameTypes;
impl Plugin for GameTypes {
    fn register(&self, registry: &mut box_core::reflect::Registry) {
        registry.register::<PlayerController>().unwrap();
    }
    fn build(&self, _app: &mut App) {}
}

#[test]
fn unknown_components_are_announced_once_and_listed_for_the_inspector() {
    // Write a scene with the game's component, as the game's editor would.
    let dir = temp_dir("unknown");
    let project = box_engine::Project::create(&dir, "unknown").unwrap();
    let game = Game::new().with_plugin(GameTypes);
    let mut world = World::with_registry(game.registry());
    let uid = Uid::from_u128(42);
    world
        .spawn_saved(
            uid,
            "Player",
            (Transform::default(), PlayerController { speed: 4.0 }),
        )
        .unwrap();
    std::fs::write(project.start_scene_path(), scene::save(&world).unwrap()).unwrap();

    // Open it with the stock editor.
    let log = LogBuffer::default();
    let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(log.layer()));
    let options = EditorOptions {
        project: Some(dir.clone()),
        ..Default::default()
    };
    let mut editor = build_editor(&options, log.clone()).unwrap();
    // The scene loader's own warning about the type is not toasted again.
    tracing::warn!(target: "box_core::scene", "unknown component `PlayerController` (kept as raw data)");
    editor.state.poll_log();
    let warnings: Vec<_> = editor
        .state
        .toasts()
        .items()
        .iter()
        .filter(|t| t.text.contains("PlayerController"))
        .cloned()
        .collect();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0].severity, Severity::Warn);
    assert!(warnings[0].text.contains("not registered"));
    assert_eq!(warnings[0].count, 1);

    let e = editor.state.world.entity_by_uid(uid).unwrap();
    let unknown = feedback::unknown_components(&editor.state.world, e);
    assert_eq!(unknown.len(), 1);
    assert_eq!(unknown[0].0, "PlayerController");
    assert!(unknown[0].1.contains("\"speed\": 4.0"), "{}", unknown[0].1);

    // Opening it again logs to the console but does not toast again.
    let start = project.start_scene_path();
    editor.state.open_scene(&start, true).unwrap();
    editor.state.poll_log();
    let n = editor
        .state
        .toasts()
        .items()
        .iter()
        .filter(|t| t.text.contains("PlayerController"))
        .map(|t| t.count)
        .sum::<u32>();
    assert_eq!(n, 1);
}

#[test]
fn a_missing_prefab_marks_its_hierarchy_row() {
    let mut editor = project_editor("prefab", LogBuffer::default(), Game::new());
    let s = &mut editor.state;
    let uid = s.add_entity(NewEntity::Empty, None).unwrap();
    s.refresh_asset_marks();
    assert!(s.asset_error(uid).is_none());
    s.add_component(uid, "PrefabInstance");
    let missing = AssetRef::path("prefabs/missing.scene.json");
    assert!(s.set_field(uid, "PrefabInstance.scene", &missing));
    s.refresh_asset_marks();
    let error = s.asset_error(uid).expect("marked").to_owned();
    assert!(error.contains("prefabs/missing.scene.json"), "{error}");
    assert!(error.contains("missing"), "{error}");
    // Undo the reference: the mark goes.
    s.undo();
    s.refresh_asset_marks();
    assert!(s.asset_error(uid).is_none());
}

#[test]
fn saving_toasts_the_file_name() {
    let mut editor = project_editor("save", LogBuffer::default(), Game::new());
    editor.state.add_entity(NewEntity::Cube, None).unwrap();
    editor.save();
    assert!(
        editor
            .state
            .toasts()
            .items()
            .iter()
            .any(|t| t.text == "Saved main.scene.json"),
        "{:?}",
        editor.state.toasts().items()
    );
}

// ----- captures -----

/// Renders `editor` headlessly at 1440x900 for a few frames (running
/// `events` in an extra one) and writes `target/captures/<name>.png`.
/// Returns false when there is no GPU (or GPU tests are skipped).
fn capture(editor: &mut Editor, name: &str, events: Vec<Event>) -> bool {
    use box_editor::headless::{CaptureOptions, Headless};
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        return false;
    }
    let options = CaptureOptions {
        size: [1440, 900],
        pixels_per_point: 1.0,
        frames: 4,
    };
    let mut h = match Headless::new(editor, &options) {
        Ok(h) => h,
        Err(e)
            if matches!(
                e.downcast_ref::<box_render::RenderError>(),
                Some(box_render::RenderError::NoAdapter)
            ) =>
        {
            eprintln!("no GPU adapter; skipping the {name} capture");
            return false;
        }
        Err(e) => panic!("{e:#}"),
    };
    for _ in 0..options.frames {
        h.frame(editor, Vec::new());
    }
    if !events.is_empty() {
        h.frame(editor, events);
        h.frame(editor, Vec::new());
    }
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures");
    std::fs::create_dir_all(&out).unwrap();
    h.image()
        .unwrap()
        .save(out.join(format!("{name}.png")))
        .unwrap();
    true
}

/// Captures of the feedback features (look at them after a change):
/// toasts and the Console badge, Play tint and banner, an unknown
/// component, and a hierarchy error mark with its inspector note.
#[test]
fn feedback_captures() {
    // Toasts over the viewport and the badge on the (hidden) Console tab.
    let log = LogBuffer::default();
    let mut editor = project_editor("cap_toasts", log.clone(), Game::new());
    editor.state.add_entity(NewEntity::Cube, None).unwrap();
    for _ in 0..3 {
        log.push(
            tracing::Level::WARN,
            "box_editor::assets",
            "stone.material.json: not loaded yet; try again".into(),
        );
    }
    log.push(
        tracing::Level::ERROR,
        "box_editor::dialogs",
        "open scene failed: loading assets/scenes/level2.scene.json: expected value at line 1 column 1".into(),
    );
    log.push(tracing::Level::WARN, "wgpu_core", "not toasted".into());
    editor.save();
    if !capture(&mut editor, "feedback_toasts", Vec::new()) {
        return;
    }
    assert_eq!(editor.state.toasts().items().len(), 3);
    assert_eq!(editor.state.console.unseen(), [4, 1]);

    // Play: tinted panels and the inspector banner.
    let mut editor = build_editor(&EditorOptions::default(), LogBuffer::default()).unwrap();
    box_editor::headless::select_sample_entity(&mut editor);
    editor.state.start_play().unwrap();
    editor.state.set_game_focus(false);
    capture(&mut editor, "feedback_play", Vec::new());
    editor.state.set_play_paused(true);
    capture(&mut editor, "feedback_paused", Vec::new());
    editor.state.stop_play();

    // An unknown component, collapsed, then opened with a click on it.
    let dir = temp_dir("cap_unknown");
    let project = box_engine::Project::create(&dir, "cap_unknown").unwrap();
    let game = Game::new().with_plugin(GameTypes);
    let mut world = World::with_registry(game.registry());
    let uid = Uid::from_u128(7);
    world
        .spawn_saved(
            uid,
            "Player",
            (Transform::default(), PlayerController { speed: 4.0 }),
        )
        .unwrap();
    std::fs::write(project.start_scene_path(), scene::save(&world).unwrap()).unwrap();
    let options = EditorOptions {
        project: Some(dir.clone()),
        ..Default::default()
    };
    let mut editor = build_editor(&options, LogBuffer::default()).unwrap();
    editor.state.select(Some(uid));
    capture(&mut editor, "feedback_unknown", Vec::new());
    let header = egui::pos2(1250.0, 256.0);
    let press = |pressed| Event::PointerButton {
        pos: header,
        button: PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    capture(
        &mut editor,
        "feedback_unknown_open",
        vec![Event::PointerMoved(header), press(true), press(false)],
    );

    // A missing prefab: the hierarchy row's mark and the inspector note.
    let mut editor = project_editor("cap_prefab", LogBuffer::default(), Game::new());
    editor.state.add_entity(NewEntity::Cube, None).unwrap();
    let uid = editor.state.add_entity(NewEntity::Empty, None).unwrap();
    editor.state.rename(uid, "Lamp Post");
    editor.state.add_component(uid, "PrefabInstance");
    let missing = AssetRef::path("prefabs/lamp.scene.json");
    assert!(
        editor
            .state
            .set_field(uid, "PrefabInstance.scene", &missing)
    );
    capture(&mut editor, "feedback_prefab_mark", Vec::new());
    assert!(editor.state.asset_error(uid).is_some());
}
