//! Regression scenes for renderer defects found in review: mirrored
//! transforms, shadow casters beyond the cascade box, cascade blend bands,
//! height fog on long downward rays and oversized resizes. Each writes its
//! capture to `target/captures/review/`. Skipped when `BOX_SKIP_GPU_TESTS` is
//! set or no GPU adapter exists.

use std::path::PathBuf;

use box_core::prelude::*;
use box_core::transform::propagate_transforms;
use box_render::{
    Camera, DirectionalLight, Environment, MeshRenderer, RenderError, Renderer, SelectionOutline,
    StandardMaterial, golden,
};

const W: u32 = 960;
const H: u32 = 540;

fn save(img: &image::RgbaImage, name: &str) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/review");
    std::fs::create_dir_all(&dir).unwrap();
    img.save(dir.join(format!("{name}.png"))).unwrap();
}

fn renderer() -> Option<Renderer> {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping");
        return None;
    }
    match Renderer::new_headless(W, H) {
        Ok(r) => Some(r),
        Err(RenderError::NoAdapter) => {
            eprintln!("no GPU adapter; skipping");
            None
        }
        Err(e) => panic!("renderer creation failed: {e}"),
    }
}

fn spawn(world: &mut World, t: Transform, mesh: MeshData, base: Color) -> Entity {
    let mesh = world.resource_or_default::<Assets<MeshData>>().add(mesh);
    let material = world
        .resource_or_default::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            base_color: base,
            roughness: 0.8,
            ..Default::default()
        });
    world.spawn((t, MeshRenderer::new(mesh, material)))
}

fn ground(world: &mut World) {
    spawn(
        world,
        Transform::default(),
        MeshData::grid(400.0, 400.0, 8, 8),
        Color::linear(0.5, 0.5, 0.5),
    );
}

fn no_fog() -> Environment {
    Environment {
        fog_density: 0.0,
        ..Environment::day()
    }
}

fn luminance(p: &image::Rgba<u8>) -> f32 {
    (p.0[0] as f32 + p.0[1] as f32 + p.0[2] as f32) / 3.0
}

/// A cube is symmetric, so mirroring it in X must not change the image: not
/// its shading, not its shadow and not its selection outline.
#[test]
fn mirrored_scale_renders_like_the_unmirrored_mesh() {
    let Some(mut r) = renderer() else { return };
    let mut render = |scale: Vec3| {
        let mut world = World::new();
        world.insert_resource(no_fog());
        ground(&mut world);
        let cube = spawn(
            &mut world,
            Transform::from_xyz(0.0, 1.0, 0.0).with_scale(scale),
            MeshData::cuboid(Vec3::ONE),
            Color::linear(0.7, 0.2, 0.1),
        );
        world.insert_resource(SelectionOutline {
            primary: vec![cube],
            color: Color::srgb(1.0, 0.0, 1.0),
            width_px: 3.0,
            ..Default::default()
        });
        world.spawn((
            Transform::from_xyz(4.0, 3.5, 6.0).looking_at(Vec3::new(0.0, 0.8, 0.0), Vec3::Y),
            Camera::default(),
        ));
        world.spawn((
            Transform::default().looking_to(Vec3::new(0.6, -0.7, -0.4), Vec3::Y),
            DirectionalLight::default(),
        ));
        propagate_transforms(&mut world);
        r.capture(&mut world).expect("capture")
    };
    let plain = render(Vec3::new(2.0, 2.0, 2.0));
    let mirrored = render(Vec3::new(-2.0, 2.0, 2.0));
    save(&plain, "mirrored_reference");
    save(&mirrored, "mirrored_cube");
    assert_eq!(r.validation_errors(), 0);
    golden::compare(&mirrored, &plain, &golden::DEFAULT_TOLERANCE)
        .expect("a mirrored cube looks like the cube");
}

/// Low sun, short shadow distance: a tall pillar far up-light still shadows
/// the ground in front of the camera.
#[test]
fn tall_caster_far_up_light_still_casts() {
    let Some(mut r) = renderer() else { return };
    let mut world = World::new();
    world.insert_resource(no_fog());
    ground(&mut world);
    // 60 m tall, 60 m toward the sun. Its shadow runs along +X through the
    // origin, 2 m wide.
    spawn(
        &mut world,
        Transform::from_xyz(-60.0, 30.0, 0.0).with_scale(Vec3::new(2.0, 60.0, 2.0)),
        MeshData::cuboid(Vec3::ONE),
        Color::linear(0.4, 0.4, 0.4),
    );
    world.spawn((
        Transform::from_xyz(0.0, 3.0, 8.0).looking_at(Vec3::ZERO, Vec3::Y),
        Camera::default(),
    ));
    world.spawn((
        Transform::default().looking_to(Vec3::new(1.0, -0.25, 0.0), Vec3::Y),
        DirectionalLight {
            shadow_distance: 12.0,
            ..Default::default()
        },
    ));
    propagate_transforms(&mut world);
    let img = r.capture(&mut world).expect("capture");
    save(&img, "tall_caster_low_sun");
    assert_eq!(r.validation_errors(), 0);
    // The origin is the centre pixel; the ground near the bottom edge is lit.
    let shadowed = luminance(img.get_pixel(W / 2, H / 2));
    let lit = luminance(img.get_pixel(W / 2, H * 9 / 10));
    assert!(
        shadowed < lit * 0.9,
        "ground at the origin is in the pillar's shadow ({shadowed} vs lit {lit})"
    );
}

/// Narrow field of view under a roof: the whole ground is in shadow, so no
/// lit stripe may appear where one cascade blends into the next.
#[test]
fn no_lit_stripe_where_cascades_blend() {
    let Some(mut r) = renderer() else { return };
    let mut world = World::new();
    world.insert_resource(no_fog());
    ground(&mut world);
    spawn(
        &mut world,
        Transform::from_xyz(0.0, 8.0, 0.0).with_scale(Vec3::new(400.0, 0.2, 400.0)),
        MeshData::cuboid(Vec3::ONE),
        Color::linear(0.4, 0.4, 0.4),
    );
    world.spawn((
        Transform::from_xyz(0.0, 2.0, 0.0).looking_at(Vec3::new(0.0, 0.0, -30.0), Vec3::Y),
        Camera {
            fov_y: 20f32.to_radians(),
            ..Default::default()
        },
    ));
    world.spawn((
        Transform::default().looking_to(Vec3::NEG_Y, Vec3::Z),
        DirectionalLight {
            shadow_distance: 60.0,
            ..Default::default()
        },
    ));
    propagate_transforms(&mut world);
    let img = r.capture(&mut world).expect("capture");
    save(&img, "cascade_blend_narrow_fov");
    assert_eq!(r.validation_errors(), 0);
    // The bottom half shows ground from 8 m to 30 m, spanning two splits.
    let column = (H / 2 + 10..H).map(|y| luminance(img.get_pixel(W / 2, y)));
    let (min, max) = column.fold((f32::MAX, f32::MIN), |(lo, hi), l| (lo.min(l), hi.max(l)));
    assert!(
        max - min < 6.0,
        "shadowed ground is evenly dark (luminance {min}..{max})"
    );
}

/// Looking straight down from above 89 m with `fog_height_falloff` 1: the
/// fog hugging the ground looks the same as from 80 m, where `exp` is still
/// in range. Rays at the same view angle cross the same amount of fog.
#[test]
fn height_fog_survives_long_downward_rays() {
    let Some(mut r) = renderer() else { return };
    let mut world = World::new();
    world.insert_resource(Environment {
        fog_density: 1.0,
        fog_height_falloff: 1.0,
        fog_base_height: 0.0,
        ..Environment::day()
    });
    ground(&mut world);
    spawn(
        &mut world,
        Transform::from_xyz(0.0, 1.0, 0.0).with_scale(Vec3::new(400.0, 2.0, 20.0)),
        MeshData::cuboid(Vec3::ONE),
        Color::linear(0.8, 0.05, 0.05),
    );
    world.spawn((
        Transform::default().looking_to(Vec3::new(0.3, -1.0, -0.2), Vec3::Y),
        DirectionalLight {
            shadows: false,
            ..Default::default()
        },
    ));
    let mut probes_at = |height: f32| {
        let camera = world.spawn((
            Transform::from_xyz(0.0, height, 0.0).looking_to(Vec3::NEG_Y, Vec3::NEG_Z),
            Camera::default(),
        ));
        propagate_transforms(&mut world);
        let img = r.capture(&mut world).expect("capture");
        save(&img, &format!("fog_downward_{height}m"));
        world.despawn(camera);
        // The red strip's top (2 m up, thin fog) and the ground (thick fog).
        [*img.get_pixel(W / 2, H / 2), *img.get_pixel(W / 2, 10)]
    };
    let low = probes_at(80.0);
    assert!(low[0].0[0] > low[0].0[1] + 40, "red strip visible: {low:?}");
    for height in [92.0, 100.0, 150.0] {
        let probes = probes_at(height);
        for (p, l) in probes.iter().zip(&low) {
            let diff = (0..3).map(|c| p.0[c].abs_diff(l.0[c])).max().unwrap();
            assert!(diff <= 4, "at {height} m: {probes:?}, at 80 m: {low:?}");
        }
    }
    assert_eq!(r.validation_errors(), 0);
}

/// A size beyond the device's texture limit is clamped, not passed to wgpu.
#[test]
fn resize_is_clamped_to_the_device_limit() {
    let Some(mut r) = renderer() else { return };
    let max = r.device().limits().max_texture_dimension_2d;
    r.resize(max + 1000, 64);
    let mut world = World::new();
    let img = r.capture(&mut world).expect("capture");
    assert_eq!(r.validation_errors(), 0, "no validation errors");
    assert_eq!(r.size(), (max, 64));
    assert_eq!(img.dimensions(), (max, 64));
}
