//! Everyday editing with less friction (docs/DX_REVIEW.md, "Editor
//! interaction"): where new objects go, Delete scoped by pane, F from
//! anywhere, the snap popover and Cmd inversion, New Scene, Point Light, the
//! searchable Add Component, the Scene row, Help ▸ Keyboard Shortcuts, the
//! hierarchy's background menu, slider precision and graceful startup.
//!
//! Headless and driven by egui events. Widgets are found by their text in
//! egui's accessibility tree, so the tests do not depend on pixel layout.
//! The last test writes captures and needs a GPU.

use std::path::{Path, PathBuf};

use box_core::identity::{Name, Uid};
use box_core::math::Aabb;
use box_core::prelude::{Entity, Transform};
use box_editor::layout::Pane;
use box_editor::log::LogBuffer;
use box_editor::panes::hierarchy::kind_of;
use box_editor::shortcuts::SHORTCUTS;
use box_editor::widgets::EntityKind;
use box_editor::{Dialog, DocAction, Editor, EditorOptions, NewEntity, build_editor};
use egui::accesskit::{Role, TreeUpdate};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use glam::Vec3;

// ----- an egui driver that reads the accessibility tree -----

#[derive(Clone, Debug)]
struct Node {
    role: Role,
    /// The label, or a plain label's text.
    text: String,
    number: Option<f64>,
    rect: Rect,
}

fn nodes_of(update: Option<&TreeUpdate>) -> Vec<Node> {
    let Some(u) = update else {
        return Vec::new();
    };
    u.nodes
        .iter()
        .filter_map(|(_, n)| {
            let b = n.bounds()?;
            let text = n.label().or(n.value()).unwrap_or_default().to_string();
            Some(Node {
                role: n.role(),
                text,
                number: n.numeric_value(),
                rect: Rect::from_min_max(
                    pos2(b.x0 as f32, b.y0 as f32),
                    pos2(b.x1 as f32, b.y1 as f32),
                ),
            })
        })
        .collect()
}

struct Driver {
    ctx: egui::Context,
    time: f64,
    nodes: Vec<Node>,
    modifiers: Modifiers,
}

const SCREEN: egui::Vec2 = vec2(1280.0, 800.0);

impl Driver {
    fn new(editor: &Editor) -> Self {
        let ctx = egui::Context::default();
        editor.apply_theme(&ctx);
        ctx.enable_accesskit();
        Self {
            ctx,
            time: 0.0,
            nodes: Vec::new(),
            modifiers: Modifiers::NONE,
        }
    }

    fn frame(&mut self, editor: &mut Editor, mut events: Vec<Event>) {
        events.insert(0, Event::ModifiersChanged(self.modifiers));
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
            time: Some(self.time),
            predicted_dt: 1.0 / 60.0,
            focused: true,
            events,
            ..Default::default()
        };
        let out = self.ctx.run_ui(raw, |ui| editor.ui(ui));
        self.nodes = nodes_of(out.platform_output.accesskit_update.as_ref());
        self.time += 1.0 / 60.0;
    }

    fn frames(&mut self, editor: &mut Editor, n: usize) {
        for _ in 0..n {
            self.frame(editor, Vec::new());
        }
    }

    /// The last widget whose text is `text` (text fields excluded: their
    /// value is what was typed).
    fn find(&self, text: &str) -> Option<Node> {
        self.nodes
            .iter()
            .rev()
            .find(|n| n.text == text && n.role != Role::TextInput)
            .cloned()
    }

    fn get(&self, text: &str) -> Node {
        self.find(text).unwrap_or_else(|| {
            let all: Vec<&str> = self.nodes.iter().map(|n| n.text.as_str()).collect();
            panic!("no widget `{text}` among {all:?}")
        })
    }

    fn press(&mut self, editor: &mut Editor, pos: Pos2, button: PointerButton) {
        let modifiers = self.modifiers;
        let b = |pressed| Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers,
        };
        self.frame(editor, vec![Event::PointerMoved(pos)]);
        self.frame(editor, vec![b(true)]);
        self.frame(editor, vec![b(false)]);
        self.frame(editor, Vec::new());
    }

    fn click_at(&mut self, editor: &mut Editor, pos: Pos2) {
        self.press(editor, pos, PointerButton::Primary);
    }

    /// Scrolls the panel under the widget labelled `text` until the widget
    /// is on screen (a long inspector can push it below the window).
    fn scroll_to(&mut self, editor: &mut Editor, text: &str) {
        for _ in 0..40 {
            let r = self.get(text).rect;
            if r.bottom() <= SCREEN.y - 8.0 {
                break;
            }
            let over = pos2(r.center().x, SCREEN.y * 0.6);
            let scroll = Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: vec2(0.0, -120.0),
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Wheel,
                modifiers: Modifiers::NONE,
            };
            self.frame(editor, vec![Event::PointerMoved(over), scroll]);
        }
        // Let smooth scrolling settle.
        self.frames(editor, 30);
    }

    /// Clicks the widget labelled `text`.
    fn click(&mut self, editor: &mut Editor, text: &str) {
        let pos = self.get(text).rect.center();
        self.click_at(editor, pos);
        self.frames(editor, 2);
    }

    fn double_click_at(&mut self, editor: &mut Editor, pos: Pos2) {
        let b = |pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        self.frame(editor, vec![Event::PointerMoved(pos)]);
        self.frame(editor, vec![b(true), b(false)]);
        self.frame(editor, vec![b(true), b(false)]);
        self.frame(editor, Vec::new());
    }

    fn drag(&mut self, editor: &mut Editor, from: Pos2, to: Pos2) {
        let modifiers = self.modifiers;
        let b = |pressed, pos| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers,
        };
        self.frame(editor, vec![Event::PointerMoved(from)]);
        self.frame(editor, vec![b(true, from)]);
        for k in 1..=8 {
            let p = from + (to - from) * (k as f32 / 8.0);
            self.frame(editor, vec![Event::PointerMoved(p)]);
        }
        self.frame(editor, vec![b(false, to)]);
        self.frame(editor, Vec::new());
    }

    fn key(&mut self, editor: &mut Editor, key: Key, modifiers: Modifiers) {
        let e = |pressed| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        };
        let before = self.modifiers;
        self.modifiers = modifiers;
        self.frame(editor, vec![e(true)]);
        self.frame(editor, vec![e(false)]);
        self.modifiers = before;
        self.frame(editor, Vec::new());
    }

    fn type_text(&mut self, editor: &mut Editor, text: &str) {
        self.frame(editor, vec![Event::Text(text.into())]);
    }
}

fn pane(editor: &Editor, pane: Pane) -> Rect {
    let id = editor.tree.tiles.find_pane(&pane).unwrap();
    editor.tree.tiles.rect(id).unwrap()
}

// ----- projects -----

fn temp_project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("box_dx_{name}_{}", Uid::new()));
    let _ = std::fs::remove_dir_all(&dir);
    box_engine::Project::create(&dir, name).unwrap();
    dir
}

fn open(dir: &Path) -> Editor {
    let options = EditorOptions {
        project: Some(dir.to_path_buf()),
        ..Default::default()
    };
    let mut editor = build_editor(&options, LogBuffer::default()).unwrap();
    editor.tree = box_editor::layout::default_layout();
    editor
}

fn editor_and_driver(dir: &Path) -> (Editor, Driver) {
    let mut editor = open(dir);
    let mut d = Driver::new(&editor);
    d.frames(&mut editor, 3);
    (editor, d)
}

fn by_name(editor: &Editor, name: &str) -> Option<Uid> {
    editor
        .state
        .world
        .query::<(&Uid, &Name)>()
        .iter()
        .find(|(_, n)| n.0 == name)
        .map(|(u, _)| *u)
}

fn entity(editor: &Editor, uid: Uid) -> Entity {
    editor.state.world.entity_by_uid(uid).unwrap()
}

fn translation(editor: &Editor, uid: Uid) -> Vec3 {
    editor
        .state
        .world
        .get::<Transform>(entity(editor, uid))
        .unwrap()
        .translation
}

fn unit_cube_at(c: Vec3) -> Aabb {
    Aabb::new(c - Vec3::splat(0.5), c + Vec3::splat(0.5))
}

fn overlap(a: &Aabb, b: &Aabb) -> bool {
    let eps = 1e-3;
    (0..3).all(|i| a.min[i] < b.max[i] - eps && b.min[i] < a.max[i] - eps)
}

// ----- E3: placement -----

/// Add ▸ Cube in a fresh project lands where the viewport centre looks,
/// resting on a surface, never inside the starter cube or the previous one.
#[test]
fn new_cubes_rest_on_the_surface_without_overlapping() {
    let dir = temp_project("place");
    let (mut ed, mut d) = editor_and_driver(&dir);
    let starter = by_name(&ed, "Cube").unwrap();
    let mut boxes = vec![unit_cube_at(translation(&ed, starter))];
    for _ in 0..3 {
        d.click(&mut ed, "+ Add");
        d.click(&mut ed, "Cube");
        let new = ed.state.primary().expect("the new cube is selected");
        assert_ne!(new, starter);
        let at = translation(&ed, new);
        let bottom = at.y - 0.5;
        // On the ground (y = 0) or on top of another cube (y = 1, 2, ...).
        assert!(
            (bottom - bottom.round()).abs() < 0.02 && bottom > -0.02,
            "rests on a surface: {at}"
        );
        let b = unit_cube_at(at);
        for other in &boxes {
            assert!(!overlap(&b, other), "{at} overlaps {other:?}");
        }
        boxes.push(b);
    }
    assert_eq!(ed.state.edits.undo_len(), 3, "one undo step per add");
    std::fs::remove_dir_all(&dir).unwrap();
}

// ----- E15: Point Light -----

#[test]
fn point_light_is_in_the_add_menu_with_its_own_icon() {
    let dir = temp_project("point");
    let (mut ed, mut d) = editor_and_driver(&dir);
    d.click(&mut ed, "+ Add");
    d.click(&mut ed, "Point Light");
    let uid = ed.state.primary().unwrap();
    let e = entity(&ed, uid);
    assert!(ed.state.world.has::<box_render::PointLight>(e));
    assert_eq!(kind_of(&ed.state.world, e), EntityKind::PointLight);
    assert_eq!(ed.state.display_name(e), "Point Light");
    // Hangs above the surface it was placed on.
    assert!(translation(&ed, uid).y >= 2.4, "{}", translation(&ed, uid));
    std::fs::remove_dir_all(&dir).unwrap();
}

// ----- E4: Delete and Duplicate scoped by pane -----

#[test]
fn delete_in_the_asset_browser_never_deletes_entities() {
    let dir = temp_project("delete");
    let (mut ed, mut d) = editor_and_driver(&dir);
    let cube = by_name(&ed, "Cube").unwrap();
    ed.state.select(Some(cube));
    // A click in the asset browser makes it the active pane.
    let assets = pane(&ed, Pane::Assets);
    d.click_at(&mut ed, assets.center() + vec2(0.0, 60.0));
    assert!(ed.state.assets.active);
    ed.state.select(Some(cube));
    d.key(&mut ed, Key::Delete, Modifiers::NONE);
    d.key(&mut ed, Key::D, Modifiers::COMMAND);
    assert!(ed.state.world.entity_by_uid(cube).is_some(), "not deleted");
    assert_eq!(ed.state.edits.undo_len(), 0, "nothing duplicated either");
    assert!(
        ed.dialog().is_none(),
        "no asset selected: nothing to delete"
    );

    // With an asset selected, Delete asks, then moves it to .box/trash/.
    let id = ed.state.new_material_asset("").unwrap();
    let rel = ed.state.asset_server().unwrap().path_of(id).unwrap();
    let file = dir.join("assets").join(&rel);
    assert!(file.is_file());
    d.frames(&mut ed, 2);
    d.key(&mut ed, Key::Backspace, Modifiers::NONE);
    assert!(
        matches!(ed.dialog(), Some(Dialog::DeleteAsset { path, .. }) if *path == rel),
        "{:?}",
        ed.dialog()
    );
    assert!(file.is_file(), "nothing happens before the confirmation");
    d.click(&mut ed, "Move to Trash");
    assert!(ed.dialog().is_none());
    assert!(!file.exists());
    let trashed: Vec<PathBuf> = walk(&dir.join(".box/trash"));
    assert!(
        trashed.iter().any(|p| p.ends_with(&rel)),
        "in the trash: {trashed:?}"
    );
    assert!(
        trashed
            .iter()
            .any(|p| p.to_string_lossy().ends_with(".meta")),
        "with its .meta: {trashed:?}"
    );
    assert!(ed.state.world.entity_by_uid(cube).is_some());

    // Back in the viewport, Delete deletes the selected entity.
    let vp = pane(&ed, Pane::Viewport);
    d.click_at(&mut ed, vp.left_top() + vec2(30.0, 60.0));
    assert!(!ed.state.assets.active);
    ed.state.select(Some(cube));
    d.key(&mut ed, Key::Delete, Modifiers::NONE);
    assert!(ed.state.world.entity_by_uid(cube).is_none());
    std::fs::remove_dir_all(&dir).unwrap();
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out
}

// ----- E9: F from anywhere, double-click on a row icon -----

#[test]
fn f_frames_the_selection_with_the_pointer_over_the_hierarchy() {
    let dir = temp_project("frame");
    let (mut ed, mut d) = editor_and_driver(&dir);
    let sun = by_name(&ed, "Camera").unwrap();
    ed.state.select(Some(sun));
    let h = pane(&ed, Pane::Hierarchy);
    d.frame(&mut ed, vec![Event::PointerMoved(h.center())]);
    let before = ed.state.camera.orbit.target;
    d.key(&mut ed, Key::F, Modifiers::NONE);
    let cube = by_name(&ed, "Cube").unwrap();
    ed.state.select(Some(cube));
    d.key(&mut ed, Key::F, Modifiers::NONE);
    let b = box_editor::entity_bounds(&ed.state.world, entity(&ed, cube)).unwrap();
    assert!(
        (ed.state.camera.orbit.target - b.center()).length() < 1e-4,
        "framed the cube from the hierarchy (target was {before})"
    );

    // Double-click on the Ground row's icon selects and frames it.
    let row_h = box_editor::theme::DARK.row_h;
    let ground = by_name(&ed, "Ground").unwrap();
    let index = ed
        .state
        .hierarchy
        .rows
        .iter()
        .position(|r| r.uid == ground)
        .unwrap();
    let icon = pos2(
        h.left() + 8.0 + 20.0,
        h.top() + 34.0 + row_h * (index as f32 + 1.5),
    );
    d.double_click_at(&mut ed, icon);
    assert_eq!(ed.state.primary(), Some(ground));
    assert!(ed.state.hierarchy.renaming_uid().is_none(), "not a rename");
    let g = box_editor::entity_bounds(&ed.state.world, entity(&ed, ground)).unwrap();
    assert!((ed.state.camera.orbit.target - g.center()).length() < 1e-3);
    std::fs::remove_dir_all(&dir).unwrap();
}

// ----- E10: snap popover, prefs, Cmd inversion, shortcut -----

#[test]
fn snap_steps_are_edited_in_a_popover_and_kept_per_project() {
    let dir = temp_project("snap");
    let (mut ed, mut d) = editor_and_driver(&dir);
    d.click(&mut ed, "0.5 m");
    let row = d.get("Move").rect;
    let field = d
        .nodes
        .iter()
        .filter(|n| n.role == Role::SpinButton)
        .find(|n| (n.rect.center().y - row.center().y).abs() < 4.0)
        .cloned()
        .expect("the Move step field");
    assert_eq!(field.number, Some(0.5));
    let from = field.rect.center();
    d.drag(&mut ed, from, from + vec2(40.0, 0.0));
    let step = ed.state.snap.translate;
    assert!(step > 0.6, "dragging the field raised the step: {step}");
    // Written when the drag ended, read back by the next editor.
    let prefs = box_editor::prefs::load(&dir.join(".box/editor.json"));
    assert_eq!(prefs.snap.translate, step);
    // Y toggles snapping, also stored.
    d.key(&mut ed, Key::Escape, Modifiers::NONE);
    d.key(&mut ed, Key::Y, Modifiers::NONE);
    assert!(ed.state.snap.enabled);
    drop(ed);
    let again = open(&dir);
    assert_eq!(again.state.snap.translate, step);
    assert!(again.state.snap.enabled);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Drags the gizmo's X arrow of the selection by `dx` points with
/// `modifiers` held; returns the selection's X after the drag.
fn gizmo_drag_x(ed: &mut Editor, d: &mut Driver, uid: Uid, dx: f32, modifiers: Modifiers) -> f32 {
    // The renderer propagates transforms each frame; these frames render nothing.
    box_core::transform::propagate_transforms(&mut ed.state.world);
    let view = ed.state.gizmo_view().unwrap();
    let frame = ed.state.gizmo_frame(&view).unwrap();
    let rect = ed.state.viewport.rect.unwrap();
    let grab = view
        .project(frame.origin + frame.axes[0] * frame.size * 0.6)
        .unwrap();
    let grab = rect.min + vec2(grab.x, grab.y);
    d.modifiers = modifiers;
    d.drag(ed, grab, grab + vec2(dx, 0.0));
    d.modifiers = Modifiers::NONE;
    d.frames(ed, 1);
    translation(ed, uid).x
}

#[test]
fn holding_cmd_during_a_gizmo_drag_inverts_snapping() {
    let dir = temp_project("invert");
    let (mut ed, mut d) = editor_and_driver(&dir);
    let cube = by_name(&ed, "Cube").unwrap();
    ed.state.select(Some(cube));
    ed.state.snap.translate = 0.5;
    ed.state.snap.enabled = false;
    d.frames(&mut ed, 2);
    // Snapping steps the drag's offset.
    let on_step = |dx: f32| dx.abs() > 1e-3 && ((dx / 0.5) - (dx / 0.5).round()).abs() < 1e-3;
    let x0 = translation(&ed, cube).x;
    let free = gizmo_drag_x(&mut ed, &mut d, cube, 37.0, Modifiers::NONE) - x0;
    assert!(!on_step(free), "snapping off: moved {free}");
    let x1 = translation(&ed, cube).x;
    let snapped = gizmo_drag_x(&mut ed, &mut d, cube, 53.0, Modifiers::COMMAND) - x1;
    assert!(on_step(snapped), "Cmd snaps: moved {snapped}");
    ed.state.snap.enabled = true;
    let x2 = translation(&ed, cube).x;
    let inverted = gizmo_drag_x(&mut ed, &mut d, cube, 29.0, Modifiers::COMMAND) - x2;
    assert!(
        !on_step(inverted),
        "Cmd with snapping on moves freely: {inverted}"
    );
    let x3 = translation(&ed, cube).x;
    let on = gizmo_drag_x(&mut ed, &mut d, cube, 41.0, Modifiers::NONE) - x3;
    assert!(on_step(on), "snapping on: moved {on}");
    assert_eq!(ed.state.edits.undo_len(), 4, "one step per drag");
    std::fs::remove_dir_all(&dir).unwrap();
}

// ----- E7: New Scene -----

#[test]
fn new_scene_asks_about_unsaved_changes_and_saves_as_into_scenes() {
    let dir = temp_project("newscene");
    let (mut ed, mut d) = editor_and_driver(&dir);
    ed.state.add_entity(NewEntity::Sphere, None).unwrap();
    assert!(ed.state.is_dirty());
    d.click(&mut ed, "File");
    d.click(&mut ed, "New Scene");
    assert!(
        matches!(
            ed.dialog(),
            Some(Dialog::Unsaved {
                action: DocAction::NewScene,
                ..
            })
        ),
        "{:?}",
        ed.dialog()
    );
    d.click(&mut ed, "Don't Save");
    assert!(ed.dialog().is_none());
    let names: Vec<String> = ed
        .state
        .world
        .query::<&Name>()
        .iter()
        .map(|n| n.0.clone())
        .collect();
    for n in ["Sun", "Ground", "Camera"] {
        assert!(names.iter().any(|x| x == n), "{n} in {names:?}");
    }
    assert!(
        !names.iter().any(|x| x == "Cube" || x == "Sphere"),
        "{names:?}"
    );
    assert!(
        ed.state
            .world
            .contains_resource::<box_render::Environment>()
    );
    assert_eq!(ed.state.scene.path, None);
    assert_eq!(ed.state.scene.name, "untitled");
    assert!(!ed.state.is_dirty());
    // The start scene kept its contents (Don't Save).
    let main = std::fs::read_to_string(dir.join("assets/scenes/main.scene.json")).unwrap();
    assert!(!main.contains("Sphere"));

    // Save goes to Save As, in assets/scenes/.
    d.key(&mut ed, Key::S, Modifiers::COMMAND);
    match ed.dialog() {
        Some(Dialog::SaveAs { path, .. }) => {
            assert_eq!(path, "assets/scenes/untitled.scene.json")
        }
        other => panic!("{other:?}"),
    }
    ed.answer_save_as(Some("assets/scenes/untitled.scene.json"));
    assert!(dir.join("assets/scenes/untitled.scene.json").is_file());
    assert!(ed.state.scene.path.is_some());

    // Assets ▸ New ▸ Scene makes a file with the same contents.
    let id = ed.state.new_scene_asset("scenes").unwrap();
    let rel = ed.state.asset_server().unwrap().path_of(id).unwrap();
    assert_eq!(rel, "scenes/New Scene.scene.json");
    let text = std::fs::read_to_string(dir.join("assets").join(&rel)).unwrap();
    assert!(text.contains("\"Sun\"") && text.contains("\"Ground\""));
    std::fs::remove_dir_all(&dir).unwrap();
}

// ----- E16: Add Component -----

#[test]
fn add_component_is_searchable_with_friendly_names_and_categories() {
    let dir = temp_project("addcomp");
    let (mut ed, mut d) = editor_and_driver(&dir);
    let cube = by_name(&ed, "Cube").unwrap();
    ed.state.select(Some(cube));
    d.frames(&mut ed, 2);
    d.scroll_to(&mut ed, "Add Component");
    d.click(&mut ed, "Add Component");
    for heading in ["Lights", "Assets & Prefabs"] {
        assert!(d.find(heading).is_some(), "category {heading}");
    }
    assert!(
        d.nodes.iter().any(|n| n.text.starts_with("Point Light")),
        "friendly names"
    );
    // The search field has focus: typing filters, Enter adds the top match.
    d.type_text(&mut ed, "point");
    d.frames(&mut ed, 1);
    assert!(
        !d.nodes.iter().any(|n| n.text.starts_with("Mesh Asset")),
        "filtered"
    );
    d.key(&mut ed, Key::Enter, Modifiers::NONE);
    assert!(
        ed.state
            .world
            .has::<box_render::PointLight>(entity(&ed, cube))
    );
    assert_eq!(ed.state.edits.undo_len(), 1);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn friendly_component_names() {
    use box_editor::inspect::{ComponentCategory, component_category, friendly_name};
    assert_eq!(friendly_name("StandardMaterial"), "Standard Material");
    assert_eq!(friendly_name("UVOffset"), "UV Offset");
    assert_eq!(friendly_name("Transform"), "Transform");
    assert_eq!(component_category("PointLight"), ComponentCategory::Lights);
    assert_eq!(component_category("Collider"), ComponentCategory::Physics);
    assert_eq!(
        component_category("MyGameThing"),
        ComponentCategory::Gameplay
    );
}

// ----- E23: Scene Settings -----

#[test]
fn the_scene_row_shows_the_environment() {
    let dir = temp_project("scenerow");
    let (mut ed, mut d) = editor_and_driver(&dir);
    let cube = by_name(&ed, "Cube").unwrap();
    ed.state.select(Some(cube));
    d.frames(&mut ed, 1);
    assert!(!ed.state.scene_settings_shown());
    let h = pane(&ed, Pane::Hierarchy);
    let row_h = box_editor::theme::DARK.row_h;
    d.click_at(&mut ed, pos2(h.center().x, h.top() + 34.0 + row_h / 2.0));
    assert!(ed.state.selection.is_empty());
    assert!(ed.state.scene_settings_shown());
    // View ▸ Scene Settings does the same.
    ed.state.select(Some(cube));
    d.click(&mut ed, "View");
    d.click(&mut ed, "Scene Settings");
    assert!(ed.state.scene_settings_shown());
    std::fs::remove_dir_all(&dir).unwrap();
}

// ----- E18: hierarchy background menu, empty scene -----

#[test]
fn right_click_below_the_rows_adds_objects() {
    let dir = temp_project("bgmenu");
    let (mut ed, mut d) = editor_and_driver(&dir);
    let count = ed.state.world.entity_count();
    let h = pane(&ed, Pane::Hierarchy);
    d.press(
        &mut ed,
        h.center_bottom() - vec2(0.0, 30.0),
        PointerButton::Secondary,
    );
    d.click(&mut ed, "Add");
    d.click(&mut ed, "Sphere");
    assert_eq!(ed.state.world.entity_count(), count + 1);
    let uid = ed.state.primary().unwrap();
    assert_eq!(ed.state.display_name(entity(&ed, uid)), "Sphere");
    assert!(ed.state.world.parent(entity(&ed, uid)).is_none(), "a root");
    std::fs::remove_dir_all(&dir).unwrap();
}

// ----- E5a: shortcuts table, Help, tooltips -----

#[test]
fn help_lists_every_shortcut_and_the_navigation() {
    let dir = temp_project("help");
    let (mut ed, mut d) = editor_and_driver(&dir);
    d.click(&mut ed, "Help");
    d.click(&mut ed, "Keyboard Shortcuts…");
    assert!(matches!(ed.dialog(), Some(Dialog::Shortcuts)));
    for s in SHORTCUTS {
        assert!(d.find(s.label).is_some(), "Help lists `{}`", s.label);
    }
    for (what, how) in box_editor::NAVIGATION {
        assert!(d.find(how).is_some(), "{what}: {how}");
    }
    d.key(&mut ed, Key::Escape, Modifiers::NONE);
    assert!(ed.dialog().is_none());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn toolbar_tooltips_name_their_shortcuts() {
    let dir = temp_project("tips");
    let (mut ed, mut d) = editor_and_driver(&dir);
    // The snap toggle sits just left of its step.
    let step = d.get("0.5 m").rect;
    let snap = pos2(step.left() - 15.0, step.center().y);
    d.frame(&mut ed, vec![Event::PointerMoved(snap)]);
    d.frames(&mut ed, 60);
    let tip = d
        .nodes
        .iter()
        .find(|n| n.text.starts_with("Snap (Y)"))
        .map(|n| n.text.clone());
    assert!(tip.is_some_and(|t| t.contains("invert")), "snap tooltip");
    std::fs::remove_dir_all(&dir).unwrap();
}

// ----- E22: slider precision -----

#[test]
fn logarithmic_sliders_spread_small_values() {
    use box_editor::widgets::{slider_fraction, slider_value};
    // Fog density 0.006 on 0..1 sits near the middle, not at the left edge.
    let f = slider_fraction(0.006, 0.0, 1.0, true);
    assert!(f > 0.35 && f < 0.6, "{f}");
    assert!((slider_value(f, 0.0, 1.0, true) - 0.006).abs() < 1e-6);
    assert_eq!(slider_value(0.0, 0.0, 1.0, true), 0.0);
    assert_eq!(slider_fraction(0.006, 0.0, 1.0, false), 0.006);
    let env = box_editor::inspect::resource_sections(&{
        let mut w = box_core::prelude::World::with_registry(box_engine::shared_engine_registry());
        w.insert_resource(box_render::Environment::day());
        w
    });
    let fog = env
        .iter()
        .flat_map(|s| &s.rows)
        .find(|r| r.path == "Environment.fog_density")
        .unwrap();
    assert!(fog.meta.as_ref().unwrap().logarithmic);
}

// ----- E27: graceful startup -----

#[test]
fn a_bad_project_path_opens_the_scratch_scene_with_an_error() {
    let missing = std::env::temp_dir().join(format!("box_dx_missing_{}", Uid::new()));
    let options = EditorOptions {
        project: Some(missing),
        ..Default::default()
    };
    let ed = build_editor(&options, LogBuffer::default()).expect("the editor opens");
    assert!(ed.state.project.is_none());
    assert_eq!(ed.state.scene.name, "scratch");
    match ed.dialog() {
        Some(Dialog::Error { title, message }) => {
            assert_eq!(title, "Could not open the project");
            assert!(message.contains("box new"), "{message}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_broken_start_scene_opens_the_project_with_an_error() {
    let dir = temp_project("broken");
    let scene = dir.join("assets/scenes/main.scene.json");
    std::fs::write(&scene, "{ not json").unwrap();
    let ed = open(&dir);
    assert!(ed.state.project.is_some(), "the project is open");
    assert_eq!(ed.state.scene.path, None, "Save never overwrites the file");
    assert!(ed.state.scene_is_empty());
    match ed.dialog() {
        Some(Dialog::Error { title, message }) => {
            assert_eq!(title, "Could not load the start scene");
            assert!(message.contains("main.scene.json"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(std::fs::read_to_string(&scene).unwrap(), "{ not json");
    std::fs::remove_dir_all(&dir).unwrap();
}

// ----- captures -----

fn skip_gpu() -> bool {
    std::env::var_os("BOX_SKIP_GPU_TESTS").is_some()
}

/// Captures for review in `target/captures/dx/`: Help ▸ Keyboard Shortcuts,
/// the snap tooltip and popover, Add Component with a search, an empty
/// scene's hints, the Scene row with the environment, and new cubes placed
/// in a fresh project.
#[test]
fn discoverability_captures() {
    use box_editor::headless::{CaptureOptions, Headless};
    if skip_gpu() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping captures");
        return;
    }
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/dx");
    std::fs::create_dir_all(&out).unwrap();
    let dir = temp_project("captures");
    let mut ed = open(&dir);
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
        nodes_of(h.accesskit_update())
    };
    let find = |nodes: &[Node], text: &str| -> Pos2 {
        nodes
            .iter()
            .rev()
            .find(|n| n.text == text)
            .unwrap_or_else(|| panic!("no `{text}`"))
            .rect
            .center()
    };
    let press = |pos, pressed| Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    let click = |h: &mut Headless, ed: &mut Editor, pos: Pos2| {
        run(h, ed, vec![Event::PointerMoved(pos)]);
        run(h, ed, vec![press(pos, true), press(pos, false)]);
        let mut n = Vec::new();
        for _ in 0..20 {
            n = run(h, ed, Vec::new());
        }
        n
    };
    let save = |h: &Headless, name: &str| {
        h.image().unwrap().save(out.join(name)).unwrap();
    };
    let mut nodes = Vec::new();
    for _ in 0..3 {
        nodes = run(&mut h, &mut ed, Vec::new());
    }

    // New cubes placed in a fresh project (two adds).
    for _ in 0..2 {
        let n = click(&mut h, &mut ed, find(&nodes, "+ Add"));
        nodes = click(&mut h, &mut ed, find(&n, "Cube"));
    }
    save(&h, "new_cube_placement.png");

    // Snap tooltip, then the popover.
    let step = nodes.iter().find(|n| n.text == "0.5 m").unwrap().rect;
    let snap = pos2(step.left() - 15.0, step.center().y);
    run(&mut h, &mut ed, vec![Event::PointerMoved(snap)]);
    for _ in 0..60 {
        run(&mut h, &mut ed, Vec::new());
    }
    save(&h, "snap_tooltip.png");
    let n = click(&mut h, &mut ed, step.center());
    save(&h, "snap_popover.png");
    // Close it with a click on empty toolbar space.
    let _ = n;
    nodes = click(&mut h, &mut ed, pos2(1100.0, 18.0));

    // Help ▸ Keyboard Shortcuts.
    let n = click(&mut h, &mut ed, find(&nodes, "Help"));
    click(&mut h, &mut ed, find(&n, "Keyboard Shortcuts…"));
    save(&h, "help_shortcuts.png");
    ed.close_shortcuts();

    // Add Component with a search, on the starter cube.
    let cube = by_name(&ed, "Cube").unwrap();
    ed.state.select(Some(cube));
    for _ in 0..3 {
        nodes = run(&mut h, &mut ed, Vec::new());
    }
    click(&mut h, &mut ed, find(&nodes, "Add Component"));
    save(&h, "add_component.png");
    run(&mut h, &mut ed, vec![Event::Text("li".into())]);
    for _ in 0..3 {
        run(&mut h, &mut ed, Vec::new());
    }
    save(&h, "add_component_search.png");
    let key = |pressed| Event::Key {
        key: Key::Escape,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: Modifiers::NONE,
    };
    run(&mut h, &mut ed, vec![key(true), key(false)]);

    // The Scene row with the environment in the inspector.
    ed.state.show_scene_settings();
    for _ in 0..3 {
        run(&mut h, &mut ed, Vec::new());
    }
    save(&h, "scene_row_environment.png");
    // Scrolled down: the RS3 look settings (sky, haze, mist, quieting,
    // vignette, wind), all reflected.
    let inspector = pos2(1180.0, 500.0);
    for _ in 0..12 {
        let scroll = Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, -400.0),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
            modifiers: Modifiers::NONE,
        };
        run(
            &mut h,
            &mut ed,
            vec![Event::PointerMoved(inspector), scroll],
        );
    }
    for _ in 0..3 {
        run(&mut h, &mut ed, Vec::new());
    }
    save(&h, "scene_settings_look.png");

    // An empty scene: hints in the hierarchy and the viewport.
    let roots: Vec<Uid> = ed
        .state
        .world
        .roots()
        .iter()
        .filter_map(|e| ed.state.world.uid(*e))
        .collect();
    ed.state.selection = roots;
    ed.state.delete_selection();
    assert!(ed.state.scene_is_empty());
    for _ in 0..3 {
        run(&mut h, &mut ed, Vec::new());
    }
    save(&h, "empty_scene.png");
    eprintln!("captures in {}", out.display());
    std::fs::remove_dir_all(&dir).unwrap();
}
