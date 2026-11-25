//! Queries, filters, triggers, change-driven sync and scene round trip.

use std::sync::Arc;

use box_core::prelude::*;
use box_core::scene;
use box_physics::*;

fn app() -> App {
    let mut app = App::new();
    app.add_plugin(&PhysicsPlugin);
    app
}

fn physics(app: &App) -> &Physics {
    app.world.resource::<Physics>()
}

fn cube_at(app: &mut App, x: f32, y: f32, z: f32) -> Entity {
    app.world
        .spawn((Transform::from_xyz(x, y, z), Collider::cuboid(Vec3::ONE)))
}

const ALL: QueryFilter = QueryFilter {
    mask: u32::MAX,
    exclude: None,
    include_triggers: false,
};

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

#[test]
fn raycast_hits_nearest_and_misses() {
    let mut app = app();
    let near = cube_at(&mut app, 0.0, 0.0, -5.0);
    let far = cube_at(&mut app, 0.0, 0.0, -10.0);
    app.update();
    let p = physics(&app);
    assert_eq!(p.len(), 2);

    let ray = Ray::new(Vec3::ZERO, Vec3::NEG_Z);
    let hit = p.raycast(&ray, 100.0, &ALL).unwrap();
    assert_eq!(hit.entity, near);
    assert!(approx(hit.toi, 4.5), "{hit:?}");
    assert!((hit.point - Vec3::new(0.0, 0.0, -4.5)).length() < 1e-3);
    assert!((hit.normal - Vec3::Z).length() < 1e-3, "{hit:?}");

    assert!(p.raycast(&ray, 4.0, &ALL).is_none(), "beyond max_toi");
    let side = Ray::new(Vec3::new(3.0, 0.0, 0.0), Vec3::NEG_Z);
    assert!(p.raycast(&side, 100.0, &ALL).is_none());
    assert!(p.raycast(&ray, f32::NAN, &ALL).is_none());

    let all = p.raycast_all(&ray, 100.0, &ALL);
    assert_eq!(
        all.iter().map(|h| h.entity).collect::<Vec<_>>(),
        vec![near, far]
    );
    assert!(approx(all[1].toi, 9.5));

    // Excluding the near cube finds the far one.
    let hit = p.raycast(&ray, 100.0, &ALL.excluding(near)).unwrap();
    assert_eq!(hit.entity, far);

    // From inside: toi 0.
    let inside = Ray::new(Vec3::new(0.0, 0.0, -5.0), Vec3::X);
    assert_eq!(p.raycast(&inside, 100.0, &ALL).unwrap().toi, 0.0);
}

#[test]
fn filters_by_layer_and_trigger() {
    let mut app = app();
    let wall = app.world.spawn((
        Transform::from_xyz(0.0, 0.0, -5.0),
        Collider::cuboid(Vec3::ONE).with_layers(0b10, u32::MAX),
    ));
    let zone = app.world.spawn((
        Transform::from_xyz(0.0, 0.0, -2.0),
        Collider::sphere(0.5).trigger(),
    ));
    app.update();
    let p = physics(&app);
    let ray = Ray::new(Vec3::ZERO, Vec3::NEG_Z);
    assert_eq!(p.raycast(&ray, 100.0, &ALL).unwrap().entity, wall);
    assert_eq!(
        p.raycast(&ray, 100.0, &ALL.with_triggers()).unwrap().entity,
        zone
    );
    assert!(p.raycast(&ray, 100.0, &ALL.with_mask(0b01)).is_none());
    assert_eq!(
        p.raycast(&ray, 100.0, &ALL.with_mask(0b11)).unwrap().entity,
        wall
    );
}

#[test]
fn shapecast_finds_time_of_impact_and_normal() {
    let mut app = app();
    // A wall whose near face is at x = 4.5.
    let wall = app.world.spawn((
        Transform::from_xyz(5.0, 0.0, 0.0),
        Collider::cuboid(Vec3::new(1.0, 4.0, 20.0)),
    ));
    let player = app
        .world
        .spawn((Transform::default(), Collider::capsule(0.5, 0.5)));
    app.update();
    let p = physics(&app);
    let capsule = QueryShape::capsule(0.5, 0.5);
    let filter = ALL.excluding(player);

    let hit = p
        .shapecast(&capsule, Vec3::ZERO, Vec3::X, 10.0, &filter)
        .unwrap();
    assert_eq!(hit.entity, wall);
    assert!(approx(hit.toi, 4.0), "{hit:?}");
    assert!((hit.normal - Vec3::NEG_X).length() < 1e-3, "{hit:?}");
    assert!(approx(hit.point.x, 4.5), "{hit:?}");
    assert!(!hit.penetrating);

    // Diagonal motion: toi measured along the normalized direction.
    let hit = p
        .shapecast(
            &capsule,
            Vec3::ZERO,
            Vec3::new(1.0, 0.0, 1.0),
            10.0,
            &filter,
        )
        .unwrap();
    assert!(approx(hit.toi, 4.0 * 2f32.sqrt()), "{hit:?}");

    // Too short, wrong way, or zero direction.
    assert!(
        p.shapecast(&capsule, Vec3::ZERO, Vec3::X, 3.9, &filter)
            .is_none()
    );
    assert!(
        p.shapecast(&capsule, Vec3::ZERO, Vec3::NEG_X, 10.0, &filter)
            .is_none()
    );
    assert!(
        p.shapecast(&capsule, Vec3::ZERO, Vec3::ZERO, 10.0, &filter)
            .is_none()
    );

    // Starting inside the wall.
    let hit = p
        .shapecast(&capsule, Vec3::new(4.6, 0.0, 0.0), Vec3::X, 1.0, &filter)
        .unwrap();
    assert!(hit.penetrating && hit.toi == 0.0, "{hit:?}");

    // Without excluding itself, the capsule hits its own collider at once.
    let own = p
        .shapecast(&capsule, Vec3::ZERO, Vec3::X, 10.0, &ALL)
        .unwrap();
    assert_eq!(own.entity, player);
}

#[test]
fn overlap_and_contains_point() {
    let mut app = app();
    let a = cube_at(&mut app, 0.0, 0.0, 0.0);
    let b = cube_at(&mut app, 1.2, 0.0, 0.0);
    let _c = cube_at(&mut app, 10.0, 0.0, 0.0);
    app.update();
    let p = physics(&app);
    let mut hits = p.overlap(&QueryShape::sphere(0.3), Vec3::new(0.6, 0.0, 0.0), &ALL);
    hits.sort();
    let mut expected = vec![a, b];
    expected.sort();
    assert_eq!(hits, expected);
    assert!(
        p.overlap(&QueryShape::sphere(0.3), Vec3::new(5.0, 0.0, 0.0), &ALL)
            .is_empty()
    );
    let rotated = Pose::new(Vec3::new(5.0, 0.0, 0.0), Quat::from_rotation_z(1.0));
    assert!(
        p.overlap(
            &QueryShape::cuboid(Vec3::new(20.0, 0.1, 0.1)),
            rotated,
            &ALL
        )
        .is_empty()
    );
    let mut first = None;
    p.overlap_each(
        &QueryShape::sphere(0.3),
        Vec3::new(0.6, 0.0, 0.0),
        &ALL,
        |e| {
            first = Some(e);
            false
        },
    );
    assert!(first.is_some());

    assert_eq!(p.contains_point(Vec3::new(0.1, 0.2, 0.3), &ALL), vec![a]);
    assert!(p.contains_point(Vec3::new(0.0, 2.0, 0.0), &ALL).is_empty());
    assert!(p.contains_point(Vec3::NAN, &ALL).is_empty());
}

#[test]
fn sync_is_change_driven() {
    let mut app = app();
    let e = cube_at(&mut app, 0.0, 0.0, -5.0);
    for i in 0..10 {
        cube_at(&mut app, 100.0 + 3.0 * i as f32, 0.0, 0.0);
    }
    app.update();
    let s = physics(&app).last_sync();
    assert_eq!((s.inserted, s.shapes_built), (11, 11));

    app.update();
    let s = physics(&app).last_sync();
    assert_eq!(s.checked, 11);
    assert_eq!(
        (s.inserted, s.updated, s.removed, s.shapes_built),
        (0, 0, 0, 0)
    );

    // Moving updates the pose only; queries follow.
    app.world.get_mut::<Transform>(e).unwrap().translation.x = 3.0;
    app.update();
    let s = physics(&app).last_sync();
    assert_eq!((s.updated, s.shapes_built), (1, 0));
    let ray = Ray::new(Vec3::ZERO, Vec3::NEG_Z);
    assert!(physics(&app).raycast(&ray, 100.0, &ALL).is_none());
    let moved = Ray::new(Vec3::new(3.0, 0.0, 0.0), Vec3::NEG_Z);
    assert_eq!(
        physics(&app).raycast(&moved, 100.0, &ALL).unwrap().entity,
        e
    );

    // Scaling rebuilds the shape.
    app.world.get_mut::<Transform>(e).unwrap().scale = Vec3::splat(4.0);
    app.update();
    assert_eq!(physics(&app).last_sync().shapes_built, 1);
    assert!(approx(
        physics(&app).raycast(&moved, 100.0, &ALL).unwrap().toi,
        3.0
    ));

    // Editing the collider rebuilds the shape.
    app.world.get_mut::<Collider>(e).unwrap().shape = ColliderShape::Sphere { radius: 0.25 };
    app.update();
    assert_eq!(physics(&app).last_sync().shapes_built, 1);
    assert!(approx(
        physics(&app).raycast(&moved, 100.0, &ALL).unwrap().toi,
        4.0
    ));

    // Removing the component removes the collider.
    app.world.remove_one::<Collider>(e).unwrap();
    app.update();
    assert_eq!(physics(&app).last_sync().removed, 1);
    assert_eq!(physics(&app).len(), 10);
    assert!(physics(&app).raycast(&moved, 100.0, &ALL).is_none());

    // Adding it back, then despawning.
    app.world
        .insert_one(e, Collider::cuboid(Vec3::ONE))
        .unwrap();
    app.update();
    assert_eq!(physics(&app).len(), 11);
    app.world.despawn(e);
    app.update();
    assert_eq!(physics(&app).last_sync().removed, 1);
    assert_eq!(physics(&app).len(), 10);
    assert!(physics(&app).raycast(&moved, 100.0, &ALL).is_none());
}

#[test]
fn children_and_offsets_use_world_transforms() {
    let mut app = app();
    let parent = app.world.spawn((Transform::from_xyz(0.0, 0.0, -10.0)
        .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)),));
    let child = app.world.spawn((
        Transform::from_xyz(0.0, 0.0, 0.0),
        Collider::sphere(0.5).with_offset(Vec3::new(0.0, 0.0, 2.0)),
    ));
    app.world.set_parent(child, Some(parent)).unwrap();
    app.update();
    // The offset (local +Z) is rotated to world +X by the parent.
    let ray = Ray::new(Vec3::new(2.0, 5.0, -10.0), Vec3::NEG_Y);
    let hit = physics(&app).raycast(&ray, 100.0, &ALL).unwrap();
    assert_eq!(hit.entity, child);
    assert!(approx(hit.toi, 4.5));
    // Moving the parent moves the child's collider.
    app.world
        .get_mut::<Transform>(parent)
        .unwrap()
        .translation
        .y = 1.0;
    app.update();
    let hit = physics(&app).raycast(&ray, 100.0, &ALL).unwrap();
    assert!(approx(hit.toi, 3.5), "{hit:?}");
}

#[test]
fn primitive_collider_follows_the_primitive() {
    let mut app = app();
    let e = app.world.spawn((
        Transform::default(),
        Primitive::new(Shape::Cuboid {
            size: Vec3::new(2.0, 2.0, 2.0),
        }),
        Collider::from_primitive(),
    ));
    let bare = app
        .world
        .spawn((Transform::from_xyz(50.0, 0.0, 0.0), Collider::default()));
    app.update();
    assert_eq!(physics(&app).len(), 1, "no primitive, no collider");
    let ray = Ray::new(Vec3::new(0.0, 10.0, 0.0), Vec3::NEG_Y);
    assert!(approx(
        physics(&app).raycast(&ray, 100.0, &ALL).unwrap().toi,
        9.0
    ));
    app.world.get_mut::<Primitive>(e).unwrap().shape = Shape::Sphere {
        radius: 3.0,
        sectors: 8,
        stacks: 4,
    };
    app.update();
    assert_eq!(physics(&app).last_sync().shapes_built, 1);
    assert!(approx(
        physics(&app).raycast(&ray, 100.0, &ALL).unwrap().toi,
        7.0
    ));
    // Losing the primitive removes the collider; giving one to the bare
    // entity adds it.
    app.world.remove_one::<Primitive>(e).unwrap();
    app.world.insert_one(bare, Primitive::default()).unwrap();
    app.update();
    assert_eq!(physics(&app).len(), 1);
    assert!(physics(&app).raycast(&ray, 100.0, &ALL).is_none());
    let at_bare = Ray::new(Vec3::new(50.0, 10.0, 0.0), Vec3::NEG_Y);
    assert_eq!(
        physics(&app).raycast(&at_bare, 100.0, &ALL).unwrap().entity,
        bare
    );
}

/// Every trigger event sent so far, read with a cursor.
fn drain(app: &App, cursor: &mut EventCursor<TriggerEvent>) -> Vec<TriggerEvent> {
    cursor
        .read(app.world.resource::<Events<TriggerEvent>>())
        .copied()
        .collect()
}

#[test]
fn triggers_report_enter_and_exit_once() {
    let mut app = app();
    let zone = app.world.spawn((
        Transform::default(),
        Collider::cuboid(Vec3::splat(2.0)).trigger(),
    ));
    let mover = app
        .world
        .spawn((Transform::from_xyz(5.0, 0.0, 0.0), Collider::sphere(0.5)));
    let _static = cube_at(&mut app, -20.0, 0.0, 0.0);
    let mut cursor = EventCursor::<TriggerEvent>::default();
    app.update();
    assert!(drain(&app, &mut cursor).is_empty());

    let set_x = |app: &mut App, x: f32| {
        app.world.get_mut::<Transform>(mover).unwrap().translation.x = x;
        app.update();
    };
    set_x(&mut app, 1.2); // touching: 1.2 - 0.5 < 1.0
    assert_eq!(
        drain(&app, &mut cursor),
        vec![TriggerEvent {
            trigger: zone,
            other: mover,
            kind: TriggerKind::Enter
        }]
    );
    set_x(&mut app, 0.5);
    set_x(&mut app, 0.0);
    app.update();
    assert!(
        drain(&app, &mut cursor).is_empty(),
        "no repeats while inside"
    );
    set_x(&mut app, 3.0);
    assert_eq!(
        drain(&app, &mut cursor),
        vec![TriggerEvent {
            trigger: zone,
            other: mover,
            kind: TriggerKind::Exit
        }]
    );
    set_x(&mut app, 4.0);
    assert!(drain(&app, &mut cursor).is_empty());

    // Despawning while inside sends Exit.
    set_x(&mut app, 0.0);
    assert_eq!(drain(&app, &mut cursor).len(), 1);
    app.world.despawn(mover);
    app.update();
    assert_eq!(
        drain(&app, &mut cursor),
        vec![TriggerEvent {
            trigger: zone,
            other: mover,
            kind: TriggerKind::Exit
        }]
    );
}

#[test]
fn trigger_masks_and_trigger_pairs() {
    let mut app = app();
    let zone = app.world.spawn((
        Transform::default(),
        Collider::sphere(1.0).trigger().with_layers(1, 0b10),
    ));
    let ignored = app.world.spawn((
        Transform::from_xyz(0.5, 0.0, 0.0),
        Collider::sphere(0.5).with_layers(0b01, u32::MAX),
    ));
    let seen = app.world.spawn((
        Transform::from_xyz(-0.5, 0.0, 0.0),
        Collider::sphere(0.5).with_layers(0b10, u32::MAX),
    ));
    let other_zone = app.world.spawn((
        Transform::from_xyz(0.0, 0.5, 0.0),
        Collider::sphere(0.5).trigger().with_layers(0b10, u32::MAX),
    ));
    let mut cursor = EventCursor::<TriggerEvent>::default();
    app.update();
    let mut events = drain(&app, &mut cursor);
    events.sort_by_key(|e| (e.trigger, e.other));
    let pairs: Vec<_> = events.iter().map(|e| (e.trigger, e.other)).collect();
    assert!(events.iter().all(|e| e.kind == TriggerKind::Enter));
    assert!(pairs.contains(&(zone, seen)));
    assert!(pairs.contains(&(zone, other_zone)));
    assert!(pairs.contains(&(other_zone, zone)));
    assert!(pairs.contains(&(other_zone, seen)));
    assert!(!pairs.contains(&(zone, ignored)), "mask excludes layer 0");
    assert!(pairs.contains(&(other_zone, ignored)));
    assert_eq!(events.len(), 5);
}

#[test]
fn collider_round_trips_through_a_scene() {
    let mut registry = Registry::with_core_types();
    register_physics_types(&mut registry);
    register_physics_types(&mut registry); // twice is harmless
    let registry = Arc::new(registry);
    let mut world = World::with_registry(registry.clone());
    let collider = Collider::capsule(0.4, 0.9)
        .trigger()
        .with_offset(Vec3::new(0.0, 1.3, 0.0))
        .with_layers(0b100, 0b011);
    let uid = Uid::new();
    world
        .spawn_saved(uid, "Player", (Transform::default(), collider.clone()))
        .unwrap();
    let primitive_uid = Uid::new();
    world
        .spawn_saved(
            primitive_uid,
            "Floor",
            (Primitive::default(), Collider::from_primitive()),
        )
        .unwrap();
    let bytes = scene::save(&world).unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(
        text.contains("Collider") && text.contains("Capsule"),
        "{text}"
    );

    let loaded = scene::load(&bytes, registry.clone()).unwrap();
    let e = loaded.entity_by_uid(uid).unwrap();
    assert_eq!(*loaded.get::<Collider>(e).unwrap(), collider);
    let f = loaded.entity_by_uid(primitive_uid).unwrap();
    assert_eq!(
        *loaded.get::<Collider>(f).unwrap(),
        Collider::from_primitive()
    );
    assert_eq!(scene::save(&loaded).unwrap(), bytes, "byte-identical");

    // The registry exposes field metadata for the inspector.
    let fields = registry.component("Collider").unwrap().fields();
    let offset = fields.iter().find(|f| f.name == "offset").unwrap();
    assert!(offset.doc.is_some());
}

#[test]
fn runtime_state_is_not_saved_and_a_cloned_world_resyncs() {
    let mut registry = Registry::with_core_types();
    register_physics_types(&mut registry);
    let mut app = app();
    app.world.set_registry(Arc::new(registry));
    app.world
        .spawn_saved(
            Uid::new(),
            "Box",
            (
                Transform::from_xyz(0.0, 0.0, -5.0),
                Collider::cuboid(Vec3::ONE),
            ),
        )
        .unwrap();
    app.update();
    let clone = scene::clone_world(&app.world).unwrap();
    let mut play = App::new();
    play.world = clone;
    play.add_plugin(&PhysicsPlugin);
    play.update();
    let ray = Ray::new(Vec3::ZERO, Vec3::NEG_Z);
    assert!(physics(&play).raycast(&ray, 100.0, &ALL).is_some());
}

#[test]
fn ray_shorthand_uses_the_default_filter_and_sees_colliders_after_post_update() {
    let mut app = app();
    let cube = cube_at(&mut app, 0.0, 0.0, -5.0);
    app.world.spawn((
        Transform::from_xyz(0.0, 0.0, -2.0),
        Collider::cuboid(Vec3::ONE).trigger(),
    ));
    // Spawned this frame: not synced yet.
    assert!(physics(&app).ray(Vec3::ZERO, Vec3::NEG_Z, 100.0).is_none());
    app.update(); // PostUpdate syncs
    let hit = physics(&app).ray(Vec3::ZERO, Vec3::new(0.0, 0.0, -3.0), 100.0);
    let hit = hit.expect("the solid cube, through the trigger");
    assert_eq!(hit.entity, cube);
    assert!(approx(hit.toi, 4.5), "{}", hit.toi);
    assert!(physics(&app).ray(Vec3::ZERO, Vec3::NEG_Z, 4.0).is_none());
    assert!(physics(&app).ray(Vec3::ZERO, Vec3::ZERO, 100.0).is_none());
}

#[test]
fn non_collidable_subtrees_take_no_part_until_switched_on() {
    let mut app = app();
    let root = app.world.spawn((Transform::IDENTITY, Collidable(false)));
    let cube = cube_at(&mut app, 0.0, 0.0, -5.0);
    app.world.set_parent(cube, Some(root)).unwrap();
    app.update();
    let ray = Ray::new(Vec3::ZERO, Vec3::NEG_Z);
    assert_eq!(physics(&app).len(), 0, "a child of a non-collidable root");
    assert!(physics(&app).raycast(&ray, 100.0, &ALL).is_none());

    app.world.component_mut::<Collidable>(root).unwrap().0 = true;
    app.update();
    assert_eq!(
        physics(&app).raycast(&ray, 100.0, &ALL).unwrap().entity,
        cube
    );

    let _ = app.world.insert_one(cube, Collidable(false));
    app.update();
    assert_eq!(
        physics(&app).len(),
        0,
        "switched off on the collider itself"
    );
    assert!(physics(&app).raycast(&ray, 100.0, &ALL).is_none());
}
