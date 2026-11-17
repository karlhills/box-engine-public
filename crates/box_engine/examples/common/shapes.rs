//! The built-in generators side by side (`examples/shapes.rs` and the
//! `builtin_shapes` golden image): primitives in front, lathes in the
//! middle, extrusions at the back, on a primitive ground plane. Every
//! object goes through the generator runtime.

use std::f32::consts::{FRAC_PI_2, PI};

use box_engine::prelude::*;

fn material(hex: u32, roughness: f32) -> StandardMaterial {
    StandardMaterial {
        base_color: Color::hex(hex),
        roughness,
        ..Default::default()
    }
}

/// The lathe cases of the gallery, left to right (also golden fixtures).
pub fn lathes() -> Vec<(&'static str, Lathe)> {
    vec![
        ("vase", Lathe::default()),
        (
            "column",
            Lathe {
                shape: LatheShape::Column {
                    height: 2.0,
                    radius: 0.2,
                },
                ..Lathe::default()
            },
        ),
        (
            "dome",
            Lathe {
                shape: LatheShape::Dome {
                    radius: 0.6,
                    rings: 12,
                },
                ..Lathe::default()
            },
        ),
        (
            "half_vase",
            Lathe {
                sweep_angle: PI * 1.25,
                start_angle: -FRAC_PI_2,
                ..Lathe::default()
            },
        ),
        (
            "ring",
            Lathe {
                shape: LatheShape::Custom {
                    profile: vec![
                        Vec2::new(0.3, 0.0),
                        Vec2::new(0.55, 0.0),
                        Vec2::new(0.55, 0.25),
                        Vec2::new(0.3, 0.25),
                    ],
                    closed: true,
                },
                segments: 48,
                ..Lathe::default()
            },
        ),
    ]
}

/// The extrude cases of the gallery, left to right (also golden fixtures).
pub fn extrudes() -> Vec<(&'static str, Extrude)> {
    vec![
        ("slab", Extrude::default()),
        (
            "hole_and_bevel",
            Extrude {
                outline: Outline::Rectangle {
                    size: Vec2::new(1.2, 0.8),
                    corner_radius: 0.1,
                },
                hole: Hole::Circle {
                    radius: 0.22,
                    segments: 32,
                    offset: Vec2::ZERO,
                },
                depth: 0.25,
                bevel_width: 0.05,
                bevel_segments: 4,
                ..Extrude::default()
            },
        ),
        (
            "star",
            Extrude {
                outline: Outline::Star {
                    points: 5,
                    outer_radius: 0.6,
                    inner_radius: 0.27,
                },
                depth: 0.2,
                bevel_width: 0.03,
                bevel_segments: 1,
                ..Extrude::default()
            },
        ),
        (
            "frame",
            Extrude {
                outline: Outline::Polygon {
                    sides: 6,
                    radius: 0.6,
                },
                hole: Hole::Inset { width: 0.15 },
                depth: 0.35,
                bevel_width: 0.02,
                ..Extrude::default()
            },
        ),
        (
            "disc",
            Extrude {
                outline: Outline::Circle {
                    radius: 0.5,
                    segments: 48,
                },
                hole: Hole::Rectangle {
                    size: Vec2::new(0.3, 0.3),
                    offset: Vec2::new(0.1, 0.0),
                },
                depth: 0.15,
                bevel_width: 0.04,
                bevel_segments: 3,
                ..Extrude::default()
            },
        ),
    ]
}

/// Ground plane, a row of primitives, a row of lathes and a row of
/// extrusions (the plaque with the hole stands upright so its hole shows).
pub fn build_shapes_scene(world: &mut World) {
    world.spawn((
        Transform::default(),
        Primitive::plane(Vec2::new(14.0, 10.0)),
        material(0x9fb08a, 0.95),
    ));
    let prims = [
        (Primitive::cuboid(Vec3::splat(0.9)), 0.45),
        (Primitive::sphere(0.5), 0.5),
        (Primitive::cylinder(0.45, 1.0), 0.5),
        (Primitive::capsule(0.35, 0.35), 0.7),
        (
            Primitive::new(Shape::Sphere {
                radius: 0.5,
                sectors: 8,
                stacks: 4,
            }),
            0.5,
        ),
    ];
    for (i, (p, lift)) in prims.into_iter().enumerate() {
        world.spawn((
            Transform::from_xyz(-4.4 + 2.2 * i as f32, lift, 2.6),
            p,
            material(0xd8c8a8, 0.7),
        ));
    }
    for (i, (_, lathe)) in lathes().into_iter().enumerate() {
        world.spawn((
            Transform::from_xyz(-4.4 + 2.2 * i as f32, 0.0, 0.0),
            lathe,
            Procedural::default(),
            material(0xc98f6a, 0.55),
        ));
    }
    for (i, (name, extrude)) in extrudes().into_iter().enumerate() {
        let mut t = Transform::from_xyz(-4.4 + 2.2 * i as f32, 0.0, -2.6);
        if name == "hole_and_bevel" {
            // Stood up on its long edge, facing the camera.
            t = t.with_rotation(Quat::from_rotation_x(FRAC_PI_2));
            t.translation.y = 0.4;
            t.translation.z -= 0.12;
        }
        world.spawn((t, extrude, Procedural::default(), material(0x7f9cc0, 0.5)));
    }
}

/// A camera framing [`build_shapes_scene`].
pub fn shapes_camera() -> OrbitCamera {
    OrbitCamera {
        target: Vec3::new(0.0, 0.2, 0.1),
        distance: 9.0,
        yaw: 0.12,
        pitch: 0.55,
        ..Default::default()
    }
}
