//! RS4 scenes: an interior lit by a shadowed spot light and point lights
//! through a window onto a bright exterior, with an ambient volume (no sky
//! ambient leak), and a particles scene (fire, smoke, a spark burst, a
//! projectile trail).

use box_engine::box_core::image::{ColorSpace, ImageData};
use box_engine::prelude::*;

use super::materials::add_image;

/// Options of the interior scene.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InteriorOptions {
    /// The room's `AmbientVolume` (off: the sky ambient leaks in).
    pub ambient_override: bool,
    /// Which light casts the local shadow.
    pub shadowed: Shadowed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shadowed {
    Spot,
    Point,
}

impl Default for InteriorOptions {
    fn default() -> Self {
        Self {
            ambient_override: true,
            shadowed: Shadowed::Spot,
        }
    }
}

/// Room interior: x -4..4, y 0..3, z -3..3; walls 0.2 m thick; a window in
/// the +Z wall.
pub const ROOM_HALF: Vec3 = Vec3::new(4.0, 1.5, 3.0);
pub const ROOM_CENTRE: Vec3 = Vec3::new(0.0, 1.5, 0.0);
/// Where the shadowed spot light hangs and the table it lights.
pub const SPOT_AT: Vec3 = Vec3::new(-0.6, 2.9, -0.8);
pub const TABLE_AT: Vec3 = Vec3::new(-0.6, 0.0, -0.8);
pub const TABLE_TOP: f32 = 0.78;

fn material(world: &mut World, m: StandardMaterial) -> Handle<StandardMaterial> {
    world.resource_mut::<Assets<StandardMaterial>>().add(m)
}

fn matte(world: &mut World, hex: u32, roughness: f32) -> Handle<StandardMaterial> {
    material(
        world,
        StandardMaterial {
            base_color: Color::hex(hex),
            roughness,
            ..Default::default()
        },
    )
}

/// A box of `size` centred at `at`.
fn block(
    world: &mut World,
    name: &str,
    at: Vec3,
    size: Vec3,
    m: Handle<StandardMaterial>,
) -> Entity {
    let mesh = world
        .resource_mut::<Assets<MeshData>>()
        .add(MeshData::cuboid(size));
    world.spawn((
        Name::new(name),
        Transform::from_translation(at),
        MeshRenderer::new(mesh, m),
    ))
}

pub fn build_interior(world: &mut World, options: InteriorOptions) {
    let wall = matte(world, 0xd9d2c5, 0.85);
    let floor = matte(world, 0x8a6a4c, 0.6);
    let wood = matte(world, 0x6b4a2e, 0.55);
    let red = matte(world, 0xb8432f, 0.5);
    let blue = matte(world, 0x3a6aa8, 0.4);
    let grass = matte(world, 0x6f8f4a, 0.9);
    let stone = matte(world, 0xa8a49a, 0.8);
    let metal = material(
        world,
        StandardMaterial {
            base_color: Color::hex(0xd0d0d6),
            roughness: 0.3,
            metallic: 1.0,
            ..Default::default()
        },
    );

    // Outside: a big field with a few stones.
    block(
        world,
        "Field",
        Vec3::new(0.0, -0.06, 0.0),
        Vec3::new(80.0, 0.1, 80.0),
        grass,
    );
    for (i, p) in [
        Vec3::new(-3.0, 0.4, 8.0),
        Vec3::new(2.5, 0.6, 10.5),
        Vec3::new(6.0, 0.5, 6.5),
    ]
    .into_iter()
    .enumerate()
    {
        block(
            world,
            &format!("Stone {i}"),
            p,
            Vec3::splat(p.y * 2.0),
            stone,
        );
    }

    // The room: floor, ceiling, walls; the +Z wall has a window
    // (x -1.6..1.6, y 0.9..2.3).
    let (hx, hz, h, t) = (ROOM_HALF.x, ROOM_HALF.z, 2.0 * ROOM_HALF.y, 0.2);
    block(
        world,
        "Floor",
        Vec3::new(0.0, -0.1, 0.0),
        Vec3::new(2.0 * hx + 2.0 * t, 0.2, 2.0 * hz + 2.0 * t),
        floor,
    );
    block(
        world,
        "Ceiling",
        Vec3::new(0.0, h + 0.1, 0.0),
        Vec3::new(2.0 * hx + 2.0 * t, 0.2, 2.0 * hz + 2.0 * t),
        wall,
    );
    block(
        world,
        "Back wall",
        Vec3::new(0.0, h / 2.0, -hz - t / 2.0),
        Vec3::new(2.0 * hx, h, t),
        wall,
    );
    block(
        world,
        "Left wall",
        Vec3::new(-hx - t / 2.0, h / 2.0, 0.0),
        Vec3::new(t, h, 2.0 * hz + 2.0 * t),
        wall,
    );
    block(
        world,
        "Right wall",
        Vec3::new(hx + t / 2.0, h / 2.0, 0.0),
        Vec3::new(t, h, 2.0 * hz + 2.0 * t),
        wall,
    );
    let (wx, w0, w1) = (1.6, 0.9, 2.3);
    let z = hz + t / 2.0;
    block(
        world,
        "Front wall left",
        Vec3::new(-(hx + wx) / 2.0, h / 2.0, z),
        Vec3::new(hx - wx, h, t),
        wall,
    );
    block(
        world,
        "Front wall right",
        Vec3::new((hx + wx) / 2.0, h / 2.0, z),
        Vec3::new(hx - wx, h, t),
        wall,
    );
    block(
        world,
        "Below window",
        Vec3::new(0.0, w0 / 2.0, z),
        Vec3::new(2.0 * wx, w0, t),
        wall,
    );
    block(
        world,
        "Above window",
        Vec3::new(0.0, (w1 + h) / 2.0, z),
        Vec3::new(2.0 * wx, h - w1, t),
        wall,
    );

    // Furniture: a table under the spot light with things on it, a shelf
    // and a crate by the lamp.
    let top = TABLE_AT + Vec3::Y * TABLE_TOP;
    block(
        world,
        "Table top",
        top - Vec3::Y * 0.03,
        Vec3::new(1.6, 0.06, 0.9),
        wood,
    );
    for (sx, sz) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        let leg = TABLE_AT + Vec3::new(0.7 * sx, (TABLE_TOP - 0.06) / 2.0, 0.37 * sz);
        block(
            world,
            "Table leg",
            leg,
            Vec3::new(0.06, TABLE_TOP - 0.06, 0.06),
            wood,
        );
    }
    let sphere = world
        .resource_mut::<Assets<MeshData>>()
        .add(MeshData::uv_sphere(0.16, 32, 16));
    world.spawn((
        Name::new("Ball"),
        Transform::from_translation(top + Vec3::new(0.25, 0.16, 0.05)),
        MeshRenderer::new(sphere, metal),
    ));
    block(
        world,
        "Book",
        top + Vec3::new(-0.35, 0.04, -0.1),
        Vec3::new(0.3, 0.08, 0.22),
        red,
    );
    block(
        world,
        "Shelf",
        Vec3::new(-3.75, 1.0, -1.6),
        Vec3::new(0.5, 2.0, 1.2),
        wood,
    );
    block(
        world,
        "Crate",
        Vec3::new(2.6, 0.35, -1.9),
        Vec3::splat(0.7),
        blue,
    );

    // Lights: the shadowed spot over the table, a warm lamp in the corner
    // and a cool fill by the shelf.
    let bulb = world
        .resource_mut::<Assets<MeshData>>()
        .add(MeshData::uv_sphere(0.05, 12, 6));
    let spot_shadows = options.shadowed == Shadowed::Spot;
    let spot = Transform::from_translation(SPOT_AT).looking_at(top, Vec3::Z);
    world.spawn((
        Name::new("Spot"),
        spot,
        SpotLight {
            color: Color::linear(1.0, 0.88, 0.7),
            intensity: 30.0,
            range: 8.0,
            inner_angle: 22f32.to_radians(),
            outer_angle: 34f32.to_radians(),
            shadows: spot_shadows,
            ..Default::default()
        },
    ));
    for (name, at, color, intensity, shadows) in [
        (
            "Lamp",
            Vec3::new(2.6, 1.5, 1.6),
            Color::linear(1.0, 0.6, 0.3),
            3.0,
            !spot_shadows,
        ),
        (
            "Fill",
            Vec3::new(-3.2, 2.4, 1.6),
            Color::linear(0.5, 0.65, 1.0),
            1.2,
            false,
        ),
    ] {
        world.spawn((
            Name::new(name),
            Transform::from_translation(at),
            PointLight {
                color,
                intensity,
                range: 6.0,
                shadows,
                ..Default::default()
            },
        ));
        let glow = material(
            world,
            StandardMaterial {
                base_color: Color::BLACK,
                emissive: color.scaled(20.0),
                ..Default::default()
            },
        );
        world.spawn((
            Transform::from_translation(at),
            MeshRenderer {
                cast_shadows: false,
                ..MeshRenderer::new(bulb, glow)
            },
        ));
    }

    if options.ambient_override {
        world.spawn((
            Name::new("Room ambient"),
            Transform::from_translation(ROOM_CENTRE),
            AmbientVolume {
                half_extents: ROOM_HALF,
                color: Color::linear(1.0, 0.85, 0.7),
                intensity: 0.03,
                sky_contribution: 0.0,
                blend_distance: 0.1,
            },
        ));
    }

    // A bright midday outside; the sun shines in through the window.
    *world.resource_or_default::<Environment>() = Environment {
        exposure: 1.4,
        ..Environment::day()
    };
    world.spawn((
        Name::new("Sun"),
        Transform::default().looking_to(Vec3::new(0.3, -0.55, -0.75), Vec3::Y),
        DirectionalLight {
            intensity: 3.2,
            shadow_distance: 30.0,
            ..Default::default()
        },
    ));
    let eye = Vec3::new(3.4, 2.0, -2.6);
    world.spawn((
        Name::new("Camera"),
        Transform::from_translation(eye).looking_at(Vec3::new(-0.8, 0.9, 1.4), Vec3::Y),
        Camera {
            fov_y: 60f32.to_radians(),
            ..Default::default()
        },
    ));
}

/// Marks the projectile that flies in a circle trailing particles.
pub struct Projectile;

/// Moves the projectile around a 3 m circle, 1.2 m up.
pub fn projectile_system(world: &mut World) {
    let t = world.resource::<Time>().elapsed;
    for (tr, _) in world.query::<(&mut Transform, &Projectile)>().iter() {
        let a = t * 1.6;
        tr.translation = Vec3::new(
            3.0 * a.cos(),
            1.2 + 0.4 * (2.0 * a).sin(),
            3.0 * a.sin() - 1.0,
        );
    }
}

/// A soft, lumpy smoke puff: a few overlapping soft blobs, white, alpha
/// coverage.
fn smoke_image() -> ImageData {
    const N: u32 = 64;
    let blobs = [
        (0.0, 0.0, 0.62),
        (-0.3, -0.18, 0.4),
        (0.28, -0.22, 0.38),
        (0.15, 0.3, 0.36),
        (-0.25, 0.25, 0.33),
    ];
    let mut px = Vec::with_capacity((N * N * 4) as usize);
    for y in 0..N {
        for x in 0..N {
            let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5) / N as f32 * 2.0 - 1.0;
            let mut a = 0.0f32;
            for (bx, by, r) in blobs {
                let d = (p - Vec2::new(bx, by)).length() / r;
                a += (1.0 - d * d).max(0.0).powi(2) * 0.6;
            }
            let shade = 200 + (a.min(1.0) * 55.0) as u8;
            px.extend_from_slice(&[shade, shade, shade, (a.min(1.0) * 255.0) as u8]);
        }
    }
    ImageData::new(N, N, ColorSpace::Srgb, px).expect("valid smoke image")
}

fn keys(values: &[(f32, f32)]) -> Vec<CurveKey> {
    values
        .iter()
        .map(|&(t, value)| CurveKey { t, value })
        .collect()
}

fn gradient(values: &[(f32, Color)]) -> Vec<GradientKey> {
    values
        .iter()
        .map(|&(t, color)| GradientKey { t, color })
        .collect()
}

/// Fire with soft smoke above it, a looping spark burst and a projectile
/// with a trail, at dusk.
pub fn build_particles(world: &mut World) {
    let ground = matte(world, 0x5d6b4a, 0.95);
    let rock = matte(world, 0x6e6a62, 0.85);
    block(
        world,
        "Ground",
        Vec3::new(0.0, -0.05, 0.0),
        Vec3::new(40.0, 0.1, 40.0),
        ground,
    );
    for (i, a) in (0..7).map(|i| (i, i as f32 / 7.0 * std::f32::consts::TAU)) {
        let p = Vec3::new(-1.5 + 0.55 * a.cos(), 0.1, 0.55 * a.sin());
        block(
            world,
            &format!("Fire stone {i}"),
            p,
            Vec3::new(0.25, 0.2, 0.2),
            rock,
        );
    }
    block(
        world,
        "Wall",
        Vec3::new(0.0, 1.0, -3.0),
        Vec3::new(8.0, 2.0, 0.3),
        rock,
    );
    let smoke = add_image(world, "smoke.png", smoke_image());

    world.spawn((
        Name::new("Fire"),
        Transform::from_xyz(-1.5, 0.05, 0.0),
        ParticleEmitter {
            rate: 160.0,
            lifetime: Vec2::new(0.5, 0.9),
            speed: Vec2::new(0.6, 1.3),
            cone_angle: 0.25,
            spawn_radius: 0.28,
            gravity: Vec3::new(0.0, 1.5, 0.0),
            drag: 1.0,
            size: keys(&[(0.0, 0.55), (0.6, 0.4), (1.0, 0.1)]),
            size_random: 0.3,
            color: gradient(&[
                (0.0, Color::linear(1.0, 0.75, 0.3).with_alpha(0.0)),
                (0.15, Color::linear(1.0, 0.55, 0.15)),
                (1.0, Color::linear(0.9, 0.15, 0.02).with_alpha(0.0)),
            ]),
            intensity: 2.0,
            spin: Vec2::new(0.5, 2.0),
            random_rotation: true,
            texture: smoke.clone(),
            blend: ParticleBlend::Additive,
            soft_distance: 0.25,
            seed: 11,
            ..Default::default()
        },
    ));
    world.spawn((
        Name::new("Smoke"),
        Transform::from_xyz(-1.5, 0.8, 0.0),
        ParticleEmitter {
            rate: 22.0,
            lifetime: Vec2::new(3.0, 4.0),
            speed: Vec2::new(0.3, 0.6),
            cone_angle: 0.3,
            spawn_radius: 0.2,
            gravity: Vec3::new(0.25, 0.35, 0.0),
            drag: 0.4,
            size: keys(&[(0.0, 0.5), (1.0, 2.2)]),
            size_random: 0.25,
            color: gradient(&[
                (0.0, Color::linear(0.35, 0.33, 0.32).with_alpha(0.0)),
                (0.2, Color::linear(0.35, 0.33, 0.32).with_alpha(0.55)),
                (1.0, Color::linear(0.4, 0.4, 0.42).with_alpha(0.0)),
            ]),
            spin: Vec2::new(0.1, 0.4),
            random_rotation: true,
            texture: smoke,
            blend: ParticleBlend::Alpha,
            soft_distance: 0.6,
            lit: true,
            seed: 12,
            ..Default::default()
        },
    ));
    world.spawn((
        Name::new("Sparks"),
        Transform::from_xyz(1.6, 0.6, 0.6),
        ParticleEmitter {
            looping: true,
            duration: 1.5,
            despawn_when_finished: false,
            speed: Vec2::new(2.5, 5.5),
            cone_angle: 0.9,
            gravity: Vec3::new(0.0, -9.81, 0.0),
            drag: 0.6,
            lifetime: Vec2::new(0.5, 1.1),
            size: keys(&[(0.0, 0.06), (1.0, 0.02)]),
            color: gradient(&[
                (0.0, Color::linear(1.0, 0.9, 0.5)),
                (1.0, Color::linear(1.0, 0.35, 0.05).with_alpha(0.0)),
            ]),
            intensity: 12.0,
            blend: ParticleBlend::Additive,
            seed: 13,
            ..ParticleEmitter::burst(150)
        },
    ));
    let orb_mesh = world
        .resource_mut::<Assets<MeshData>>()
        .add(MeshData::uv_sphere(0.08, 16, 8));
    let orb = material(
        world,
        StandardMaterial {
            base_color: Color::BLACK,
            emissive: Color::linear(0.3, 0.8, 1.0).scaled(30.0),
            ..Default::default()
        },
    );
    world.spawn((
        Name::new("Projectile"),
        Projectile,
        Transform::from_xyz(3.0, 1.2, -1.0),
        MeshRenderer {
            cast_shadows: false,
            ..MeshRenderer::new(orb_mesh, orb)
        },
        ParticleEmitter {
            rate: 0.0,
            rate_over_distance: 40.0,
            speed: Vec2::new(0.0, 0.15),
            cone_angle: std::f32::consts::PI,
            lifetime: Vec2::new(0.45, 0.6),
            size: keys(&[(0.0, 0.16), (1.0, 0.0)]),
            color: gradient(&[
                (0.0, Color::linear(0.4, 0.85, 1.0)),
                (1.0, Color::linear(0.1, 0.3, 1.0).with_alpha(0.0)),
            ]),
            intensity: 6.0,
            blend: ParticleBlend::Additive,
            seed: 14,
            ..Default::default()
        },
    ));

    *world.resource_or_default::<Environment>() = Environment {
        exposure: 1.0,
        ..Environment::evening()
    };
    world.spawn((
        Name::new("Sun"),
        Transform::default().looking_to(Vec3::new(-0.8, -0.25, -0.5), Vec3::Y),
        DirectionalLight {
            color: Color::linear(1.0, 0.6, 0.35),
            intensity: 1.2,
            ..Default::default()
        },
    ));
    world.spawn((
        Name::new("Camera"),
        Transform::from_xyz(0.5, 2.2, 6.5).looking_at(Vec3::new(0.0, 1.0, -0.5), Vec3::Y),
        Camera::default(),
    ));
}
