//! M2 acceptance tests for the editor, run headlessly. The GPU is only needed
//! for the frame-cost measurement, which is skipped with `BOX_SKIP_GPU_TESTS`
//! or without an adapter.

use std::sync::Arc;

use box_core::identity::Uid;
use box_core::prelude::{Primitive, Shape, Transform, World};
use box_core::reflect::{Reflect, ReflectComponent, ReflectDefault, Registry};
use box_core::scene;
use box_core::world::Placement;
use box_editor::headless::{CaptureOptions, Headless};
use box_editor::inspect::{self, FieldKind};
use box_editor::log::LogBuffer;
use box_editor::picking::{camera_ray, pick};
use box_editor::{Editor, EditorOptions, EditorState, NewEntity, TransformTool, build_editor};
use box_render::{RenderError, StandardMaterial};
use egui::{Rect, pos2, vec2};
use glam::{Vec2, Vec3};

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("box_editor_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
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

/// (b) A component type the editor has never heard of is inspected, added and
/// saved through reflection alone.
mod game {
    use super::*;

    /// Makes an entity bob up and down.
    #[derive(Reflect, Clone, Debug, PartialEq)]
    #[reflect(Component, Default)]
    #[type_path = "test_game"]
    pub struct Wobble {
        /// Oscillations per second.
        #[reflect(@box_core::reflect::Range { min: 0.0, max: 5.0 }, @box_core::reflect::Unit("Hz"))]
        pub frequency: f32,
        pub enabled: bool,
    }

    impl Default for Wobble {
        fn default() -> Self {
            Self {
                frequency: 1.5,
                enabled: true,
            }
        }
    }
}

#[test]
fn unknown_component_appears_in_inspector_and_saved_scene() {
    let mut registry: Registry = box_engine::engine_registry();
    registry.register::<game::Wobble>().unwrap();
    let mut world = World::with_registry(Arc::new(registry));
    let a = Uid::from_u128(1);
    let b = Uid::from_u128(2);
    world
        .spawn_saved(a, "Bobber", (Transform::default(), game::Wobble::default()))
        .unwrap();
    world
        .spawn_saved(b, "Plain", (Transform::default(),))
        .unwrap();
    let mut state = state_with(world);

    let e = state.world.entity_by_uid(a).unwrap();
    let sections = inspect::entity_sections(&state.world, e);
    let wobble = sections
        .iter()
        .find(|s| s.component == "Wobble")
        .expect("Wobble section");
    assert_eq!(wobble.doc, Some("Makes an entity bob up and down."));
    let freq = &wobble.rows[0];
    assert_eq!(
        (freq.path.as_str(), &freq.kind),
        ("Wobble.frequency", &FieldKind::F32)
    );
    let meta = freq.meta.as_ref().unwrap();
    assert_eq!(meta.unit, Some("Hz"));
    assert_eq!(meta.doc, Some("Oscillations per second."));
    assert_eq!(wobble.rows[1].kind, FieldKind::Bool);

    // Offered in Add Component on the other entity; adding it is undoable.
    let other = state.world.entity_by_uid(b).unwrap();
    assert!(inspect::addable_components(&state.world, other).contains(&"Wobble"));
    state.add_component(b, "Wobble");
    state.set_field(b, "Wobble.frequency", &3.0f32);
    let text = String::from_utf8(scene::save(&state.world).unwrap()).unwrap();
    assert_eq!(text.matches("\"Wobble\"").count(), 2, "{text}");
    assert!(text.contains("\"frequency\": 3.0"), "{text}");
    state.undo();
    state.undo();
    assert!(!state.world.has::<game::Wobble>(other));
}

/// (c) Save, quit and reopen a project: the scene bytes and the layout are restored.
#[test]
fn project_save_and_reopen_restores_scene_and_layout() {
    let dir = temp_dir("project_roundtrip");
    box_engine::Project::create(&dir, "Roundtrip").unwrap();
    let options = EditorOptions {
        project: Some(dir.clone()),
        ..Default::default()
    };
    let mut editor = build_editor(&options, LogBuffer::default()).unwrap();
    assert_eq!(editor.state.scene.name, "main");
    assert!(!editor.state.is_dirty());

    let cube = editor.state.add_entity(NewEntity::Sphere, None).unwrap();
    editor
        .state
        .set_field(cube, "Transform.translation", &Vec3::new(1.0, 2.0, 3.0));
    assert!(editor.state.is_dirty());
    assert!(editor.title().ends_with('•'));
    // A layout change: shrink the root split's first share.
    let root = editor.tree.root().unwrap();
    if let Some(egui_tiles::Tile::Container(egui_tiles::Container::Linear(l))) =
        editor.tree.tiles.get_mut(root)
    {
        let first = l.children[0];
        l.shares.set_share(first, 0.5);
    }
    let path = editor.state.save_scene().unwrap();
    assert!(!editor.state.is_dirty());
    editor.on_quit();
    let saved = std::fs::read(&path).unwrap();
    let layout = serde_json::to_value(&editor.tree).unwrap();
    drop(editor);

    let reopened = build_editor(&options, LogBuffer::default()).unwrap();
    assert_eq!(
        scene::save(&reopened.state.world).unwrap(),
        saved,
        "scene bytes"
    );
    assert_eq!(
        serde_json::to_value(&reopened.tree).unwrap(),
        layout,
        "layout"
    );
    assert!(reopened.state.world.entity_by_uid(cube).is_some());
    assert!(
        !reopened.state.edits.can_undo(),
        "undo history starts empty"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Unsaved changes are kept in `.box/recovery` on quit, never lost.
#[test]
fn quit_with_unsaved_changes_writes_recovery() {
    let dir = temp_dir("recovery");
    box_engine::Project::create(&dir, "Recovery").unwrap();
    let options = EditorOptions {
        project: Some(dir.clone()),
        ..Default::default()
    };
    let mut editor: Editor = build_editor(&options, LogBuffer::default()).unwrap();
    editor.state.add_entity(NewEntity::Cube, None).unwrap();
    editor.on_quit();
    let recovery = dir.join(".box/recovery/project/assets/scenes/main.scene.json");
    let bytes = std::fs::read(&recovery).expect("recovery file");
    assert_eq!(bytes, scene::save(&editor.state.world).unwrap());
    assert!(dir.join(".box/layout.json").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

fn picking_world() -> (EditorState, Uid, Uid) {
    let mut world = World::with_registry(box_engine::shared_engine_registry());
    let near = Uid::from_u128(10);
    let far = Uid::from_u128(11);
    let cube = || Primitive::new(Shape::Cuboid { size: Vec3::ONE });
    world
        .spawn_saved(
            near,
            "Near",
            (
                Transform::from_xyz(0.0, 0.0, 2.0),
                cube(),
                StandardMaterial::default(),
            ),
        )
        .unwrap();
    world
        .spawn_saved(
            far,
            "Far",
            (
                Transform::from_xyz(0.0, 0.0, -2.0),
                cube(),
                StandardMaterial::default(),
            ),
        )
        .unwrap();
    let mut state = state_with(world);
    state.camera.orbit.target = Vec3::ZERO;
    state.camera.orbit.yaw = 0.0;
    state.camera.orbit.pitch = 0.0;
    state.camera.orbit.distance = 10.0;
    settle(&mut state);
    (state, near, far)
}

/// (e) Picking hits the nearest mesh under the cursor, maps it to its saved
/// entity, and misses empty space.
#[test]
fn picking_hits_the_nearest_entity() {
    let (state, near, far) = picking_world();
    let hit = pick(
        &state.world,
        &camera_ray(&state.camera, Vec2::ZERO, 4.0 / 3.0),
    )
    .unwrap();
    assert_eq!(state.world.uid(hit.entity), Some(near));
    assert!((hit.distance - 7.5).abs() < 1e-3, "{}", hit.distance);
    // Seen from behind, the other cube is nearest.
    let mut back = state.camera;
    back.orbit.yaw = std::f32::consts::PI;
    let hit = pick(&state.world, &camera_ray(&back, Vec2::ZERO, 4.0 / 3.0)).unwrap();
    assert_eq!(state.world.uid(hit.entity), Some(far));
    assert!(
        pick(
            &state.world,
            &camera_ray(&state.camera, Vec2::new(0.9, 0.9), 4.0 / 3.0)
        )
        .is_none()
    );
}

/// (d) A gizmo drag is exactly one undo step, and undo restores a
/// byte-identical scene.
#[test]
fn gizmo_drag_is_one_undo_step() {
    let (mut state, near, _) = picking_world();
    // Look from an angle so all axes have screen length.
    state.camera.orbit.yaw = 0.6;
    state.camera.orbit.pitch = 0.4;
    state.select(Some(near));
    state.tool = TransformTool::Translate;
    let before = scene::save(&state.world).unwrap();
    let steps = state.edits.undo_len();

    let view = state.gizmo_view().unwrap();
    let frame = state.gizmo_frame(&view).unwrap();
    let grab = view
        .project(frame.origin + frame.axes[0] * frame.size * 0.6)
        .unwrap();
    assert!(state.gizmo_begin(grab), "grabbed the X arrow");
    for k in 1..=10 {
        state.gizmo_update(grab + Vec2::new(8.0 * k as f32, 0.0));
        settle(&mut state);
    }
    state.gizmo_end();

    assert_eq!(state.edits.undo_len(), steps + 1, "one step");
    assert_eq!(state.edits.undo_label(), Some("Move Near"));
    let e = state.world.entity_by_uid(near).unwrap();
    let moved = *state.world.get::<Transform>(e).unwrap();
    assert!(moved.translation.x > 0.1, "{}", moved.translation);
    assert!((moved.translation.y).abs() < 1e-4 && (moved.translation.z - 2.0).abs() < 1e-4);

    state.undo();
    assert_eq!(
        scene::save(&state.world).unwrap(),
        before,
        "byte-identical after undo"
    );
    assert_eq!(state.primary(), Some(near), "selection survives undo");
    state.redo();
    assert_eq!(*state.world.get::<Transform>(e).unwrap(), moved);
}

/// Hierarchy edits: reparent with index, cycles rejected without a step,
/// duplicate and delete are single steps, selection follows uids.
#[test]
fn hierarchy_edits_are_undoable_and_reject_cycles() {
    let mut world = World::with_registry(box_engine::shared_engine_registry());
    let id = Uid::from_u128;
    for (n, name) in [(1, "A"), (2, "B"), (3, "C")] {
        world
            .spawn_saved(id(n), name, (Transform::default(),))
            .unwrap();
    }
    let mut state = state_with(world);
    assert!(state.reparent(id(2), Some(id(1)), None));
    let steps = state.edits.undo_len();
    assert!(!state.reparent(id(1), Some(id(2)), None), "cycle rejected");
    assert_eq!(state.edits.undo_len(), steps);
    // Reorder roots: C before A.
    assert!(state.reparent(id(3), None, Some(0)));
    let roots: Vec<Uid> = state
        .world
        .roots()
        .iter()
        .filter_map(|e| state.world.uid(*e))
        .collect();
    assert_eq!(roots, [id(3), id(1)]);
    assert_eq!(
        state
            .world
            .placement(state.world.entity_by_uid(id(2)).unwrap()),
        Placement {
            parent: Some(id(1)),
            index: 0
        }
    );

    state.select(Some(id(1)));
    state.duplicate_selection();
    let copy = state.primary().unwrap();
    assert_ne!(copy, id(1));
    assert_eq!(state.world.entity_count(), 5, "A and its child B copied");
    state.delete_selection();
    assert_eq!(state.world.entity_count(), 3);
    state.undo();
    assert_eq!(state.world.entity_count(), 5);
    state.undo();
    assert_eq!(state.world.entity_count(), 3);
    assert_eq!(
        state.primary(),
        Some(id(1)),
        "the copy is gone; undo restores the selection from before the duplicate"
    );
}

/// (a) Frame cost with 1,000+ entities, all expanded in the hierarchy, one
/// selected (full inspector) and the translate gizmo shown.
#[test]
fn thousand_entity_frame_cost() {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        return;
    }
    let mut editor = build_editor(&EditorOptions::default(), LogBuffer::default()).unwrap();
    let w = &mut editor.state.world;
    for g in 0..100u128 {
        let group = Uid::from_u128(0x1000 + g);
        let e = w
            .spawn_saved(
                group,
                &format!("Block {g}"),
                (Transform::from_xyz(
                    (g % 10) as f32 * 6.0 - 30.0,
                    0.0,
                    (g / 10) as f32 * 6.0 - 60.0,
                ),),
            )
            .unwrap();
        let _ = e;
        for k in 0..9u128 {
            let uid = Uid::from_u128(0x10_0000 + g * 16 + k);
            let c = w
                .spawn_saved(
                    uid,
                    &format!("Crate {g}.{k}"),
                    (
                        Transform::from_xyz((k % 3) as f32 * 1.5, 0.5 + (k / 3) as f32 * 1.2, 0.0),
                        Primitive::new(Shape::Cuboid { size: Vec3::ONE }),
                        StandardMaterial::default(),
                    ),
                )
                .unwrap();
            w.place(c, Placement::last(Some(group))).unwrap();
        }
    }
    let count = editor.state.world.entity_count();
    assert!(count >= 1000, "{count}");
    editor
        .state
        .select(Some(Uid::from_u128(0x10_0000 + 55 * 16 + 4)));
    editor.state.frame_view();
    let options = CaptureOptions::default();
    let mut h = match Headless::new(&mut editor, &options) {
        Ok(h) => h,
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
    for _ in 0..10 {
        h.frame(&mut editor, Vec::new());
    }
    let mut samples = Vec::new();
    for i in 0..120 {
        // Scroll the hierarchy every frame so rows are re-laid out.
        let scroll = egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, if i % 2 == 0 { -400.0 } else { 300.0 }),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
            modifiers: Default::default(),
        };
        let pointer = egui::Event::PointerMoved(pos2(100.0, 400.0));
        samples.push(h.frame(&mut editor, vec![pointer, scroll]));
    }
    h.image().unwrap();
    let stats = |f: fn(&box_editor::headless::FrameTiming) -> f32| {
        let mut v: Vec<f32> = samples.iter().map(f).collect();
        v.sort_by(f32::total_cmp);
        let mean = v.iter().sum::<f32>() / v.len() as f32;
        (mean, v[(v.len() as f32 * 0.95) as usize])
    };
    let (ui, ui95) = stats(|t| t.ui_ms);
    let (render, render95) = stats(|t| t.render_ms);
    let (total, total95) = stats(|t| t.total_ms);
    eprintln!(
        "{count} entities, 2560x1440@2x: ui {ui:.2} ms (p95 {ui95:.2}), render cpu {render:.2} ms (p95 {render95:.2}), total incl. GPU wait {total:.2} ms (p95 {total95:.2})"
    );
    assert!(ui < 16.0, "UI pass {ui:.2} ms mean");
}
