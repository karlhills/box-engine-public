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
