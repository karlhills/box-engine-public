//! Rigid bodies in the editor (M7): Play simulates them and Stop leaves the
//! edit scene byte-identical with no simulation state in the edit world;
//! the physics components are in the inspector's registry with their
//! ranges; a capture of Play with contact points and a selected hinge.

use std::path::{Path, PathBuf};

use box_core::identity::{Name, Uid};
use box_core::prelude::*;
use box_core::scene;
use box_editor::headless::{CaptureOptions, Headless};
use box_editor::icons::ColliderDisplay;
use box_editor::inspect::{ComponentCategory, component_category};
use box_editor::log::LogBuffer;
use box_editor::{Editor, EditorOptions, PlayState, build_game_editor, layout};
use box_engine::Game;
use box_engine::box_physics::{Collider, Dynamics, Joint, RigidBody, Velocity};
use box_render::{RenderError, StandardMaterial};

const DT: f32 = 1.0 / 60.0;

fn temp_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("box_editor_dynamics_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn editor(autosave: &Path) -> Editor {
    let options = EditorOptions::default();
    let mut editor = build_game_editor(&options, LogBuffer::default(), Game::new()).unwrap();
    editor.state.autosave_dir = autosave.to_path_buf();
    editor
}

fn material(hex: u32) -> StandardMaterial {
    StandardMaterial {
        base_color: Color::hex(hex),
        roughness: 0.8,
        ..Default::default()
    }
}

struct Scene {
    falling: Uid,
    door: Uid,
}

/// Replace the scratch scene (keeping its sun and ground, which gets a
/// collider) with a box stack, a falling box and a hinged door.
fn physics_scene(editor: &mut Editor) -> Scene {
    let world = &mut editor.state.world;
    let roots: Vec<Entity> = world.roots().to_vec();
    for e in roots {
        let keep = world
            .get::<Name>(e)
            .is_some_and(|n| n.as_str() == "Sun" || n.as_str() == "Ground");
        if !keep {
            world.despawn(e);
        }
    }
    let ground = world.find_by_name("Ground").unwrap();
    world
        .insert_one(ground, Collider::from_primitive())
        .unwrap();
    let cube = |size: f32| Primitive::cuboid(Vec3::splat(size));
    for i in 0..4 {
        world
            .spawn_saved(
                Uid::new(),
                &format!("Box {i}"),
                (
                    Transform::from_xyz(-1.5, 0.4 + i as f32 * 0.8, 0.0),
                    cube(0.8),
                    material(0xd8a55a),
                    Collider::from_primitive(),
                    RigidBody::dynamic(2.0),
                ),
            )
            .unwrap();
    }
    let falling = Uid::new();
    world
        .spawn_saved(
            falling,
            "Falling",
            (
                Transform::from_xyz(-1.3, 6.0, 0.1).with_rotation(Quat::from_rotation_z(0.4)),
                cube(0.8),
                material(0xc0504d),
                Collider::from_primitive(),
                RigidBody::dynamic(4.0),
                Velocity::default(),
            ),
        )
        .unwrap();
    let door = Uid::new();
    world
        .spawn_saved(
            door,
            "Door",
            (
                Transform::from_xyz(2.0, 1.1, 0.0),
                Primitive::cuboid(Vec3::new(1.4, 2.2, 0.1)),
                material(0x8a5a3c),
                Collider::from_primitive(),
                RigidBody::dynamic(8.0),
                Joint::hinge(Vec3::new(-0.7, 0.0, 0.0), Vec3::Y).with_limits(-1.2, 1.2),
                Velocity {
                    linear: Vec3::ZERO,
                    angular: Vec3::new(0.0, 2.0, 0.0),
                },
            ),
        )
        .unwrap();
    editor.state.camera.orbit.target = Vec3::new(0.3, 1.2, 0.0);
    editor.state.camera.orbit.distance = 9.0;
    editor.state.camera.orbit.yaw = 0.5;
    editor.state.camera.orbit.pitch = 0.35;
    Scene { falling, door }
}

fn translation(world: &World, uid: Uid) -> Vec3 {
    let e = world.entity_by_uid(uid).unwrap();
    world.get::<Transform>(e).unwrap().translation
}

#[test]
fn play_simulates_bodies_and_stop_restores_the_edit_scene() {
    let dir = temp_dir("round_trip");
    let mut editor = editor(&dir);
    let s = physics_scene(&mut editor);
    let before = scene::save(&editor.state.world).unwrap();
    let start = translation(&editor.state.world, s.falling);

    editor.state.start_play().unwrap();
    for _ in 0..90 {
        assert!(editor.state.update_play(DT).unwrap());
    }
    let world = &editor.state.world;
    assert!(
        world.contains_resource::<Dynamics>(),
        "Play started the simulation"
    );
    let fell = translation(world, s.falling);
    assert!(
        fell.y < start.y - 2.0,
        "the box fell during Play: {start} -> {fell}"
    );
    let door = world.entity_by_uid(s.door).unwrap();
    let angle = world.resource::<Dynamics>().joint_position(door).unwrap();
    assert!(angle.abs() > 0.1, "the door turned: {angle}");

    editor.state.stop_play();
    assert_eq!(editor.state.play_state(), PlayState::Editing);
    let world = &editor.state.world;
    assert_eq!(
        scene::save(world).unwrap(),
        before,
        "the edit scene is byte-identical after Play"
    );
    assert!(
        !world.contains_resource::<Dynamics>(),
        "no simulation in the edit world"
    );
    assert_eq!(translation(world, s.falling), start);
    let e = world.entity_by_uid(s.falling).unwrap();
    assert!(
        !world.has::<box_core::interpolate::Interpolated>(e),
        "the play world's runtime additions stay there"
    );
}

#[test]
fn physics_components_are_in_the_inspector_registry_with_ranges() {
    let registry = box_engine::shared_engine_registry();
    for name in [
        "RigidBody",
        "Joint",
        "Velocity",
        "Collider",
        "KinematicCharacter",
    ] {
        assert!(
            registry.component(name).is_some(),
            "{name} is not registered"
        );
    }
    for name in ["RigidBody", "Joint", "Collider"] {
        assert_eq!(
            component_category(name),
            ComponentCategory::Physics,
            "{name}"
        );
    }
    let fields = registry.component("RigidBody").unwrap().fields();
    let mass = fields.iter().find(|f| f.name == "mass").unwrap();
    assert!(mass.range.is_some() && mass.doc.is_some(), "{mass:?}");
    let fields = registry.component("Collider").unwrap().fields();
    assert!(
        fields
            .iter()
            .any(|f| f.name == "friction" && f.range.is_some())
    );
    assert!(registry.resource("PhysicsSettings").is_some());
}

#[test]
fn capture_play_with_contact_points_and_a_selected_hinge() {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping the dynamics capture");
        return;
    }
    let dir = temp_dir("capture");
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/dynamics");
    std::fs::create_dir_all(&out).unwrap();
    let options = CaptureOptions {
        size: [1280, 720],
        pixels_per_point: 1.0,
        frames: 4,
    };
    let mut editor = editor(&dir);
    editor.tree = layout::default_layout();
    let s = physics_scene(&mut editor);
    editor.state.viewport.view.colliders = ColliderDisplay::All;
    editor.state.viewport.view.contacts = true;
    editor.state.select(Some(s.door));
    editor.state.start_play().unwrap();
    for _ in 0..80 {
        editor.state.update_play(DT).unwrap();
    }
    let mut h = match Headless::new(&mut editor, &options) {
        Ok(h) => h,
        Err(e)
            if matches!(
                e.downcast_ref::<RenderError>(),
                Some(RenderError::NoAdapter)
            ) =>
        {
            eprintln!("no GPU adapter; skipping the dynamics capture");
            return;
        }
        Err(e) => panic!("{e:#}"),
    };
    for _ in 0..options.frames {
        h.frame(&mut editor, Vec::new());
    }
    assert_eq!(editor.state.play_state(), PlayState::Playing);
    let contacts = editor
        .state
        .world
        .resource::<Dynamics>()
        .contact_points()
        .len();
    assert!(contacts > 4, "resting boxes touch: {contacts}");
    h.image()
        .unwrap()
        .save(out.join("editor_play_contacts.png"))
        .unwrap();
}
