//! Tests of the RS3 on-demand features: atmosphere, layered sky and stars,
//! shadows/midtones/highlights and vignette, alpha-tested shadow casters,
//! wind sway in every pass, soft transparency, environment animation
//! without pipeline builds, and the PBR Neutral tone mapper.

use box_core::assets::Assets;
use box_core::color::Color;
use box_core::material::{AlphaMode, StandardMaterial, WindSway, WindWeight};
use box_core::mesh::MeshData;
use box_core::transform::{Transform, propagate_transforms};
use box_core::world::{Entity, World};
use glam::{Vec2, Vec3};
use image::RgbaImage;

use crate::components::{
    AerialHaze, Camera, DirectionalLight, DistanceQuieting, Environment, HeightMist, MeshRenderer,
    SelectionOutline, Sky, SkyGradient, Tonemapper, Vignette,
};
use crate::renderer::{RenderError, Renderer};
use crate::uniforms::GlobalsUniform;

fn renderer(w: u32, h: u32) -> Option<Renderer> {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        return None;
    }
    match Renderer::new_headless(w, h) {
        Ok(r) => Some(r),
        Err(RenderError::NoAdapter) => None,
        Err(e) => panic!("{e}"),
    }
}

/// An environment without fog, AO and bloom, so pixels show one effect.
fn plain_env() -> Environment {
    let mut env = Environment::day();
    env.fog_density = 0.0;
    env.ambient_occlusion.enabled = false;
    env.bloom.enabled = false;
    env
}

fn material(world: &mut World, m: StandardMaterial) -> box_core::assets::Handle<StandardMaterial> {
    world
        .resource_or_default::<Assets<StandardMaterial>>()
        .add(m)
}

fn mesh(world: &mut World, m: MeshData) -> box_core::assets::Handle<MeshData> {
    world.resource_or_default::<Assets<MeshData>>().add(m)
}

/// An unlit-looking surface: black base colour, emissive `c`.
fn glow(c: Color) -> StandardMaterial {
    StandardMaterial {
        base_color: Color::BLACK,
        emissive: c,
        roughness: 1.0,
        ..Default::default()
    }
}

fn rgb(img: &RgbaImage, x: u32, y: u32) -> [i32; 3] {
    let p = img.get_pixel(x, y).0;
    [p[0] as i32, p[1] as i32, p[2] as i32]
}

fn saturation(c: [i32; 3]) -> i32 {
    c.iter().max().unwrap() - c.iter().min().unwrap()
}

#[test]
fn look_settings_are_off_by_default_and_write_nothing() {
    let env = Environment::default();
    assert!(!env.aerial_haze.enabled && !env.height_mist.enabled);
    assert!(!env.distance_quieting.enabled);
    assert_eq!(env.vignette.intensity, 0.0);
    assert_eq!(env.sky, Sky::default());
    assert!(env.color_grading.is_identity());
    let mut g: GlobalsUniform = bytemuck::Zeroable::zeroed();
    crate::post::write_look(&env, 1.0, &mut g);
    crate::post::write_grading(&env.color_grading, &mut g);
    for (name, v) in [
        ("haze", g.haze),
        ("mist", g.mist),
        ("quiet_tone", g.quiet_tone),
        ("vignette", g.vignette),
        ("sky_upper", g.sky_upper),
    ] {
        assert_eq!(v, [0.0; 4], "{name}");
    }
    assert_eq!(
        g.highlights_tint[3], 0.0,
        "no shadows/midtones/highlights pass"
    );
    assert_eq!(g.grade[2], 0.0, "no scene grade");
    assert_eq!(g.sky_sun, [1.0, 1.0, 0.0, 0.0], "the RS1 sky");
}

/// A red wall far away (beyond the haze start) and a near one, seen from
/// the origin, and a red floor deep in the mist band.
fn atmosphere_world() -> World {
    let mut world = World::new();
    world.insert_resource(plain_env());
    let red = material(&mut world, glow(Color::linear(1.0, 0.05, 0.05)));
    let quad = mesh(&mut world, MeshData::cuboid(Vec3::new(1.0, 1.0, 0.1)));
    // Left: near (10 m). Right: far (1500 m), scaled to the same view size.
    world.spawn((
        Transform::from_xyz(-2.5, 0.0, -10.0).with_scale(Vec3::new(3.0, 3.0, 1.0)),
        MeshRenderer::new(quad, red),
    ));
    world.spawn((
        Transform::from_xyz(375.0, 0.0, -1500.0).with_scale(Vec3::new(450.0, 450.0, 1.0)),
        MeshRenderer::new(quad, red),
    ));
    // Bottom: a floor far below (in the mist band).
    let floor = mesh(&mut world, MeshData::plane(4000.0, 4000.0));
    world.spawn((
        Transform::from_xyz(0.0, -150.0, 0.0),
        MeshRenderer::new(floor, red),
    ));
    world.spawn((Transform::default(), Camera::default()));
    propagate_transforms(&mut world);
    world
}

#[test]
fn haze_mist_and_quieting_change_distant_surfaces_only() {
    let Some(mut r) = renderer(64, 64) else {
        eprintln!("skipping: no GPU");
        return;
    };
    let (near, far, low) = ((20, 32), (44, 32), (32, 62));
    let mut world = atmosphere_world();
    let base = r.capture(&mut world).unwrap();
    for p in [near, far, low] {
        let c = rgb(&base, p.0, p.1);
        assert!(c[0] > c[2] + 80, "red without atmosphere at {p:?}: {c:?}");
    }

    let haze_color = Color::linear(0.2, 0.4, 0.9);
    world.resource_mut::<Environment>().aerial_haze = AerialHaze {
        enabled: true,
        color: haze_color,
        density: 0.004,
        start_distance: 60.0,
        max_opacity: 0.9,
        sun_scatter: 0.0,
        sky_height: 0.0,
    };
    let hazed = r.capture(&mut world).unwrap();
    assert_eq!(
        rgb(&hazed, near.0, near.1),
        rgb(&base, near.0, near.1),
        "near: no haze"
    );
    let f = rgb(&hazed, far.0, far.1);
    assert!(f[2] > f[0], "far wall turns toward the blue haze: {f:?}");

    world.resource_mut::<Environment>().aerial_haze.enabled = false;
    world.resource_mut::<Environment>().height_mist = HeightMist {
        enabled: true,
        color: Color::linear(0.1, 0.9, 0.1),
        top: -20.0,
        bottom: -100.0,
        max_opacity: 1.0,
        near_fade: 10.0,
    };
    let misted = r.capture(&mut world).unwrap();
    let l = rgb(&misted, low.0, low.1);
    assert!(
        l[1] > l[0],
        "the floor below the band is in green mist: {l:?}"
    );
    assert_eq!(
        rgb(&misted, near.0, near.1),
        rgb(&base, near.0, near.1),
        "above the band"
    );

    world.resource_mut::<Environment>().height_mist.enabled = false;
    world.resource_mut::<Environment>().distance_quieting = DistanceQuieting {
        enabled: true,
        start: 100.0,
        end: 1000.0,
        desaturate: 1.0,
        contrast: 0.0,
        tone: Color::BLACK,
    };
    let quiet = r.capture(&mut world).unwrap();
    assert_eq!(rgb(&quiet, near.0, near.1), rgb(&base, near.0, near.1));
    let q = rgb(&quiet, far.0, far.1);
    assert!(
        saturation(q) * 4 < saturation(rgb(&base, far.0, far.1)),
        "far wall greyed: {q:?}"
    );
    assert_eq!(r.validation_errors(), 0);
}

#[test]
fn layered_sky_horizon_glow_stars_and_sky_haze() {
    let Some(mut r) = renderer(256, 256) else {
        eprintln!("skipping: no GPU");
        return;
    };
    let mut world = World::new();
    world.insert_resource(plain_env());
    // Looking up at 60 degrees: only sky.
    world.spawn((
        Transform::default().looking_to(Vec3::new(0.0, 1.7, -1.0), Vec3::Y),
        Camera::default(),
    ));
    propagate_transforms(&mut world);
    let simple = r.capture(&mut world).unwrap();
    let green = Color::linear(0.1, 0.8, 0.1);
    world.resource_mut::<Environment>().sky.gradient = SkyGradient::Layered {
        upper: green,
        below_horizon: Color::BLACK,
    };
    world.resource_mut::<Environment>().sky_zenith = green;
    let layered = r.capture(&mut world).unwrap();
    let (s, l) = (rgb(&simple, 128, 128), rgb(&layered, 128, 128));
    assert!(
        s[2] > s[1] && l[1] > l[2],
        "simple blue {s:?}, layered green {l:?}"
    );

    let bright = |img: &RgbaImage| {
        let bg = rgb(img, 128, 128).iter().sum::<i32>();
        img.pixels()
            .filter(|p| p.0[..3].iter().map(|&c| c as i32).sum::<i32>() > bg + 90)
            .count()
    };
    assert_eq!(bright(&layered), 0, "no stars by default");
    world.resource_mut::<Environment>().sky.stars = 4.0;
    let stars = r.capture(&mut world).unwrap();
    let n = bright(&stars);
    assert!(
        (1..2000).contains(&n),
        "a sparse star field: {n} bright pixels"
    );

    // The haze covers the sky up to its sky height, the zenith stays.
    let mut world = World::new();
    world.insert_resource(plain_env());
    world.spawn((Transform::default(), Camera::default()));
    propagate_transforms(&mut world);
    let level = r.capture(&mut world).unwrap();
    world.resource_mut::<Environment>().aerial_haze = AerialHaze {
        enabled: true,
        color: Color::linear(0.9, 0.1, 0.1),
        sky_height: 0.3,
        ..Default::default()
    };
    let hazed = r.capture(&mut world).unwrap();
    let (a, b) = (rgb(&level, 128, 130), rgb(&hazed, 128, 130));
    assert!(
        a[2] > a[0] && b[0] > b[2] + 60,
        "horizon toward the red haze: {a:?} -> {b:?}"
    );
    assert_eq!(
        rgb(&hazed, 128, 0),
        rgb(&level, 128, 0),
        "above the sky height"
    );
    assert_eq!(r.validation_errors(), 0);
}

#[test]
fn shadows_midtones_highlights_and_vignette() {
    let Some(mut r) = renderer(64, 64) else {
        eprintln!("skipping: no GPU");
        return;
    };
    // Left half dark, right half bright, filling the view.
    let mut world = World::new();
    let mut env = plain_env();
    env.tonemapper = Tonemapper::None;
    env.exposure = 1.0;
    world.insert_resource(env);
    let dark = material(&mut world, glow(Color::linear(0.05, 0.05, 0.05)));
    let light = material(&mut world, glow(Color::linear(0.8, 0.8, 0.8)));
    let quad = mesh(&mut world, MeshData::cuboid(Vec3::new(1.0, 1.0, 0.1)));
    for (x, m) in [(-1.0, dark), (1.0, light)] {
        world.spawn((
            Transform::from_xyz(x, 0.0, -2.0).with_scale(Vec3::new(2.0, 4.0, 1.0)),
            MeshRenderer::new(quad, m),
        ));
    }
    world.spawn((Transform::default(), Camera::default()));
    propagate_transforms(&mut world);
    let base = r.capture(&mut world).unwrap();
    let (d0, l0) = (rgb(&base, 16, 32), rgb(&base, 48, 32));
    // Blue shadows, red highlights.
    let g = &mut world.resource_mut::<Environment>().color_grading;
    g.shadows = Vec3::new(1.0, 1.0, 1.6);
    g.highlights = Vec3::new(1.15, 1.0, 1.0);
    let graded = r.capture(&mut world).unwrap();
    let (d, l) = (rgb(&graded, 16, 32), rgb(&graded, 48, 32));
    let near = |a: i32, b: i32| (a - b).abs() <= 1;
    assert!(
        d[2] - d[0] > d0[2] - d0[0] + 8 && near(d[0], d0[0]),
        "shadows bluer: {d0:?} -> {d:?}"
    );
    assert!(
        l[0] - l[2] > l0[0] - l0[2] + 8 && near(l[2], l0[2]),
        "highlights redder: {l0:?} -> {l:?}"
    );

    world.resource_mut::<Environment>().color_grading = Default::default();
    world.resource_mut::<Environment>().vignette = Vignette {
        intensity: 0.5,
        smoothness: 0.5,
        rounded: false,
    };
    let v = r.capture(&mut world).unwrap();
    let (corner, centre) = (rgb(&v, 60, 2), rgb(&v, 36, 32));
    assert!(
        corner[0] + 20 < rgb(&base, 60, 2)[0],
        "corner darkened: {corner:?}"
    );
    let before = rgb(&base, 36, 32);
    assert!(
        (centre[0] - before[0]).abs() <= 4,
        "centre almost unchanged: {before:?} -> {centre:?}"
    );
    assert_eq!(r.validation_errors(), 0);
}

/// Ground, a horizontal card at y = 3 with `card` and the sun at 45 degrees
/// (shadows land 3 m toward +x), seen from straight above.
fn card_world(card: StandardMaterial, cards: &[f32]) -> (World, Vec<Entity>) {
    let mut world = World::new();
    world.insert_resource(plain_env());
    let ground = material(
        &mut world,
        StandardMaterial::color(Color::linear(0.7, 0.7, 0.7)),
    );
    let card = material(&mut world, card);
    let plane = mesh(&mut world, MeshData::plane(40.0, 40.0));
    world.spawn((Transform::default(), MeshRenderer::new(plane, ground)));
    // A 2 x 2 card whose vertex alpha runs 0 (x = -1) to 1 (x = +1).
    let mut m = MeshData::grid(2.0, 2.0, 1, 1);
    m.colors = m
        .positions
        .iter()
        .map(|p| [1.0, 1.0, 1.0, (p.x + 1.0) * 0.5])
        .collect();
    let card_mesh = mesh(&mut world, m);
    let entities = cards
        .iter()
        .map(|&z| {
            world.spawn((
                Transform::from_xyz(0.0, 3.0, z),
                MeshRenderer::new(card_mesh, card),
            ))
        })
        .collect();
    world.spawn((
        Transform::default().looking_to(Vec3::new(1.0, -1.0, 0.0), Vec3::Y),
        DirectionalLight {
            shadow_distance: 40.0,
            ..Default::default()
        },
    ));
    world.spawn((
        Transform::from_xyz(1.5, 12.0, 0.0).looking_at(Vec3::new(1.5, 0.0, 0.0), Vec3::NEG_Z),
        Camera::default(),
    ));
    propagate_transforms(&mut world);
    (world, entities)
}

fn luma(img: &RgbaImage, x: u32, y: u32) -> i32 {
    rgb(img, x, y).iter().sum()
}

/// The x pixel columns where row `y` is darker than `threshold`.
fn dark_columns(img: &RgbaImage, y: u32, threshold: i32) -> Vec<u32> {
    (0..img.width())
        .filter(|&x| luma(img, x, y) < threshold)
        .collect()
}

#[test]
fn alpha_masked_casters_cast_their_cut_out_and_batch() {
    let Some(mut r) = renderer(128, 128) else {
        eprintln!("skipping: no GPU");
        return;
    };
    let card = |alpha_mode| StandardMaterial {
        base_color: Color::linear(0.7, 0.7, 0.7),
        alpha_mode,
        double_sided: true,
        ..Default::default()
    };
    let (mut opaque, _) = card_world(card(AlphaMode::Opaque), &[0.0]);
    let (mut masked, _) = card_world(card(AlphaMode::Mask { cutoff: 0.5 }), &[0.0]);
    let a = r.capture(&mut opaque).unwrap();
    let b = r.capture(&mut masked).unwrap();
    let lit = luma(&a, 120, 64);
    let shadow_a = dark_columns(&a, 64, lit * 3 / 4);
    let shadow_b = dark_columns(&b, 64, lit * 3 / 4);
    // The opaque card's shadow is twice as wide as the cut-out's: the
    // card's half with alpha below the cutoff casts nothing.
    assert!(!shadow_b.is_empty(), "the masked card casts a shadow");
    let width = |v: &[u32]| v.len() as f32;
    assert!(
        width(&shadow_b) < width(&shadow_a) * 0.7,
        "cut-out shadow {} px vs opaque {} px",
        shadow_b.len(),
        shadow_a.len()
    );
    // Many masked cards share one instanced shadow draw per cascade.
    let one_shadow_draws = r.stats().shadow_draws;
    let (mut many, _) = card_world(card(AlphaMode::Mask { cutoff: 0.5 }), &[-6.0, 0.0, 6.0]);
    r.capture(&mut many).unwrap();
    assert_eq!(r.stats().shadow_draws, one_shadow_draws, "instanced");
    assert_eq!(r.validation_errors(), 0);
}

#[test]
fn wind_moves_the_surface_its_shadow_and_its_outline_alike() {
    let Some(mut r) = renderer(128, 128) else {
        eprintln!("skipping: no GPU");
        return;
    };
    let magenta = [255u8, 0, 255, 255];
    let card = StandardMaterial {
        base_color: Color::linear(0.05, 0.05, 0.05),
        double_sided: true,
        wind: WindSway {
            strength: 1.0,
            frequency: 0.0,
            // Weight 1 everywhere: the card moves (nearly) rigidly.
            weight: WindWeight::HeightRamp {
                bottom: -10.0,
                top: -10.0,
            },
        },
        ..Default::default()
    };
    let (mut world, cards) = card_world(card, &[0.0]);
    world.insert_resource(SelectionOutline {
        primary: cards.clone(),
        color: Color::srgb(1.0, 0.0, 1.0),
        ..Default::default()
    });
    let env = world.resource_mut::<Environment>();
    env.wind.direction = Vec2::new(1.0, 0.0);
    env.wind.gust_length = 1.0e6;
    let mut extents = |strength: f32| {
        world.resource_mut::<Environment>().wind.strength = strength;
        let img = r.capture(&mut world).unwrap();
        let lit = luma(&img, 120, 100);
        // Row 64 crosses the grey card and its shadow, which the sky light
        // tints blue.
        let dark = dark_columns(&img, 64, lit * 4 / 5);
        let blue = |x: u32| {
            let c = rgb(&img, x, 64);
            c[2] - c[0] > 15
        };
        let card: Vec<u32> = dark
            .iter()
            .copied()
            .filter(|&x| !blue(x) && img.get_pixel(x, 64).0 != magenta)
            .collect();
        let shadow: Vec<u32> = dark.iter().copied().filter(|&x| blue(x)).collect();
        let outline: Vec<u32> = (0..128)
            .filter(|&x| img.get_pixel(x, 64).0 == magenta)
            .collect();
        let span = |v: &[u32]| {
            (
                *v.first().expect("found") as i32,
                *v.last().expect("found") as i32,
            )
        };
        [span(&card), span(&shadow), span(&outline)]
    };
    let calm = extents(0.0);
    let windy = extents(1.0);
    let shift = |i: usize| (windy[i].0 - calm[i].0, windy[i].1 - calm[i].1);
    let (card, shadow, outline) = (shift(0), shift(1), shift(2));
    assert!(
        card.0 >= 2 && card.1 >= 2,
        "the card moves downwind: {calm:?} -> {windy:?}"
    );
    assert!(
        shadow.0 >= 2 && shadow.1 >= 2,
        "its shadow moves: {calm:?} -> {windy:?}"
    );
    assert!(
        (outline.0 - card.0).abs() <= 1 && (outline.1 - card.1).abs() <= 1,
        "the outline moves with the card: {calm:?} -> {windy:?}"
    );
    assert_eq!(r.validation_errors(), 0);
}

#[test]
fn depth_fade_softens_intersections_and_runs_the_prepass_only_when_visible() {
    let Some(mut r) = renderer(64, 64) else {
        eprintln!("skipping: no GPU");
        return;
    };
    let scene = |fade: f32, look: Vec3| {
        let mut world = World::new();
        world.insert_resource(plain_env());
        let ground = material(&mut world, glow(Color::linear(0.3, 0.3, 0.3)));
        let sheet = material(
            &mut world,
            StandardMaterial {
                base_color: Color::linear_rgba(0.0, 0.0, 0.0, 0.9),
                emissive: Color::linear(0.0, 0.0, 2.0),
                alpha_mode: AlphaMode::Blend,
                double_sided: true,
                depth_fade: fade,
                ..Default::default()
            },
        );
        let plane = mesh(&mut world, MeshData::plane(40.0, 40.0));
        world.spawn((Transform::default(), MeshRenderer::new(plane, ground)));
        // A vertical blue sheet standing on the ground, facing the camera.
        let wall = mesh(&mut world, MeshData::cuboid(Vec3::new(6.0, 4.0, 0.01)));
        world.spawn((
            Transform::from_xyz(0.0, 2.0, 0.0),
            MeshRenderer::new(wall, sheet),
        ));
        world.spawn((
            Transform::from_xyz(0.0, 3.0, 6.0).looking_to(look, Vec3::Y),
            Camera::default(),
        ));
        propagate_transforms(&mut world);
        world
    };
    let toward = Vec3::new(0.0, -0.45, -1.0);
    let away = Vec3::new(0.0, 0.0, 1.0);
    let mut hard = scene(0.0, toward);
    let mut soft = scene(1.0, toward);
    let a = r.capture(&mut hard).unwrap();
    let hard_draws = r.stats().draws;
    let b = r.capture(&mut soft).unwrap();
    let soft_draws = r.stats().draws;
    assert!(
        soft_draws > hard_draws,
        "the prepass runs: {hard_draws} -> {soft_draws}"
    );
    // Just above where the sheet meets the ground: blue without the fade,
    // nearly the ground colour with it; high on the sheet, alike.
    let foot = (0..64u32)
        .rev()
        .find(|&y| rgb(&a, 32, y)[2] > rgb(&a, 32, y)[0] + 40)
        .expect("the sheet's lowest row");
    let (ha, hb) = (rgb(&a, 32, foot), rgb(&b, 32, foot));
    assert!(hb[2] + 30 < ha[2], "softened at the foot: {ha:?} vs {hb:?}");
    let top = (0..64u32)
        .find(|&y| rgb(&a, 32, y)[2] > rgb(&a, 32, y)[0] + 40)
        .unwrap();
    let (ta, tb) = (rgb(&a, 32, top + 2), rgb(&b, 32, top + 2));
    assert!(
        (ta[2] - tb[2]).abs() <= 6,
        "unchanged away from geometry: {ta:?} {tb:?}"
    );

    // The fading sheet out of view: no prepass.
    let mut hidden = scene(1.0, away);
    let mut hidden_hard = scene(0.0, away);
    r.capture(&mut hidden_hard).unwrap();
    let base = r.stats().draws;
    r.capture(&mut hidden).unwrap();
    assert_eq!(r.stats().draws, base, "no prepass for an invisible fade");
    assert_eq!(r.validation_errors(), 0);
}

#[test]
fn animating_the_environment_builds_no_pipelines_and_no_buffers() {
    let Some(mut r) = renderer(64, 64) else {
        eprintln!("skipping: no GPU");
        return;
    };
    let swaying = StandardMaterial {
        alpha_mode: AlphaMode::Mask { cutoff: 0.5 },
        wind: WindSway {
            strength: 0.3,
            ..Default::default()
        },
        ..Default::default()
    };
    let (mut world, _) = card_world(swaying, &[0.0, 4.0]);
    let puff = material(
        &mut world,
        StandardMaterial {
            alpha_mode: AlphaMode::Blend,
            depth_fade: 2.0,
            ..Default::default()
        },
    );
    let ball = mesh(&mut world, MeshData::uv_sphere(1.0, 12, 6));
    world.spawn((
        Transform::from_xyz(2.0, 0.5, 0.0),
        MeshRenderer::new(ball, puff),
    ));
    propagate_transforms(&mut world);
    {
        let env = world.resource_mut::<Environment>();
        env.ambient_occlusion.enabled = true;
        env.bloom.enabled = true;
        env.aerial_haze.enabled = true;
        env.height_mist.enabled = true;
        env.distance_quieting.enabled = true;
    }
    r.capture(&mut world).unwrap();
    let pipelines = r.stats().material_pipelines;
    for frame in 0..40 {
        let t = frame as f32 / 40.0;
        let env = world.resource_mut::<Environment>();
        env.sky_zenith = Color::linear(0.1 * t, 0.2, 0.5);
        env.sky.gradient = SkyGradient::Layered {
            upper: Color::linear(0.3, 0.4 + 0.2 * t, 0.7),
            below_horizon: Color::linear(0.5, 0.5, 0.6),
        };
        env.sky.stars = t;
        env.sky.horizon_glow = 1.0 - t;
        env.aerial_haze.density = 0.0005 + 0.001 * t;
        env.height_mist.top = -10.0 - 50.0 * t;
        env.distance_quieting.contrast = t * 0.5;
        env.color_grading.shadows = Vec3::new(1.0, 1.0, 1.0 + t * 0.2);
        env.vignette.intensity = t * 0.3;
        env.wind.strength = 2.0 * t;
        env.exposure = 0.5 + t;
        env.bloom.threshold = t;
        env.tonemapper = [Tonemapper::AgX, Tonemapper::Neutral][frame % 2];
        r.capture(&mut world).unwrap();
        let s = r.stats();
        assert_eq!(s.material_pipelines, pipelines, "frame {frame}");
        assert_eq!(s.buffer_reallocations, 0, "frame {frame}");
    }
    assert_eq!(r.validation_errors(), 0);
}

/// The Khronos PBR Neutral reference (KhronosGroup/ToneMapping, PBR_Neutral),
/// display-linear out.
fn pbr_neutral_reference(c: [f32; 3]) -> [f32; 3] {
    const START: f32 = 0.8 - 0.04;
    const DESATURATION: f32 = 0.15;
    let x = c[0].min(c[1]).min(c[2]);
    let offset = if x < 0.08 { x - 6.25 * x * x } else { 0.04 };
    let mut c = c.map(|v| v - offset);
    let peak = c[0].max(c[1]).max(c[2]);
    if peak < START {
        return c;
    }
    let d = 1.0 - START;
    let new_peak = 1.0 - d * d / (peak + d - START);
    c = c.map(|v| v * new_peak / peak);
    let g = 1.0 - 1.0 / (DESATURATION * (peak - new_peak) + 1.0);
    c.map(|v| v + (new_peak - v) * g)
}

fn srgb_byte(linear: f32) -> i32 {
    let v = linear.clamp(0.0, 1.0);
    let e = if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    (e * 255.0).round() as i32
}

#[test]
fn pbr_neutral_reference_values() {
    // Below the start of compression the curve is the identity after the
    // toe offset: 0.04 off every channel once the darkest is 0.08 or more.
    assert_eq!(pbr_neutral_reference([0.0; 3]), [0.0; 3]);
    let grey = pbr_neutral_reference([0.18; 3]);
    assert!((grey[0] - 0.14).abs() < 1e-6, "{grey:?}");
    let c = pbr_neutral_reference([0.5, 0.3, 0.2]);
    assert!((c[0] - 0.46).abs() < 1e-6 && (c[2] - 0.16).abs() < 1e-6);
    // Monotonic, below 1, and toward white for large values.
    let mut last = -1.0;
    for i in 0..2000 {
        let v = pbr_neutral_reference([i as f32 * 0.01; 3])[0];
        assert!(v >= last && v < 1.0, "{i}: {v}");
        last = v;
    }
    assert!(last > 0.99);
}

/// The shader's Neutral tone mapper matches the reference, and keeps a bright
/// saturated colour more saturated than AgX.
#[test]
fn neutral_tonemapper_matches_the_khronos_reference() {
    let Some(mut r) = renderer(16, 16) else {
        eprintln!("skipping: no GPU");
        return;
    };
    let mut world = World::new();
    let mut env = plain_env();
    env.tonemapper = Tonemapper::Neutral;
    env.exposure = 1.0;
    env.sky_zenith = Color::BLACK;
    env.sky_horizon = Color::BLACK;
    env.ground = Color::BLACK;
    world.insert_resource(env);
    let m = material(&mut world, glow(Color::BLACK));
    let quad = mesh(&mut world, MeshData::cuboid(Vec3::new(1.0, 1.0, 0.1)));
    world.spawn((
        Transform::from_xyz(0.0, 0.0, -1.0).with_scale(Vec3::new(8.0, 8.0, 1.0)),
        MeshRenderer::new(quad, m),
    ));
    world.spawn((Transform::default(), Camera::default()));
    propagate_transforms(&mut world);
    let mut shade = |world: &mut World, c: [f32; 3], t: Tonemapper| {
        world.resource_mut::<Environment>().tonemapper = t;
        world
            .resource_mut::<Assets<StandardMaterial>>()
            .get_mut(m)
            .unwrap()
            .emissive = Color::linear(c[0], c[1], c[2]);
        rgb(&r.capture(world).unwrap(), 8, 8)
    };
    for c in [
        [0.02, 0.02, 0.02],
        [0.18, 0.18, 0.18],
        [0.5, 0.3, 0.2],
        [0.7, 0.7, 0.7],
        [1.5, 1.5, 1.5],
        [4.0, 1.0, 0.5],
        [20.0, 20.0, 20.0],
    ] {
        let got = shade(&mut world, c, Tonemapper::Neutral);
        let want = pbr_neutral_reference(c).map(srgb_byte);
        for i in 0..3 {
            assert!(
                (got[i] - want[i]).abs() <= 2,
                "{c:?}: shader {got:?}, reference {want:?}"
            );
        }
    }
    let hot = [2.0, 0.5, 0.25];
    let (neutral, agx) = (
        shade(&mut world, hot, Tonemapper::Neutral),
        shade(&mut world, hot, Tonemapper::AgX),
    );
    assert!(
        saturation(neutral) > saturation(agx) + 20,
        "Neutral {neutral:?} more saturated than AgX {agx:?}"
    );
    assert_eq!(r.validation_errors(), 0);
}
