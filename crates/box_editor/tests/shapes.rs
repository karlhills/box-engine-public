//! The built-in generators in the editor (M5): Add ▸ Shapes ▸ Lathe and
//! Extrude (Primitive keeps its per-shape entries), placement by the
//! generated bounds (resting on the surface, beside existing meshes, one
//! undo step), point lists in the inspector, and the acceptance drag: a
//! real egui drag on a lathe's inspector field at 60 Hz with frame times.
//!
//! The last two tests need a GPU (skipped without one); captures go to
//! `target/captures/shapes/`. Frame times are printed with
//! `cargo test --release -p box_editor --test shapes -- --nocapture`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use box_core::math::Aabb;
use box_core::prelude::Entity;
use box_editor::headless::{CaptureOptions, FrameTiming, Headless};
use box_editor::log::LogBuffer;
use box_editor::{Editor, EditorOptions, NewEntity, build_editor};
use box_engine::box_proc::{Lathe, LatheShape, ProcClock, ProcSettings, ProcState};
use box_render::MeshRenderer;
use egui::accesskit::{Role, TreeUpdate};
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use glam::{Vec2, Vec3};

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn project_editor(name: &str) -> (Editor, TempDir) {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "box_editor_shapes_{name}_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    box_engine::Project::create(&dir, name).unwrap();
    let options = EditorOptions {
        project: Some(dir.clone()),
        ..Default::default()
    };
    let mut editor = build_editor(&options, LogBuffer::default()).unwrap();
    editor.tree = box_editor::layout::default_layout();
    (editor, TempDir(dir))
}

fn sync(editor: &mut Editor) {
    box_core::transform::propagate_transforms(&mut editor.state.world);
    box_engine::box_proc::settle(&mut editor.state.world);
    box_engine::sync_scene_meshes(&mut editor.state.world);
}

fn entity(editor: &Editor) -> Entity {
    editor
        .state
        .world
        .entity_by_uid(editor.state.selection[0])
        .unwrap()
}

/// World bounds of every mesh but `skip`'s.
fn other_bounds(editor: &Editor, skip: Entity) -> Vec<Aabb> {
    editor
        .state
        .world
        .query::<(Entity, &MeshRenderer)>()
        .iter()
        .filter(|(e, _)| *e != skip)
        .filter_map(|(e, _)| box_editor::entity_bounds(&editor.state.world, e))
        .filter(|b| b.half_extents().min_element() >= 1e-4)
        .collect()
}

fn overlaps(a: &Aabb, b: &Aabb) -> bool {
    let eps = Vec3::splat(1e-3);
    (a.min + eps).cmplt(b.max).all() && (a.max - eps).cmpgt(b.min).all()
}

#[test]
fn shapes_are_offered_and_placed_on_the_surface_beside_other_meshes() {
    let (mut editor, _dir) = project_editor("place");
    let entries = editor.state.generator_entries();
    let shapes: Vec<(&str, &str)> = entries
        .iter()
        .filter(|g| g.category == "Shapes")
        .map(|g| (g.label.as_str(), g.name))
        .collect();
    // Sorted by label; Primitive has its own entries (Cube, Sphere, ...).
    assert_eq!(
        shapes,
        [
            ("Extrude", "Extrude"),
            ("Lathe", "Lathe"),
            ("Spline", "Sweep")
        ]
    );
    let actions = box_editor::actions::context_actions(&mut editor.state);
    let ids: Vec<&str> = actions.iter().map(|a| a.id()).collect();
    assert!(ids.contains(&"add.generator:Lathe") && ids.contains(&"add.generator:Extrude"));
    assert!(!ids.contains(&"add.generator:Primitive"));

    // A cube first, then lathes and an extrusion at the same view: each
    // rests on the ground (y = 0 under the view centre) and overlaps nothing.
    editor.state.add_entity(NewEntity::Cube, None).unwrap();
    for name in ["Lathe", "Lathe", "Extrude"] {
        let undo = editor.state.edits.undo_len();
        editor.state.add_generator(name).unwrap();
        assert_eq!(editor.state.edits.undo_len(), undo + 1, "one undo step");
        sync(&mut editor);
        let e = entity(&editor);
        let b = box_editor::entity_bounds(&editor.state.world, e).expect("drawn");
        assert!(b.min.y.abs() < 1e-3, "{name} rests on the ground: {b:?}");
        for other in other_bounds(&editor, e) {
            assert!(!overlaps(&b, &other), "{name} {b:?} overlaps {other:?}");
        }
    }
}

#[test]
fn primitives_regenerate_through_the_runtime_in_the_editor() {
    let (mut editor, _dir) = project_editor("prim");
    editor.state.add_entity(NewEntity::Sphere, None).unwrap();
    sync(&mut editor);
    let e = entity(&editor);
    let s = editor.state.world.get::<ProcState>(e).expect("a generator");
    assert_eq!(s.generator(), "Primitive");
    assert!(s.warnings().is_empty());
    drop(s);
    // Bake to Mesh is offered for primitives (they are generators).
    let bake = editor.state.actions().get("edit.bake").cloned().unwrap();
    assert_eq!(bake.availability(&editor.state), Ok(()));
}

// ----- egui driving -----

#[derive(Clone, Debug)]
struct Node {
    role: Role,
    text: String,
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
            Some(Node {
                role: n.role(),
                text: n.label().or(n.value()).unwrap_or_default().to_string(),
                rect: Rect::from_min_max(
                    pos2(b.x0 as f32, b.y0 as f32),
                    pos2(b.x1 as f32, b.y1 as f32),
                ),
            })
        })
        .collect()
}

fn find(nodes: &[Node], text: &str) -> Rect {
    nodes
        .iter()
        .rev()
        .find(|n| n.text.trim() == text && n.role != Role::TextInput)
        .map(|n| n.rect)
        .unwrap_or_else(|| {
            let all: Vec<&str> = nodes.iter().map(|n| n.text.as_str()).collect();
            panic!("no `{text}` among {all:?}")
        })
}

/// The drag field on the inspector row labelled `label`.
fn field_of(nodes: &[Node], label: &str) -> Rect {
    let rows: Vec<&Node> = nodes.iter().filter(|n| n.text == label).collect();
    nodes
        .iter()
        .filter(|n| n.role == Role::SpinButton)
        .find(|n| {
            rows.iter()
                .any(|r| (n.rect.center().y - r.rect.center().y).abs() < 4.0)
        })
        .map(|n| n.rect)
        .unwrap_or_else(|| {
            let near: Vec<String> = nodes
                .iter()
                .filter(|n| {
                    rows.iter()
                        .any(|r| (n.rect.center().y - r.rect.center().y).abs() < 8.0)
                })
                .map(|n| format!("{:?} {:?} {:?}", n.role, n.text, n.rect))
                .collect();
            panic!("no drag field on the `{label}` row: {near:?}")
        })
}

fn press(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

fn headless(editor: &mut Editor) -> Option<Headless> {
    let options = CaptureOptions {
        size: [2560, 1600],
        pixels_per_point: 2.0,
        frames: 1,
    };
    match Headless::new(editor, &options) {
        Ok(h) => Some(h),
        Err(e)
            if matches!(
                e.downcast_ref::<box_render::RenderError>(),
                Some(box_render::RenderError::NoAdapter)
            ) =>
        {
            eprintln!("no GPU adapter; skipping");
            None
        }
        Err(e) => panic!("{e:#}"),
    }
}

fn out_dir() -> PathBuf {
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/shapes");
    std::fs::create_dir_all(&out).unwrap();
    out
}

/// The M5 acceptance drag, in the editor: a lathe's Height field dragged
/// with the mouse for one second at 60 Hz (frames paced to 16.7 ms, the
/// real-clock throttle as in a session). Every frame runs the full editor
/// (UI, generator flush, scene and UI render, GPU wait).
#[test]
fn dragging_a_lathe_parameter_updates_every_frame_budget() {
    let (mut editor, _dir) = project_editor("drag");
    let uid = editor.state.add_generator("Lathe").unwrap();
    editor.state.frame_view();
    let Some(mut h) = headless(&mut editor) else {
        return;
    };
    h.enable_accesskit();
    for _ in 0..10 {
        h.frame(&mut editor, Vec::new());
    }
    let e = editor.state.world.entity_by_uid(uid).unwrap();
    let nodes = nodes_of(h.accesskit_update());
    let field = field_of(&nodes, "Height");
    let generations = |ed: &Editor| ed.state.world.get::<ProcState>(e).unwrap().generations();
    let top = |ed: &Editor| box_editor::entity_bounds(&ed.state.world, e).unwrap().max.y;
    let before = generations(&editor);
    let undo = editor.state.edits.undo_len();
    let height_before = top(&editor);

    let from = field.center();
    h.frame(&mut editor, vec![Event::PointerMoved(from)]);
    // The regeneration throttle runs on simulated time, one 60 Hz step per
    // frame, so the regeneration count does not depend on how evenly a
    // loaded machine wakes the paced loop below. The base is past any real
    // time the throttle has seen.
    let step = Duration::from_micros(16_667);
    let base = Duration::from_secs(3600);
    let set_clock = |ed: &mut Editor, t: Duration| {
        ed.state.world.resource_or_default::<ProcSettings>().clock = ProcClock::Manual(t);
    };
    set_clock(&mut editor, base);
    h.frame(&mut editor, vec![press(from, true)]);
    let start = Instant::now();
    let mut samples: Vec<FrameTiming> = Vec::new();
    for k in 1..=60u32 {
        let due = start + step * k;
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
        set_clock(&mut editor, base + step * k);
        let p = from + vec2(2.0 * k as f32, 0.0);
        samples.push(h.frame(&mut editor, vec![Event::PointerMoved(p)]));
    }
    let during = generations(&editor) - before;
    let end = from + vec2(120.0, 0.0);
    h.frame(&mut editor, vec![press(end, false)]);
    // Past the throttle interval, so the last value lands.
    set_clock(&mut editor, base + step * 64);
    for _ in 0..3 {
        h.frame(&mut editor, Vec::new());
    }
    let dragged = match editor.state.world.get::<Lathe>(e).unwrap().shape {
        LatheShape::Vase { height, .. } => height,
        ref other => panic!("{other:?}"),
    };
    let mut totals: Vec<f32> = samples.iter().map(|s| s.total_ms).collect();
    totals.sort_by(f32::total_cmp);
    let mean = totals.iter().sum::<f32>() / totals.len() as f32;
    let p95 = totals[(totals.len() as f32 * 0.95) as usize];
    let worst = totals[totals.len() - 1];
    let over = totals.iter().filter(|t| **t > 16.7).count();
    eprintln!(
        "editor drag of Lathe height (2560x1600@2x, 60 frames at 60 Hz): {during} regenerations; frame total incl. GPU wait mean {mean:.2} ms, p95 {p95:.2} ms, worst {worst:.2} ms; {over} frame(s) over 16.7 ms; height {height_before:.3} -> {dragged:.3} m"
    );
    assert!(dragged > 1.05, "the drag changed the height: {dragged}");
    assert!(
        (top(&editor) - dragged).abs() < 1e-3,
        "the last value is drawn"
    );
    assert!((20..=40).contains(&during), "{during} regenerations");
    assert_eq!(
        editor.state.edits.undo_len(),
        undo + 1,
        "the drag is one undo step"
    );
    assert_eq!(h.renderer().stats().validation_errors, 0);
    assert!(mean < 16.7, "mean frame {mean:.2} ms");
    h.image()
        .unwrap()
        .save(out_dir().join("lathe_drag.png"))
        .unwrap();
}

/// Add ▸ Shapes, the lathe inspector with its preset, a custom profile as
/// editable points (+ adds one), and the shapes in the viewport.
#[test]
fn captures_shapes_menu_inspector_and_viewport() {
    let (mut editor, _dir) = project_editor("capture");
    let Some(mut h) = headless(&mut editor) else {
        return;
    };
    h.enable_accesskit();
    let run = |h: &mut Headless, ed: &mut Editor, events: Vec<Event>| {
        h.frame(ed, events);
        nodes_of(h.accesskit_update())
    };
    let settle = |h: &mut Headless, ed: &mut Editor| {
        let mut n = Vec::new();
        for _ in 0..8 {
            n = run(h, ed, Vec::new());
        }
        n
    };
    let click = |h: &mut Headless, ed: &mut Editor, at: Pos2| {
        run(h, ed, vec![Event::PointerMoved(at)]);
        run(h, ed, vec![press(at, true), press(at, false)]);
    };
    let out = out_dir();
    let mut nodes = settle(&mut h, &mut editor);

    // + Add ▸ Shapes: Extrude and Lathe.
    click(&mut h, &mut editor, find(&nodes, "+ Add").center());
    nodes = settle(&mut h, &mut editor);
    let shapes = find(&nodes, "Shapes").center();
    run(&mut h, &mut editor, vec![Event::PointerMoved(shapes)]);
    nodes = settle(&mut h, &mut editor);
    find(&nodes, "Extrude");
    let lathe = find(&nodes, "Lathe").center();
    h.image()
        .unwrap()
        .save(out.join("add_shapes_menu.png"))
        .unwrap();

    // Lathe: placed on the ground, its preset in the inspector.
    click(&mut h, &mut editor, lathe);
    nodes = settle(&mut h, &mut editor);
    let e = entity(&editor);
    assert!(editor.state.world.has::<Lathe>(e));
    for label in [
        "Lathe",
        "Shape",
        "Height",
        "Radius",
        "Segments",
        "Sweep angle",
    ] {
        find(&nodes, label);
    }
    // An extrusion and a few more lathe presets beside it, framed.
    editor.state.add_generator("Extrude").unwrap();
    for shape in [
        LatheShape::Column {
            height: 1.6,
            radius: 0.15,
        },
        LatheShape::Dome {
            radius: 0.5,
            rings: 12,
        },
    ] {
        let uid = editor.state.add_generator("Lathe").unwrap();
        let path = editor.state.world.field_path("Lathe.shape").unwrap();
        editor
            .state
            .edits
            .set_field(&mut editor.state.world, uid, &path, &shape)
            .unwrap();
    }
    // Framed on the first lathe, pulled back to show its neighbours.
    editor
        .state
        .select(Some(editor.state.world.uid(e).unwrap()));
    editor.state.frame_view();
    editor.state.camera.orbit.distance *= 4.0;
    settle(&mut h, &mut editor);
    h.image()
        .unwrap()
        .save(out.join("lathe_inspector.png"))
        .unwrap();

    // Custom profile: its points are a list (opened with its chevron);
    // + appends a copy of the last point, and a point row edits one item.
    let path = editor.state.world.field_path("Lathe.shape").unwrap();
    let uid = editor.state.world.uid(e).unwrap();
    let custom = LatheShape::Custom {
        profile: vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(0.4, 0.0),
            Vec2::new(0.25, 0.6),
        ],
        closed: false,
    };
    editor
        .state
        .edits
        .set_field(&mut editor.state.world, uid, &path, &custom)
        .unwrap();
    nodes = settle(&mut h, &mut editor);
    let header = find(&nodes, "3 items");
    click(&mut h, &mut editor, header.center());
    nodes = settle(&mut h, &mut editor);
    find(&nodes, "Profile 2");
    let plus = nodes
        .iter()
        .filter(|n| n.text == "+" && (n.rect.center().y - header.center().y).abs() < 4.0)
        .map(|n| n.rect.center())
        .next_back()
        .expect("+ on the profile row");
    click(&mut h, &mut editor, plus);
    nodes = settle(&mut h, &mut editor);
    find(&nodes, "4 items");
    find(&nodes, "Profile 3");
    let item = editor
        .state
        .world
        .field_path("Lathe.shape.profile[3]")
        .unwrap();
    editor
        .state
        .edits
        .set_field(&mut editor.state.world, uid, &item, &Vec2::new(0.1, 1.2))
        .unwrap();
    match &editor.state.world.get::<Lathe>(e).unwrap().shape {
        LatheShape::Custom { profile, .. } => assert_eq!(
            profile.as_slice(),
            [
                Vec2::new(0.0, 0.0),
                Vec2::new(0.4, 0.0),
                Vec2::new(0.25, 0.6),
                Vec2::new(0.1, 1.2)
            ]
        ),
        other => panic!("{other:?}"),
    }
    settle(&mut h, &mut editor);
    h.image()
        .unwrap()
        .save(out.join("lathe_custom_profile.png"))
        .unwrap();
    assert_eq!(h.renderer().stats().validation_errors, 0);
}
