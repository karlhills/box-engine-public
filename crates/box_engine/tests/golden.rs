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
