//! Paint Scatter (M6): the brush tool paints a `Scatter`'s mask on its
//! ground, a stroke is one undo step and its undo restores a byte-identical
//! scene, Shift erases, the keys change radius, strength and mode, and the
//! frame cost of a 120-frame stroke over a dense scatter. The last test
//! needs a GPU: a capture mid-stroke with the brush cursor in
//! `target/captures/scatter/`.
//!
//! `cargo test --release -p box_editor --test scatter_brush -- --nocapture`
//! prints the stroke timings.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use box_core::asset_ref::AssetRef;
use box_core::color::Color;
use box_core::identity::{Name, Uid};
use box_core::material::StandardMaterial;
use box_core::prelude::{Entity, GlobalTransform, Transform};
use box_core::scene;
use box_editor::log::LogBuffer;
use box_editor::{Editor, EditorOptions, build_editor};
use box_engine::box_proc::glb::{GlbMaterial, write_glb};
use box_engine::box_proc::scatter::{Scatter, ScatterSpecies};
use box_engine::box_proc::{Generator, Lathe, LatheShape, ProcClock, ProcSettings};
use box_engine::scatter_glue::{ScatterChunk, ScatterRuntime};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, vec2};
use glam::{Vec2, Vec3};

// ----- an egui driver (as in tests/splines.rs) -----

struct Driver {
    ctx: egui::Context,
    time: f64,
    modifiers: Modifiers,
}

const SCREEN: egui::Vec2 = vec2(1280.0, 800.0);
const IDLE: Duration = Duration::from_secs(10);

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

    /// One 60 Hz frame; the manual regeneration clock advances 1/60 s.
    fn frame(&mut self, editor: &mut Editor, mut events: Vec<Event>) {
        tick(editor);
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

    /// Press at `from`, move to `to` in `steps` frames, release.
    fn drag(&mut self, editor: &mut Editor, from: Pos2, to: Pos2, steps: usize) {
        self.frame(editor, vec![Event::PointerMoved(from)]);
        let down = self.button(from, true);
        self.frame(editor, vec![down]);
        for k in 1..=steps {
            let p = from + (to - from) * (k as f32 / steps as f32);
            self.frame(editor, vec![Event::PointerMoved(p)]);
        }
        let up = self.button(to, false);
        self.frame(editor, vec![up]);
        self.frames(editor, 3);
    }
}

fn tick(editor: &mut Editor) {
    if let Some(s) = editor.state.world.get_resource_mut::<ProcSettings>()
        && let ProcClock::Manual(t) = &mut s.clock
    {
        *t += Duration::from_secs_f64(1.0 / 60.0);
    }
}

// ----- project and scene -----

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn lathe(profile: &[(f32, f32)], segments: u32) -> box_core::mesh::MeshData {
    Lathe {
        shape: LatheShape::Custom {
            profile: profile.iter().map(|&(r, y)| Vec2::new(r, y)).collect(),
            closed: false,
        },
        segments,
        smooth_angle: 1.2,
        ..Lathe::default()
    }
    .generate(&box_engine::box_proc::GenContext::new(0))
    .mesh
    .unwrap()
}

/// Writes `assets/scatter/<name>.glb`: a lathe mesh with one material.
fn write_species(dir: &Path, name: &str, mesh: &box_core::mesh::MeshData, color: Color) {
    let material = StandardMaterial {
        roughness: 0.9,
        ..StandardMaterial::color(color)
    };
    let glb = write_glb(
        mesh,
        name,
        &[GlbMaterial {
            name,
            material: &material,
        }],
    )
    .unwrap();
    let path = dir.join("assets/scatter").join(format!("{name}.glb"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, glb.bytes).unwrap();
}

fn species(name: &str) -> ScatterSpecies {
    ScatterSpecies {
        tint_jitter: 0.15,
        ..ScatterSpecies::mesh(
            AssetRef::path(&format!("scatter/{name}.glb#Mesh/{name}")),
            vec![AssetRef::path(&format!(
                "scatter/{name}.glb#Material/{name}"
            ))],
        )
    }
}

/// A project with two species files, opened; a hill (a flattened dome)
/// and a `Scatter` on it with nothing painted yet. Returns (scatter uid,
/// hill uid).
fn open(name: &str, density: f32) -> (Editor, Driver, Uid, TempDir) {
    let dir = std::env::temp_dir().join(format!("box_scatter_{name}_{}", Uid::new()));
    let _ = std::fs::remove_dir_all(&dir);
    box_engine::Project::create(&dir, name).unwrap();
    let bush = lathe(
        &[
            (0.0, 0.0),
            (0.38, 0.04),
            (0.55, 0.3),
            (0.5, 0.58),
            (0.28, 0.8),
            (0.0, 0.86),
        ],
        7,
    );
    let tuft = lathe(&[(0.0, 0.0), (0.07, 0.0), (0.05, 0.2), (0.0, 0.55)], 4);
    write_species(&dir, "bush", &bush, Color::hex(0x4f7a35));
    write_species(&dir, "tuft", &tuft, Color::hex(0x8fae4a));
    let options = EditorOptions {
        project: Some(dir.clone()),
        ..Default::default()
    };
    let mut ed = build_editor(&options, LogBuffer::default()).unwrap();
    ed.tree = box_editor::layout::default_layout();
    ed.state.world.insert_resource(ProcSettings {
        clock: ProcClock::Manual(Duration::from_secs(3600)),
        ..ProcSettings::default()
    });
    let mut d = Driver::new(&ed);
    d.frames(&mut ed, 2);

    let hill = Uid::new();
    ed.state.world.spawn((
        hill,
        Name::new("Hill"),
        Transform {
            scale: Vec3::new(1.0, 0.25, 1.0),
            ..Transform::IDENTITY
        },
        Lathe {
            shape: LatheShape::Dome {
                radius: 14.0,
                rings: 16,
            },
            segments: 48,
            ..Lathe::default()
        },
        StandardMaterial::color(Color::hex(0x7d8a52)),
    ));
    let uid = Uid::new();
    let mut tuft = species("tuft");
    tuft.weight = 5.0;
    tuft.cast_shadows = false;
    ed.state.world.spawn((
        uid,
        Name::new("Meadow"),
        Transform::IDENTITY,
        Scatter {
            species: vec![tuft, species("bush")],
            density,
            min_distance: 0.25,
            max_slope: 1.0,
            surface: Some(hill),
            ..Scatter::default()
        },
    ));
    ed.state.select(Some(uid));
    let orbit = &mut ed.state.camera.orbit;
    orbit.target = Vec3::new(0.0, 1.5, 0.0);
    orbit.distance = 22.0;
    orbit.yaw = 0.3;
    orbit.pitch = 0.65;
    settle(&mut ed);
    d.frames(&mut ed, 2);
    (ed, d, uid, TempDir(dir))
}

fn settle(ed: &mut Editor) {
    assert!(ed.state.wait_assets_idle(IDLE));
    assert!(box_engine::settle_assets(&mut ed.state.world, IDLE));
    box_core::transform::propagate_transforms(&mut ed.state.world);
    box_engine::sync_scene_meshes(&mut ed.state.world);
    box_engine::scatter_glue::settle(&mut ed.state.world);
}

fn entity(ed: &Editor, uid: Uid) -> Entity {
    ed.state.world.entity_by_uid(uid).unwrap()
}

/// Instance world positions of the scatter `uid`.
fn instances(ed: &Editor, uid: Uid) -> Vec<Vec3> {
    let w = &ed.state.world;
    let mut out = Vec::new();
    for c in w.children(entity(ed, uid)) {
        if w.get::<ScatterChunk>(c).is_none() {
            continue;
        }
        for i in w.children(c) {
            if let Some(g) = w.get::<GlobalTransform>(i) {
                out.push(g.0.translation.into());
            }
        }
    }
    out
}

/// Screen position (points) of a world point in the viewport.
fn screen(ed: &Editor, p: Vec3) -> Pos2 {
    let s = ed.state.gizmo_view().unwrap().project(p).unwrap();
    ed.state.viewport.rect.unwrap().min + vec2(s.x, s.y)
}

/// The world point on the hill above (x, z).
fn ground(ed: &Editor, uid: Uid, x: f32, z: f32) -> Vec3 {
    let rt = ed.state.world.resource::<ScatterRuntime>();
    let y = rt
        .surface(entity(ed, uid))
        .and_then(|s| s.sample(Vec2::new(x, z)))
        .map_or(0.0, |h| h.height);
    Vec3::new(x, y, z)
}

fn scene_text(ed: &Editor) -> String {
    scene::save_string(&ed.state.world).unwrap()
}

// ----- tests -----

#[test]
fn a_stroke_paints_as_one_undo_step_and_undo_restores_the_scene() {
    let (mut ed, mut d, uid, _dir) = open("stroke", 3.0);
    // Listed after Place and Spline; needs a scatter selected.
    let tools: Vec<&str> = ed.state.tools().iter().map(|t| t.id).collect();
    assert_eq!(tools, ["place_on_surface", "edit_spline", "paint_scatter"]);
    let action = ed
        .state
        .actions()
        .get("tool.paint_scatter")
        .cloned()
        .unwrap();
    assert_eq!(
        (action.category(), action.label()),
        ("Tools", "Paint Scatter Tool")
    );
    assert!(ed.state.run_action("tool.paint_scatter").is_ok());
    assert!(ed.state.tool_wants_input());
    assert!(instances(&ed, uid).is_empty(), "nothing painted yet");
    d.frames(&mut ed, 2);
    let hint = ed.state.tool_hint().unwrap();
    assert!(
        hint.contains("Add") && hint.contains("radius 2.0 m"),
        "{hint}"
    );

    let before = scene_text(&ed);
    let undo = ed.state.edit_undo_len();
    let from = screen(&ed, ground(&ed, uid, -5.0, 2.0));
    let to = screen(&ed, ground(&ed, uid, 5.0, 2.0));
    d.drag(&mut ed, from, to, 40);
    assert_eq!(ed.state.edit_undo_len(), undo + 1, "one undo step");
    assert_eq!(ed.state.edits.undo_label(), Some("Paint Scatter"));
    let painted = instances(&ed, uid);
    assert!(painted.len() > 30, "{}", painted.len());
    // Along the stroke, within the brush radius (plus the falloff's cells).
    for p in &painted {
        assert!(p.x.abs() <= 7.6 && (p.z - 2.0).abs() <= 2.6, "{p}");
    }
    let mask = ed
        .state
        .world
        .get::<Scatter>(entity(&ed, uid))
        .unwrap()
        .mask
        .clone();
    assert!(!mask.tiles.is_empty());

    // Undo: the scene file is byte-identical and the instances are gone.
    ed.state.undo_scene();
    d.frames(&mut ed, 3);
    assert_eq!(scene_text(&ed), before);
    assert!(instances(&ed, uid).is_empty());
    // Redo: the same mask and the same instances.
    ed.state.redo_scene();
    d.frames(&mut ed, 3);
    let world = &ed.state.world;
    assert_eq!(world.get::<Scatter>(entity(&ed, uid)).unwrap().mask, mask);
    assert_eq!(instances(&ed, uid).len(), painted.len());
}

#[test]
fn shift_erases_and_keys_change_the_brush() {
    let (mut ed, mut d, uid, _dir) = open("erase", 3.0);
    assert!(ed.state.activate_tool("paint_scatter"));
    let a = screen(&ed, ground(&ed, uid, -4.0, 0.0));
    let b = screen(&ed, ground(&ed, uid, 4.0, 0.0));
    d.drag(&mut ed, a, b, 30);
    d.drag(&mut ed, b, a, 30);
    let full = instances(&ed, uid).len();
    assert!(full > 30);

    // Bigger brush, full strength, Shift: erase the middle.
    d.key(&mut ed, Key::CloseBracket);
    d.key(&mut ed, Key::CloseBracket);
    for _ in 0..6 {
        d.key(&mut ed, Key::Equals);
    }
    let hint = ed.state.tool_hint().unwrap();
    assert!(
        hint.contains("radius 3.1 m") && hint.contains("100%"),
        "{hint}"
    );
    d.modifiers = Modifiers::SHIFT;
    let m = screen(&ed, ground(&ed, uid, 0.0, 0.0));
    d.frame(&mut ed, vec![Event::PointerMoved(m)]);
    assert!(ed.state.tool_hint().unwrap().contains("Erase"));
    let (l, r) = (
        screen(&ed, ground(&ed, uid, -1.5, 0.0)),
        screen(&ed, ground(&ed, uid, 1.5, 0.0)),
    );
    d.drag(&mut ed, l, r, 12);
    d.drag(&mut ed, r, l, 12);
    d.modifiers = Modifiers::NONE;
    assert_eq!(ed.state.edits.undo_label(), Some("Erase Scatter"));
    let left = instances(&ed, uid);
    assert!(left.len() < full);
    assert!(
        !left.iter().any(|p| Vec2::new(p.x, p.z).length() < 1.0),
        "the middle is erased"
    );

    // X switches to erasing without Shift; [ and − shrink and weaken.
    d.key(&mut ed, Key::X);
    d.key(&mut ed, Key::OpenBracket);
    d.key(&mut ed, Key::Minus);
    let hint = ed.state.tool_hint().unwrap();
    assert!(
        hint.contains("Erase") && hint.contains("radius 2.5 m") && hint.contains("90%"),
        "{hint}"
    );
    // Escape leaves the tool.
    d.key(&mut ed, Key::Escape);
    assert_eq!(ed.state.active_tool(), None);
}

#[test]
fn stroke_frame_cost() {
    let (mut ed, mut d, uid, _dir) = open("cost", 6.0);
    assert!(ed.state.activate_tool("paint_scatter"));
    // Paint the whole hill first, then measure a long stroke over it.
    for z in [-9.0, -6.0, -3.0, 0.0, 3.0, 6.0, 9.0] {
        let a = screen(&ed, ground(&ed, uid, -10.0, z));
        let b = screen(&ed, ground(&ed, uid, 10.0, z));
        d.drag(&mut ed, a, b, 20);
    }
    settle(&mut ed);
    let total = instances(&ed, uid).len();
    let frames = 120;
    let center = screen(&ed, ground(&ed, uid, 0.0, 0.0));
    let mut times = Vec::with_capacity(frames);
    let mut flushes = Vec::new();
    d.modifiers = Modifiers::SHIFT;
    d.frame(&mut ed, vec![Event::PointerMoved(center)]);
    let down = d.button(center, true);
    d.frame(&mut ed, vec![down]);
    for k in 1..=frames {
        let a = k as f32 / frames as f32 * std::f32::consts::TAU;
        let pos = center + vec2(a.sin() * 160.0, (1.0 - a.cos()) * 60.0 - 60.0);
        let before = ed.state.world.resource::<ScatterRuntime>().stats().flushes;
        let start = Instant::now();
        d.frame(&mut ed, vec![Event::PointerMoved(pos)]);
        times.push(start.elapsed());
        let s = ed.state.world.resource::<ScatterRuntime>().stats();
        if s.flushes > before {
            flushes.push(s.last);
        }
    }
    let up = d.button(center, false);
    d.frame(&mut ed, vec![up]);
    d.modifiers = Modifiers::NONE;
    d.frames(&mut ed, 3);
    assert_eq!(ed.state.edits.undo_label(), Some("Erase Scatter"));

    let ms = |d: &Duration| d.as_secs_f64() * 1e3;
    let mut sorted: Vec<f64> = times.iter().map(ms).collect();
    sorted.sort_by(f64::total_cmp);
    let mean = sorted.iter().sum::<f64>() / sorted.len() as f64;
    let p95 = sorted[sorted.len() * 95 / 100];
    let max = *sorted.last().unwrap();
    let flush_mean = flushes.iter().map(|f| ms(&f.time)).sum::<f64>() / flushes.len().max(1) as f64;
    let flush_max = flushes.iter().map(|f| ms(&f.time)).fold(0.0, f64::max);
    let chunks_mean =
        flushes.iter().map(|f| f.chunks_placed).sum::<usize>() as f64 / flushes.len().max(1) as f64;
    eprintln!(
        "paint stroke over {total} instances ({frames} frames at 60 Hz): editor frame mean {mean:.2} ms, \
         p95 {p95:.2} ms, max {max:.2} ms; {} scatter flushes (mean {flush_mean:.2} ms, max {flush_max:.2} ms, \
         {chunks_mean:.1} chunks each)",
        flushes.len()
    );
    // The throttle: about every other frame at 60 Hz, never every frame.
    assert!(flushes.len() <= frames * 2 / 3 + 1, "{}", flushes.len());
    assert!(flush_max < 16.7, "a flush fits a frame: {flush_max} ms");
}

// ----- GPU: capture -----

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

/// The tool mid-stroke: the brush ring on the hill, painted instances
/// behind it, the button still held.
#[test]
fn paint_capture() {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping captures");
        return;
    }
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/scatter");
    std::fs::create_dir_all(&out).unwrap();
    let (mut ed, mut d, uid, _dir) = open("capture", 4.0);
    assert!(ed.state.activate_tool("paint_scatter"));
    d.frames(&mut ed, 2);
    // A first stroke, finished.
    let a = screen(&ed, ground(&ed, uid, -9.0, 4.0));
    let b = screen(&ed, ground(&ed, uid, 6.0, 5.0));
    d.drag(&mut ed, a, b, 30);
    let Some(mut h) = headless(&mut ed, [2560, 1600]) else {
        return;
    };
    let frame = |h: &mut box_editor::headless::Headless, ed: &mut Editor, e: Vec<Event>| {
        tick(ed);
        h.frame(ed, e);
    };
    frame(&mut h, &mut ed, Vec::new());
    // The second stroke, mid-way: pressed, dragged, not released.
    let from = screen(&ed, ground(&ed, uid, -7.0, -3.0));
    frame(&mut h, &mut ed, vec![Event::PointerMoved(from)]);
    frame(
        &mut h,
        &mut ed,
        vec![Event::PointerButton {
            pos: from,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }],
    );
    let to = screen(&ed, ground(&ed, uid, 2.0, -1.0));
    for k in 1..=12 {
        let p = from + (to - from) * (k as f32 / 12.0);
        frame(&mut h, &mut ed, vec![Event::PointerMoved(p)]);
    }
    for _ in 0..3 {
        frame(&mut h, &mut ed, vec![Event::PointerMoved(to)]);
    }
    let path = out.join("paint_mid_stroke.png");
    h.image().unwrap().save(&path).unwrap();
    eprintln!("{}", path.display());
    assert!(ed.state.edits.is_open(), "the stroke is still open");
    frame(
        &mut h,
        &mut ed,
        vec![Event::PointerButton {
            pos: to,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }],
    );
    assert!(!ed.state.edits.is_open());
}

/// A selected scatter owner with about 10,000 instances: what the
/// selection draws (`target/captures/m6_ux/scatter_selected.png`) and
/// what it costs, selected against not selected (CPU: `prepare_render`,
/// which collects the outline; whole headless frames, GPU included).
#[test]
fn selected_scatter_outline() {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping captures");
        return;
    }
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/m6_ux");
    std::fs::create_dir_all(&out).unwrap();
    let (mut ed, mut d, uid, _dir) = open("outline", 16.0);
    {
        let e = entity(&ed, uid);
        let mut s = ed.state.world.get_mut::<Scatter>(e).unwrap();
        s.area = vec![
            Vec2::new(-14.0, -14.0),
            Vec2::new(14.0, -14.0),
            Vec2::new(14.0, 14.0),
            Vec2::new(-14.0, 14.0),
        ];
        s.min_distance = 0.0;
    }
    settle(&mut ed);
    d.frames(&mut ed, 2);
    let count = instances(&ed, uid).len();
    assert!(count > 8000, "{count} instances");
    let Some(mut h) = headless(&mut ed, [2560, 1600]) else {
        return;
    };
    let ms = |t: Duration| t.as_secs_f64() * 1e3;
    let measure = |ed: &mut Editor, h: &mut box_editor::headless::Headless| {
        for _ in 0..5 {
            h.frame(ed, Vec::new());
        }
        let (mut cpu, mut total) = (0.0, 0.0);
        let n = 30;
        for _ in 0..n {
            let start = Instant::now();
            ed.prepare_render();
            cpu += ms(start.elapsed());
            total += h.frame(ed, Vec::new()).total_ms as f64;
        }
        (cpu / n as f64, total / n as f64)
    };
    ed.state.select(None);
    let (cpu_off, total_off) = measure(&mut ed, &mut h);
    ed.state.select(Some(uid));
    let (cpu_on, total_on) = measure(&mut ed, &mut h);
    let outline = ed
        .state
        .world
        .resource::<box_render::SelectionOutline>()
        .clone();
    eprintln!(
        "selected scatter with {count} instances: {} outlined meshes; prepare_render {cpu_on:.2} ms \
         (not selected {cpu_off:.2} ms), headless frame {total_on:.2} ms (not selected {total_off:.2} ms)",
        outline.primary.len() + outline.secondary.len()
    );
    let path = out.join("scatter_selected.png");
    h.image().unwrap().save(&path).unwrap();
    eprintln!("{}", path.display());
}
