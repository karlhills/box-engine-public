//! Scenes shared by the examples and the golden-image tests.
#![allow(dead_code)]

use box_engine::prelude::*;

#[cfg(feature = "dynamics")]
pub mod dynamics;
pub mod interior;
pub mod look;
pub mod materials;
pub mod scatter;
pub mod shapes;
pub mod steps;

/// Rotates an entity about +Y.
#[derive(Clone, Copy, Debug)]
pub struct Spin(pub f32);

pub fn spin_system(world: &mut World) {
    let dt = world.resource::<Time>().delta;
    for (t, s) in world.query::<(&mut Transform, &Spin)>().iter() {
        t.rotate_y(s.0 * dt);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeOfDay {
    Day,
    Evening,
}

/// Marks the sun entity.
pub struct Sun;

/// Set the environment and sun direction for a time of day.
pub fn set_time_of_day(world: &mut World, tod: TimeOfDay) {
    let (env, dir, color, intensity) = match tod {
        TimeOfDay::Day => (
            Environment::day(),
            Vec3::new(0.35, -0.7, -0.6),
            Color::linear(1.0, 0.96, 0.9),
            3.2,
        ),
        TimeOfDay::Evening => (
            Environment::evening(),
            Vec3::new(-0.8, -0.22, 0.45),
            Color::linear(1.0, 0.62, 0.36),
            2.6,
        ),
    };
    *world.resource_mut::<Environment>() = env;
    for (t, light, _) in world
        .query::<(&mut Transform, &mut DirectionalLight, &Sun)>()
        .iter()
    {
        *t = Transform::default().looking_to(dir, Vec3::Y);
        light.color = color;
        light.intensity = intensity;
    }
}

fn material(
    world: &mut World,
    color: Color,
    roughness: f32,
    metallic: f32,
) -> Handle<StandardMaterial> {
    world
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            base_color: color,
            roughness,
            metallic,
            emissive: Color::BLACK,
            ..Default::default()
        })
}

fn mesh(world: &mut World, m: MeshData) -> Handle<MeshData> {
    world.resource_mut::<Assets<MeshData>>().add(m)
}

fn spawn_mesh(
    world: &mut World,
    mesh: Handle<MeshData>,
    material: Handle<StandardMaterial>,
    t: Transform,
) -> Entity {
    world.spawn((
        t,
        MeshRenderer {
            mesh,
            materials: vec![material.into()],
            cast_shadows: true,
        },
    ))
}

/// Sun light and an orbit camera.
pub fn spawn_sun_and_camera(world: &mut World, orbit: OrbitCamera) {
    world.spawn((
        Transform::default(),
        DirectionalLight {
            shadows: true,
            shadow_distance: 70.0,
            ..Default::default()
        },
        Sun,
    ));
    world.spawn((orbit.transform(), Camera::default(), orbit));
}

/// A small stylized courtyard: a stone pillar, simple buildings, an arch,
/// columns and a few material samples on a ground plane.
pub fn build_showcase(world: &mut World) {
    let sand = material(world, Color::hex(0xc9b48f), 0.95, 0.0);
    let stone = material(world, Color::hex(0xa59a8c), 0.85, 0.0);
    let plaster = material(world, Color::hex(0xe8dcc6), 0.9, 0.0);
    let roof = material(world, Color::hex(0xa4553b), 0.7, 0.0);
    let wood = material(world, Color::hex(0x6b4a33), 0.8, 0.0);
    let green = material(world, Color::hex(0x6f8f4e), 0.9, 0.0);
    let brass = material(world, Color::hex(0xd8b25a), 0.3, 1.0);
    let glazed = material(world, Color::hex(0x3f6f8f), 0.15, 0.0);

    let ground = mesh(world, MeshData::plane(1200.0, 1200.0));
    spawn_mesh(world, ground, sand, Transform::default());

    // Central pillar.
    let pillar = mesh(world, MeshData::cylinder(2.2, 14.0, 40));
    spawn_mesh(world, pillar, stone, Transform::from_xyz(0.0, 7.0, 0.0));
    let cap = mesh(world, MeshData::cylinder(2.8, 0.6, 40));
    spawn_mesh(world, cap, stone, Transform::from_xyz(0.0, 14.3, 0.0));

    // Houses: body plus a roof made of a rotated box.
    let body = mesh(world, MeshData::cuboid(Vec3::new(4.0, 3.0, 3.0)));
    let roof_mesh = mesh(world, MeshData::cuboid(Vec3::new(4.4, 0.25, 2.3)));
    for (x, z, yaw) in [(7.0, 3.0, 0.3f32), (-6.5, 5.5, -0.5), (4.0, -7.5, 1.2)] {
        let rot = Quat::from_rotation_y(yaw);
        let base = Transform::from_xyz(x, 1.5, z).with_rotation(rot);
        spawn_mesh(world, body, plaster, base);
        for side in [-1.0f32, 1.0] {
            let tilt = Quat::from_rotation_x(side * 0.62);
            let offset = rot * Vec3::new(0.0, 3.55, side * 0.85);
            spawn_mesh(
                world,
                roof_mesh,
                roof,
                Transform::from_translation(Vec3::new(x, 0.0, z) + offset)
                    .with_rotation(rot * tilt),
            );
        }
        let door = mesh(world, MeshData::cuboid(Vec3::new(0.9, 1.8, 0.1)));
        let door_pos = Vec3::new(x, 0.9, z) + rot * Vec3::new(0.0, 0.0, 1.52);
        spawn_mesh(
            world,
            door,
            wood,
            Transform::from_translation(door_pos).with_rotation(rot),
        );
    }

    // An arch of boxes.
    let post = mesh(world, MeshData::cuboid(Vec3::new(0.7, 4.0, 0.7)));
    let lintel = mesh(world, MeshData::cuboid(Vec3::new(4.0, 0.7, 0.9)));
    spawn_mesh(world, post, stone, Transform::from_xyz(-9.0, 2.0, -4.0));
    spawn_mesh(world, post, stone, Transform::from_xyz(-6.0, 2.0, -4.0));
    spawn_mesh(world, lintel, stone, Transform::from_xyz(-7.5, 4.35, -4.0));

    // A colonnade.
    let column = mesh(world, MeshData::cylinder(0.3, 3.5, 20));
    for i in 0..7 {
        spawn_mesh(
            world,
            column,
            plaster,
            Transform::from_xyz(-10.0 + i as f32 * 2.2, 1.75, 10.0),
        );
    }

    // Shrubs and material samples.
    let shrub = mesh(world, MeshData::uv_sphere(0.9, 24, 12));
    for (x, z, s) in [
        (9.5, -2.0, 1.0f32),
        (10.5, -0.8, 0.7),
        (-3.5, 8.0, 1.2),
        (2.5, 9.0, 0.8),
    ] {
        spawn_mesh(
            world,
            shrub,
            green,
            Transform::from_xyz(x, 0.6 * s, z).with_scale(Vec3::splat(s)),
        );
    }
    let ball = mesh(world, MeshData::uv_sphere(0.8, 32, 16));
    let e = spawn_mesh(world, ball, brass, Transform::from_xyz(3.5, 0.8, 4.5));
    let _ = e;
    spawn_mesh(world, ball, glazed, Transform::from_xyz(5.5, 0.8, 5.5));
    let capsule = mesh(world, MeshData::capsule(0.45, 0.6, 24, 8));
    let spinner = spawn_mesh(
        world,
        capsule,
        roof,
        Transform::from_xyz(1.5, 1.05, 6.0).with_rotation(Quat::from_rotation_z(0.9)),
    );
    let _ = world.insert_one(spinner, Spin(0.8));
}

/// The default showcase camera.
pub fn showcase_camera() -> OrbitCamera {
    OrbitCamera {
        target: Vec3::new(0.0, 3.0, 0.0),
        distance: 34.0,
        yaw: 0.75,
        pitch: 0.42,
        ..Default::default()
    }
}

/// A grid of spheres: roughness increases left to right, metallic bottom to top.
pub fn build_material_grid(world: &mut World) {
    let ball = mesh(world, MeshData::uv_sphere(0.42, 32, 16));
    let floor_mat = material(world, Color::hex(0x8a8a8a), 0.9, 0.0);
    let floor = mesh(world, MeshData::plane(20.0, 20.0));
    spawn_mesh(world, floor, floor_mat, Transform::from_xyz(0.0, -0.5, 0.0));
    for row in 0..5 {
        for col in 0..5 {
            let m = material(
                world,
                Color::hex(0xc2502e),
                0.05 + col as f32 * 0.235,
                row as f32 / 4.0,
            );
            spawn_mesh(
                world,
                ball,
                m,
                Transform::from_xyz(col as f32 - 2.0, row as f32 * 1.0 + 0.0, 0.0),
            );
        }
    }
}

pub fn material_grid_camera() -> OrbitCamera {
    OrbitCamera {
        target: Vec3::new(0.0, 2.0, 0.0),
        distance: 8.5,
        yaw: 0.0,
        pitch: 0.12,
        ..Default::default()
    }
}
