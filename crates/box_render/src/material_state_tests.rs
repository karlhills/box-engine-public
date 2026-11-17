//! R6/R7 render tests: per-material pipeline state (sort bias, depth write
//! and test, `Multiply2x`, the depth pre-write), scene depth in material
//! shaders and culling-bounds padding. Rendered headlessly from a camera
//! 4 m above the origin looking down (+X right, −Z up the image; the
//! ground's half-width in view is 4 tan 25° ≈ 1.865 m).

use box_core::asset_ref::{AssetId, AssetRef};
use box_core::assets::{AssetLookup, Assets};
use box_core::color::Color;
use box_core::mesh::MeshData;
use box_core::shader_material::ShaderSource;
use box_core::transform::{Transform, propagate_transforms};
use box_core::world::World;
use glam::Vec3;

use crate::components::*;
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

/// Linear RGB (sRGB decoded) of a pixel.
fn linear(img: &image::RgbaImage, (x, y): (u32, u32)) -> Vec3 {
    let c = |v: u8| {
        let c = v as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let p = img.get_pixel(x, y).0;
    Vec3::new(c(p[0]), c(p[1]), c(p[2]))
}

/// A grey ground plane seen from above: no sun, no post effects, no fog.
fn ground_world() -> World {
    let mut world = World::new();
    let env = world.resource_or_default::<Environment>();
    env.ambient_occlusion.enabled = false;
    env.bloom.enabled = false;
    env.fog_density = 0.0;
    env.ambient_intensity = 0.6;
    env.exposure = 1.0;
    env.tonemapper = Tonemapper::None;
    world.spawn((
        Transform::from_xyz(0.0, 4.0, 0.0).looking_at(Vec3::ZERO, Vec3::NEG_Z),
        Camera::default(),
    ));
    let ground = standard(
        &mut world,
        StandardMaterial {
            base_color: Color::linear(0.5, 0.5, 0.5),
            roughness: 1.0,
            ..Default::default()
        },
    );
    let plane = mesh(&mut world, MeshData::plane(8.0, 8.0));
    world.spawn((Transform::default(), MeshRenderer::new(plane, ground)));
    world
}

fn standard(world: &mut World, m: StandardMaterial) -> MaterialHandle {
    world
        .resource_or_default::<Assets<StandardMaterial>>()
        .add(m)
        .into()
}

fn mesh(world: &mut World, m: MeshData) -> box_core::assets::Handle<MeshData> {
    world.resource_or_default::<Assets<MeshData>>().add(m)
}

/// A shader material from `text`, registered under `path` the way the asset
/// server does.
fn shader(world: &mut World, path: &str, text: &str, m: ShaderMaterial) -> MaterialHandle {
    let handle = world
        .resource_or_default::<Assets<ShaderSource>>()
        .add(ShaderSource::new(path, text));
    world
        .resource_or_default::<AssetLookup>()
        .insert(AssetId::new(), None, Some(path), handle);
    world
        .resource_or_default::<Assets<ShaderMaterial>>()
        .add(ShaderMaterial {
            shader: AssetRef::path(path),
            ..m
        })
        .into()
}

fn spawn(world: &mut World, at: Vec3, m: MeshData, material: MaterialHandle) {
    let mesh = mesh(world, m);
    world.spawn((
        Transform::from_translation(at),
        MeshRenderer::new(mesh, material),
    ));
}

fn capture(r: &mut Renderer, world: &mut World) -> image::RgbaImage {
    propagate_transforms(world);
    // The first frame compiles material shaders; the second draws with them.
    r.capture(world).unwrap();
    r.capture(world).unwrap()
}

fn near(a: Vec3, b: Vec3, tolerance: f32) -> bool {
    (a - b).abs().max_element() <= tolerance
}

/// Mid grey leaves the ground unchanged, white doubles it.
#[test]
fn multiply_2x_keeps_mid_grey_and_doubles_white() {
    let Some(mut r) = headless() else {
        eprintln!("skipping: no GPU");
        return;
    };
    let mut world = ground_world();
    let plain = capture(&mut r, &mut world);
    let decal = |world: &mut World, grey: f32, x: f32| {
        let m = standard(
            world,
            StandardMaterial {
                base_color: Color::linear(grey, grey, grey),
                alpha_mode: AlphaMode::Multiply2x,
                ..Default::default()
            },
        );
        spawn(world, Vec3::new(x, 0.05, 0.0), MeshData::plane(1.2, 1.2), m);
    };
    decal(&mut world, 0.5, -0.9);
    decal(&mut world, 1.0, 0.9);
    let img = capture(&mut r, &mut world);
    let ground = linear(&plain, px(0.0, 1.2));
    assert!(ground.x > 0.05 && ground.x < 0.45, "ground {ground}");
    let neutral = linear(&img, px(-0.9, 0.0));
    assert!(near(neutral, ground, 0.02), "{neutral} vs {ground}");
    let doubled = linear(&img, px(0.9, 0.0));
    let ratio = doubled.x / ground.x;
    assert!((ratio - 2.0).abs() < 0.15, "white doubles: x{ratio}");
    assert_eq!(r.validation_errors(), 0);
}

/// Two overlapping blended sheets: the nearer is on top unless the farther
/// one has a higher sort bias.
#[test]
fn sort_bias_overrides_the_back_to_front_order() {
    let Some(mut r) = headless() else {
        eprintln!("skipping: no GPU");
        return;
    };
    let render = |r: &mut Renderer, blue_bias: i32| {
        let mut world = ground_world();
        let sheet = |world: &mut World, color: Color, bias: i32, y: f32| {
            let m = standard(
                world,
                StandardMaterial {
                    base_color: Color::BLACK.with_alpha(0.75),
                    emissive: color,
                    alpha_mode: AlphaMode::Blend,
                    sort_bias: bias,
                    ..Default::default()
                },
            );
            spawn(world, Vec3::new(0.0, y, 0.0), MeshData::plane(2.0, 2.0), m);
        };
        sheet(&mut world, Color::linear(1.0, 0.0, 0.0), 0, 0.6);
        sheet(&mut world, Color::linear(0.0, 0.0, 1.0), blue_bias, 0.3);
        let img = capture(r, &mut world);
        linear(&img, px(0.0, 0.0))
    };
    let default = render(&mut r, 0);
    assert!(default.x > default.z, "nearer red on top: {default}");
    let biased = render(&mut r, 1);
    assert!(biased.z > biased.x, "biased blue on top: {biased}");
    assert_eq!(r.validation_errors(), 0);
}

/// Depth test off draws through an opaque cube; depth write on makes a
/// transparent sheet hide transparent surfaces drawn after it and behind.
#[test]
fn depth_test_and_write_overrides() {
    let Some(mut r) = headless() else {
        eprintln!("skipping: no GPU");
        return;
    };
    let render = |r: &mut Renderer, test: bool| {
        let mut world = ground_world();
        let cube = standard(&mut world, StandardMaterial::default());
        spawn(&mut world, Vec3::ZERO, MeshData::cuboid(Vec3::ONE), cube);
        let glow = standard(
            &mut world,
            StandardMaterial {
                base_color: Color::BLACK,
                emissive: Color::linear(0.0, 1.0, 0.0),
                alpha_mode: AlphaMode::Additive,
                depth_test: test,
                ..Default::default()
            },
        );
        // Below the cube's top face (y = 0.5).
        spawn(&mut world, Vec3::Y * 0.2, MeshData::plane(2.0, 2.0), glow);
        let img = capture(r, &mut world);
        (linear(&img, px(0.0, 0.0)), linear(&img, px(0.8, 0.0)))
    };
    let (hidden, beside) = render(&mut r, true);
    let (through, _) = render(&mut r, false);
    assert!(beside.y > hidden.y + 0.3, "tested: hidden under the cube");
    assert!(through.y > hidden.y + 0.3, "untested: drawn over the cube");

    // Depth write: a near sheet that writes depth hides a far one drawn
    // after it (higher bias), which it would not without.
    let render = |r: &mut Renderer, write: DepthWrite| {
        let mut world = ground_world();
        let near_sheet = standard(
            &mut world,
            StandardMaterial {
                base_color: Color::BLACK.with_alpha(0.3),
                emissive: Color::linear(0.2, 0.0, 0.0),
                alpha_mode: AlphaMode::Blend,
                depth_write: write,
                ..Default::default()
            },
        );
        let far_sheet = standard(
            &mut world,
            StandardMaterial {
                base_color: Color::BLACK,
                emissive: Color::linear(0.0, 0.0, 1.0),
                alpha_mode: AlphaMode::Additive,
                sort_bias: 1,
                ..Default::default()
            },
        );
        let sheet = MeshData::plane(2.0, 2.0);
        spawn(&mut world, Vec3::Y * 0.6, sheet.clone(), near_sheet);
        spawn(&mut world, Vec3::Y * 0.3, sheet, far_sheet);
        let img = capture(r, &mut world);
        linear(&img, px(0.0, 0.0))
    };
    let auto = render(&mut r, DepthWrite::Auto);
    let written = render(&mut r, DepthWrite::On);
    assert!(auto.z > 0.5, "far glow added: {auto}");
    assert!(
        auto.z - written.z > 0.5 && (auto.x - written.x).abs() < 0.01,
        "far glow hidden by the near depth: {written} vs {auto}"
    );
    assert_eq!(r.validation_errors(), 0);
}

/// A double-sided transparent cube with `prewrite_depth` shows only its
/// nearest layer: the same as one sheet at its top face.
#[test]
fn depth_prewrite_draws_only_the_nearest_layer() {
    let Some(mut r) = headless() else {
        eprintln!("skipping: no GPU");
        return;
    };
    let ghost = |prewrite: bool| StandardMaterial {
        base_color: Color::linear(0.1, 0.6, 0.9).with_alpha(0.5),
        alpha_mode: AlphaMode::Blend,
        double_sided: true,
        prewrite_depth: prewrite,
        ..Default::default()
    };
    let render = |r: &mut Renderer, m: StandardMaterial, cube: bool| {
        let mut world = ground_world();
        let m = standard(&mut world, m);
        // Above the ground, so the bottom face is not hidden by it.
        if cube {
            spawn(&mut world, Vec3::Y * 0.6, MeshData::cuboid(Vec3::ONE), m);
        } else {
            spawn(&mut world, Vec3::Y * 1.1, MeshData::plane(1.0, 1.0), m);
        }
        let img = capture(r, &mut world);
        linear(&img, px(0.0, 0.0))
    };
    let layered = render(&mut r, ghost(false), true);
    let ghosted = render(&mut r, ghost(true), true);
    let single = render(&mut r, ghost(false), false);
    assert!(
        near(ghosted, single, 0.01),
        "{ghosted} vs one layer {single}"
    );
    assert!(
        !near(layered, single, 0.01),
        "{layered}: two layers without"
    );
    assert_eq!(r.validation_errors(), 0);
}

const SOFT_EDGE: &str = "\
@material struct Soft { @range(0.1, 4.0) depth: f32 = 0.5 }
fn surface(in: SurfaceInput, p: Soft) -> Surface {
    var s = default_surface(in);
    s.base_color = vec3f(0.0);
    s.emissive = vec3f(1.0, 1.0, 1.0);
    let gap = scene_depth(in.frag_coord) - fragment_view_depth(in.frag_coord);
    s.alpha = saturate(gap / p.depth);
    return s;
}
";

/// A material shader reading the scene depth fades out where it meets
/// geometry, and the prepass runs for it alone.
#[test]
fn material_shaders_read_the_scene_depth() {
    let Some(mut r) = headless() else {
        eprintln!("skipping: no GPU");
        return;
    };
    let mut world = ground_world();
    let cube = standard(&mut world, StandardMaterial::default());
    // Top face at y = 0.45, just under the sheet at 0.5.
    spawn(
        &mut world,
        Vec3::new(0.9, 0.0, 0.0),
        MeshData::cuboid(Vec3::splat(0.9)),
        cube,
    );
    let plain = capture(&mut r, &mut world);
    let soft = shader(
        &mut world,
        "shaders/soft.wgsl",
        SOFT_EDGE,
        ShaderMaterial {
            alpha_mode: AlphaMode::Blend,
            ..Default::default()
        },
    );
    spawn(&mut world, Vec3::Y * 0.5, MeshData::plane(3.0, 3.0), soft);
    let img = capture(&mut r, &mut world);
    // Over the ground the gap is 0.5 m: fully opaque white emission.
    let open = linear(&img, px(-0.9, 0.0));
    assert!(open.x > 0.9, "deep: opaque {open}");
    // Over the cube top the gap is 5 cm: 10 % coverage.
    let (shallow, under) = (linear(&img, px(0.9, 0.0)), linear(&plain, px(0.9, 0.0)));
    let coverage = (shallow.x - under.x) / (1.0 - under.x);
    assert!((coverage - 0.1).abs() < 0.06, "shallow: {coverage}");
    assert_eq!(r.validation_errors(), 0);
}

const SHIFT: &str = "\
fn surface(in: SurfaceInput, p: MaterialParams) -> Surface {
    var s = default_surface(in);
    s.base_color = vec3f(0.0);
    s.emissive = vec3f(1.0, 0.0, 0.0);
    return s;
}
fn vertex(position: vec3f, in: VertexInput, p: MaterialParams) -> vec3f {
    return position + vec3f(3.0, 0.0, 0.0);
}
";

/// A vertex hook moves a cube 3 m into view from just outside it: culled
/// by its mesh bounds alone, drawn with `bounds_padding`.
#[test]
fn bounds_padding_keeps_displaced_meshes_drawn() {
    let Some(mut r) = headless() else {
        eprintln!("skipping: no GPU");
        return;
    };
    let render = |r: &mut Renderer, padding: f32| {
        let mut world = ground_world();
        let m = shader(
            &mut world,
            "shaders/shift.wgsl",
            SHIFT,
            ShaderMaterial {
                bounds_padding: padding,
                ..Default::default()
            },
        );
        // Mesh bounds x -3.9..-3.1, outside the view (|x| < 1.865 at the
        // ground, less higher up); drawn at x -0.9..-0.1.
        spawn(
            &mut world,
            Vec3::new(-3.5, 0.4, 0.0),
            MeshData::cuboid(Vec3::splat(0.8)),
            m,
        );
        let img = capture(r, &mut world);
        (linear(&img, px(-0.5, 0.0)), r.stats().visible)
    };
    let (culled, visible) = render(&mut r, 0.0);
    assert_eq!(visible, 1, "only the ground");
    assert!(culled.x < 0.5, "not drawn: {culled}");
    let (drawn, visible) = render(&mut r, 3.0);
    assert_eq!(visible, 2, "the padded cube is visible");
    assert!(drawn.x > 0.9 && drawn.y < 0.1, "drawn red: {drawn}");
    assert_eq!(r.validation_errors(), 0);
}
