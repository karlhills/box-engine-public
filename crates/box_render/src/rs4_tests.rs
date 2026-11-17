//! RS4 render tests: spot cone falloff, local light shadows (spot and
//! point), ambient volumes and particles, rendered headlessly from a
//! camera 4 m above the origin looking down (+X right, −Z up the image;
//! the ground's half-width in view is 4 tan 25° ≈ 1.865 m).

use box_core::assets::Assets;
use box_core::color::Color;
use box_core::mesh::MeshData;
use box_core::time::Time;
use box_core::transform::{Transform, propagate_transforms};
use box_core::world::World;
use glam::{Vec2, Vec3};

use crate::components::*;
use crate::particles::{CurveKey, GradientKey, ParticleBlend, ParticleEmitter, simulate_particles};
use crate::renderer::Renderer;

const SIZE: u32 = 128;
const HALF_VIEW: f32 = 1.865;

fn headless() -> Option<Renderer> {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        return None;
    }
    Renderer::new_headless(SIZE, SIZE).ok()
}

/// Pixel of ground point (x, 0, z).
fn px(x: f32, z: f32) -> (u32, u32) {
    let f = |v: f32| (SIZE as f32 * 0.5 + v / HALF_VIEW * SIZE as f32 * 0.5) as u32;
    (f(x), f(z))
}

fn luma(img: &image::RgbaImage, (x, y): (u32, u32)) -> u32 {
    img.get_pixel(x, y).0[..3].iter().map(|&c| c as u32).sum()
}

/// Linear red channel (sRGB decoded) of a pixel.
fn linear(img: &image::RgbaImage, (x, y): (u32, u32)) -> f32 {
    let c = img.get_pixel(x, y).0[0] as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// A grey ground plane seen from above, no sun, no post effects, no fog,
/// `ambient` sky ambient.
fn ground_world(ambient: f32) -> World {
    let mut world = World::new();
    let env = world.resource_or_default::<Environment>();
    env.ambient_occlusion.enabled = false;
    env.bloom.enabled = false;
    env.fog_density = 0.0;
    env.ambient_intensity = ambient;
    env.exposure = 1.0;
    env.tonemapper = Tonemapper::None;
    world.spawn((
        Transform::from_xyz(0.0, 4.0, 0.0).looking_at(Vec3::ZERO, Vec3::NEG_Z),
        Camera::default(),
    ));
    let material = world
        .resource_or_default::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            base_color: Color::linear(0.5, 0.5, 0.5),
            roughness: 1.0,
            ..Default::default()
        });
    let plane = world
        .resource_or_default::<Assets<MeshData>>()
        .add(MeshData::plane(8.0, 8.0));
    world.spawn((Transform::default(), MeshRenderer::new(plane, material)));
    world
}

fn cube(world: &mut World, at: Vec3, size: f32) {
    let material = world
        .resource_or_default::<Assets<StandardMaterial>>()
        .add(StandardMaterial::default());
    let mesh = world
        .resource_or_default::<Assets<MeshData>>()
        .add(MeshData::cuboid(Vec3::splat(size)));
    world.spawn((
        Transform::from_translation(at),
        MeshRenderer::new(mesh, material),
    ));
}

fn down_from(at: Vec3) -> Transform {
    Transform::from_translation(at).looking_at(at - Vec3::Y, Vec3::Z)
}

#[test]
fn cone_attenuation_is_one_inside_zero_outside_and_a_smooth_square_between() {
    let spot = SpotLight {
        inner_angle: 15f32.to_radians(),
        outer_angle: 25f32.to_radians(),
        ..Default::default()
    };
    assert_eq!(spot.cone_attenuation(1.0), 1.0);
    assert_eq!(spot.cone_attenuation(15f32.to_radians().cos()), 1.0);
    assert_eq!(spot.cone_attenuation(25f32.to_radians().cos()), 0.0);
    assert_eq!(spot.cone_attenuation(40f32.to_radians().cos()), 0.0);
    let (ci, co) = (15f32.to_radians().cos(), 25f32.to_radians().cos());
    let mid = 0.5 * (ci + co);
    assert!((spot.cone_attenuation(mid) - 0.25).abs() < 1e-4);
    // Equal angles: a hard edge, no division by zero.
    let hard = SpotLight {
        inner_angle: 0.3,
        outer_angle: 0.3,
        ..Default::default()
    };
    assert_eq!(hard.cone_attenuation(0.29f32.cos()), 1.0);
    assert_eq!(hard.cone_attenuation(0.31f32.cos()), 0.0);
}

/// The rendered falloff across the cone matches `I cosθ / d² · cone(θ)`.
#[test]
fn spot_light_falloff_matches_the_cone_on_the_ground() {
    let Some(mut r) = headless() else {
        eprintln!("skipping: no GPU");
        return;
    };
    let spot = SpotLight {
        color: Color::WHITE,
        intensity: 3.0,
        range: 100.0,
        inner_angle: 15f32.to_radians(),
        outer_angle: 25f32.to_radians(),
        ..Default::default()
    };
    let mut world = ground_world(0.0);
    world.spawn((down_from(Vec3::new(0.0, 2.0, 0.0)), spot));
    propagate_transforms(&mut world);
    let lit = r.capture(&mut world).unwrap();
    assert_eq!(r.stats().spot_lights, 1);
    let mut dark = ground_world(0.0);
    propagate_transforms(&mut dark);
    let dark = r.capture(&mut dark).unwrap();

    let centre = linear(&lit, px(0.0, 0.0));
    for x in [0.3f32, 0.6, 0.75, 0.85] {
        let d2 = 4.0 + x * x;
        let cos = 2.0 / d2.sqrt();
        let expected = cos * spot.cone_attenuation(cos) / d2 * 4.0;
        let got = linear(&lit, px(x, 0.0)) / centre;
        assert!(
            (got - expected).abs() < 0.03 + 0.08 * expected,
            "x = {x}: {got} against {expected}"
        );
    }
    // Beyond the outer angle (2 tan 25° = 0.93 m): exactly unlit.
    for p in [px(1.0, 0.0), px(0.0, 1.2), px(-1.3, -0.5)] {
        assert_eq!(lit.get_pixel(p.0, p.1), dark.get_pixel(p.0, p.1), "{p:?}");
    }
    assert_eq!(r.validation_errors(), 0);
}

/// A known occluder darkens a known ground pixel, for a spot light and for
/// a point light through two cube faces (−Y below it, +X beside it); with
/// shadows off the same pixel is lit.
#[test]
fn the_shadowed_local_light_is_blocked_by_an_occluder() {
    let Some(mut r) = headless() else {
        eprintln!("skipping: no GPU");
        return;
    };
    let target = px(1.0, 0.0);
    let away = px(-1.0, 0.0);
    // (light, occluder centre, occluder size, spot?)
    let cases = [
        (
            Vec3::new(0.0, 3.0, 0.0),
            Vec3::new(0.3, 2.0, 0.0),
            0.2,
            true,
        ),
        (
            Vec3::new(0.0, 3.0, 0.0),
            Vec3::new(0.3, 2.0, 0.0),
            0.2,
            false,
        ),
        (
            Vec3::new(0.0, 0.6, 0.0),
            Vec3::new(0.5, 0.3, 0.0),
            0.2,
            false,
        ),
    ];
    for (light, occluder, size, spot) in cases {
        let image = |r: &mut Renderer, shadows: bool| {
            let mut world = ground_world(0.0);
            cube(&mut world, occluder, size);
            if spot {
                world.spawn((
                    down_from(light),
                    SpotLight {
                        intensity: 6.0,
                        range: 10.0,
                        inner_angle: 50f32.to_radians(),
                        outer_angle: 60f32.to_radians(),
                        shadows,
                        ..Default::default()
                    },
                ));
            } else {
                world.spawn((
                    Transform::from_translation(light),
                    PointLight {
                        intensity: 3.0,
                        range: 10.0,
                        shadows,
                        ..Default::default()
                    },
                ));
            }
            propagate_transforms(&mut world);
            let img = r.capture(&mut world).unwrap();
            (
                img,
                r.stats().local_shadow_views,
                r.stats().local_shadow_draws,
            )
        };
        let (on, views, draws) = image(&mut r, true);
        let (off, no_views, _) = image(&mut r, false);
        assert_eq!(views, if spot { 1 } else { 6 });
        assert!(draws >= 1, "the occluder is drawn into the shadow map");
        assert_eq!(no_views, 0);
        let (shadowed, lit) = (luma(&on, target), luma(&off, target));
        assert!(
            shadowed * 3 < lit,
            "light {light}, spot {spot}: {shadowed} shadowed against {lit} lit"
        );
        // Away from the occluder the shadow changes nothing visible (no
        // acne): within the dither.
        let (a, b) = (luma(&on, away) as i32, luma(&off, away) as i32);
        assert!((a - b).abs() <= 6, "light {light}, spot {spot}: {a} vs {b}");
    }
    assert_eq!(r.validation_errors(), 0);
}

/// Reallocating the local shadow map (a new resolution) remakes every frame
/// bind group, also the ambient occlusion one.
#[test]
fn local_shadow_resolution_changes_with_ambient_occlusion_on() {
    let Some(mut r) = headless() else {
        eprintln!("skipping: no GPU");
        return;
    };
    let mut world = ground_world(0.2);
    world
        .resource_mut::<Environment>()
        .ambient_occlusion
        .enabled = true;
    cube(&mut world, Vec3::new(0.3, 2.0, 0.0), 0.2);
    let light = world.spawn((
        down_from(Vec3::new(0.0, 3.0, 0.0)),
        SpotLight {
            intensity: 6.0,
            range: 10.0,
            inner_angle: 50f32.to_radians(),
            outer_angle: 60f32.to_radians(),
            shadows: true,
            ..Default::default()
        },
    ));
    propagate_transforms(&mut world);
    let before = r.capture(&mut world).unwrap();
    for resolution in [512, 2048, 1024] {
        world.get_mut::<SpotLight>(light).unwrap().shadow.resolution = resolution;
        let img = r.capture(&mut world).unwrap();
        let (a, b) = (luma(&img, px(1.0, 0.0)), luma(&before, px(1.0, 0.0)));
        assert!(a.abs_diff(b) <= 9, "{resolution}: {a} vs {b}");
    }
    assert_eq!(r.validation_errors(), 0);
}

/// Inside an ambient volume the sky ambient is replaced; outside (beyond
/// the blend distance) the image is unchanged.
#[test]
fn ambient_volumes_replace_the_sky_ambient_inside_only() {
    let Some(mut r) = headless() else {
        eprintln!("skipping: no GPU");
        return;
    };
    let build = |volume: Option<AmbientVolume>| {
        let mut world = ground_world(1.0);
        if let Some(v) = volume {
            world.spawn((Transform::from_xyz(-1.0, 0.0, 0.0), v));
        }
        propagate_transforms(&mut world);
        world
    };
    let sky = r.capture(&mut build(None)).unwrap();
    let closed = r
        .capture(&mut build(Some(AmbientVolume {
            half_extents: Vec3::new(1.0, 1.0, 3.0),
            color: Color::BLACK,
            intensity: 0.0,
            sky_contribution: 0.0,
            blend_distance: 0.2,
        })))
        .unwrap();
    assert_eq!(r.stats().ambient_volumes, 1);
    let red = r
        .capture(&mut build(Some(AmbientVolume {
            half_extents: Vec3::new(1.0, 1.0, 3.0),
            color: Color::linear(1.0, 0.0, 0.0),
            intensity: 0.5,
            sky_contribution: 0.0,
            blend_distance: 0.2,
        })))
        .unwrap();
    let inside = px(-1.0, 0.0);
    let outside = px(1.0, 0.0);
    assert!(
        luma(&sky, inside) > 150,
        "the sky ambient lights the ground"
    );
    assert!(
        luma(&closed, inside) < 3,
        "no sky ambient leaks into the box"
    );
    let [rr, g, b, _] = red.get_pixel(inside.0, inside.1).0;
    assert!(
        rr > 60 && g < 3 && b < 3,
        "the volume's own ambient: {rr} {g} {b}"
    );
    for img in [&closed, &red] {
        assert_eq!(
            img.get_pixel(outside.0, outside.1),
            sky.get_pixel(outside.0, outside.1)
        );
    }
    // The blend: in between at the boundary (x = 0 .. 0.2).
    let edge = luma(&closed, px(0.1, 0.0));
    assert!(edge > 3 && edge < luma(&sky, px(0.1, 0.0)), "{edge}");
    assert_eq!(r.validation_errors(), 0);
}

/// Particles draw where they are, count in the stats, and soft particles
/// (prepass depth) render without validation errors.
#[test]
fn particles_draw_instanced_and_soft() {
    let Some(mut r) = headless() else {
        eprintln!("skipping: no GPU");
        return;
    };
    let mut world = ground_world(0.1);
    world.insert_resource(Time {
        delta: 1.0 / 60.0,
        ..Default::default()
    });
    let emitter = |blend, soft_distance, x: f32| {
        (
            Transform::from_xyz(x, 0.5, 0.0),
            ParticleEmitter {
                rate: 600.0,
                speed: Vec2::ZERO,
                spawn_radius: 0.2,
                lifetime: Vec2::splat(10.0),
                size: vec![CurveKey { t: 0.0, value: 0.3 }],
                color: vec![GradientKey {
                    t: 0.0,
                    color: Color::linear(1.0, 0.2, 0.1),
                }],
                intensity: 2.0,
                blend,
                soft_distance,
                ..Default::default()
            },
        )
    };
    world.spawn(emitter(ParticleBlend::Additive, 0.0, -1.0));
    world.spawn(emitter(ParticleBlend::Alpha, 0.5, 1.0));
    propagate_transforms(&mut world);
    let plain = r.capture(&mut world).unwrap();
    assert_eq!(r.stats().particles, 0, "no state yet: nothing drawn");
    for _ in 0..30 {
        simulate_particles(&mut world);
        world.apply_deferred();
    }
    let img = r.capture(&mut world).unwrap();
    let s = r.stats();
    assert_eq!(s.particle_emitters, 2);
    assert_eq!(s.particles, 600, "two emitters, 300 particles each");
    for x in [-1.0, 1.0] {
        let p = px(x, 0.0);
        let [red, _, blue, _] = img.get_pixel(p.0, p.1).0;
        assert!(
            red > plain.get_pixel(p.0, p.1).0[0] + 40 && red >= blue,
            "particles at x = {x}: {:?} over {:?}",
            img.get_pixel(p.0, p.1),
            plain.get_pixel(p.0, p.1)
        );
    }
    let empty = px(0.0, 1.2);
    assert_eq!(
        img.get_pixel(empty.0, empty.1),
        plain.get_pixel(empty.0, empty.1)
    );
    assert_eq!(r.validation_errors(), 0);
}
