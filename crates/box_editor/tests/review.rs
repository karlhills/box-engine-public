//! Regression tests for reviewed editor defects. The editor is driven without
//! a GPU: document flows through `Editor`'s methods, input through a bare
//! `egui::Context` fed with events.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use box_core::identity::Uid;
use box_core::prelude::{Primitive, Shape, Transform, World};
use box_core::scene;
use box_core::world::Placement;
use box_editor::layout::Pane;
use box_editor::log::LogBuffer;
use box_editor::{
    DocAction, Editor, EditorOptions, EditorState, NewEntity, TransformTool, build_editor,
};
use box_render::StandardMaterial;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use glam::{Vec2, Vec3};

const SCREEN: egui::Vec2 = vec2(1280.0, 800.0);

/// Feeds the editor UI with events, one frame per call, without rendering.
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
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
            time: Some(self.time),
            predicted_dt: 1.0 / 60.0,
            focused: true,
            events,
            ..Default::default()
        };
        let _ = self.ctx.run_ui(raw, |ui| editor.ui(ui));
        self.time += 1.0 / 60.0;
    }

    fn frames(&mut self, editor: &mut Editor, n: usize) {
        for _ in 0..n {
            self.frame(editor, Vec::new());
        }
    }

    fn click(&mut self, editor: &mut Editor, pos: Pos2, button: PointerButton) {
        let press = |pressed| Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: Modifiers::NONE,
        };
        self.frame(editor, vec![Event::PointerMoved(pos)]);
        self.frame(editor, vec![press(true)]);
        self.frame(editor, vec![press(false)]);
        self.frame(editor, Vec::new());
    }

    fn key(&mut self, editor: &mut Editor, key: Key, modifiers: Modifiers) {
        let event = |pressed| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        };
        self.frame(editor, vec![event(true)]);
        self.frame(editor, vec![event(false)]);
    }
}

/// The pane's content rectangle (below its tab bar), after a frame.
fn pane_rect(editor: &Editor, pane: Pane) -> Rect {
    let id = editor.tree.tiles.find_pane(&pane).expect("pane in layout");
    editor.tree.tiles.rect(id).expect("pane is visible")
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("box_editor_review_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn project_editor(name: &str) -> (Editor, PathBuf) {
    let dir = temp_dir(name);
    box_engine::Project::create(&dir, name).unwrap();
    let editor = reopen(&dir);
    let root = editor.state.project.as_ref().unwrap().root().to_path_buf();
    (editor, root)
}

fn reopen(root: &Path) -> Editor {
    let options = EditorOptions {
        project: Some(root.to_path_buf()),
        ..Default::default()
    };
    build_editor(&options, LogBuffer::default()).unwrap()
}

fn set_mtime(path: &Path, t: SystemTime) {
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(t)
        .unwrap();
}

fn state_with(world: World) -> EditorState {
    let mut world = world;
    box_editor::scratch::prepare_world(&mut world);
    let mut s = EditorState::new(world, LogBuffer::default());
    s.viewport.rect = Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0)));
    s
}

fn settle(state: &mut EditorState) {
    box_core::transform::propagate_transforms(&mut state.world);
    box_engine::sync_scene_meshes(&mut state.world);
}

/// One cube at (0, 0, 2), selected, seen from an angle with the move tool.
fn cube_state() -> (EditorState, Uid) {
    let mut world = World::with_registry(box_engine::shared_engine_registry());
    let uid = Uid::from_u128(10);
    world
        .spawn_saved(
            uid,
            "Near",
            (
                Transform::from_xyz(0.0, 0.0, 2.0),
                Primitive::new(Shape::Cuboid { size: Vec3::ONE }),
                StandardMaterial::default(),
            ),
        )
        .unwrap();
    let mut state = state_with(world);
    state.camera.orbit.target = Vec3::ZERO;
    state.camera.orbit.yaw = 0.6;
    state.camera.orbit.pitch = 0.4;
    state.camera.orbit.distance = 10.0;
    settle(&mut state);
    state.select(Some(uid));
    state.tool = TransformTool::Translate;
    (state, uid)
}

fn transform_of(state: &EditorState, uid: Uid) -> Transform {
    let e = state.world.entity_by_uid(uid).unwrap();
    *state.world.get::<Transform>(e).unwrap()
}

/// Grabs the X arrow of the selection's gizmo and drags it `steps` times.
/// Returns the grab point.
fn start_drag(state: &mut EditorState, steps: usize) -> Vec2 {
    let view = state.gizmo_view().unwrap();
    let frame = state.gizmo_frame(&view).unwrap();
    let grab = view
        .project(frame.origin + frame.axes[0] * frame.size * 0.6)
        .unwrap();
    assert!(state.gizmo_begin(grab), "grabbed the X arrow");
    drag_more(state, grab, 1, steps);
    grab
}

fn drag_more(state: &mut EditorState, grab: Vec2, from: usize, steps: usize) {
    for k in from..from + steps {
        state.gizmo_update(grab + Vec2::new(8.0 * k as f32, 0.0));
        settle(state);
    }
}

// ----- 1: recovery files are keyed by the scene's path -----

#[test]
fn same_named_scenes_do_not_share_a_recovery_file() {
    let (mut editor, root) = project_editor("recovery_key");
    let a = root.join("assets/a/main.scene.json");
    let b = root.join("assets/b/main.scene.json");
    editor.state.save_scene_as(&a).unwrap();
    editor.state.save_scene_as(&b).unwrap();
    let old = SystemTime::now() - Duration::from_secs(3600);
    for scene in [&a, &b, &root.join("assets/scenes/main.scene.json")] {
        set_mtime(scene, old);
    }

    // Unsaved changes to a/main, kept on quit.
    editor.state.open_scene(&a, false).unwrap();
    let cube = editor.state.add_entity(NewEntity::Cube, None).unwrap();
    editor.on_quit();
    let recovery_a = editor.state.recovery_path().unwrap();
    assert!(recovery_a.exists());
    drop(editor);

    // The start scene (scenes/main) and b/main are not offered a's changes.
    let mut editor = reopen(&root);
    assert_eq!(editor.dialog(), None, "scenes/main has no recovery copy");
    editor.state.open_scene(&b, false).unwrap();
    assert_ne!(editor.state.recovery_path().unwrap(), recovery_a);
    assert_eq!(editor.state.recovery_candidate(), None);

    // Saving b keeps a's recovery copy.
    editor.state.add_entity(NewEntity::Sphere, None).unwrap();
    editor.state.save_scene().unwrap();
    assert!(recovery_a.exists(), "saving b/main must not drop a/main's");

    // a/main is offered its own.
    editor.begin(DocAction::OpenScene(a.clone()));
    assert_eq!(
        editor.dialog(),
        Some(&box_editor::Dialog::Recovery {
            recovery: recovery_a.clone()
        })
    );
    editor.answer_recovery(true);
    assert!(editor.state.world.entity_by_uid(cube).is_some());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_recovery_file_from_the_old_naming_scheme_is_not_offered() {
    let (editor, root) = project_editor("recovery_old_scheme");
    drop(editor);
    // What an older editor wrote for *some* scene called `main`.
    let stale = root.join(".box/recovery/main.scene.json");
    std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
    std::fs::copy(root.join("assets/scenes/main.scene.json"), &stale).unwrap();
    set_mtime(
        &root.join("assets/scenes/main.scene.json"),
        SystemTime::now() - Duration::from_secs(3600),
    );
    let editor = reopen(&root);
    assert_eq!(editor.dialog(), None);
    let _ = std::fs::remove_dir_all(&root);
}

// ----- 2: periodic recovery writes -----

#[test]
fn unsaved_changes_reach_the_recovery_file_while_editing() {
    let (mut editor, root) = project_editor("autosave");
    let mut ui = Driver::new(&editor);
    ui.frames(&mut editor, 3);
    let recovery = editor.state.recovery_path().unwrap();
    assert!(!recovery.exists(), "nothing to recover while clean");

    let cube = editor.state.add_entity(NewEntity::Cube, None).unwrap();
    ui.time += 60.0;
    ui.frames(&mut editor, 2);
    assert_eq!(
        std::fs::read(&recovery).expect("recovery written without quitting"),
        scene::save(&editor.state.world).unwrap()
    );

    // Throttled: a change right after a write waits for the interval.
    let first = std::fs::read(&recovery).unwrap();
    editor
        .state
        .set_field(cube, "Transform.translation", &Vec3::new(1.0, 2.0, 3.0));
    ui.time += 5.0;
    ui.frames(&mut editor, 2);
    assert_eq!(std::fs::read(&recovery).unwrap(), first, "throttled");
    ui.time += 60.0;
    ui.frames(&mut editor, 2);
    let second = std::fs::read(&recovery).unwrap();
    assert_eq!(second, scene::save(&editor.state.world).unwrap());
    assert_ne!(second, first);

    // Idle frames write nothing.
    let stamp = std::fs::metadata(&recovery).unwrap().modified().unwrap();
    ui.time += 120.0;
    ui.frames(&mut editor, 2);
    assert_eq!(
        std::fs::metadata(&recovery).unwrap().modified().unwrap(),
        stamp
    );

    // Saving drops the copy.
    editor.save();
    assert!(!recovery.exists());

    // Never in the middle of a drag (its transaction is open); right after.
    let t = SystemTime::now();
    editor.state.select(Some(cube));
    editor.state.tool = TransformTool::Translate;
    ui.frames(&mut editor, 2);
    box_core::transform::propagate_transforms(&mut editor.state.world);
    ui.time += 60.0;
    start_drag(&mut editor.state, 2);
    assert!(editor.state.is_dirty());
    assert_eq!(editor.state.autosave_recovery(ui.time), None, "mid-drag");
    assert!(!recovery.exists());
    editor.state.gizmo_end();
    assert_eq!(
        editor.state.autosave_recovery(ui.time),
        Some(recovery.clone())
    );
    assert!(std::fs::metadata(&recovery).unwrap().modified().unwrap() >= t);

    // Undone back to the saved state: the copy would offer changes that no
    // longer exist, so it goes.
    editor.state.undo();
    assert!(!editor.state.is_dirty());
    ui.frames(&mut editor, 2);
    assert!(!recovery.exists(), "clean again: no recovery copy");
    assert!(!editor.state.recovery_pending(), "idle");
    let _ = std::fs::remove_dir_all(&root);
}

// ----- 3: undo while typing belongs to the text field -----

#[test]
fn undo_while_typing_does_not_undo_a_scene_edit() {
    let mut editor = build_editor(&EditorOptions::default(), LogBuffer::default()).unwrap();
    let cube = editor.state.add_entity(NewEntity::Cube, None).unwrap();
    let steps = editor.state.edits.undo_len();
    let mut ui = Driver::new(&editor);
    ui.frames(&mut editor, 3);

    // Focus the hierarchy search field and type.
    let h = pane_rect(&editor, Pane::Hierarchy);
    ui.click(
        &mut editor,
        pos2(h.center().x, h.top() + 17.0),
        PointerButton::Primary,
    );
    assert!(ui.ctx.egui_wants_keyboard_input(), "search field focused");
    ui.frame(&mut editor, vec![Event::Text("cu".into())]);
    assert_eq!(editor.state.hierarchy.filter, "cu");

    ui.key(&mut editor, Key::Z, Modifiers::COMMAND);
    assert_eq!(editor.state.edits.undo_len(), steps, "no scene undo");
    assert!(editor.state.world.entity_by_uid(cube).is_some());
    ui.key(&mut editor, Key::Z, Modifiers::COMMAND | Modifiers::SHIFT);
    assert_eq!(editor.state.edits.undo_len(), steps, "no scene redo");

    // Without a focused field the shortcut undoes the scene edit.
    ui.key(&mut editor, Key::Escape, Modifiers::NONE);
    assert!(!ui.ctx.egui_wants_keyboard_input());
    ui.key(&mut editor, Key::Z, Modifiers::COMMAND);
    assert!(editor.state.world.entity_by_uid(cube).is_none());
}

// ----- 4: a drag does not outlive its transaction -----

#[test]
fn undo_during_a_gizmo_drag_ends_the_drag() {
    let (mut state, uid) = cube_state();
    let before = transform_of(&state, uid);
    let steps = state.edits.undo_len();
    let grab = start_drag(&mut state, 3);
    state.undo();
    assert!(state.gizmo.drag.is_none(), "the drag ended");
    assert_eq!(transform_of(&state, uid), before, "the drag was undone");
    // The mouse is still held and moving: nothing more is recorded.
    drag_more(&mut state, grab, 4, 3);
    assert_eq!(state.edits.undo_len(), steps);
    assert_eq!(state.edits.redo_len(), 1, "the drag is one redo step");
    assert_eq!(transform_of(&state, uid), before);
}

#[test]
fn save_during_a_gizmo_drag_commits_it_as_one_step() {
    let (mut state, uid) = cube_state();
    let file = temp_dir("save_mid_drag").join("s.scene.json");
    state.save_scene_as(&file).unwrap();
    let steps = state.edits.undo_len();
    let grab = start_drag(&mut state, 3);
    state.save_scene().unwrap();
    let saved = transform_of(&state, uid);
    assert!(state.gizmo.drag.is_none(), "the drag ended");
    drag_more(&mut state, grab, 4, 3);
    assert_eq!(state.edits.undo_len(), steps + 1, "one step");
    assert_eq!(transform_of(&state, uid), saved, "the file matches");
    assert!(!state.is_dirty());
    let _ = std::fs::remove_dir_all(file.parent().unwrap());
}

#[test]
fn delete_during_a_gizmo_drag_ends_the_drag() {
    let (mut state, uid) = cube_state();
    let steps = state.edits.undo_len();
    let grab = start_drag(&mut state, 3);
    state.delete_selection();
    assert!(state.gizmo.drag.is_none(), "the drag ended");
    assert!(state.world.entity_by_uid(uid).is_none());
    drag_more(&mut state, grab, 4, 3);
    assert_eq!(state.edits.undo_len(), steps + 2, "move, then delete");
    assert_eq!(state.edits.undo_label(), Some("Delete Near"));
}

#[test]
fn escape_cancels_a_gizmo_drag() {
    let (mut state, uid) = cube_state();
    let before = transform_of(&state, uid);
    let steps = state.edits.undo_len();
    start_drag(&mut state, 3);
    assert_ne!(transform_of(&state, uid), before);
    let mut editor = Editor::new(state);
    let mut ui = Driver::new(&editor);
    ui.key(&mut editor, Key::Escape, Modifiers::NONE);
    let state = &editor.state;
    assert!(state.gizmo.drag.is_none(), "the drag ended");
    assert_eq!(transform_of(state, uid), before, "pre-drag values restored");
    assert_eq!(state.edits.undo_len(), steps, "nothing recorded");
    assert!(!state.edits.can_undo());
    assert_eq!(state.primary(), Some(uid), "Escape did not also deselect");
    assert!(!state.is_dirty());
}

// ----- 5: no-op edits record nothing -----

#[test]
fn renaming_to_the_same_name_and_dropping_in_place_record_nothing() {
    let mut world = World::with_registry(box_engine::shared_engine_registry());
    let id = Uid::from_u128;
    for (n, name) in [(1, "A"), (2, "B")] {
        world
            .spawn_saved(id(n), name, (Transform::default(),))
            .unwrap();
    }
    let mut state = state_with(world);
    let generation = state.generation;

    state.rename(id(1), "A");
    assert_eq!(state.edits.undo_len(), 0, "no Set Name step");
    assert!(!state.is_dirty());
    assert_eq!(state.generation, generation);

    // B is already the second root.
    assert!(state.reparent(id(2), None, Some(1)));
    assert_eq!(state.edits.undo_len(), 0);
    assert!(!state.is_dirty());
    assert_eq!(state.generation, generation, "nothing changed");

    state.rename(id(1), "House");
    assert_eq!(state.edits.undo_len(), 1);
    assert!(state.is_dirty());
}

// ----- 6: undoing back to the saved state is clean -----

#[test]
fn undo_to_the_saved_state_is_clean_and_diverged_history_is_not() {
    let (mut state, uid) = cube_state();
    let file = temp_dir("dirty").join("s.scene.json");
    let set_x = |state: &mut EditorState, x: f32| {
        state.set_field(uid, "Transform.translation", &Vec3::new(x, 0.0, 2.0));
    };
    set_x(&mut state, 1.0);
    state.save_scene_as(&file).unwrap();
    assert!(!state.is_dirty());

    set_x(&mut state, 2.0);
    assert!(state.is_dirty());
    state.undo();
    assert!(!state.is_dirty(), "back at the saved state");
    state.redo();
    assert!(state.is_dirty());
    state.undo();
    assert!(!state.is_dirty());

    // Below the save point, then a new edit: the same depth, another state.
    state.undo();
    assert!(state.is_dirty(), "before the saved state");
    set_x(&mut state, 5.0);
    assert_eq!(state.edits.undo_len(), 1, "same depth as when saved");
    assert!(state.is_dirty(), "diverged history is never clean");
    state.undo();
    assert!(state.is_dirty());
    state.redo();
    assert!(state.is_dirty());

    // Saving makes the new state the clean one.
    state.save_scene().unwrap();
    assert!(!state.is_dirty());
    state.undo();
    assert!(state.is_dirty());
    state.redo();
    assert!(!state.is_dirty());

    // An open drag is dirty until it is cancelled.
    start_drag(&mut state, 2);
    assert!(state.is_dirty(), "mid-drag");
    state.gizmo_end();
    assert!(state.is_dirty());
    state.undo();
    assert!(!state.is_dirty());
    let _ = std::fs::remove_dir_all(file.parent().unwrap());
}

// ----- 8: duplicate copies only the topmost selected entities -----

#[test]
fn duplicating_a_parent_and_its_child_copies_the_subtree_once() {
    let id = Uid::from_u128;
    for selection in [[id(1), id(2)], [id(2), id(1)]] {
        let mut world = World::with_registry(box_engine::shared_engine_registry());
        let _a = world
            .spawn_saved(id(1), "A", (Transform::default(),))
            .unwrap();
        let b = world
            .spawn_saved(id(2), "B", (Transform::default(),))
            .unwrap();
        world.place(b, Placement::last(Some(id(1)))).unwrap();
        let mut state = state_with(world);
        state.selection = selection.to_vec();
        state.duplicate_selection();
        assert_eq!(
            state.world.entity_count(),
            4,
            "A and B, and one copy of each ({selection:?})"
        );
        assert_eq!(state.selection.len(), 1, "the copy of A is selected");
        let copy = state.world.entity_by_uid(state.selection[0]).unwrap();
        assert!(state.world.parent(copy).is_none(), "a root, next to A");
        assert_eq!(state.world.children(copy).len(), 1);
        state.undo();
        assert_eq!(state.world.entity_count(), 2, "one undo step");
    }
}

// ----- 9: a failed save is shown, not only logged -----

#[cfg(unix)]
#[test]
fn a_failed_save_opens_an_error_dialog() {
    use std::os::unix::fs::PermissionsExt;
    let (mut editor, root) = project_editor("save_fails");
    editor.state.add_entity(NewEntity::Cube, None).unwrap();
    let scenes = root.join("assets/scenes");
    let before = std::fs::read(scenes.join("main.scene.json")).unwrap();
    std::fs::set_permissions(&scenes, std::fs::Permissions::from_mode(0o555)).unwrap();
    editor.save();
    std::fs::set_permissions(&scenes, std::fs::Permissions::from_mode(0o755)).unwrap();
    match editor.dialog() {
        Some(box_editor::Dialog::Error { title, message }) => {
            assert_eq!(title, "Could not save the scene");
            assert!(message.contains("main.scene"), "{message}");
        }
        other => panic!("the failure must be shown to the user, got {other:?}"),
    }
    assert!(editor.state.is_dirty(), "still unsaved");
    // Shortcuts are off while it is open; it closes with OK.
    editor.dismiss_error();
    assert_eq!(editor.dialog(), None);
    assert_eq!(
        std::fs::read(scenes.join("main.scene.json")).unwrap(),
        before
    );
    let _ = std::fs::remove_dir_all(&root);
}

// ----- 11: non-finite text in a drag field -----

/// Types `text` into the first field of an `axes_edit` and commits with Enter.
fn type_into_axes_edit(text: &str, start: f32) -> f32 {
    let ctx = egui::Context::default();
    box_editor::theme::apply(&ctx, &box_editor::theme::DARK);
    let mut v = [start, 0.0, 0.0];
    let mut field = Rect::NOTHING;
    let mut time = 0.0;
    let mut frame = |v: &mut [f32; 3], field: &mut Rect, events: Vec<Event>| {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 200.0))),
            time: Some(time),
            focused: true,
            events,
            ..Default::default()
        };
        time += 1.0 / 60.0;
        let _ = ctx.run_ui(raw, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.allocate_ui(vec2(300.0, 20.0), |ui| {
                    let r = box_editor::widgets::axes_edit(ui, v, 0.01, 2, "", None);
                    // The union's rect spans all three fields.
                    *field = Rect::from_min_size(r.rect.min, vec2(r.rect.width() / 3.0, 20.0));
                });
            });
        });
    };
    frame(&mut v, &mut field, Vec::new());
    let pos = field.center();
    let press = |pressed| Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    let key = |pressed| Event::Key {
        key: Key::Enter,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: Modifiers::NONE,
    };
    for events in [
        vec![Event::PointerMoved(pos)],
        vec![press(true)],
        vec![press(false)],
        Vec::new(),
        Vec::new(),
        vec![Event::Text(text.into())],
        vec![key(true)],
        vec![key(false)],
        Vec::new(),
    ] {
        frame(&mut v, &mut field, events);
    }
    v[0]
}

#[test]
fn typed_numbers_reach_a_drag_field_but_non_finite_ones_do_not() {
    assert_eq!(type_into_axes_edit("2.5", 1.0), 2.5, "the harness types");
    for text in ["inf", "-inf", "nan", "NaN", "infinity", "1e39"] {
        let v = type_into_axes_edit(text, 1.0);
        assert!(v.is_finite(), "`{text}` became {v}");
        assert_eq!(v, 1.0, "`{text}` is rejected, the value stays");
    }
}

// ----- 12: Escape that closes a menu keeps the selection -----

#[test]
fn escape_closing_a_context_menu_keeps_the_selection() {
    let mut editor = build_editor(&EditorOptions::default(), LogBuffer::default()).unwrap();
    let cube = editor.state.add_entity(NewEntity::Cube, None).unwrap();
    assert_eq!(editor.state.primary(), Some(cube));
    let mut ui = Driver::new(&editor);
    ui.frames(&mut editor, 3);

    // Right-click the first entity row (below the 34 pt search bar and the
    // pinned Scene row).
    let h = pane_rect(&editor, Pane::Hierarchy);
    let row_h = box_editor::theme::DARK.row_h;
    ui.click(
        &mut editor,
        pos2(h.center().x, h.top() + 34.0 + row_h * 1.5),
        PointerButton::Secondary,
    );
    assert!(egui::Popup::is_any_open(&ui.ctx), "context menu open");
    assert_eq!(editor.state.primary(), Some(cube));

    ui.key(&mut editor, Key::Escape, Modifiers::NONE);
    ui.frames(&mut editor, 2);
    assert!(!egui::Popup::is_any_open(&ui.ctx), "Escape closed the menu");
    assert_eq!(
        editor.state.primary(),
        Some(cube),
        "closing a menu must not clear the selection"
    );

    // With nothing else open, Escape still deselects.
    ui.key(&mut editor, Key::Escape, Modifiers::NONE);
    assert_eq!(editor.state.primary(), None);
}

// ----- 10: a hidden viewport tab is not rendered -----

fn skip_gpu() -> bool {
    std::env::var_os("BOX_SKIP_GPU_TESTS").is_some()
}

fn no_adapter(e: &anyhow::Error) -> bool {
    matches!(
        e.downcast_ref::<box_render::RenderError>(),
        Some(box_render::RenderError::NoAdapter)
    )
}

#[test]
fn the_scene_is_not_rendered_while_the_viewport_tab_is_hidden() {
    use box_editor::headless::{CaptureOptions, Headless};
    if skip_gpu() {
        return;
    }
    let mut editor = build_editor(&EditorOptions::default(), LogBuffer::default()).unwrap();
    editor.tree = box_editor::layout::default_layout();
    let options = CaptureOptions {
        size: [1280, 800],
        pixels_per_point: 1.0,
        frames: 1,
    };
    let mut h = match Headless::new(&mut editor, &options) {
        Ok(h) => h,
        Err(e) if no_adapter(&e) => return,
        Err(e) => panic!("{e:#}"),
    };
    for _ in 0..3 {
        assert!(h.frame(&mut editor, Vec::new()).scene_rendered);
    }
    assert!(editor.state.status.draws > 0);

    // Dock the console as a tab next to the viewport and bring it to front.
    let tiles = &editor.tree.tiles;
    let viewport = tiles.find_pane(&Pane::Viewport).unwrap();
    let console = tiles.find_pane(&Pane::Console).unwrap();
    let tabs = tiles.parent_of(viewport).unwrap();
    editor.tree.move_tile_to_container(console, tabs, 1, false);
    let show = |editor: &mut Editor, tile| match editor.tree.tiles.get_mut(tabs) {
        Some(egui_tiles::Tile::Container(egui_tiles::Container::Tabs(t))) => t.set_active(tile),
        other => panic!("expected the viewport's tab container, got {other:?}"),
    };
    show(&mut editor, console);
    for _ in 0..3 {
        let t = h.frame(&mut editor, Vec::new());
        assert!(!t.scene_rendered, "hidden viewport still rendered");
    }
    assert_eq!(editor.state.status.draws, 0);
    assert!(!editor.state.viewport.hovered);
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/review");
    std::fs::create_dir_all(&out).unwrap();
    h.image()
        .unwrap()
        .save(out.join("viewport_tab_hidden.png"))
        .unwrap();

    // Shown again, it renders again.
    show(&mut editor, viewport);
    h.frame(&mut editor, Vec::new());
    assert!(h.frame(&mut editor, Vec::new()).scene_rendered);
    assert!(editor.state.status.draws > 0);
    h.image().unwrap();
}

// ----- captures of the modals this review added or changed -----

/// Renders the save-error and recovery modals to
/// `target/captures/review/`. Skipped with `BOX_SKIP_GPU_TESTS` or without
/// an adapter.
#[cfg(unix)]
#[test]
fn error_and_recovery_dialog_captures() {
    use box_editor::headless::{CaptureOptions, capture};
    use std::os::unix::fs::PermissionsExt;
    if skip_gpu() {
        return;
    }
    let options = CaptureOptions {
        size: [1600, 1000],
        pixels_per_point: 2.0,
        // Past the modal's fade-in.
        frames: 30,
    };
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/review");
    std::fs::create_dir_all(&out).unwrap();
    let shoot = |editor: &mut Editor, name: &str| {
        editor.state.console.log.set_manual_time(0.0);
        editor.tree = box_editor::layout::default_layout();
        match capture(editor, &options) {
            Ok(image) => image.save(out.join(name)).unwrap(),
            Err(e) if no_adapter(&e) => {}
            Err(e) => panic!("{e:#}"),
        }
    };

    let (mut editor, root) = project_editor("captures");
    editor.state.add_entity(NewEntity::Cube, None).unwrap();
    let scenes = root.join("assets/scenes");
    std::fs::set_permissions(&scenes, std::fs::Permissions::from_mode(0o555)).unwrap();
    editor.save();
    std::fs::set_permissions(&scenes, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        editor.dialog(),
        Some(box_editor::Dialog::Error { .. })
    ));
    shoot(&mut editor, "save_error_dialog.png");
    editor.dismiss_error();
    assert_eq!(editor.dialog(), None);

    // Quit with the unsaved cube, reopen: the recovery prompt.
    editor.on_quit();
    set_mtime(
        &scenes.join("main.scene.json"),
        SystemTime::now() - Duration::from_secs(3600),
    );
    let mut editor = reopen(&root);
    assert!(matches!(
        editor.dialog(),
        Some(box_editor::Dialog::Recovery { .. })
    ));
    shoot(&mut editor, "recovery_dialog.png");
    let _ = std::fs::remove_dir_all(&root);
}
