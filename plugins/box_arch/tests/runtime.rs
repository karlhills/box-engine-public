//! box_arch in a running game: `ArchPlugin` registers the generators
//! (Add ▸ Architecture), the engine's runtime regenerates them, and
//! anchored decorations follow their target when its parameters or
//! transform change.

use box_arch::{Anchor, AnchorSurface, ArchPlugin, Opening, Roof, Wall, WallSide, resolve_anchors};
use box_engine::box_proc::generator::GeneratorRegistry;
use box_engine::box_proc::{ProcState, settle};
use box_engine::prelude::*;

const DT: f32 = 1.0 / 60.0;

fn world_pos(app: &App, e: Entity) -> Vec3 {
    app.world.get::<GlobalTransform>(e).unwrap().translation()
}

#[test]
fn generators_are_listed_under_architecture() {
    let game = Game::new().with_plugin(ArchPlugin);
    assert!(
        game.registration_errors().is_empty(),
        "{:?}",
        game.registration_errors()
    );
    let registry = game.registry();
    let mut names: Vec<(&str, &str)> = registry
        .generators()
        .iter()
        .filter(|g| g.category() == "Architecture")
        .map(|g| (g.name(), g.category()))
        .collect();
    names.sort();
    assert_eq!(
        names.iter().map(|n| n.0).collect::<Vec<_>>(),
        ["Platform", "Roof", "Stair", "Wall"]
    );
    assert!(registry.contains("Anchor"));
    // Not part of the engine's default registry.
    assert!(box_engine::engine_registry().generator("Wall").is_none());
}

/// A roof, platform or wall given with only some fields of a variant gets
/// that variant's defaults (never zero sizes), and those are in range.
#[test]
fn partial_variants_take_variant_defaults() {
    let game = Game::new().with_plugin(ArchPlugin);
    let registry = game.registry();
    assert_eq!(registry.variant_default_problems(), Vec::<String>::new());
    let ty = registry.component("Roof").unwrap().type_id();
    let mut json = serde_json::json!({ "shape": { "Dome": { "radius": 4.0 } } });
    assert!(
        registry
            .complete_variants(ty, &mut json, "Roof")
            .unwrap()
            .is_empty()
    );
    assert_eq!(json["shape"]["Dome"]["segments"], 32);
    assert_eq!(json["shape"]["Dome"]["radius"], 4.0);
}

#[test]
fn walls_regenerate_through_the_runtime() {
    let game = Game::new().with_plugin(ArchPlugin);
    let mut app = game.new_app();
    let e = app
        .world
        .spawn_saved(Uid::new(), "wall", (Transform::default(), Wall::default()))
        .unwrap();
    app.run_frames(2, DT);
    let gens = app.world.get::<ProcState>(e).unwrap().generations();
    assert!(gens >= 1);
    assert!(app.world.get::<ProcState>(e).unwrap().warnings().is_empty());
    app.world.component_mut::<Wall>(e).unwrap().height = 4.5;
    app.run_frames(1, DT);
    settle(&mut app.world);
    let s = app.world.get::<ProcState>(e).unwrap();
    assert!(s.generations() > gens, "the edit regenerated");
}

#[test]
fn anchors_follow_their_target() {
    let game = Game::new().with_plugin(ArchPlugin);
    let mut app = game.new_app();
    let wall_uid = Uid::new();
    // A wall along +x from (1, 0, 0), outer side +z, door at U = 2.
    let wall = app
        .world
        .spawn_saved(
            wall_uid,
            "wall",
            (
                Transform::from_xyz(1.0, 0.0, 0.0),
                Wall {
                    openings: vec![Opening::door(2.0)],
                    ..Wall::through(&[Vec2::new(0.0, 0.0), Vec2::new(5.0, 0.0)])
                },
            ),
        )
        .unwrap();
    let lamp = app.world.spawn((
        Name::new("lamp"),
        Transform::default().with_scale(Vec3::splat(2.0)),
        Anchor::new(
            wall_uid,
            AnchorSurface::WallOpening {
                index: 0,
                side: WallSide::Outer,
            },
            Vec3::new(0.0, 0.2, 0.1),
        ),
    ));
    app.run_frames(2, DT);
    let h = 0.15;
    let expect = Vec3::new(1.0 + 2.0, 2.15 + 0.2, h + 0.1);
    assert!(
        world_pos(&app, lamp).distance(expect) < 1e-4,
        "{:?}",
        world_pos(&app, lamp)
    );
    // +Z of the decoration points out of the wall; its scale is kept.
    let t = *app.world.get::<Transform>(lamp).unwrap();
    assert!((t.rotation * Vec3::Z).distance(Vec3::Z) < 1e-4);
    assert_eq!(t.scale, Vec3::splat(2.0));
    // Idle: nothing to resolve.
    assert_eq!(resolve_anchors(&mut app.world), 0);

    // The opening moves: the lamp follows.
    app.world.component_mut::<Wall>(wall).unwrap().openings[0].offset = 3.5;
    app.run_frames(1, DT);
    assert!((world_pos(&app, lamp).x - 4.5).abs() < 1e-4);
    // The wall gets thicker: the face moves out.
    app.world.component_mut::<Wall>(wall).unwrap().thickness = 0.5;
    app.run_frames(1, DT);
    assert!((world_pos(&app, lamp).z - 0.35).abs() < 1e-4);
    // The wall turns: the lamp turns with it.
    app.world.component_mut::<Transform>(wall).unwrap().rotation =
        Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
    app.run_frames(1, DT);
    let p = world_pos(&app, lamp);
    // Local (3.5, 2.35, 0.35) rotated a quarter turn about Y: (0.35, 2.35, -3.5) + (1, 0, 0).
    assert!(p.distance(Vec3::new(1.35, 2.35, -3.5)) < 1e-3, "{p:?}");

    // A decoration under a parent keeps its world pose.
    let parent = app.world.spawn((Transform::from_xyz(0.0, 5.0, 0.0),));
    let flag = app.world.spawn((
        Transform::default(),
        Anchor::new(wall_uid, AnchorSurface::WallTop, Vec3::new(1.0, 0.0, 0.0)),
    ));
    app.world.set_parent(flag, Some(parent)).unwrap();
    app.run_frames(1, DT);
    let p = world_pos(&app, flag);
    assert!(
        (p.y - 3.0).abs() < 1e-4,
        "on the top, not 5 m above it: {p:?}"
    );
    app.world.component_mut::<Wall>(wall).unwrap().height = 4.0;
    app.run_frames(1, DT);
    assert!((world_pos(&app, flag).y - 4.0).abs() < 1e-4);
}

#[test]
fn anchors_on_roofs_and_bad_targets() {
    let game = Game::new().with_plugin(ArchPlugin);
    let mut app = game.new_app();
    let roof_uid = Uid::new();
    let roof = app
        .world
        .spawn_saved(
            roof_uid,
            "roof",
            (Transform::from_xyz(0.0, 3.0, 0.0), Roof::default()),
        )
        .unwrap();
    let vane = app.world.spawn((
        Transform::default(),
        Anchor::new(roof_uid, AnchorSurface::RoofRidge, Vec3::ZERO),
    ));
    let lost = app.world.spawn((
        Transform::from_xyz(7.0, 7.0, 7.0),
        Anchor::new(Uid::new(), AnchorSurface::Local, Vec3::ZERO),
    ));
    app.run_frames(2, DT);
    let y0 = world_pos(&app, vane).y;
    assert!(y0 > 3.5, "{y0}");
    if let Some(r) = app.world.component_mut::<Roof>(roof) {
        r.pitch = 0.9;
    }
    app.run_frames(1, DT);
    assert!(
        world_pos(&app, vane).y > y0 + 0.3,
        "a steeper roof lifts the ridge"
    );
    // A missing target leaves the decoration where it was.
    assert_eq!(world_pos(&app, lost), Vec3::splat(7.0));
}

/// Every vertex of a wall generated through the runtime has a unit tangent
/// perpendicular to its normal (the generator itself emits none).
#[test]
fn generated_walls_get_tangents() {
    let game = Game::new().with_plugin(ArchPlugin);
    let mut app = game.new_app();
    let e = app
        .world
        .spawn_saved(
            Uid::new(),
            "wall",
            (
                Transform::default(),
                Wall {
                    openings: vec![Opening::door(2.0)],
                    ..Wall::through(&[Vec2::new(0.0, 0.0), Vec2::new(5.0, 0.0)])
                },
            ),
        )
        .unwrap();
    app.run_frames(2, DT);
    let handle = app.world.get::<ProcState>(e).unwrap().mesh().unwrap();
    let mesh = app
        .world
        .resource::<Assets<MeshData>>()
        .get(handle)
        .unwrap();
    assert!(mesh.vertex_count() > 0);
    assert_eq!(mesh.tangents.len(), mesh.vertex_count());
    for (t, n) in mesh.tangents.iter().zip(&mesh.normals) {
        let t3 = t.truncate();
        assert!((t3.length() - 1.0).abs() < 1e-3, "unit: {t}");
        assert!(t3.dot(*n).abs() < 1e-3, "perpendicular: {t} to {n}");
        assert!(t.w == 1.0 || t.w == -1.0);
    }
}

/// A normal map shows on a generated wall: the same wall under a grazing
/// sun, with and without a ribbed normal map. The capture goes to
/// `target/captures/arch/normal_map.png`.
#[test]
fn normal_maps_show_on_generated_walls() {
    use box_engine::box_render::Renderer;
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("skipping: BOX_SKIP_GPU_TESTS");
        return;
    }
    const W: u32 = 640;
    const H: u32 = 360;
    let Ok(mut renderer) = Renderer::new_headless(W, H) else {
        eprintln!("skipping: no GPU");
        return;
    };
    let render = |renderer: &mut Renderer, normal_map: bool| {
        let game = Game::new().with_plugin(ArchPlugin);
        let mut app = game.new_app();
        // Horizontal ribs, one per metre (the wall's UVs are in metres).
        let size = 64u32;
        let mut rgba = Vec::with_capacity((size * size * 4) as usize);
        for y in 0..size {
            let phase = (y as f32 + 0.5) / size as f32 * std::f32::consts::TAU;
            let n = Vec3::new(0.0, -box_engine::box_core::pmath::cos(phase) * 1.5, 1.0).normalize();
            let c = (n * 0.5 + Vec3::splat(0.5)) * 255.0;
            for _ in 0..size {
                rgba.extend([c.x as u8, c.y as u8, c.z as u8, 255]);
            }
        }
        let mut image = ImageData::new(size, size, ColorSpace::Linear, rgba).unwrap();
        image.generate_mips();
        let handle = app
            .world
            .resource_or_default::<Assets<ImageData>>()
            .add(image);
        app.world.resource_or_default::<AssetLookup>().insert(
            AssetId::new(),
            None,
            Some("textures/ribs.png"),
            handle,
        );
        {
            let env = app.world.resource_or_default::<Environment>();
            *env = Environment::day();
            env.ambient_occlusion.enabled = false;
            env.bloom.enabled = false;
        }
        app.world.spawn((
            Transform::default().looking_to(Vec3::new(-0.3, -0.8, -0.35), Vec3::Y),
            DirectionalLight {
                shadows: false,
                ..Default::default()
            },
        ));
        app.world.spawn((
            Transform::from_xyz(2.5, 1.6, 6.0).looking_at(Vec3::new(2.5, 1.4, 0.0), Vec3::Y),
            Camera::default(),
        ));
        app.world
            .spawn_saved(
                Uid::new(),
                "wall",
                (
                    Transform::default(),
                    Wall {
                        openings: vec![Opening::door(2.0)],
                        ..Wall::through(&[Vec2::new(0.0, 0.0), Vec2::new(5.0, 0.0)])
                    },
                    StandardMaterial {
                        base_color: Color::srgb(0.85, 0.8, 0.7),
                        roughness: 0.8,
                        normal_texture: if normal_map {
                            AssetRef::path("textures/ribs.png")
                        } else {
                            AssetRef::NONE
                        },
                        ..Default::default()
                    },
                ),
            )
            .unwrap();
        app.run_frames(3, DT);
        settle(&mut app.world);
        app.run_frames(2, DT);
        renderer.capture(&mut app.world).unwrap();
        renderer.capture(&mut app.world).unwrap()
    };
    let flat = render(&mut renderer, false);
    let ribbed = render(&mut renderer, true);
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/arch");
    std::fs::create_dir_all(&dir).unwrap();
    ribbed.save(dir.join("normal_map.png")).unwrap();
    flat.save(dir.join("normal_map_off.png")).unwrap();
    // Luminance along a column of the wall face, left of the door: flat
    // without the map, banded with it.
    let column = |rgba: &[u8]| -> Vec<f32> {
        (H / 2 - 40..H / 2 + 40)
            .map(|y| {
                let i = ((y * W + W / 4) * 4) as usize;
                (rgba[i] as f32 + rgba[i + 1] as f32 + rgba[i + 2] as f32) / 3.0
            })
            .collect()
    };
    let spread = |v: &[f32]| {
        v.iter().cloned().fold(f32::MIN, f32::max) - v.iter().cloned().fold(f32::MAX, f32::min)
    };
    let (a, b) = (
        spread(&column(flat.as_raw())),
        spread(&column(ribbed.as_raw())),
    );
    assert!(a < 12.0, "flat wall without the map: spread {a}");
    assert!(b > a + 25.0, "ribs with the map: spread {b} against {a}");
    assert_eq!(renderer.validation_errors(), 0);
}
