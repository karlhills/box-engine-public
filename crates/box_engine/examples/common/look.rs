//! RS3 on-demand look scenes, shared by the `look` example and tests:
//! atmosphere (aerial haze, height mist, distance quieting) over a layered
//! sky, a reference grade and vignette, alpha-tested foliage shadows, wind
//! sway, soft transparency, lit windows at night, and a wide view of rock
//! pillars standing in a cloud sea.
//!
//! Colours, distances and grading are a stylized reference look (time-of-day
//! keys from midday to night); everything is built from engine primitives.

use std::f32::consts::{FRAC_PI_2, TAU};

use box_engine::box_render::SelectionOutline;
use box_engine::prelude::*;

use super::materials::{add_image, add_shader};
use super::{Sun, spawn_sun_and_camera};

/// The look scenes, by name (`--scene` of the `look` example).
pub const LOOK_SCENES: &[&str] = &[
    "pillars",
    "atmosphere",
    "foliage",
    "wind",
    "soft_particles",
    "night",
];

/// Builds the look scene `name` with its lights and camera. Returns false
/// for an unknown name.
pub fn build_look_scene(world: &mut World, name: &str) -> bool {
    match name {
        "pillars" => build_pillars(world),
        "atmosphere" => build_atmosphere(world),
        "foliage" => build_foliage(world),
        "wind" => build_wind(world),
        "soft_particles" => build_soft_particles(world),
        "night" => build_night(world),
        _ => return false,
    }
    true
}

/// The reference grade in the engine's terms: white balance 6/2, contrast
/// +12, saturation −4, and shadows/midtones/highlights. The SMH tints are
/// given gamma encoded, so the linear tints are value^2.2.
pub fn reference_grade() -> ColorGrading {
    let lin = |v: Vec3| v.powf(2.2);
    ColorGrading {
        temperature: 0.06,
        tint: 0.02,
        contrast: 1.12,
        saturation: 0.96,
        shadows: lin(Vec3::new(0.95, 0.98, 1.06)),
        highlights: lin(Vec3::new(1.03, 1.0, 0.96)),
        ..Default::default()
    }
}

/// The tone mapper of a `--tonemap` flag: `agx`, `aces`, `neutral` or `none`.
pub fn parse_tonemapper(name: &str) -> anyhow::Result<Tonemapper> {
    Ok(match name.to_ascii_lowercase().as_str() {
        "agx" => Tonemapper::AgX,
        "aces" | "acesfitted" => Tonemapper::AcesFitted,
        "neutral" => Tonemapper::Neutral,
        "none" => Tonemapper::None,
        _ => anyhow::bail!("unknown tone mapper `{name}`; available: agx, aces, neutral, none"),
    })
}

/// The reference vignette: intensity 0.18, smoothness 0.5, not rounded.
pub fn reference_vignette() -> Vignette {
    Vignette {
        intensity: 0.18,
        smoothness: 0.5,
        rounded: false,
    }
}

/// One time-of-day key: the sun and the environment colours.
#[derive(Clone, Copy)]
struct Mood {
    hour: f32,
    /// Sun azimuth and elevation (degrees) and intensity.
    yaw: f32,
    elevation: f32,
    intensity: f32,
    sun: Color,
    zenith: Color,
    upper: Color,
    horizon: Color,
    below: Color,
    ground: Color,
    haze: Color,
    haze_density: f32,
    mist: Color,
    ambient: f32,
    exposure: f32,
    stars: f32,
    horizon_glow: f32,
}

fn moods() -> [Mood; 4] {
    let c = Color::srgb;
    [
        Mood {
            hour: 13.0,
            yaw: -35.0,
            elevation: 58.0,
            intensity: 3.2,
            sun: c(1.0, 0.97, 0.92),
            zenith: c(0.33, 0.5, 0.78),
            upper: c(0.52, 0.68, 0.88),
            horizon: c(0.84, 0.87, 0.88),
            below: c(0.78, 0.82, 0.86),
            ground: c(0.4, 0.37, 0.33),
            haze: c(0.62, 0.75, 0.91),
            haze_density: 0.00045,
            mist: c(0.78 * 0.72, 0.83 * 0.78, 0.9 * 0.92),
            ambient: 1.0,
            exposure: 0.55,
            stars: 0.0,
            horizon_glow: 0.1,
        },
        Mood {
            hour: 16.5,
            yaw: -62.0,
            elevation: 31.0,
            intensity: 3.1,
            sun: c(1.0, 0.88, 0.7),
            zenith: c(0.36, 0.53, 0.78),
            upper: c(0.55, 0.7, 0.88),
            horizon: c(0.86, 0.87, 0.85),
            below: c(0.78, 0.82, 0.85),
            ground: c(0.45, 0.4, 0.35),
            haze: c(0.64, 0.74, 0.86),
            haze_density: 0.0005,
            mist: c(0.78 * 0.72, 0.83 * 0.78, 0.89 * 0.92),
            ambient: 1.0,
            exposure: 0.72,
            stars: 0.0,
            horizon_glow: 0.15,
        },
        Mood {
            hour: 19.1,
            yaw: -100.0,
            elevation: 4.0,
            intensity: 1.8,
            sun: c(1.0, 0.58, 0.34),
            zenith: c(0.3, 0.33, 0.52),
            upper: c(0.46, 0.46, 0.62),
            horizon: c(0.86, 0.66, 0.56),
            below: c(0.64, 0.56, 0.66),
            ground: c(0.26, 0.23, 0.26),
            haze: c(0.62, 0.56, 0.68),
            haze_density: 0.0007,
            mist: c(0.64 * 0.72, 0.58 * 0.78, 0.7 * 0.92),
            ambient: 0.8,
            exposure: 0.8,
            stars: 0.0,
            horizon_glow: 0.6,
        },
        Mood {
            hour: 21.5,
            yaw: -130.0,
            elevation: 26.0,
            intensity: 0.35,
            sun: c(0.58, 0.66, 0.95),
            zenith: c(0.03, 0.04, 0.09),
            upper: c(0.06, 0.08, 0.16),
            horizon: c(0.14, 0.17, 0.27),
            below: c(0.11, 0.13, 0.2),
            ground: c(0.05, 0.05, 0.07),
            haze: c(0.11, 0.14, 0.24),
            haze_density: 0.0007,
            mist: c(0.16, 0.2, 0.32),
            ambient: 0.6,
            exposure: 1.6,
            stars: 1.0,
            horizon_glow: 0.0,
        },
    ]
}

/// Sets the reference look at `hour` (13 midday .. 21.5 night, keys blended):
/// sun direction, colour and intensity, the layered sky with glow and
/// stars, haze, mist, quieting, grade and vignette. Only uniforms change,
/// so calling it every frame (a time-of-day slider) costs nothing extra.
pub fn set_reference_time(world: &mut World, hour: f32) {
    let keys = moods();
    let h = hour.clamp(keys[0].hour, keys[keys.len() - 1].hour);
    let i = keys
        .windows(2)
        .position(|w| h <= w[1].hour)
        .unwrap_or(keys.len() - 2);
    let (a, b) = (keys[i], keys[i + 1]);
    let t = ((h - a.hour) / (b.hour - a.hour)).clamp(0.0, 1.0);
    let f = |x: f32, y: f32| x + (y - x) * t;
    let col = |x: Color, y: Color| x.lerp(y, t);
    let env = world.resource_mut::<Environment>();
    env.sky_zenith = col(a.zenith, b.zenith);
    env.sky_horizon = col(a.horizon, b.horizon);
    env.ground = col(a.ground, b.ground);
    env.ambient_intensity = f(a.ambient, b.ambient);
    env.exposure = f(a.exposure, b.exposure);
    env.sun_disc_size = 0.7f32.to_radians();
    // Architecture fog (exponential 0.0009): constant density.
    env.fog_density = 0.0009;
    env.fog_height_falloff = 0.0;
    env.fog_base_height = 0.0;
    env.sky = Sky {
        gradient: SkyGradient::Layered {
            upper: col(a.upper, b.upper),
            below_horizon: col(a.below, b.below),
        },
        sun_glow: 0.8,
        sun_disc: 0.5,
        horizon_glow: f(a.horizon_glow, b.horizon_glow),
        stars: f(a.stars, b.stars),
    };
    let haze = col(a.haze, b.haze);
    env.aerial_haze = AerialHaze {
        enabled: true,
        color: haze,
        density: f(a.haze_density, b.haze_density),
        start_distance: 60.0,
        max_opacity: 0.97,
        sun_scatter: 0.12,
        sky_height: 0.08,
    };
    env.height_mist = HeightMist {
        enabled: true,
        color: col(a.mist, b.mist),
        top: -110.0,
        bottom: -235.0,
        max_opacity: 1.0,
        near_fade: 120.0,
    };
    env.distance_quieting = DistanceQuieting {
        enabled: true,
        start: 90.0,
        end: 1800.0,
        desaturate: 0.5,
        contrast: 0.38,
        tone: haze.scaled(0.82),
    };
    env.color_grading = reference_grade();
    env.vignette = reference_vignette();
    let (yaw, elevation) = (
        f(a.yaw, b.yaw).to_radians(),
        f(a.elevation, b.elevation).to_radians(),
    );
    let to_sun = Vec3::new(
        elevation.cos() * yaw.sin(),
        elevation.sin(),
        elevation.cos() * yaw.cos(),
    );
    let (sun, intensity) = (col(a.sun, b.sun), f(a.intensity, b.intensity));
    for (tr, light, _) in world
        .query::<(&mut Transform, &mut DirectionalLight, &Sun)>()
        .iter()
    {
        *tr = Transform::default().looking_to(-to_sun, Vec3::Y);
        light.color = sun;
        light.intensity = intensity;
    }
}

fn std_material(world: &mut World, m: StandardMaterial) -> Handle<StandardMaterial> {
    world
        .resource_or_default::<Assets<StandardMaterial>>()
        .add(m)
}

fn color_material(world: &mut World, color: Color, roughness: f32) -> Handle<StandardMaterial> {
    std_material(
        world,
        StandardMaterial::color(color).with_roughness(roughness),
    )
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

/// Cheap integer hash to 0..1.
fn hash(a: u32, b: u32) -> f32 {
    let mut h = a.wrapping_mul(0x9e37_79b9) ^ b.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    (h & 0xffff) as f32 / 65535.0
}

fn sun_and_camera(world: &mut World, orbit: OrbitCamera, shadow_distance: f32) {
    spawn_sun_and_camera(world, orbit);
    for l in world.query::<&mut DirectionalLight>().iter() {
        l.shadow_distance = shadow_distance;
    }
}

/// A quad of `w` x `h` facing +Z, its bottom edge at y = 0 (V runs down).
fn standing_quad(w: f32, h: f32) -> MeshData {
    let mut m = MeshData::plane(w, h);
    let r = Quat::from_rotation_x(FRAC_PI_2);
    for p in &mut m.positions {
        *p = r * *p + Vec3::new(0.0, h * 0.5, 0.0);
    }
    for n in &mut m.normals {
        *n = r * *n;
    }
    m
}

/// Two crossed standing quads (a foliage card cross), bottom at y = 0.
fn card_cross(w: f32, h: f32) -> MeshData {
    let a = standing_quad(w, h);
    let mut b = a.clone();
    let r = Quat::from_rotation_y(FRAC_PI_2);
    for p in &mut b.positions {
        *p = r * *p;
    }
    for n in &mut b.normals {
        *n = r * *n;
    }
    let mut m = a;
    let base = m.positions.len() as u32;
    m.positions.extend(b.positions);
    m.normals.extend(b.normals);
    for set in 0..m.uvs.len().min(b.uvs.len()) {
        let more = b.uvs[set].clone();
        m.uvs[set].extend(more);
    }
    m.colors.extend(b.colors);
    m.indices.extend(b.indices.iter().map(|i| i + base));
    m
}

/// A foliage atlas cell: leaf discs inside a rounded crown, alpha 0
/// between them (cut out by the alpha mask), lighter toward the top.
fn leaves_image(seed: u32) -> ImageData {
    let size = 128u32;
    let discs: Vec<(f32, f32, f32)> = (0..70)
        .map(|i| {
            let (a, r) = (hash(seed, i * 3) * TAU, hash(seed, i * 3 + 1).sqrt());
            let (x, y) = (0.5 + a.cos() * r * 0.4, 0.42 + a.sin() * r * 0.36);
            (x, y, 0.035 + 0.045 * hash(seed, i * 3 + 2))
        })
        .collect();
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for py in 0..size {
        for px in 0..size {
            let (u, v) = (
                (px as f32 + 0.5) / size as f32,
                (py as f32 + 0.5) / size as f32,
            );
            // A trunk below the crown.
            let trunk = (u - 0.5).abs() < 0.035 && v > 0.6;
            let leaf = discs
                .iter()
                .position(|&(x, y, r)| (u - x).powi(2) + (v - y).powi(2) < r * r);
            let px_rgba = match (leaf, trunk) {
                (Some(i), _) => {
                    let k = 0.75 + 0.25 * hash(seed + 7, i as u32);
                    let lift = 1.15 - v * 0.5;
                    [
                        (70.0 * k * lift) as u8,
                        (118.0 * k * lift) as u8,
                        (52.0 * k * lift) as u8,
                        255,
                    ]
                }
                (None, true) => [92, 70, 52, 255],
                _ => [60, 90, 45, 0],
            };
            rgba.extend(px_rgba);
        }
    }
    ImageData::new(size, size, ColorSpace::Srgb, rgba).expect("valid image")
}

/// A faceted boulder of unit size: a low-poly sphere with its vertices
/// pushed in and out, flat shaded.
fn boulder(seed: u32) -> MeshData {
    let sphere = MeshData::uv_sphere(1.0, 7, 5);
    // Jitter by quantised position, so seam and pole duplicates stay joined.
    let jitter = |p: Vec3| {
        let q = (p * 64.0).round();
        let k = (q.x as i32 as u32).wrapping_mul(73_856_093)
            ^ (q.y as i32 as u32).wrapping_mul(19_349_663)
            ^ (q.z as i32 as u32).wrapping_mul(83_492_791);
        p * (0.78 + 0.32 * hash(seed, k))
    };
    let mut m = MeshData::default();
    for tri in sphere.indices.as_chunks::<3>().0 {
        let [a, b, c] = tri.map(|i| jitter(sphere.positions[i as usize]));
        let n = (b - a).cross(c - a).normalize_or_zero();
        let base = m.positions.len() as u32;
        m.positions.extend([a, b, c]);
        m.normals.extend([n, n, n]);
        m.indices.extend([base, base + 1, base + 2]);
    }
    m
}

/// A rock pillar of stacked, offset faceted boulders, its top at `top`,
/// reaching down to `bottom`.
fn pillar(
    world: &mut World,
    rock: Handle<StandardMaterial>,
    at: Vec3,
    radius: f32,
    top: f32,
    bottom: f32,
    seed: u32,
) {
    let mut y = top;
    let mut i = 0;
    while y > bottom {
        let h = (radius * (1.0 + hash(seed, i) * 0.9))
            .min(y - bottom)
            .max(3.0);
        let r = radius * (0.78 + 0.4 * hash(seed, i + 50));
        let off = Vec3::new(
            (hash(seed, i + 100) - 0.5) * radius * 0.5,
            0.0,
            (hash(seed, i + 200) - 0.5) * radius * 0.5,
        );
        let mesh = add_mesh(world, boulder(seed * 97 + i));
        spawn(
            world,
            Transform::from_translation(at + off + Vec3::new(0.0, y - h * 0.5, 0.0))
                .with_rotation(Quat::from_rotation_y(hash(seed, i + 300) * TAU))
                .with_scale(Vec3::new(
                    r,
                    h * 0.75,
                    r * (0.8 + 0.3 * hash(seed, i + 400)),
                )),
            mesh,
            rock,
        );
        y -= h * 0.6;
        i += 1;
    }
}

/// Soft white cloud puffs: blended spheres that fade where they meet
/// geometry (depth fade), around `at`.
fn cloud_puffs(
    world: &mut World,
    puff: MaterialHandle,
    sphere: Handle<MeshData>,
    at: Vec3,
    spread: f32,
    count: u32,
    seed: u32,
) {
    for i in 0..count {
        let a = hash(seed, i) * TAU;
        let r = spread * hash(seed, i + 1000).sqrt();
        let s = spread * (0.18 + 0.22 * hash(seed, i + 2000));
        let p = at
            + Vec3::new(
                a.cos() * r,
                (hash(seed, i + 3000) - 0.3) * s * 0.6,
                a.sin() * r,
            );
        spawn(
            world,
            Transform::from_translation(p).with_scale(Vec3::new(s * 1.4, s * 0.75, s * 1.4)),
            sphere,
            puff,
        );
    }
}

/// A sea of broad, flat cloud puffs at height `y` within `radius` of the
/// origin (denser near the middle).
fn cloud_deck(
    world: &mut World,
    puff: MaterialHandle,
    sphere: Handle<MeshData>,
    y: f32,
    radius: f32,
    count: u32,
    seed: u32,
) {
    for i in 0..count {
        let a = hash(seed, i) * TAU;
        let r = radius * hash(seed, i + 1000).powf(0.8);
        let w = 18.0 + 30.0 * hash(seed, i + 2000) + r * 0.04;
        let p = Vec3::new(
            a.cos() * r,
            y + (hash(seed, i + 3000) - 0.5) * 6.0,
            a.sin() * r - r * 0.3,
        );
        spawn(
            world,
            Transform::from_translation(p).with_scale(Vec3::new(w, w * 0.22, w * 0.8)),
            sphere,
            puff,
        );
    }
}

/// A cloud puff material shader: lit like a standard surface, its opacity
/// fading toward the silhouette so a puff has no hard rim.
pub const CLOUD_WGSL: &str = "\
@material struct Cloud {
    /// Cloud colour (linear).
    @color tint: vec3f = vec3f(0.95, 0.96, 1.0),
    @range(0.0, 1.0) opacity: f32 = 0.9,
    /// Higher values thin the rim sooner.
    @range(0.1, 4.0) softness: f32 = 1.2,
    /// Light that is not from the sun or sky (scattering inside the cloud).
    @range(0.0, 1.0) glow: f32 = 0.12,
}

fn surface(in: SurfaceInput, p: Cloud) -> Surface {
    var s = default_surface(in);
    s.base_color = p.tint;
    s.roughness = 1.0;
    let facing = saturate(dot(in.normal, in.view_dir));
    s.alpha = p.opacity * pow(facing, p.softness);
    s.emissive = p.tint * p.glow * (0.5 + 0.5 * in.normal.y) * globals.ambient.x;
    return s;
}
";

/// Soft cloud puffs (`CLOUD_WGSL`), fading `depth_fade` metres in front of
/// the geometry they meet.
fn cloud_material(world: &mut World, depth_fade: f32) -> Handle<ShaderMaterial> {
    let shader = add_shader(world, "shaders/cloud.wgsl", CLOUD_WGSL);
    world
        .resource_or_default::<Assets<ShaderMaterial>>()
        .add(ShaderMaterial {
            alpha_mode: AlphaMode::Blend,
            depth_fade,
            ..ShaderMaterial::new(shader)
        })
}

fn puff_material(world: &mut World, depth_fade: f32) -> Handle<StandardMaterial> {
    std_material(
        world,
        StandardMaterial {
            base_color: Color::linear_rgba(0.95, 0.96, 1.0, 0.72),
            roughness: 1.0,
            alpha_mode: AlphaMode::Blend,
            depth_fade,
            ..Default::default()
        },
    )
}

/// A small home on a plateau: walls, a tiled roof, stairs, cypresses and
/// shrubs (swaying in the wind).
fn home(world: &mut World, at: Vec3) {
    let plaster = color_material(world, Color::srgb(0.92, 0.89, 0.83), 0.9);
    let roof = color_material(world, Color::srgb(0.72, 0.33, 0.24), 0.75);
    let grass = color_material(world, Color::srgb(0.47, 0.55, 0.32), 1.0);
    let cypress = std_material(
        world,
        StandardMaterial {
            base_color: Color::srgb(0.2, 0.33, 0.2),
            roughness: 0.95,
            wind: WindSway {
                strength: 0.08,
                frequency: 0.9,
                weight: WindWeight::HeightRamp {
                    bottom: -2.0,
                    top: 3.0,
                },
            },
            ..Default::default()
        },
    );
    let rock = color_material(world, Color::srgb(0.66, 0.6, 0.53), 0.92);
    let slab = add_mesh(world, MeshData::cylinder(15.5, 7.0, 9));
    spawn(
        world,
        Transform::from_translation(at + Vec3::new(0.0, -3.9, 0.0)),
        slab,
        rock,
    );
    let disc = add_mesh(world, MeshData::cylinder(16.0, 0.6, 9));
    spawn(
        world,
        Transform::from_translation(at + Vec3::new(0.0, -0.3, 0.0)),
        disc,
        grass,
    );
    let body = add_mesh(world, MeshData::cuboid(Vec3::new(5.0, 6.0, 5.0)));
    spawn(
        world,
        Transform::from_translation(at + Vec3::new(1.0, 3.0, -2.0)),
        body,
        plaster,
    );
    let annex = add_mesh(world, MeshData::cuboid(Vec3::new(6.0, 3.5, 4.0)));
    spawn(
        world,
        Transform::from_translation(at + Vec3::new(5.5, 1.75, -1.5)),
        annex,
        plaster,
    );
    let cap = add_mesh(world, MeshData::cuboid(Vec3::new(6.0, 0.8, 6.0)));
    spawn(
        world,
        Transform::from_translation(at + Vec3::new(1.0, 6.4, -2.0)),
        cap,
        roof,
    );
    let tree = add_mesh(world, MeshData::capsule(0.9, 2.2, 8, 4));
    for (x, z) in [(-5.0, 1.0), (-6.5, -2.0), (9.5, 3.0)] {
        spawn(
            world,
            Transform::from_translation(at + Vec3::new(x, 3.0, z)),
            tree,
            cypress,
        );
    }
}

/// A wide afternoon view: an elevated camera over a home plateau on a rock
/// pillar, taller pillars around it, distant ones quieted by haze, all
/// standing in a cloud sea (the height mist band) with soft cloud puffs
/// around the pillars.
pub fn build_pillars(world: &mut World) {
    let rock = color_material(world, Color::srgb(0.66, 0.6, 0.53), 0.92);
    let puff = cloud_material(world, 8.0).into();
    let sphere = add_mesh(world, MeshData::uv_sphere(1.0, 24, 12));
    // The home pillar under its plateau.
    pillar(world, rock, Vec3::ZERO, 18.0, -15.0, -260.0, 1);
    home(world, Vec3::ZERO);
    // Taller pillars around it (the left ones end above the cloud deck),
    // then distant ones quieted by distance.
    let pillars = [
        (Vec3::new(-105.0, 0.0, -55.0), 17.0, 120.0, -45.0, 2),
        (Vec3::new(-62.0, 0.0, -165.0), 19.0, 150.0, -60.0, 3),
        (Vec3::new(62.0, 0.0, -95.0), 15.0, 135.0, -260.0, 4),
        (Vec3::new(-110.0, 0.0, 60.0), 10.0, -25.0, -260.0, 5),
        (Vec3::new(170.0, 0.0, -460.0), 24.0, 150.0, -260.0, 6),
        (Vec3::new(-300.0, 0.0, -720.0), 28.0, 190.0, -260.0, 7),
        (Vec3::new(440.0, 0.0, -1100.0), 32.0, 170.0, -260.0, 8),
        (Vec3::new(-650.0, 0.0, -1500.0), 36.0, 230.0, -260.0, 9),
    ];
    for (at, radius, top, bottom, seed) in pillars {
        pillar(world, rock, at, radius, top, bottom, seed);
    }
    // The cloud deck: broad flat puffs on top of the mist band, out to the
    // haze; then clusters around the pillars at different heights.
    cloud_deck(world, puff, sphere, -104.0, 900.0, 700, 10);
    cloud_puffs(
        world,
        puff,
        sphere,
        Vec3::new(-100.0, -48.0, -55.0),
        34.0,
        16,
        12,
    );
    cloud_puffs(
        world,
        puff,
        sphere,
        Vec3::new(-62.0, -62.0, -165.0),
        34.0,
        14,
        16,
    );
    cloud_puffs(
        world,
        puff,
        sphere,
        Vec3::new(62.0, -40.0, -80.0),
        30.0,
        12,
        13,
    );
    cloud_puffs(
        world,
        puff,
        sphere,
        Vec3::new(-20.0, 60.0, -260.0),
        50.0,
        14,
        14,
    );
    cloud_puffs(
        world,
        puff,
        sphere,
        Vec3::new(10.0, -60.0, 15.0),
        36.0,
        16,
        15,
    );
    sun_and_camera(
        world,
        OrbitCamera {
            target: Vec3::new(0.0, -20.0, -60.0),
            distance: 175.0,
            yaw: 0.0,
            pitch: 0.3,
            max_distance: 2000.0,
            ..Default::default()
        },
        140.0,
    );
    set_reference_time(world, 16.5);
}

/// Haze, mist and quieting on a line of banded columns receding from 60 m
/// to 1.6 km, standing in a mist band, under the layered sky.
pub fn build_atmosphere(world: &mut World) {
    let bands = [
        color_material(world, Color::srgb(0.75, 0.22, 0.16), 0.8),
        color_material(world, Color::srgb(0.25, 0.6, 0.25), 0.8),
        color_material(world, Color::srgb(0.2, 0.32, 0.75), 0.8),
        color_material(world, Color::srgb(0.92, 0.9, 0.86), 0.8),
    ];
    for (i, d) in [60.0f32, 120.0, 240.0, 450.0, 800.0, 1600.0]
        .into_iter()
        .enumerate()
    {
        let radius = 3.0 + d * 0.035;
        let x = if i % 2 == 0 { -1.0 } else { 1.0 } * (8.0 + d * 0.18);
        let band_h = 25.0;
        for (b, &m) in bands.iter().cycle().take(12).enumerate() {
            let mesh = add_mesh(world, MeshData::cylinder(radius, band_h, 12));
            let y = 40.0 - band_h * (b as f32 + 0.5);
            spawn(world, Transform::from_xyz(x, y, -d), mesh, m);
        }
    }
    sun_and_camera(
        world,
        OrbitCamera {
            target: Vec3::new(0.0, -15.0, -60.0),
            distance: 75.0,
            yaw: 0.0,
            pitch: 0.24,
            max_distance: 2000.0,
            ..Default::default()
        },
        140.0,
    );
    set_reference_time(world, 16.5);
    let env = world.resource_mut::<Environment>();
    env.height_mist.top = -20.0;
    env.height_mist.bottom = -140.0;
}

/// Alpha-tested foliage cards (crossed quads with a cut-out leaf texture)
/// casting cut-out shadows, many sharing one mesh and material (instanced).
pub fn build_foliage(world: &mut World) {
    let leaves = add_image(world, "textures/leaves.png", leaves_image(3));
    let foliage = std_material(
        world,
        StandardMaterial {
            base_color: Color::WHITE,
            base_color_texture: leaves,
            roughness: 0.9,
            alpha_mode: AlphaMode::Mask { cutoff: 0.5 },
            double_sided: true,
            ..Default::default()
        },
    );
    let ground = color_material(world, Color::srgb(0.6, 0.56, 0.48), 0.95);
    let plane = add_mesh(world, MeshData::plane(60.0, 60.0));
    spawn(world, Transform::default(), plane, ground);
    let card = add_mesh(world, card_cross(4.0, 4.0));
    for i in 0..24u32 {
        let p = Vec3::new((hash(21, i) - 0.5) * 22.0, 0.0, (hash(22, i) - 0.5) * 22.0);
        let s = 0.8 + 0.6 * hash(23, i);
        spawn(
            world,
            Transform::from_translation(p)
                .with_rotation(Quat::from_rotation_y(hash(24, i) * TAU))
                .with_scale(Vec3::splat(s)),
            card,
            foliage,
        );
    }
    sun_and_camera(
        world,
        OrbitCamera {
            target: Vec3::new(0.0, 1.5, 0.0),
            distance: 26.0,
            yaw: 0.6,
            pitch: 0.55,
            ..Default::default()
        },
        60.0,
    );
    super::set_time_of_day(world, super::TimeOfDay::Day);
    for (t, _) in world.query::<(&mut Transform, &Sun)>().iter() {
        *t = Transform::default().looking_to(Vec3::new(0.5, -0.55, -0.35), Vec3::Y);
    }
}

/// A material shader banner hanging from its top edge, swaying with
/// `wind_offset` in its `vertex` hook.
pub const BANNER_WGSL: &str = "\
@material struct Banner {
    /// Cloth colour (linear).
    @color tint: vec3f = vec3f(0.62, 0.1, 0.08),
    /// Sway in metres per unit of wind at the free (bottom) edge.
    @range(0.0, 2.0) sway: f32 = 0.5,
}

// Pinned along the top edge (v = 0), free at the bottom (v = 1).
fn vertex(position: vec3f, in: VertexInput, p: Banner) -> vec3f {
    return position + wind_offset(position, in.uv.y, p.sway, 1.6);
}

fn surface(in: SurfaceInput, p: Banner) -> Surface {
    var s = default_surface(in);
    s.base_color = p.tint;
    s.roughness = 0.85;
    return s;
}
";

/// Wind sway: grass cards (weight from the vertex alpha), trees (weight by
/// height), a material shader banner (`wind_offset`), all moving in every
/// pass: their shadows follow, and the selected tree's outline follows it.
pub fn build_wind(world: &mut World) {
    let ground = color_material(world, Color::srgb(0.45, 0.5, 0.3), 1.0);
    let plane = add_mesh(world, MeshData::plane(60.0, 60.0));
    spawn(world, Transform::default(), plane, ground);
    // Grass: blades whose vertex alpha is 0 at the root and 1 at the tip.
    let mut blade = card_cross(0.16, 1.2);
    blade.colors = blade
        .positions
        .iter()
        .map(|p| [0.16, 0.3, 0.07, (p.y / 1.2).clamp(0.0, 1.0)])
        .collect();
    let blade = add_mesh(world, blade);
    let grass = std_material(
        world,
        StandardMaterial {
            base_color: Color::WHITE,
            roughness: 0.9,
            double_sided: true,
            wind: WindSway {
                strength: 0.35,
                frequency: 1.8,
                weight: WindWeight::VertexAlpha,
            },
            ..Default::default()
        },
    );
    for i in 0..400u32 {
        let p = Vec3::new((hash(31, i) - 0.5) * 16.0, 0.0, (hash(32, i) - 0.5) * 12.0);
        spawn(
            world,
            Transform::from_translation(p).with_rotation(Quat::from_rotation_y(hash(33, i) * TAU)),
            blade,
            grass,
        );
    }
    // Trees: trunk and a crown that sways more toward the top.
    let crown_material = std_material(
        world,
        StandardMaterial {
            base_color: Color::srgb(0.25, 0.42, 0.2),
            roughness: 0.9,
            wind: WindSway {
                strength: 0.45,
                frequency: 0.8,
                weight: WindWeight::HeightRamp {
                    bottom: -1.5,
                    top: 1.5,
                },
            },
            ..Default::default()
        },
    );
    let trunk_material = color_material(world, Color::srgb(0.4, 0.3, 0.22), 0.9);
    let crown = add_mesh(world, MeshData::capsule(1.0, 1.4, 12, 6));
    let trunk = add_mesh(world, MeshData::cylinder(0.18, 3.0, 8));
    let mut selected = None;
    for (i, x) in [-6.0f32, -2.0, 3.0, 7.0].into_iter().enumerate() {
        let z = -5.0 - i as f32 * 0.7;
        spawn(world, Transform::from_xyz(x, 1.5, z), trunk, trunk_material);
        let e = spawn(world, Transform::from_xyz(x, 4.2, z), crown, crown_material);
        if i == 2 {
            selected = Some(e);
        }
    }
    // A banner on a pole (material shader with a vertex hook).
    let shader = add_shader(world, "shaders/banner.wgsl", BANNER_WGSL);
    let banner_material =
        world
            .resource_or_default::<Assets<ShaderMaterial>>()
            .add(ShaderMaterial {
                double_sided: true,
                ..ShaderMaterial::new(shader)
            });
    let pole = add_mesh(world, MeshData::cylinder(0.06, 5.0, 8));
    spawn(
        world,
        Transform::from_xyz(-9.5, 2.5, -1.0),
        pole,
        trunk_material,
    );
    let mut cloth = MeshData::grid(1.4, 2.4, 6, 12);
    let r = Quat::from_rotation_x(FRAC_PI_2);
    for p in &mut cloth.positions {
        *p = r * *p;
    }
    for n in &mut cloth.normals {
        *n = r * *n;
    }
    let cloth = add_mesh(world, cloth);
    spawn(
        world,
        Transform::from_xyz(-8.75, 3.7, -1.0),
        cloth,
        banner_material,
    );
    if let Some(e) = selected {
        world.insert_resource(SelectionOutline {
            primary: vec![e],
            ..Default::default()
        });
    }
    sun_and_camera(
        world,
        OrbitCamera {
            target: Vec3::new(-1.0, 2.0, -2.0),
            distance: 19.0,
            yaw: 0.25,
            pitch: 0.3,
            ..Default::default()
        },
        50.0,
    );
    super::set_time_of_day(world, super::TimeOfDay::Day);
    world.resource_mut::<Environment>().wind = Wind {
        direction: Vec2::new(1.0, 0.25),
        strength: 1.0,
        gust_length: 12.0,
        gust_speed: 4.0,
    };
}

/// Soft transparency: mist sheets and cloud puffs crossing pillars and the
/// ground; the left set has no depth fade (hard intersection lines), the
/// right set fades over 1.5 m.
pub fn build_soft_particles(world: &mut World) {
    let ground = color_material(world, Color::srgb(0.55, 0.5, 0.44), 0.95);
    let rock = color_material(world, Color::srgb(0.6, 0.55, 0.5), 0.9);
    let plane = add_mesh(world, MeshData::plane(60.0, 60.0));
    spawn(world, Transform::default(), plane, ground);
    let column = add_mesh(world, MeshData::cylinder(1.2, 8.0, 10));
    let sheet = add_mesh(world, MeshData::plane(9.0, 9.0));
    let sphere = add_mesh(world, MeshData::uv_sphere(1.0, 24, 12));
    for (side, fade) in [(-1.0f32, 0.0f32), (1.0, 1.5)] {
        let cx = side * 6.0;
        let mist = std_material(
            world,
            StandardMaterial {
                base_color: Color::linear_rgba(0.85, 0.88, 0.95, 0.65),
                roughness: 1.0,
                alpha_mode: AlphaMode::Blend,
                double_sided: true,
                depth_fade: fade,
                ..Default::default()
            },
        );
        let puff = puff_material(world, fade);
        for (dx, dz) in [(-2.0, -1.5), (2.0, 1.0)] {
            spawn(world, Transform::from_xyz(cx + dx, 4.0, dz), column, rock);
        }
        spawn(world, Transform::from_xyz(cx, 2.5, 0.0), sheet, mist);
        for (i, (dx, dz)) in [(-2.0, 1.5), (1.0, -2.0), (2.6, 2.5)]
            .into_iter()
            .enumerate()
        {
            spawn(
                world,
                Transform::from_xyz(cx + dx, 0.3 + i as f32 * 0.2, dz)
                    .with_scale(Vec3::new(1.6, 1.0, 1.6)),
                sphere,
                puff,
            );
        }
    }
    sun_and_camera(
        world,
        OrbitCamera {
            target: Vec3::new(0.0, 2.0, 0.0),
            distance: 21.0,
            yaw: 0.0,
            pitch: 0.38,
            ..Default::default()
        },
        50.0,
    );
    super::set_time_of_day(world, super::TimeOfDay::Day);
}

/// Night: stars, a dim blue moon, and houses with lit (emissive) windows
/// glowing through bloom.
pub fn build_night(world: &mut World) {
    let plaster = color_material(world, Color::srgb(0.85, 0.82, 0.76), 0.9);
    let roof = color_material(world, Color::srgb(0.6, 0.3, 0.22), 0.8);
    let ground = color_material(world, Color::srgb(0.4, 0.42, 0.36), 1.0);
    let window = std_material(
        world,
        StandardMaterial {
            base_color: Color::srgb(0.2, 0.15, 0.1),
            emissive: Color::linear(3.0, 1.5, 0.45),
            roughness: 0.4,
            ..Default::default()
        },
    );
    let plane = add_mesh(world, MeshData::plane(120.0, 120.0));
    spawn(world, Transform::default(), plane, ground);
    let pane = add_mesh(world, MeshData::cuboid(Vec3::new(0.7, 1.0, 0.08)));
    for (i, (x, z, w, h)) in [
        (-8.0f32, -4.0f32, 5.0f32, 5.0f32),
        (0.0, -7.0, 6.0, 7.5),
        (8.0, -3.0, 4.5, 4.0),
    ]
    .into_iter()
    .enumerate()
    {
        let body = add_mesh(world, MeshData::cuboid(Vec3::new(w, h, 4.0)));
        spawn(world, Transform::from_xyz(x, h * 0.5, z), body, plaster);
        let cap = add_mesh(world, MeshData::cuboid(Vec3::new(w + 0.6, 0.6, 4.6)));
        spawn(world, Transform::from_xyz(x, h + 0.3, z), cap, roof);
        let rows = (h / 2.4).floor() as i32;
        for row in 0..rows {
            for col in [-1.0f32, 1.0] {
                if hash(41 + i as u32, (row * 2) as u32 + (col > 0.0) as u32) < 0.25 {
                    continue;
                }
                spawn(
                    world,
                    Transform::from_xyz(x + col * w * 0.25, 1.4 + row as f32 * 2.4, z + 2.02),
                    pane,
                    window,
                );
            }
        }
    }
    sun_and_camera(
        world,
        OrbitCamera {
            target: Vec3::new(0.0, 4.5, -3.0),
            distance: 28.0,
            yaw: 0.0,
            pitch: 0.12,
            ..Default::default()
        },
        60.0,
    );
    set_reference_time(world, 21.5);
    let env = world.resource_mut::<Environment>();
    env.height_mist.enabled = false;
    env.bloom.intensity = 0.05;
}
