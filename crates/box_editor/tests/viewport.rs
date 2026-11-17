//! Direct manipulation in the viewport (docs/DX_REVIEW.md, batch 2): icons
//! for lights, cameras and empties (click, marquee, priority over meshes),
//! fly navigation, the View menu, the clickable axis gizmo, framing on open
//! and the gizmo readout.
//!
//! Headless and driven by egui events. The last two tests need a GPU: the
//! 1,000-entity cost (run with `--release -- --nocapture` for the numbers)
//! and the captures in `target/captures/viewport/`.

use std::path::{Path, PathBuf};
use std::time::Instant;

use box_core::identity::{Name, Uid};
use box_core::prelude::{Entity, GlobalTransform, Transform};
use box_editor::icons::{ColliderDisplay, IconKind, ViewportIcon};
use box_editor::log::LogBuffer;
use box_editor::{Editor, EditorOptions, EditorState, NewEntity, TransformTool, build_editor};
use egui::accesskit::{Role, TreeUpdate};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use glam::Vec3;

// ----- an egui driver (as in tests/discoverability.rs) -----

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

    fn get(&self, text: &str) -> Node {
        self.nodes
            .iter()
            .rev()
            .find(|n| n.text == text && n.role != Role::TextInput)
            .cloned()
            .unwrap_or_else(|| {
                let all: Vec<&str> = self.nodes.iter().map(|n| n.text.as_str()).collect();
                panic!("no widget `{text}` among {all:?}")
            })
    }

    fn button(&self, pos: Pos2, button: PointerButton, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: self.modifiers,
        }
    }

    fn click_at(&mut self, editor: &mut Editor, pos: Pos2) {
        self.frame(editor, vec![Event::PointerMoved(pos)]);
        let down = self.button(pos, PointerButton::Primary, true);
        self.frame(editor, vec![down]);
        let up = self.button(pos, PointerButton::Primary, false);
        self.frame(editor, vec![up]);
        self.frame(editor, Vec::new());
    }

    fn click(&mut self, editor: &mut Editor, text: &str) {
        let pos = self.get(text).rect.center();
        self.click_at(editor, pos);
        self.frames(editor, 2);
    }

    fn drag(&mut self, editor: &mut Editor, from: Pos2, to: Pos2) {
        self.frame(editor, vec![Event::PointerMoved(from)]);
        let down = self.button(from, PointerButton::Primary, true);
        self.frame(editor, vec![down]);
        for k in 1..=8 {
            let p = from + (to - from) * (k as f32 / 8.0);
            self.frame(editor, vec![Event::PointerMoved(p)]);
        }
        let up = self.button(to, PointerButton::Primary, false);
        self.frame(editor, vec![up]);
        self.frame(editor, Vec::new());
    }

    fn key(&mut self, editor: &mut Editor, key: Key, pressed: bool) {
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

// ----- projects and helpers -----

fn temp_project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("box_vp_{name}_{}", Uid::new()));
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

fn by_name(editor: &Editor, name: &str) -> Uid {
    editor
        .state
        .world
        .query::<(&Uid, &Name)>()
        .iter()
        .find(|(_, n)| n.0 == name)
        .map(|(u, _)| *u)
        .unwrap_or_else(|| panic!("no entity `{name}`"))
}

fn entity(editor: &Editor, uid: Uid) -> Entity {
    editor.state.world.entity_by_uid(uid).unwrap()
}

fn viewport(editor: &Editor) -> Rect {
    editor.state.viewport.rect.expect("the viewport was drawn")
}

/// Screen position (points) of a world point in the viewport.
fn screen(editor: &Editor, p: Vec3) -> Pos2 {
    let s = editor.state.gizmo_view().unwrap().project(p).unwrap();
    viewport(editor).min + vec2(s.x, s.y)
}

fn icon_of(editor: &Editor, uid: Uid) -> Option<ViewportIcon> {
    editor
        .state
        .viewport
        .overlay
        .icons
        .iter()
        .find(|i| i.uid == uid)
        .copied()
}

fn icon_pos(editor: &Editor, uid: Uid) -> Pos2 {
    let i = icon_of(editor, uid).expect("the entity has a visible icon");
    viewport(editor).min + vec2(i.pos.x, i.pos.y)
}

fn move_to(ed: &mut Editor, uid: Uid, at: Vec3) {
    let t = Transform::from_translation(at);
    assert!(ed.state.set_field(uid, "Transform", &t));
}

fn eye(ed: &Editor) -> Vec3 {
    ed.state.camera.orbit.eye()
}

// ----- E2: icons -----

#[test]
fn clicking_light_and_camera_icons_selects_them() {
    let dir = temp_project("icons");
    let (mut ed, mut d) = editor_and_driver(&dir);
    let camera = by_name(&ed, "Camera");
    let light = ed.state.add_entity(NewEntity::PointLight, None).unwrap();
    ed.state.select(None);
    d.frames(&mut ed, 2);
    let kinds: Vec<IconKind> = ed
        .state
        .viewport
        .overlay
        .icons
        .iter()
        .map(|i| i.kind)
        .collect();
    assert!(kinds.contains(&IconKind::Camera), "{kinds:?}");
    assert!(kinds.contains(&IconKind::PointLight), "{kinds:?}");
    for uid in [camera, light] {
        let at = icon_pos(&ed, uid);
        d.click_at(&mut ed, at);
        assert_eq!(ed.state.selection, vec![uid]);
        ed.state.select(None);
    }
    // F frames a selected entity without a mesh.
    ed.state.select(Some(light));
    d.key(&mut ed, Key::F, true);
    d.key(&mut ed, Key::F, false);
    let p = screen(
        &ed,
        ed.state
            .world
            .get::<GlobalTransform>(entity(&ed, light))
            .unwrap()
            .translation(),
    );
    assert!(
        p.distance(viewport(&ed).center()) < 2.0,
        "framed on the light"
    );
    // With icons off they are neither drawn nor clicked.
    ed.state.viewport.view.icons = false;
    d.frames(&mut ed, 1);
    assert!(ed.state.viewport.overlay.icons.is_empty());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_icon_wins_over_the_mesh_behind_it() {
    let dir = temp_project("priority");
    let (mut ed, mut d) = editor_and_driver(&dir);
    let cube = by_name(&ed, "Cube");
    let centre = Vec3::new(0.0, 0.5, 0.0);
    let empty = ed.state.add_entity(NewEntity::Empty, None).unwrap();
    // Between the camera and the cube, so its icon covers the cube.
    let toward_eye = (eye(&ed) - centre).normalize();
    move_to(&mut ed, empty, centre + toward_eye * 1.5);
    ed.state.select(None);
    d.frames(&mut ed, 2);
    let cube_at = screen(&ed, centre);
    assert!(icon_pos(&ed, empty).distance(cube_at) < 1.0);
    d.click_at(&mut ed, cube_at);
    assert_eq!(ed.state.selection, vec![empty]);
    // Beside the icon (outside its disc) the cube is picked (with nothing
    // selected, so no gizmo is in the way).
    ed.state.select(None);
    d.click_at(&mut ed, cube_at + vec2(0.0, 16.0));
    let names: Vec<String> = ed
        .state
        .selection
        .iter()
        .map(|u| ed.state.display_name(entity(&ed, *u)))
        .collect();
    assert_eq!(ed.state.selection, vec![cube], "{names:?}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_marquee_hits_icons_like_clicks_do() {
    let dir = temp_project("marquee");
    let (mut ed, mut d) = editor_and_driver(&dir);
    let camera = by_name(&ed, "Camera");
    d.frames(&mut ed, 1);
    let at = icon_pos(&ed, camera);
    // A rectangle that only touches the icon's disc (not its centre).
    d.drag(&mut ed, at + vec2(6.0, -30.0), at + vec2(40.0, 30.0));
    assert_eq!(ed.state.selection, vec![camera]);
    // A rectangle that misses the disc.
    d.drag(&mut ed, at + vec2(16.0, -30.0), at + vec2(40.0, 30.0));
    assert!(ed.state.selection.is_empty(), "{:?}", ed.state.selection);
    std::fs::remove_dir_all(&dir).unwrap();
}

// ----- E8: fly navigation -----

#[test]
fn right_button_and_w_fly_without_switching_tools() {
    let dir = temp_project("fly");
    let (mut ed, mut d) = editor_and_driver(&dir);
    ed.state.tool = TransformTool::Rotate;
    let start = eye(&ed);
    let forward = ed.state.camera.orbit.transform().forward();
    let distance = ed.state.camera.orbit.distance;
    let c = viewport(&ed).center();
    d.frame(&mut ed, vec![Event::PointerMoved(c)]);
    let down = d.button(c, PointerButton::Secondary, true);
    d.frame(&mut ed, vec![down]);
    d.key(&mut ed, Key::W, true);
    d.frames(&mut ed, 30);
    d.key(&mut ed, Key::W, false);
    assert!(ed.state.camera.flying);
    let moved = eye(&ed) - start;
    assert!(moved.dot(forward) > 1.0, "flew forward: {moved}");
    assert!(moved.reject_from(forward).length() < 1e-2, "{moved}");
    // E (up) does not pick the Rotate tool either; the tool is unchanged.
    ed.state.tool = TransformTool::Scale;
    let start = eye(&ed);
    d.key(&mut ed, Key::E, true);
    d.frames(&mut ed, 10);
    d.key(&mut ed, Key::E, false);
    assert_eq!(ed.state.tool, TransformTool::Scale);
    assert!(eye(&ed).y > start.y + 0.2, "flew up");
    // The wheel sets the speed and shows it.
    let speed = ed.state.camera.fly_speed;
    d.frame(
        &mut ed,
        vec![Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: vec2(0.0, 2.0),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
            modifiers: Modifiers::NONE,
        }],
    );
    assert!(ed.state.camera.fly_speed > speed);
    assert!(ed.state.viewport.overlay.fly_pill_until > d.time);
    let up = d.button(c, PointerButton::Secondary, false);
    d.frame(&mut ed, vec![up]);
    d.frames(&mut ed, 1);
    assert!(!ed.state.camera.flying);
    assert!((ed.state.camera.orbit.distance - distance).abs() < 1e-3);
    // Without the right button, W is the Move tool again.
    d.key(&mut ed, Key::W, true);
    d.key(&mut ed, Key::W, false);
    assert_eq!(ed.state.tool, TransformTool::Translate);
    std::fs::remove_dir_all(&dir).unwrap();
}

// ----- E19: axis gizmo and View menu -----

/// The axis gizmo's end for `axis` (as the viewport draws it).
fn axis_end(ed: &Editor, axis: Vec3) -> Pos2 {
    let r = viewport(ed);
    let c = r.right_top() + vec2(-40.0, 42.0);
    let t = ed.state.camera.orbit.transform();
    c + vec2(axis.dot(t.right()), -axis.dot(t.up())) * 19.0
}

#[test]
fn clicking_an_axis_looks_along_it() {
    let dir = temp_project("axis");
    let (mut ed, mut d) = editor_and_driver(&dir);
    let selection = ed.state.selection.clone();
    let at = axis_end(&ed, Vec3::X);
    d.click_at(&mut ed, at);
    d.frames(&mut ed, 30);
    let f = ed.state.camera.orbit.transform().forward();
    assert!((f - Vec3::NEG_X).length() < 1e-3, "looks along −X: {f}");
    assert_eq!(ed.state.selection, selection, "no pick through the gizmo");
    // Again: from the other side.
    let at = axis_end(&ed, Vec3::X);
    d.click_at(&mut ed, at);
    d.frames(&mut ed, 30);
    let f = ed.state.camera.orbit.transform().forward();
    assert!((f - Vec3::X).length() < 1e-3, "{f}");
    // Top view.
    let at = axis_end(&ed, Vec3::Y);
    d.click_at(&mut ed, at);
    d.frames(&mut ed, 30);
    let f = ed.state.camera.orbit.transform().forward();
    assert!(f.y < -0.999, "{f}");
    std::fs::remove_dir_all(&dir).unwrap();
}

fn view_pill(ed: &Editor) -> Pos2 {
    viewport(ed).left_top() + vec2(40.0, 20.0)
}

#[test]
fn the_view_menu_toggles_grid_icons_and_colliders() {
    let dir = temp_project("view");
    let (mut ed, mut d) = editor_and_driver(&dir);
    assert!(ed.state.show_grid && ed.state.viewport.view.icons);
    let pill = view_pill(&ed);
    d.click_at(&mut ed, pill);
    d.frames(&mut ed, 2);
    assert!(egui::Popup::is_any_open(&d.ctx), "the View menu opened");
    d.click(&mut ed, "Grid");
    d.click(&mut ed, "Icons");
    // Collider wireframes' "All" (the upper of the two rows; the other is
    // Particle preview, tested in `m7_usability.rs`).
    let all = d
        .nodes
        .iter()
        .filter(|n| n.text == "All")
        .map(|n| n.rect.center())
        .min_by(|a, b| a.y.total_cmp(&b.y))
        .expect("an All option");
    d.click_at(&mut ed, all);
    d.frames(&mut ed, 2);
    assert!(!ed.state.show_grid);
    assert!(!ed.state.viewport.view.icons);
    assert_eq!(ed.state.viewport.view.colliders, ColliderDisplay::All);
    assert!(ed.state.viewport.overlay.icons.is_empty());
    d.click(&mut ed, "Top  (+Y)");
    d.frames(&mut ed, 30);
    assert!(ed.state.camera.orbit.transform().forward().y < -0.999);
    std::fs::remove_dir_all(&dir).unwrap();
}

// ----- W10: framing on open -----

#[test]
fn a_new_project_opens_on_the_cube_and_its_camera() {
    let dir = temp_project("frame");
    let (mut ed, mut d) = editor_and_driver(&dir);
    let r = viewport(&ed);
    let top = screen(&ed, Vec3::new(0.0, 1.0, 0.0));
    let bottom = screen(&ed, Vec3::ZERO);
    let height = bottom.y - top.y;
    assert!(
        height > r.height() * 0.1,
        "the cube is {height} of {} points tall",
        r.height()
    );
    let camera = by_name(&ed, "Camera");
    assert!(
        icon_of(&ed, camera).is_some(),
        "the scene camera is in view"
    );
    // Undo does not reframe.
    let cube = by_name(&ed, "Cube");
    let before = ed.state.camera.orbit;
    move_to(&mut ed, cube, Vec3::new(3.0, 0.5, 0.0));
    ed.state.undo();
    d.frames(&mut ed, 2);
    assert_eq!(ed.state.camera.orbit.target, before.target);
    assert_eq!(ed.state.camera.orbit.distance, before.distance);
    std::fs::remove_dir_all(&dir).unwrap();
}

// ----- E26: gizmo readout -----

#[test]
fn a_gizmo_drag_shows_its_change() {
    let dir = temp_project("readout");
    let (mut ed, mut d) = editor_and_driver(&dir);
    let cube = by_name(&ed, "Cube");
    ed.state.select(Some(cube));
    ed.state.snap.enabled = true;
    d.frames(&mut ed, 2);
    let origin = Vec3::new(0.0, 0.5, 0.0);
    let from = screen(&ed, origin + Vec3::X * 0.4);
    let to = screen(&ed, origin + Vec3::X * 1.6);
    d.frame(&mut ed, vec![Event::PointerMoved(from)]);
    let down = d.button(from, PointerButton::Primary, true);
    d.frame(&mut ed, vec![down]);
    for k in 1..=6 {
        d.frame(
            &mut ed,
            vec![Event::PointerMoved(from + (to - from) * (k as f32 / 6.0))],
        );
    }
    let text = ed.state.gizmo_readout().expect("a drag is in progress");
    assert_eq!(text, "X  Δ 1.00 m   ·   Snap 0.5 m");
    let up = d.button(to, PointerButton::Primary, false);
    d.frame(&mut ed, vec![up]);
    assert!(ed.state.gizmo_readout().is_none());
    std::fs::remove_dir_all(&dir).unwrap();
}

// ----- GPU: cost and captures -----

fn skip_gpu() -> bool {
    std::env::var_os("BOX_SKIP_GPU_TESTS").is_some()
}

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

/// 1,000 entities with icons (empties, lights and cameras in a grid): the
/// cost of placing the icons (`icons::collect`) and of a whole headless
/// frame with icons on and off. Run with `--release -- --nocapture`.
#[test]
fn icon_cost_for_a_thousand_entities() {
    if skip_gpu() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping");
        return;
    }
    let mut world = box_core::prelude::World::with_registry(box_engine::shared_engine_registry());
    box_editor::scratch::prepare_world(&mut world);
    for k in 0..1000u32 {
        let (x, z) = ((k % 40) as f32 - 20.0, (k / 40) as f32 - 12.0);
        let t = Transform::from_xyz(x * 1.5, 1.0, z * 1.5);
        let uid = Uid::from_u128(1000 + k as u128);
        let name = format!("E{k}");
        match k % 10 {
            0 => world.spawn_saved(uid, &name, (t, box_render::Camera::default())),
            1 => world.spawn_saved(uid, &name, (t, box_render::PointLight::default())),
            _ => world.spawn_saved(uid, &name, (t,)),
        }
        .unwrap();
    }
    box_core::transform::propagate_transforms(&mut world);
    let mut state = EditorState::new(world, LogBuffer::default());
    state.camera.orbit.target = Vec3::ZERO;
    state.camera.orbit.distance = 70.0;
    let mut ed = Editor::new(state);
    ed.tree = box_editor::layout::default_layout();
    let Some(mut h) = headless(&mut ed, [2560, 1440]) else {
        return;
    };
    for _ in 0..4 {
        h.frame(&mut ed, Vec::new());
    }
    let shown = ed.state.viewport.overlay.icons.len();
    assert!(shown > 900, "{shown} icons on screen");
    let view = ed.state.gizmo_view().unwrap();
    let mut icons = Vec::new();
    let n = 200;
    let t0 = Instant::now();
    for _ in 0..n {
        box_editor::icons::collect(&ed.state.world, &view, &mut icons);
    }
    let collect_ms = t0.elapsed().as_secs_f64() * 1000.0 / n as f64;
    let frame = |ed: &mut Editor, h: &mut box_editor::headless::Headless| {
        let mut ui = 0.0;
        let mut render = 0.0;
        for _ in 0..4 {
            h.frame(ed, Vec::new());
        }
        let frames = 30;
        for _ in 0..frames {
            let t = h.frame(ed, Vec::new());
            ui += t.ui_ms as f64;
            render += t.render_ms as f64;
        }
        (ui / frames as f64, render / frames as f64)
    };
    let (ui_on, render_on) = frame(&mut ed, &mut h);
    ed.state.viewport.view.icons = false;
    let (ui_off, render_off) = frame(&mut ed, &mut h);
    eprintln!(
        "1,000 icon entities ({shown} on screen): collect {collect_ms:.3} ms; \
         frame UI {ui_on:.2} ms with icons / {ui_off:.2} ms without, \
         paint+render {render_on:.2} / {render_off:.2} ms"
    );
    if !cfg!(debug_assertions) {
        assert!(collect_ms < 1.0, "collect took {collect_ms} ms");
    }
}

/// Captures for review in `target/captures/viewport/`.
#[test]
fn viewport_captures() {
    if skip_gpu() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping captures");
        return;
    }
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/viewport");
    std::fs::create_dir_all(&out).unwrap();
    let dir = temp_project("captures");
    let mut ed = open(&dir);
    let Some(mut h) = headless(&mut ed, [2560, 1600]) else {
        return;
    };
    h.enable_accesskit();
    let run = |h: &mut box_editor::headless::Headless, ed: &mut Editor, events: Vec<Event>| {
        h.frame(ed, events);
        nodes_of(h.accesskit_update())
    };
    let settle = |h: &mut box_editor::headless::Headless, ed: &mut Editor, n: usize| {
        let mut nodes = Vec::new();
        for _ in 0..n {
            h.frame(ed, Vec::new());
            nodes = nodes_of(h.accesskit_update());
        }
        nodes
    };
    let save = |h: &box_editor::headless::Headless, name: &str| {
        h.image().unwrap().save(out.join(name)).unwrap();
    };
    let press = |pos, button, pressed| Event::PointerButton {
        pos,
        button,
        pressed,
        modifiers: Modifiers::NONE,
    };
    settle(&mut h, &mut ed, 3);
    save(&h, "starter_open.png");

    // A point light and a capsule with colliders, then the starter's
    // camera selected with its frustum.
    let light = ed.state.add_entity(NewEntity::PointLight, None).unwrap();
    move_to(&mut ed, light, Vec3::new(-2.0, 2.5, 0.5));
    let capsule = ed.state.add_entity(NewEntity::Capsule, None).unwrap();
    move_to(&mut ed, capsule, Vec3::new(1.8, 1.0, -0.6));
    let cube = by_name(&ed, "Cube");
    ed.state.add_component(capsule, "Collider");
    ed.state.add_component(cube, "Collider");
    let camera = by_name(&ed, "Camera");
    ed.state.select(Some(camera));
    let orbit = &mut ed.state.camera.orbit;
    orbit.target = Vec3::new(1.0, 1.0, 1.4);
    orbit.distance = 9.0;
    orbit.yaw = 1.9;
    orbit.pitch = 0.35;
    settle(&mut h, &mut ed, 3);
    save(&h, "selected_camera_frustum.png");

    // The sun's direction.
    let sun = by_name(&ed, "Sun");
    ed.state.select(Some(sun));
    let orbit = &mut ed.state.camera.orbit;
    orbit.target = Vec3::new(0.0, 6.0, 0.0);
    orbit.distance = 16.0;
    orbit.yaw = 0.75;
    orbit.pitch = 0.2;
    settle(&mut h, &mut ed, 3);
    save(&h, "selected_sun_direction.png");

    // The point light's range.
    ed.state.select(Some(light));
    let orbit = &mut ed.state.camera.orbit;
    orbit.target = Vec3::new(-2.0, 2.0, 0.5);
    orbit.distance = 34.0;
    orbit.pitch = 0.45;
    settle(&mut h, &mut ed, 3);
    save(&h, "selected_point_light_range.png");

    // A spot light (an empty with the component) aimed down and sideways:
    // its icon and cone.
    let spot = ed.state.add_entity(NewEntity::Empty, None).unwrap();
    let aim = Transform::from_xyz(-1.0, 3.0, 1.0).looking_at(Vec3::new(0.5, 0.0, 0.0), Vec3::Y);
    assert!(ed.state.set_field(spot, "Transform", &aim));
    ed.state.add_component(spot, "SpotLight");
    ed.state.select(Some(spot));
    let orbit = &mut ed.state.camera.orbit;
    orbit.target = Vec3::new(0.0, 1.5, 0.5);
    orbit.distance = 12.0;
    orbit.yaw = 0.9;
    orbit.pitch = 0.3;
    settle(&mut h, &mut ed, 3);
    let kinds: Vec<IconKind> = ed
        .state
        .viewport
        .overlay
        .icons
        .iter()
        .map(|i| i.kind)
        .collect();
    assert!(kinds.contains(&IconKind::SpotLight), "{kinds:?}");
    save(&h, "selected_spot_light_cone.png");

    // Collider wireframes: the selected capsule, then all.
    ed.state.select(Some(capsule));
    ed.state.frame_on_open();
    settle(&mut h, &mut ed, 3);
    save(&h, "collider_selected.png");
    ed.state.viewport.view.colliders = ColliderDisplay::All;
    ed.state.select(None);
    settle(&mut h, &mut ed, 3);
    save(&h, "collider_all.png");

    // The View menu, open.
    let vp = viewport(&ed);
    let pill = view_pill(&ed);
    run(&mut h, &mut ed, vec![Event::PointerMoved(pill)]);
    run(
        &mut h,
        &mut ed,
        vec![
            press(pill, PointerButton::Primary, true),
            press(pill, PointerButton::Primary, false),
        ],
    );
    settle(&mut h, &mut ed, 3);
    save(&h, "view_menu.png");
    let away = vp.left_bottom() + vec2(200.0, -200.0);
    run(
        &mut h,
        &mut ed,
        vec![
            Event::PointerMoved(away),
            press(away, PointerButton::Secondary, true),
            press(away, PointerButton::Secondary, false),
        ],
    );
    ed.state.viewport.view.colliders = ColliderDisplay::Selected;

    // Top view from the axis gizmo (+Y), with the cube selected.
    ed.state.select(Some(cube));
    ed.state.frame_on_open();
    settle(&mut h, &mut ed, 2);
    let at = axis_end(&ed, Vec3::Y);
    run(&mut h, &mut ed, vec![Event::PointerMoved(at)]);
    run(
        &mut h,
        &mut ed,
        vec![
            press(at, PointerButton::Primary, true),
            press(at, PointerButton::Primary, false),
        ],
    );
    settle(&mut h, &mut ed, 6);
    save(&h, "axis_turning.png");
    settle(&mut h, &mut ed, 30);
    save(&h, "axis_top_view.png");

    // The gizmo readout mid-drag.
    ed.state.frame_on_open();
    ed.state.snap.enabled = true;
    settle(&mut h, &mut ed, 2);
    let origin = Vec3::new(0.0, 0.5, 0.0);
    let from = screen(&ed, origin + Vec3::X * 0.4);
    let to = screen(&ed, origin + Vec3::X * 1.7);
    run(&mut h, &mut ed, vec![Event::PointerMoved(from)]);
    run(
        &mut h,
        &mut ed,
        vec![press(from, PointerButton::Primary, true)],
    );
    for k in 1..=6 {
        let p = from + (to - from) * (k as f32 / 6.0);
        run(&mut h, &mut ed, vec![Event::PointerMoved(p)]);
    }
    save(&h, "gizmo_readout.png");
    run(
        &mut h,
        &mut ed,
        vec![press(to, PointerButton::Primary, false)],
    );
    ed.state.undo();

    // Flying: the speed after a wheel step.
    let c = vp.center();
    run(&mut h, &mut ed, vec![Event::PointerMoved(c)]);
    run(
        &mut h,
        &mut ed,
        vec![press(c, PointerButton::Secondary, true)],
    );
    run(
        &mut h,
        &mut ed,
        vec![Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: vec2(0.0, 3.0),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
            modifiers: Modifiers::NONE,
        }],
    );
    settle(&mut h, &mut ed, 2);
    save(&h, "fly_speed.png");
    run(
        &mut h,
        &mut ed,
        vec![press(c, PointerButton::Secondary, false)],
    );

    // The scratch scene.
    let mut scratch =
        build_editor(&EditorOptions::default(), LogBuffer::default()).expect("scratch editor");
    scratch.tree = box_editor::layout::default_layout();
    let Some(mut h) = headless(&mut scratch, [2560, 1600]) else {
        return;
    };
    settle(&mut h, &mut scratch, 4);
    save(&h, "scratch_open.png");
    eprintln!("captures in {}", out.display());
    std::fs::remove_dir_all(&dir).unwrap();
}
