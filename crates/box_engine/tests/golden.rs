//! Golden-image tests for the M1 renderer.
//!
//! Skipped when `BOX_SKIP_GPU_TESTS` is set or no GPU adapter exists.
//! Regenerate references with `BOX_UPDATE_GOLDEN=1 cargo test -p box_engine --test golden`.
//! References are recorded on the macOS development machine; other platforms
//! may differ until per-platform references exist (M9).

#[path = "../examples/common/mod.rs"]
mod common;

use std::path::PathBuf;

use box_engine::prelude::*;
use box_render::{RenderError, golden};
use common::*;

type SceneBuilder = Box<dyn FnOnce(&mut App)>;

const W: u32 = 960;
const H: u32 = 540;

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn renderer() -> Option<Renderer> {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping golden tests");
        return None;
    }
    match Renderer::new_headless(W, H) {
        Ok(r) => Some(r),
        Err(RenderError::NoAdapter) => {
            eprintln!("no GPU adapter; skipping golden tests");
            None
        }
        Err(e) => panic!("renderer creation failed: {e}"),
    }
}

fn render_scene(renderer: &mut Renderer, build: impl FnOnce(&mut App)) -> image::RgbaImage {
    let mut app = box_engine::default_app();
    app.world.insert_resource(WindowSize {
        width: W,
        height: H,
        scale_factor_milli: 1000,
    });
    build(&mut app);
    for _ in 0..3 {
        app.tick(1.0 / 60.0);
    }
    renderer.capture(&mut app.world).expect("capture")
}

#[test]
fn golden_scenes() {
    let Some(mut r) = renderer() else { return };
    let scenes: Vec<(&str, SceneBuilder)> = vec![
        (
            "showcase_day",
            Box::new(|app: &mut App| {
                build_showcase(&mut app.world);
                spawn_sun_and_camera(&mut app.world, showcase_camera());
                set_time_of_day(&mut app.world, TimeOfDay::Day);
            }),
        ),
        (
            "showcase_evening",
            Box::new(|app: &mut App| {
                build_showcase(&mut app.world);
                spawn_sun_and_camera(&mut app.world, showcase_camera());
                set_time_of_day(&mut app.world, TimeOfDay::Evening);
            }),
        ),
        (
            "material_grid",
            Box::new(|app: &mut App| {
                build_material_grid(&mut app.world);
                spawn_sun_and_camera(&mut app.world, material_grid_camera());
                set_time_of_day(&mut app.world, TimeOfDay::Day);
            }),
        ),
        (
            "debug_draw",
            Box::new(|app: &mut App| {
                build_showcase(&mut app.world);
                spawn_sun_and_camera(&mut app.world, showcase_camera());
                set_time_of_day(&mut app.world, TimeOfDay::Day);
                app.add_system(Stage::Update, |w: &mut World| {
                    let dd = w.resource_mut::<DebugDraw>();
                    dd.grid(Vec3::ZERO, 20.0, 1.0, Color::linear(0.1, 0.1, 0.1));
                    dd.aabb(
                        &Aabb::new(Vec3::new(-2.4, 0.0, -2.4), Vec3::new(2.4, 14.6, 2.4)),
                        Color::linear(1.0, 0.8, 0.1),
                    );
                    dd.axes(Vec3::new(0.0, 15.0, 0.0), 3.0);
                });
            }),
        ),
    ];
    let mut failures = Vec::new();
    for (name, build) in scenes {
        let img = render_scene(&mut r, build);
        let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures");
        std::fs::create_dir_all(&out).unwrap();
        img.save(out.join(format!("{name}.png"))).unwrap();
        if let Err(e) = golden::check(&golden_dir(), name, &img) {
            failures.push(format!("{name}: {e}"));
        }
    }
    assert_eq!(r.stats().validation_errors, 0, "GPU validation errors");
    assert!(
        failures.is_empty(),
        "golden mismatches:\n{}",
        failures.join("\n")
    );
}

/// RS2 material scenes (see `examples/common/materials.rs`), at 640x360:
/// textures, alpha modes, double-sided faces, point lights with and without
/// the sun, two materials on one mesh, a custom shader material (plus a
/// broken one drawing the error material), and per-material pipeline state
/// with a shader reading the scene depth.
#[test]
fn golden_material_scenes() {
    let Some(mut r) = renderer() else { return };
    r.resize(640, 360);
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures");
    std::fs::create_dir_all(&out).unwrap();
    let mut failures = Vec::new();
    for &name in common::materials::MATERIAL_SCENES {
        let img = render_scene(&mut r, |app: &mut App| {
            assert!(common::materials::build_material_scene(
                &mut app.world,
                name
            ));
        });
        img.save(out.join(format!("{name}.png"))).unwrap();
        if let Err(e) = golden::check(&golden_dir(), name, &img) {
            failures.push(format!("{name}: {e}"));
        }
        assert_eq!(
            r.stats().validation_errors,
            0,
            "{name}: GPU validation errors"
        );
    }
    assert!(
        failures.is_empty(),
        "golden mismatches:\n{}",
        failures.join("\n")
    );
}

/// M5: a game-defined generator (`Steps`, registered by its plugin) drawn
/// from parameters: two flights, one with a seeded wobble. The generator's
/// output hashes are pinned separately (`tests/procedural.rs`).
#[test]
fn golden_procedural_steps() {
    let Some(mut r) = renderer() else { return };
    let mut app = Game::new()
        .with_plugin(common::steps::StepsPlugin)
        .new_app();
    app.world.insert_resource(WindowSize {
        width: W,
        height: H,
        scale_factor_milli: 1000,
    });
    common::steps::build_steps_scene(&mut app.world);
    spawn_sun_and_camera(&mut app.world, common::steps::steps_camera());
    set_time_of_day(&mut app.world, TimeOfDay::Day);
    for _ in 0..3 {
        app.tick(1.0 / 60.0);
    }
    let img = r.capture(&mut app.world).expect("capture");
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures");
    std::fs::create_dir_all(&out).unwrap();
    img.save(out.join("procedural_steps.png")).unwrap();
    assert_eq!(r.stats().validation_errors, 0, "GPU validation errors");
    golden::check(&golden_dir(), "procedural_steps", &img).unwrap();
}

/// The built-in generators (primitives, lathes, extrusions) through the
/// generator runtime.
#[test]
fn golden_builtin_shapes() {
    let Some(mut r) = renderer() else { return };
    let img = render_scene(&mut r, |app: &mut App| {
        common::shapes::build_shapes_scene(&mut app.world);
        spawn_sun_and_camera(&mut app.world, common::shapes::shapes_camera());
        set_time_of_day(&mut app.world, TimeOfDay::Day);
    });
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures");
    std::fs::create_dir_all(&out).unwrap();
    img.save(out.join("builtin_shapes.png")).unwrap();
    assert_eq!(r.stats().validation_errors, 0, "GPU validation errors");
    golden::check(&golden_dir(), "builtin_shapes", &img).unwrap();
}

/// The tolerance must catch small missing objects: removing any one shrub from
/// the day showcase fails the comparison, while the full scene still passes.
#[test]
fn golden_detects_a_missing_shrub() {
    let Some(mut r) = renderer() else { return };
    let reference = image::open(golden_dir().join("showcase_day.png"))
        .expect("showcase_day golden")
        .to_rgba8();
    let full = render_scene(&mut r, |app: &mut App| {
        build_showcase(&mut app.world);
        spawn_sun_and_camera(&mut app.world, showcase_camera());
        set_time_of_day(&mut app.world, TimeOfDay::Day);
    });
    let stats = golden::compare(&full, &reference, &golden::DEFAULT_TOLERANCE)
        .expect("unchanged scene matches");
    eprintln!("unchanged scene: {stats:?}");
    // The shrubs are the uv spheres at these positions in `build_showcase`.
    for shrub in [
        Vec3::new(9.5, 0.6, -2.0),
        Vec3::new(10.5, 0.42, -0.8),
        Vec3::new(-3.5, 0.72, 8.0),
        Vec3::new(2.5, 0.48, 9.0),
    ] {
        let img = render_scene(&mut r, move |app: &mut App| {
            build_showcase(&mut app.world);
            spawn_sun_and_camera(&mut app.world, showcase_camera());
            set_time_of_day(&mut app.world, TimeOfDay::Day);
            let found: Vec<Entity> = app
                .world
                .query::<(Entity, &Transform, &MeshRenderer)>()
                .iter()
                .filter(|(_, t, _)| (t.translation - shrub).length() < 1e-3)
                .map(|(e, _, _)| e)
                .collect();
            assert_eq!(found.len(), 1, "shrub at {shrub}");
            app.world.despawn(found[0]);
        });
        let stats = golden::diff_stats(
            &img,
            &reference,
            golden::DEFAULT_TOLERANCE.channel_threshold,
        )
        .unwrap();
        eprintln!("without shrub at {shrub}: {stats:?}");
        assert!(
            golden::compare(&img, &reference, &golden::DEFAULT_TOLERANCE).is_err(),
            "removing the shrub at {shrub} must fail the golden ({stats:?})"
        );
    }
    assert_eq!(r.stats().validation_errors, 0);
}

/// RS4 scenes (see `examples/common/interior.rs`), at 640x360: the interior
/// with its shadowed spot light and ambient volume, the same room with the
/// point light shadowed (a six-view cube), and the particles scene after
/// 110 fixed frames (seeded emitters: the same particles every run).
#[test]
fn golden_rs4_scenes() {
    use common::interior::*;
    let Some(mut r) = renderer() else { return };
    r.resize(640, 360);
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures");
    std::fs::create_dir_all(&out).unwrap();
    let scenes: Vec<(&str, usize, SceneBuilder)> = vec![
        (
            "interior",
            3,
            Box::new(|app: &mut App| build_interior(&mut app.world, InteriorOptions::default())),
        ),
        (
            "interior_point_shadow",
            3,
            Box::new(|app: &mut App| {
                build_interior(
                    &mut app.world,
                    InteriorOptions {
                        shadowed: Shadowed::Point,
                        ..Default::default()
                    },
                )
            }),
        ),
        (
            "particles",
            110,
            Box::new(|app: &mut App| {
                build_particles(&mut app.world);
                app.add_system(Stage::Update, projectile_system);
            }),
        ),
    ];
    let mut failures = Vec::new();
    for (name, frames, build) in scenes {
        let mut app = box_engine::default_app();
        build(&mut app);
        for _ in 0..frames {
            app.tick(1.0 / 60.0);
        }
        let img = r.capture(&mut app.world).expect("capture");
        img.save(out.join(format!("{name}.png"))).unwrap();
        if let Err(e) = golden::check(&golden_dir(), name, &img) {
            failures.push(format!("{name}: {e}"));
        }
        assert_eq!(
            r.stats().validation_errors,
            0,
            "{name}: GPU validation errors"
        );
    }
    assert!(
        failures.is_empty(),
        "golden mismatches:\n{}",
        failures.join("\n")
    );
}

/// A plastered wall behind clustered tree crowns, close up: the case where
/// 12 AO samples left visible low-frequency blotches in the wall's
/// occluded area (docs/WORLD_BUILDING.md G10). Renders with AO on and off
/// and measures the band-pass energy of the darkening AO adds there.
#[test]
fn golden_ao_closeup_has_no_blotches() {
    let Some(mut r) = renderer() else { return };
    let build = |ao: bool| {
        move |app: &mut App| {
            let w = &mut app.world;
            let plaster = StandardMaterial::color(Color::hex(0xe8dcc6));
            let leaves = StandardMaterial::color(Color::hex(0xd98a3a));
            let grass = StandardMaterial::color(Color::hex(0x7f9a5a));
            w.spawn((
                Transform::from_xyz(0.0, -0.05, 0.0),
                Primitive::cuboid(Vec3::new(60.0, 0.1, 60.0)),
                grass,
            ));
            w.spawn((
                Transform::from_xyz(0.0, 2.5, -1.0),
                Primitive::cuboid(Vec3::new(12.0, 5.0, 0.4)),
                plaster,
            ));
            let crowns = [
                (-3.0, 1.6, 0.3, 1.0),
                (-2.2, 2.1, 0.0, 0.9),
                (-3.4, 2.4, -0.2, 0.8),
                (2.5, 1.2, 1.0, 1.1),
                (3.2, 1.9, 0.6, 0.9),
            ];
            for (x, y, z, radius) in crowns {
                w.spawn((
                    Transform::from_xyz(x, y, z),
                    Primitive::sphere(radius),
                    leaves.clone(),
                ));
            }
            spawn_sun_and_camera(
                w,
                OrbitCamera {
                    target: Vec3::new(0.5, 1.7, 0.0),
                    distance: 5.0,
                    yaw: 0.0,
                    pitch: 0.0,
                    ..Default::default()
                },
            );
            set_time_of_day(w, TimeOfDay::Day);
            w.resource_mut::<Environment>().ambient_occlusion.enabled = ao;
        }
    };
    let on = render_scene(&mut r, build(true));
    let off = render_scene(&mut r, build(false));
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures");
    std::fs::create_dir_all(&out).unwrap();
    on.save(out.join("ao_closeup.png")).unwrap();
    off.save(out.join("ao_closeup_off.png")).unwrap();
    assert_eq!(r.stats().validation_errors, 0, "GPU validation errors");

    let blotches = ao_band_energy(&on, &off, (282, 216, 336, 300));
    eprintln!("AO band-pass energy beside the crown: {blotches:.3}");
    assert!(blotches < AO_BLOTCH_LIMIT, "AO blotches: {blotches:.3}");
    golden::check(&golden_dir(), "ao_closeup", &on).unwrap();
}

/// Measured on the close-up above (Apple M3 Ultra, Metal): 12 samples
/// 2.85, 24 samples 1.84.
const AO_BLOTCH_LIMIT: f32 = 2.35;

/// Mean band-pass (blur 1 minus blur 6) magnitude of the darkening `on`
/// has over `off` (scaled by 8) inside `region` (x0, y0, x1, y1).
fn ao_band_energy(
    on: &image::RgbaImage,
    off: &image::RgbaImage,
    region: (u32, u32, u32, u32),
) -> f32 {
    let (x0, y0, x1, y1) = region;
    let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
    let luma =
        |p: &image::Rgba<u8>| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32;
    let mut d = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let (px, py) = (x0 + x as u32, y0 + y as u32);
            let v = (luma(off.get_pixel(px, py)) - luma(on.get_pixel(px, py))) * 8.0;
            d[y * w + x] = v.clamp(0.0, 255.0);
        }
    }
    let fine = box_blur(&d, w, h, 1, 2);
    let coarse = box_blur(&d, w, h, 6, 3);
    let margin = 8;
    let mut sum = 0.0;
    let mut n = 0;
    for y in margin..h - margin {
        for x in margin..w - margin {
            sum += (fine[y * w + x] - coarse[y * w + x]).abs();
            n += 1;
        }
    }
    sum / n as f32
}

/// `passes` separable box blurs of radius `r` (edges clamped).
fn box_blur(src: &[f32], w: usize, h: usize, r: usize, passes: usize) -> Vec<f32> {
    let mut a = src.to_vec();
    let mut b = vec![0.0; a.len()];
    let r = r as isize;
    for _ in 0..passes {
        for (dx, dy) in [(1isize, 0isize), (0, 1)] {
            for y in 0..h as isize {
                for x in 0..w as isize {
                    let mut s = 0.0;
                    for k in -r..=r {
                        let qx = (x + k * dx).clamp(0, w as isize - 1) as usize;
                        let qy = (y + k * dy).clamp(0, h as isize - 1) as usize;
                        s += a[qy * w + qx];
                    }
                    b[y as usize * w + x as usize] = s / (2 * r + 1) as f32;
                }
            }
            std::mem::swap(&mut a, &mut b);
        }
    }
    a
}

/// W2: a terrain of every stamp kind, with its layer colours (grass, rock
/// on the steep flanks, a dirt path) and the edge skirt.
#[test]
fn golden_terrain() {
    let Some(mut r) = renderer() else { return };
    let mut app = box_engine::default_app();
    app.world.insert_resource(WindowSize {
        width: W,
        height: H,
        scale_factor_milli: 1000,
    });
    app.world.spawn((
        Uid::new(),
        Transform::default(),
        Terrain {
            size: Vec2::splat(96.0),
            stamps: vec![
                TerrainStamp::Noise {
                    seed: 7,
                    scale: 18.0,
                    amplitude: 0.6,
                    octaves: 3,
                },
                TerrainStamp::Hill {
                    center: Vec2::new(-30.0, -28.0),
                    radius: 22.0,
                    height: 11.0,
                },
                TerrainStamp::Ridge {
                    points: vec![
                        Vec2::new(-46.0, 20.0),
                        Vec2::new(-20.0, 40.0),
                        Vec2::new(46.0, 30.0),
                    ],
                    width: 26.0,
                    height: 6.0,
                },
                TerrainStamp::Basin {
                    center: Vec2::new(18.0, 6.0),
                    radius: 9.0,
                    depth: 1.6,
                },
                TerrainStamp::Flatten {
                    center: Vec2::new(0.0, -20.0),
                    radius: 6.0,
                    height: 1.4,
                    bank: 5.0,
                },
                TerrainStamp::FlattenPath {
                    points: vec![
                        Vec3::new(0.0, 0.1, 30.0),
                        Vec3::new(-4.0, 0.3, 14.0),
                        Vec3::new(-2.0, 0.67, 2.0),
                        Vec3::new(0.0, 1.4, -16.0),
                    ],
                    width: 3.0,
                    bank: 2.0,
                },
            ],
            ..Terrain::default()
        },
    ));
    spawn_sun_and_camera(
        &mut app.world,
        OrbitCamera {
            target: Vec3::new(0.0, 0.0, 0.0),
            distance: 110.0,
            yaw: 0.6,
            pitch: 0.55,
            ..Default::default()
        },
    );
    set_time_of_day(&mut app.world, TimeOfDay::Day);
    app.tick(1.0 / 60.0);
    box_engine::box_proc::settle(&mut app.world);
    for _ in 0..3 {
        app.tick(1.0 / 60.0);
    }
    let img = r.capture(&mut app.world).expect("capture");
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures");
    std::fs::create_dir_all(&out).unwrap();
    img.save(out.join("terrain.png")).unwrap();
    assert_eq!(r.stats().validation_errors, 0, "GPU validation errors");
    golden::check(&golden_dir(), "terrain", &img).unwrap();
}
