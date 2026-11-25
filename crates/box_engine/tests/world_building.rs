//! World building fixes (docs/WORLD_BUILDING.md W0): `ColliderShape::Mesh`
//! and `ConvexHull` on generated meshes (a `Primitive`, a `Sweep`), so
//! ground shaped with generators holds a character.

use box_engine::prelude::*;

const DT: f32 = 1.0 / 60.0;
/// Capsule centre height of a default character standing on y = 0.
const STAND: f32 = 0.9 + 0.02;

fn app() -> App {
    Game::new().new_app()
}

fn floor(app: &mut App) {
    app.world.spawn((
        Transform::from_xyz(0.0, -0.5, 0.0),
        Collider::cuboid(Vec3::new(80.0, 1.0, 80.0)),
    ));
}

fn character(app: &mut App, at: Vec3) -> Entity {
    app.world.spawn((
        Transform::from_translation(at),
        KinematicCharacter::default(),
    ))
}

fn pos(app: &App, e: Entity) -> Vec3 {
    app.world.get::<Transform>(e).unwrap().translation
}

/// Walk `e` with horizontal velocity `v` for `frames` frames; returns the
/// highest centre height reached.
fn walk(app: &mut App, e: Entity, v: Vec3, frames: usize) -> f32 {
    let mut top = f32::MIN;
    for _ in 0..frames {
        if let Some(c) = app.world.component_mut::<KinematicCharacter>(e) {
            c.velocity.x = v.x;
            c.velocity.z = v.z;
        }
        app.run_frames(1, DT);
        top = top.max(pos(app, e).y);
    }
    top
}

fn ray_down(app: &App, x: f32, z: f32) -> Option<RayHit> {
    app.world
        .resource::<Physics>()
        .ray(Vec3::new(x, 20.0, z), Vec3::NEG_Y, 40.0)
}

#[test]
fn terrain_paths_join_and_follow_the_exact_surface_across_chunks() {
    use box_core::derived::Derived;
    use box_engine::box_proc::terrain_path::TERRAIN_PATH_RULE;

    let mut app = app();
    let uid = Uid::new();
    let terrain = app.world.spawn((
        uid,
        Transform::IDENTITY,
        Terrain {
            size: Vec2::splat(24.0),
            cell_size: 0.5,
            stamps: vec![TerrainStamp::Hill {
                center: Vec2::splat(4.0),
                radius: 6.0,
                height: 0.75,
            }],
            ..Terrain::default()
        },
    ));
    // A crossing and a shared endpoint, right on a four-chunk corner.
    // Saved spline heights are deliberately unrelated to the terrain.
    for (a, b, target) in [
        (
            Vec3::new(-1.0, 5.0, 4.0),
            Vec3::new(9.0, -5.0, 4.0),
            Some(uid),
        ),
        (Vec3::new(4.0, 0.0, -1.0), Vec3::new(4.0, 0.0, 4.0), None),
        (Vec3::new(4.0, 0.0, 4.0), Vec3::new(4.0, 0.0, 9.0), None),
    ] {
        app.world.spawn((
            Uid::new(),
            Transform::IDENTITY,
            TerrainPath {
                width: 2.0,
                terrain: target,
            },
            Spline::from_positions(&[a, b], SplineMode::Polyline, false),
        ));
    }
    let stairs = app.world.spawn((
        Uid::new(),
        Transform::from_xyz(4.0, 10.0, 4.0),
        FollowGround { offset: 0.15 },
    ));
    let check_surface = |world: &World| {
        let ground = world.resource::<Ground>();
        let meshes = world.resource::<Assets<MeshData>>();
        let mut parts = 0;
        let mut projected_area = 0.0;
        for (entity, derived, state) in world.query::<(Entity, &Derived, &ProcState)>().iter() {
            if derived.rule != TERRAIN_PATH_RULE {
                continue;
            }
            assert_eq!(derived.owner, terrain);
            assert!(!world.has::<Collider>(entity));
            let mesh = meshes
                .get(state.mesh().expect("generated path mesh"))
                .unwrap();
            mesh.validate().unwrap();
            parts += 1;
            for indices in mesh.indices.as_chunks::<3>().0 {
                let [a, b, c] = indices.map(|index| mesh.positions[index as usize]);
                let area = (b - a).cross(c - a).y * 0.5;
                assert!(area >= 0.0, "wrong path winding at {a:?}");
                projected_area += area;
                for point in [a, b, c, (a + b + c) / 3.0] {
                    let h = ground.height(point.x, point.z).unwrap();
                    assert!(
                        (point.y - h - 0.01).abs() < 1e-4,
                        "path floats through {point:?} at height {h}"
                    );
                }
            }
        }
        assert_eq!(parts, 4);
        // Two 10 × 2 strips with four half-disc caps and one shared 2 × 2
        // patch. Neither the intersection nor the shared ends add area twice.
        assert!((projected_area - 42.1229).abs() < 0.01, "{projected_area}");
    };
    app.run_frames(3, DT);
    check_surface(&app.world);
    let before = pos(&app, stairs).y;
    let TerrainStamp::Hill { height, .. } =
        &mut app.world.component_mut::<Terrain>(terrain).unwrap().stamps[0]
    else {
        unreachable!();
    };
    *height += 0.5;
    app.run_frames(1, DT);
    box_engine::box_proc::settle(&mut app.world);
    app.run_frames(2, DT);
    check_surface(&app.world);
    assert!((pos(&app, stairs).y - before - 0.5).abs() < 1e-5);
    let hit = ray_down(&app, 4.0, 4.0).expect("terrain under the path collides");
    assert!((hit.point.y - 1.25).abs() < 0.01);
}

#[test]
fn a_character_walks_over_a_generated_hill_with_a_mesh_collider() {
    let mut app = app();
    floor(&mut app);
    // A 1.8 m hill: the cap of a 10 m Primitive sphere sunk into the floor
    // (35 degrees at the rim), as an agent shapes ground.
    let hill = app.world.spawn((
        Name::new("Hill"),
        Transform::from_xyz(0.0, -8.2, 0.0),
        Primitive::sphere(10.0),
        Collider::new(ColliderShape::Mesh),
    ));
    let e = character(&mut app, Vec3::new(-10.0, STAND, 0.0));
    app.run_frames(5, DT);
    let hit = ray_down(&app, 0.0, 0.0).expect("the hill collides");
    assert_eq!(hit.entity, hill);
    assert!((hit.point.y - 1.8).abs() < 0.02, "{:?}", hit.point);

    let top = walk(&mut app, e, Vec3::new(3.0, 0.0, 0.0), 360);
    assert!(top > 1.7 + STAND, "climbed the hill: top {top}");
    let end = pos(&app, e);
    assert!(end.x > 6.5, "crossed it: {end}");
    assert!((end.y - STAND).abs() < 0.05, "back on the floor: {end}");
    assert!(app.world.get::<KinematicCharacter>(e).unwrap().grounded);
}

#[test]
fn primitive_and_sweep_meshes_collide_and_follow_regeneration() {
    let mut app = app();
    // A Primitive sphere with a Mesh collider (not its analytic shape).
    let ball = app.world.spawn((
        Transform::from_xyz(-6.0, 0.0, 0.0),
        Primitive::sphere(2.0),
        Collider::new(ColliderShape::Mesh),
    ));
    // A 4 m wide, 0.3 m thick ramp swept along a rising polyline (its own
    // spline is found by uid, as in a scene).
    let ramp = app.world.spawn((
        Uid::new(),
        Transform::default(),
        Spline::from_positions(
            &[Vec3::new(4.0, 0.0, 0.0), Vec3::new(12.0, 2.0, 0.0)],
            SplineMode::Polyline,
            false,
        ),
        Sweep {
            profile: SweepProfile::Rect {
                width: 4.0,
                height: 0.3,
            },
            ..Sweep::default()
        },
        Collider::new(ColliderShape::Mesh),
    ));
    app.run_frames(3, DT);
    let hit = ray_down(&app, -6.0, 0.0).expect("sphere");
    assert_eq!(hit.entity, ball);
    assert!((hit.point.y - 2.0).abs() < 0.05, "{:?}", hit.point);
    let hit = ray_down(&app, 8.0, 0.0).expect("ramp");
    assert_eq!(hit.entity, ramp);
    assert!((hit.point.y - 1.3).abs() < 0.05, "{:?}", hit.point);

    // Regenerating the ramp (wider) rebuilds the collider.
    assert!(ray_down(&app, 8.0, 3.0).is_none());
    app.world.component_mut::<Sweep>(ramp).unwrap().profile = SweepProfile::Rect {
        width: 8.0,
        height: 0.3,
    };
    // Edits regenerate on the runtime's throttle; settle it, then sync.
    box_engine::box_proc::settle(&mut app.world);
    app.run_frames(2, DT);
    let hit = ray_down(&app, 8.0, 3.0).expect("the wider ramp");
    assert_eq!(hit.entity, ramp);

    // Removing the generator removes the collider's mesh (and the collider).
    let _ = app.world.remove_one::<Sweep>(ramp);
    box_engine::box_proc::settle(&mut app.world);
    app.run_frames(2, DT);
    assert!(ray_down(&app, 8.0, 0.0).is_none());
}

#[test]
fn a_convex_hull_of_a_generated_mesh_is_solid() {
    let mut app = app();
    let e = app.world.spawn((
        Transform::from_xyz(0.0, 0.0, 0.0),
        Primitive::cuboid(Vec3::splat(2.0)),
        Collider::new(ColliderShape::ConvexHull),
    ));
    app.run_frames(3, DT);
    let physics = app.world.resource::<Physics>();
    assert_eq!(
        physics.contains_point(Vec3::ZERO, &QueryFilter::default()),
        vec![e]
    );
}

#[test]
fn a_mesh_collider_without_a_mesh_source_collides_with_nothing() {
    let mut app = app();
    app.world.spawn((
        Name::new("Bare"),
        Transform::default(),
        Collider::new(ColliderShape::Mesh),
    ));
    app.run_frames(3, DT);
    assert!(ray_down(&app, 0.0, 0.0).is_none());
    assert!(app.world.resource::<Physics>().is_empty());
}

/// W1 route check: a probe walks waypoints against colliders and reports
/// reached legs, a blocking wall, a step up and a drop.
#[test]
fn walk_route_reports_reached_stuck_steps_and_falls() {
    use box_engine::route::{RouteSpec, walk_route};
    let mut app = app();
    floor(&mut app);
    // A wall across x = 5 (z from -4 to 4), a 0.25 m kerb across x = -5,
    // and a 1.5 m high block at z = 10 to walk off.
    for (at, size) in [
        (Vec3::new(5.0, 1.5, 0.0), Vec3::new(0.4, 3.0, 8.0)),
        (Vec3::new(-5.0, 0.125, 0.0), Vec3::new(0.6, 0.25, 8.0)),
        (Vec3::new(0.0, 0.75, 10.0), Vec3::new(4.0, 1.5, 4.0)),
    ] {
        app.world
            .spawn((Transform::from_translation(at), Collider::cuboid(size)));
    }
    app.run_frames(2, DT);
    let spec: RouteSpec = serde_json::from_value(serde_json::json!({
        "waypoints": [[0, 0, 0], [-8, 0, 0], [0, 0, 0], [9, 0, 0], [0, 1.5, 10], [0, 0, 14]]
    }))
    .unwrap();
    let r = walk_route(&app.world, &spec).unwrap();
    let legs = &r.legs;
    assert_eq!(legs.len(), 5);
    // Over the kerb: reached, with a step of about its height.
    assert!(legs[0].reached, "{legs:?}");
    assert!(
        legs[0].max_step_up > 0.2 && legs[0].max_step_up < 0.3,
        "{legs:?}"
    );
    assert!(legs[1].reached);
    // Into the wall: stuck in front of it.
    assert!(!legs[2].reached);
    let at = legs[2].stuck_at.unwrap();
    assert!(at[0] > 4.0 && at[0] < 5.0, "{at:?}");
    assert!(!r.reached_all);
    // Up onto the block is too high a step: stuck at its foot; the walk
    // goes on from the waypoint on top and drops off the far side.
    assert!(!legs[3].reached);
    assert!(legs[4].reached, "{legs:?}");
    assert_eq!(legs[4].falls, 1);
    assert!((legs[4].max_drop - 1.5).abs() < 0.1, "{legs:?}");
    assert!(r.start[1].abs() < 0.05, "{:?}", r.start);
}

/// W2: a terrain of stamps is drawn in chunks with mesh colliders; a
/// character walks over its hill; editing one stamp regenerates only the
/// chunks under it.
#[test]
fn terrain_chunks_collide_and_regenerate_locally() {
    use box_engine::box_proc::ProcState;
    let mut app = app();
    let terrain = app.world.spawn((
        Uid::new(),
        Transform::default(),
        Terrain {
            size: Vec2::splat(128.0),
            stamps: vec![
                TerrainStamp::Hill {
                    center: Vec2::new(0.0, 0.0),
                    radius: 20.0,
                    height: 4.0,
                },
                TerrainStamp::Hill {
                    center: Vec2::new(-48.0, -48.0),
                    radius: 8.0,
                    height: 2.0,
                },
            ],
            ..Terrain::default()
        },
    ));
    box_engine::box_proc::settle(&mut app.world);
    app.run_frames(2, DT);
    let chunks = box_engine::box_proc::derive::parts_of(&app.world, terrain);
    assert_eq!(chunks.len(), 16, "128 m at 1 m cells: 4 x 4 chunks of 32");
    let hit = ray_down(&app, 0.0, 0.0).expect("the terrain collides");
    assert!((hit.point.y - 4.0).abs() < 0.01, "{:?}", hit.point);

    // Walk over the hill's shoulder.
    let e = character(&mut app, Vec3::new(-30.0, STAND + 0.5, 4.0));
    app.run_frames(10, DT);
    let top = walk(&mut app, e, Vec3::new(4.0, 0.0, 0.0), 700);
    assert!(top > 3.5 + STAND, "climbed: {top}");
    assert!(pos(&app, e).x > 10.0, "crossed: {}", pos(&app, e));

    // Raise the far hill: only the chunk at (-48, -48) regenerates.
    let generations = |app: &App| -> Vec<u64> {
        box_engine::box_proc::derive::parts_of(&app.world, terrain)
            .iter()
            .map(|(p, _)| app.world.get::<ProcState>(*p).unwrap().generations())
            .collect()
    };
    let before = generations(&app);
    if let TerrainStamp::Hill { height, .. } =
        &mut app.world.component_mut::<Terrain>(terrain).unwrap().stamps[1]
    {
        *height = 3.0;
    }
    app.run_frames(1, DT);
    box_engine::box_proc::settle(&mut app.world);
    app.run_frames(2, DT);
    let after = generations(&app);
    let changed = before.iter().zip(&after).filter(|(a, b)| a != b).count();
    assert_eq!(changed, 1, "{before:?} -> {after:?}");
    let hit = ray_down(&app, -48.0, -48.0).unwrap();
    assert!((hit.point.y - 3.0).abs() < 0.01, "{:?}", hit.point);
}

/// W2 cost (`cargo test --release -p box_engine --test world_building
/// terrain_cost -- --ignored --nocapture`): a 256 m terrain at 1 m cells
/// (64 chunks) with eight stamps, built once, then one hill moved a step at
/// a time as a drag would, with the chunk regenerations and the mesh
/// collider rebuilds each step causes.
#[test]
#[ignore]
fn terrain_cost() {
    use std::time::Instant;
    let mut app = app();
    let mut stamps = vec![TerrainStamp::Noise {
        seed: 3,
        scale: 24.0,
        amplitude: 0.8,
        octaves: 3,
    }];
    for i in 0..6 {
        let a = i as f32;
        stamps.push(TerrainStamp::Hill {
            center: Vec2::new(-90.0 + a * 36.0, -60.0 + (a * 1.7).sin() * 40.0),
            radius: 18.0,
            height: 6.0,
        });
    }
    stamps.push(TerrainStamp::FlattenPath {
        points: vec![Vec3::new(-120.0, 0.5, 0.0), Vec3::new(120.0, 1.5, 10.0)],
        width: 3.0,
        bank: 2.0,
    });
    let terrain = app.world.spawn((
        Uid::new(),
        Transform::default(),
        Terrain {
            size: Vec2::splat(256.0),
            stamps,
            ..Terrain::default()
        },
    ));
    // Transforms, the terrain rule's 64 chunk parts, every chunk's mesh.
    box_engine::box_core::transform::propagate_transforms(&mut app.world);
    let start = Instant::now();
    box_engine::box_proc::rule::run_rules(&mut app.world);
    box_engine::box_proc::settle(&mut app.world);
    println!(
        "256 m terrain, 64 chunks, 8 stamps: first build {:.1} ms ({} generated)",
        start.elapsed().as_secs_f64() * 1e3,
        app.world
            .resource::<box_engine::box_proc::ProcRuntime>()
            .stats()
            .generated
    );
    let t = Instant::now();
    app.run_frames(1, DT);
    println!(
        "  then the first frame with 64 heightfield colliders: {:.2} ms",
        t.elapsed().as_secs_f64() * 1e3
    );
    app.run_frames(1, DT);
    let mut gen_ms = Vec::new();
    let mut sync_ms = Vec::new();
    let mut regenerated = Vec::new();
    for _ in 0..20 {
        if let TerrainStamp::Hill { center, .. } =
            &mut app.world.component_mut::<Terrain>(terrain).unwrap().stamps[1]
        {
            center.x += 0.5;
        }
        let before = app
            .world
            .resource::<box_engine::box_proc::ProcRuntime>()
            .stats()
            .generated;
        // Rules, then the flush of the chunks they changed.
        let t = Instant::now();
        box_engine::box_proc::rule::run_rules(&mut app.world);
        box_engine::box_proc::settle(&mut app.world);
        gen_ms.push(t.elapsed().as_secs_f64() * 1e3);
        regenerated.push(
            app.world
                .resource::<box_engine::box_proc::ProcRuntime>()
                .stats()
                .generated
                - before,
        );
        // A frame: physics rebuilds the changed chunks' mesh colliders.
        let t = Instant::now();
        app.run_frames(1, DT);
        sync_ms.push(t.elapsed().as_secs_f64() * 1e3);
    }
    let med = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        (v[v.len() / 2], v[v.len() - 1])
    };
    let (g, gmax) = med(&mut gen_ms);
    let (f, fmax) = med(&mut sync_ms);
    println!(
        "moving one 18 m hill ({regenerated:?} chunks per step): rule + regeneration median {g:.2} ms (max {gmax:.2}), next frame with collider rebuilds median {f:.2} ms (max {fmax:.2})"
    );
}

/// W3 cost (`cargo test --release -p box_engine --test world_building
/// ground_cost -- --ignored --nocapture`): the ground system with 1,000
/// `FollowGround` entities on a 256 m terrain, idle and after a ridge edit
/// that moves about a third of them.
#[test]
#[ignore]
fn ground_cost() {
    use std::time::Instant;
    let mut app = app();
    let terrain = app.world.spawn((
        Uid::new(),
        Transform::default(),
        Terrain {
            size: Vec2::splat(256.0),
            stamps: vec![TerrainStamp::Ridge {
                points: vec![Vec2::new(-128.0, 0.0), Vec2::new(128.0, 0.0)],
                width: 80.0,
                height: 4.0,
            }],
            ..Terrain::default()
        },
    ));
    for i in 0..1000 {
        let (x, z) = ((i % 40) as f32 * 6.0 - 117.0, (i / 40) as f32 * 9.0 - 112.0);
        app.world.spawn((
            Uid::new(),
            Transform::from_xyz(x, 0.0, z),
            FollowGround::default(),
        ));
    }
    box_engine::terrain_glue::update_ground(&mut app.world);
    let time = |app: &mut App, n: u32| {
        let start = Instant::now();
        for _ in 0..n {
            box_engine::terrain_glue::update_ground(&mut app.world);
        }
        start.elapsed().as_secs_f64() * 1000.0 / n as f64
    };
    let idle = time(&mut app, 200);
    let mut edits = Vec::new();
    for k in 0..20 {
        if let TerrainStamp::Ridge { height, .. } =
            &mut app.world.component_mut::<Terrain>(terrain).unwrap().stamps[0]
        {
            *height = 4.0 + k as f32 * 0.05;
        }
        edits.push(time(&mut app, 1));
    }
    let edit = edits.iter().sum::<f64>() / edits.len() as f64;
    println!(
        "ground system, 1,000 followers on a 256 m terrain: idle {idle:.4} ms per frame, ridge edit {edit:.3} ms (mean of 20)"
    );
}
