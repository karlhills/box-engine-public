//! Fast iteration and safe experimentation (docs/DX_REVIEW.md, Batch 2):
//! reset, copy and paste in the inspector (E13), hide and lock (E17),
//! linked materials on primitives (E11), undo for material file edits
//! (E12) and keeping Play changes (A14).
//!
//! Headless and driven by egui events; widgets are found by their text in
//! egui's accessibility tree. Tests that render need a GPU and are skipped
//! without one (or with `BOX_SKIP_GPU_TESTS`); the last writes captures to
//! `target/captures/iteration_*.png`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use box_core::asset_ref::AssetId;
use box_core::assets::{AssetLookup, Assets};
use box_core::identity::{Name, Uid};
use box_core::material::StandardMaterial;
use box_core::mesh_asset::MaterialRef;
use box_core::prelude::{Entity, Transform};
use box_editor::assets::AssetItem;
use box_editor::headless::{CaptureOptions, Headless};
use box_editor::layout::Pane;
use box_editor::log::LogBuffer;
use box_editor::{Editor, EditorOptions, NewEntity, SelectMode, build_editor};
use box_render::{MaterialHandle, MeshRenderer, RenderError};
use egui::accesskit::{Role, TreeUpdate};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use glam::{Vec2, Vec3};

const IDLE: Duration = Duration::from_secs(30);

// ----- an egui driver that reads the accessibility tree -----

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
            let text = n.label().or(n.value()).unwrap_or_default().to_string();
            Some(Node {
                role: n.role(),
                text,
                rect: Rect::from_min_max(
                    pos2(b.x0 as f32, b.y0 as f32),
                    pos2(b.x1 as f32, b.y1 as f32),
                ),
            })
        })
        .collect()
}

/// Drives the editor's UI without rendering.
struct Driver {
    ctx: egui::Context,
    time: f64,
    nodes: Vec<Node>,
    copied: Option<String>,
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
            copied: None,
        }
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
        let out = self.ctx.run_ui(raw, |ui| editor.ui(ui));
        self.nodes = nodes_of(out.platform_output.accesskit_update.as_ref());
        for c in &out.platform_output.commands {
            if let egui::OutputCommand::CopyText(t) = c {
                self.copied = Some(t.clone());
            }
        }
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

    fn press(&mut self, editor: &mut Editor, pos: Pos2, button: PointerButton) {
        let b = |pressed| Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: Modifiers::NONE,
        };
        self.frame(editor, vec![Event::PointerMoved(pos)]);
        self.frame(editor, vec![b(true)]);
        self.frame(editor, vec![b(false)]);
        self.frame(editor, Vec::new());
    }

    fn click(&mut self, editor: &mut Editor, text: &str) {
        let pos = self.get(text).rect.center();
        self.press(editor, pos, PointerButton::Primary);
        self.frames(editor, 2);
    }

    /// Right-clicks the widget labelled `text` (a field label opens its menu).
    fn context(&mut self, editor: &mut Editor, text: &str) {
        let pos = self.get(text).rect.center();
        self.press(editor, pos, PointerButton::Secondary);
        self.frames(editor, 1);
    }

    fn key(&mut self, editor: &mut Editor, key: Key, modifiers: Modifiers) {
        let e = |pressed| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        };
        self.frame(editor, vec![Event::ModifiersChanged(modifiers), e(true)]);
        self.frame(
            editor,
            vec![e(false), Event::ModifiersChanged(Modifiers::NONE)],
        );
        self.frame(editor, Vec::new());
    }
}

// ----- projects -----

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../box_assets/tests/fixtures")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            copy_dir(&p, &to.join(e.file_name()));
        } else {
            std::fs::copy(&p, to.join(e.file_name())).unwrap();
        }
    }
}

/// A new project (the starter scene: Ground, Cube, Sun, Camera) with the
/// fixture materials and textures, in the default layout.
fn project(name: &str) -> (Editor, PathBuf) {
    let dir = std::env::temp_dir().join(format!("box_iter_{name}_{}", Uid::new()));
    let _ = std::fs::remove_dir_all(&dir);
    box_engine::Project::create(&dir, name).unwrap();
    for sub in ["materials", "textures"] {
        copy_dir(&fixtures().join(sub), &dir.join("assets").join(sub));
    }
    let options = EditorOptions {
        project: Some(dir.clone()),
        ..Default::default()
    };
    let mut editor = build_editor(&options, LogBuffer::default()).unwrap();
    editor.tree = box_editor::layout::default_layout();
    (editor, dir)
}

fn by_name(editor: &Editor, name: &str) -> Uid {
    editor
        .state
        .world
        .query::<(&Uid, &Name)>()
        .iter()
        .find(|(_, n)| n.0 == name)
        .map(|(u, _)| *u)
        .unwrap_or_else(|| panic!("no entity {name}"))
}

fn entity(editor: &Editor, uid: Uid) -> Entity {
    editor.state.world.entity_by_uid(uid).unwrap()
}

fn material(editor: &Editor, uid: Uid) -> StandardMaterial {
    editor
        .state
        .world
        .get::<StandardMaterial>(entity(editor, uid))
        .map(|m| StandardMaterial::clone(&m))
        .unwrap()
}

fn translation(editor: &Editor, uid: Uid) -> Vec3 {
    editor
        .state
        .world
        .get::<Transform>(entity(editor, uid))
        .unwrap()
        .translation
}

fn load(editor: &mut Editor, path: &str) -> AssetId {
    let id = editor.state.asset_server().unwrap().id_of(path).unwrap();
    editor.state.request_file(id);
    assert!(editor.state.wait_assets_idle(IDLE));
    id
}

fn pane(editor: &Editor, pane: Pane) -> Rect {
    let id = editor.tree.tiles.find_pane(&pane).unwrap();
    editor.tree.tiles.rect(id).unwrap()
}

fn gpu_skipped(name: &str) -> bool {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping {name}");
        return true;
    }
    false
}

fn headless(editor: &mut Editor, size: [u32; 2]) -> Option<Headless> {
    let options = CaptureOptions {
        size,
        pixels_per_point: 1.0,
        frames: 1,
    };
    match Headless::new(editor, &options) {
        Ok(h) => Some(h),
        Err(e)
            if matches!(
                e.downcast_ref::<RenderError>(),
                Some(RenderError::NoAdapter)
            ) =>
        {
            eprintln!("no GPU adapter; skipping");
            None
        }
        Err(e) => panic!("{e:#}"),
    }
}

// ----- E13: reset, copy and paste -----

#[test]
fn fields_reset_copy_and_paste_from_their_context_menu() {
    let (mut ed, dir) = project("field_menu");
    let cube = by_name(&ed, "Cube");
    let other = ed.state.add_entity(NewEntity::Cube, None).unwrap();
    ed.state.select(Some(cube));
    let mut d = Driver::new(&ed);
    d.frames(&mut ed, 3);

    // The cube's roughness (0.6) is not the default (0.5): Reset to Default.
    let steps = ed.state.edits.undo_len();
    d.context(&mut ed, "Roughness");
    d.click(&mut ed, "Reset to Default");
    assert_eq!(material(&ed, cube).roughness, 0.5);
    assert_eq!(ed.state.edits.undo_len(), steps + 1, "one undo step");
    assert_eq!(ed.state.edits.undo_label(), Some("Cube: Reset Roughness"));
    ed.state.undo();
    assert_eq!(material(&ed, cube).roughness, 0.6);

    // Copy Value writes JSON through egui's clipboard; Paste Value on the
    // other cube takes it, one undo step.
    d.frames(&mut ed, 2);
    d.context(&mut ed, "Roughness");
    d.click(&mut ed, "Copy Value");
    let copied = d.copied.clone().expect("copied through egui");
    assert!(
        copied.contains("\"box\":\"value\"") && copied.contains("f32"),
        "{copied}"
    );
    ed.state.select(Some(other));
    d.frames(&mut ed, 2);
    let steps = ed.state.edits.undo_len();
    d.context(&mut ed, "Roughness");
    d.click(&mut ed, "Paste Value (f32)");
    assert_eq!(material(&ed, other).roughness, 0.6);
    assert_eq!(ed.state.edits.undo_len(), steps + 1);
    assert_eq!(ed.state.edits.undo_label(), Some("Cube 2: Paste Roughness"));

    // A value of another type is refused: no paste, no step.
    let steps = ed.state.edits.undo_len();
    assert!(!ed.state.paste_field(&[other], "Transform.translation"));
    assert_eq!(ed.state.edits.undo_len(), steps);
    d.context(&mut ed, "Translation (m)");
    assert!(d.find("Paste Value (f32)").is_none());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn components_reset_copy_and_paste_across_a_multi_selection() {
    let (mut ed, dir) = project("component_menu");
    let cube = by_name(&ed, "Cube");
    let ground = by_name(&ed, "Ground");
    let other = ed.state.add_entity(NewEntity::Sphere, None).unwrap();
    let empty = ed.state.add_entity(NewEntity::Empty, None).unwrap();

    // Copy the cube's material; paste its values over two others at once.
    ed.state
        .copy_component(&[cube], "StandardMaterial")
        .unwrap();
    let steps = ed.state.edits.undo_len();
    assert!(
        ed.state
            .paste_component_values(&[ground, other], "StandardMaterial")
    );
    assert_eq!(ed.state.edits.undo_len(), steps + 1, "one step for both");
    assert_eq!(material(&ed, ground), material(&ed, cube));
    assert_eq!(material(&ed, other), material(&ed, cube));
    // Not into another component.
    assert!(!ed.state.paste_component_values(&[ground], "Transform"));
    assert_eq!(ed.state.edits.undo_len(), steps + 1);

    // Paste as New Component where it is missing (the empty), values kept.
    assert_eq!(
        ed.state.pastable_new_component(&[cube, empty]),
        Some("StandardMaterial")
    );
    assert!(ed.state.pastable_new_component(&[cube]).is_none());
    assert!(ed.state.paste_as_new_component(&[cube, empty]));
    assert_eq!(material(&ed, empty), material(&ed, cube));
    assert_eq!(ed.state.edits.undo_len(), steps + 2);

    // Reset Component on several entities: one step, all defaults; undo
    // brings each one's values back.
    let before = (material(&ed, cube), material(&ed, ground));
    assert!(ed.state.reset_field(&[cube, ground], "StandardMaterial"));
    assert_eq!(ed.state.edits.undo_len(), steps + 3);
    assert_eq!(
        ed.state.edits.undo_label(),
        Some("2 entities: Reset Standard Material")
    );
    assert_eq!(material(&ed, cube), StandardMaterial::default());
    assert_eq!(material(&ed, ground), StandardMaterial::default());
    ed.state.undo();
    assert_eq!((material(&ed, cube), material(&ed, ground)), before);

    // A field reset over a multi-selection from the field's menu: one step.
    ed.state.select_with(&[ground, cube], SelectMode::Replace);
    let mut d = Driver::new(&ed);
    d.frames(&mut ed, 3);
    let steps = ed.state.edits.undo_len();
    d.context(&mut ed, "Translation (m)");
    d.click(&mut ed, "Reset to Default");
    assert_eq!(translation(&ed, cube), Vec3::ZERO);
    assert_eq!(translation(&ed, ground), Vec3::ZERO);
    assert_eq!(ed.state.edits.undo_len(), steps + 1);
    assert_eq!(
        ed.state.edits.undo_label(),
        Some("2 entities: Reset Translation")
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn non_default_values_are_marked() {
    let (ed, dir) = project("markers");
    let cube = entity(&ed, by_name(&ed, "Cube"));
    let registry = ed.state.world.registry().clone();
    let sections = box_editor::inspect::entity_sections(&ed.state.world, cube);
    let modified: Vec<&str> = sections
        .iter()
        .flat_map(|s| &s.rows)
        .filter(|r| r.kind != box_editor::inspect::FieldKind::Struct)
        .filter(|r| {
            box_editor::values::differs_from_default(&registry, &r.path, r.value.as_ref())
                == Some(true)
        })
        .map(|r| r.path.as_str())
        .collect();
    assert_eq!(
        modified,
        [
            "Transform.translation",
            "StandardMaterial.base_color",
            "StandardMaterial.roughness"
        ]
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

// ----- E17: hide and lock -----

#[test]
fn hidden_entities_are_not_drawn_except_in_play() {
    if gpu_skipped("hide") {
        return;
    }
    let (mut ed, dir) = project("hide");
    let cube = by_name(&ed, "Cube");
    let Some(mut h) = headless(&mut ed, [1280, 800]) else {
        return;
    };
    fn objects(ed: &mut Editor, h: &mut Headless) -> u32 {
        h.frame(ed, Vec::new());
        h.frame(ed, Vec::new());
        h.renderer().stats().objects
    }
    let all = objects(&mut ed, &mut h);
    assert!(all >= 2, "ground and cube");

    // H hides the selection: one object fewer.
    ed.state.select(Some(cube));
    let key = |pressed| Event::Key {
        key: Key::H,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: Modifiers::NONE,
    };
    h.frame(&mut ed, vec![key(true), key(false)]);
    assert!(ed.state.is_hidden(cube));
    assert_eq!(objects(&mut ed, &mut h), all - 1);
    // The marker is runtime data: not saved, not in the scene file.
    let saved = box_core::scene::save(&ed.state.world).unwrap();
    assert!(!String::from_utf8_lossy(&saved).contains("Hidden"));

    // Play draws everything (the play world is built from the scene).
    ed.state.start_play().unwrap();
    assert_eq!(objects(&mut ed, &mut h), all);
    ed.state.stop_play();
    assert_eq!(objects(&mut ed, &mut h), all - 1, "hidden again after Stop");

    // Alt+H shows everything again.
    let alt = |pressed| Event::Key {
        key: Key::H,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: Modifiers::ALT,
    };
    h.frame(&mut ed, vec![alt(true), alt(false)]);
    assert!(!ed.state.is_hidden(cube));
    assert_eq!(objects(&mut ed, &mut h), all);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn locked_entities_are_not_picked_or_moved_in_the_viewport() {
    let (mut ed, dir) = project("lock");
    let cube = by_name(&ed, "Cube");
    let ground = by_name(&ed, "Ground");
    let mut d = Driver::new(&ed);
    d.frames(&mut ed, 3);
    box_core::transform::propagate_transforms(&mut ed.state.world);
    box_engine::sync_scene_meshes(&mut ed.state.world);
    let b = box_editor::entity_bounds(&ed.state.world, entity(&ed, cube)).unwrap();
    ed.state.camera.frame(&b);
    d.frames(&mut ed, 2);
    let rect = ed.state.viewport.rect.unwrap();
    let view = ed.state.gizmo_view().unwrap();
    let at = rect.min + {
        let p = view.project(b.center()).unwrap();
        vec2(p.x, p.y)
    };

    // A click on the cube selects it.
    d.press(&mut ed, at, PointerButton::Primary);
    assert_eq!(ed.state.primary(), Some(cube));
    ed.state.select(None);

    // Locked: the click goes through to the ground behind it.
    ed.state.toggle_locked(cube);
    d.press(&mut ed, at, PointerButton::Primary);
    assert_eq!(ed.state.primary(), Some(ground), "picked through the cube");

    // Still selectable in the hierarchy, but no gizmo and no drag.
    ed.state.select(Some(cube));
    let frame = ed.state.gizmo_frame(&view);
    assert!(frame.is_none(), "no gizmo on a locked entity");
    assert!(
        !ed.state
            .gizmo_begin(Vec2::new(at.x - rect.left(), at.y - rect.top()))
    );
    assert!(ed.state.gizmo_targets().is_empty());

    // A marquee over the whole viewport skips it.
    ed.state.marquee_select(
        Vec2::ZERO,
        Vec2::new(rect.width(), rect.height()),
        SelectMode::Replace,
    );
    assert!(!ed.state.selection.contains(&cube));

    // A child of a locked parent is locked too.
    let child = ed.state.add_entity(NewEntity::Sphere, Some(cube)).unwrap();
    assert!(ed.state.blocked_in_viewport(entity(&ed, child)));
    ed.state.toggle_locked(cube);
    assert!(!ed.state.blocked_in_viewport(entity(&ed, child)));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_hierarchy_eye_and_lock_toggle_on_click() {
    let (mut ed, dir) = project("eye");
    let cube = by_name(&ed, "Cube");
    let mut d = Driver::new(&ed);
    d.frames(&mut ed, 3);
    let h = pane(&ed, Pane::Hierarchy);
    let row_h = box_editor::theme::DARK.row_h;
    let index = ed
        .state
        .hierarchy
        .rows
        .iter()
        .position(|r| r.uid == cube)
        .unwrap();
    // Below the search bar (34) and the Scene row.
    let y = h.top() + 34.0 + row_h * (index as f32 + 1.5);
    let eye = pos2(h.right() - 30.0, y);
    let lock = pos2(h.right() - 11.0, y);
    // Shown on hover; a click toggles.
    d.frame(&mut ed, vec![Event::PointerMoved(eye)]);
    d.press(&mut ed, eye, PointerButton::Primary);
    assert!(ed.state.is_hidden(cube));
    assert!(ed.state.selection.is_empty(), "the eye does not select");
    d.press(&mut ed, lock, PointerButton::Primary);
    assert!(ed.state.is_locked(cube));
    d.press(&mut ed, eye, PointerButton::Primary);
    assert!(!ed.state.is_hidden(cube));
    // Not scene edits: nothing to undo, the scene stays clean.
    assert!(!ed.state.edits.can_undo());
    assert!(!ed.state.is_dirty());
    std::fs::remove_dir_all(&dir).unwrap();
}

// ----- E11: linked materials -----

/// The handle the asset server gave a material file in the edit world.
fn material_handle(ed: &Editor, id: AssetId) -> box_core::assets::Handle<StandardMaterial> {
    ed.state
        .edit_world()
        .resource::<AssetLookup>()
        .get::<StandardMaterial>(&box_core::asset_ref::AssetRef::id(id))
        .unwrap()
}

fn drawn(ed: &Editor, uid: Uid) -> Vec<MaterialHandle> {
    ed.state
        .world
        .get::<MeshRenderer>(entity(ed, uid))
        .map(|r| r.materials.clone())
        .unwrap()
}

fn settle(ed: &mut Editor) {
    for _ in 0..3 {
        assert!(ed.state.wait_assets_idle(IDLE));
        box_engine::sync_scene_meshes(&mut ed.state.world);
    }
}

#[test]
fn dropped_materials_are_linked_and_follow_the_file() {
    let (mut ed, dir) = project("link");
    let stone = load(&mut ed, "materials/stone.material.json");
    let cube = by_name(&ed, "Cube");
    let steps = ed.state.edits.undo_len();
    assert!(ed.state.assign_material(cube, &AssetItem::file(stone)));
    assert_eq!(ed.state.edits.undo_len(), steps + 1, "one undo step");
    assert!(
        ed.state
            .edits
            .undo_label()
            .is_some_and(|l| l.starts_with("Link stone.material.json")),
        "{:?}",
        ed.state.edits.undo_label()
    );
    settle(&mut ed);
    let h = material_handle(&ed, stone);
    assert_eq!(
        drawn(&ed, cube),
        [MaterialHandle::Standard(h)],
        "draws the file"
    );

    // A file edit (any editor) reloads it in place: the cube shows it.
    let path = dir.join("assets/materials/stone.material.json");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        text.replace("\"roughness\": 0.8", "\"roughness\": 0.15"),
    )
    .unwrap();
    ed.state.asset_server().unwrap().rescan();
    settle(&mut ed);
    assert_eq!(drawn(&ed, cube), [MaterialHandle::Standard(h)]);
    let roughness = ed
        .state
        .world
        .resource::<Assets<StandardMaterial>>()
        .get(h)
        .unwrap()
        .roughness;
    assert!((roughness - 0.15).abs() < 1e-6, "reloaded: {roughness}");

    // Edit shows the material asset in the inspector.
    let r = ed
        .state
        .world
        .get::<MaterialRef>(entity(&ed, cube))
        .unwrap()
        .material
        .clone();
    ed.state.edit_linked_material(&r);
    assert_eq!(
        ed.state.assets.inspect.as_ref().map(|i| i.item.clone()),
        Some(AssetItem::file(stone))
    );
    ed.state.select(Some(cube));

    // Unlink copies the values into the inline material: one undo step.
    let steps = ed.state.edits.undo_len();
    assert!(ed.state.unlink_material(&[cube]));
    assert_eq!(ed.state.edits.undo_len(), steps + 1);
    assert!(!ed.state.world.has::<MaterialRef>(entity(&ed, cube)));
    let m = material(&ed, cube);
    assert!((m.roughness - 0.15).abs() < 1e-6 && m.double_sided);
    settle(&mut ed);
    assert_ne!(
        drawn(&ed, cube),
        [MaterialHandle::Standard(h)],
        "its own again"
    );
    ed.state.undo();
    assert!(ed.state.world.has::<MaterialRef>(entity(&ed, cube)));
    assert_eq!(material(&ed, cube).roughness, 0.6, "inline values back");

    // Saved as a reference with the asset's id.
    let saved = box_core::scene::save(&ed.state.world).unwrap();
    let text = String::from_utf8(saved).unwrap();
    assert!(text.contains("MaterialRef") && text.contains(&stone.to_string()));
    std::fs::remove_dir_all(&dir).unwrap();
}

// ----- E12: undo for material file edits -----

#[test]
fn material_file_edits_have_their_own_undo() {
    let (mut ed, dir) = project("asset_undo");
    let stone = load(&mut ed, "materials/stone.material.json");
    let path = dir.join("assets/materials/stone.material.json");
    let original = std::fs::read(&path).unwrap();
    // A scene step first, which must survive.
    let cube = by_name(&ed, "Cube");
    assert!(ed.state.set_field_step(
        &[cube],
        "Transform.translation",
        &Vec3::new(1.0, 0.5, 0.0),
        "Move"
    ));
    let scene_steps = ed.state.edits.undo_len();

    ed.state.select_asset(Some(AssetItem::file(stone)));
    assert!(ed.state.asset_undo_active());
    ed.state
        .edit_material_labelled(1.0, Some("Roughness".into()), |m| m.roughness = 0.2);
    ed.state.flush_material_edit(2.0, true);
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(written.contains("\"roughness\": 0.2"), "{written}");
    assert_eq!(ed.state.asset_undo_len(stone), 1);
    assert_eq!(
        ed.state.undo_menu_item(false),
        ("Undo stone.material.json: Roughness".to_string(), true)
    );

    // Cmd+Z with the material shown: the file goes back, the scene stays.
    let mut d = Driver::new(&ed);
    d.frames(&mut ed, 2);
    d.key(&mut ed, Key::Z, Modifiers::COMMAND);
    assert_eq!(std::fs::read(&path).unwrap(), original, "byte for byte");
    assert_eq!(
        ed.state.edits.undo_len(),
        scene_steps,
        "scene undo untouched"
    );
    assert_eq!(translation(&ed, cube), Vec3::new(1.0, 0.5, 0.0));
    assert!(ed.state.wait_assets_idle(IDLE));
    let m = ed
        .state
        .assets
        .inspect
        .as_ref()
        .unwrap()
        .material
        .as_ref()
        .unwrap();
    assert_eq!(m.value.roughness, 0.8, "the inspector shows the file again");
    assert_eq!(
        ed.state.undo_menu_item(true).0,
        "Redo stone.material.json: Roughness"
    );
    ed.state.redo();
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("\"roughness\": 0.2")
    );

    // With an entity in the inspector, undo is the scene's again.
    ed.state.select(Some(cube));
    assert!(!ed.state.asset_undo_active());
    assert!(ed.state.undo_menu_item(false).0.starts_with("Undo Cube"));
    ed.state.undo();
    assert_eq!(translation(&ed, cube), Vec3::new(0.0, 0.5, 0.0));
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("\"roughness\": 0.2")
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

// ----- A14: keep Play changes -----

#[test]
fn play_changes_marked_keep_are_applied_on_stop_as_one_step() {
    let (mut ed, dir) = project("keep");
    let cube = by_name(&ed, "Cube");
    let steps = ed.state.edits.undo_len();
    ed.state.select(Some(cube));
    ed.state.start_play().unwrap();

    // Tune during Play; mark the translation (from its field menu) and the
    // material (the section's ⋯ menu path) to keep.
    ed.state
        .set_field(cube, "Transform.translation", &Vec3::new(2.0, 0.5, 0.0));
    ed.state
        .set_field(cube, "StandardMaterial.roughness", &0.25f32);
    ed.state
        .set_field(cube, "Transform.scale", &Vec3::splat(3.0));
    ed.state.finish_edit();
    let mut d = Driver::new(&ed);
    d.frames(&mut ed, 3);
    d.context(&mut ed, "Translation (m)");
    d.click(&mut ed, "Keep after Stop");
    assert!(ed.state.is_kept(cube, "Transform.translation"));
    assert_eq!(ed.state.keep_after_stop(&[cube], "StandardMaterial"), 1);
    assert!(
        ed.state.is_kept(cube, "StandardMaterial.roughness"),
        "inside a kept component"
    );
    assert_eq!(ed.state.kept_changes().len(), 2);

    // An entity the game made is refused.
    let runtime = Uid::new();
    ed.state
        .world
        .spawn_saved(runtime, "Spawned", (Transform::default(),))
        .unwrap();
    assert_eq!(ed.state.keep_after_stop(&[runtime], "Transform"), 0);
    assert_eq!(ed.state.kept_changes().len(), 2);

    // Stop: kept values land in the edit scene as one step; the scale
    // (not kept) is discarded.
    ed.state.stop_play();
    assert_eq!(translation(&ed, cube), Vec3::new(2.0, 0.5, 0.0));
    assert_eq!(material(&ed, cube).roughness, 0.25);
    let scale = ed
        .state
        .world
        .get::<Transform>(entity(&ed, cube))
        .unwrap()
        .scale;
    assert_eq!(scale, Vec3::ONE);
    assert_eq!(ed.state.edits.undo_len(), steps + 1);
    assert_eq!(ed.state.edits.undo_label(), Some("Apply Play changes"));
    assert!(ed.state.is_dirty());
    ed.state.undo();
    assert_eq!(translation(&ed, cube), Vec3::new(0.0, 0.5, 0.0));
    assert_eq!(material(&ed, cube).roughness, 0.6);
    assert!(ed.state.world.entity_by_uid(runtime).is_none());

    // Nothing kept: Stop changes nothing.
    ed.state.start_play().unwrap();
    ed.state
        .set_field(cube, "Transform.translation", &Vec3::new(5.0, 0.5, 0.0));
    ed.state.stop_play();
    assert_eq!(translation(&ed, cube), Vec3::new(0.0, 0.5, 0.0));
    std::fs::remove_dir_all(&dir).unwrap();
}

// ----- captures -----

/// Runs frames with `events` in the first, waiting for imports between
/// frames, and saves the last as `target/captures/<name>.png`.
fn shot(ed: &mut Editor, h: &mut Headless, events: Vec<Event>, name: &str) -> Vec<Node> {
    ed.state.wait_assets_idle(IDLE);
    h.frame(ed, events);
    // Popups fade in over a few frames.
    for _ in 0..12 {
        ed.state.wait_assets_idle(IDLE);
        h.frame(ed, Vec::new());
    }
    let image = h.image().unwrap();
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/captures");
    std::fs::create_dir_all(&out).unwrap();
    image.save(out.join(format!("{name}.png"))).unwrap();
    nodes_of(h.accesskit_update())
}

fn node(nodes: &[Node], text: &str) -> Node {
    nodes
        .iter()
        .rev()
        .find(|n| n.text == text && n.role != Role::TextInput)
        .cloned()
        .unwrap_or_else(|| panic!("no widget `{text}`"))
}

fn right_click(at: Pos2) -> Vec<Event> {
    let b = |pressed| Event::PointerButton {
        pos: at,
        button: PointerButton::Secondary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    vec![Event::PointerMoved(at), b(true), b(false)]
}

#[test]
fn capture_menus_markers_hide_lock_links_and_kept_changes() {
    if gpu_skipped("iteration captures") {
        return;
    }
    let (mut ed, dir) = project("captures");
    let stone = load(&mut ed, "materials/stone.material.json");
    let cube = by_name(&ed, "Cube");
    let ground = by_name(&ed, "Ground");
    let size = [1600, 1000];
    let Some(mut h) = headless(&mut ed, size) else {
        return;
    };
    h.enable_accesskit();
    ed.state.select(Some(cube));
    // Something on the clipboard, so Paste Value is enabled.
    ed.state
        .copy_field(&[cube], "StandardMaterial.metallic")
        .unwrap();
    let nodes = shot(&mut ed, &mut h, Vec::new(), "iteration_markers_setup");

    // 1. Non-default markers, with the ↺ of the hovered Roughness row.
    let rough = node(&nodes, "Roughness").rect.center();
    shot(
        &mut ed,
        &mut h,
        vec![Event::PointerMoved(rough)],
        "iteration_markers",
    );

    // 2. The field context menu: Reset to Default, Copy Value, Paste Value.
    let nodes = shot(&mut ed, &mut h, right_click(rough), "iteration_field_menu");
    for item in ["Reset to Default", "Copy Value", "Paste Value (f32)"] {
        node(&nodes, item);
    }
    h.frame(
        &mut ed,
        vec![Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }],
    );

    // 3. The hierarchy: Ground hidden and Cube locked, Sun row hovered.
    ed.state.set_hidden(&[ground], true);
    ed.state.set_locked(&[cube], true);
    let hier = pane(&ed, Pane::Hierarchy);
    let sun_row = ed
        .state
        .hierarchy
        .rows
        .iter()
        .position(|r| r.uid == by_name(&ed, "Sun"))
        .unwrap();
    let row_h = box_editor::theme::DARK.row_h;
    let over = pos2(
        hier.center().x,
        hier.top() + 34.0 + row_h * (sun_row as f32 + 1.5),
    );
    shot(
        &mut ed,
        &mut h,
        vec![Event::PointerMoved(over)],
        "iteration_hide_lock",
    );
    ed.state.unhide_all();
    ed.state.set_locked(&[cube], false);

    // 4. A linked material: the Material Ref section with Edit and Unlink.
    assert!(ed.state.assign_material(cube, &AssetItem::file(stone)));
    let nodes = shot(
        &mut ed,
        &mut h,
        vec![Event::PointerMoved(Pos2::ZERO)],
        "iteration_material_link",
    );
    node(&nodes, "Unlink");
    node(&nodes, "Edit");

    // 5. Play with two changes kept: the banner count and the row marks.
    ed.state.start_play().unwrap();
    ed.state.set_play_paused(true);
    ed.state.keep_after_stop(&[cube], "Transform.translation");
    ed.state.keep_after_stop(&[cube], "Transform.scale");
    assert_eq!(ed.state.kept_changes().len(), 2);
    shot(&mut ed, &mut h, Vec::new(), "iteration_play_keep");
    ed.state.stop_play();
    std::fs::remove_dir_all(&dir).unwrap();
}
