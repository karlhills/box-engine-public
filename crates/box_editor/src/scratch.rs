//! The scratch scene opened when the editor starts without a project: a small
//! courtyard like the engine showcase. Every entity is saved data (`Uid`,
//! `Name`, `Transform`, `Primitive` + `StandardMaterial`, lights), grouped
//! under parent entities so the hierarchy has depth, and it can be saved.

use std::sync::Arc;

use box_core::prelude::*;
use box_core::reflect::Registry;
use box_core::world::Placement;
use box_engine::shared_engine_registry;
use box_render::{DebugDraw, DirectionalLight, Environment, RenderSettings, StandardMaterial};
use glam::UVec2;

/// Adds the runtime resources the editor and renderer need to a world (for
/// example a freshly loaded scene). Existing resources are kept.
pub fn prepare_world(world: &mut World) {
    world.resource_or_default::<Assets<MeshData>>();
    world.resource_or_default::<Assets<StandardMaterial>>();
    world.resource_or_default::<Environment>();
    world.resource_or_default::<RenderSettings>();
    world.resource_or_default::<DebugDraw>();
}

/// The scratch scene in a world using the engine registry. It has a sun but
/// no camera entity: the editor camera is not part of the scene.
pub fn scratch_world() -> World {
    scratch_world_with(shared_engine_registry())
}

/// The scratch scene in a world using `registry` (a game's registry, which
/// includes the engine types).
pub fn scratch_world_with(registry: Arc<Registry>) -> World {
    let mut world = World::with_registry(registry);
    world.insert_resource(box_engine::project::starter_environment());
    prepare_world(&mut world);
    build(&mut world);
    box_core::transform::propagate_transforms(&mut world);
    box_engine::sync_scene_meshes(&mut world);
    world
}

/// Fixed uids so the scratch scene (and captures of it) are reproducible.
fn next_uid(world: &World) -> Uid {
    Uid::from_u128(0x5C7A_7C00_0000_7000_8000_0000_0000_0000 + world.entity_count() as u128 + 1)
}

fn spawn(
    world: &mut World,
    name: &str,
    parent: Option<Uid>,
    components: impl box_core::world::DynamicBundle,
) -> Uid {
    let uid = next_uid(world);
    let e = world
        .spawn_saved(uid, name, components)
        .expect("fresh uids are unique");
    if parent.is_some() {
        world
            .place(e, Placement::last(parent))
            .expect("a new entity under an existing parent cannot form a cycle");
    }
    uid
}

fn material(hex: u32, roughness: f32, metallic: f32) -> StandardMaterial {
    StandardMaterial {
        base_color: Color::hex(hex),
        roughness,
        metallic,
        emissive: Color::BLACK,
        ..Default::default()
    }
}

fn shape(
    world: &mut World,
    name: &str,
    parent: Option<Uid>,
    s: Shape,
    mat: StandardMaterial,
    t: Transform,
) -> Uid {
    spawn(world, name, parent, (t, Primitive::new(s), mat))
}

fn build(world: &mut World) {
    spawn(
        world,
        "Sun",
        None,
        (
            Transform::from_xyz(0.0, 20.0, 0.0).looking_to(Vec3::new(0.35, -0.7, -0.6), Vec3::Y),
            DirectionalLight {
                color: Color::linear(1.0, 0.96, 0.9),
                intensity: 3.2,
                shadows: true,
                shadow_distance: 70.0,
            },
        ),
    );

    let sand = material(0xc9b48f, 0.95, 0.0);
    let stone = material(0xa59a8c, 0.85, 0.0);
    let plaster = material(0xe8dcc6, 0.9, 0.0);
    let roof = material(0xa4553b, 0.7, 0.0);
    let wood = material(0x6b4a33, 0.8, 0.0);
    let green = material(0x6f8f4e, 0.9, 0.0);

    let mut ground = Primitive::new(Shape::Plane {
        size: Vec2::splat(200.0),
        cells: UVec2::ONE,
    });
    ground.cast_shadows = false;
    spawn(world, "Ground", None, (Transform::default(), ground, sand));

    let pillar = shape(
        world,
        "Pillar",
        None,
        Shape::Cylinder {
            radius: 2.2,
            height: 14.0,
            segments: 40,
        },
        stone.clone(),
        Transform::from_xyz(0.0, 7.0, 0.0),
    );
    shape(
        world,
        "Cap",
        Some(pillar),
        Shape::Cylinder {
            radius: 2.8,
            height: 0.6,
            segments: 40,
        },
        stone.clone(),
        Transform::from_xyz(0.0, 7.3, 0.0),
    );

    for (i, (x, z, yaw)) in [(7.0, 3.0, 0.3f32), (-6.5, 5.5, -0.5), (4.0, -7.5, 1.2)]
        .into_iter()
        .enumerate()
    {
        let house = spawn(
            world,
            &format!("House {}", i + 1),
            None,
            (Transform::from_xyz(x, 0.0, z).with_rotation(Quat::from_rotation_y(yaw)),),
        );
        shape(
            world,
            "Walls",
            Some(house),
            Shape::Cuboid {
                size: Vec3::new(4.0, 3.0, 3.0),
            },
            plaster.clone(),
            Transform::from_xyz(0.0, 1.5, 0.0),
        );
        for (side, label) in [(-1.0f32, "Roof Back"), (1.0, "Roof Front")] {
            shape(
                world,
                label,
                Some(house),
                Shape::Cuboid {
                    size: Vec3::new(4.4, 0.25, 2.3),
                },
                roof.clone(),
                Transform::from_xyz(0.0, 3.55, side * 0.85)
                    .with_rotation(Quat::from_rotation_x(side * 0.62)),
            );
        }
        shape(
            world,
            "Door",
            Some(house),
            Shape::Cuboid {
                size: Vec3::new(0.9, 1.8, 0.1),
            },
            wood.clone(),
            Transform::from_xyz(0.0, 0.9, 1.52),
        );
    }

    let arch = spawn(world, "Arch", None, (Transform::from_xyz(-7.5, 0.0, -4.0),));
    for (name, x) in [("Post Left", -1.5f32), ("Post Right", 1.5)] {
        shape(
            world,
            name,
            Some(arch),
            Shape::Cuboid {
                size: Vec3::new(0.7, 4.0, 0.7),
            },
            stone.clone(),
            Transform::from_xyz(x, 2.0, 0.0),
        );
    }
    shape(
        world,
        "Lintel",
        Some(arch),
        Shape::Cuboid {
            size: Vec3::new(4.0, 0.7, 0.9),
        },
        stone.clone(),
        Transform::from_xyz(0.0, 4.35, 0.0),
    );

    let colonnade = spawn(
        world,
        "Colonnade",
        None,
        (Transform::from_xyz(-3.4, 0.0, 10.0),),
    );
    for i in 0..7 {
        shape(
            world,
            &format!("Column {}", i + 1),
            Some(colonnade),
            Shape::Cylinder {
                radius: 0.3,
                height: 3.5,
                segments: 20,
            },
            plaster.clone(),
            Transform::from_xyz(-6.6 + i as f32 * 2.2, 1.75, 0.0),
        );
    }

    for (i, (x, z, s)) in [
        (9.5, -2.0, 1.0f32),
        (10.5, -0.8, 0.7),
        (-3.5, 8.0, 1.2),
        (2.5, 9.0, 0.8),
    ]
    .into_iter()
    .enumerate()
    {
        shape(
            world,
            &format!("Shrub {}", i + 1),
            None,
            Shape::Sphere {
                radius: 0.9,
                sectors: 24,
                stacks: 12,
            },
            green.clone(),
            Transform::from_xyz(x, 0.6 * s, z).with_scale(Vec3::splat(s)),
        );
    }
    let ball = Shape::Sphere {
        radius: 0.8,
        sectors: 32,
        stacks: 16,
    };
    shape(
        world,
        "Brass Ball",
        None,
        ball.clone(),
        material(0xd8b25a, 0.3, 1.0),
        Transform::from_xyz(3.5, 0.8, 4.5),
    );
    shape(
        world,
        "Glazed Ball",
        None,
        ball,
        material(0x3f6f8f, 0.15, 0.0),
        Transform::from_xyz(5.5, 0.8, 5.5),
    );
    shape(
        world,
        "Capsule",
        None,
        Shape::Capsule {
            radius: 0.45,
            half_height: 0.6,
            segments: 24,
            rings: 8,
        },
        roof.clone(),
        Transform::from_xyz(1.5, 1.05, 6.0).with_rotation(Quat::from_rotation_z(0.9)),
    );
}
