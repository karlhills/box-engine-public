//! Editor usability fixes from Milestone 6: nested structs in the inspector
//! are folding headings (closed by default, remembered per type and field
//! path, an `enabled` switch in the heading, whole-value menus, mixed
//! values), Add Component is pinned in view, list items are named by the
//! singular of their field, check boxes have a visible box everywhere (the
//! rule list), and a selected owner with many generated meshes shows one
//! bounds box instead of an outline per mesh.
//!
//! Headless and driven by egui events; widgets are found in egui's
//! accessibility tree. The last test writes captures to
//! `target/captures/m6_ux/` and needs a GPU.

use std::path::{Path, PathBuf};

use box_core::derived::PartKey;
use box_core::identity::{Name, Uid};
use box_core::material::{StandardMaterial, WindSway};
use box_core::prelude::{Entity, Primitive, Shape, Transform};
use box_editor::layout::Pane;
use box_editor::log::LogBuffer;
use box_editor::panes::inspector::fold_open;
use box_editor::{Editor, EditorOptions, GENERATED_OUTLINE_LIMIT, NewEntity, build_editor};
use box_engine::box_proc::derive::{Part, reconcile_with};
use box_engine::box_proc::rule::{RuleOutput, RuleRegistry};
use box_render::{AerialHaze, Environment, SelectionOutline};
use egui::accesskit::{Role, TreeUpdate};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use glam::Vec3;

// ----- an egui driver that reads the accessibility tree -----

#[derive(Clone, Debug)]
struct Node {
    role: Role,
    text: String,
    toggled: Option<bool>,
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
                toggled: n.toggled().map(|t| t == egui::accesskit::Toggled::True),
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
    screen: egui::Vec2,
}

impl Driver {
    fn new(editor: &Editor, screen: egui::Vec2) -> Self {
        let ctx = egui::Context::default();
        editor.apply_theme(&ctx);
        ctx.enable_accesskit();
        Self {
            ctx,
            time: 0.0,
            nodes: Vec::new(),
            screen,
        }
    }

    fn frame(&mut self, editor: &mut Editor, events: Vec<Event>) {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.screen)),
            time: Some(self.time),
            predicted_dt: 1.0 / 60.0,
            focused: true,
            events,
            ..Default::default()
        };
        let out = self.ctx.run_ui(raw, |ui| editor.ui(ui));
        self.nodes = nodes_of(out.platform_output.accesskit_update.as_ref());
        editor.prepare_render();
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

    fn has(&self, text: &str) -> bool {
        self.find(text).is_some()
    }

    /// The check box on the same row as the label `text`.
    fn checkbox_beside(&self, text: &str) -> Node {
        let row = self.get(text).rect;
        self.nodes
            .iter()
            .find(|n| {
                n.role == Role::CheckBox
                    && n.rect.center().y > row.top()
                    && n.rect.center().y < row.bottom()
            })
            .cloned()
            .unwrap_or_else(|| panic!("no check box beside `{text}`"))
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
        self.frames(editor, 2);
    }

    fn click(&mut self, editor: &mut Editor, text: &str) {
        let pos = self.get(text).rect.center();
        self.press(editor, pos, PointerButton::Primary);
    }

    fn right_click(&mut self, editor: &mut Editor, text: &str) {
        let pos = self.get(text).rect.center();
        self.press(editor, pos, PointerButton::Secondary);
    }

    fn key(&mut self, editor: &mut Editor, key: Key) {
        let e = |pressed| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        self.frame(editor, vec![e(true)]);
        self.frame(editor, vec![e(false)]);
        self.frame(editor, Vec::new());
    }
}

// ----- projects -----

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_project(name: &str) -> TempDir {
    let dir = std::env::temp_dir().join(format!("box_m6ux_{name}_{}", Uid::new()));
    let _ = std::fs::remove_dir_all(&dir);
    box_engine::Project::create(&dir, name).unwrap();
    TempDir(dir)
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

/// The default window size (`box-editor` opens at 1440 × 900 points).
const DEFAULT: egui::Vec2 = vec2(1440.0, 900.0);

fn editor(name: &str, screen: egui::Vec2) -> (TempDir, Editor, Driver) {
    let dir = temp_project(name);
    let mut ed = open(&dir.0);
    let mut d = Driver::new(&ed, screen);
    d.frames(&mut ed, 3);
    (dir, ed, d)
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

fn wind(editor: &Editor, uid: Uid) -> WindSway {
    let e = entity(editor, uid);
    editor.state.world.get::<StandardMaterial>(e).unwrap().wind
}

fn pane(editor: &Editor, pane: Pane) -> Rect {
    let id = editor.tree.tiles.find_pane(&pane).unwrap();
    editor.tree.tiles.rect(id).unwrap()
}

// ----- nested structs fold -----

#[test]
fn nested_structs_are_closed_and_remembered_per_field_path() {
    let (_dir, mut ed, mut d) = editor("fold", DEFAULT);
    let cube = by_name(&ed, "Cube");
    ed.state.select(Some(cube));
    d.frames(&mut ed, 2);
    // Closed by default: the heading, not its fields.
    assert!(d.has("Wind"));
    assert!(!d.has("Strength") && !d.has("Frequency"));
    assert!(!fold_open(&d.ctx, "StandardMaterial.wind"));

    // A click on the heading opens it; the choice is the field path's.
    d.click(&mut ed, "Wind");
    assert!(d.has("Strength") && d.has("Frequency"));
    assert!(fold_open(&d.ctx, "StandardMaterial.wind"));
    // Another entity with a Standard Material (a copy of the cube; the
    // ground's longer inspector puts Wind below the window): open too.
    ed.state.duplicate_selection();
    let edits = ed.state.edits.undo_len();
    d.frames(&mut ed, 2);
    assert!(d.has("Strength"));
    // Closing it closes it everywhere.
    d.click(&mut ed, "Wind");
    assert!(!d.has("Strength"));
    ed.state.select(Some(cube));
    d.frames(&mut ed, 2);
    assert!(!d.has("Strength"));
    // Opening and closing is not an edit (the copy is the only one).
    assert_eq!(ed.state.edits.undo_len(), edits);
}

#[test]
fn the_enabled_switch_sits_in_the_heading() {
    let (_dir, mut ed, mut d) = editor("switch", DEFAULT);
    ed.state.show_scene_settings();
    d.frames(&mut ed, 2);
    let haze = |ed: &Editor| ed.state.world.resource::<Environment>().aerial_haze.enabled;
    assert!(!haze(&ed));
    let check = d.checkbox_beside("Aerial haze");
    assert_eq!(check.toggled, Some(false));
    // Switching it on does not open the heading; one undo step.
    d.press(&mut ed, check.rect.center(), PointerButton::Primary);
    assert!(haze(&ed));
    assert_eq!(ed.state.edits.undo_len(), 1);
    assert!(!fold_open(&d.ctx, "Environment.aerial_haze"));
    assert_eq!(d.checkbox_beside("Aerial haze").toggled, Some(true));
    // Open: the fields, without a second "Enabled" row.
    d.click(&mut ed, "Aerial haze");
    assert!(fold_open(&d.ctx, "Environment.aerial_haze"));
    assert!(d.has("Color"));
    assert!(!d.has("Enabled"), "the switch is in the heading only");
    ed.state.undo_scene();
    d.frames(&mut ed, 2);
    assert!(!haze(&ed));
    assert_eq!(
        ed.state.world.resource::<Environment>().aerial_haze,
        AerialHaze::default()
    );
}

#[test]
fn heading_menus_reset_copy_and_paste_the_whole_struct() {
    let (_dir, mut ed, mut d) = editor("menus", DEFAULT);
    let cube = by_name(&ed, "Cube");
    // The other entity: a copy of the cube (the ground's longer inspector
    // puts Wind below the window).
    ed.state.select(Some(cube));
    ed.state.duplicate_selection();
    let ground = ed.state.selection[0];
    ed.state.select(Some(cube));
    let swaying = WindSway {
        strength: 0.3,
        frequency: 2.0,
        ..WindSway::default()
    };
    assert!(ed.state.set_field(cube, "StandardMaterial.wind", &swaying));
    let base = ed.state.edits.undo_len();
    ed.state.select(Some(cube));
    d.frames(&mut ed, 2);
    // Copy on the closed heading, paste on another entity's.
    d.right_click(&mut ed, "Wind");
    d.click(&mut ed, "Copy Value");
    ed.state.select(Some(ground));
    d.frames(&mut ed, 2);
    d.right_click(&mut ed, "Wind");
    let paste = d
        .nodes
        .iter()
        .find(|n| n.text.starts_with("Paste Value"))
        .cloned()
        .expect("Paste Value");
    assert_eq!(paste.text, "Paste Value (WindSway)");
    d.press(&mut ed, paste.rect.center(), PointerButton::Primary);
    assert_eq!(wind(&ed, ground), swaying);
    assert_eq!(ed.state.edits.undo_len(), base + 1);
    // Reset the whole struct from the heading: one undo step.
    d.right_click(&mut ed, "Wind");
    d.click(&mut ed, "Reset to Default");
    assert_eq!(wind(&ed, ground), WindSway::default());
    assert_eq!(ed.state.edits.undo_len(), base + 2);
    assert_eq!(wind(&ed, cube), swaying, "only the inspected entity");
}

#[test]
fn a_multi_selection_shows_mixed_struct_fields_and_resets_all() {
    let (_dir, mut ed, mut d) = editor("multi", DEFAULT);
    let (cube, ground) = (by_name(&ed, "Cube"), by_name(&ed, "Ground"));
    let a = WindSway {
        strength: 0.3,
        ..WindSway::default()
    };
    assert!(ed.state.set_field(cube, "StandardMaterial.wind", &a));
    ed.state.selection = vec![ground, cube];
    d.frames(&mut ed, 2);
    // Closed, the heading says its fields differ.
    let row = d.get("Wind").rect;
    assert!(
        d.nodes.iter().any(|n| n.text == "—"
            && n.rect.center().y > row.top()
            && n.rect.center().y < row.bottom()),
        "a mixed mark on the heading"
    );
    // Open, the differing field shows "—".
    d.click(&mut ed, "Wind");
    assert!(d.has("Strength"));
    let base = ed.state.edits.undo_len();
    d.right_click(&mut ed, "Wind");
    d.click(&mut ed, "Reset to Default");
    assert_eq!(wind(&ed, cube), WindSway::default());
    assert_eq!(wind(&ed, ground), WindSway::default());
    assert_eq!(ed.state.edits.undo_len(), base + 1, "one step for both");
}

// ----- Add Component stays in view -----

#[test]
fn add_component_is_pinned_below_the_sections() {
    // A short window and a long inspector.
    let (_dir, mut ed, mut d) = editor("pinned", vec2(1280.0, 720.0));
    let cube = by_name(&ed, "Cube");
    let addable = box_editor::inspect::addable_components(&ed.state.world, entity(&ed, cube));
    for name in addable.iter().take(6) {
        ed.state.add_component(cube, name);
    }
    ed.state.select(Some(cube));
    d.frames(&mut ed, 3);
    let inspector = pane(&ed, Pane::Inspector);
    let button = d.get("Add Component").rect;
    assert!(
        inspector.contains_rect(button) && button.bottom() <= 720.0,
        "Add Component {button:?} in the inspector {inspector:?}"
    );
    // The sections scroll above it: the last one is below the window.
    let last = d
        .nodes
        .iter()
        .filter(|n| n.role == Role::Label && n.rect.left() > inspector.left())
        .map(|n| n.rect.bottom())
        .fold(0.0f32, f32::max);
    assert!(
        last > button.top(),
        "the inspector is longer than the window"
    );
    // It opens its list above itself, with the search focused.
    d.click(&mut ed, "Add Component");
    let search = d
        .nodes
        .iter()
        .find(|n| n.role == Role::TextInput)
        .cloned()
        .expect("the search field");
    assert!(search.rect.bottom() <= button.top() + 1.0, "{search:?}");
    d.frame(&mut ed, vec![Event::Text("point".into())]);
    d.key(&mut ed, Key::Enter);
    assert!(
        ed.state
            .world
            .has::<box_render::PointLight>(entity(&ed, cube))
    );
}

// ----- list item labels -----

#[test]
fn list_items_are_named_by_the_singular() {
    use box_editor::inspect::singular;
    for (plural, one) in [
        ("points", "point"),
        ("species", "species"),
        ("entries", "entry"),
        ("materials", "material"),
        ("edge_slots", "edge_slot"),
        ("vertices", "vertex"),
        ("indices", "index"),
        ("boxes", "box"),
        ("meshes", "mesh"),
        ("patches", "patch"),
        ("classes", "class"),
        ("caches", "cache"),
        ("holes", "hole"),
        ("uvs", "uv"),
        ("series", "series"),
        ("status", "status"),
        ("area", "area"),
        ("s", "s"),
    ] {
        assert_eq!(singular(plural), one, "{plural}");
    }
    // Every list field of the engine's reflected types: a word, never a
    // clipped one ("specie").
    use box_core::reflect::bevy_reflect::TypeInfo;
    let registry = box_engine::shared_engine_registry();
    let mut lists = Vec::new();
    for r in registry.type_registry().iter() {
        if let TypeInfo::Struct(s) = r.type_info() {
            for f in s.iter() {
                if matches!(f.type_info(), Some(TypeInfo::List(_))) {
                    lists.push((s.type_path_table().short_path(), f.name()));
                }
            }
        }
    }
    assert!(lists.iter().any(|(_, f)| *f == "species"), "{lists:?}");
    for (ty, field) in &lists {
        let one = singular(field);
        assert!(
            !one.is_empty() && !one.ends_with("ie") && !one.ends_with('_'),
            "{ty}.{field} -> {one}"
        );
        eprintln!("{ty}.{field} -> {one}");
    }
}

#[test]
fn scatter_species_items_say_species() {
    use box_engine::box_proc::scatter::{Scatter, ScatterSpecies};
    let (_dir, mut ed, mut d) = editor("species", DEFAULT);
    let uid = Uid::new();
    ed.state.world.spawn((
        uid,
        Name::new("Meadow"),
        Transform::IDENTITY,
        Scatter {
            species: vec![ScatterSpecies::default(), ScatterSpecies::default()],
            ..Scatter::default()
        },
    ));
    ed.state.select(Some(uid));
    d.frames(&mut ed, 2);
    assert!(d.has("Species 0") && d.has("Species 1"));
    assert!(!d.nodes.iter().any(|n| n.text.starts_with("Specie ")));
}

// ----- check boxes -----

fn rules(ed: &mut Editor) {
    let mut rules = RuleRegistry::default();
    rules.add::<Primitive, _>("walls", |_| RuleOutput::default());
    rules.add::<StandardMaterial, _>("box_arch", |_| RuleOutput::default());
    ed.state.world.insert_resource(rules);
}

#[test]
fn rule_list_switches_are_check_boxes() {
    let (_dir, mut ed, mut d) = editor("rules", DEFAULT);
    rules(&mut ed);
    ed.state.set_rules_panel(true);
    d.frames(&mut ed, 2);
    let walls = d.checkbox_beside("walls");
    assert_eq!(walls.toggled, Some(true));
    d.press(&mut ed, walls.rect.center(), PointerButton::Primary);
    assert!(!ed.state.rule_enabled("walls"));
    assert_eq!(d.checkbox_beside("walls").toggled, Some(false));
    let walls = d.checkbox_beside("walls");
    d.press(&mut ed, walls.rect.center(), PointerButton::Primary);
    assert!(ed.state.rule_enabled("walls"));
}

// ----- selection outline of generated meshes -----

fn owner_with_parts(ed: &mut Editor, n: usize) -> (Uid, Vec<Entity>) {
    let owner = ed.state.add_entity(NewEntity::Empty, None).unwrap();
    let o = entity(ed, owner);
    let parts = (0..n)
        .map(|i| {
            Part::new(PartKey::new(owner, format!("post{i}")))
                .at(Transform::from_xyz(i as f32 * 0.3, 0.5, 0.0))
                .with(Primitive::new(Shape::Cuboid {
                    size: Vec3::splat(0.2),
                }))
        })
        .collect();
    let r = reconcile_with(&mut ed.state.world, o, "posts", parts, Vec::new()).unwrap();
    assert_eq!(r.spawned, n);
    ed.prepare_render();
    let world = &ed.state.world;
    let parts = world.children(o);
    (owner, parts)
}

#[test]
fn generated_meshes_are_outlined_up_to_the_limit() {
    let (_dir, mut ed, mut d) = editor("outline", DEFAULT);
    // A few rule parts (walls, building parts): each outlined.
    let (few, parts) = owner_with_parts(&mut ed, 12);
    ed.state.select(Some(few));
    d.frames(&mut ed, 2);
    let outline = ed.state.world.resource::<SelectionOutline>().clone();
    assert_eq!(outline.primary.len(), 12);
    assert!(parts.iter().all(|p| outline.primary.contains(p)));
    // Over the limit: none of them, but the owner's own mesh still.
    let (many, _) = owner_with_parts(&mut ed, GENERATED_OUTLINE_LIMIT + 1);
    ed.state.add_component(many, "Primitive");
    ed.state.select(Some(many));
    d.frames(&mut ed, 2);
    let outline = ed.state.world.resource::<SelectionOutline>().clone();
    assert_eq!(outline.primary, vec![entity(&ed, many)]);
    // Both selected: the few still outlined (secondary), the many boxed.
    ed.state.selection = vec![few, many];
    d.frames(&mut ed, 2);
    let outline = ed.state.world.resource::<SelectionOutline>().clone();
    assert_eq!(outline.secondary.len(), 12);
    assert_eq!(outline.primary.len(), 1);
}

// ----- captures -----

/// Captures for review: the starter cube with Add Component in view, Scene
/// Settings closed and with Bloom open, the rule list (one rule off).
#[test]
fn usability_captures() {
    use box_editor::headless::{CaptureOptions, Headless};
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping captures");
        return;
    }
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/m6_ux");
    std::fs::create_dir_all(&out).unwrap();
    let dir = temp_project("captures");
    let mut ed = open(&dir.0);
    let options = CaptureOptions {
        size: [2880, 1800],
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
    let settle = |h: &mut Headless, ed: &mut Editor| {
        let mut nodes = Vec::new();
        for _ in 0..4 {
            nodes = run(h, ed, Vec::new());
        }
        nodes
    };
    let find = |nodes: &[Node], text: &str| -> Rect {
        nodes
            .iter()
            .rev()
            .find(|n| n.text == text)
            .unwrap_or_else(|| panic!("no `{text}`"))
            .rect
    };
    let save = |h: &Headless, name: &str| {
        let path = out.join(name);
        h.image().unwrap().save(&path).unwrap();
        eprintln!("{}", path.display());
    };

    // The starter cube: every section and Add Component in view.
    ed.state.select(Some(by_name(&ed, "Cube")));
    let nodes = settle(&mut h, &mut ed);
    assert!(find(&nodes, "Add Component").bottom() <= 900.0);
    save(&h, "cube_inspector.png");
    // A modified struct: the closed heading carries the modified dot.
    let cube = by_name(&ed, "Cube");
    let sway = WindSway {
        strength: 0.2,
        ..WindSway::default()
    };
    assert!(ed.state.set_field(cube, "StandardMaterial.wind", &sway));
    settle(&mut h, &mut ed);
    save(&h, "cube_wind_modified.png");

    // Scene Settings: closed headings with their switches, then Bloom open.
    ed.state.show_scene_settings();
    let nodes = settle(&mut h, &mut ed);
    save(&h, "scene_settings_closed.png");
    let bloom = find(&nodes, "Bloom").center();
    let press = |pressed| Event::PointerButton {
        pos: bloom,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    run(&mut h, &mut ed, vec![Event::PointerMoved(bloom)]);
    run(&mut h, &mut ed, vec![press(true), press(false)]);
    // Away from the heading, so it is not drawn hovered.
    run(
        &mut h,
        &mut ed,
        vec![Event::PointerMoved(pos2(700.0, 500.0))],
    );
    settle(&mut h, &mut ed);
    save(&h, "scene_settings_bloom_open.png");

    // The rule list, one rule switched off: both boxes visible.
    rules(&mut ed);
    ed.state.set_rule_enabled("box_arch", false);
    ed.state.set_rules_panel(true);
    let nodes = settle(&mut h, &mut ed);
    save(&h, "rule_list.png");
    // The off box is drawn: its border differs from the window behind it.
    let off = nodes
        .iter()
        .filter(|n| n.role == Role::CheckBox && n.toggled == Some(false))
        .map(|n| n.rect)
        .find(|r| {
            let row = find(&nodes, "box_arch");
            r.center().y > row.top() && r.center().y < row.bottom()
        })
        .expect("the box_arch switch");
    let image = h.image().unwrap();
    let px = |p: Pos2| image.get_pixel((p.x * 2.0) as u32, (p.y * 2.0) as u32).0;
    let edge = px(pos2(off.left() + 0.5, off.center().y));
    let window = px(pos2(off.left() - 4.0, off.center().y));
    let diff: i32 = (0..3)
        .map(|i| (edge[i] as i32 - window[i] as i32).abs())
        .sum();
    assert!(diff > 40, "box edge {edge:?} vs window {window:?}");
}
