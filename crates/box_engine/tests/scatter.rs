//! Scatter in the engine (`box_engine::scatter_glue`): instances are
//! runtime entities under derived chunk parts, never saved; save and reload
//! reproduce identical instance transforms; a paint stroke through
//! `box_core::edit` regenerates only the touched chunks and its undo
//! restores a byte-identical scene; ground edits re-scatter only the chunks
//! under them; and the cost of 10,000 and 50,000 instances (spawn, idle
//! frame, a paint stroke) against one `reconcile` part per instance.
//!
//! `cargo test --release -p box_engine --test scatter -- --nocapture`
//! prints the timings.

#[allow(dead_code)]
#[path = "../examples/common/scatter.rs"]
mod scatter_demo;

use std::time::{Duration, Instant};

use box_engine::box_core::asset_ref::AssetRef;
use box_engine::box_core::assets::AssetLookup;
use box_engine::box_core::derived::{Derived, PartKey};
use box_engine::box_core::edit::EditStack;
use box_engine::box_core::scene;
use box_engine::box_proc::derive::{Part, reconcile};
use box_engine::box_proc::hash::StableHasher;
use box_engine::box_proc::scatter::{ChunkCoord, Scatter, ScatterMask};
use box_engine::box_proc::{ProcClock, ProcSettings};
use box_engine::prelude::*;
use box_engine::scatter_glue::{ScatterChunk, ScatterRuntime, ScatterStats};
use box_engine::{Game, settle_assets};
use scatter_demo::{ScatterDemoPlugin, add_mesh_asset, build_scatter_scene, demo_species, square};

fn game() -> Game {
    Game::new().with_plugin(ScatterDemoPlugin)
}

/// An app with a manual regeneration clock (advanced by [`frame`]).
fn app() -> App {
    let mut app = game().new_app();
    app.world.insert_resource(ProcSettings {
        clock: ProcClock::Manual(Duration::from_secs(3600)),
        ..ProcSettings::default()
    });
    app
}

/// One 60 Hz frame.
fn frame(app: &mut App) {
    if let Some(s) = app.world.get_resource_mut::<ProcSettings>()
        && let ProcClock::Manual(t) = &mut s.clock
    {
        *t += Duration::from_secs_f64(1.0 / 60.0);
    }
    app.update();
}

fn settle(app: &mut App) {
    app.update();
    settle_assets(&mut app.world, Duration::from_secs(5));
}

fn entity(world: &World, uid: Uid) -> Entity {
    world.entity_by_uid(uid).unwrap()
}

fn stats(world: &World) -> ScatterStats {
    world.resource::<ScatterRuntime>().stats()
}

/// The chunks of `owner` with their instance entities and transforms, by
/// chunk.
fn chunks(world: &World, owner: Entity) -> Vec<(ChunkCoord, Vec<(Entity, Transform)>)> {
    let mut out: Vec<(ChunkCoord, Vec<(Entity, Transform)>)> = world
        .children(owner)
        .into_iter()
        .filter_map(|c| {
            let k = *world.get::<ScatterChunk>(c)?;
            let list = world
                .children(c)
                .into_iter()
                .map(|i| (i, *world.get::<Transform>(i).unwrap()))
                .collect();
            Some(((k.x, k.z), list))
        })
        .collect();
    out.sort_by_key(|(c, _)| (c.1, c.0));
    out
}

/// A hash of every instance transform of `owner`, by chunk.
fn transforms_hash(world: &World, owner: Entity) -> u64 {
    let mut h = StableHasher::new();
    for (c, list) in chunks(world, owner) {
        h.write_i64(c.0.into());
        h.write_i64(c.1.into());
        // Child order follows spawn order; compare as a set.
        let mut bits: Vec<Vec<u32>> = list
            .iter()
            .map(|(_, t)| {
                t.translation
                    .to_array()
                    .into_iter()
                    .chain(t.rotation.to_array())
                    .chain(t.scale.to_array())
                    .map(f32::to_bits)
                    .collect()
            })
            .collect();
        bits.sort();
        for v in bits.into_iter().flatten() {
            h.write_u32(v);
        }
    }
    h.finish()
}

fn instance_count(world: &World, owner: Entity) -> usize {
    chunks(world, owner).iter().map(|(_, l)| l.len()).sum()
}

#[test]
fn instances_are_runtime_only_and_a_reload_reproduces_them() {
    let mut app = app();
    let (_, meadow, rocks, flowers) = build_scatter_scene(&mut app.world);
    settle(&mut app);
    let w = &app.world;
    let owners = [meadow, rocks, flowers].map(|u| entity(w, u));
    let counts = owners.map(|o| instance_count(w, o));
    assert!(counts[0] > 1500, "{counts:?}");
    assert!(counts[1] > 20 && counts[2] > 100, "{counts:?}");
    let hashes = owners.map(|o| transforms_hash(w, o));
    // Instances are generated: chunk parts and instances are derived.
    let (_, list) = &chunks(w, owners[0])[0];
    let (e, _) = list[0];
    assert!(box_engine::box_core::prefab::is_generated(w, e));
    assert!(w.uid(e).is_none());
    assert_eq!(
        box_engine::box_core::prefab::editable_ancestor(w, e),
        owners[0]
    );
    assert!(w.get::<MeshRenderer>(e).is_some());
    assert!(
        w.get::<GlobalTransform>(e).is_some(),
        "propagated in the flush"
    );

    // The saved scene holds the intent only.
    let text = scene::save_string(w).unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let entities = json["entities"].as_array().unwrap();
    assert_eq!(entities.len(), 4, "hills and three scatters");
    assert!(!text.contains("chunk_"), "no chunk part is saved");
    assert!(!text.contains("MeshRenderer"));

    // Reload: the same instance transforms (assets registered again).
    let g = game();
    let loaded = scene::load(text.as_bytes(), g.registry()).unwrap();
    let mut reloaded = g.app(loaded);
    demo_species(&mut reloaded.world);
    settle(&mut reloaded);
    let w2 = &reloaded.world;
    for (i, uid) in [meadow, rocks, flowers].into_iter().enumerate() {
        assert_eq!(
            transforms_hash(w2, entity(w2, uid)),
            hashes[i],
            "scatter {i}"
        );
    }
    // Saving the reloaded world gives the same file.
    assert_eq!(scene::save_string(w2).unwrap(), text);
}

#[test]
fn a_stroke_regenerates_only_touched_chunks_and_undo_restores_the_scene() {
    let mut app = app();
    let (_, _, _, flowers) = build_scatter_scene(&mut app.world);
    settle(&mut app);
    let owner = entity(&app.world, flowers);
    let before_chunks = chunks(&app.world, owner);
    let before_scene = scene::save_string(&app.world).unwrap();
    let before_hash = transforms_hash(&app.world, owner);
    assert!(before_chunks.len() >= 6, "{}", before_chunks.len());

    // One stroke: three dabs inside chunk (−1, 1), as one edit step.
    let mut mask: ScatterMask = app.world.get::<Scatter>(owner).unwrap().mask.clone();
    let mut touched = Vec::new();
    for x in [-5.0, -4.5, -4.0] {
        touched.extend(mask.paint(&[], Vec2::new(x, 12.0), 0.8, 1.0, true));
    }
    touched.sort();
    touched.dedup();
    assert_eq!(touched, vec![(-1, 1)]);
    let mut edits = EditStack::new();
    let path = app.world.field_path("Scatter.mask").unwrap();
    edits.begin("Erase Scatter");
    edits
        .set_field(&mut app.world, flowers, &path, &mask)
        .unwrap();
    edits.commit();
    let placed = stats(&app.world).chunks_placed;
    // Within the throttle interval of the settle's flush: two frames.
    frame(&mut app);
    frame(&mut app);
    let s = stats(&app.world);
    assert_eq!(s.chunks_placed - placed, 1, "one chunk placed again");
    assert!(s.last.despawned > 0 && s.last.spawned == 0, "{:?}", s.last);
    let after_chunks = chunks(&app.world, owner);
    for ((c, a), (c2, b)) in before_chunks.iter().zip(&after_chunks) {
        assert_eq!(c, c2);
        if touched.contains(c) {
            assert!(b.len() < a.len());
            assert!(b.iter().all(|x| a.contains(x)), "kept instances untouched");
        } else {
            assert_eq!(a, b, "chunk {c:?}: same entities and transforms");
        }
    }

    // Undo: the scene file is byte-identical, the instances come back.
    assert!(edits.undo(&mut app.world).unwrap());
    frame(&mut app);
    frame(&mut app);
    assert_eq!(scene::save_string(&app.world).unwrap(), before_scene);
    assert_eq!(transforms_hash(&app.world, owner), before_hash);
}

#[test]
fn ground_edits_rescatter_only_the_chunks_under_them() {
    let mut app = app();
    let w = &mut app.world;
    // A flat 32 m grid as an in-memory mesh asset on a ground entity.
    let n = 32;
    let mut positions = Vec::new();
    for j in 0..=n {
        for i in 0..=n {
            positions.push(Vec3::new(i as f32 - 16.0, 0.0, j as f32 - 16.0));
        }
    }
    let mut indices = Vec::new();
    for j in 0..n {
        for i in 0..n {
            let a = (j * (n + 1) + i) as u32;
            let c = a + n as u32 + 1;
            indices.extend([a, c, a + 1, a + 1, c, c + 1]);
        }
    }
    let grid = MeshData {
        positions,
        indices,
        ..MeshData::default()
    };
    let mesh = add_mesh_asset(w, "test/ground.mesh", grid.clone());
    let ground = Uid::new();
    w.spawn((
        ground,
        Transform::IDENTITY,
        box_engine::box_core::mesh_asset::MeshAsset {
            mesh: mesh.clone(),
            materials: vec![AssetRef::NONE],
            cast_shadows: true,
        },
    ));
    let species = demo_species(w);
    let owner_uid = Uid::new();
    w.spawn((
        owner_uid,
        Transform::IDENTITY,
        Scatter {
            species: vec![species.rock],
            density: 1.0,
            min_distance: 0.4,
            area: square(30.0),
            surface: Some(ground),
            ..Scatter::default()
        },
    ));
    settle(&mut app);
    let owner = entity(&app.world, owner_uid);
    let before = chunks(&app.world, owner);
    assert_eq!(before.len(), 16);
    assert!(before.iter().all(|(_, l)| !l.is_empty()));

    // Raise one vertex near (−12, −12): only the chunk under it changes.
    let mut bumped = grid.clone();
    let i = bumped
        .positions
        .iter()
        .position(|p| p.x == -12.0 && p.z == -12.0)
        .unwrap();
    bumped.positions[i].y = 0.6;
    let handle = app
        .world
        .resource::<AssetLookup>()
        .get::<MeshData>(&mesh)
        .unwrap();
    app.world
        .resource_mut::<Assets<MeshData>>()
        .set(handle, bumped);
    let placed = stats(&app.world).chunks_placed;
    frame(&mut app);
    frame(&mut app);
    assert_eq!(stats(&app.world).chunks_placed - placed, 1);
    let after = chunks(&app.world, owner);
    for ((c, a), (_, b)) in before.iter().zip(&after) {
        if *c != (-2, -2) {
            assert_eq!(a, b, "chunk {c:?}");
        }
    }

    // Moving the ground re-scatters every chunk: instances follow it.
    let ge = entity(&app.world, ground);
    *app.world.get_mut::<Transform>(ge).unwrap() = Transform::from_xyz(0.0, 2.0, 0.0);
    let placed = stats(&app.world).chunks_placed;
    frame(&mut app);
    frame(&mut app);
    assert_eq!(stats(&app.world).chunks_placed - placed, 16);
    for (_, list) in chunks(&app.world, owner) {
        assert!(list.iter().all(|(_, t)| t.translation.y >= 2.0 - 1e-4));
    }

    // Removing the Scatter removes its chunks and instances.
    app.world.remove_one::<Scatter>(owner).unwrap();
    frame(&mut app);
    assert!(chunks(&app.world, owner).is_empty());
    assert!(app.world.query::<&Derived>().iter().next().is_none());
}

// ----- timings -----

/// Flat ground, two species with tints, `n` instances (about).
fn bench_scatter(world: &mut World, n: usize) -> Uid {
    let species = demo_species(world);
    let density = 2.0;
    let size = (n as f32 / density).sqrt();
    let uid = Uid::new();
    world.spawn((
        uid,
        Transform::IDENTITY,
        Scatter {
            species: vec![species.grass, species.bush],
            density,
            min_distance: 0.3,
            area: square(size),
            ..Scatter::default()
        },
    ));
    uid
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

#[test]
fn scatter_timings() {
    for n in [10_000usize, 50_000] {
        let mut app = app();
        let uid = bench_scatter(&mut app.world, n);
        let start = Instant::now();
        settle(&mut app);
        let spawn = start.elapsed();
        let owner = entity(&app.world, uid);
        let count = instance_count(&app.world, owner);
        let chunk_count = chunks(&app.world, owner).len();
        assert!(count as f32 > n as f32 * 0.85, "{count}");

        // Idle frames: nothing changes.
        let mut idle = Vec::new();
        for _ in 0..30 {
            let t = Instant::now();
            frame(&mut app);
            idle.push(t.elapsed());
        }
        let idle_mean = idle.iter().map(|d| ms(*d)).sum::<f64>() / idle.len() as f64;

        // A paint stroke: 60 frames of dabs (one per frame) along a line,
        // written into the Scatter each frame; the flush is throttled.
        let mut mask = app.world.get::<Scatter>(owner).unwrap().mask.clone();
        let area = app.world.get::<Scatter>(owner).unwrap().area.clone();
        let mut frames = Vec::new();
        let mut flushes = Vec::new();
        let placed0 = stats(&app.world).chunks_placed;
        for k in 0..60 {
            let p = Vec2::new(-10.0 + k as f32 * 0.3, 2.0);
            let t = Instant::now();
            mask.paint(&area, p, 1.5, 0.6, true);
            app.world.get_mut::<Scatter>(owner).unwrap().mask = mask.clone();
            let before = stats(&app.world).flushes;
            frame(&mut app);
            frames.push(t.elapsed());
            let s = stats(&app.world);
            if s.flushes > before {
                flushes.push(s.last);
            }
        }
        frame(&mut app);
        frame(&mut app);
        let stroke_chunks = stats(&app.world).chunks_placed - placed0;
        let frame_mean = frames.iter().map(|d| ms(*d)).sum::<f64>() / frames.len() as f64;
        let frame_max = frames.iter().map(|d| ms(*d)).fold(0.0, f64::max);
        let flush_mean =
            flushes.iter().map(|f| ms(f.time)).sum::<f64>() / flushes.len().max(1) as f64;
        let flush_max = flushes.iter().map(|f| ms(f.time)).fold(0.0, f64::max);

        // The alternative: one reconcile part per instance.
        let mut world = game().new_world();
        let o = world.spawn((Uid::new(), Transform::IDENTITY));
        let ouid = world.uid(o).unwrap();
        let mesh = world
            .resource_or_default::<Assets<MeshData>>()
            .add(MeshData::cuboid(Vec3::ONE));
        let mat = world
            .resource_or_default::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let renderer = MeshRenderer::new(mesh, mat);
        let parts = |shift: f32| -> Vec<Part> {
            (0..count)
                .map(|i| {
                    Part::new(PartKey::new(ouid, format!("instance{i}")))
                        .at(Transform::from_xyz(i as f32 * 0.01 + shift, 0.0, 0.0))
                        .with(renderer.clone())
                })
                .collect()
        };
        let p = parts(0.0);
        let t = Instant::now();
        reconcile(&mut world, o, "naive", p).unwrap();
        let naive_spawn = t.elapsed();
        let p = parts(0.0);
        let t = Instant::now();
        reconcile(&mut world, o, "naive", p).unwrap();
        let naive_same = t.elapsed();

        eprintln!(
            "scatter {n}: {count} instances in {chunk_count} chunks; spawn (settle) {:.1} ms \
             ({:.2} µs per instance); idle frame {idle_mean:.3} ms; stroke of 60 dabs at 60 Hz: \
             frame mean {frame_mean:.2} ms, max {frame_max:.2} ms, {} flushes (mean {flush_mean:.2} ms, \
             max {flush_max:.2} ms), {stroke_chunks} chunk placements; \
             naive reconcile (one part per instance): spawn {:.1} ms, unchanged pass {:.1} ms",
            ms(spawn),
            ms(spawn) * 1e3 / count as f64,
            flushes.len(),
            ms(naive_spawn),
            ms(naive_same),
        );
        // A stroke touches a few chunks per flush, never all of them.
        assert!(flushes.iter().all(|f| f.chunks_placed <= 6), "{flushes:?}");
        assert!(stroke_chunks < chunk_count as u64);
    }
}
