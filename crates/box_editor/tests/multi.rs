//! M4b: marquee selection and multi-object editing (gizmo, inspector,
//! hierarchy), headless. The last test writes captures and needs a GPU.

use std::path::PathBuf;

use box_core::identity::Uid;
use box_core::prelude::{GlobalTransform, Primitive, Shape, Transform, World};
use box_core::scene;
use box_core::world::Placement;
use box_editor::gizmo::Handle;
use box_editor::inspect::{self, Mixed};
use box_editor::layout::Pane;
use box_editor::log::LogBuffer;
use box_editor::marquee::marquee_hits;
use box_editor::{Editor, EditorState, Pivot, SelectMode, Space, TransformTool};
use box_render::{DirectionalLight, StandardMaterial};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use glam::{Quat, Vec2, Vec3};
use proptest::prelude::*;

// ----- scene -----

/// Three cubes in a row (B has a child cube above it), a light and an
/// empty above the outer cubes, seen from the front and a little above.
struct Ids {
    a: Uid,
    b: Uid,
    child: Uid,
    c: Uid,
    lamp: Uid,
    empty: Uid,
}

fn mesh(world: &mut World, uid: Uid, name: &str, t: Transform, shape: Shape) {
    world
        .spawn_saved(
            uid,
            name,
            (t, Primitive::new(shape), StandardMaterial::default()),
        )
        .unwrap();
}

fn cube() -> Shape {
    Shape::Cuboid { size: Vec3::ONE }
}

fn settle(state: &mut EditorState) {
    box_core::transform::propagate_transforms(&mut state.world);
    box_engine::sync_scene_meshes(&mut state.world);
}

fn scene() -> (EditorState, Ids) {
    let mut world = World::with_registry(box_engine::shared_engine_registry());
    box_editor::scratch::prepare_world(&mut world);
    let id = Ids {
        a: Uid::from_u128(1),
        b: Uid::from_u128(2),
        child: Uid::from_u128(3),
        c: Uid::from_u128(4),
        lamp: Uid::from_u128(5),
        empty: Uid::from_u128(6),
    };
    mesh(
        &mut world,
        id.a,
        "A",
        Transform::from_xyz(-3.0, 0.0, 0.0),
        cube(),
    );
    mesh(
        &mut world,
        id.b,
        "B",
        Transform::from_xyz(0.0, 0.0, 0.0),
        cube(),
    );
    mesh(
        &mut world,
        id.c,
        "C",
        Transform::from_xyz(3.0, 0.0, 0.0),
        cube(),
    );
    world
        .spawn_saved(
            id.lamp,
            "Lamp",
            (
                Transform::from_xyz(-3.0, 2.5, 0.0),
                DirectionalLight::default(),
            ),
        )
        .unwrap();
    world
        .spawn_saved(id.empty, "Empty", (Transform::from_xyz(3.0, 2.5, 0.0),))
        .unwrap();
    mesh(
        &mut world,
        id.child,
        "Child",
        Transform::from_xyz(0.0, 2.5, 0.0),
        cube(),
    );
    let child = world.entity_by_uid(id.child).unwrap();
    world.place(child, Placement::last(Some(id.b))).unwrap();
    let mut state = EditorState::new(world, LogBuffer::default());
    state.viewport.rect = Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0)));
    look(&mut state, 0.3);
    settle(&mut state);
    (state, id)
}

fn look(state: &mut EditorState, pitch: f32) {
    state.camera.orbit.target = Vec3::new(0.0, 1.0, 0.0);
    state.camera.orbit.yaw = 0.0;
    state.camera.orbit.pitch = pitch;
    state.camera.orbit.distance = 14.0;
}

fn screen(state: &EditorState, p: Vec3) -> Vec2 {
    state.gizmo_view().unwrap().project(p).unwrap()
}

/// The rectangle around `points` grown by `pad` points.
fn around(points: &[Vec2], pad: f32) -> (Vec2, Vec2) {
    let lo = points.iter().fold(Vec2::splat(f32::MAX), |a, p| a.min(*p));
    let hi = points.iter().fold(Vec2::splat(f32::MIN), |a, p| a.max(*p));
    (lo - pad, hi + pad)
}

fn translation(state: &EditorState, uid: Uid) -> Vec3 {
    transform(state, uid).translation
}

fn transform(state: &EditorState, uid: Uid) -> Transform {
    let e = state.world.entity_by_uid(uid).unwrap();
    *state.world.get::<Transform>(e).unwrap()
}

fn world_pos(state: &EditorState, uid: Uid) -> Vec3 {
    let e = state.world.entity_by_uid(uid).unwrap();
    state
        .world
        .get::<GlobalTransform>(e)
        .unwrap()
        .0
        .translation
        .into()
}

fn save(state: &EditorState) -> String {
    String::from_utf8(scene::save(&state.world).unwrap()).unwrap()
}

// ----- marquee -----

#[test]
fn marquee_selects_by_bounds_and_origin_with_modifiers() {
    let (mut s, id) = scene();
    let p = |s: &EditorState, v: Vec3| screen(s, v);
    let (a, b, c) = (
        p(&s, Vec3::new(-3.0, 0.0, 0.0)),
        p(&s, Vec3::ZERO),
        p(&s, Vec3::X * 3.0),
    );
    let lamp = p(&s, Vec3::new(-3.0, 2.5, 0.0));
    let child = p(&s, Vec3::new(0.0, 2.5, 0.0));
    let empty = p(&s, Vec3::new(3.0, 2.5, 0.0));

    // A and B by their boxes; nothing above them.
    let (lo, hi) = around(&[a, b], 8.0);
    s.marquee_select(lo, hi, SelectMode::Replace);
    assert_eq!(s.selection, vec![id.a, id.b]);
    // Shift adds C (now the primary), Cmd toggles B and C off.
    let (lo, hi) = around(&[c], 8.0);
    s.marquee_select(lo, hi, SelectMode::Add);
    assert_eq!(s.selection, vec![id.a, id.b, id.c]);
    assert_eq!(s.primary(), Some(id.c));
    let (lo, hi) = around(&[b, c], 8.0);
    s.marquee_select(lo, hi, SelectMode::Toggle);
    assert_eq!(s.selection, vec![id.a]);
    // A mesh-less entity by its origin.
    let (lo, hi) = around(&[lamp], 8.0);
    s.marquee_select(lo, hi, SelectMode::Replace);
    assert_eq!(s.selection, vec![id.lamp]);
    // A child mesh and an empty, in hierarchy order.
    let (lo, hi) = around(&[empty, child], 8.0);
    s.marquee_select(lo, hi, SelectMode::Replace);
    assert_eq!(s.selection, vec![id.child, id.empty]);
    // Empty space: Replace clears, Shift keeps.
    s.marquee_select(Vec2::new(5.0, 5.0), Vec2::new(30.0, 30.0), SelectMode::Add);
    assert_eq!(s.selection, vec![id.child, id.empty]);
    s.marquee_select(
        Vec2::new(5.0, 5.0),
        Vec2::new(30.0, 30.0),
        SelectMode::Replace,
    );
    assert!(s.selection.is_empty());
    // The whole viewport: everything, in hierarchy order. Corners beyond
    // the viewport are clamped to it.
    s.marquee_select(
        Vec2::new(-50.0, -50.0),
        Vec2::new(900.0, 700.0),
        SelectMode::Replace,
    );
    assert_eq!(
        s.selection,
        vec![id.a, id.b, id.child, id.c, id.lamp, id.empty]
    );
    // The rectangle may be dragged in any direction.
    let (lo, hi) = around(&[a, b], 8.0);
    s.marquee_select(hi, lo, SelectMode::Replace);
    assert_eq!(s.selection, vec![id.a, id.b]);
}

#[test]
fn marquee_uses_the_box_outline_not_just_the_centre() {
    let (s, id) = scene();
    let view = s.gizmo_view().unwrap();
    // A thin rectangle over B's right face, away from its centre.
    let right = screen(&s, Vec3::new(0.45, 0.0, 0.5));
    let hits = marquee_hits(&s.world, &view, right - 2.0, right + 2.0);
    assert_eq!(hits, vec![id.b]);
    // Just past the outline of B and short of C.
    let gap = screen(&s, Vec3::new(1.5, 0.0, 0.0));
    let hits = marquee_hits(&s.world, &view, gap - 2.0, gap + 2.0);
    assert!(hits.is_empty(), "{hits:?}");
}

#[test]
fn flat_meshes_need_to_be_inside_the_marquee() {
    let (mut s, id) = scene();
    let floor = Uid::from_u128(50);
    let tile = Uid::from_u128(51);
    let plane = |size| Shape::Plane {
        size: Vec2::splat(size),
        cells: glam::UVec2::ONE,
    };
    mesh(
        &mut s.world,
        floor,
        "Floor",
        Transform::from_xyz(0.0, -0.5, 0.0),
        plane(40.0),
    );
    mesh(
        &mut s.world,
        tile,
        "Tile",
        Transform::from_xyz(0.0, -0.5, 3.0),
        plane(1.0),
    );
    settle(&mut s);
    // Over the floor around A and B: the cubes, not the floor.
    let (a, b) = (
        screen(&s, Vec3::new(-3.0, 0.0, 0.0)),
        screen(&s, Vec3::ZERO),
    );
    let (lo, hi) = around(&[a, b], 8.0);
    s.marquee_select(lo, hi, SelectMode::Replace);
    assert_eq!(s.selection, vec![id.a, id.b]);
    // Partly over the tile: no; around all of it: yes.
    let corners: Vec<Vec2> = [(-0.5, 2.5), (0.5, 2.5), (-0.5, 3.5), (0.5, 3.5)]
        .map(|(x, z)| screen(&s, Vec3::new(x, -0.5, z)))
        .to_vec();
    let (lo, hi) = around(&corners, -4.0);
    s.marquee_select(lo, hi, SelectMode::Replace);
    assert!(s.selection.is_empty(), "{:?}", s.selection);
    let (lo, hi) = around(&corners, 4.0);
    s.marquee_select(lo, hi, SelectMode::Replace);
    assert_eq!(s.selection, vec![tile]);
}

#[test]
fn occluded_entities_are_selected_too() {
    let (mut s, id) = scene();
    // A cube right behind B on the line of sight.
    let eye = s.camera.orbit.transform().translation;
    let behind = Vec3::ZERO + (Vec3::ZERO - eye).normalize() * 6.0;
    let hidden = Uid::from_u128(99);
    mesh(
        &mut s.world,
        hidden,
        "Hidden",
        Transform::from_translation(behind),
        cube(),
    );
    settle(&mut s);
    let b = screen(&s, Vec3::ZERO);
    s.marquee_select(b - 3.0, b + 3.0, SelectMode::Replace);
    assert_eq!(s.selection, vec![id.b, hidden]);
}

#[test]
fn a_box_around_the_camera_is_clipped_not_lost() {
    let (mut s, _) = scene();
    // A large box enclosing the camera: corners behind it are clipped, so
    // its outline covers the screen and any rectangle hits it.
    let big = Uid::from_u128(77);
    mesh(
        &mut s.world,
        big,
        "Room",
        Transform::from_xyz(0.0, 1.0, 0.0).with_scale(Vec3::splat(60.0)),
        cube(),
    );
    settle(&mut s);
    s.marquee_select(
        Vec2::new(5.0, 5.0),
        Vec2::new(30.0, 30.0),
        SelectMode::Replace,
    );
    assert_eq!(s.selection, vec![big]);
}

// ----- the viewport, driven with pointer events -----

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

    fn frame(&mut self, editor: &mut Editor, mut events: Vec<Event>, modifiers: Modifiers) {
        events.insert(0, Event::ModifiersChanged(modifiers));
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

    fn button(pos: Pos2, pressed: bool, modifiers: Modifiers) -> Event {
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers,
        }
    }

    /// Press at `from`, move to `to` in steps, then (if `release`) let go.
    fn drag(&mut self, ed: &mut Editor, from: Pos2, to: Pos2, m: Modifiers, release: bool) {
        self.frame(ed, vec![Event::PointerMoved(from)], m);
        self.frame(ed, vec![Self::button(from, true, m)], m);
        for k in 1..=6 {
            let p = from + (to - from) * (k as f32 / 6.0);
            self.frame(ed, vec![Event::PointerMoved(p)], m);
        }
        if release {
            self.frame(ed, vec![Self::button(to, false, m)], m);
            self.frame(ed, Vec::new(), m);
        }
    }

    fn click(&mut self, ed: &mut Editor, pos: Pos2, m: Modifiers) {
        self.frame(ed, vec![Event::PointerMoved(pos)], m);
        self.frame(ed, vec![Self::button(pos, true, m)], m);
        self.frame(ed, vec![Self::button(pos, false, m)], m);
        self.frame(ed, Vec::new(), m);
    }

    fn key(&mut self, ed: &mut Editor, key: Key) {
        let event = |pressed| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        self.frame(ed, vec![event(true)], Modifiers::NONE);
        self.frame(ed, vec![event(false)], Modifiers::NONE);
    }
}

fn editor_with_scene() -> (Editor, Driver, Ids) {
    let (state, id) = scene();
    let mut editor = Editor::new(state);
    editor.tree = box_editor::layout::default_layout();
    let mut d = Driver::new(&editor);
    for _ in 0..3 {
        d.frame(&mut editor, Vec::new(), Modifiers::NONE);
    }
    (editor, d, id)
}

/// A world point in screen points (viewport offset added).
fn on_screen(editor: &Editor, p: Vec3) -> Pos2 {
    let rect = editor.state.viewport.rect.unwrap();
    let v = screen(&editor.state, p);
    rect.min + vec2(v.x, v.y)
}

#[test]
fn dragging_in_the_viewport_draws_a_marquee_and_selects_live() {
    let (mut ed, mut d, id) = editor_with_scene();
    let a = on_screen(&ed, Vec3::new(-3.0, 0.0, 0.0));
    let b = on_screen(&ed, Vec3::ZERO);
    let c = on_screen(&ed, Vec3::X * 3.0);
    let from = a + vec2(-60.0, 40.0);
    let to = b + vec2(8.0, -8.0);

    d.drag(&mut ed, from, to, Modifiers::NONE, false);
    assert!(ed.state.viewport.marquee.is_some(), "marquee while held");
    assert_eq!(ed.state.selection, vec![id.a, id.b], "selected live");
    d.frame(
        &mut ed,
        vec![Driver::button(to, false, Modifiers::NONE)],
        Modifiers::NONE,
    );
    d.frame(&mut ed, Vec::new(), Modifiers::NONE);
    assert!(ed.state.viewport.marquee.is_none());
    assert_eq!(ed.state.selection, vec![id.a, id.b]);

    // Shift adds C, Cmd toggles B off.
    d.drag(
        &mut ed,
        c + vec2(-30.0, 30.0),
        c + vec2(30.0, -30.0),
        Modifiers::SHIFT,
        true,
    );
    assert_eq!(ed.state.selection, vec![id.a, id.b, id.c]);
    d.drag(
        &mut ed,
        b + vec2(-10.0, 10.0),
        b + vec2(10.0, -10.0),
        Modifiers::COMMAND,
        true,
    );
    assert_eq!(ed.state.selection, vec![id.a, id.c]);

    // Escape mid-drag restores the selection from before the drag.
    d.drag(
        &mut ed,
        b + vec2(-10.0, 10.0),
        b + vec2(10.0, -10.0),
        Modifiers::NONE,
        false,
    );
    assert_eq!(ed.state.selection, vec![id.b]);
    d.key(&mut ed, Key::Escape);
    assert!(ed.state.viewport.marquee.is_none());
    assert_eq!(ed.state.selection, vec![id.a, id.c]);
    d.frame(
        &mut ed,
        vec![Driver::button(b, false, Modifiers::NONE)],
        Modifiers::NONE,
    );
    d.frame(&mut ed, Vec::new(), Modifiers::NONE);
    assert_eq!(
        ed.state.selection,
        vec![id.a, id.c],
        "the release does not pick"
    );

    // A click without a drag still picks; Shift-click adds, Cmd-click
    // toggles, a click on empty space clears.
    d.click(&mut ed, b, Modifiers::NONE);
    assert_eq!(ed.state.selection, vec![id.b]);
    d.click(&mut ed, a, Modifiers::SHIFT);
    assert_eq!(ed.state.selection, vec![id.b, id.a]);
    d.click(&mut ed, b, Modifiers::COMMAND);
    assert_eq!(ed.state.selection, vec![id.a]);
    d.click(&mut ed, a + vec2(0.0, 150.0), Modifiers::NONE);
    assert!(ed.state.selection.is_empty());
}

#[test]
fn a_drag_that_grabs_a_gizmo_handle_does_not_start_a_marquee() {
    let (mut ed, mut d, id) = editor_with_scene();
    ed.state.selection = vec![id.a, id.b];
    d.frame(&mut ed, Vec::new(), Modifiers::NONE);
    let view = ed.state.gizmo_view().unwrap();
    let frame = ed.state.gizmo_frame(&view).unwrap();
    let rect = ed.state.viewport.rect.unwrap();
    let grab = view
        .project(frame.origin + frame.axes[0] * frame.size * 0.6)
        .unwrap();
    let grab = rect.min + vec2(grab.x, grab.y);
    d.drag(
        &mut ed,
        grab,
        grab + vec2(60.0, 0.0),
        Modifiers::NONE,
        false,
    );
    assert!(ed.state.viewport.marquee.is_none());
    assert!(ed.state.gizmo.drag.is_some());
    d.frame(
        &mut ed,
        vec![Driver::button(grab, false, Modifiers::NONE)],
        Modifiers::NONE,
    );
    assert_eq!(ed.state.selection, vec![id.a, id.b]);
    assert_eq!(ed.state.edits.undo_len(), 1, "one step for both");
}

// ----- gizmo on a multi-selection -----

/// Grabs the X arrow of the gizmo and drags it `steps` times by 8 points.
fn drag_x(state: &mut EditorState, steps: usize) {
    let view = state.gizmo_view().unwrap();
    let frame = state.gizmo_frame(&view).unwrap();
    let grab = view
        .project(frame.origin + frame.axes[0] * frame.size * 0.6)
        .unwrap();
    assert!(state.gizmo_begin(grab), "grabbed the X arrow");
    for k in 1..=steps {
        state.gizmo_update(grab + Vec2::new(8.0 * k as f32, 0.0));
        settle(state);
    }
    state.gizmo_end();
    settle(state);
}

#[test]
fn moving_a_multi_selection_is_one_step_and_children_move_once() {
    let (mut s, id) = scene();
    s.selection = vec![id.a, id.child, id.b];
    let before = save(&s);
    let child_world = world_pos(&s, id.child);
    drag_x(&mut s, 6);
    assert_eq!(s.edits.undo_len(), 1);
    assert_eq!(s.edits.undo_label(), Some("Move 2 entities"));
    let da = translation(&s, id.a) - Vec3::new(-3.0, 0.0, 0.0);
    let db = translation(&s, id.b);
    assert!(da.x > 0.5, "{da}");
    assert!((da - db).length() < 1e-4, "same move: {da} vs {db}");
    // The child is moved by its parent only: its local transform is as it
    // was and its world position moved once.
    assert_eq!(translation(&s, id.child), Vec3::new(0.0, 2.5, 0.0));
    assert!((world_pos(&s, id.child) - child_world - db).length() < 1e-4);
    // Unselected entities stay.
    assert_eq!(translation(&s, id.c), Vec3::new(3.0, 0.0, 0.0));

    let after = save(&s);
    s.undo();
    assert_eq!(save(&s), before, "undo restores every entity exactly");
    s.redo();
    assert_eq!(save(&s), after);
}

#[test]
fn rotating_a_multi_selection_turns_it_about_the_primary() {
    let (mut s, id) = scene();
    look(&mut s, 0.9);
    settle(&mut s);
    s.tool = TransformTool::Rotate;
    s.selection = vec![id.a, id.c, id.b];
    let view = s.gizmo_view().unwrap();
    let f = s.gizmo_frame(&view).unwrap();
    assert_eq!(f.origin, Vec3::ZERO, "the primary's origin");
    let ring = |t: f32| f.origin + (f.axes[2] * t.cos() + f.axes[0] * t.sin()) * f.size;
    let grab = view.project(ring(0.3)).unwrap();
    assert!(s.gizmo_begin(grab));
    assert_eq!(s.gizmo.hover, Some(Handle::Ring(1)));
    s.gizmo_update(view.project(ring(1.0)).unwrap());
    s.gizmo_end();
    settle(&mut s);
    assert_eq!(s.edits.undo_len(), 1);
    // B, at the pivot, keeps its position exactly; A and C swing around it.
    assert_eq!(translation(&s, id.b), Vec3::ZERO);
    let turned = transform(&s, id.b).rotation;
    assert!(turned.angle_between(Quat::IDENTITY) > 0.3);
    for (uid, start) in [
        (id.a, Vec3::new(-3.0, 0.0, 0.0)),
        (id.c, Vec3::new(3.0, 0.0, 0.0)),
    ] {
        let t = transform(&s, uid);
        assert!(
            (t.translation - turned * start).length() < 1e-3,
            "{}",
            t.translation
        );
        assert!(t.rotation.angle_between(turned) < 1e-3);
    }
}

#[test]
fn scaling_about_the_selection_centre_spreads_the_selection() {
    let (mut s, id) = scene();
    s.tool = TransformTool::Scale;
    s.pivot = Pivot::Centre;
    s.space = Space::Local;
    s.selection = vec![id.a, id.c];
    let view = s.gizmo_view().unwrap();
    let f = s.gizmo_frame(&view).unwrap();
    assert!(f.origin.length() < 1e-6, "centre of A and C: {}", f.origin);
    let centre = view.project(f.origin).unwrap();
    assert!(s.gizmo_begin(centre));
    assert_eq!(s.gizmo.hover, Some(Handle::ScaleUniform));
    s.gizmo_update(centre + Vec2::new(30.0, 0.0));
    s.gizmo_end();
    let k = (30.0f32 / 150.0).exp();
    for (uid, x) in [(id.a, -3.0), (id.c, 3.0)] {
        let t = transform(&s, uid);
        assert!((t.translation.x - x * k).abs() < 1e-4, "{}", t.translation);
        assert!((t.scale - Vec3::splat(k)).length() < 1e-4);
    }
    assert_eq!(s.edits.undo_len(), 1);
}

// ----- inspector -----

fn entities(s: &EditorState, uids: &[Uid]) -> Vec<box_core::prelude::Entity> {
    uids.iter()
        .map(|u| s.world.entity_by_uid(*u).unwrap())
        .collect()
}

fn row<'a>(sections: &'a [inspect::Section], path: &str) -> Option<&'a inspect::FieldRow> {
    sections
        .iter()
        .flat_map(|s| &s.rows)
        .find(|r| r.path == path)
}

#[test]
fn shared_sections_mark_mixed_values() {
    let (s, id) = scene();
    // C becomes a sphere: its shape variant differs from A and B.
    let c = s.world.entity_by_uid(id.c).unwrap();
    s.world.get_mut::<Primitive>(c).unwrap().shape = Shape::Sphere {
        radius: 0.5,
        sectors: 16,
        stacks: 8,
    };
    let sections = inspect::shared_sections(&s.world, &entities(&s, &[id.a, id.b, id.c]));
    let names: Vec<&str> = sections.iter().map(|s| s.component).collect();
    assert_eq!(names, ["Transform", "Primitive", "StandardMaterial"]);
    let mixed = |p: &str| row(&sections, p).map(|r| r.mixed);
    assert_eq!(mixed("Transform.translation"), Some(Mixed::Lanes(0b001)));
    assert_eq!(mixed("Transform.rotation"), Some(Mixed::Same));
    assert_eq!(mixed("Transform.scale"), Some(Mixed::Same));
    assert_eq!(mixed("Primitive.shape"), Some(Mixed::Whole));
    assert_eq!(
        mixed("Primitive.shape.size"),
        None,
        "fields of a mixed variant are hidden"
    );
    assert_eq!(mixed("Primitive.cast_shadows"), Some(Mixed::Same));
    // The same variant with different fields: the field is mixed.
    let sections = inspect::shared_sections(&s.world, &entities(&s, &[id.a, id.b]));
    assert_eq!(
        row(&sections, "Primitive.shape").map(|r| r.mixed),
        Some(Mixed::Same)
    );
    // A light and a cube share only the transform; Y differs.
    let sections = inspect::shared_sections(&s.world, &entities(&s, &[id.lamp, id.a]));
    let names: Vec<&str> = sections.iter().map(|s| s.component).collect();
    assert_eq!(names, ["Transform"]);
    assert_eq!(
        row(&sections, "Transform.translation").map(|r| r.mixed),
        Some(Mixed::Lanes(0b010))
    );
    // One entity: nothing is mixed.
    let one = inspect::shared_sections(&s.world, &entities(&s, &[id.a]));
    assert!(one.iter().flat_map(|s| &s.rows).all(|r| !r.mixed.any()));
}

#[test]
fn a_multi_edit_sets_every_entity_as_one_step() {
    let (mut s, id) = scene();
    let uids = [id.a, id.lamp, id.b];
    let before = save(&s);
    // A drag of X across three entities: one transaction, merged per entity.
    s.edits.begin("Set 3 entities Transform.translation");
    for x in [1.0f32, 2.0, 5.0] {
        let v = Vec3::new(x, 0.0, 0.0);
        assert!(s.set_field_on(
            &uids,
            "Transform.translation",
            &v,
            Some((0b001, &[x, 0.0, 0.0]))
        ));
    }
    s.finish_edit();
    assert_eq!(s.edits.undo_len(), 1);
    // X is set everywhere; the lamp keeps its own Y.
    assert_eq!(translation(&s, id.a), Vec3::new(5.0, 0.0, 0.0));
    assert_eq!(translation(&s, id.lamp), Vec3::new(5.0, 2.5, 0.0));
    assert_eq!(translation(&s, id.b), Vec3::new(5.0, 0.0, 0.0));
    // A rotation angle: the others change only that angle.
    s.edits.begin("Set rotation");
    let q = Quat::from_rotation_y(30f32.to_radians());
    assert!(s.set_field_on(
        &uids,
        "Transform.rotation",
        &q,
        Some((0b010, &[0.0, 30.0, 0.0]))
    ));
    s.finish_edit();
    for u in uids {
        let r = transform(&s, u).rotation;
        assert!(r.abs_diff_eq(q, 1e-6), "{r} vs {q}");
    }
    // A whole value (no lanes) on entities that share the component.
    s.edits.begin("Set roughness");
    assert!(s.set_field_on(&[id.a, id.b], "StandardMaterial.roughness", &0.25f32, None));
    s.finish_edit();
    for u in [id.a, id.b] {
        let e = s.world.entity_by_uid(u).unwrap();
        assert_eq!(s.world.get::<StandardMaterial>(e).unwrap().roughness, 0.25);
    }
    assert_eq!(s.edits.undo_len(), 3);
    for _ in 0..3 {
        s.undo();
    }
    assert_eq!(save(&s), before);
}

#[test]
fn add_and_remove_component_apply_to_the_whole_selection() {
    let (mut s, id) = scene();
    let before = save(&s);
    let uids = [id.a, id.lamp, id.empty];
    s.add_component_to(&uids, "DirectionalLight");
    assert_eq!(s.edits.undo_len(), 1, "one step, though the lamp had one");
    for u in uids {
        let e = s.world.entity_by_uid(u).unwrap();
        assert!(s.world.has::<DirectionalLight>(e));
    }
    let with_lights = save(&s);
    s.remove_component_from(&uids, "DirectionalLight");
    assert_eq!(s.edits.undo_len(), 2);
    for u in uids {
        let e = s.world.entity_by_uid(u).unwrap();
        assert!(!s.world.has::<DirectionalLight>(e));
    }
    s.undo();
    assert_eq!(save(&s), with_lights);
    s.undo();
    assert_eq!(save(&s), before);
}

// ----- hierarchy -----

#[test]
fn hierarchy_shift_click_selects_a_range_and_cmd_click_toggles() {
    let (mut ed, mut d, id) = editor_with_scene();
    let pane = ed
        .tree
        .tiles
        .rect(ed.tree.tiles.find_pane(&Pane::Hierarchy).unwrap())
        .unwrap();
    let row_h = box_editor::theme::DARK.row_h;
    // Rows: A, B, Child, C, Lamp, Empty (below the 34-point search bar
    // and the pinned Scene row).
    let row = |i: usize| {
        pos2(
            pane.center().x,
            pane.top() + 34.0 + (i as f32 + 1.5) * row_h,
        )
    };
    d.click(&mut ed, row(0), Modifiers::NONE);
    assert_eq!(ed.state.selection, vec![id.a]);
    d.click(&mut ed, row(3), Modifiers::SHIFT);
    assert_eq!(ed.state.selection, vec![id.a, id.b, id.child, id.c]);
    assert_eq!(ed.state.primary(), Some(id.c));
    d.click(&mut ed, row(1), Modifiers::COMMAND);
    assert_eq!(ed.state.selection, vec![id.a, id.child, id.c]);
    // Cmd + Shift adds the range from the anchor (B) to Empty.
    d.click(&mut ed, row(5), Modifiers::COMMAND | Modifiers::SHIFT);
    assert_eq!(
        ed.state.selection,
        vec![id.a, id.b, id.child, id.c, id.lamp, id.empty]
    );
    // A range going up ends on the clicked row, the new primary.
    d.click(&mut ed, row(4), Modifiers::NONE);
    d.click(&mut ed, row(2), Modifiers::SHIFT);
    assert_eq!(ed.state.selection, vec![id.lamp, id.c, id.child]);

    // Delete and duplicate act on the whole selection as one step each.
    let before = save(&ed.state);
    ed.state.duplicate_selection();
    assert_eq!(ed.state.selection.len(), 3);
    ed.state.delete_selection();
    assert_eq!(ed.state.edits.undo_len(), 2);
    ed.state.undo();
    ed.state.undo();
    assert_eq!(save(&ed.state), before);
}

// ----- property test -----

#[derive(Clone, Debug)]
enum Op {
    Select(Vec<usize>),
    Marquee {
        a: (f32, f32),
        b: (f32, f32),
        mode: u8,
    },
    Move(u8),
    Scale(i8, bool),
    SetLane {
        lane: u8,
        v: f32,
    },
    Roughness(f32),
    Light(bool),
    Duplicate,
    Delete,
}

fn op() -> impl Strategy<Value = Op> {
    let f = -4.0f32..4.0;
    prop_oneof![
        3 => prop::collection::vec(any::<usize>(), 0..4).prop_map(Op::Select),
        1 => ((0f32..800.0, 0f32..600.0), (0f32..800.0, 0f32..600.0), 0u8..3)
            .prop_map(|(a, b, mode)| Op::Marquee { a, b, mode }),
        3 => (1u8..6).prop_map(Op::Move),
        2 => (any::<i8>(), any::<bool>()).prop_map(|(d, c)| Op::Scale(d, c)),
        2 => (0u8..3, f.clone()).prop_map(|(lane, v)| Op::SetLane { lane, v }),
        1 => (0f32..1.0).prop_map(Op::Roughness),
        1 => any::<bool>().prop_map(Op::Light),
        1 => Just(Op::Duplicate),
        1 => Just(Op::Delete),
    ]
}

fn live(s: &EditorState) -> Vec<Uid> {
    let mut u: Vec<Uid> = s.world.query::<&Uid>().iter().copied().collect();
    u.sort();
    u
}

fn targets(s: &EditorState) -> Vec<Uid> {
    s.selection
        .iter()
        .copied()
        .filter(|u| s.world.entity_by_uid(*u).is_some())
        .collect()
}

fn run(s: &mut EditorState, op: &Op) {
    match op {
        Op::Select(picks) => {
            let all = live(s);
            if !all.is_empty() {
                s.selection = picks.iter().map(|i| all[i % all.len()]).collect();
                s.selection.dedup();
            }
        }
        Op::Marquee { a, b, mode } => {
            let mode = [SelectMode::Replace, SelectMode::Add, SelectMode::Toggle][*mode as usize];
            s.marquee_select(Vec2::new(a.0, a.1), Vec2::new(b.0, b.1), mode);
        }
        Op::Move(steps) => {
            s.tool = TransformTool::Translate;
            let Some(view) = s.gizmo_view() else { return };
            let Some(f) = s.gizmo_frame(&view) else {
                return;
            };
            let Some(grab) = view.project(f.origin + f.axes[0] * f.size * 0.6) else {
                return;
            };
            if s.gizmo_begin(grab) {
                for k in 1..=*steps {
                    s.gizmo_update(grab + Vec2::new(7.0 * k as f32, 3.0));
                    settle(s);
                }
                s.gizmo_end();
            }
        }
        Op::Scale(d, centre) => {
            s.tool = TransformTool::Scale;
            s.pivot = if *centre {
                Pivot::Centre
            } else {
                Pivot::Primary
            };
            let Some(view) = s.gizmo_view() else { return };
            let Some(f) = s.gizmo_frame(&view) else {
                return;
            };
            let Some(c) = view.project(f.origin) else {
                return;
            };
            if s.gizmo_begin(c) {
                s.gizmo_update(c + Vec2::new(*d as f32 * 0.3, 0.0));
                settle(s);
                s.gizmo_update(c + Vec2::new(*d as f32 * 0.5, 0.0));
                s.gizmo_end();
            }
        }
        Op::SetLane { lane, v } => {
            let uids = targets(s);
            if uids.is_empty() {
                return;
            }
            let mut lanes = [0.0f32; 3];
            s.edits.begin("lane");
            for step in 1..=3 {
                lanes[*lane as usize] = v * step as f32;
                let value = Vec3::from_array(lanes);
                s.set_field_on(
                    &uids,
                    "Transform.translation",
                    &value,
                    Some((1 << lane, &lanes)),
                );
            }
            s.finish_edit();
        }
        Op::Roughness(r) => {
            let uids = targets(s);
            let es: Vec<_> = uids
                .iter()
                .filter_map(|u| s.world.entity_by_uid(*u))
                .collect();
            let shared = inspect::shared_sections(&s.world, &es);
            if uids.is_empty() || !shared.iter().any(|x| x.component == "StandardMaterial") {
                return;
            }
            s.edits.begin("roughness");
            s.set_field_on(&uids, "StandardMaterial.roughness", r, None);
            s.set_field_on(&uids, "StandardMaterial.roughness", &(r * 0.5), None);
            s.finish_edit();
        }
        Op::Light(add) => {
            let uids = targets(s);
            if uids.is_empty() {
                return;
            }
            if *add {
                s.add_component_to(&uids, "DirectionalLight");
            } else {
                s.remove_component_from(&uids, "DirectionalLight");
            }
        }
        Op::Duplicate => s.duplicate_selection(),
        Op::Delete => s.delete_selection(),
    }
    settle(s);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

    /// Random multi-selections and multi-object edits, each at most one
    /// undo step, then as many undos: the scene serializes byte-identically.
    #[test]
    fn multi_edits_then_undos_restore_the_scene(ops in prop::collection::vec(op(), 1..24)) {
        let (mut s, _) = scene();
        let original = save(&s);
        for op in &ops {
            let before = s.edits.undo_len();
            run(&mut s, op);
            prop_assert!(s.edits.undo_len() <= before + 1, "{op:?} made more than one step");
            prop_assert!(!s.edits.is_open());
        }
        let edited = save(&s);
        let steps = s.edits.undo_len();
        for _ in 0..steps {
            s.undo();
        }
        prop_assert_eq!(s.edits.undo_len(), 0);
        prop_assert_eq!(save(&s), original);
        for _ in 0..steps {
            s.redo();
        }
        prop_assert_eq!(save(&s), edited);
    }
}

// ----- captures -----

fn skip_gpu() -> bool {
    std::env::var_os("BOX_SKIP_GPU_TESTS").is_some()
}

/// Headless captures of the marquee mid-drag, a multi-selection with mixed
/// inspector values, and the gizmo of a multi-selection about its centre,
/// in `target/captures/multi/`. Skipped with `BOX_SKIP_GPU_TESTS` or
/// without a GPU adapter.
#[test]
fn multi_selection_captures() {
    use box_editor::headless::{CaptureOptions, Headless};
    use box_editor::{EditorOptions, build_editor};
    if skip_gpu() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping captures");
        return;
    }
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/multi");
    std::fs::create_dir_all(&out).unwrap();
    let options = CaptureOptions {
        size: [1600, 1000],
        pixels_per_point: 1.0,
        frames: 4,
    };
    let mut editor = build_editor(&EditorOptions::default(), LogBuffer::default()).unwrap();
    editor.tree = box_editor::layout::default_layout();
    let mut h = match Headless::new(&mut editor, &options) {
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
    for _ in 0..3 {
        h.frame(&mut editor, Vec::new());
    }
    let rect = editor.state.viewport.rect.unwrap();
    let names = |ed: &Editor| -> Vec<String> {
        ed.state
            .selection
            .iter()
            .filter_map(|u| ed.state.world.entity_by_uid(*u))
            .map(|e| ed.state.display_name(e))
            .collect()
    };

    // (a) A marquee mid-drag across the middle of the scene.
    let from = rect.min + vec2(rect.width() * 0.30, rect.height() * 0.40);
    let to = rect.min + vec2(rect.width() * 0.62, rect.height() * 0.72);
    let press = |pos, pressed| Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    h.frame(&mut editor, vec![Event::PointerMoved(from)]);
    h.frame(&mut editor, vec![press(from, true)]);
    for k in 1..=8 {
        h.frame(
            &mut editor,
            vec![Event::PointerMoved(from + (to - from) * (k as f32 / 8.0))],
        );
    }
    h.frame(&mut editor, Vec::new());
    assert!(editor.state.viewport.marquee.is_some());
    let picked = names(&editor);
    assert!(picked.len() >= 2, "the marquee selects several: {picked:?}");
    h.image()
        .unwrap()
        .save(out.join("marquee_drag.png"))
        .unwrap();
    h.frame(&mut editor, vec![press(to, false)]);
    h.frame(&mut editor, Vec::new());
    eprintln!("marquee selected {picked:?}");

    // (b) Three entities with different positions: mixed inspector values.
    let by_name = |ed: &Editor, n: &str| {
        let w = &ed.state.world;
        w.query::<(&Uid, &box_core::identity::Name)>()
            .iter()
            .find(|(_, name)| name.0 == n)
            .map(|(u, _)| *u)
            .unwrap_or_else(|| panic!("no {n}"))
    };
    editor.state.selection = ["Shrub 1", "Glazed Ball", "Brass Ball"]
        .map(|n| by_name(&editor, n))
        .to_vec();
    // Frame all three.
    let bounds = editor
        .state
        .selection
        .iter()
        .filter_map(|u| editor.state.world.entity_by_uid(*u))
        .filter_map(|e| box_editor::entity_bounds(&editor.state.world, e))
        .reduce(|a, b| box_core::math::Aabb::new(a.min.min(b.min), a.max.max(b.max)))
        .unwrap();
    editor.state.camera.frame(&bounds);
    for _ in 0..3 {
        h.frame(&mut editor, Vec::new());
    }
    h.image()
        .unwrap()
        .save(out.join("multi_inspector_mixed.png"))
        .unwrap();

    // (c) The gizmo about the selection's centre, rotate tool.
    editor.state.pivot = Pivot::Centre;
    editor.state.tool = TransformTool::Rotate;
    for _ in 0..3 {
        h.frame(&mut editor, Vec::new());
    }
    h.image()
        .unwrap()
        .save(out.join("multi_gizmo_centre.png"))
        .unwrap();
    eprintln!("captures in {}", out.display());
}
