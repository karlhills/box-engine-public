//! Interactions in the editor (docs/INTERACTIONS.md): an `Interactable`'s
//! entity target is an entity picker in the inspector (any `Option<Uid>`
//! field is), picking another bridge is one undoable edit, and the
//! `examples/ember_bridge` brazier works in an editor Play session.
//! Capture: `target/captures/interact/editor_inspector.png`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use box_core::identity::Uid;
use box_core::prelude::*;
use box_editor::headless::{CaptureOptions, Headless};
use box_editor::inspect::{FieldKind, rows};
use box_editor::log::LogBuffer;
use box_editor::panes::inspector::set_list_open;
use box_editor::{EditorOptions, build_editor, layout};
use box_engine::box_core::action::{Actions, InjectedInput};
use box_engine::box_physics::Collidable;
use box_engine::interact::{GameVars, InteractAction, Interactable, Target};

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() == ".box" {
            continue;
        }
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn uid_of(world: &World, name: &str) -> Uid {
    let e = world.find_by_name(name).unwrap();
    *world.get::<Uid>(e).unwrap()
}

#[test]
fn bridges_are_picked_in_the_inspector_and_braziers_work_in_play() {
    let log = LogBuffer::default();
    let dir = std::env::temp_dir().join(format!("box_editor_interact_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _cleanup = TempDir(dir.clone());
    copy_dir(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/ember_bridge"),
        &dir,
    );
    let options = EditorOptions {
        project: Some(dir.clone()),
        no_restore: true,
        ..Default::default()
    };
    let mut editor = build_editor(&options, log.clone()).unwrap();
    editor.state.autosave_dir = dir.join(".box/autosave");
    editor.tree = layout::default_layout();
    let brazier = uid_of(&editor.state.world, "Brazier West");
    let west = uid_of(&editor.state.world, "Bridge West");
    let east = uid_of(&editor.state.world, "Bridge East");

    // The Show action's target is an entity picker row.
    let e = editor.state.world.entity_by_uid(brazier).unwrap();
    let value: Interactable = (*editor.state.world.get::<Interactable>(e).unwrap()).clone();
    let index = value
        .actions
        .iter()
        .position(|a| matches!(a, InteractAction::Show { .. }))
        .unwrap();
    let registry = editor.state.world.registry().clone();
    let all = rows(&registry, "Interactable", &value);
    let path = format!("Interactable.actions[{index}].target.0");
    let row = all.iter().find(|r| r.path == path).unwrap_or_else(|| {
        panic!(
            "no row {path}: {:?}",
            all.iter().map(|r| &r.path).collect::<Vec<_>>()
        )
    });
    assert_eq!(row.kind, FieldKind::Entity);
    let shown = |editor: &box_editor::Editor| {
        let e = editor.state.world.entity_by_uid(brazier).unwrap();
        match &editor.state.world.get::<Interactable>(e).unwrap().actions[index] {
            InteractAction::Show {
                target: Target::Entity(u),
                ..
            } => *u,
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(shown(&editor), Some(west));

    // Picking the other bridge (what the picker sends) is one undo step.
    assert!(editor.state.set_field(brazier, &path, &Some(east)));
    assert_eq!(shown(&editor), Some(east));
    editor.state.undo_scene();
    assert_eq!(shown(&editor), Some(west));

    // The inspector with the brazier selected (GPU runs only).
    editor.state.select(Some(brazier));
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_none() {
        // Open the actions list and show the Show action's target picker.
        let mut h = Headless::new(
            &mut editor,
            &CaptureOptions {
                size: [1600, 1500],
                pixels_per_point: 1.0,
                frames: 4,
            },
        )
        .unwrap();
        set_list_open(h.ctx(), "Interactable.actions", true);
        for _ in 0..4 {
            h.frame(&mut editor, Vec::new());
            editor.state.wait_assets_idle(Duration::from_secs(20));
        }
        let img = h.image().unwrap();
        let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/captures/interact/editor_inspector.png");
        std::fs::create_dir_all(out.parent().unwrap()).unwrap();
        img.save(&out).unwrap();
    }
    let saved = box_core::scene::save(&editor.state.world).unwrap();

    // Play: with an ember, standing at the brazier, `interact` lights it.
    editor.state.start_play().unwrap();
    let step = |editor: &mut box_editor::Editor| {
        editor.state.update_assets();
        editor.state.update_play(1.0 / 60.0).unwrap();
    };
    for _ in 0..30 {
        step(&mut editor);
        assert!(editor.state.wait_assets_idle(Duration::from_secs(20)));
    }
    let world = &mut editor.state.world;
    world.resource_mut::<GameVars>().set("embers", 1.0);
    let player = world.find_by_name("Player").unwrap();
    world
        .component_mut::<Transform>(player)
        .unwrap()
        .translation = Vec3::new(1.3, 0.92, 0.2);
    for _ in 0..5 {
        step(&mut editor);
    }
    editor
        .state
        .world
        .resource_mut::<Actions>()
        .inject("interact", InjectedInput::press(2));
    for _ in 0..3 {
        step(&mut editor);
    }
    let world = &editor.state.world;
    let bridge = world.entity_by_uid(west).unwrap();
    assert_eq!(world.get::<Visible>(bridge).map(|v| v.0), Some(true));
    assert_eq!(world.get::<Collidable>(bridge).map(|c| c.0), Some(true));
    assert_eq!(world.resource::<GameVars>().get("embers"), 0.0);

    // Stop: the edit scene is as it was.
    editor.state.stop_play();
    assert_eq!(box_core::scene::save(&editor.state.world).unwrap(), saved);
}
