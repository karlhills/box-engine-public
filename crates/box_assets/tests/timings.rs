//! Import timings of the fixtures and of a generated 100k-triangle GLB.
//! `cargo test --release -p box_assets --test timings -- --nocapture`
//! prints them (docs/measurements/ records the numbers).

mod common;

use std::time::{Duration, Instant};

use box_assets::LoadState;
use box_core::asset_ref::AssetRef;
use box_core::assets::{Assets, Handle};
use box_core::mesh::MeshData;
use box_core::world::World;
use common::{TempAssets, settle};

#[test]
fn import_timings() {
    let assets = TempAssets::new("timings");
    std::fs::write(
        assets.path("models/grid100k.glb"),
        common::grid_glb(100_000),
    )
    .unwrap();
    let server = assets.server();
    let mut world = World::new();
    let good = [
        "textures/checker.png",
        "textures/photo.jpg",
        "models/tower.glb",
        "models/crate.gltf",
        "materials/stone.material.json",
        "scenes/level.scene.json",
    ];
    for path in good {
        server.load_file(&AssetRef::path(path)).unwrap();
    }
    settle(&server, &mut world);
    let mut report: Vec<(String, Duration)> = server.take_import_timings();

    let start = Instant::now();
    let grid: Handle<MeshData> = server
        .load(&AssetRef::path("models/grid100k.glb#Mesh/Grid"))
        .unwrap();
    settle(&server, &mut world);
    let wall = start.elapsed();
    assert_eq!(server.handle_state(grid), Some(LoadState::Loaded));
    let mesh = world.resource::<Assets<MeshData>>().get(grid).unwrap();
    assert!(mesh.triangle_count() >= 100_000);
    assert_eq!(
        mesh.tangents.len(),
        mesh.vertex_count(),
        "tangents generated"
    );
    let (triangles, vertices) = (mesh.triangle_count(), mesh.vertex_count());
    report.extend(server.take_import_timings());

    // The same file without tangent generation.
    let id = server.id_of("models/grid100k.glb").unwrap();
    let meta = std::fs::read_to_string(assets.path("models/grid100k.glb.meta")).unwrap();
    std::fs::write(
        assets.path("models/grid100k.glb.meta"),
        meta.replace(
            "\"generate_tangents\": true",
            "\"generate_tangents\": false",
        ),
    )
    .unwrap();
    server.reimport(id).unwrap();
    settle(&server, &mut world);
    assert!(
        world
            .resource::<Assets<MeshData>>()
            .get(grid)
            .unwrap()
            .tangents
            .is_empty()
    );
    let no_tangents = server.take_import_timings();
    report.extend(
        no_tangents
            .into_iter()
            .map(|(p, t)| (format!("{p} (no tangents)"), t)),
    );
    report.sort();
    for (path, t) in &report {
        eprintln!("{path:40} {:8.2} ms", t.as_secs_f64() * 1000.0);
    }
    eprintln!(
        "grid100k.glb: {} triangles, {} vertices; load to Loaded {:.2} ms wall",
        triangles,
        vertices,
        wall.as_secs_f64() * 1000.0
    );
}
