//! Navigation in apps (docs/NAVIGATION.md): the navmesh bakes itself from a
//! scene's static colliders, follows changes tile by tile, leaves out
//! moving things, and agrees with the character controller.
//!
//! - `bakes_from_colliders_and_paths_around_walls`: background bake, path
//!   through a wall's gap; hiding the gap's door opens a shorter path within
//!   0.3 s; moving one box rebuilds only its tiles; movers and triggers are
//!   not baked.
//! - `a_character_can_walk_every_path`: a probe character walks the path's
//!   corners with `walk_route` and reaches each.
//! - `settings_change_and_switch_off`: `NavSettings` changes rebake;
//!   `enabled: false` empties the mesh.

use box_engine::box_core::edit::{Command, EditStack, apply_commands};
use box_engine::nav_glue::{Navigation, nav_path, nav_reachable, settle_navigation};
use box_engine::prelude::*;
use box_engine::{app_with_world, shared_engine_registry};
use box_physics::Collidable;

const DT: f32 = 1.0 / 60.0;

fn app(extra: serde_json::Value) -> App {
    let mut world = World::with_registry(shared_engine_registry());
    let mut cmds = vec![
        serde_json::json!({ "op": "spawn", "name": "Ground", "components": {
            "Transform": { "translation": [0.0, -0.5, 0.0] },
            "Primitive": { "shape": { "Cuboid": { "size": [60.0, 1.0, 30.0] } } },
            "Collider": { "shape": "Primitive" } } }),
        // A wall across z = 0 with a 2 m gap at x = 0 closed by a door.
        serde_json::json!({ "op": "spawn", "name": "Wall West", "components": {
            "Transform": { "translation": [-15.5, 1.5, 0.0] },
            "Primitive": { "shape": { "Cuboid": { "size": [29.0, 3.0, 0.4] } } },
            "Collider": { "shape": "Primitive" } } }),
        serde_json::json!({ "op": "spawn", "name": "Wall East", "components": {
            "Transform": { "translation": [15.5, 1.5, 0.0] },
            "Primitive": { "shape": { "Cuboid": { "size": [29.0, 3.0, 0.4] } } },
            "Collider": { "shape": "Primitive" } } }),
        serde_json::json!({ "op": "spawn", "name": "Door", "components": {
            "Transform": { "translation": [0.0, 1.5, 0.0] },
            "Primitive": { "shape": { "Cuboid": { "size": [2.0, 3.0, 0.4] } } },
            "Collider": { "shape": "Primitive" } } }),
    ];
    cmds.extend(extra.as_array().cloned().unwrap_or_default());
    let cmds: Vec<Command> = serde_json::from_value(serde_json::Value::Array(cmds)).unwrap();
    apply_commands(&mut world, &mut EditStack::default(), &cmds, "level").unwrap();
    app_with_world(world)
}

fn step(app: &mut App, n: usize) {
    app.run_frames(n, DT);
}

/// One frame paced like a real 60 fps run, so worker bakes can finish.
fn frame_rt(app: &mut App) {
    std::thread::sleep(std::time::Duration::from_secs_f32(DT));
    app.run_frames(1, DT);
}

#[test]
fn bakes_from_colliders_and_paths_around_walls() {
    let mut app = app(serde_json::json!([
        // A moving platform and a trigger: neither is baked.
        { "op": "spawn", "name": "Lift", "components": {
            "Transform": { "translation": [-20.0, 0.5, -8.0] },
            "Primitive": { "shape": { "Cuboid": { "size": [3.0, 1.0, 3.0] } } },
            "Collider": { "shape": "Primitive" },
            "Mover": { "points": [[0.0, 0.0, 0.0], [0.0, 2.0, 0.0]] } } },
        { "op": "spawn", "name": "Zone", "components": {
            "Transform": { "translation": [-20.0, 1.0, 8.0] },
            "Primitive": { "shape": { "Cuboid": { "size": [3.0, 2.0, 3.0] } } },
            "Collider": { "shape": "Primitive", "is_trigger": true } } }
    ]));
    // Background bake: ready within a couple of flushes.
    let mut frames = 0;
    while frames < 120 {
        frame_rt(&mut app);
        frames += 1;
        let nav = app.world.resource::<Navigation>();
        if !nav.mesh().is_empty() && !nav.is_baking() {
            break;
        }
    }
    assert!(frames < 120, "navmesh not ready after {frames} frames");
    let (a, b) = (Vec3::new(-6.0, 0.0, -6.0), Vec3::new(-6.0, 0.0, 6.0));
    assert!(!nav_reachable(&app.world, a, b), "the door closes the gap");

    // Opening the door: the path appears within 0.3 s.
    let door = app.world.find_by_name("Door").unwrap();
    app.world.insert_one(door, Collidable(false)).unwrap();
    let mut opened = None;
    for f in 0..60 {
        frame_rt(&mut app);
        if nav_reachable(&app.world, a, b) {
            opened = Some(f);
            break;
        }
    }
    let f = opened.expect("door opened but no path");
    assert!(f <= 18, "path after {f} frames");
    let path = nav_path(&app.world, a, b).unwrap();
    assert!(
        path.points
            .iter()
            .any(|p| p.x.abs() < 1.0 && p.z.abs() < 1.0)
            || path.points.len() >= 3
    );

    // The lift and the trigger zone are not in the mesh: the floor under
    // the lift is walkable at ground height, and the zone is flat ground.
    let under_lift = app
        .world
        .resource::<Navigation>()
        .mesh()
        .closest_point(Vec3::new(-20.0, 0.0, -8.0))
        .unwrap();
    assert!(under_lift.y < 0.4, "{under_lift}");
    let zone = app
        .world
        .resource::<Navigation>()
        .mesh()
        .closest_point(Vec3::new(-20.0, 0.0, 8.0))
        .unwrap();
    assert!(
        (zone.x + 20.0).abs() < 0.2 && (zone.z - 8.0).abs() < 0.2,
        "{zone}"
    );

    // Moving a small box rebuilds only the tiles around it.
    settle_navigation(&mut app.world);
    let before = app.world.resource::<Navigation>().tiles_baked;
    let tiles = app.world.resource::<Navigation>().mesh().tiles().count() as u64;
    let crate_ = app.world.spawn((
        Name::new("Crate"),
        Transform::from_translation(Vec3::new(10.0, 0.5, 8.0)),
        Collider::cuboid(Vec3::ONE),
    ));
    step(&mut app, 20);
    settle_navigation(&mut app.world);
    let rebuilt = app.world.resource::<Navigation>().tiles_baked - before;
    assert!(
        rebuilt >= 1 && rebuilt < tiles,
        "rebuilt {rebuilt} of {tiles}"
    );
    let on_crate = app
        .world
        .resource::<Navigation>()
        .mesh()
        .closest_point(Vec3::new(10.0, 0.0, 8.0))
        .unwrap();
    assert!(
        Vec2::new(on_crate.x - 10.0, on_crate.z - 8.0).length() > 0.8,
        "the crate is an obstacle: {on_crate}"
    );
    let _ = crate_;
}

#[test]
fn a_character_can_walk_every_path() {
    let mut app = app(serde_json::json!([
        { "op": "spawn", "name": "Block", "components": {
            "Transform": { "translation": [-6.0, 1.0, -8.0] },
            "Primitive": { "shape": { "Cuboid": { "size": [6.0, 2.0, 6.0] } } },
            "Collider": { "shape": "Primitive" } } }
    ]));
    let door = app.world.find_by_name("Door").unwrap();
    app.world.insert_one(door, Collidable(false)).unwrap();
    step(&mut app, 2);
    settle_navigation(&mut app.world);
    let path = nav_path(
        &app.world,
        Vec3::new(-6.0, 0.0, -13.0),
        Vec3::new(-8.0, 0.0, 8.0),
    )
    .unwrap();
    assert!(path.is_complete(), "{:?}", path.points);
    assert!(path.points.len() >= 3, "goes around: {:?}", path.points);
    // A probe character walks the corners.
    let spec: box_engine::route::RouteSpec = serde_json::from_value(serde_json::json!({
        "waypoints": path.points.iter().map(|p| [p.x, p.y + 0.05, p.z]).collect::<Vec<_>>()
    }))
    .unwrap();
    let report = box_engine::route::walk_route(&app.world, &spec).unwrap();
    assert!(report.reached_all, "{report:?}");
}

#[test]
fn settings_change_and_switch_off() {
    let mut app = app(serde_json::json!([]));
    step(&mut app, 2);
    settle_navigation(&mut app.world);
    let p = Vec3::new(0.0, 0.0, -10.0);
    let edge = app
        .world
        .resource::<Navigation>()
        .mesh()
        .closest_point(Vec3::new(29.9, 0.0, -10.0))
        .unwrap();
    assert!(edge.x < 29.65, "{edge}");
    // A bigger agent keeps further from the edge.
    app.world
        .insert_resource(box_engine::nav_glue::NavSettings {
            agent_radius: 1.0,
            ..Default::default()
        });
    step(&mut app, 1);
    settle_navigation(&mut app.world);
    let edge = app
        .world
        .resource::<Navigation>()
        .mesh()
        .closest_point(Vec3::new(29.9, 0.0, -10.0))
        .unwrap();
    assert!(edge.x < 29.1, "{edge}");
    assert!(
        app.world
            .resource::<Navigation>()
            .mesh()
            .closest_point(p)
            .is_some()
    );
    app.world
        .insert_resource(box_engine::nav_glue::NavSettings {
            enabled: false,
            ..Default::default()
        });
    step(&mut app, 1);
    assert!(app.world.resource::<Navigation>().mesh().is_empty());
    assert!(nav_path(&app.world, p, Vec3::ZERO).is_some_and(|p| !p.is_complete()));
}

/// Bake and query costs (docs/NAVIGATION.md acceptance), on a 200 × 200 m
/// floor with 500 boxes: `cargo test --release -p box_engine --test
/// navigation cost -- --ignored --nocapture`.
#[test]
#[ignore]
fn cost() {
    use std::time::Instant;
    let mut boxes = Vec::new();
    let mut seed = 7u64;
    let mut rnd = || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((seed >> 33) as f32) / (1u64 << 31) as f32
    };
    for i in 0..500 {
        let at = [rnd() * 190.0 - 95.0, 0.75, rnd() * 190.0 - 95.0];
        let size = [0.5 + rnd() * 3.0, 1.5, 0.5 + rnd() * 3.0];
        boxes.push(
            serde_json::json!({ "op": "spawn", "name": format!("Box {i}"), "components": {
            "Transform": { "translation": at },
            "Primitive": { "shape": { "Cuboid": { "size": size } } },
            "Collider": { "shape": "Primitive" } } }),
        );
    }
    let mut world = World::with_registry(shared_engine_registry());
    let mut cmds = vec![
        serde_json::json!({ "op": "spawn", "name": "Ground", "components": {
        "Transform": { "translation": [0.0, -0.5, 0.0] },
        "Primitive": { "shape": { "Cuboid": { "size": [200.0, 1.0, 200.0] } } },
        "Collider": { "shape": "Primitive" } } }),
    ];
    cmds.extend(boxes);
    let cmds: Vec<Command> = serde_json::from_value(serde_json::Value::Array(cmds)).unwrap();
    apply_commands(&mut world, &mut EditStack::default(), &cmds, "level").unwrap();
    let mut app = app_with_world(world);
    app.world.insert_resource(Navigation::inline());
    // The first frames sync the colliders and bake inline.
    let t = Instant::now();
    step(&mut app, 1);
    let first = t.elapsed();
    let nav = app.world.resource::<Navigation>();
    let tiles = nav.mesh().tiles().count();
    let polys = nav.mesh().poly_count();
    println!(
        "full bake (inline, one thread): {:.1} ms, {tiles} tiles, {polys} polygons (frame incl. sync)",
        first.as_secs_f64() * 1000.0
    );

    // One box moved: its tiles rebake.
    let e = app.world.find_by_name("Box 0").unwrap();
    app.world
        .component_mut::<Transform>(e)
        .unwrap()
        .translation
        .x += 2.0;
    let before = app.world.resource::<Navigation>().tiles_baked;
    let t = Instant::now();
    step(&mut app, 1);
    let one = t.elapsed();
    let n = app.world.resource::<Navigation>().tiles_baked - before;
    println!(
        "one box moved: {n} tile(s) in {:.1} ms (frame incl. sync)",
        one.as_secs_f64() * 1000.0
    );

    // Idle frame.
    let t = Instant::now();
    step(&mut app, 60);
    println!(
        "idle: {:.3} ms per frame (whole app)",
        t.elapsed().as_secs_f64() * 1000.0 / 60.0
    );

    // Path queries across the level.
    let mesh = app.world.resource::<Navigation>().mesh().clone();
    let mut times = Vec::new();
    let mut complete = 0;
    for _ in 0..200 {
        let a = Vec3::new(rnd() * 180.0 - 90.0, 0.0, rnd() * 180.0 - 90.0);
        let b = Vec3::new(rnd() * 180.0 - 90.0, 0.0, rnd() * 180.0 - 90.0);
        let t = Instant::now();
        let p = mesh.find_path(a, b);
        times.push(t.elapsed().as_secs_f64() * 1000.0);
        complete += p.is_complete() as usize;
    }
    times.sort_by(f64::total_cmp);
    println!(
        "path queries: median {:.3} ms, p95 {:.3} ms, max {:.3} ms ({complete}/200 complete)",
        times[100], times[190], times[199]
    );

    // 50 agents walking across the level, goals changing every second.
    let goals: Vec<Vec3> = (0..50)
        .map(|_| Vec3::new(rnd() * 180.0 - 90.0, 0.0, rnd() * 180.0 - 90.0))
        .collect();
    let agents: Vec<Entity> = (0..50)
        .map(|i| {
            let at = Vec3::new(rnd() * 180.0 - 90.0, 0.92, rnd() * 180.0 - 90.0);
            app.world.spawn((
                Name::new(format!("Agent {i}")),
                Transform::from_translation(at),
                box_engine::nav_agent::NavAgent {
                    destination: Some(goals[i]),
                    ..Default::default()
                },
            ))
        })
        .collect();
    step(&mut app, 30);
    let mut agent_ms = Vec::new();
    for f in 0..300 {
        if f % 60 == 0 {
            for (i, e) in agents.iter().enumerate() {
                let g = goals[(i + f / 60) % goals.len()];
                app.world
                    .component_mut::<box_engine::nav_agent::NavAgent>(*e)
                    .unwrap()
                    .destination = Some(g);
            }
        }
        let t = Instant::now();
        box_engine::nav_agent::drive_nav_agents(&mut app.world);
        agent_ms.push(t.elapsed().as_secs_f64() * 1000.0);
        step(&mut app, 1);
    }
    agent_ms.sort_by(f64::total_cmp);
    println!(
        "50 agents (nav_agents system, one fixed step): median {:.3} ms, p95 {:.3} ms, max {:.3} ms",
        agent_ms[150], agent_ms[285], agent_ms[299]
    );
}

fn agent(app: &mut App, name: &str, at: Vec3, to: Option<Vec3>) -> Entity {
    app.world.spawn((
        Name::new(name),
        Transform::from_translation(at + Vec3::Y * 0.92),
        box_engine::nav_agent::NavAgent {
            destination: to,
            ..Default::default()
        },
    ))
}

#[test]
fn agents_walk_around_walls_and_take_turns_planning() {
    use box_engine::nav_agent::{NavAgentState, NavStatus, REPLANS_PER_STEP, nav_status};
    let mut app = app(serde_json::json!([]));
    app.world.insert_resource(Navigation::inline());
    app.check_order().unwrap();
    let door = app.world.find_by_name("Door").unwrap();
    app.world.insert_one(door, Collidable(false)).unwrap();
    step(&mut app, 3);
    let goal = Vec3::new(-6.0, 0.0, 6.0);
    let walker = agent(&mut app, "Walker", Vec3::new(-6.0, 0.0, -6.0), Some(goal));
    let mut arrived = None;
    for f in 0..60 * 15 {
        step(&mut app, 1);
        if nav_status(&app.world, walker) == NavStatus::Arrived {
            arrived = Some(f);
            break;
        }
    }
    let f = arrived.expect("the agent did not arrive");
    let at = app.world.get::<Transform>(walker).unwrap().translation;
    assert!(
        Vec2::new(at.x - goal.x, at.z - goal.z).length() < 0.6,
        "{at}"
    );
    // Around the wall through the gap: about 2 × 8.5 m at 3.5 m/s.
    assert!(f > 60 * 4 && f < 60 * 9, "{f} frames");

    // Closing the door: unreachable, it waits at the nearest point.
    app.world.insert_one(door, Collidable(true)).unwrap();
    app.world
        .component_mut::<box_engine::nav_agent::NavAgent>(walker)
        .unwrap()
        .destination = Some(Vec3::new(-6.0, 0.0, -6.0));
    step(&mut app, 60 * 10);
    assert_eq!(nav_status(&app.world, walker), NavStatus::Unreachable);
    let at = app.world.get::<Transform>(walker).unwrap().translation;
    assert!(at.z > 0.0 && at.z < 1.5, "waits by the wall: {at}");

    // Twenty agents asking at once plan at most REPLANS_PER_STEP a step.
    let crowd: Vec<Entity> = (0..20)
        .map(|i| {
            let x = -9.0 + (i % 10) as f32 * 2.0;
            let z = 4.0 + (i / 10) as f32 * 2.0;
            agent(
                &mut app,
                &format!("A{i}"),
                Vec3::new(x, 0.0, z),
                Some(Vec3::new(5.0, 0.0, 10.0)),
            )
        })
        .collect();
    step(&mut app, 2);
    let planned = crowd
        .iter()
        .filter(|e| {
            app.world
                .get::<NavAgentState>(**e)
                .is_some_and(|s| s.status != NavStatus::Idle && !s.corners.is_empty())
        })
        .count();
    assert!(
        (REPLANS_PER_STEP..=REPLANS_PER_STEP * 2 + 1).contains(&planned),
        "{planned}"
    );
    step(&mut app, 60 * 8);
    // They arrive without stacking on one spot.
    let ends: Vec<Vec3> = crowd
        .iter()
        .map(|e| app.world.get::<Transform>(*e).unwrap().translation)
        .collect();
    let near = ends
        .iter()
        .filter(|p| Vec2::new(p.x - 5.0, p.z - 10.0).length() < 3.5)
        .count();
    assert!(near >= 15, "{near} of 20 near the goal");
    for (i, a) in ends.iter().enumerate() {
        for b in &ends[i + 1..] {
            assert!(
                Vec2::new(a.x - b.x, a.z - b.z).length() > 0.3,
                "{a} {b} overlap"
            );
        }
    }
}
