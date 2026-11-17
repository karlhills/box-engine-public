//! M5 in the editor: a generator defined by a native project's plugin is
//! offered under Add ▸ Procedural (menus and palette), shows in the
//! inspector, regenerates on edits, and bakes to a mesh as one undo step.
//! The capture test writes `target/captures/m5/` and needs a GPU.

#[allow(dead_code)]
#[path = "../../box_engine/examples/common/steps.rs"]
mod steps;

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use box_core::mesh_asset::MeshAsset;
use box_editor::headless::{CaptureOptions, Headless};
use box_editor::log::LogBuffer;
use box_editor::{Editor, EditorOptions, build_game_editor};
use box_engine::Game;
use box_engine::box_proc::{ProcState, Procedural};
use box_render::{MeshRenderer, StandardMaterial};
use egui::accesskit::TreeUpdate;
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, pos2};
use steps::{Steps, StepsPlugin};

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn project_editor(name: &str) -> (Editor, TempDir) {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "box_editor_proc_{name}_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    box_engine::Project::create(&dir, name).unwrap();
    let options = EditorOptions {
        project: Some(dir.clone()),
        ..Default::default()
    };
    let game = Game::new().with_plugin(StepsPlugin);
    let editor = build_game_editor(&options, LogBuffer::default(), game).unwrap();
    (editor, TempDir(dir))
}

fn sync(editor: &mut Editor) {
    box_core::transform::propagate_transforms(&mut editor.state.world);
    box_engine::sync_scene_meshes(&mut editor.state.world);
}

#[test]
fn a_project_generator_is_offered_added_edited_and_baked() {
    let (mut editor, _dir) = project_editor("add");
    // The edit world gets the game's rules before its first sync.
    editor.prepare_render();
    assert!(
        editor
            .state
            .world
            .contains_resource::<box_engine::box_proc::rule::RuleRegistry>()
    );
    let entries = editor.state.generator_entries();
    let steps = entries.iter().find(|g| g.name == "Steps").expect("listed");
    assert_eq!(steps.category, "Procedural");
    assert_eq!(steps.label, "Steps");
    assert!(steps.doc.unwrap().contains("flight of steps"));

    // The palette offers it as Add ▸ Procedural ▸ Steps.
    let actions = box_editor::actions::context_actions(&mut editor.state);
    let add = actions
        .iter()
        .find(|a| a.id() == "add.generator:Steps")
        .expect("palette action");
    assert_eq!(add.label(), "Procedural ▸ Steps");
    let undo_before = editor.state.edits.undo_len();
    editor.state.perform(add).unwrap();
    assert_eq!(
        editor.state.edits.undo_len(),
        undo_before + 1,
        "one undo step"
    );
    let uid = editor.state.selection[0];
    let e = editor.state.world.entity_by_uid(uid).unwrap();
    for c in ["Steps", "Procedural", "StandardMaterial", "Transform"] {
        assert!(
            editor.state.world.component_names(e).contains(&c),
            "{c} missing"
        );
    }
    sync(&mut editor);
    let generations = editor
        .state
        .world
        .get::<ProcState>(e)
        .unwrap()
        .generations();
    assert_eq!(generations, 1);
    let r = (*editor.state.world.get::<MeshRenderer>(e).unwrap()).clone();
    assert_eq!(r.materials.len(), 2);

    // An inspector-style edit through the edit stack regenerates.
    let path = editor.state.world.field_path("Steps.count").unwrap();
    editor
        .state
        .edits
        .set_field(&mut editor.state.world, uid, &path, &3u32)
        .unwrap();
    std::thread::sleep(Duration::from_millis(35)); // past the real-clock throttle
    sync(&mut editor);
    let s = editor.state.world.get::<ProcState>(e).unwrap();
    assert_eq!(s.generations(), 2);
    drop(s);
    let tris = |ed: &Editor| {
        let r = ed.state.world.get::<MeshRenderer>(e).unwrap();
        ed.state
            .world
            .resource::<box_core::assets::Assets<box_core::mesh::MeshData>>()
            .get(r.mesh)
            .unwrap()
            .triangle_count()
    };
    assert_eq!(tris(&editor), 3 * 12);

    // Bake to Mesh: one undo step; undo restores the generator.
    editor.state.select(Some(uid));
    let bake = editor.state.actions().get("edit.bake").cloned().unwrap();
    assert_eq!(bake.availability(&editor.state), Ok(()));
    editor.state.perform(&bake).unwrap();
    assert!(editor.state.world.has::<MeshAsset>(e));
    assert!(!editor.state.world.has::<Steps>(e) && !editor.state.world.has::<Procedural>(e));
    assert_eq!(editor.state.edits.undo_label(), Some("Bake to Mesh"));
    assert!(_dir.0.join("assets/baked/Steps.glb").is_file());
    assert!(
        bake.availability(&editor.state).is_err(),
        "a baked object is no longer procedural"
    );
    editor.state.undo();
    assert!(editor.state.world.has::<Steps>(e) && !editor.state.world.has::<MeshAsset>(e));
    assert!(editor.state.world.has::<StandardMaterial>(e));
}

// ----- capture: the Add menu and the inspector -----

fn texts(update: Option<&TreeUpdate>) -> Vec<(String, Rect)> {
    let Some(u) = update else {
        return Vec::new();
    };
    u.nodes
        .iter()
        .filter_map(|(_, n)| {
            let b = n.bounds()?;
            let text = n.label().or(n.value()).unwrap_or_default().to_string();
            Some((
                text,
                Rect::from_min_max(
                    pos2(b.x0 as f32, b.y0 as f32),
                    pos2(b.x1 as f32, b.y1 as f32),
                ),
            ))
        })
        .collect()
}

#[test]
fn captures_add_procedural_menu_and_inspector() {
    let (mut editor, _dir) = project_editor("capture");
    let options = CaptureOptions {
        size: [2560, 1600],
        pixels_per_point: 2.0,
        frames: 1,
    };
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
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/m5");
    std::fs::create_dir_all(&out).unwrap();
    h.enable_accesskit();
    let run = |h: &mut Headless, ed: &mut Editor, events: Vec<Event>| {
        h.frame(ed, events);
        texts(h.accesskit_update())
    };
    let find = |nodes: &[(String, Rect)], text: &str| -> Pos2 {
        nodes
            .iter()
            .rev()
            .find(|(t, _)| t == text || t.starts_with(&format!("{text}\n")))
            .unwrap_or_else(|| {
                let all: Vec<&str> = nodes.iter().map(|(t, _)| t.as_str()).collect();
                panic!("no `{text}` among {all:?}")
            })
            .1
            .center()
    };
    let press = |pos, pressed| Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    let settle = |h: &mut Headless, ed: &mut Editor| {
        let mut n = Vec::new();
        for _ in 0..10 {
            n = run(h, ed, Vec::new());
        }
        n
    };
    let mut nodes = settle(&mut h, &mut editor);

    // Toolbar + Add, then hover Procedural: the submenu lists Steps.
    let add = find(&nodes, "+ Add");
    run(&mut h, &mut editor, vec![Event::PointerMoved(add)]);
    run(
        &mut h,
        &mut editor,
        vec![press(add, true), press(add, false)],
    );
    nodes = settle(&mut h, &mut editor);
    let procedural = find(&nodes, "Procedural");
    run(&mut h, &mut editor, vec![Event::PointerMoved(procedural)]);
    nodes = settle(&mut h, &mut editor);
    let steps = find(&nodes, "Steps");
    h.image()
        .unwrap()
        .save(out.join("add_procedural_menu.png"))
        .unwrap();

    // Click Steps: a new flight, selected, with its parameters in the
    // inspector.
    run(&mut h, &mut editor, vec![Event::PointerMoved(steps)]);
    run(
        &mut h,
        &mut editor,
        vec![press(steps, true), press(steps, false)],
    );
    nodes = settle(&mut h, &mut editor);
    assert_eq!(editor.state.selection.len(), 1);
    let e = editor
        .state
        .world
        .entity_by_uid(editor.state.selection[0])
        .unwrap();
    assert!(editor.state.world.has::<Steps>(e));
    assert!(editor.state.world.has::<MeshRenderer>(e), "drawn");
    for label in ["Steps", "Procedural", "Count", "Wobble", "Seed"] {
        find(&nodes, label);
    }
    h.image()
        .unwrap()
        .save(out.join("steps_inspector.png"))
        .unwrap();
    assert_eq!(h.renderer().stats().validation_errors, 0);
}
