//! RS2 material scenes, shared by the `materials` example and the golden
//! tests: textures, alpha modes, double-sided faces, point lights, multiple
//! materials per mesh and a custom shader material.
//!
//! Textures and shaders are made in code and registered the way the asset
//! server registers loaded files: in `Assets<ImageData>` /
//! `Assets<ShaderSource>` plus a path entry in the `AssetLookup`; materials
//! reference them by `AssetRef` path.

use std::f32::consts::FRAC_PI_2;

use box_engine::prelude::*;

use super::{TimeOfDay, set_time_of_day};

/// The material scenes, by name (`--scene` of the `materials` example).
pub const MATERIAL_SCENES: &[&str] = &[
    "textured",
    "alpha_modes",
    "double_sided",
    "point_lights_night",
    "point_lights_day",
    "multi_material",
    "shader_material",
];

/// Builds the material scene `name` with its lights and camera. Returns
/// false for an unknown name.
pub fn build_material_scene(world: &mut World, name: &str) -> bool {
    match name {
        "textured" => build_textured(world),
        "alpha_modes" => build_alpha_modes(world),
        "double_sided" => build_double_sided(world),
        "point_lights_night" => build_point_lights(world, false),
        "point_lights_day" => build_point_lights(world, true),
        "multi_material" => build_multi_material(world),
        "shader_material" => build_shader_material(world),
        _ => return false,
    }
    true
}

/// Registers `image` (with a full mip chain) under `path`, as the asset
/// server does for a loaded file, and returns a reference to it.
pub fn add_image(world: &mut World, path: &str, mut image: ImageData) -> AssetRef {
    image.generate_mips();
    let handle = world.resource_or_default::<Assets<ImageData>>().add(image);
    world
        .resource_or_default::<AssetLookup>()
        .insert(AssetId::new(), None, Some(path), handle);
    AssetRef::path(path)
}

/// Registers a material shader under `path` and returns a reference to it.
pub fn add_shader(world: &mut World, path: &str, text: &str) -> AssetRef {
    let handle = world
        .resource_or_default::<Assets<ShaderSource>>()
        .add(ShaderSource::new(path, text));
    world
        .resource_or_default::<AssetLookup>()
        .insert(AssetId::new(), None, Some(path), handle);
    AssetRef::path(path)
}

/// An image from a per-pixel function.
fn image(size: u32, space: ColorSpace, f: impl Fn(f32, f32) -> [u8; 4]) -> ImageData {
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (u, v) = (
                (x as f32 + 0.5) / size as f32,
                (y as f32 + 0.5) / size as f32,
            );
            rgba.extend(f(u, v));
        }
    }
    ImageData::new(size, size, space, rgba).expect("valid image")
}

/// A tangent-space normal map from a height function (texture rows run
/// along +V, matching the engine's bitangent).
fn normal_map(size: u32, strength: f32, height: impl Fn(f32, f32) -> f32) -> ImageData {
    let e = 1.0 / size as f32;
    image(size, ColorSpace::Linear, |u, v| {
        let dx = (height(u + e, v) - height(u - e, v)) / (2.0 * e);
        let dy = (height(u, v + e) - height(u, v - e)) / (2.0 * e);
        let n = Vec3::new(-dx * strength, -dy * strength, 1.0).normalize();
        let c = (n * 0.5 + Vec3::splat(0.5)) * 255.0;
        [c.x as u8, c.y as u8, c.z as u8, 255]
    })
}

fn unit(x: f32) -> u8 {
    (x.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// Cheap integer hash to 0..1.
fn hash(a: u32, b: u32) -> f32 {
    let mut h = a.wrapping_mul(0x9e37_79b9) ^ b.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    (h & 0xffff) as f32 / 65535.0
}

/// Brick layout: (brick column, row, distance to the nearest mortar edge in
/// UV units).
fn brick(u: f32, v: f32) -> (u32, u32, f32) {
    let (rows, cols) = (8.0, 4.0);
    let row = (v * rows).floor();
    let shifted = u * cols + if row as u32 % 2 == 1 { 0.5 } else { 0.0 };
    let col = shifted.floor();
    let fx = (shifted - col).min(1.0 - (shifted - col)) / cols;
    let fy = (v * rows - row).min(1.0 - (v * rows - row)) / rows;
    (col as u32 % 4, row as u32, fx.min(fy))
}

fn std_material(world: &mut World, m: StandardMaterial) -> Handle<StandardMaterial> {
    world
        .resource_or_default::<Assets<StandardMaterial>>()
        .add(m)
}

fn add_mesh(world: &mut World, m: MeshData) -> Handle<MeshData> {
    world.resource_or_default::<Assets<MeshData>>().add(m)
}

fn spawn(
    world: &mut World,
    t: Transform,
    mesh: Handle<MeshData>,
    material: impl Into<MaterialHandle>,
) -> Entity {
    world.spawn((t, MeshRenderer::new(mesh, material)))
}

fn ground(world: &mut World, size: f32) {
    let floor = std_material(
        world,
        StandardMaterial {
            base_color: Color::hex(0x8f8a80),
            roughness: 0.9,
            ..Default::default()
        },
    );
    let plane = add_mesh(world, MeshData::plane(size, size));
    spawn(world, Transform::default(), plane, floor);
}

fn sun_and_camera(world: &mut World, orbit: OrbitCamera) {
    super::spawn_sun_and_camera(world, orbit);
    set_time_of_day(world, TimeOfDay::Day);
}

/// A quad of `w` x `h` facing +Z (V runs down the quad, like image rows).
fn quad(w: f32, h: f32) -> MeshData {
    let mut m = MeshData::plane(w, h);
    let r = Quat::from_rotation_x(FRAC_PI_2);
    for p in &mut m.positions {
        *p = r * *p;
    }
    for n in &mut m.normals {
        *n = r * *n;
    }
    m.with_tangents()
}

/// Bricks (base colour, normal and ORM maps) on a ground plane, and a
/// metal-striped, dimpled sphere.
pub fn build_textured(world: &mut World) {
    let brick_color = add_image(
        world,
        "textures/bricks_color.png",
        image(256, ColorSpace::Srgb, |u, v| {
            let (c, r, edge) = brick(u, v);
            if edge < 0.012 {
                [200, 196, 186, 255]
            } else {
                let k = 0.8 + 0.35 * hash(c, r);
                [unit(0.62 * k), unit(0.27 * k), unit(0.18 * k), 255]
            }
        }),
    );
    let brick_height = |u: f32, v: f32| {
        let (_, _, edge) = brick(u.rem_euclid(1.0), v.rem_euclid(1.0));
        (edge / 0.03).min(1.0)
    };
    let brick_normal = add_image(
        world,
        "textures/bricks_normal.png",
        normal_map(256, 0.04, brick_height),
    );
    let brick_orm = add_image(
        world,
        "textures/bricks_orm.png",
        image(256, ColorSpace::Linear, |u, v| {
            let (_, _, edge) = brick(u, v);
            let mortar = edge < 0.012;
            let ao = 0.45 + 0.55 * (edge / 0.03).min(1.0);
            [unit(ao), if mortar { 255 } else { 190 }, 0, 255]
        }),
    );
    let bricks = std_material(
        world,
        StandardMaterial {
            base_color: Color::WHITE,
            roughness: 1.0,
            base_color_texture: brick_color,
            normal_texture: brick_normal,
            orm_texture: brick_orm,
            ..Default::default()
        },
    );
    let mut plane = MeshData::plane(6.0, 6.0);
    for uv in &mut plane.uvs[0] {
        *uv *= 2.0;
    }
    let plane = add_mesh(world, plane.with_tangents());
    spawn(world, Transform::default(), plane, bricks);

    let checker = add_image(
        world,
        "textures/checker.png",
        image(128, ColorSpace::Srgb, |u, v| {
            let odd = ((u * 8.0).floor() + (v * 4.0).floor()) as u32 % 2 == 1;
            if odd {
                [214, 170, 80, 255]
            } else {
                [70, 110, 160, 255]
            }
        }),
    );
    let dimples = add_image(
        world,
        "textures/dimples_normal.png",
        normal_map(128, 0.02, |u, v| {
            let (fu, fv) = ((u * 16.0).fract() - 0.5, (v * 8.0).fract() - 0.5);
            (fu * fu + fv * fv).sqrt().min(0.5)
        }),
    );
    let stripes_orm = add_image(
        world,
        "textures/metal_stripes_orm.png",
        image(128, ColorSpace::Linear, |u, _| {
            let metal = ((u * 8.0).floor() as u32).is_multiple_of(2);
            if metal {
                [255, 80, 255, 255]
            } else {
                [255, 230, 0, 255]
            }
        }),
    );
    let ball = std_material(
        world,
        StandardMaterial {
            base_color: Color::WHITE,
            roughness: 1.0,
            metallic: 1.0,
            base_color_texture: checker,
            normal_texture: dimples,
            orm_texture: stripes_orm,
            ..Default::default()
        },
    );
    let sphere = add_mesh(world, MeshData::uv_sphere(0.9, 48, 24).with_tangents());
    spawn(world, Transform::from_xyz(0.6, 0.9, 0.4), sphere, ball);
    // The same sphere without textures, for comparison.
    let plain = std_material(
        world,
        StandardMaterial {
            base_color: Color::hex(0xb08a50),
            roughness: 0.5,
            ..Default::default()
        },
    );
    spawn(world, Transform::from_xyz(-1.6, 0.9, -0.6), sphere, plain);
    sun_and_camera(
        world,
        OrbitCamera {
            target: Vec3::new(-0.3, 0.5, 0.0),
            distance: 6.0,
            yaw: 0.35,
            pitch: 0.5,
            ..Default::default()
        },
    );
}

/// Mask (with shadow cut-outs), blend panes sorted against each other and
/// opaque cubes, an additive glow and multiply tinted glass.
pub fn build_alpha_modes(world: &mut World) {
    ground(world, 14.0);
    let cube = add_mesh(world, MeshData::cuboid(Vec3::ONE));
    let stone = std_material(
        world,
        StandardMaterial {
            base_color: Color::hex(0xd8d2c4),
            roughness: 0.8,
            ..Default::default()
        },
    );
    let pane = add_mesh(world, quad(1.6, 1.6));

    // Mask: a lattice of round holes, cut out in the shadow too.
    let holes = add_image(
        world,
        "textures/holes.png",
        image(128, ColorSpace::Srgb, |u, v| {
            let (fu, fv) = ((u * 4.0).fract() - 0.5, (v * 4.0).fract() - 0.5);
            let a = if fu * fu + fv * fv < 0.12 { 0 } else { 255 };
            [90, 160, 70, a]
        }),
    );
    let mask = std_material(
        world,
        StandardMaterial {
            base_color_texture: holes,
            base_color: Color::WHITE,
            alpha_mode: AlphaMode::Mask { cutoff: 0.5 },
            double_sided: true,
            ..Default::default()
        },
    );
    spawn(world, Transform::from_xyz(-3.0, 0.8, 0.0), pane, mask);

    // Blend: red in front of blue, both crossing an opaque cube.
    let red = std_material(
        world,
        StandardMaterial {
            base_color: Color::linear(0.9, 0.1, 0.08).with_alpha(0.5),
            roughness: 0.2,
            alpha_mode: AlphaMode::Blend,
            ..Default::default()
        },
    );
    let blue = std_material(
        world,
        StandardMaterial {
            base_color: Color::linear(0.1, 0.2, 0.95).with_alpha(0.5),
            roughness: 0.2,
            alpha_mode: AlphaMode::Blend,
            ..Default::default()
        },
    );
    spawn(world, Transform::from_xyz(-0.6, 0.8, -0.6), pane, blue);
    spawn(world, Transform::from_xyz(-0.1, 0.9, 0.4), pane, red);
    spawn(
        world,
        Transform::from_xyz(-0.3, 0.5, -0.1).with_scale(Vec3::new(0.6, 1.0, 0.6)),
        cube,
        stone,
    );

    // Additive glow over a cube.
    let glow = std_material(
        world,
        StandardMaterial {
            base_color: Color::BLACK,
            emissive: Color::linear(2.0, 0.7, 0.15),
            alpha_mode: AlphaMode::Additive,
            ..Default::default()
        },
    );
    spawn(world, Transform::from_xyz(2.0, 0.5, -0.6), cube, stone);
    spawn(world, Transform::from_xyz(2.0, 0.9, 0.3), pane, glow);

    // Multiply: cyan glass in front of a white cube.
    let glass = std_material(
        world,
        StandardMaterial {
            base_color: Color::linear(0.15, 0.75, 0.85),
            alpha_mode: AlphaMode::Multiply,
            ..Default::default()
        },
    );
    spawn(world, Transform::from_xyz(4.2, 0.5, -0.6), cube, stone);
    spawn(world, Transform::from_xyz(4.2, 0.8, 0.4), pane, glass);

    sun_and_camera(
        world,
        OrbitCamera {
            target: Vec3::new(0.6, 0.6, 0.0),
            distance: 7.0,
            yaw: 0.0,
            pitch: 0.25,
            ..Default::default()
        },
    );
}

/// Quads seen from the front and from behind: double-sided ones show both
/// faces (lit as their own side), a single-sided one disappears from behind.
pub fn build_double_sided(world: &mut World) {
    ground(world, 10.0);
    let arrow = add_image(
        world,
        "textures/arrow.png",
        image(128, ColorSpace::Srgb, |u, v| {
            // An arrow pointing right: tells front from back (mirrored).
            let shaft = (0.42..0.58).contains(&v) && (0.15..0.6).contains(&u);
            let head = (0.6..0.88).contains(&u) && (v - 0.5).abs() < (0.88 - u) * 0.9;
            if shaft || head {
                [240, 230, 210, 255]
            } else {
                [40, 120, 150, 255]
            }
        }),
    );
    let two_sided = std_material(
        world,
        StandardMaterial {
            base_color_texture: arrow.clone(),
            base_color: Color::WHITE,
            double_sided: true,
            ..Default::default()
        },
    );
    let one_sided = std_material(
        world,
        StandardMaterial {
            base_color_texture: arrow,
            base_color: Color::WHITE,
            ..Default::default()
        },
    );
    let pane = add_mesh(world, quad(1.6, 1.6));
    let facing = Transform::from_xyz(-2.0, 0.9, 0.0);
    let away = |x: f32| {
        Transform::from_xyz(x, 0.9, 0.0).with_rotation(Quat::from_rotation_y(std::f32::consts::PI))
    };
    spawn(world, facing, pane, two_sided);
    spawn(world, away(0.0), pane, two_sided);
    spawn(world, away(2.0), pane, one_sided);
    sun_and_camera(
        world,
        OrbitCamera {
            target: Vec3::new(0.0, 0.8, 0.0),
            distance: 6.5,
            yaw: 0.0,
            pitch: 0.15,
            ..Default::default()
        },
    );
}

/// Coloured point lights over a few objects, at night (no sun) or by day.
pub fn build_point_lights(world: &mut World, sun: bool) {
    ground(world, 16.0);
    let white = std_material(
        world,
        StandardMaterial {
            base_color: Color::hex(0xd8d4cc),
            roughness: 0.6,
            ..Default::default()
        },
    );
    let metal = std_material(
        world,
        StandardMaterial {
            base_color: Color::hex(0xc8c8d0),
            roughness: 0.25,
            metallic: 1.0,
            ..Default::default()
        },
    );
    let cube = add_mesh(world, MeshData::cuboid(Vec3::ONE));
    let sphere = add_mesh(world, MeshData::uv_sphere(0.6, 32, 16));
    let bulb = add_mesh(world, MeshData::uv_sphere(0.08, 12, 6));
    spawn(world, Transform::from_xyz(-1.5, 0.5, 0.0), cube, white);
    spawn(world, Transform::from_xyz(1.5, 0.6, 0.0), sphere, metal);
    spawn(world, Transform::from_xyz(0.0, 0.6, -1.8), sphere, white);
    for (pos, color, intensity) in [
        (
            Vec3::new(-2.6, 1.2, 1.0),
            Color::linear(1.0, 0.15, 0.1),
            4.0,
        ),
        (Vec3::new(0.0, 1.4, 0.6), Color::linear(0.2, 1.0, 0.25), 4.0),
        (Vec3::new(2.6, 1.2, 1.0), Color::linear(0.2, 0.35, 1.0), 5.0),
        (
            Vec3::new(0.0, 2.2, -2.8),
            Color::linear(1.0, 0.75, 0.4),
            6.0,
        ),
    ] {
        let marker = std_material(
            world,
            StandardMaterial {
                base_color: Color::BLACK,
                emissive: color.scaled(6.0),
                ..Default::default()
            },
        );
        world.spawn((
            Transform::from_translation(pos),
            PointLight {
                color,
                intensity,
                range: 6.0,
                ..Default::default()
            },
        ));
        let e = spawn(world, Transform::from_translation(pos), bulb, marker);
        if let Some(mut r) = world.get_mut::<MeshRenderer>(e) {
            r.cast_shadows = false;
        }
    }
    let orbit = OrbitCamera {
        target: Vec3::new(0.0, 0.6, 0.0),
        distance: 7.5,
        yaw: 0.0,
        pitch: 0.42,
        ..Default::default()
    };
    if sun {
        sun_and_camera(world, orbit);
    } else {
        *world.resource_or_default::<Environment>() = Environment {
            sky_zenith: Color::linear(0.004, 0.006, 0.02),
            sky_horizon: Color::linear(0.02, 0.025, 0.05),
            ground: Color::linear(0.01, 0.01, 0.01),
            ambient_intensity: 0.5,
            fog_density: 0.0,
            exposure: 1.0,
            ..Environment::evening()
        };
        world.spawn((orbit.transform(), Camera::default(), orbit));
    }
}

/// One mesh, two material groups: a cube with sides in one material and
/// top and bottom in another; a sphere split at the equator; and a cube with
/// only one material for two groups (the second draws with the default).
pub fn build_multi_material(world: &mut World) {
    ground(world, 10.0);
    let group = |first_index, index_count, material_slot| MaterialGroup {
        first_index,
        index_count,
        material_slot,
    };
    // Cuboid faces: +X, -X, +Y, -Y, +Z, -Z, six indices each.
    let mut cube = MeshData::cuboid(Vec3::ONE);
    cube.groups = vec![group(0, 12, 0), group(12, 12, 1), group(24, 12, 0)];
    let cube = add_mesh(world, cube);
    let mut sphere = MeshData::uv_sphere(0.7, 32, 16);
    let half = (sphere.indices.len() / 2) as u32 / 3 * 3;
    let total = sphere.indices.len() as u32;
    sphere.groups = vec![group(0, half, 0), group(half, total - half, 1)];
    let sphere = add_mesh(world, sphere);
    let red = std_material(
        world,
        StandardMaterial {
            base_color: Color::hex(0xb83a2a),
            roughness: 0.6,
            ..Default::default()
        },
    );
    let gold = std_material(
        world,
        StandardMaterial {
            base_color: Color::hex(0xe0b050),
            roughness: 0.3,
            metallic: 1.0,
            ..Default::default()
        },
    );
    world.spawn((
        Transform::from_xyz(-1.8, 0.5, 0.0),
        MeshRenderer::with_materials(cube, vec![red.into(), gold.into()]),
    ));
    world.spawn((
        Transform::from_xyz(0.0, 0.7, 0.0),
        MeshRenderer::with_materials(sphere, vec![gold.into(), red.into()]),
    ));
    world.spawn((
        Transform::from_xyz(1.8, 0.5, 0.0),
        MeshRenderer::with_materials(cube, vec![red.into()]),
    ));
    sun_and_camera(
        world,
        OrbitCamera {
            target: Vec3::new(0.0, 0.5, 0.0),
            distance: 6.0,
            yaw: 0.3,
            pitch: 0.45,
            ..Default::default()
        },
    );
}

/// The `stripes.wgsl` material shader with different parameters, plus a
/// broken shader, which draws with the magenta error material.
pub fn build_shader_material(world: &mut World) {
    use box_core::shader_material::ShaderParamValue as V;
    ground(world, 10.0);
    let stripes = add_shader(
        world,
        "shaders/stripes.wgsl",
        include_str!("../assets/stripes.wgsl"),
    );
    let broken = add_shader(
        world,
        "shaders/broken.wgsl",
        "fn surface(in: SurfaceInput, p: MaterialParams) -> Surface {\n    return 1.0;\n}\n",
    );
    let shader_materials = world.resource_or_default::<Assets<ShaderMaterial>>();
    let warm = shader_materials.add(ShaderMaterial::new(stripes.clone()));
    let cool = shader_materials.add(
        ShaderMaterial::new(stripes)
            .with_param("tint", V::Color(Color::linear(0.1, 0.35, 0.9)))
            .with_param("frequency", V::Float(3.0))
            .with_param("width", V::Float(0.3))
            .with_param("glow", V::Float(3.0)),
    );
    let error = shader_materials.add(ShaderMaterial::new(broken));
    let sphere = add_mesh(world, MeshData::uv_sphere(0.8, 48, 24));
    let cube = add_mesh(world, MeshData::cuboid(Vec3::splat(1.2)));
    let small = add_mesh(world, MeshData::cuboid(Vec3::splat(0.6)));
    spawn(world, Transform::from_xyz(-1.3, 0.8, 0.0), sphere, warm);
    spawn(world, Transform::from_xyz(1.2, 0.6, 0.0), cube, cool);
    spawn(world, Transform::from_xyz(0.0, 0.3, 1.6), small, error);
    sun_and_camera(
        world,
        OrbitCamera {
            target: Vec3::new(0.0, 0.6, 0.4),
            distance: 6.0,
            yaw: 0.25,
            pitch: 0.4,
            ..Default::default()
        },
    );
}
