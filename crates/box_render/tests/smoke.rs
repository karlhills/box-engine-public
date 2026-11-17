//! GPU smoke test: renders a small lit, shadowed scene headlessly and writes
//! captures to `target/captures/`. Skipped when `BOX_SKIP_GPU_TESTS` is set or
//! no GPU adapter exists.

use std::path::PathBuf;

use box_core::prelude::*;
use box_core::transform::propagate_transforms;
use box_render::{
    Camera, DebugDraw, DirectionalLight, Environment, MeshRenderer, RenderError, RenderSettings,
    Renderer, StandardMaterial,
};

const W: u32 = 960;
const H: u32 = 540;

fn captures_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn material(world: &mut World, m: StandardMaterial) -> Handle<StandardMaterial> {
    world
        .resource_or_default::<Assets<StandardMaterial>>()
        .add(m)
}

fn mesh(world: &mut World, m: MeshData) -> Handle<MeshData> {
    world.resource_or_default::<Assets<MeshData>>().add(m)
}

fn color_material(base: Color, roughness: f32, metallic: f32) -> StandardMaterial {
    StandardMaterial {
        base_color: base,
        roughness,
        metallic,
        ..Default::default()
    }
}

/// Ground, a few primitives with different materials, distant pillars for
/// fog, a camera and a sun.
fn build_scene(world: &mut World, sun_direction: Vec3, sun: DirectionalLight) {
    let ground_mesh = mesh(world, MeshData::grid(200.0, 200.0, 20, 20));
    let cube = mesh(world, MeshData::cuboid(Vec3::ONE));
    let sphere = mesh(world, MeshData::uv_sphere(1.0, 48, 24));
    let cylinder = mesh(world, MeshData::cylinder(0.5, 1.0, 32));
    // Vertex colours multiply the base colour: a two-tone box.
    let mut tinted = MeshData::cuboid(Vec3::ONE);
    tinted.colors = tinted
        .positions
        .iter()
        .map(|p| {
            if p.y > 0.0 {
                [1.0, 1.0, 1.0, 1.0]
            } else {
                [0.35, 0.35, 0.35, 1.0]
            }
        })
        .collect();
    let tinted = mesh(world, tinted);

    let ground = material(
        world,
        color_material(Color::linear(0.16, 0.22, 0.09), 0.95, 0.0),
    );
    let brick = material(
        world,
        color_material(Color::linear(0.62, 0.18, 0.10), 0.7, 0.0),
    );
    let stone = material(
        world,
        color_material(Color::linear(0.42, 0.40, 0.37), 0.85, 0.0),
    );
    let gold = material(
        world,
        color_material(Color::linear(1.0, 0.77, 0.34), 0.28, 1.0),
    );
    let plastic = material(
        world,
        color_material(Color::linear(0.10, 0.30, 0.75), 0.35, 0.0),
    );
    let white = material(
        world,
        color_material(Color::linear(0.75, 0.75, 0.75), 0.6, 0.0),
    );
    let glow = material(
        world,
        StandardMaterial {
            base_color: Color::linear(0.1, 0.1, 0.1),
            emissive: Color::linear(4.0, 1.6, 0.4),
            ..Default::default()
        },
    );

    let spawn = |world: &mut World, t: Transform, mesh, material| {
        world.spawn((t, MeshRenderer::new(mesh, material)))
    };
    spawn(world, Transform::default(), ground_mesh, ground);
    spawn(
        world,
        Transform::from_xyz(-2.2, 0.75, 0.0).with_scale(Vec3::splat(1.5)),
        cube,
        brick,
    );
    spawn(
        world,
        Transform::from_xyz(-0.2, 1.5, -2.5).with_scale(Vec3::new(1.0, 3.0, 1.0)),
        cube,
        stone,
    );
    spawn(world, Transform::from_xyz(1.2, 1.0, 0.4), sphere, gold);
    spawn(
        world,
        Transform::from_xyz(3.4, 0.6, -1.2).with_scale(Vec3::splat(0.6)),
        sphere,
        plastic,
    );
    spawn(
        world,
        Transform::from_xyz(-0.6, 1.0, 2.4).with_scale(Vec3::new(1.2, 2.0, 1.2)),
        cylinder,
        white,
    );
    spawn(
        world,
        Transform::from_xyz(2.0, 0.5, 2.6).with_rotation(Quat::from_rotation_y(0.6)),
        tinted,
        white,
    );
    spawn(
        world,
        Transform::from_xyz(0.9, 0.2, 2.0).with_scale(Vec3::splat(0.4)),
        cube,
        glow,
    );
    // Pillars receding into the fog.
    for i in 0..10 {
        let z = -8.0 - i as f32 * 9.0;
        for x in [-9.0, 9.0] {
            spawn(
                world,
                Transform::from_xyz(x, 2.5, z).with_scale(Vec3::new(1.2, 5.0, 1.2)),
                cube,
                stone,
            );
        }
    }

    world.spawn((
        Transform::from_xyz(7.5, 4.0, 9.0).looking_at(Vec3::new(0.0, 1.0, -1.0), Vec3::Y),
        Camera::default(),
    ));
    world.spawn((Transform::default().looking_to(sun_direction, Vec3::Y), sun));

    let dd = world.resource_or_default::<DebugDraw>();
    dd.aabb(
        &Aabb::new(Vec3::new(-2.95, 0.0, -0.75), Vec3::new(-1.45, 1.5, 0.75)),
        Color::linear(1.0, 0.9, 0.1),
    );
    dd.sphere(Vec3::new(1.2, 1.0, 0.4), 1.15, Color::linear(0.1, 0.9, 1.0));
    dd.grid(
        Vec3::new(4.0, 0.01, 4.0),
        1.5,
        0.5,
        Color::linear(0.9, 0.9, 0.9),
    );
    dd.axes(Vec3::new(0.0, 0.02, 0.0), 1.0);
    dd.line_overlay(
        Vec3::new(-3.0, 3.0, 0.0),
        Vec3::new(3.0, 3.0, 0.0),
        Color::linear(1.0, 0.2, 0.8),
    );

    propagate_transforms(world);
}

fn renderer() -> Option<Renderer> {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping GPU smoke test");
        return None;
    }
    match Renderer::new_headless(W, H) {
        Ok(r) => Some(r),
        Err(RenderError::NoAdapter) => {
            eprintln!("no GPU adapter; skipping GPU smoke test");
            None
        }
        Err(e) => panic!("renderer creation failed: {e}"),
    }
}

fn assert_not_uniform(img: &image::RgbaImage) {
    let first = img.get_pixel(0, 0);
    let differing = img.pixels().filter(|p| *p != first).count();
    assert!(
        differing > (W * H / 4) as usize,
        "image is (nearly) uniform: {differing} pixels differ from the first"
    );
}

#[test]
fn render_smoke() {
    let Some(mut r) = renderer() else { return };
    eprintln!(
        "adapter: {} ({:?})",
        r.adapter_info().name,
        r.adapter_info().backend
    );
    assert_eq!(r.size(), (W, H));
    let dir = captures_dir();

    // Day: sun at ~35 degrees elevation from the left.
    let mut world = World::new();
    world.insert_resource(Environment::day());
    build_scene(
        &mut world,
        Vec3::new(0.65, -0.55, -0.45),
        DirectionalLight::default(),
    );
    let day = r.capture(&mut world).expect("capture day");
    day.save(dir.join("render_smoke_day.png")).unwrap();
    let s = r.stats().clone();
    eprintln!("day stats: {s:?}");
    assert_not_uniform(&day);
    assert_eq!(s.validation_errors, 0);
    assert_eq!(s.objects, 28);
    assert!(s.visible > 0 && s.visible <= s.objects);
    assert!(s.draws > s.visible);
    assert!(
        world.resource::<DebugDraw>().is_empty(),
        "debug draw cleared"
    );

    // Evening: low warm sun in view, cascade debug tint.
    let mut world = World::new();
    world.insert_resource(Environment::evening());
    world.insert_resource(RenderSettings {
        show_cascades: true,
        ..Default::default()
    });
    build_scene(
        &mut world,
        Vec3::new(0.55, -0.1, 0.83),
        DirectionalLight {
            color: Color::linear(1.0, 0.62, 0.32),
            intensity: 2.6,
            ..Default::default()
        },
    );
    let evening = r.capture(&mut world).expect("capture evening");
    evening.save(dir.join("render_smoke_cascades.png")).unwrap();
    assert_not_uniform(&evening);

    // Evening without the debug tint.
    world.resource_mut::<RenderSettings>().show_cascades = false;
    let evening = r.capture(&mut world).expect("capture evening");
    evening.save(dir.join("render_smoke_evening.png")).unwrap();
    assert_not_uniform(&evening);
    assert_eq!(r.stats().validation_errors, 0);

    // A world with nothing in it still draws the sky.
    let mut empty = World::new();
    let img = r.capture(&mut empty).expect("capture empty");
    assert_eq!(img.dimensions(), (W, H));
    assert_eq!(r.stats().validation_errors, 0);

    // Resize, then render + capture again.
    r.resize(480, 270);
    r.render(&mut world).expect("render after resize");
    let small = r.capture(&mut world).expect("capture after resize");
    assert_eq!(small.dimensions(), (480, 270));
    assert_eq!(r.stats().validation_errors, 0);
}

/// `Visible(false)` hides an entity and its subtree (meshes, shadows and
/// point lights); showing it again draws it. Captures
/// `render_visible_{shown,hidden}.png` (M8 gap report item 9).
#[test]
fn visible_false_hides_a_subtree() {
    let Some(mut r) = renderer() else { return };
    let dir = captures_dir();
    let mut world = World::new();
    world.insert_resource(Environment::day());
    build_scene(
        &mut world,
        Vec3::new(0.65, -0.55, -0.45),
        DirectionalLight::default(),
    );
    // A red pillar group in front of the camera: a parent cube with a
    // child cube on top and a red point light.
    let cube = mesh(&mut world, MeshData::cuboid(Vec3::ONE));
    let red = material(
        &mut world,
        color_material(Color::srgb(0.9, 0.05, 0.05), 0.5, 0.0),
    );
    let camera_at = world
        .query::<(&Camera, &GlobalTransform)>()
        .iter()
        .next()
        .map(|(_, g)| g.translation())
        .unwrap();
    let group = world.spawn((
        Transform::from_translation(camera_at + Vec3::new(0.0, -0.8, -4.0)),
        MeshRenderer::new(cube, red),
    ));
    let top = world.spawn((
        Transform::from_xyz(0.0, 1.0, 0.0),
        MeshRenderer::new(cube, red),
    ));
    let lamp = world.spawn((
        Transform::from_xyz(0.0, 2.0, 1.0),
        box_render::PointLight {
            color: Color::srgb(1.0, 0.1, 0.1),
            intensity: 40.0,
            range: 6.0,
            ..Default::default()
        },
    ));
    world.set_parent(top, Some(group)).unwrap();
    world.set_parent(lamp, Some(group)).unwrap();
    propagate_transforms(&mut world);
    let reddish = |img: &image::RgbaImage| {
        img.pixels()
            .filter(|p| p[0] as i32 - p[1] as i32 > 80 && p[0] as i32 - p[2] as i32 > 80)
            .count()
    };

    let shown = r.capture(&mut world).expect("capture shown");
    shown.save(dir.join("render_visible_shown.png")).unwrap();
    let objects = r.stats().objects;
    let lights = r.stats().point_lights;
    world.insert_one(group, Visible(false)).unwrap();
    let hidden = r.capture(&mut world).expect("capture hidden");
    hidden.save(dir.join("render_visible_hidden.png")).unwrap();
    assert_eq!(r.stats().objects, objects - 2, "parent and child");
    assert_eq!(r.stats().point_lights, lights - 1);
    let (red_shown, red_hidden) = (reddish(&shown), reddish(&hidden));
    assert!(
        red_shown > 5000 && red_hidden < red_shown / 20,
        "{red_shown} red pixels shown, {red_hidden} hidden"
    );
    world.insert_one(group, Visible(true)).unwrap();
    let again = r.capture(&mut world).expect("capture shown again");
    // Equal up to a few pixels (exposure adapts over frames).
    assert!(reddish(&again).abs_diff(red_shown) < red_shown / 100);
    assert_eq!(r.stats().validation_errors, 0);
}
