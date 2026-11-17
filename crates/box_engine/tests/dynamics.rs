//! Dynamics through the engine (M7): the demo scene runs headlessly and
//! deterministically and does what it shows; bodies, joints and velocities
//! survive a scene save and load; scenes without bodies never start rapier.

#[path = "../examples/common/dynamics.rs"]
mod dynamics_demo;

use std::hash::{Hash, Hasher};

use box_engine::box_core::scene;
use box_engine::box_physics::Dynamics;
use box_engine::prelude::*;
use dynamics_demo::{DynamicsDemo, LANDING};

const DT: f32 = 1.0 / 60.0;

fn demo_app() -> App {
    Game::new().with_plugin(DynamicsDemo).new_app()
}

fn named(app: &App, name: &str) -> Entity {
    app.world
        .find_by_name(name)
        .unwrap_or_else(|| panic!("no {name}"))
}

fn at(app: &App, name: &str) -> Vec3 {
    app.world
        .get::<Transform>(named(app, name))
        .unwrap()
        .translation
}

fn body_hash(app: &App) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let mut q = app
        .world
        .query::<(&Name, &Transform)>()
        .with::<&RigidBody>();
    let mut rows: Vec<(String, [u32; 7])> = q
        .iter()
        .map(|(n, t)| {
            let v = [
                t.translation.x,
                t.translation.y,
                t.translation.z,
                t.rotation.x,
                t.rotation.y,
                t.rotation.z,
                t.rotation.w,
            ];
            (n.0.clone(), v.map(f32::to_bits))
        })
        .collect();
    rows.sort();
    rows.hash(&mut h);
    h.finish()
}

#[test]
fn the_demo_runs_deterministically_and_everything_happens() {
    let run = || {
        let mut app = demo_app();
        let mut hashes = Vec::new();
        for _ in 0..8 {
            app.run_frames(50, DT);
            hashes.push(body_hash(&app));
        }
        (app, hashes)
    };
    let (app, first) = run();
    let (_, second) = run();
    assert_eq!(
        first, second,
        "same inputs, same body hashes every 50 frames"
    );

    let dynamics = app.world.resource::<Dynamics>();
    let door = dynamics
        .joint_position(named(&app, "Door"))
        .expect("door hinge");
    assert!(door.abs() > 0.5, "the ball swung the door open: {door}");
    assert!(door.abs() <= 1.2 + 0.03, "within its limits: {door}");

    let toppled = (0..6).any(|i| {
        let p = at(&app, &format!("Stack {i}"));
        p.y < 0.35 + i as f32 * 0.7 - 0.3
    });
    assert!(toppled, "the boulder knocked the stack over");

    let character = at(&app, "Character");
    assert!(
        character.x > 1.0 && character.y > LANDING + 0.85,
        "the character climbed the stairs: {character}"
    );
    let crate_ = at(&app, "Crate");
    assert!(
        crate_.x > 4.0 && crate_.y < 0.5,
        "the crate was pushed off the landing: {crate_}"
    );
    let link = at(&app, "Link 7");
    assert!(link.y < 4.6 - 1.5, "the chain swung down: {link}");
}

#[test]
fn bodies_joints_and_velocities_survive_save_and_load() {
    let registry = box_engine::shared_engine_registry();
    let mut world = World::with_registry(registry.clone());
    let anchor = Uid::new();
    world
        .spawn_saved(
            anchor,
            "Anchor",
            (
                Transform::from_xyz(0.0, 3.0, 0.0),
                Collider::cuboid(Vec3::ONE).with_material(0.9, 0.3),
                RigidBody::fixed(),
            ),
        )
        .unwrap();
    let body = RigidBody {
        mass: 12.5,
        linear_damping: 0.2,
        gravity_scale: 0.5,
        ccd: true,
        locks: AxisLocks::ROTATION,
        ..RigidBody::default()
    };
    let joint = Joint::hinge(Vec3::new(0.5, 0.0, 0.0), Vec3::Z)
        .connected_to(anchor)
        .with_limits(-0.3, 1.2);
    let velocity = Velocity {
        linear: Vec3::new(1.0, 2.0, 3.0),
        angular: Vec3::new(0.0, 0.5, 0.0),
    };
    let swing = Uid::new();
    world
        .spawn_saved(
            swing,
            "Swing",
            (
                Transform::from_xyz(1.0, 3.0, 0.0),
                Collider::sphere(0.5),
                body.clone(),
                joint.clone(),
                velocity,
            ),
        )
        .unwrap();
    let bytes = scene::save(&world).unwrap();
    let loaded = scene::load(&bytes, registry.clone()).unwrap();
    let e = loaded.entity_by_uid(swing).unwrap();
    assert_eq!(*loaded.get::<RigidBody>(e).unwrap(), body);
    assert_eq!(*loaded.get::<Joint>(e).unwrap(), joint);
    assert_eq!(*loaded.get::<Velocity>(e).unwrap(), velocity);
    let a = loaded.entity_by_uid(anchor).unwrap();
    assert_eq!(loaded.get::<Collider>(a).unwrap().friction, 0.9);
    assert_eq!(scene::save(&loaded).unwrap(), bytes, "byte-identical");

    // Simulating the loaded scene leaves no runtime state that would be saved.
    let mut app = box_engine::app_with_world(loaded);
    app.run_frames(30, DT);
    assert!(app.world.contains_resource::<Dynamics>());
    let saved = String::from_utf8(scene::save(&app.world).unwrap()).unwrap();
    for runtime in ["Dynamics", "handle", "ColliderProxy"] {
        assert!(!saved.contains(runtime), "{runtime} leaked into the save");
    }
}

#[test]
fn colliders_saved_before_m7_load_with_default_material() {
    let registry = box_engine::shared_engine_registry();
    let text = r#"{
      "version": 1,
      "entities": [
        {
          "uid": "c0ffee00-0000-4000-8000-000000000001",
          "name": "Old",
          "parent": null,
          "components": {
            "Collider": {
              "shape": "Primitive",
              "offset": [0.0, 0.0, 0.0],
              "is_trigger": false,
              "layers": 1,
              "mask": 4294967295
            }
          }
        }
      ]
    }"#;
    let world = scene::load(text.as_bytes(), registry).unwrap();
    let e = world.find_by_name("Old").unwrap();
    let c = world.get::<Collider>(e).unwrap();
    assert_eq!(c.friction, Collider::DEFAULT_FRICTION);
    assert_eq!(c.restitution, 0.0);
}

#[test]
fn a_scene_without_bodies_never_starts_the_simulation() {
    let mut app = box_engine::default_app();
    app.add_plugin(&CharacterPlugin);
    app.world.spawn((
        Transform::default(),
        Primitive::cuboid(Vec3::new(10.0, 1.0, 10.0)),
        Collider::from_primitive(),
    ));
    app.world.spawn((
        Transform::from_xyz(0.0, 3.0, 0.0),
        KinematicCharacter::default(),
    ));
    app.run_frames(120, DT);
    assert!(!app.world.contains_resource::<Dynamics>());
    assert!(
        app.world
            .get::<KinematicCharacter>(app.world.first_with::<KinematicCharacter>().unwrap())
            .unwrap()
            .grounded
    );
}
