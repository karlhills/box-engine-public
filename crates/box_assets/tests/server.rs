//! Asset server behaviour: background loading, identity (rename, copies),
//! deletion, failed re-imports, dependencies, the app plugin.

mod common;

use std::time::{Duration, Instant};

use box_assets::{AssetError, AssetEvent, AssetEventKind, AssetPlugin, AssetServer, LoadState};
use box_core::app::App;
use box_core::asset_ref::{AssetId, AssetRef};
use box_core::assets::{AssetLookup, Assets, Handle};
use box_core::event::{EventCursor, Events};
use box_core::image::ImageData;
use box_core::material::StandardMaterial;
use box_core::mesh::MeshData;
use box_core::world::World;
use common::{TempAssets, settle};

fn r(path: &str) -> AssetRef {
    AssetRef::path(path)
}

fn events(world: &World) -> Vec<AssetEvent> {
    let mut cursor = EventCursor::<AssetEvent>::default();
    world
        .get_resource::<Events<AssetEvent>>()
        .map(|e| cursor.read(e).cloned().collect())
        .unwrap_or_default()
}

fn kinds_for<T: 'static>(world: &World, h: Handle<T>) -> Vec<AssetEventKind> {
    events(world)
        .into_iter()
        .filter(|e| e.is(h))
        .map(|e| e.kind)
        .collect()
}

#[test]
fn loads_in_the_background_without_blocking() {
    let assets = TempAssets::new("background");
    // Twelve copies of a 50k-triangle model: imports take a while.
    let big = common::grid_glb(50_000);
    for i in 0..12 {
        std::fs::write(assets.path(&format!("big/grid{i}.glb")), &big).unwrap_or_else(|_| {
            std::fs::create_dir_all(assets.path("big")).unwrap();
            std::fs::write(assets.path(&format!("big/grid{i}.glb")), &big).unwrap();
        });
    }
    let server = assets.server();
    let mut world = World::new();
    let start = Instant::now();
    let handles: Vec<Handle<MeshData>> = (0..12)
        .map(|i| {
            server
                .load(&r(&format!("big/grid{i}.glb#Mesh/Grid")))
                .unwrap()
        })
        .collect();
    let checker: Handle<ImageData> = server.load(&r("textures/checker.png")).unwrap();
    let load_time = start.elapsed();
    assert!(
        load_time < Duration::from_millis(100),
        "13 load calls took {load_time:?}"
    );
    // Same reference, same handle; the value is not there yet.
    assert_eq!(
        server
            .load::<MeshData>(&r("big/grid0.glb#Mesh/Grid"))
            .unwrap(),
        handles[0]
    );
    assert_eq!(server.handle_state(handles[11]), Some(LoadState::Loading));
    server.update(&mut world);
    settle(&server, &mut world);
    for h in &handles {
        assert_eq!(server.handle_state(*h), Some(LoadState::Loaded));
        assert_eq!(
            world
                .resource::<Assets<MeshData>>()
                .get(*h)
                .unwrap()
                .triangle_count(),
            50_562
        );
    }
    assert_eq!(server.handle_state(checker), Some(LoadState::Loaded));
    assert_eq!(kinds_for(&world, checker), [AssetEventKind::Loaded]);
    eprintln!(
        "13 loads queued in {load_time:?}; {} workers",
        server.worker_count()
    );
}

#[test]
fn a_sub_asset_load_imports_the_file_once() {
    let assets = TempAssets::new("once");
    let server = assets.server();
    let mut world = World::new();
    let roof: Handle<MeshData> = server.load(&r("models/tower.glb#Mesh/Roof")).unwrap();
    let stone: Handle<StandardMaterial> =
        server.load(&r("models/tower.glb#Material/Stone")).unwrap();
    let tex: Handle<ImageData> = server.load(&r("models/tower.glb#Texture/0")).unwrap();
    settle(&server, &mut world);
    let timings = server.take_import_timings();
    assert_eq!(timings.len(), 1, "{timings:?}");
    assert_eq!(timings[0].0, "models/tower.glb");
    for state in [
        server.handle_state(roof),
        server.handle_state(stone),
        server.handle_state(tex),
    ] {
        assert_eq!(state, Some(LoadState::Loaded));
    }
    // Every sub-asset got a slot, requested or not.
    assert_eq!(
        server
            .sub_assets(server.id_of("models/tower.glb").unwrap())
            .len(),
        8
    );
    assert!(
        server
            .get_handle::<MeshData>(&r("models/tower.glb#Mesh/Base"))
            .is_some()
    );
}

#[test]
fn rename_moves_file_and_meta_and_keeps_references() {
    let assets = TempAssets::new("rename");
    let server = assets.server();
    let mut world = World::new();
    let roof: Handle<MeshData> = server.load(&r("models/tower.glb#Mesh/Roof")).unwrap();
    settle(&server, &mut world);
    let id = server.id_of("models/tower.glb").unwrap();
    let by_id = AssetRef::id(id)
        .with_label("Mesh/Roof")
        .with_path("models/tower.glb");

    server
        .rename("models/tower.glb", "props/castle.glb")
        .unwrap();
    assert!(assets.path("props/castle.glb").is_file());
    assert!(assets.path("props/castle.glb.meta").is_file());
    assert!(!assets.path("models/tower.glb").exists());
    assert!(!assets.path("models/tower.glb.meta").exists());
    server.update(&mut world);
    let moved: Vec<_> = events(&world)
        .into_iter()
        .filter(|e| e.kind == AssetEventKind::Moved && e.is(roof))
        .collect();
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0].path, "props/castle.glb");
    assert_eq!(moved[0].old_path.as_deref(), Some("models/tower.glb"));

    // References by id still resolve (the path hint is stale), to the same
    // handle; the lookup follows the new path.
    assert_eq!(server.load::<MeshData>(&by_id).unwrap(), roof);
    let lookup = world.resource::<AssetLookup>();
    assert_eq!(lookup.get(&by_id), Some(roof));
    assert_eq!(lookup.get(&r("props/castle.glb#Mesh/Roof")), Some(roof));
    assert_eq!(
        lookup.get::<MeshData>(&r("models/tower.glb#Mesh/Roof")),
        None
    );
    // canonical_ref repairs the stale path, and fills the id of path-only
    // references.
    assert_eq!(
        server.canonical_ref(&by_id).unwrap().path.as_deref(),
        Some("props/castle.glb")
    );
    assert_eq!(
        server
            .canonical_ref(&r("props/castle.glb#Scene"))
            .unwrap()
            .id,
        Some(id)
    );
    assert_eq!(server.canonical_ref(&r("models/tower.glb")), None);
    // A reference with an unknown id but a valid path is repaired by path.
    let stale = AssetRef::id(AssetId::new())
        .with_label("Mesh/Roof")
        .with_path("props/castle.glb");
    assert_eq!(server.load::<MeshData>(&stale).unwrap(), roof);

    assert!(matches!(
        server.rename("props/castle.glb", "textures/checker.png"),
        Err(AssetError::AlreadyExists(_))
    ));
    assert!(matches!(
        server.rename("models/tower.glb", "x.glb"),
        Err(AssetError::NotFound(_))
    ));
    assert!(matches!(
        server.rename("props/castle.glb", "../x.glb"),
        Err(AssetError::OutsideAssets(_))
    ));
}

#[test]
fn external_rename_is_picked_up_by_rescan() {
    let assets = TempAssets::new("external_rename");
    let server = assets.server();
    let mut world = World::new();
    // A material that references the texture by id (and path hint).
    let mat: Handle<StandardMaterial> =
        server.load(&r("models/crate.gltf#Material/Crate")).unwrap();
    settle(&server, &mut world);
    let reference = world
        .resource::<Assets<StandardMaterial>>()
        .get(mat)
        .unwrap()
        .base_color_texture
        .clone();
    let tex: Handle<ImageData> = server.load(&reference).unwrap();
    settle(&server, &mut world);
    let id = server.id_of("models/crate_color.png").unwrap();
    assert_eq!(reference.id, Some(id));
    // Another tool moves the file and its .meta.
    std::fs::create_dir_all(assets.path("textures/crate")).unwrap();
    std::fs::rename(
        assets.path("models/crate_color.png"),
        assets.path("textures/crate/color.png"),
    )
    .unwrap();
    std::fs::rename(
        assets.path("models/crate_color.png.meta"),
        assets.path("textures/crate/color.png.meta"),
    )
    .unwrap();
    server.rescan();
    server.update(&mut world);
    assert_eq!(
        server.path_of(id).as_deref(),
        Some("textures/crate/color.png")
    );
    assert_eq!(kinds_for(&world, tex).last(), Some(&AssetEventKind::Moved));
    assert_eq!(server.handle_state(tex), Some(LoadState::Loaded));
    let lookup = world.resource::<AssetLookup>();
    assert_eq!(lookup.get(&r("textures/crate/color.png")), Some(tex));
    // The stored reference (id plus stale path) still resolves.
    assert_eq!(lookup.get(&reference), Some(tex));
    assert_eq!(server.load::<ImageData>(&reference).unwrap(), tex);
    assert_eq!(
        server.canonical_ref(&reference).unwrap().path.as_deref(),
        Some("textures/crate/color.png")
    );
    // Content unchanged: no re-import.
    assert!(server.is_idle());
}

#[test]
fn copied_file_gets_a_fresh_id() {
    let assets = TempAssets::new("duplicate");
    std::thread::sleep(Duration::from_millis(20));
    // A copy made by a tool that knows nothing about .meta files.
    std::fs::write(
        assets.path("textures/copy.png"),
        std::fs::read(assets.path("textures/checker.png")).unwrap(),
    )
    .unwrap();
    std::fs::write(
        assets.path("textures/copy.png.meta"),
        std::fs::read(assets.path("textures/checker.png.meta")).unwrap(),
    )
    .unwrap();
    let server = assets.server();
    let original = AssetId::parse("10000000-0000-4000-8000-000000000001").unwrap();
    assert_eq!(
        server.id_of("textures/checker.png"),
        Some(original),
        "the older file keeps its id"
    );
    let copy = server.id_of("textures/copy.png").unwrap();
    assert_ne!(copy, original);
    let meta = std::fs::read_to_string(assets.path("textures/copy.png.meta")).unwrap();
    assert!(
        meta.contains(&copy.to_string()),
        "the copy's .meta was given the new id"
    );

    // The same while running.
    std::fs::write(
        assets.path("textures/copy2.png"),
        std::fs::read(assets.path("textures/checker.png")).unwrap(),
    )
    .unwrap();
    std::fs::write(
        assets.path("textures/copy2.png.meta"),
        std::fs::read(assets.path("textures/checker.png.meta")).unwrap(),
    )
    .unwrap();
    server.rescan();
    assert_eq!(server.id_of("textures/checker.png"), Some(original));
    let copy2 = server.id_of("textures/copy2.png").unwrap();
    assert!(copy2 != original && copy2 != copy);
}

#[test]
fn deleted_file_keeps_its_slot_with_the_placeholder() {
    let assets = TempAssets::new("delete");
    let server = assets.server();
    let mut world = World::new();
    let tex: Handle<ImageData> = server.load(&r("textures/checker.png")).unwrap();
    settle(&server, &mut world);
    let bytes = std::fs::read(assets.path("textures/checker.png")).unwrap();
    std::fs::remove_file(assets.path("textures/checker.png")).unwrap();
    server.rescan();
    server.update(&mut world);
    assert!(
        matches!(server.handle_state(tex), Some(LoadState::Failed(m)) if m.contains("deleted"))
    );
    assert!(server.is_placeholder(tex));
    assert_eq!(
        kinds_for(&world, tex),
        [AssetEventKind::Loaded, AssetEventKind::Removed]
    );
    assert_eq!(
        *world.resource::<Assets<ImageData>>().get(tex).unwrap(),
        box_assets::placeholder::placeholder_image()
    );
    // Still resolvable by id, to the same slot.
    let id = AssetId::parse("10000000-0000-4000-8000-000000000001").unwrap();
    assert_eq!(server.load::<ImageData>(&AssetRef::id(id)).unwrap(), tex);
    // The file comes back (its .meta was left in place).
    std::fs::write(assets.path("textures/checker.png"), bytes).unwrap();
    server.rescan();
    settle(&server, &mut world);
    assert_eq!(server.handle_state(tex), Some(LoadState::Loaded));
    assert!(!server.is_placeholder(tex));
    assert_eq!(
        world
            .resource::<Assets<ImageData>>()
            .get(tex)
            .unwrap()
            .width,
        4
    );
}

#[test]
fn failed_reimport_keeps_the_last_good_value() {
    let assets = TempAssets::new("bad_reload");
    let server = assets.server();
    let mut world = World::new();
    let tex: Handle<ImageData> = server.load(&r("textures/checker.png")).unwrap();
    settle(&server, &mut world);
    let good = world
        .resource::<Assets<ImageData>>()
        .get(tex)
        .unwrap()
        .clone();
    std::fs::write(assets.path("textures/checker.png"), b"not a png").unwrap();
    server.rescan();
    settle(&server, &mut world);
    assert!(matches!(
        server.handle_state(tex),
        Some(LoadState::Failed(_))
    ));
    assert!(!server.is_placeholder(tex));
    assert_eq!(
        *world.resource::<Assets<ImageData>>().get(tex).unwrap(),
        good
    );
    assert_eq!(kinds_for(&world, tex).last(), Some(&AssetEventKind::Failed));
}

#[test]
fn changing_a_dependency_reimports_the_gltf() {
    let assets = TempAssets::new("deps");
    let server = assets.server();
    let mut world = World::new();
    let mesh: Handle<MeshData> = server.load(&r("models/crate.gltf#Mesh/Crate")).unwrap();
    settle(&server, &mut world);
    // Move the first vertex: positions are the first 12 bytes of crate.bin.
    let mut bin = std::fs::read(assets.path("models/crate.bin")).unwrap();
    bin[0..4].copy_from_slice(&(-2.0f32).to_le_bytes());
    std::fs::write(assets.path("models/crate.bin"), bin).unwrap();
    server.rescan();
    settle(&server, &mut world);
    assert_eq!(
        world
            .resource::<Assets<MeshData>>()
            .get(mesh)
            .unwrap()
            .positions[0]
            .x,
        -2.0
    );
    assert_eq!(
        kinds_for(&world, mesh),
        [AssetEventKind::Loaded, AssetEventKind::Reloaded]
    );
}

#[test]
fn reimport_and_versions() {
    let assets = TempAssets::new("reimport");
    let server = assets.server();
    let mut world = World::new();
    let tex: Handle<ImageData> = server.load(&r("textures/checker.png")).unwrap();
    settle(&server, &mut world);
    let v1 = world.resource::<Assets<ImageData>>().version(tex).unwrap();
    server
        .reimport(server.id_of("textures/checker.png").unwrap())
        .unwrap();
    settle(&server, &mut world);
    assert!(world.resource::<Assets<ImageData>>().version(tex).unwrap() > v1);
    assert!(matches!(
        server.reimport(AssetId::new()),
        Err(AssetError::NotFound(_))
    ));
}

#[test]
fn asset_plugin_updates_every_frame() {
    let assets = TempAssets::new("plugin");
    let server = assets.server();
    let mut app = App::new();
    app.add_plugin(&AssetPlugin::new(server));
    let h: Handle<StandardMaterial> = app
        .world
        .resource::<AssetServer>()
        .clone()
        .load(&r("materials/stone.material.json"))
        .unwrap();
    let mut cursor = EventCursor::<AssetEvent>::default();
    let mut loaded = false;
    let start = Instant::now();
    while !loaded && start.elapsed() < Duration::from_secs(10) {
        app.update();
        loaded = cursor
            .read(app.world.resource::<Events<AssetEvent>>())
            .any(|e| e.is(h) && e.kind == AssetEventKind::Loaded);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(loaded);
    assert_eq!(
        app.world
            .resource::<Assets<StandardMaterial>>()
            .get(h)
            .unwrap()
            .roughness,
        0.8
    );
}

#[test]
fn set_settings_rewrites_the_meta_and_reimports() {
    let assets = TempAssets::new("set_settings");
    let server = assets.server();
    let mut world = World::new();
    let tex: Handle<ImageData> = server.load(&r("textures/checker.png")).unwrap();
    settle(&server, &mut world);
    let id = server.id_of("textures/checker.png").unwrap();
    let images = |w: &World| w.resource::<Assets<ImageData>>().get(tex).cloned().unwrap();
    let v1 = world.resource::<Assets<ImageData>>().version(tex).unwrap();
    assert!(images(&world).mips.len() > 1);
    let generation = server.index_generation();

    server
        .set_settings(
            id,
            serde_json::json!({"color_space": "linear", "mipmaps": false}),
        )
        .unwrap();
    assert_eq!(server.file_state(id), Some(LoadState::Loading));
    settle(&server, &mut world);
    let image = images(&world);
    assert_eq!(image.mips.len(), 1);
    assert_eq!(image.color_space, box_core::image::ColorSpace::Linear);
    assert!(world.resource::<Assets<ImageData>>().version(tex).unwrap() > v1);
    assert_eq!(server.handle_state(tex), Some(LoadState::Loaded));
    let meta = server.meta(id).unwrap();
    assert_eq!((meta.id, meta.importer.as_str()), (id, "texture"));
    assert_eq!(meta.settings["mipmaps"], serde_json::json!(false));
    // Settings do not change the index.
    assert_eq!(server.index_generation(), generation);

    // Wrongly typed or non-object settings are refused; the file is kept.
    assert!(matches!(
        server.set_settings(id, serde_json::json!({"mipmaps": "yes"})),
        Err(AssetError::BadMeta { .. })
    ));
    assert!(server.set_settings(id, serde_json::json!([1])).is_err());
    let gltf = server.id_of("models/tower.glb").unwrap();
    assert!(
        server
            .set_settings(gltf, serde_json::json!({"scale": 0.0}))
            .is_err()
    );
    assert_eq!(
        server.meta(id).unwrap().settings["mipmaps"],
        serde_json::json!(false)
    );
}

#[test]
fn write_and_create_go_through_the_index() {
    let assets = TempAssets::new("write_create");
    let server = assets.server();
    let mut world = World::new();
    let generation = server.index_generation();
    let id = server
        .create("materials/new.material.json", b"{\"roughness\": 0.25}")
        .unwrap();
    assert!(server.index_generation() > generation);
    assert_eq!(
        server.path_of(id).as_deref(),
        Some("materials/new.material.json")
    );
    assert!(assets.path("materials/new.material.json.meta").is_file());
    assert!(matches!(
        server.create("materials/new.material.json", b"{}"),
        Err(AssetError::AlreadyExists(_))
    ));
    let h: Handle<StandardMaterial> = server.load(&AssetRef::id(id)).unwrap();
    settle(&server, &mut world);
    let roughness = |w: &World| {
        w.resource::<Assets<StandardMaterial>>()
            .get(h)
            .unwrap()
            .roughness
    };
    assert_eq!(roughness(&world), 0.25);

    server.write(id, b"{\"roughness\": 0.75}").unwrap();
    settle(&server, &mut world);
    assert_eq!(roughness(&world), 0.75);
    assert!(kinds_for(&world, h).contains(&AssetEventKind::Reloaded));
    // No temporary file is left behind.
    let leftovers = std::fs::read_dir(assets.path("materials"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .count();
    assert_eq!(leftovers, 0);
    assert!(
        server
            .broken_files()
            .iter()
            .any(|(p, _)| p == "bad/bad_meta.png")
    );
}
