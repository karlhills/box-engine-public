//! Splines in the editor (M5): Add ▸ Shapes ▸ Spline, the Edit Spline
//! viewport tool (select, drag with one undo step and live sweep updates,
//! Shift for height, Cmd-click insert, Delete, Enter and Escape), the
//! inspector's list of points, and the frame cost of dragging a point of a
//! 200-sample tube at 60 Hz.
//!
//! Headless and driven by egui events. The last test needs a GPU: captures
//! in `target/captures/splines/`.
//!
//! `cargo test --release -p box_editor --test splines -- --nocapture`
//! prints the drag timings.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use box_core::assets::Assets;
use box_core::identity::{Name, Uid};
use box_core::mesh::MeshData;
use box_core::prelude::{Entity, GlobalTransform, Transform};
use box_core::scene;
use box_editor::log::LogBuffer;
use box_editor::{Editor, EditorOptions, build_editor};
use box_engine::box_proc::spline::{Orientation, Path as SplinePath, Sampling};
use box_engine::box_proc::{
    ControlPoint, ProcClock, ProcSettings, ProcState, Spline, SplineMode, Sweep, SweepProfile,
    mesh_hash,
};
use box_engine::box_render::MeshRenderer;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, vec2};
use glam::Vec3;

// ----- an egui driver (as in tests/generated.rs) -----

struct Driver {
    ctx: egui::Context,
    time: f64,
    modifiers: Modifiers,
}

const SCREEN: egui::Vec2 = vec2(1280.0, 800.0);

impl Driver {
    fn new(editor: &Editor) -> Self {
        let ctx = egui::Context::default();
        editor.apply_theme(&ctx);
        Self {
            ctx,
            time: 0.0,
            modifiers: Modifiers::NONE,
        }
    }

    /// One 60 Hz frame. A manual regeneration clock (see [`add_spline`])
    /// advances by 1/60 s, so the 30 ms throttle sees real frame timing.
    fn frame(&mut self, editor: &mut Editor, mut events: Vec<Event>) {
        if let Some(s) = editor.state.world.get_resource_mut::<ProcSettings>()
            && let ProcClock::Manual(t) = &mut s.clock
        {
            *t += Duration::from_secs_f64(1.0 / 60.0);
        }
        events.insert(0, Event::ModifiersChanged(self.modifiers));
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
            time: Some(self.time),
            predicted_dt: 1.0 / 60.0,
            focused: true,
            events,
            ..Default::default()
        };
        let _ = self.ctx.run_ui(raw, |ui| editor.ui(ui));
        editor.prepare_render();
        self.time += 1.0 / 60.0;
    }

    fn frames(&mut self, editor: &mut Editor, n: usize) {
        for _ in 0..n {
            self.frame(editor, Vec::new());
        }
    }

    fn button(&self, pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: self.modifiers,
        }
    }

    fn click_at(&mut self, editor: &mut Editor, pos: Pos2) {
        self.frame(editor, vec![Event::PointerMoved(pos)]);
        let down = self.button(pos, true);
        self.frame(editor, vec![down]);
        let up = self.button(pos, false);
        self.frame(editor, vec![up]);
        self.frame(editor, Vec::new());
    }

    /// Press at `from`, move to `to` in `steps` frames (`during` runs after
    /// each move), release.
    fn drag(
        &mut self,
        editor: &mut Editor,
        from: Pos2,
        to: Pos2,
        steps: usize,
        mut during: impl FnMut(&mut Editor, usize),
    ) {
        self.frame(editor, vec![Event::PointerMoved(from)]);
        let down = self.button(from, true);
        self.frame(editor, vec![down]);
        for k in 1..=steps {
            let p = from + (to - from) * (k as f32 / steps as f32);
            self.frame(editor, vec![Event::PointerMoved(p)]);
            during(editor, k);
        }
        let up = self.button(to, false);
        self.frame(editor, vec![up]);
        self.frame(editor, Vec::new());
    }

    fn key(&mut self, editor: &mut Editor, key: Key) {
        for pressed in [true, false] {
            self.frame(
                editor,
                vec![Event::Key {
                    key,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: self.modifiers,
                }],
            );
        }
    }
}

fn command() -> Modifiers {
    Modifiers {
        alt: false,
        ctrl: !cfg!(target_os = "macos"),
        shift: false,
        mac_cmd: cfg!(target_os = "macos"),
        command: true,
    }
}

// ----- projects and helpers -----

fn temp_project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("box_spline_{name}_{}", Uid::new()));
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

fn entity(ed: &Editor, uid: Uid) -> Entity {
    ed.state.world.entity_by_uid(uid).unwrap()
}

fn spline(ed: &Editor, uid: Uid) -> Spline {
    Spline::clone(&ed.state.world.get::<Spline>(entity(ed, uid)).unwrap())
}

/// Screen position (points) of a world point in the viewport.
fn screen(ed: &Editor, p: Vec3) -> Pos2 {
    let s = ed.state.gizmo_view().unwrap().project(p).unwrap();
    ed.state.viewport.rect.unwrap().min + vec2(s.x, s.y)
}

fn world_of(ed: &Editor, uid: Uid, local: Vec3) -> Vec3 {
    let g = ed
        .state
        .world
        .get::<GlobalTransform>(entity(ed, uid))
        .unwrap()
        .0;
    g.transform_point3(local)
}

fn mesh_of(ed: &Editor, uid: Uid) -> Option<u64> {
    let e = entity(ed, uid);
    let r = ed.state.world.get::<MeshRenderer>(e)?;
    let meshes = ed.state.world.get_resource::<Assets<MeshData>>()?;
    meshes.get(r.mesh).map(mesh_hash)
}

fn generations(ed: &Editor, uid: Uid) -> u64 {
    ed.state
        .world
        .get::<ProcState>(entity(ed, uid))
        .map_or(0, |s| s.generations())
}

/// Add ▸ Shapes ▸ Spline, moved to `at` (as a separate step) and looked at.
/// The regeneration throttle gets a manual clock that the driver advances
/// 1/60 s per frame (test frames take far less wall time than that).
fn add_spline(ed: &mut Editor, d: &mut Driver, at: Vec3) -> Uid {
    // Far past the real-time epoch the runtime's last flush was stamped in.
    ed.state.world.insert_resource(ProcSettings {
        clock: ProcClock::Manual(Duration::from_secs(3600)),
        ..ProcSettings::default()
    });
    let uid = ed
        .state
        .add_generator("Sweep")
        .expect("Add ▸ Shapes ▸ Spline");
    assert!(
        ed.state
            .set_field(uid, "Transform", &Transform::from_translation(at))
    );
    let orbit = &mut ed.state.camera.orbit;
    orbit.target = at + Vec3::new(0.0, 0.2, 0.0);
    orbit.distance = 6.5;
    orbit.yaw = 0.0;
    orbit.pitch = 0.75;
    d.frames(ed, 3);
    uid
}

fn scene_text(ed: &Editor) -> String {
    scene::save_string(&ed.state.world).unwrap()
}

// ----- tests -----

#[test]
fn add_spline_is_a_tube_on_a_default_spline_in_one_undo_step() {
    let dir = temp_project("add");
    let mut ed = open(&dir);
    let mut d = Driver::new(&ed);
    d.frames(&mut ed, 3);
    let entry = ed
        .state
        .generator_entries()
        .into_iter()
        .find(|g| g.name == "Sweep")
        .expect("listed");
    assert_eq!((entry.category, entry.label.as_str()), ("Shapes", "Spline"));
    ed.state.open_palette();
    ed.state.set_palette_query("spline");
    let rows = ed.state.palette_results();
    assert!(
        rows.iter()
            .any(|r| r.id == "add.generator:Sweep" && r.label == "Shapes ▸ Spline"),
        "{rows:?}"
    );
    assert!(rows.iter().any(|r| r.id == "tool.edit_spline"), "{rows:?}");
    ed.state.close_palette();

    let undo = ed.state.edit_undo_len();
    let uid = ed.state.add_generator("Sweep").unwrap();
    assert_eq!(ed.state.edit_undo_len(), undo + 1);
    assert_eq!(ed.state.selection, vec![uid]);
    let e = entity(&ed, uid);
    let names = ed.state.world.component_names(e);
    for c in ["Spline", "Sweep", "Procedural", "StandardMaterial"] {
        assert!(names.contains(&c), "{c} missing: {names:?}");
    }
    assert_eq!(ed.state.world.get::<Name>(e).unwrap().0, "Spline");
    assert_eq!(spline(&ed, uid), Spline::default());
    d.frames(&mut ed, 2);
    assert!(mesh_of(&ed, uid).is_some(), "a visible tube");
    assert!(
        ed.state
            .world
            .get::<ProcState>(e)
            .unwrap()
            .warnings()
            .is_empty()
    );
    // The inspector shows the profile preset and the list of points.
    let sections = box_editor::inspect::entity_sections(&ed.state.world, e);
    let rows = |c: &str| {
        sections
            .iter()
            .find(|s| s.component == c)
            .unwrap()
            .rows
            .iter()
            .map(|r| (r.path.clone(), r.kind.clone()))
            .collect::<Vec<_>>()
    };
    let sweep_rows = rows("Sweep");
    assert!(sweep_rows.iter().any(|(p, k)| p == "Sweep.profile"
        && matches!(k, box_editor::inspect::FieldKind::Enum { current, .. } if current == "Circle")));
    let spline_rows = rows("Spline");
    assert!(spline_rows.iter().any(
        |(p, k)| p == "Spline.points" && *k == box_editor::inspect::FieldKind::List { len: 4 }
    ));
    assert!(spline_rows.iter().any(
        |(p, k)| p == "Spline.points[3].position" && *k == box_editor::inspect::FieldKind::Vec3
    ));
    // The inspector's + adds a point (a copy of the last): one undo step.
    let value = ed.state.world.get::<Spline>(e).unwrap().points.clone();
    let added =
        box_editor::inspect::list_with_item_added(ed.state.world.registry(), &value).unwrap();
    assert!(ed.state.set_field(uid, "Spline.points", added.as_ref()));
    assert_eq!(spline(&ed, uid).points.len(), 5);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_tool_drags_a_point_as_one_undo_step_and_the_sweep_follows_live() {
    let dir = temp_project("drag");
    let mut ed = open(&dir);
    let mut d = Driver::new(&ed);
    d.frames(&mut ed, 3);
    let uid = add_spline(&mut ed, &mut d, Vec3::new(0.0, 0.0, 2.5));
    // Listed after Place; needs a spline selected.
    let tools: Vec<&str> = ed.state.tools().iter().map(|t| t.id).collect();
    assert_eq!(&tools[..2], ["place_on_surface", "edit_spline"]);
    let action = ed.state.actions().get("tool.edit_spline").cloned().unwrap();
    assert_eq!(
        (action.category(), action.label()),
        ("Tools", "Edit Spline Tool")
    );
    assert!(ed.state.run_action("tool.edit_spline").is_ok());
    assert_eq!(ed.state.active_tool(), Some("edit_spline"));
    assert!(ed.state.tool_wants_input());
    let hint = ed.state.tool_hint().unwrap();
    assert!(hint.contains("4 points"), "{hint}");

    let before = scene_text(&ed);
    let mesh_before = mesh_of(&ed, uid).unwrap();
    let undo = ed.state.edit_undo_len();
    let p1 = spline(&ed, uid).points[1].position;
    let from = screen(&ed, world_of(&ed, uid, p1));
    let mut live = Vec::new();
    d.drag(&mut ed, from, from + vec2(60.0, 40.0), 6, |ed, _| {
        live.push(mesh_of(ed, uid));
    });
    assert_eq!(ed.state.edit_undo_len(), undo + 1, "one undo step");
    assert_eq!(
        ed.state.edits.undo_label(),
        Some("Move Spline Point 1"),
        "labelled"
    );
    let after = spline(&ed, uid).points[1].position;
    assert!(
        (after.y - p1.y).abs() < 1e-4,
        "on the horizontal plane: {p1} -> {after}"
    );
    assert!(after.distance(p1) > 0.3, "moved: {p1} -> {after}");
    assert!(
        live.iter().any(|h| h.is_some_and(|h| h != mesh_before)),
        "the tube changed during the drag"
    );
    let hint = ed.state.tool_hint().unwrap();
    assert!(hint.contains("point 1 of 4"), "{hint}");
    d.frames(&mut ed, 3);
    let mesh_after = mesh_of(&ed, uid).unwrap();
    assert_ne!(mesh_after, mesh_before);

    // Shift: up and down only.
    let p2 = spline(&ed, uid).points[2].position;
    let from = screen(&ed, world_of(&ed, uid, p2));
    d.modifiers = Modifiers::SHIFT;
    d.drag(&mut ed, from, from + vec2(25.0, -50.0), 5, |_, _| {});
    d.modifiers = Modifiers::NONE;
    let q2 = spline(&ed, uid).points[2].position;
    assert!(q2.y > p2.y + 0.2, "raised: {p2} -> {q2}");
    assert!(
        (q2.x - p2.x).abs() < 1e-4 && (q2.z - p2.z).abs() < 1e-4,
        "{p2} -> {q2}"
    );
    assert_eq!(ed.state.edit_undo_len(), undo + 2);

    // Undo twice: the scene is byte-identical and the tube as before.
    ed.state.undo();
    ed.state.undo();
    d.frames(&mut ed, 3);
    assert_eq!(scene_text(&ed), before);
    assert_eq!(mesh_of(&ed, uid), Some(mesh_before));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn cmd_click_inserts_on_the_curve_and_delete_removes_the_point() {
    let dir = temp_project("insert");
    let mut ed = open(&dir);
    let mut d = Driver::new(&ed);
    d.frames(&mut ed, 3);
    let uid = add_spline(&mut ed, &mut d, Vec3::new(0.0, 0.0, 2.5));
    assert!(ed.state.activate_tool("edit_spline"));
    d.frames(&mut ed, 1);
    let before = scene_text(&ed);
    let undo = ed.state.edit_undo_len();
    let s = spline(&ed, uid);
    let mid = s.curve().position(1.5);
    let at = screen(&ed, world_of(&ed, uid, mid));
    // A plain click on the curve inserts nothing.
    d.click_at(&mut ed, at);
    assert_eq!(spline(&ed, uid).points.len(), 4);
    d.modifiers = command();
    d.click_at(&mut ed, at);
    d.modifiers = Modifiers::NONE;
    let s2 = spline(&ed, uid);
    assert_eq!(s2.points.len(), 5);
    assert_eq!(ed.state.edit_undo_len(), undo + 1);
    assert!(
        s2.points[2].position.distance(mid) < 0.05,
        "{:?}",
        s2.points[2]
    );
    assert!(ed.state.tool_hint().unwrap().contains("point 2 of 5"));
    // Delete removes the selected point (not the entity): one step.
    d.key(&mut ed, Key::Delete);
    assert_eq!(spline(&ed, uid).points, s.points);
    assert!(ed.state.world.entity_by_uid(uid).is_some());
    assert_eq!(ed.state.edit_undo_len(), undo + 2);
    // Backspace too.
    d.key(&mut ed, Key::Backspace);
    assert_eq!(spline(&ed, uid).points.len(), 3);
    // Undo back to the start: byte-identical.
    ed.state.undo();
    ed.state.undo();
    ed.state.undo();
    d.frames(&mut ed, 2);
    assert_eq!(scene_text(&ed), before);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn bezier_insert_keeps_the_shape_and_handles_turn_tangents() {
    let dir = temp_project("bezier");
    let mut ed = open(&dir);
    let mut d = Driver::new(&ed);
    d.frames(&mut ed, 3);
    let uid = add_spline(&mut ed, &mut d, Vec3::new(0.0, 0.0, 2.5));
    assert!(ed.state.set_field(uid, "Spline.mode", &SplineMode::Bezier));
    assert!(ed.state.activate_tool("edit_spline"));
    d.frames(&mut ed, 2);
    let s = spline(&ed, uid);
    let curve = s.curve();
    let samples: Vec<Vec3> = (0..=60).map(|j| curve.position(j as f32 * 0.05)).collect();
    let at = screen(&ed, world_of(&ed, uid, curve.position(0.4)));
    d.modifiers = command();
    d.click_at(&mut ed, at);
    d.modifiers = Modifiers::NONE;
    let s2 = spline(&ed, uid);
    assert_eq!(s2.points.len(), 5);
    let c2 = s2.curve();
    for p in samples {
        let q = c2.closest_point(p);
        assert!(q.distance < 1e-3, "shape kept: {p} is {} away", q.distance);
    }
    // Drag the new point's out handle: both tangents turn (it is smooth
    // only if they were equal; a split point is not, so only the out one).
    let i = 1;
    let p = s2.points[i];
    let handle = p.position + p.out_tangent.unwrap() / 3.0;
    let from = screen(&ed, world_of(&ed, uid, handle));
    let undo = ed.state.edit_undo_len();
    d.drag(&mut ed, from, from + vec2(0.0, 30.0), 4, |_, _| {});
    assert_eq!(ed.state.edit_undo_len(), undo + 1);
    assert_eq!(ed.state.edits.undo_label(), Some("Turn Spline Tangent 1"));
    let q = spline(&ed, uid).points[i];
    assert_eq!(q.position, p.position);
    assert_ne!(q.out_tangent, p.out_tangent);
    assert_eq!(
        q.in_tangent, p.in_tangent,
        "a broken point keeps its other tangent"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn escape_cancels_a_drag_and_enter_finishes_the_tool() {
    let dir = temp_project("cancel");
    let mut ed = open(&dir);
    let mut d = Driver::new(&ed);
    d.frames(&mut ed, 3);
    let uid = add_spline(&mut ed, &mut d, Vec3::new(0.0, 0.0, 2.5));
    assert!(ed.state.activate_tool("edit_spline"));
    d.frames(&mut ed, 1);
    let before = spline(&ed, uid);
    let undo = ed.state.edit_undo_len();
    let from = screen(&ed, world_of(&ed, uid, before.points[0].position));
    d.frame(&mut ed, vec![Event::PointerMoved(from)]);
    let down = d.button(from, true);
    d.frame(&mut ed, vec![down]);
    for k in 1..=4 {
        d.frame(
            &mut ed,
            vec![Event::PointerMoved(from + vec2(10.0 * k as f32, 0.0))],
        );
    }
    assert_ne!(spline(&ed, uid), before, "moving");
    d.key(&mut ed, Key::Escape);
    assert_eq!(spline(&ed, uid), before, "rolled back");
    assert_eq!(ed.state.edit_undo_len(), undo, "no undo step");
    assert_eq!(ed.state.active_tool(), None);
    let up = d.button(from + vec2(40.0, 0.0), false);
    d.frame(&mut ed, vec![up]);

    assert!(ed.state.activate_tool("edit_spline"));
    d.key(&mut ed, Key::Enter);
    assert_eq!(ed.state.active_tool(), None, "Enter finishes");
    assert_eq!(ed.state.selection, vec![uid], "the selection stays");
    let _ = std::fs::remove_dir_all(dir);
}

/// Dragging a point of a 200-sample tube at 60 Hz: the editor frame (UI,
/// transforms, regeneration through the throttled flush, overlays) and the
/// sweep's generation time. The throttle uses a manual clock advanced 1/60 s
/// per frame, as a real 60 Hz drag would.
#[test]
fn drag_latency_of_a_200_sample_tube() {
    let dir = temp_project("latency");
    let mut ed = open(&dir);
    let mut d = Driver::new(&ed);
    d.frames(&mut ed, 3);
    let uid = add_spline(&mut ed, &mut d, Vec3::new(0.0, 0.0, 0.0));
    // A closed loop of 12 points, radius 2.5; the tolerance is lowered
    // until the swept path has at least 200 samples.
    let ring = Spline::from_positions(
        &(0..12)
            .map(|i| {
                let a = i as f32 / 12.0 * std::f32::consts::TAU;
                Vec3::new(a.cos() * 2.5, 0.4 + 0.3 * (a * 3.0).sin(), a.sin() * 2.5)
            })
            .collect::<Vec<_>>(),
        SplineMode::CatmullRom,
        true,
    );
    let mut tolerance = 0.01f32;
    let samples = loop {
        let n = SplinePath::from_spline(
            &ring,
            Sampling::Adaptive {
                tolerance,
                max_step: f32::MAX,
            },
            Orientation::RotationMinimizing(Vec3::Y),
        )
        .stations
        .len();
        if n >= 200 {
            break n;
        }
        tolerance *= 0.8;
    };
    assert!(ed.state.set_field(uid, "Spline", &ring));
    let sweep = Sweep {
        profile: SweepProfile::Circle {
            radius: 0.12,
            segments: 16,
        },
        tolerance,
        ..Sweep::default()
    };
    assert!(ed.state.set_field(uid, "Sweep", &sweep));
    ed.state.camera.orbit.distance = 9.0;
    d.frames(&mut ed, 3);
    assert!(ed.state.activate_tool("edit_spline"));
    d.frames(&mut ed, 2);
    let tris = {
        let e = entity(&ed, uid);
        let r = ed.state.world.get::<MeshRenderer>(e).unwrap();
        ed.state
            .world
            .resource::<Assets<MeshData>>()
            .get(r.mesh)
            .unwrap()
            .triangle_count()
    };

    let p = spline(&ed, uid).points[3].position;
    let from = screen(&ed, world_of(&ed, uid, p));
    let g0 = generations(&ed, uid);
    let frames = 120;
    let mut times: Vec<Duration> = Vec::with_capacity(frames);
    let mut gen_times: Vec<Duration> = Vec::new();
    d.frame(&mut ed, vec![Event::PointerMoved(from)]);
    let down = d.button(from, true);
    d.frame(&mut ed, vec![down]);
    for k in 1..=frames {
        // A circle of 80 points radius around the start: always moving.
        let a = k as f32 / frames as f32 * std::f32::consts::TAU;
        let pos = from + vec2(a.sin() * 80.0, (1.0 - a.cos()) * 60.0);
        let before = generations(&ed, uid);
        let start = Instant::now();
        d.frame(&mut ed, vec![Event::PointerMoved(pos)]);
        times.push(start.elapsed());
        if generations(&ed, uid) > before
            && let Some(t) = ed
                .state
                .world
                .get::<ProcState>(entity(&ed, uid))
                .unwrap()
                .last_generate()
        {
            gen_times.push(t);
        }
    }
    let up = d.button(from, false);
    d.frame(&mut ed, vec![up]);
    d.frames(&mut ed, 2);
    let regenerations = generations(&ed, uid) - g0;
    assert_eq!(ed.state.edits.undo_label(), Some("Move Spline Point 3"));

    let ms = |d: &Duration| d.as_secs_f64() * 1e3;
    let mut sorted: Vec<f64> = times.iter().map(ms).collect();
    sorted.sort_by(f64::total_cmp);
    let mean = sorted.iter().sum::<f64>() / sorted.len() as f64;
    let p95 = sorted[(sorted.len() * 95 / 100).min(sorted.len() - 1)];
    let max = *sorted.last().unwrap();
    let gen_mean = gen_times.iter().map(ms).sum::<f64>() / gen_times.len().max(1) as f64;
    let gen_max = gen_times.iter().map(ms).fold(0.0, f64::max);
    eprintln!(
        "spline drag, 200-sample tube ({samples} samples, {tris} triangles, 60 Hz, {frames} frames): \
         editor frame mean {mean:.2} ms, p95 {p95:.2} ms, max {max:.2} ms; \
         {regenerations} regenerations, generate mean {gen_mean:.3} ms, max {gen_max:.3} ms"
    );
    // 30 ms throttle at 60 Hz: about every other frame regenerates.
    assert!(
        (frames as u64 / 3..=frames as u64).contains(&regenerations),
        "{regenerations}"
    );
    // A dropped frame at 60 Hz is a frame over 16.7 ms.
    assert!(
        gen_max < 16.7,
        "generation alone must fit a frame: {gen_max} ms"
    );
    let _ = std::fs::remove_dir_all(dir);
}

// ----- GPU: captures -----

fn headless(ed: &mut Editor, size: [u32; 2]) -> Option<box_editor::headless::Headless> {
    use box_editor::headless::{CaptureOptions, Headless};
    let options = CaptureOptions {
        size,
        pixels_per_point: 2.0,
        frames: 1,
    };
    match Headless::new(ed, &options) {
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

/// Captures for review in `target/captures/splines/`: Add ▸ Spline, the
/// tool mid-drag, a Bézier spline with tangent handles, and a sweep along
/// another entity's spline after an edit.
#[test]
fn spline_captures() {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping captures");
        return;
    }
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/splines");
    std::fs::create_dir_all(&out).unwrap();
    let dir = temp_project("captures");
    let mut ed = open(&dir);
    let Some(mut h) = headless(&mut ed, [2560, 1600]) else {
        return;
    };
    let settle = |h: &mut box_editor::headless::Headless, ed: &mut Editor, n: usize| {
        for _ in 0..n {
            h.frame(ed, Vec::new());
        }
    };
    let save = |h: &box_editor::headless::Headless, name: &str| {
        let path = out.join(name);
        h.image().unwrap().save(&path).unwrap();
        eprintln!("{}", path.display());
    };
    settle(&mut h, &mut ed, 2);

    // Add ▸ Shapes ▸ Spline, as placed, selected.
    let uid = ed.state.add_generator("Sweep").unwrap();
    assert!(
        ed.state
            .set_field(uid, "Transform", &Transform::from_xyz(0.0, 0.0, 2.2))
    );
    let orbit = &mut ed.state.camera.orbit;
    orbit.target = Vec3::new(0.0, 0.3, 1.6);
    orbit.distance = 7.0;
    orbit.yaw = 0.35;
    orbit.pitch = 0.55;
    settle(&mut h, &mut ed, 4);
    save(&h, "add_spline.png");

    // The tool mid-drag: point 2 pulled sideways, the button still held.
    assert!(ed.state.activate_tool("edit_spline"));
    settle(&mut h, &mut ed, 2);
    let p = spline(&ed, uid).points[2].position;
    let from = screen(&ed, world_of(&ed, uid, p));
    h.frame(&mut ed, vec![Event::PointerMoved(from)]);
    h.frame(
        &mut ed,
        vec![Event::PointerButton {
            pos: from,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }],
    );
    for k in 1..=6 {
        h.frame(
            &mut ed,
            vec![Event::PointerMoved(
                from + vec2(14.0 * k as f32, 9.0 * k as f32),
            )],
        );
    }
    settle(&mut h, &mut ed, 2);
    save(&h, "edit_spline_mid_drag.png");
    let to = from + vec2(84.0, 54.0);
    h.frame(
        &mut ed,
        vec![Event::PointerButton {
            pos: to,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }],
    );
    settle(&mut h, &mut ed, 2);

    // A Bézier spline with tangent handles (a broken point in the middle).
    let bezier = Spline {
        points: vec![
            ControlPoint::new(Vec3::new(-2.0, 0.2, 0.6)).with_tangent(Vec3::new(3.0, 0.0, -3.0)),
            ControlPoint::new(Vec3::new(0.0, 0.6, -0.4))
                .with_tangents(Vec3::new(3.0, 0.0, 1.5), Vec3::new(2.5, 0.0, -2.0)),
            ControlPoint::new(Vec3::new(2.0, 0.2, 0.5)).with_tangent(Vec3::new(1.5, 0.0, 3.0)),
        ],
        mode: SplineMode::Bezier,
        closed: false,
    };
    assert!(ed.state.set_field(uid, "Spline", &bezier));
    settle(&mut h, &mut ed, 4);
    save(&h, "edit_spline_bezier_handles.png");
    ed.state.cancel_tool();
    assert!(ed.state.set_field(uid, "Spline", &Spline::default()));

    // A rectangular kerb on its own entity, sweeping the tube's spline
    // (both transforms count), after a point of that spline was moved.
    let kerb = Uid::new();
    {
        let s = &mut ed.state;
        let registry = s.world.registry().clone();
        let material = registry
            .component("StandardMaterial")
            .and_then(|c| c.default_value())
            .unwrap();
        let sweep = Sweep {
            spline: Some(uid),
            profile: SweepProfile::Rect {
                width: 0.6,
                height: 0.06,
            },
            orientation: box_engine::box_proc::SweepOrientation::Up,
            ..Sweep::default()
        };
        let transform = Transform::from_xyz(0.0, -0.15, 0.0);
        let components: Vec<(&str, &dyn box_core::reflect::PartialReflect)> = vec![
            ("Transform", &transform),
            ("Sweep", &sweep),
            ("StandardMaterial", material.as_ref()),
        ];
        s.edits
            .spawn(
                &mut s.world,
                kerb,
                "Kerb",
                box_core::world::Placement::last(None),
                &components,
            )
            .unwrap();
        s.changed(true);
    }
    let mut moved = spline(&ed, uid).points[1];
    moved.position += Vec3::new(0.0, 0.0, 0.8);
    assert!(ed.state.set_field(uid, "Spline.points[1]", &moved));
    ed.state.select(Some(kerb));
    settle(&mut h, &mut ed, 4);
    let warnings = ed
        .state
        .world
        .get::<ProcState>(entity(&ed, kerb))
        .unwrap()
        .warnings()
        .to_vec();
    assert!(warnings.is_empty(), "{warnings:?}");
    save(&h, "sweep_follows_spline_after_edit.png");
    let _ = std::fs::remove_dir_all(dir);
}
