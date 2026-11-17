//! Rigid-body dynamics (M7): lazy start, falling and resting, determinism,
//! stacking, joints and limits, CCD, collision events, teleports, velocity,
//! impulses, kinematic platforms, and the character controller on stairs,
//! slopes and platforms, pushing crates.

use std::hash::{Hash, Hasher};

use box_core::event::EventCursor;
use box_core::interpolate::Interpolated;
use box_core::prelude::*;
use box_physics::*;

const DT: f32 = 1.0 / 60.0;
const SKIN: f32 = 0.02;
/// Capsule centre height of a default character standing on y = 0.
const STAND: f32 = 0.9 + SKIN;

fn app() -> App {
    let mut app = App::new();
    app.add_plugin(&PhysicsPlugin);
    app.add_plugin(&CharacterPlugin);
    app
}

fn ground(app: &mut App) -> Entity {
    app.world.spawn((
        Transform::from_xyz(0.0, -0.5, 0.0),
        Collider::cuboid(Vec3::new(60.0, 1.0, 60.0)),
    ))
}

fn solid(app: &mut App, at: Vec3, size: Vec3, rotation: Quat) -> Entity {
    app.world.spawn((
        Transform::from_translation(at).with_rotation(rotation),
        Collider::cuboid(size),
    ))
}

fn body(app: &mut App, at: Vec3, size: Vec3, rb: RigidBody) -> Entity {
    app.world
        .spawn((Transform::from_translation(at), Collider::cuboid(size), rb))
}

fn pos(app: &App, e: Entity) -> Vec3 {
    app.world.get::<Transform>(e).unwrap().translation
}

fn dynamics(app: &App) -> &Dynamics {
    app.world.resource::<Dynamics>()
}

fn close(a: f32, b: f32, eps: f32) -> bool {
    (a - b).abs() <= eps
}

#[test]
fn no_simulation_without_bodies_and_it_stops_with_the_last_one() {
    let mut app = app();
    ground(&mut app);
    solid(
        &mut app,
        Vec3::new(3.0, 0.5, 0.0),
        Vec3::ONE,
        Quat::IDENTITY,
    );
    app.world.spawn((
        Transform::from_xyz(0.0, STAND, 0.0),
        KinematicCharacter::default(),
    ));
    app.run_frames(30, DT);
    assert!(
        !app.world.contains_resource::<Dynamics>(),
        "colliders and characters alone must not start rapier"
    );
    let b = body(
        &mut app,
        Vec3::new(0.0, 3.0, 3.0),
        Vec3::ONE,
        RigidBody::default(),
    );
    app.run_frames(2, DT);
    let stats = dynamics(&app).stats();
    assert_eq!(stats.bodies, 1);
    assert_eq!(stats.static_colliders, 2);
    assert_eq!(stats.characters, 1);
    assert!(
        app.world.has::<Interpolated>(b),
        "moving bodies are interpolated"
    );
    app.world.despawn(b);
    app.run_frames(2, DT);
    assert!(!app.world.contains_resource::<Dynamics>());
}

#[test]
fn a_box_falls_lands_and_sleeps() {
    let mut app = app();
    ground(&mut app);
    let b = body(
        &mut app,
        Vec3::new(0.0, 4.0, 0.0),
        Vec3::ONE,
        RigidBody::default(),
    );
    app.run_frames(30, DT);
    assert!(pos(&app, b).y < 4.0 - 0.5, "falls: {}", pos(&app, b));
    app.run_frames(240, DT);
    let p = pos(&app, b);
    assert!(
        close(p.y, 0.5, 0.02) && close(p.x, 0.0, 0.02),
        "rests on the ground: {p}"
    );
    assert_eq!(dynamics(&app).is_sleeping(b), Some(true));
    // The query world saw the move (a ray from above hits the box top).
    let hit = app
        .world
        .resource::<Physics>()
        .ray(Vec3::new(0.0, 10.0, 0.0), Vec3::NEG_Y, 20.0)
        .unwrap();
    assert_eq!(hit.entity, b);
    assert!(close(hit.point.y, 1.0, 0.02), "{hit:?}");
}

/// A pyramid of boxes, a few balls and a fast projectile.
fn busy_scene(app: &mut App) -> Vec<Entity> {
    ground(app);
    let mut out = Vec::new();
    for row in 0..5 {
        for i in 0..(5 - row) {
            let x = i as f32 * 1.02 - (4 - row) as f32 * 0.51;
            out.push(body(
                app,
                Vec3::new(x, 0.5 + row as f32 * 1.0, 0.0),
                Vec3::ONE,
                RigidBody::dynamic(2.0),
            ));
        }
    }
    for i in 0..4 {
        out.push(app.world.spawn((
            Transform::from_xyz(i as f32 * 0.3 - 0.5, 8.0 + i as f32, 0.2),
            Collider::sphere(0.3).with_material(0.4, 0.6),
            RigidBody::dynamic(1.0),
        )));
    }
    out.push(app.world.spawn((
        Transform::from_xyz(-12.0, 1.5, 0.0),
        Collider::sphere(0.2),
        RigidBody::dynamic(5.0).with_ccd(),
        Velocity::linear(Vec3::new(40.0, 2.0, 0.0)),
    )));
    out
}

fn hash_bodies(app: &App, bodies: &[Entity]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for &e in bodies {
        let t = app.world.get::<Transform>(e).unwrap();
        for v in t
            .translation
            .to_array()
            .into_iter()
            .chain(t.rotation.to_array())
        {
            v.to_bits().hash(&mut h);
        }
    }
    h.finish()
}

#[test]
fn same_inputs_give_the_same_hashes() {
    let run = || {
        let mut app = app();
        let bodies = busy_scene(&mut app);
        let mut hashes = Vec::new();
        for _ in 0..6 {
            app.run_frames(50, DT);
            hashes.push(hash_bodies(&app, &bodies));
        }
        hashes
    };
    let a = run();
    let b = run();
    assert_eq!(a, b);
    // The scene actually moved (the hashes are not all the same).
    assert!(a.windows(2).any(|w| w[0] != w[1]));
}

#[test]
fn a_stack_of_ten_boxes_stands() {
    let mut app = app();
    ground(&mut app);
    let boxes: Vec<Entity> = (0..10)
        .map(|i| {
            body(
                &mut app,
                Vec3::new(0.0, 0.5 + i as f32 * 1.0, 0.0),
                Vec3::ONE,
                RigidBody::dynamic(1.0),
            )
        })
        .collect();
    app.run_frames(300, DT);
    for (i, &b) in boxes.iter().enumerate() {
        let p = pos(&app, b);
        assert!(
            close(p.y, 0.5 + i as f32, 0.05) && Vec2::new(p.x, p.z).length() < 0.05,
            "box {i} moved to {p}"
        );
    }
    assert!(
        boxes
            .iter()
            .all(|&b| dynamics(&app).is_sleeping(b) == Some(true)),
        "the stack comes to rest"
    );
}

/// A 1 x 2 x 0.1 door hinged on its left edge to the world.
fn door(app: &mut App, joint: Joint) -> Entity {
    let e = body(
        app,
        Vec3::new(0.5, 1.5, 0.0),
        Vec3::new(1.0, 2.0, 0.1),
        RigidBody::dynamic(10.0),
    );
    app.world.insert_one(e, joint).unwrap();
    e
}

#[test]
fn hinge_limits_hold_and_free_hinges_swing() {
    for limited in [true, false] {
        let mut app = app();
        let mut joint = Joint::hinge(Vec3::new(-0.5, 0.0, 0.0), Vec3::Y);
        if limited {
            joint = joint.with_limits(-0.5, 0.5);
        }
        let d = door(&mut app, joint);
        app.run_frames(2, DT);
        // Shove the far edge hard along +Z, every step for half a second.
        let mut widest: f32 = 0.0;
        for _ in 0..120 {
            let far =
                pos(&app, d) + app.world.get::<Transform>(d).unwrap().rotation * Vec3::X * 0.5;
            if app.world.get::<Transform>(d).is_some() {
                app.world.resource_mut::<Physics>().apply_impulse_at_point(
                    d,
                    Vec3::new(0.0, 0.0, 4.0),
                    far,
                );
            }
            app.run_frames(1, DT);
            widest = widest.max(dynamics(&app).joint_position(d).unwrap().abs());
        }
        // The hinge keeps the door's edge in place and it does not fall.
        let edge = pos(&app, d)
            + app.world.get::<Transform>(d).unwrap().rotation * Vec3::new(-0.5, 0.0, 0.0);
        assert!(
            (edge - Vec3::new(0.0, 1.5, 0.0)).length() < 0.03,
            "hinge moved: {edge}"
        );
        if limited {
            assert!(widest <= 0.5 + 0.03, "limit exceeded: {widest}");
            assert!(widest > 0.45, "reached the limit: {widest}");
        } else {
            assert!(widest > 1.0, "a free hinge swings past the limit: {widest}");
        }
    }
}

#[test]
fn fixed_joint_holds_and_a_chain_hangs() {
    let mut app = app();
    let welded = body(
        &mut app,
        Vec3::new(5.0, 3.0, 0.0),
        Vec3::ONE,
        RigidBody::default(),
    );
    app.world
        .insert_one(welded, Joint::fixed(Vec3::ZERO))
        .unwrap();
    // Five links along +X from an anchor at the origin; they swing down.
    let mut links = Vec::new();
    for i in 0..5 {
        let uid = Uid::new();
        let link = app.world.spawn((
            uid,
            Transform::from_xyz(0.3 + i as f32 * 0.6, 4.0, 0.0),
            Collider::cuboid(Vec3::new(0.5, 0.1, 0.1)),
            RigidBody::dynamic(0.5).with_damping(0.5, 0.5),
        ));
        let joint = match links.last() {
            None => Joint::ball(Vec3::new(-0.3, 0.0, 0.0)),
            Some(&(_, prev)) => Joint::ball(Vec3::new(-0.3, 0.0, 0.0)).connected_to(prev),
        };
        app.world.insert_one(link, joint).unwrap();
        links.push((link, uid));
    }
    // Ends of a link, from its transform.
    let ends = |app: &App, e: Entity| {
        let t = *app.world.get::<Transform>(e).unwrap();
        (
            t.translation + t.rotation * Vec3::new(-0.3, 0.0, 0.0),
            t.translation + t.rotation * Vec3::new(0.3, 0.0, 0.0),
        )
    };
    for _ in 0..20 {
        app.run_frames(60, DT);
        // Every ball joint holds: each link starts where the previous ends.
        let mut prev = Vec3::new(0.0, 4.0, 0.0);
        for (i, &(link, _)) in links.iter().enumerate() {
            let (start, end) = ends(&app, link);
            assert!(
                (start - prev).length() < 0.02,
                "link {i} came apart: {start} vs {prev}"
            );
            prev = end;
        }
    }
    let w = pos(&app, welded);
    assert!(
        (w - Vec3::new(5.0, 3.0, 0.0)).length() < 0.01,
        "weld fell: {w}"
    );
    let last = pos(&app, links[4].0);
    // Swung down and settling under the anchor (2.7 m below it when still).
    assert!(
        last.x.abs() < 0.2 && close(last.y, 4.0 - 2.7, 0.1),
        "chain end at {last}"
    );
    assert_eq!(dynamics(&app).stats().joints, 6);
}

#[test]
fn prismatic_travel_is_limited() {
    let mut app = app();
    let e = body(
        &mut app,
        Vec3::new(0.0, 3.0, 0.0),
        Vec3::splat(0.5),
        RigidBody::default(),
    );
    app.world
        .insert_one(
            e,
            Joint::prismatic(Vec3::ZERO, Vec3::Y).with_limits(-1.0, 0.0),
        )
        .unwrap();
    app.run_frames(180, DT);
    let p = pos(&app, e);
    assert!(
        close(p.y, 2.0, 0.03) && close(p.x, 0.0, 1e-3),
        "slid to {p}"
    );
    assert!(close(dynamics(&app).joint_position(e).unwrap(), -1.0, 0.03));
}

#[test]
fn fast_projectiles_never_tunnel_through_static_walls() {
    let mut app = app();
    solid(
        &mut app,
        Vec3::new(10.0, 0.0, 0.0),
        Vec3::new(0.05, 4.0, 4.0),
        Quat::IDENTITY,
    );
    let mut rb = RigidBody::dynamic(0.1);
    rb.gravity_scale = 0.0;
    let bullet = app.world.spawn((
        Transform::default(),
        Collider::sphere(0.05),
        rb,
        Velocity::linear(Vec3::new(400.0, 0.0, 0.0)),
    ));
    app.run_frames(10, DT);
    let x = pos(&app, bullet).x;
    assert!(x < 10.0, "went through a static wall: {x}");
}

#[test]
fn ccd_stops_a_fast_projectile_at_a_thin_moving_wall() {
    for ccd in [true, false] {
        let mut app = app();
        app.world.spawn((
            Transform::from_xyz(10.0, 0.0, 0.0),
            Collider::cuboid(Vec3::new(0.1, 4.0, 4.0)),
            RigidBody::kinematic(),
        ));
        let mut rb = RigidBody::dynamic(0.1);
        rb.gravity_scale = 0.0;
        rb.ccd = ccd;
        let bullet = app.world.spawn((
            Transform::default(),
            Collider::sphere(0.05),
            rb,
            Velocity::linear(Vec3::new(200.0, 0.0, 0.0)),
        ));
        app.run_frames(10, DT);
        let x = pos(&app, bullet).x;
        if ccd {
            assert!(x < 10.0, "CCD projectile went through: {x}");
        } else {
            assert!(
                x > 10.0,
                "without CCD it tunnels (the test is meaningful): {x}"
            );
        }
    }
}

#[test]
fn collisions_report_start_with_impulse_and_stop() {
    let mut app = app();
    let floor = ground(&mut app);
    let b = body(
        &mut app,
        Vec3::new(0.0, 2.0, 0.0),
        Vec3::ONE,
        RigidBody::default(),
    );
    let mut cursor = EventCursor::<CollisionEvent>::default();
    let mut started = None;
    for _ in 0..60 {
        app.run_frames(1, DT);
        for e in cursor.read(
            app.world
                .resource::<box_core::event::Events<CollisionEvent>>(),
        ) {
            if e.kind == CollisionKind::Started && started.is_none() {
                started = Some(*e);
            }
        }
    }
    let started = started.expect("a Started event");
    assert!(
        started.involves(b) && started.other(b) == Some(floor),
        "{started:?}"
    );
    assert!(started.impulse > 0.5, "landing impulse: {started:?}");
    // Teleport the box up: the contact stops.
    app.world
        .component_mut::<Transform>(b)
        .unwrap()
        .translation
        .y = 5.0;
    let mut stopped = false;
    for _ in 0..3 {
        app.run_frames(1, DT);
        stopped |= cursor
            .read(
                app.world
                    .resource::<box_core::event::Events<CollisionEvent>>(),
            )
            .any(|e| e.kind == CollisionKind::Stopped && e.involves(b) && e.impulse == 0.0);
    }
    assert!(stopped);
}

#[test]
fn teleport_velocity_and_impulses() {
    let mut app = app();
    ground(&mut app);
    let b = body(
        &mut app,
        Vec3::new(0.0, 0.5, 0.0),
        Vec3::ONE,
        RigidBody::dynamic(2.0),
    );
    app.world.insert_one(b, Velocity::default()).unwrap();
    app.run_frames(120, DT);
    // Teleport.
    app.world.component_mut::<Transform>(b).unwrap().translation = Vec3::new(4.0, 0.5, 0.0);
    app.run_frames(1, DT);
    assert!(close(pos(&app, b).x, 4.0, 1e-3), "{}", pos(&app, b));
    // Written velocity is applied, then simulated.
    app.world.component_mut::<Velocity>(b).unwrap().linear = Vec3::new(0.0, 5.0, 0.0);
    app.run_frames(1, DT);
    let v = *app.world.get::<Velocity>(b).unwrap();
    assert!(
        v.linear.y > 4.5 && v.linear.y < 5.0,
        "gravity acts on the set velocity: {v:?}"
    );
    app.run_frames(120, DT);
    // An impulse of 2 kg * 3 m/s along +X.
    app.world
        .resource_mut::<Physics>()
        .apply_impulse(b, Vec3::new(6.0, 0.0, 0.0));
    app.run_frames(1, DT);
    let v = app.world.get::<Velocity>(b).unwrap().linear;
    assert!(close(v.x, 3.0, 0.3), "{v}");
    // Forces last one step: 120 N on 2 kg for 1/60 s = 1 m/s.
    let before = app.world.get::<Velocity>(b).unwrap().linear.y;
    app.world
        .resource_mut::<Physics>()
        .add_force(b, Vec3::new(0.0, 120.0 + 2.0 * 9.81, 0.0));
    app.run_frames(1, DT);
    let after = app.world.get::<Velocity>(b).unwrap().linear.y;
    assert!(close(after - before, 1.0, 0.1), "{before} -> {after}");
}

#[test]
fn a_kinematic_platform_carries_a_box_and_a_character() {
    let mut app = app();
    ground(&mut app);
    let platform = app.world.spawn((
        Transform::from_xyz(0.0, 1.0, 0.0),
        Collider::cuboid(Vec3::new(4.0, 0.2, 4.0)),
        RigidBody::kinematic(),
        Velocity::default(),
    ));
    let crate_ = body(
        &mut app,
        Vec3::new(1.0, 1.6, 1.0),
        Vec3::ONE,
        RigidBody::dynamic(5.0),
    );
    let ch = app.world.spawn((
        Transform::from_xyz(-1.0, 1.1 + STAND, -1.0),
        KinematicCharacter::default(),
    ));
    app.run_frames(60, DT);
    assert!(app.world.get::<KinematicCharacter>(ch).unwrap().grounded);
    let (c0, b0) = (pos(&app, ch), pos(&app, crate_));
    app.world
        .component_mut::<Velocity>(platform)
        .unwrap()
        .linear = Vec3::new(1.0, 0.5, 0.0);
    app.run_frames(60, DT);
    let p = pos(&app, platform);
    assert!(
        close(p.x, 1.0, 0.02) && close(p.y, 1.5, 0.02),
        "platform at {p}"
    );
    let (c1, b1) = (pos(&app, ch), pos(&app, crate_));
    assert!(
        close(c1.x - c0.x, 1.0, 0.05) && close(c1.y - c0.y, 0.5, 0.05),
        "character {c0} -> {c1}"
    );
    assert!(
        close(b1.x - b0.x, 1.0, 0.1) && close(b1.y - b0.y, 0.5, 0.05),
        "crate {b0} -> {b1}"
    );
    assert!(app.world.get::<KinematicCharacter>(ch).unwrap().grounded);
}

fn character(app: &mut App, at: Vec3) -> Entity {
    let e = app.world.spawn((
        Transform::from_translation(at),
        KinematicCharacter::default(),
    ));
    app.run_frames(10, DT);
    e
}

fn walk(app: &mut App, e: Entity, v: Vec3, frames: usize) {
    for _ in 0..frames {
        if let Some(c) = app.world.component_mut::<KinematicCharacter>(e) {
            c.velocity.x = v.x;
            c.velocity.z = v.z;
        }
        app.run_frames(1, DT);
    }
    if let Some(c) = app.world.component_mut::<KinematicCharacter>(e) {
        c.velocity.x = 0.0;
        c.velocity.z = 0.0;
    }
}

/// Six 0.25 m steps, 0.4 m deep, from x = 2 up to a landing at 1.5 m.
fn stairs(app: &mut App, rise: f32) {
    for i in 0..6 {
        let top = rise * (i + 1) as f32;
        solid(
            app,
            Vec3::new(2.2 + i as f32 * 0.4, top * 0.5, 0.0),
            Vec3::new(0.4, top, 3.0),
            Quat::IDENTITY,
        );
    }
    let top = rise * 6.0;
    solid(
        app,
        Vec3::new(6.4, top * 0.5, 0.0),
        Vec3::new(3.0, top, 3.0),
        Quat::IDENTITY,
    );
    app.update();
}

#[test]
fn the_character_climbs_stairs_and_comes_back_down() {
    let mut app = app();
    ground(&mut app);
    stairs(&mut app, 0.25);
    let e = character(&mut app, Vec3::new(0.0, STAND, 0.0));
    walk(&mut app, e, Vec3::new(3.0, 0.0, 0.0), 150);
    let top = pos(&app, e);
    assert!(
        top.x > 5.5 && close(top.y, 1.5 + STAND, 0.02),
        "on the landing: {top}"
    );
    assert!(app.world.get::<KinematicCharacter>(e).unwrap().grounded);
    // Down again without ever leaving the ground for long.
    let mut airborne = 0;
    for _ in 0..120 {
        walk(&mut app, e, Vec3::new(-3.0, 0.0, 0.0), 1);
        if !app.world.get::<KinematicCharacter>(e).unwrap().grounded {
            airborne += 1;
        }
    }
    let bottom = pos(&app, e);
    assert!(
        bottom.x < 1.9 && close(bottom.y, STAND, 0.02),
        "back down: {bottom}"
    );
    assert!(
        airborne <= 2,
        "followed the steps down ({airborne} airborne steps)"
    );
}

#[test]
fn steps_higher_than_step_height_block() {
    let mut app = app();
    ground(&mut app);
    stairs(&mut app, 0.45);
    let e = character(&mut app, Vec3::new(0.0, STAND, 0.0));
    walk(&mut app, e, Vec3::new(3.0, 0.0, 0.0), 90);
    let p = pos(&app, e);
    assert!(
        p.x < 2.0 - 0.4 + 0.01 && close(p.y, STAND, 0.02),
        "blocked at the first step: {p}"
    );
}

#[test]
fn walkable_slopes_hold_and_steep_slopes_slide() {
    // 25 degrees: stand still without sliding, walk up.
    let mut app = app();
    ground(&mut app);
    let a = 25f32.to_radians();
    solid(
        &mut app,
        Vec3::new(4.0, 0.0, 0.0),
        Vec3::new(8.0, 1.0, 4.0),
        Quat::from_rotation_z(a),
    );
    app.update();
    let e = character(&mut app, Vec3::new(4.0, 3.0, 0.0));
    app.run_frames(60, DT);
    let rest = pos(&app, e);
    app.run_frames(60, DT);
    assert!(
        (pos(&app, e) - rest).length() < 1e-3,
        "slid on a walkable slope: {rest} -> {}",
        pos(&app, e)
    );
    assert!(app.world.get::<KinematicCharacter>(e).unwrap().grounded);
    walk(&mut app, e, Vec3::new(2.0, 0.0, 0.0), 30);
    assert!(
        pos(&app, e).y > rest.y + 0.3,
        "walks up: {rest} -> {}",
        pos(&app, e)
    );

    // 65 degrees (steeper than max_slope): slides down to the floor.
    let mut app = crate::app();
    ground(&mut app);
    let a = 65f32.to_radians();
    solid(
        &mut app,
        Vec3::new(4.0, 0.0, 0.0),
        Vec3::new(8.0, 1.0, 4.0),
        Quat::from_rotation_z(a),
    );
    app.update();
    // The upper face runs from (3.55, 0.21) up and to the right at 65 degrees.
    let e = character(&mut app, Vec3::new(3.8, 4.0, 0.0));
    let start = pos(&app, e);
    app.run_frames(120, DT);
    let end = pos(&app, e);
    assert!(
        end.y < start.y - 1.0 && end.x < start.x,
        "slid down: {start} -> {end}"
    );
    // And it cannot walk up it.
    walk(&mut app, e, Vec3::new(3.0, 0.0, 0.0), 60);
    assert!(
        pos(&app, e).y < start.y,
        "climbed a steep slope: {}",
        pos(&app, e)
    );
}

#[test]
fn the_character_pushes_a_crate_by_its_mass() {
    let mut moved = Vec::new();
    for mass in [80.0, 0.0] {
        let mut app = app();
        ground(&mut app);
        let crate_ = body(
            &mut app,
            Vec3::new(2.0, 0.5, 0.0),
            Vec3::ONE,
            RigidBody::dynamic(20.0),
        );
        let e = app.world.spawn((
            Transform::from_xyz(0.0, STAND, 0.0),
            KinematicCharacter {
                mass,
                ..KinematicCharacter::default()
            },
        ));
        app.run_frames(30, DT);
        let start = pos(&app, crate_);
        walk(&mut app, e, Vec3::new(2.0, 0.0, 0.0), 120);
        let end = pos(&app, crate_);
        let ch = pos(&app, e);
        assert!(
            ch.x < end.x - 0.5 - 0.4 + 0.01,
            "the character never enters the crate: {ch} {end}"
        );
        assert!(close(end.y, 0.5, 0.03), "slid, did not tip or fly: {end}");
        moved.push(end.x - start.x);
    }
    assert!(moved[0] > 1.5, "pushed: {moved:?}");
    assert!(
        moved[1].abs() < 0.01,
        "mass 0 pushes nothing (the kinematic capsule does not shove): {moved:?}"
    );
}

#[test]
fn queries_see_bodies_at_this_steps_pose() {
    let mut app = app();
    ground(&mut app);
    let b = body(
        &mut app,
        Vec3::new(0.0, 6.0, 0.0),
        Vec3::ONE,
        RigidBody::default(),
    );
    app.run_frames(20, DT);
    // In FixedUpdate after the step, a ray finds the box where it is now.
    app.add_system(Stage::FixedUpdate, move |w: &mut World| {
        let y = w.get::<Transform>(b).unwrap().translation.y;
        let hit = w
            .resource::<Physics>()
            .ray(Vec3::new(0.0, 20.0, 0.0), Vec3::NEG_Y, 40.0)
            .unwrap();
        assert!(
            close(hit.point.y, y + 0.5, 1e-3),
            "query lagged: {} vs {}",
            hit.point.y,
            y + 0.5
        );
    });
    app.run_frames(10, DT);
}

#[test]
fn despawned_and_removed_bodies_leave_the_simulation() {
    let mut app = app();
    ground(&mut app);
    let a = body(
        &mut app,
        Vec3::new(0.0, 0.5, 0.0),
        Vec3::ONE,
        RigidBody::default(),
    );
    let b = body(
        &mut app,
        Vec3::new(3.0, 0.5, 0.0),
        Vec3::ONE,
        RigidBody::default(),
    );
    let c = body(
        &mut app,
        Vec3::new(6.0, 0.5, 0.0),
        Vec3::ONE,
        RigidBody::default(),
    );
    app.run_frames(2, DT);
    assert_eq!(dynamics(&app).stats().bodies, 3);
    app.world.despawn(a);
    app.world.remove_one::<RigidBody>(b).unwrap();
    app.run_frames(1, DT);
    let d = dynamics(&app);
    assert_eq!(d.stats().bodies, 1);
    assert!(d.has_body(c) && !d.has_body(a) && !d.has_body(b));
    // `b` is now a static collider.
    assert_eq!(d.stats().static_colliders, 2);
}

// ---- mesh colliders (M8: imported level geometry) ---------------------------

/// One mesh of cuboids (centre, size): a floor and a stair run, as an
/// imported level node would be.
fn level_mesh(boxes: &[(Vec3, Vec3)]) -> MeshData {
    let mut out = MeshData::default();
    for &(at, size) in boxes {
        let m = MeshData::cuboid(size);
        let base = out.positions.len() as u32;
        out.positions.extend(m.positions.iter().map(|p| *p + at));
        out.normals.extend_from_slice(&m.normals);
        out.indices.extend(m.indices.iter().map(|i| i + base));
    }
    out
}

/// Adds `mesh` to the world's mesh store and asset lookup as `path`.
fn add_mesh(app: &mut App, path: &str, mesh: MeshData) -> AssetRef {
    let handle = app
        .world
        .resource_or_default::<Assets<MeshData>>()
        .add(mesh);
    let r = AssetRef::path(path);
    app.world.resource_or_default::<AssetLookup>().insert(
        AssetId::new(),
        r.label.as_deref(),
        r.path.as_deref(),
        handle,
    );
    r
}

fn mesh_entity(app: &mut App, at: Vec3, mesh: AssetRef) -> Entity {
    app.world.spawn((
        Transform::from_translation(at),
        MeshAsset {
            mesh,
            ..MeshAsset::default()
        },
        Collider::new(ColliderShape::Mesh),
    ))
}

/// Floor 40 x 40 at y = 0, six 0.25 m steps from x = 2 to a landing at
/// 1.5 m: the stairs test's layout, as one triangle mesh.
fn stair_level() -> MeshData {
    let mut boxes = vec![(Vec3::new(0.0, -0.5, 0.0), Vec3::new(40.0, 1.0, 40.0))];
    for i in 0..6 {
        let top = 0.25 * (i + 1) as f32;
        boxes.push((
            Vec3::new(2.2 + i as f32 * 0.4, top * 0.5, 0.0),
            Vec3::new(0.4, top, 3.0),
        ));
    }
    boxes.push((Vec3::new(6.4, 0.75, 0.0), Vec3::new(3.0, 1.5, 3.0)));
    level_mesh(&boxes)
}

#[test]
fn mesh_colliders_hold_characters_bodies_and_queries() {
    let mut app = app();
    let level = add_mesh(&mut app, "level.glb#Mesh/Floor", stair_level());
    let floor = mesh_entity(&mut app, Vec3::ZERO, level);
    app.run_frames(1, DT);
    // Queries: a ray from above hits the top of the floor and the landing.
    let physics = app.world.resource::<Physics>();
    let hit = physics
        .ray(Vec3::new(-5.0, 5.0, 0.0), Vec3::NEG_Y, 10.0)
        .unwrap();
    assert_eq!(hit.entity, floor);
    assert!(close(hit.point.y, 0.0, 1e-4), "{:?}", hit.point);
    let hit = physics
        .ray(Vec3::new(6.4, 5.0, 0.0), Vec3::NEG_Y, 10.0)
        .unwrap();
    assert!(close(hit.point.y, 1.5, 1e-4), "{:?}", hit.point);

    // A character climbs the mesh stairs as it climbs separate boxes.
    let e = character(&mut app, Vec3::new(0.0, STAND, 0.0));
    walk(&mut app, e, Vec3::new(3.0, 0.0, 0.0), 150);
    let top = pos(&app, e);
    assert!(
        top.x > 5.5 && close(top.y, 1.5 + STAND, 0.02),
        "on the landing: {top}"
    );
    // And walks across the floor's triangle seams without catching.
    walk(&mut app, e, Vec3::new(-3.0, 0.0, 0.0), 200);
    walk(&mut app, e, Vec3::new(0.0, 0.0, 3.0), 100);
    let p = pos(&app, e);
    assert!(close(p.y, STAND, 0.02) && p.z > 4.9, "{p}");

    // A dynamic crate falls onto the mesh floor and rests on it.
    let b = body(
        &mut app,
        Vec3::new(-6.0, 3.0, -6.0),
        Vec3::ONE,
        RigidBody::default(),
    );
    app.run_frames(150, DT);
    let rest = pos(&app, b);
    assert!(close(rest.y, 0.5, 0.03), "resting at {rest}");
    assert_eq!(dynamics(&app).stats().static_colliders, 1);
}

#[test]
fn mesh_colliders_appear_when_the_mesh_loads_and_follow_scale() {
    let mut app = app();
    // A body so the simulation runs; the mesh is not loaded yet.
    let falling = body(
        &mut app,
        Vec3::new(14.0, 4.0, 0.0),
        Vec3::ONE,
        RigidBody::default(),
    );
    let e = mesh_entity(&mut app, Vec3::ZERO, AssetRef::path("late.glb#Mesh/Floor"));
    app.world.get_mut::<Transform>(e).unwrap().scale = Vec3::new(2.0, 1.0, 2.0);
    app.run_frames(2, DT);
    let p = app.world.resource::<Physics>();
    assert!(
        p.ray(Vec3::new(18.0, 5.0, 0.0), Vec3::NEG_Y, 10.0)
            .is_none()
    );
    // Loads: a 20 m slab, 40 m wide at the entity's scale.
    add_mesh(
        &mut app,
        "late.glb#Mesh/Floor",
        level_mesh(&[(Vec3::new(0.0, -0.5, 0.0), Vec3::new(20.0, 1.0, 20.0))]),
    );
    app.run_frames(150, DT);
    let p = app.world.resource::<Physics>();
    let hit = p.ray(Vec3::new(18.0, 5.0, 0.0), Vec3::NEG_Y, 10.0).unwrap();
    assert_eq!(hit.entity, e);
    assert!(close(hit.point.y, 0.0, 1e-4));
    assert!(
        close(pos(&app, falling).y, 0.5, 0.03),
        "{}",
        pos(&app, falling)
    );
}

#[test]
fn a_mesh_collider_on_a_moving_body_is_not_simulated() {
    let mut app = app();
    ground(&mut app);
    let r = add_mesh(
        &mut app,
        "rock.glb#Mesh/Rock",
        level_mesh(&[(Vec3::ZERO, Vec3::ONE)]),
    );
    let e = mesh_entity(&mut app, Vec3::new(0.0, 3.0, 0.0), r);
    app.world.insert_one(e, RigidBody::default()).unwrap();
    app.run_frames(60, DT);
    // No collider in the simulation: it falls through the ground.
    assert!(pos(&app, e).y < -1.0, "{}", pos(&app, e));
}

/// A `ConvexHull` collider on a dynamic body (an imported prop) is solid:
/// the body falls, lands on the ground at its hull's half height and is
/// found by queries; scale applies (M8 gap report item 1).
#[test]
fn a_convex_hull_collider_simulates_on_a_dynamic_body() {
    let mut app = app();
    ground(&mut app);
    // A 1 x 2 x 1 box made of two stacked cubes (shared faces inside: the
    // hull fills them).
    let r = add_mesh(
        &mut app,
        "props.glb#Mesh/Crate",
        level_mesh(&[
            (Vec3::new(0.0, -0.5, 0.0), Vec3::ONE),
            (Vec3::new(0.0, 0.5, 0.0), Vec3::ONE),
        ]),
    );
    let e = app.world.spawn((
        Transform::from_xyz(0.0, 4.0, 0.0).with_scale(Vec3::splat(0.5)),
        MeshAsset {
            mesh: r,
            ..MeshAsset::default()
        },
        Collider::new(ColliderShape::ConvexHull),
        RigidBody::default(),
    ));
    app.run_frames(150, DT);
    let rest = pos(&app, e);
    // Half height 1 m at scale 0.5.
    assert!(close(rest.y, 0.5, 0.03), "resting at {rest}");
    assert_eq!(
        dynamics(&app).stats().static_colliders,
        1,
        "only the ground"
    );
    let hit = app
        .world
        .resource::<Physics>()
        .ray(Vec3::new(0.0, 5.0, 0.0), Vec3::NEG_Y, 10.0)
        .unwrap();
    assert_eq!(hit.entity, e);
    assert!(close(hit.point.y, 1.0, 0.03), "{:?}", hit.point);
    let inside = app
        .world
        .resource::<Physics>()
        .contains_point(rest, &QueryFilter::default());
    assert_eq!(inside, [e], "a hull is solid");

    // A flat mesh has no hull: no collider, no panic.
    let flat = add_mesh(
        &mut app,
        "props.glb#Mesh/Sheet",
        MeshData::grid(1.0, 1.0, 1, 1),
    );
    let sheet = app.world.spawn((
        Transform::from_xyz(5.0, 2.0, 0.0),
        MeshAsset {
            mesh: flat,
            ..MeshAsset::default()
        },
        Collider::new(ColliderShape::ConvexHull),
    ));
    app.run_frames(2, DT);
    let miss = app
        .world
        .resource::<Physics>()
        .ray(Vec3::new(5.0, 5.0, 0.0), Vec3::NEG_Y, 3.5);
    assert!(miss.is_none_or(|h| h.entity != sheet));
}
