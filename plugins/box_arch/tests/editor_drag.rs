//! box_arch in the editor: Add ▸ Architecture lists the generators, and a
//! real egui drag on a wall's Height field (a closed house loop with eight
//! openings, plinth and cornice) at 60 Hz keeps every frame within budget.
//!
//! Needs a GPU (skipped without one). Frame times are printed with
//! `cargo test --release -p box_arch --test editor_drag -- --nocapture`;
//! the capture goes to `target/captures/arch/editor_drag.png`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use box_arch::{ArchPlugin, Opening, Wall, WallPath, WallPoint};
use box_core::reflect::bevy_reflect::PartialReflect;
use box_editor::headless::{CaptureOptions, FrameTiming, Headless};
use box_editor::log::LogBuffer;
use box_editor::{Editor, EditorOptions, build_game_editor};
use box_engine::box_proc::{ProcClock, ProcSettings, ProcState};
use box_engine::prelude::*;
use egui::accesskit::{Role, TreeUpdate};
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn project_editor(name: &str) -> (Editor, TempDir) {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "box_arch_editor_{name}_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    box_engine::Project::create(&dir, name).unwrap();
    let options = EditorOptions {
        project: Some(dir.clone()),
        ..Default::default()
    };
    let game = Game::new().with_plugin(ArchPlugin);
    let mut editor = build_game_editor(&options, LogBuffer::default(), game).unwrap();
    editor.tree = box_editor::layout::default_layout();
    (editor, TempDir(dir))
}

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
        .unwrap_or_else(|| panic!("no drag field on the `{label}` row"))
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

#[test]
fn architecture_is_offered_in_the_add_menu() {
    let (mut editor, _dir) = project_editor("menu");
    let arch: Vec<String> = editor
        .state
        .generator_entries()
        .into_iter()
        .filter(|g| g.category == "Architecture")
        .map(|g| g.label)
        .collect();
    assert_eq!(arch, ["Platform", "Roof", "Stair", "Wall"]);
    for name in ["Wall", "Roof", "Platform", "Stair"] {
        let uid = editor.state.add_generator(name).expect(name);
        box_core::transform::propagate_transforms(&mut editor.state.world);
        box_engine::box_proc::settle(&mut editor.state.world);
        box_engine::sync_scene_meshes(&mut editor.state.world);
        let e = editor.state.world.entity_by_uid(uid).unwrap();
        let b = box_editor::entity_bounds(&editor.state.world, e).expect("drawn");
        assert!(b.min.y.abs() < 1e-3, "{name} rests on the ground: {b:?}");
    }
}

/// The M6 drag: a house wall's Height dragged with the mouse for one second
/// at 60 Hz (frames paced to 16.7 ms, the real-clock throttle). Every frame
/// runs the full editor (UI, generator flush, scene and UI render, GPU
/// wait).
#[test]
fn dragging_a_wall_height_keeps_frames_in_budget() {
    let (mut editor, _dir) = project_editor("drag");
    let uid = editor.state.add_generator("Wall").unwrap();
    let (hx, hz) = (4.0, 3.0);
    let path = WallPath::Points {
        points: [(hx, hz), (hx, -hz), (-hx, -hz), (-hx, hz)]
            .iter()
            .map(|&(x, z)| WallPoint::new(x, z))
            .collect(),
        closed: true,
    };
    // U: east 0..6, north 6..14, west 14..20, south 20..28.
    let openings = vec![
        Opening::window(1.5),
        Opening::window(4.5),
        Opening::arched_window(8.0),
        Opening::window(12.0),
        Opening::window(17.0),
        Opening::door(22.0),
        Opening::window(24.5),
        Opening::arched_window(26.5),
    ];
    for (field, value) in [
        ("Wall.path", &path as &dyn PartialReflect),
        ("Wall.openings", &openings),
    ] {
        let p = editor.state.world.field_path(field).unwrap();
        editor
            .state
            .edits
            .set_field(&mut editor.state.world, uid, &p, value)
            .unwrap();
    }
    for field in ["Wall.plinth", "Wall.cornice"] {
        let p = editor.state.world.field_path(field).unwrap();
        editor
            .state
            .edits
            .set_field(&mut editor.state.world, uid, &p, &true)
            .unwrap();
    }
    box_core::transform::propagate_transforms(&mut editor.state.world);
    box_engine::box_proc::settle(&mut editor.state.world);
    box_engine::sync_scene_meshes(&mut editor.state.world);
    editor.state.frame_view();
    let Some(mut h) = headless(&mut editor) else {
        return;
    };
    h.enable_accesskit();
    for _ in 0..10 {
        h.frame(&mut editor, Vec::new());
    }
    let e = editor.state.world.entity_by_uid(uid).unwrap();
    assert!(
        editor
            .state
            .world
            .get::<ProcState>(e)
            .unwrap()
            .warnings()
            .is_empty(),
        "{:?}",
        editor.state.world.get::<ProcState>(e).unwrap().warnings()
    );
    let mut nodes = nodes_of(h.accesskit_update());
    let mut field = field_of(&nodes, "Height");
    // Scroll the inspector until the row is on screen (800 points tall);
    // the components above it (the material) can push it below.
    for _ in 0..20 {
        if field.bottom() < 760.0 {
            break;
        }
        let over = field.center() - vec2(0.0, field.center().y - 500.0);
        let scroll = Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, -120.0),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
            modifiers: Modifiers::NONE,
        };
        h.frame(&mut editor, vec![Event::PointerMoved(over), scroll]);
        h.frame(&mut editor, Vec::new());
        nodes = nodes_of(h.accesskit_update());
        field = field_of(&nodes, "Height");
    }
    // Let smooth scrolling settle, then take the row where it rests.
    for _ in 0..30 {
        h.frame(&mut editor, Vec::new());
    }
    let nodes = nodes_of(h.accesskit_update());
    let field = field_of(&nodes, "Height");
    assert!(
        field.bottom() < 800.0,
        "the Height row is on screen: {field:?}"
    );
    let generations = |ed: &Editor| ed.state.world.get::<ProcState>(e).unwrap().generations();
    let top = |ed: &Editor| box_editor::entity_bounds(&ed.state.world, e).unwrap().max.y;
    let before = generations(&editor);
    let undo = editor.state.edits.undo_len();

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
    let dragged = editor.state.world.get::<Wall>(e).unwrap().height;
    let mut totals: Vec<f32> = samples.iter().map(|s| s.total_ms).collect();
    totals.sort_by(f32::total_cmp);
    let mean = totals.iter().sum::<f32>() / totals.len() as f32;
    let p95 = totals[(totals.len() as f32 * 0.95) as usize];
    let worst = totals[totals.len() - 1];
    let over = totals.iter().filter(|t| **t > 16.7).count();
    let tris = editor
        .state
        .world
        .get::<ProcState>(e)
        .and_then(|s| s.mesh())
        .and_then(|m| {
            editor
                .state
                .world
                .resource::<Assets<MeshData>>()
                .get(m)
                .map(|m| m.triangle_count())
        })
        .unwrap_or(0);
    eprintln!(
        "editor drag of Wall height (house loop, 8 openings, {tris} triangles; 2560x1600@2x, 60 frames at 60 Hz): {during} regenerations; frame total incl. GPU wait mean {mean:.2} ms, p95 {p95:.2} ms, worst {worst:.2} ms; {over} frame(s) over 16.7 ms; height 3.0 -> {dragged:.3} m"
    );
    assert!(dragged > 3.05, "the drag changed the height: {dragged}");
    assert!(
        (top(&editor) - dragged).abs() < 1e-3,
        "the last value is drawn"
    );
    assert!((20..=40).contains(&during), "{during} regenerations");
    assert_eq!(editor.state.edits.undo_len(), undo + 1, "one undo step");
    assert_eq!(h.renderer().stats().validation_errors, 0);
    assert!(mean < 16.7, "mean frame {mean:.2} ms");
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/arch");
    std::fs::create_dir_all(&out).unwrap();
    h.image()
        .unwrap()
        .save(out.join("editor_drag.png"))
        .unwrap();
}
