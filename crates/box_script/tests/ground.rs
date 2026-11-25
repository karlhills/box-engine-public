//! Ground-aware placement (docs/WORLD_BUILDING.md W3): raising the terrain
//! under a lane (`Sweep::follow_ground`), a lamp (`FollowGround`) and a
//! planting intent (a script rule sampling `ground`) moves all three with
//! it, with no gameplay script involved; a character still walks up.

mod common;

use std::time::Duration;

use box_engine::box_core::derived::PartKey;
use box_engine::box_proc::rule::RuleRegistry;
use box_engine::box_script::ScriptIntent;
use box_engine::prelude::*;
use box_engine::{Game, app_with_world};

use common::Project;

const DT: f32 = 1.0 / 60.0;

/// Plants one marker per intent point, standing on the ground.
const PLANTING: &str = r#"
local Planting = { scope = "intent" }

function Planting.rule(intents: { RuleIntent }, out: RuleOut)
    for _, it in intents do
        for i, p in it.points do
            local w = it:to_world(p)
            local y = ground.height(w.x, w.z) or w.y
            out:part(it, `plant{i}`, {
                Primitive = { shape = { Sphere = { radius = 0.3 } } },
            }, { position = it:to_local(vector.create(w.x, y, w.z)) })
        end
    end
end

return Planting
"#;
const PLANTING_PATH: &str = "rules/planting.luau";

/// The ridge stamp: across the lane at z = 0, `height` high.
fn ridge(height: f32) -> TerrainStamp {
    TerrainStamp::Ridge {
        points: vec![Vec2::new(-20.0, 0.0), Vec2::new(20.0, 0.0)],
        width: 16.0,
        height,
    }
}

struct Scene {
    terrain: Entity,
    lamp: Entity,
    far_lamp: Entity,
    lane: Entity,
    planting: Entity,
    far_planting: Entity,
}

fn build(w: &mut World) -> Scene {
    let terrain = w.spawn((
        Uid::new(),
        Name::new("Ground"),
        Transform::IDENTITY,
        Terrain {
            size: Vec2::splat(64.0),
            cell_size: 0.5,
            stamps: vec![
                ridge(2.0),
                TerrainStamp::Hill {
                    center: Vec2::new(24.0, 24.0),
                    radius: 4.0,
                    height: 0.5,
                },
            ],
            ..Terrain::default()
        },
    ));
    let lamp = w.spawn((
        Uid::new(),
        Name::new("Lamp"),
        Transform::from_xyz(3.0, 0.0, 0.0),
        FollowGround { offset: 0.1 },
    ));
    let far_lamp = w.spawn((
        Uid::new(),
        Name::new("Far lamp"),
        Transform::from_xyz(-25.0, 7.0, -25.0),
        FollowGround::default(),
    ));
    let lane = w.spawn((
        Uid::new(),
        Name::new("Lane"),
        Transform::from_xyz(0.0, 0.0, 0.0),
        Spline::from_positions(
            &[Vec3::new(0.0, 0.0, -20.0), Vec3::new(0.0, 0.0, 20.0)],
            SplineMode::Polyline,
            false,
        ),
        Sweep {
            profile: SweepProfile::Rect {
                width: 2.0,
                height: 0.05,
            },
            orientation: SweepOrientation::Up,
            follow_ground: true,
            ground_offset: -0.02,
            ..Sweep::default()
        },
        Procedural::default(),
    ));
    let planting = w.spawn((
        Uid::new(),
        Name::new("Planting"),
        Transform::IDENTITY,
        ScriptIntent::new(PLANTING_PATH)
            .with_points([Vec3::new(-4.0, 0.0, 1.0), Vec3::new(5.0, 0.0, -2.0)]),
    ));
    let far_planting = w.spawn((
        Uid::new(),
        Name::new("Far planting"),
        Transform::IDENTITY,
        ScriptIntent::new(PLANTING_PATH).with_points([Vec3::new(-25.0, 0.0, 25.0)]),
    ));
    Scene {
        terrain,
        lamp,
        far_lamp,
        lane,
        planting,
        far_planting,
    }
}

fn part(w: &World, owner: Entity, role: &str) -> Option<Entity> {
    w.entity_by_uid(PartKey::new(w.uid(owner)?, role).uid())
}

fn world_y(w: &World, e: Entity) -> f32 {
    box_engine::box_core::transform::compute_global(w, e)
        .translation
        .y
}

fn ground_y(w: &World, x: f32, z: f32) -> f32 {
    w.resource::<Ground>().height(x, z).unwrap()
}

/// The lane mesh's highest point near the ridge (world y).
fn lane_top(w: &World, lane: Entity) -> f32 {
    let mesh = w
        .get::<box_engine::box_core::mesh_asset::GeneratedMesh>(lane)
        .and_then(|g| g.mesh)
        .and_then(|h| {
            w.resource::<box_engine::box_core::assets::Assets<MeshData>>()
                .get(h)
                .cloned()
        })
        .expect("the lane generated");
    mesh.bounds().max.y
}

fn raise_ridge(w: &mut World, s: &Scene, height: f32) {
    w.component_mut::<Terrain>(s.terrain).unwrap().stamps[0] = ridge(height);
}

fn rule_runs(w: &World) -> u64 {
    w.resource::<RuleRegistry>()
        .get(PLANTING_PATH)
        .map_or(0, |r| r.runs)
}

/// Until the rule has made both plantings' parts and the lane generated.
fn ready(w: &World, s: &Scene) -> bool {
    part(w, s.planting, "plant2").is_some()
        && part(w, s.far_planting, "plant1").is_some()
        && w.get::<box_engine::box_core::mesh_asset::GeneratedMesh>(s.lane)
            .is_some_and(|g| g.mesh.is_some())
}

/// A frame (the ground updates and stamps what reads it), then every
/// stamped generator flushed.
fn settle(app: &mut App) {
    app.run_frames(1, DT);
    box_engine::box_proc::settle(&mut app.world);
    app.run_frames(2, DT);
}

#[test]
fn raising_the_ground_moves_lane_lamp_and_planting_in_play() {
    let p = Project::new("ground_play", &[(PLANTING_PATH, PLANTING)]);
    let mut app = p.app();
    let s = build(&mut app.world);
    assert!(common::run_until(&mut app, 5.0, |app| ready(
        &app.world, &s
    )));
    settle(&mut app);

    let w = &app.world;
    let lamp0 = world_y(w, s.lamp);
    assert!(
        (lamp0 - (ground_y(w, 3.0, 0.0) + 0.1)).abs() < 1e-4,
        "{lamp0}"
    );
    assert!(lamp0 > 1.9, "on the ridge: {lamp0}");
    let plant = part(w, s.planting, "plant1").unwrap();
    let plant0 = world_y(w, plant);
    assert!((plant0 - ground_y(w, -4.0, 1.0)).abs() < 1e-4, "{plant0}");
    let far_plant = part(w, s.far_planting, "plant1").unwrap();
    let far_plant0 = world_y(w, far_plant);
    let far_lamp0 = world_y(w, s.far_lamp);
    assert!(
        far_lamp0.abs() < 1e-4,
        "dropped onto flat ground: {far_lamp0}"
    );
    let lane0 = lane_top(w, s.lane);
    assert!(
        (lane0 - (2.0 - 0.02 + 0.05)).abs() < 0.02,
        "the lane crosses the ridge top: {lane0}"
    );

    // A change away from every intent does not re-run the rule.
    let runs = rule_runs(&app.world);
    if let TerrainStamp::Hill { height, .. } = &mut app
        .world
        .component_mut::<Terrain>(s.terrain)
        .unwrap()
        .stamps[1]
    {
        *height = 0.8;
    }
    settle(&mut app);
    assert_eq!(rule_runs(&app.world), runs, "nothing of it under the hill");

    // Raise the ridge half a metre.
    raise_ridge(&mut app.world, &s, 2.5);
    settle(&mut app);
    let w = &app.world;
    let lamp1 = world_y(w, s.lamp);
    assert!((lamp1 - (ground_y(w, 3.0, 0.0) + 0.1)).abs() < 1e-4);
    assert!(lamp1 - lamp0 > 0.4, "{lamp0} -> {lamp1}");
    let plant1 = world_y(w, part(w, s.planting, "plant1").unwrap());
    assert!((plant1 - ground_y(w, -4.0, 1.0)).abs() < 1e-4);
    assert!(plant1 - plant0 > 0.3, "{plant0} -> {plant1}");
    assert_eq!(rule_runs(w), runs + 1);
    assert_eq!(
        world_y(w, part(w, s.far_planting, "plant1").unwrap()),
        far_plant0
    );
    assert_eq!(world_y(w, s.far_lamp), far_lamp0);
    let lane1 = lane_top(w, s.lane);
    assert!((lane1 - lane0 - 0.5).abs() < 0.02, "{lane0} -> {lane1}");

    // A character walks up the lane over the raised ridge without jumping.
    let c = app.world.spawn((
        Transform::from_xyz(0.0, 1.5, -18.0),
        KinematicCharacter::default(),
    ));
    app.run_frames(10, DT);
    let mut top = f32::MIN;
    for _ in 0..600 {
        if let Some(k) = app.world.component_mut::<KinematicCharacter>(c) {
            k.velocity.z = 5.0;
        }
        app.run_frames(1, DT);
        top = top.max(app.world.get::<Transform>(c).unwrap().translation.y);
    }
    let end = app.world.get::<Transform>(c).unwrap().translation;
    assert!(end.z > 15.0, "crossed the ridge: {end}");
    assert!(top > 2.5, "went over the top: {top}");
}

#[test]
fn the_edit_world_follows_the_ground_too() {
    let p = Project::new("ground_edit", &[(PLANTING_PATH, PLANTING)]);
    let game = Game::new();
    let mut app = app_with_world(game.new_world());
    app.world
        .insert_resource(box_engine::asset_server(p.assets()).unwrap());
    app.world.insert_resource(game.rules());
    let s = build(&mut app.world);
    let frame = |w: &mut World| {
        if let Some(server) = w
            .get_resource::<box_engine::box_assets::AssetServer>()
            .cloned()
        {
            server.update_until_idle(w, Duration::from_millis(50));
        }
        box_engine::box_core::transform::propagate_transforms(w);
        box_engine::sync_scene_meshes(w);
    };
    let start = std::time::Instant::now();
    while !ready(&app.world, &s) && start.elapsed() < Duration::from_secs(5) {
        frame(&mut app.world);
        box_engine::box_proc::settle(&mut app.world);
    }
    assert!(ready(&app.world, &s));
    let lamp0 = world_y(&app.world, s.lamp);
    raise_ridge(&mut app.world, &s, 2.5);
    for _ in 0..3 {
        frame(&mut app.world);
    }
    box_engine::box_proc::settle(&mut app.world);
    let w = &app.world;
    assert!(world_y(w, s.lamp) - lamp0 > 0.4);
    let plant = part(w, s.planting, "plant1").unwrap();
    assert!((world_y(w, plant) - ground_y(w, -4.0, 1.0)).abs() < 1e-4);
}
