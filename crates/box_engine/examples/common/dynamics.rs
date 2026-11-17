//! The dynamics demo scene, used by `examples/dynamics.rs` and the dynamics
//! tests: a stack of boxes knocked over by a ball rolling down a ramp, a
//! hinged door swung open by a thrown ball, a swinging chain, and a
//! kinematic character walking up stairs and pushing a crate off the
//! landing, on scripted input. Everything is built from saved components
//! (`Primitive`, `Collider`, `RigidBody`, `Joint`, `Velocity`,
//! `KinematicCharacter`), the same data a scene file holds.

#![allow(dead_code)]

use box_engine::prelude::*;

/// The character's scripted walk: for `WALK_TICKS` fixed steps from
/// `WALK_FROM_TICK` on, at `WALK_SPEED`, which takes it from x = -5.5 to
/// about x = 3.4, near the landing's edge (x = 4.0).
pub const WALK_FROM_TICK: u64 = 20;
pub const WALK_TICKS: u64 = 222;
pub const WALK_SPEED: f32 = 2.4;
/// Height of the stair landing.
pub const LANDING: f32 = 1.2;

/// The scene, its scripted character and the character plugin.
pub struct DynamicsDemo;

impl Plugin for DynamicsDemo {
    fn build(&self, app: &mut App) {
        build_scene(&mut app.world);
        app.add_plugin(&CharacterPlugin);
        app.system(Stage::FixedUpdate, scripted_walk)
            .before("move_characters");
    }
}

/// The orbit camera the example and captures use.
pub fn dynamics_camera() -> OrbitCamera {
    OrbitCamera {
        target: Vec3::new(-1.0, 1.0, -0.5),
        distance: 14.0,
        yaw: 0.3,
        pitch: 0.45,
        ..Default::default()
    }
}

fn material(hex: u32) -> StandardMaterial {
    StandardMaterial {
        base_color: Color::hex(hex),
        roughness: 0.8,
        ..Default::default()
    }
}

/// A static box: rendered and collidable.
fn block(world: &mut World, name: &str, at: Vec3, size: Vec3, rotation: Quat, hex: u32) -> Entity {
    world.spawn((
        Name::new(name),
        Transform::from_translation(at).with_rotation(rotation),
        Primitive::cuboid(size),
        material(hex),
        Collider::from_primitive(),
    ))
}

/// A dynamic box of `mass` kilograms.
fn crate_box(world: &mut World, name: &str, at: Vec3, size: f32, mass: f32, hex: u32) -> Entity {
    world.spawn((
        Name::new(name),
        Transform::from_translation(at),
        Primitive::cuboid(Vec3::splat(size)),
        material(hex),
        Collider::from_primitive(),
        RigidBody::dynamic(mass),
    ))
}

fn ball(world: &mut World, name: &str, at: Vec3, radius: f32, mass: f32, velocity: Vec3) -> Entity {
    world.spawn((
        Name::new(name),
        Transform::from_translation(at),
        Primitive::sphere(radius),
        material(0xc0504d),
        Collider::from_primitive().with_material(0.6, 0.2),
        RigidBody::dynamic(mass),
        Velocity::linear(velocity),
    ))
}

pub fn build_scene(world: &mut World) {
    world.spawn((
        Name::new("Ground"),
        Transform::default(),
        Primitive::plane(Vec2::new(30.0, 20.0)),
        material(0x9fb08a),
        Collider::cuboid(Vec3::new(30.0, 0.0, 20.0)),
    ));

    // A ramp on the left with a heavy ball rolling down into a box stack.
    block(
        world,
        "Ramp",
        Vec3::new(-8.6, 1.1, -2.0),
        Vec3::new(5.0, 0.2, 2.0),
        Quat::from_rotation_z(-0.35),
        0xb9b0a2,
    );
    ball(
        world,
        "Boulder",
        Vec3::new(-10.25, 2.35, -2.0),
        0.45,
        20.0,
        Vec3::ZERO,
    );
    for i in 0..6 {
        let hex = if i % 2 == 0 { 0xd8a55a } else { 0xc98f4a };
        crate_box(
            world,
            &format!("Stack {i}"),
            Vec3::new(-4.6, 0.35 + i as f32 * 0.7, -2.0),
            0.7,
            2.0,
            hex,
        );
    }

    // A hinged door between two posts, swung open by a thrown ball. The
    // posts are on layer 2 and the door does not collide with that layer,
    // so its edge can turn against the post it hangs on.
    const POSTS: u32 = 1 << 2;
    for (name, x) in [("Post L", 1.4), ("Post R", 3.0)] {
        let post = block(
            world,
            name,
            Vec3::new(x, 1.2, -3.0),
            Vec3::new(0.2, 2.4, 0.2),
            Quat::IDENTITY,
            0x7d6b5a,
        );
        if let Some(c) = world.component_mut::<Collider>(post) {
            c.layers = POSTS;
        }
    }
    world.spawn((
        Name::new("Door"),
        Transform::from_xyz(2.2, 1.15, -3.0),
        Primitive::cuboid(Vec3::new(1.36, 2.2, 0.08)),
        material(0x8a5a3c),
        Collider::from_primitive().with_layers(Collider::DEFAULT_LAYERS, !POSTS),
        RigidBody::dynamic(8.0).with_damping(0.0, 1.0),
        // Hinged on its left edge, about Y, opening up to 70 degrees.
        Joint::hinge(Vec3::new(-0.7, 0.0, 0.0), Vec3::Y).with_limits(-1.2, 1.2),
    ));
    ball(
        world,
        "Thrown ball",
        Vec3::new(2.6, 1.0, 1.0),
        0.25,
        1.0,
        Vec3::new(0.0, 1.5, -8.0),
    );

    // A chain of capsule links hanging from a beam, released from
    // horizontal. Links are capsules along Y laid along X; in a link's own
    // frame its ends are at ±0.25 along Y, and +Y points to -X.
    block(
        world,
        "Beam",
        Vec3::new(-1.5, 4.6, -4.0),
        Vec3::new(0.3, 0.3, 1.2),
        Quat::IDENTITY,
        0x5a5f66,
    );
    let mut previous: Option<Uid> = None;
    for i in 0..8 {
        let uid = Uid::new();
        let joint = Joint::ball(Vec3::new(0.0, 0.25, 0.0));
        world.spawn((
            uid,
            Name::new(format!("Link {i}")),
            Transform::from_xyz(-1.05 + i as f32 * 0.5, 4.6, -4.0)
                .with_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2)),
            Primitive::capsule(0.07, 0.18),
            material(0x9aa3ad),
            Collider::from_primitive(),
            RigidBody::dynamic(0.4).with_damping(0.05, 0.2),
            match previous {
                Some(p) => joint.connected_to(p),
                None => joint,
            },
        ));
        previous = Some(uid);
    }

    // Stairs up to a landing with a crate, for the character.
    let z = 3.0;
    let (rise, run) = (LANDING / 6.0, 0.4);
    for i in 0..6 {
        let top = rise * (i + 1) as f32;
        block(
            world,
            &format!("Step {i}"),
            Vec3::new(-1.8 + i as f32 * run, top * 0.5, z),
            Vec3::new(run, top, 2.0),
            Quat::IDENTITY,
            0xb9b0a2,
        );
    }
    block(
        world,
        "Landing",
        Vec3::new(2.2, LANDING * 0.5, z),
        Vec3::new(3.6, LANDING, 2.0),
        Quat::IDENTITY,
        0xa89f90,
    );
    crate_box(
        world,
        "Crate",
        Vec3::new(2.0, LANDING + 0.45, z),
        0.9,
        15.0,
        0xd8a55a,
    );

    // The character: a capsule (1.8 m, radius 0.4) standing on y = 0.
    world.spawn((
        Name::new("Character"),
        Transform::from_xyz(-5.5, 0.92, z),
        Primitive::capsule(0.4, 0.5),
        material(0x4f81bd),
        KinematicCharacter::default(),
        Interpolated::default(),
    ));
}

/// Walk the character along +X: up the stairs, then into the crate,
/// pushing it off the landing, and stop near the edge.
pub fn scripted_walk(world: &mut World) {
    let tick = world.resource::<FixedTime>().tick;
    let walking = (WALK_FROM_TICK..WALK_FROM_TICK + WALK_TICKS).contains(&tick);
    let speed = if walking { WALK_SPEED } else { 0.0 };
    for c in world.query_mut::<&mut KinematicCharacter>() {
        c.velocity.x = speed;
        c.velocity.z = 0.0;
    }
}
