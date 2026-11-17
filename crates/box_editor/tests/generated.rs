//! Generated objects in the editor (M5 derived entities) and the viewport
//! tool extension point: a click on a derived part selects its owner,
//! Cmd+Alt+click inspects the part read-only, the inspector's "Generated
//! by" and "Generated" sections, rule warnings (inspector, hierarchy and
//! viewport badges), and the Place on Surface tool (activation, one undo
//! step per placement, Escape).
//!
//! Headless and driven by egui events. The last test needs a GPU: captures
//! in `target/captures/generated/`.

use std::path::{Path, PathBuf};

use box_core::derived::{Derived, PartKey};
use box_core::identity::{Name, Uid};
use box_core::prelude::{Entity, GlobalTransform, Primitive, Shape, Transform};
use box_editor::log::LogBuffer;
use box_editor::{Editor, EditorOptions, NewEntity, TransformTool, build_editor};
use box_proc::derive::{Part, reconcile_with};
use egui::accesskit::{Role, TreeUpdate};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use glam::Vec3;

// ----- an egui driver (as in tests/viewport.rs) -----

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
        // What the renderer does before drawing: meshes and transforms.
        editor.prepare_render();
        self.nodes = nodes_of(out.platform_output.accesskit_update.as_ref());
        self.time += 1.0 / 60.0;
    }

    fn frames(&mut self, editor: &mut Editor, n: usize) {
        for _ in 0..n {
            self.frame(editor, Vec::new());
        }
    }

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

    fn has_text_containing(&self, part: &str) -> bool {
        self.nodes.iter().any(|n| n.text.contains(part))
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

/// Cmd + Alt (Ctrl + Alt elsewhere).
fn cmd_alt() -> Modifiers {
    Modifiers {
        alt: true,
        ctrl: !cfg!(target_os = "macos"),
        shift: false,
        mac_cmd: cfg!(target_os = "macos"),
        command: true,
    }
}

// ----- projects and helpers -----

fn temp_project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("box_gen_{name}_{}", Uid::new()));
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

fn world_pos(editor: &Editor, uid: Uid) -> Vec3 {
    let e = entity(editor, uid);
    Vec3::from(
        editor
            .state
            .world
            .get::<GlobalTransform>(e)
            .unwrap()
            .0
            .translation,
    )
}

fn post(size: Vec3) -> Primitive {
    Primitive::new(Shape::Cuboid { size })
}

/// A "Fence" owner at (-2, 0, 1.5) with two posts and a rail made by the
/// rule `fence`, and one rule warning when `warn`.
fn fence(ed: &mut Editor, warn: bool) -> (Uid, Vec<Uid>) {
    let owner = ed.state.add_entity(NewEntity::Empty, None).unwrap();
    ed.state.rename(owner, "Fence");
    assert!(
        ed.state
            .set_field(owner, "Transform", &Transform::from_xyz(-2.0, 0.0, 1.5))
    );
    let o = entity(ed, owner);
    let parts = vec![
        Part::new(PartKey::new(owner, "post/start"))
            .at(Transform::from_xyz(0.0, 0.6, 0.0))
            .with(post(Vec3::new(0.2, 1.2, 0.2))),
        Part::new(PartKey::new(owner, "post/end"))
            .at(Transform::from_xyz(1.6, 0.6, 0.0))
            .with(post(Vec3::new(0.2, 1.2, 0.2)))
            .warn("post leans 3° past the limit, straightened"),
        Part::new(PartKey::new(owner, "rail"))
            .at(Transform::from_xyz(0.8, 0.9, 0.0))
            .with(post(Vec3::new(1.6, 0.12, 0.08))),
    ];
    let rule_warnings = if warn {
        vec!["fence end is 4 cm from a wall: not joined".to_string()]
    } else {
        Vec::new()
    };
    let r = reconcile_with(&mut ed.state.world, o, "fence", parts, rule_warnings).unwrap();
    assert_eq!(r.spawned, 3);
    ed.prepare_render();
    let uids = ["post/start", "post/end", "rail"]
        .iter()
        .map(|r| PartKey::new(owner, r).uid())
        .collect();
    (owner, uids)
}

fn look_at_fence(ed: &mut Editor) {
    let orbit = &mut ed.state.camera.orbit;
    orbit.target = Vec3::new(-1.0, 0.6, 1.0);
    orbit.distance = 7.0;
    orbit.yaw = 0.35;
    orbit.pitch = 0.3;
}

fn editor_with_fence(name: &str, warn: bool) -> (PathBuf, Editor, Driver, Uid, Vec<Uid>) {
    let dir = temp_project(name);
    let mut ed = open(&dir);
    let mut d = Driver::new(&ed);
    d.frames(&mut ed, 3);
    let (owner, parts) = fence(&mut ed, warn);
    look_at_fence(&mut ed);
    ed.state.select(None);
    d.frames(&mut ed, 3);
    (dir, ed, d, owner, parts)
}

// ----- selection -----

#[test]
fn clicking_a_part_selects_its_owner_and_parts_are_hidden_in_the_hierarchy() {
    let (dir, mut ed, mut d, owner, parts) = editor_with_fence("click", false);
    let at = screen(&ed, world_pos(&ed, parts[0]));
    d.click_at(&mut ed, at);
    assert_eq!(ed.state.selection, vec![owner]);
    assert_eq!(ed.state.inspected_part(), None);
    // The marquee maps parts to the owner too.
    ed.state.select(None);
    let rail = screen(&ed, world_pos(&ed, parts[2]));
    d.modifiers = Modifiers::NONE;
    let (a, b) = (rail - vec2(40.0, 30.0), rail + vec2(40.0, 30.0));
    d.frame(&mut ed, vec![Event::PointerMoved(a)]);
    let down = d.button(a, PointerButton::Primary, true);
    d.frame(&mut ed, vec![down]);
    for k in 1..=6 {
        d.frame(
            &mut ed,
            vec![Event::PointerMoved(a + (b - a) * (k as f32 / 6.0))],
        );
    }
    let up = d.button(b, PointerButton::Primary, false);
    d.frame(&mut ed, vec![up]);
    d.frames(&mut ed, 2);
    assert!(
        ed.state.selection.contains(&owner),
        "{:?}",
        ed.state.selection
    );
    assert!(parts.iter().all(|p| !ed.state.selection.contains(p)));
    // Hierarchy rows: the owner, never its parts; the owner has a parts mark.
    let rows: Vec<Uid> = ed.state.hierarchy.rows.iter().map(|r| r.uid).collect();
    assert!(rows.contains(&owner));
    assert!(parts.iter().all(|p| !rows.contains(p)));
    assert_eq!(ed.state.generated_mark(owner).unwrap().parts, 3);
    // Deleting the owner deletes the parts; undo brings back the owner only
    // (its rule rebuilds them), and the scene has no part in it.
    ed.state.select(Some(owner));
    ed.state.delete_selection();
    assert_eq!(ed.state.world.query::<&Derived>().iter().count(), 0);
    ed.state.undo();
    assert!(ed.state.world.entity_by_uid(owner).is_some());
    let saved = String::from_utf8(box_core::scene::save(&ed.state.world).unwrap()).unwrap();
    assert!(!saved.contains("post/start"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn cmd_alt_click_inspects_a_part_read_only() {
    let (dir, mut ed, mut d, owner, parts) = editor_with_fence("inspect", false);
    let undo = ed.state.edit_undo_len();
    let at = screen(&ed, world_pos(&ed, parts[1]));
    d.modifiers = cmd_alt();
    d.click_at(&mut ed, at);
    d.modifiers = Modifiers::NONE;
    d.frames(&mut ed, 2);
    assert_eq!(ed.state.inspected_part(), Some(parts[1]));
    assert!(ed.state.selection.is_empty(), "the part is not selected");
    // "Generated by": the rule, the key's role and a link to the owner; the
    // part's own warning.
    d.get("fence");
    d.get("post/end");
    assert!(d.has_text_containing("leans 3°"));
    // Its components are shown (disabled) and cannot be edited.
    let t = Transform::from_xyz(9.0, 9.0, 9.0);
    assert!(!ed.state.set_field(parts[1], "Transform", &t));
    assert_eq!(ed.state.edit_undo_len(), undo);
    // The owner link selects the owner.
    d.click(&mut ed, "Fence");
    assert_eq!(ed.state.selection, vec![owner]);
    assert_eq!(ed.state.inspected_part(), None);
    // On an ordinary object Cmd+Alt+click just selects it.
    let cube = by_name(&ed, "Cube");
    let at = screen(&ed, world_pos(&ed, cube));
    d.modifiers = cmd_alt();
    d.click_at(&mut ed, at);
    d.modifiers = Modifiers::NONE;
    assert_eq!(ed.state.selection, vec![cube]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_generated_section_lists_parts_and_opens_them() {
    let (dir, mut ed, mut d, owner, parts) = editor_with_fence("section", false);
    ed.state.select(Some(owner));
    d.frames(&mut ed, 2);
    d.get("fence  ·  3 parts");
    d.click(&mut ed, "rail");
    assert_eq!(ed.state.inspected_part(), Some(parts[2]));
    // Any selection change goes back to the entity inspector.
    ed.state.select(Some(owner));
    assert_eq!(ed.state.inspected_part(), None);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn rule_warnings_show_on_the_owner_its_row_and_a_viewport_badge() {
    let (dir, mut ed, mut d, owner, _) = editor_with_fence("warnings", true);
    let mark = ed.state.generated_mark(owner).unwrap().clone();
    assert_eq!(mark.warnings.len(), 2, "{:?}", mark.warnings);
    assert!(mark.warnings[0].contains("not joined"));
    assert!(box_editor::feedback::toasts_for_target("box_proc::derive"));
    // The viewport badge sits up and right of the owner; a click selects it.
    let at = screen(&ed, world_pos(&ed, owner)) + vec2(14.0, -14.0);
    d.click_at(&mut ed, at);
    d.frames(&mut ed, 2);
    assert_eq!(ed.state.selection, vec![owner]);
    // The inspector lists the warnings, one line each.
    assert!(d.has_text_containing("fence: fence end is 4 cm from a wall"));
    assert!(d.has_text_containing("fence (post/end): post leans"));
    // Fixed: the marks follow (rebuilt on the generated revision only).
    let o = entity(&ed, owner);
    let parts = vec![Part::new(PartKey::new(owner, "rail"))];
    reconcile_with(&mut ed.state.world, o, "fence", parts, Vec::new()).unwrap();
    d.frames(&mut ed, 2);
    let mark = ed.state.generated_mark(owner).unwrap();
    assert!(mark.warnings.is_empty());
    assert_eq!(mark.parts, 1);
    let _ = std::fs::remove_dir_all(dir);
}

// ----- viewport tools -----

#[test]
fn place_on_surface_is_listed_activated_cancelled_and_one_undo_step() {
    let dir = temp_project("place");
    let mut ed = open(&dir);
    let mut d = Driver::new(&ed);
    d.frames(&mut ed, 3);
    // Listed in the toolbar group and the palette.
    let info = ed.state.tools()[0];
    assert_eq!((info.id, info.label), ("place_on_surface", "Place"));
    let action = ed.state.actions().get("tool.place_on_surface").cloned();
    assert_eq!(
        action.as_ref().map(|a| (a.category(), a.label())),
        Some(("Tools", "Place on Surface Tool"))
    );
    ed.state.open_palette();
    ed.state.set_palette_query("place on");
    let rows = ed.state.palette_results();
    assert_eq!(rows[0].id, "tool.place_on_surface", "{rows:?}");
    ed.state.palette_run_selected().unwrap();
    assert_eq!(ed.state.active_tool(), Some("place_on_surface"));

    // With nothing selected, clicks still select.
    let cube = by_name(&ed, "Cube");
    let sphere = ed.state.add_entity(NewEntity::Sphere, None).unwrap();
    assert!(
        ed.state
            .set_field(sphere, "Transform", &Transform::from_xyz(2.5, 0.5, 2.0))
    );
    ed.state.select(None);
    let orbit = &mut ed.state.camera.orbit;
    orbit.target = Vec3::new(0.8, 0.5, 0.8);
    orbit.distance = 7.0;
    orbit.yaw = 0.5;
    orbit.pitch = 0.45;
    d.frames(&mut ed, 2);
    let sphere_at = screen(&ed, world_pos(&ed, sphere));
    d.click_at(&mut ed, sphere_at);
    assert_eq!(ed.state.selection, vec![sphere]);
    assert_eq!(ed.state.active_tool(), Some("place_on_surface"));

    // A click on the cube's top puts the sphere on it: one undo step.
    let undo = ed.state.edit_undo_len();
    let top = screen(&ed, Vec3::new(0.1, 1.0, 0.1));
    d.click_at(&mut ed, top);
    d.frames(&mut ed, 1);
    assert_eq!(ed.state.edit_undo_len(), undo + 1);
    assert_eq!(ed.state.selection, vec![sphere], "the selection stays");
    let b = box_editor::entity_bounds(&ed.state.world, entity(&ed, sphere)).unwrap();
    assert!((b.min.y - 1.0).abs() < 0.02, "rests on the cube: {b:?}");
    let p = world_pos(&ed, sphere);
    assert!((p.x - 0.1).abs() < 0.02 && (p.z - 0.1).abs() < 0.02, "{p}");
    assert_eq!(world_pos(&ed, cube), Vec3::new(0.0, 0.5, 0.0));
    // On the ground, again one step.
    let ground = screen(&ed, Vec3::new(1.5, 0.0, -1.0));
    d.click_at(&mut ed, ground);
    assert_eq!(ed.state.edit_undo_len(), undo + 2);
    let b = box_editor::entity_bounds(&ed.state.world, entity(&ed, sphere)).unwrap();
    assert!(b.min.y.abs() < 0.02, "rests on the ground: {b:?}");
    ed.state.undo();
    d.frames(&mut ed, 1);
    let b = box_editor::entity_bounds(&ed.state.world, entity(&ed, sphere)).unwrap();
    assert!((b.min.y - 1.0).abs() < 0.02, "undo: back on the cube");

    // Escape leaves the tool and keeps the selection; W too.
    d.key(&mut ed, Key::Escape);
    assert_eq!(ed.state.active_tool(), None);
    assert_eq!(ed.state.selection, vec![sphere]);
    assert!(ed.state.run_action("tool.place_on_surface").is_ok());
    assert_eq!(ed.state.active_tool(), Some("place_on_surface"));
    ed.state.tool = TransformTool::Rotate;
    d.key(&mut ed, Key::W);
    assert_eq!(ed.state.active_tool(), None);
    assert_eq!(ed.state.tool, TransformTool::Translate);
    let _ = std::fs::remove_dir_all(dir);
}

/// A game tool through the public API: keys reach it first, and a
/// finished tool goes back to the transform tools.
#[test]
fn registered_tools_get_keys_and_can_finish() {
    use box_editor::tools::{ToolEvent, ToolEventKind, ToolResponse, ViewportTool};
    struct Counter {
        keys: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }
    impl ViewportTool for Counter {
        fn id(&self) -> &'static str {
            "counter"
        }
        fn label(&self) -> &'static str {
            "Count"
        }
        fn tooltip(&self) -> &'static str {
            "Counts Enter presses; a click finishes"
        }
        fn on_pointer(&mut self, _: &mut box_editor::EditorState, e: &ToolEvent) -> ToolResponse {
            match e.kind {
                ToolEventKind::Click => ToolResponse::Finished,
                _ => ToolResponse::Ignored,
            }
        }
        fn on_key(&mut self, _: &mut box_editor::EditorState, key: Key, _: Modifiers) -> bool {
            if key == Key::Enter {
                self.keys.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return true;
            }
            false
        }
    }
    let dir = temp_project("custom_tool");
    let mut ed = open(&dir);
    let keys = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    ed.register_tool(Counter { keys: keys.clone() });
    assert!(ed.state.actions().get("tool.counter").is_some());
    let mut d = Driver::new(&ed);
    d.frames(&mut ed, 3);
    assert!(ed.state.activate_tool("counter"));
    d.key(&mut ed, Key::Enter);
    assert_eq!(keys.load(std::sync::atomic::Ordering::Relaxed), 1);
    let centre = viewport(&ed).center();
    d.click_at(&mut ed, centre);
    assert_eq!(ed.state.active_tool(), None, "finished");
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

/// Captures for review in `target/captures/generated/`: an owner with its
/// Generated section and warnings, a part inspected read-only, and Place on
/// Surface in use.
#[test]
fn generated_captures() {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping captures");
        return;
    }
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/generated");
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
        h.image().unwrap().save(out.join(name)).unwrap();
    };
    settle(&mut h, &mut ed, 2);
    let (owner, parts) = fence(&mut ed, true);
    look_at_fence(&mut ed);
    ed.state.select(Some(owner));
    settle(&mut h, &mut ed, 4);
    save(&h, "owner_generated_section_warning.png");

    assert!(ed.state.inspect_part(parts[1]));
    settle(&mut h, &mut ed, 4);
    save(&h, "part_inspected_read_only.png");

    // Place on Surface: the sphere selected, the cursor over the cube's
    // top, the ghost and the ring shown.
    let sphere = ed.state.add_entity(NewEntity::Sphere, None).unwrap();
    assert!(
        ed.state
            .set_field(sphere, "Transform", &Transform::from_xyz(1.5, 0.5, 1.8))
    );
    ed.state.select(Some(sphere));
    assert!(ed.state.activate_tool("place_on_surface"));
    let orbit = &mut ed.state.camera.orbit;
    orbit.target = Vec3::new(0.2, 0.6, 0.6);
    orbit.distance = 6.5;
    orbit.yaw = 0.55;
    orbit.pitch = 0.45;
    settle(&mut h, &mut ed, 2);
    let at = screen(&ed, Vec3::new(0.1, 1.0, 0.15));
    h.frame(&mut ed, vec![Event::PointerMoved(at)]);
    settle(&mut h, &mut ed, 3);
    save(&h, "place_on_surface_hover.png");
    eprintln!("captures in {}", out.display());
    let _ = std::fs::remove_dir_all(dir);
}
