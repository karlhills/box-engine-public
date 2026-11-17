//! Importer tests on the fixture corpus, with insta snapshots of the
//! imported structure.

mod common;

use std::sync::Arc;

use box_assets::{AssetError, AssetServer, LoadState, SceneAsset};
use box_core::asset_ref::{AssetId, AssetRef};
use box_core::assets::{AssetLookup, Assets, Handle};
use box_core::image::{ColorSpace, ImageData};
use box_core::material::StandardMaterial;
use box_core::mesh::MeshData;
use box_core::mesh_asset::MeshAsset;
use box_core::reflect::Registry;
use box_core::world::World;
use common::{TempAssets, settle};
use serde_json::{Value, json};

fn r(path: &str) -> AssetRef {
    AssetRef::path(path)
}

fn round(v: f32) -> f64 {
    (f64::from(v) * 1000.0).round() / 1000.0
}

fn short(type_name: &str) -> &str {
    type_name.rsplit("::").next().unwrap_or(type_name)
}

fn mesh_summary(m: &MeshData) -> Value {
    for t in &m.tangents {
        assert!(
            (t.truncate().length() - 1.0).abs() < 1e-3,
            "unit tangent {t}"
        );
        assert!(t.w == 1.0 || t.w == -1.0, "handedness {t}");
    }
    m.validate().unwrap();
    json!({
        "vertices": m.vertex_count(),
        "triangles": m.triangle_count(),
        "uv_sets": m.uvs.len(),
        "tangents": !m.tangents.is_empty(),
        "colors": !m.colors.is_empty(),
        "groups": m.groups.iter().map(|g| json!([g.first_index, g.index_count, g.material_slot])).collect::<Vec<_>>(),
        "bounds": {
            "min": m.bounds().min.to_array().map(round),
            "max": m.bounds().max.to_array().map(round),
        },
    })
}

fn material_summary(m: &StandardMaterial) -> Value {
    json!({
        "base_color": m.base_color.to_array().map(round),
        "roughness": round(m.roughness),
        "metallic": round(m.metallic),
        "emissive": m.emissive.to_array().map(round),
        "base_color_texture": m.base_color_texture.to_string(),
        "normal_texture": m.normal_texture.to_string(),
        "orm_texture": m.orm_texture.to_string(),
        "emissive_texture": m.emissive_texture.to_string(),
        "alpha_mode": format!("{:?}", m.alpha_mode),
        "double_sided": m.double_sided,
    })
}

fn image_summary(i: &ImageData) -> Value {
    json!({
        "size": [i.width, i.height],
        "color_space": format!("{:?}", i.color_space),
        "mips": i.mips.len(),
        "first_pixel": &i.mips[0][..4],
    })
}

/// Every sub-asset of a file with its imported content.
fn summarize(server: &AssetServer, world: &World, path: &str) -> Value {
    let id = server.id_of(path).unwrap();
    let mut out = serde_json::Map::new();
    for sub in server.sub_assets(id) {
        let label = sub.label.clone().unwrap_or_default();
        assert_eq!(sub.state, LoadState::Loaded, "{path}#{label}");
        let rf = AssetRef::id(id);
        let rf = match &sub.label {
            Some(l) => rf.with_label(l),
            None => rf,
        };
        let value = match short(sub.type_name) {
            "MeshData" => {
                let h = server.get_handle::<MeshData>(&rf).unwrap();
                mesh_summary(world.resource::<Assets<MeshData>>().get(h).unwrap())
            }
            "StandardMaterial" => {
                let h = server.get_handle::<StandardMaterial>(&rf).unwrap();
                material_summary(world.resource::<Assets<StandardMaterial>>().get(h).unwrap())
            }
            "ImageData" => {
                let h = server.get_handle::<ImageData>(&rf).unwrap();
                image_summary(world.resource::<Assets<ImageData>>().get(h).unwrap())
            }
            "SceneAsset" => {
                let h = server.get_handle::<SceneAsset>(&rf).unwrap();
                world
                    .resource::<Assets<SceneAsset>>()
                    .get(h)
                    .unwrap()
                    .json
                    .clone()
            }
            other => json!(other),
        };
        out.insert(format!("{label} ({})", short(sub.type_name)), value);
    }
    Value::Object(out)
}

#[test]
fn tower_glb_imports_meshes_materials_textures_and_scene() {
    let assets = TempAssets::new("tower");
    let server = assets.server();
    let mut world = World::new();
    server.load_file(&r("models/tower.glb")).unwrap();
    settle(&server, &mut world);
    insta::assert_json_snapshot!("tower_glb", summarize(&server, &world, "models/tower.glb"));

    // Sub-assets are registered in the world's AssetLookup by id and path.
    let lookup = world.resource::<AssetLookup>();
    let roof: Handle<MeshData> = lookup.get(&r("models/tower.glb#Mesh/Roof")).unwrap();
    let id = AssetId::parse("10000000-0000-4000-8000-000000000002").unwrap();
    assert_eq!(
        lookup.get(&AssetRef::id(id).with_label("Mesh/Roof")),
        Some(roof)
    );
    // Loading a sub-asset after the import returns the existing slot.
    assert_eq!(
        server
            .load::<MeshData>(&r("models/tower.glb#Mesh/Roof"))
            .unwrap(),
        roof
    );
}

#[test]
fn imported_scene_instantiates_with_core_scene_code() {
    let assets = TempAssets::new("tower_scene");
    let server = assets.server();
    let mut world = World::new();
    let scene: Handle<SceneAsset> = server.load(&r("models/tower.glb#Scene")).unwrap();
    settle(&server, &mut world);
    let scene = world
        .resource::<Assets<SceneAsset>>()
        .get(scene)
        .unwrap()
        .clone();
    let loaded =
        box_core::scene::load_value(&scene.json, Arc::new(Registry::with_core_types())).unwrap();
    assert_eq!(loaded.entity_count(), 5); // file root + Tower, Base, Roof, Flag
    let meshes: Vec<MeshAsset> = loaded.query::<&MeshAsset>().iter().cloned().collect();
    assert_eq!(meshes.len(), 2);
    // Every reference in the scene resolves through the lookup.
    let lookup = world.resource::<AssetLookup>();
    for m in &meshes {
        assert!(lookup.get::<MeshData>(&m.mesh).is_some(), "{}", m.mesh);
        for mat in &m.materials {
            assert!(lookup.get::<StandardMaterial>(mat).is_some(), "{mat}");
        }
    }
    // As a prefab: fresh uids, instantiated twice into one world.
    let mut target = World::with_registry(Arc::new(Registry::with_core_types()));
    for _ in 0..2 {
        let mut records = scene.entities().to_vec();
        box_core::scene::remap_uids(&mut records);
        box_core::scene::instantiate(&mut target, &records, None).unwrap();
    }
    assert_eq!(target.entity_count(), 10);
}

#[test]
fn gltf_with_external_buffer_and_textures() {
    let assets = TempAssets::new("crate");
    let server = assets.server();
    let mut world = World::new();
    server.load_file(&r("models/crate.gltf")).unwrap();
    settle(&server, &mut world);
    insta::assert_json_snapshot!(
        "crate_gltf",
        summarize(&server, &world, "models/crate.gltf")
    );
    let mat: Handle<StandardMaterial> =
        server.load(&r("models/crate.gltf#Material/Crate")).unwrap();
    let mat = world
        .resource::<Assets<StandardMaterial>>()
        .get(mat)
        .unwrap()
        .clone();
    // External textures are references to the image files, by id.
    assert_eq!(
        mat.base_color_texture.id,
        server.id_of("models/crate_color.png")
    );
    assert_eq!(mat.orm_texture.id, server.id_of("models/crate_mr.png"));
    let orm: Handle<ImageData> = server.load(&mat.orm_texture).unwrap();
    settle(&server, &mut world);
    let orm = world.resource::<Assets<ImageData>>().get(orm).unwrap();
    assert_eq!(
        orm.color_space,
        ColorSpace::Linear,
        "from crate_mr.png.meta"
    );
}

/// An external ORM image without a `.meta`: the index creates one with the
/// default sRGB, and the glTF import switches it to linear.
#[test]
fn gltf_sets_new_metas_of_data_maps_to_linear() {
    let assets = TempAssets::new("orm_meta");
    std::fs::remove_file(assets.path("models/crate_mr.png.meta")).unwrap();
    std::fs::remove_file(assets.path("models/crate_color.png.meta")).unwrap();
    let server = assets.server();
    let mut world = World::new();
    let _: Handle<StandardMaterial> = server.load(&r("models/crate.gltf#Material/Crate")).unwrap();
    settle(&server, &mut world);
    let meta = |name: &str| {
        let text = std::fs::read_to_string(assets.path(&format!("models/{name}.meta"))).unwrap();
        serde_json::from_str::<serde_json::Value>(&text).unwrap()["settings"]["color_space"].clone()
    };
    assert_eq!(meta("crate_mr.png"), "linear");
    assert_eq!(meta("crate_color.png"), "srgb", "colour maps stay sRGB");
    // A later import of the ORM image reads the new setting.
    let orm: Handle<ImageData> = server.load(&r("models/crate_mr.png")).unwrap();
    settle(&server, &mut world);
    let orm = world.resource::<Assets<ImageData>>().get(orm).unwrap();
    assert_eq!(orm.color_space, ColorSpace::Linear);
}

#[test]
fn material_and_scene_files() {
    let assets = TempAssets::new("data");
    let server = assets.server();
    let mut world = World::new();
    let mat: Handle<StandardMaterial> = server.load(&r("materials/stone.material.json")).unwrap();
    let scene: Handle<SceneAsset> = server.load(&r("scenes/level.scene.json")).unwrap();
    settle(&server, &mut world);
    let mat = world
        .resource::<Assets<StandardMaterial>>()
        .get(mat)
        .unwrap();
    insta::assert_json_snapshot!("stone_material", material_summary(mat));
    assert_eq!(
        mat.base_color_texture.id,
        server.id_of("textures/checker.png"),
        "path-only texture reference gets the texture's id"
    );
    let scene = world.resource::<Assets<SceneAsset>>().get(scene).unwrap();
    assert_eq!(scene.entities().len(), 1);
    assert!(
        box_core::scene::load_value(&scene.json, Arc::new(Registry::with_core_types())).is_ok()
    );
}

#[test]
fn textures_follow_meta_settings() {
    let assets = TempAssets::new("textures");
    let server = assets.server();
    let mut world = World::new();
    // photo.jpg had no .meta: the server created one with the defaults.
    let meta = std::fs::read_to_string(assets.path("textures/photo.jpg.meta")).unwrap();
    let meta: Value = serde_json::from_str(&meta).unwrap();
    assert_eq!(meta["importer"], "texture");
    assert_eq!(
        meta["settings"],
        json!({"color_space": "srgb", "mipmaps": true})
    );

    let checker: Handle<ImageData> = server.load(&r("textures/checker.png")).unwrap();
    let photo: Handle<ImageData> = server.load(&r("textures/photo.jpg")).unwrap();
    settle(&server, &mut world);
    let images = world.resource::<Assets<ImageData>>();
    let c = images.get(checker).unwrap();
    assert_eq!(
        (c.width, c.height, c.mips.len(), c.color_space),
        (4, 4, 3, ColorSpace::Srgb)
    );
    assert_eq!(&c.mips[0][..4], &[200, 40, 40, 255]);
    let p = images.get(photo).unwrap();
    assert_eq!((p.width, p.height, p.mips.len()), (8, 8, 4));

    // A settings change (partial settings: the rest default) re-imports.
    std::fs::write(
        assets.path("textures/checker.png.meta"),
        r#"{"version": 1, "id": "10000000-0000-4000-8000-000000000001", "importer": "texture",
            "settings": {"color_space": "linear", "mipmaps": false, "compress": "bc7"}}"#,
    )
    .unwrap();
    server.rescan();
    settle(&server, &mut world);
    let c = world.resource::<Assets<ImageData>>().get(checker).unwrap();
    assert_eq!((c.mips.len(), c.color_space), (1, ColorSpace::Linear));
}

/// Loads `path` as `T`, waits, and returns the failure message.
fn failure<T: Send + 'static>(server: &AssetServer, world: &mut World, path: &str) -> String {
    let h: Handle<T> = server.load(&r(path)).unwrap();
    settle(server, world);
    match server.handle_state(h) {
        Some(LoadState::Failed(m)) => m,
        other => panic!("{path}: expected Failed, got {other:?}"),
    }
}

#[test]
fn malformed_files_fail_with_clear_errors_and_placeholders() {
    let assets = TempAssets::new("bad");
    let server = assets.server();
    let mut world = World::new();
    let messages = json!({
        "truncated.png": failure::<ImageData>(&server, &mut world, "bad/truncated.png"),
        "garbage.glb": failure::<SceneAsset>(&server, &mut world, "bad/garbage.glb#Scene"),
        "empty.glb": failure::<SceneAsset>(&server, &mut world, "bad/empty.glb#Scene"),
        "bad_buffer.gltf": failure::<MeshData>(&server, &mut world, "bad/bad_buffer.gltf#Mesh/0"),
        "draco.gltf": failure::<MeshData>(&server, &mut world, "bad/draco.gltf#Mesh/0"),
        "wrong.material.json": failure::<StandardMaterial>(&server, &mut world, "bad/wrong.material.json"),
        "tower.glb (no main asset)": failure::<MeshData>(&server, &mut world, "models/tower.glb"),
        "tower.glb#Mesh/Nope": failure::<MeshData>(&server, &mut world, "models/tower.glb#Mesh/Nope"),
    });
    insta::assert_json_snapshot!("malformed_messages", messages);
    assert!(messages["draco.gltf"].as_str().unwrap().contains("Draco"));

    // Placeholders are stored and flagged.
    let png: Handle<ImageData> = server.load(&r("bad/truncated.png")).unwrap();
    assert!(server.is_placeholder(png));
    let img = world.resource::<Assets<ImageData>>().get(png).unwrap();
    assert_eq!(*img, box_assets::placeholder::placeholder_image());
    let mesh: Handle<MeshData> = server.load(&r("bad/bad_buffer.gltf#Mesh/0")).unwrap();
    assert_eq!(
        *world.resource::<Assets<MeshData>>().get(mesh).unwrap(),
        box_assets::placeholder::placeholder_mesh()
    );
    let mat: Handle<StandardMaterial> = server.load(&r("bad/wrong.material.json")).unwrap();
    assert_eq!(
        *world
            .resource::<Assets<StandardMaterial>>()
            .get(mat)
            .unwrap(),
        box_assets::placeholder::placeholder_material()
    );
    // Placeholders resolve through the lookup, so references never dangle.
    assert_eq!(
        world.resource::<AssetLookup>().get(&r("bad/truncated.png")),
        Some(png)
    );
}

#[test]
fn reference_errors() {
    let assets = TempAssets::new("refs");
    let server = assets.server();
    let mut world = World::new();
    let bad_meta = std::fs::read(assets.path("bad/bad_meta.png.meta")).unwrap();
    assert!(matches!(
        server.load::<ImageData>(&r("bad/bad_meta.png")),
        Err(AssetError::BadMeta { .. })
    ));
    assert_eq!(
        std::fs::read(assets.path("bad/bad_meta.png.meta")).unwrap(),
        bad_meta,
        "a broken .meta is never overwritten"
    );
    assert!(matches!(
        server.load::<ImageData>(&r("textures/none.png")),
        Err(AssetError::NotFound(_))
    ));
    assert!(matches!(
        server.load::<ImageData>(&r("../outside.png")),
        Err(AssetError::OutsideAssets(_))
    ));
    assert!(matches!(
        server.load::<ImageData>(&r("models/crate.bin")),
        Err(AssetError::NoImporter(_))
    ));
    assert!(matches!(
        server.load::<u32>(&r("textures/checker.png")),
        Err(AssetError::UnregisteredType(_))
    ));
    let _: Handle<ImageData> = server.load(&r("textures/checker.png")).unwrap();
    assert!(matches!(
        server.load::<MeshData>(&r("textures/checker.png")),
        Err(AssetError::TypeMismatch { .. })
    ));

    // A glTF whose external texture is missing still imports; the material
    // keeps the path, which fails to load on its own.
    let mat: Handle<StandardMaterial> = server
        .load(&r("bad/missing_texture.gltf#Material/Ghost"))
        .unwrap();
    settle(&server, &mut world);
    assert_eq!(server.handle_state(mat), Some(LoadState::Loaded));
    let tex = world
        .resource::<Assets<StandardMaterial>>()
        .get(mat)
        .unwrap()
        .base_color_texture
        .clone();
    assert_eq!((tex.id, tex.path.as_deref()), (None, Some("bad/nope.png")));
    assert!(matches!(
        server.load::<ImageData>(&tex),
        Err(AssetError::NotFound(_))
    ));
}

/// Truncated and corrupted variants of the good fixtures: every import ends
/// as Loaded or Failed, and no importer panics (a caught panic would show up
/// as "panicked" in the message).
#[test]
fn corrupted_files_never_panic() {
    let assets = TempAssets::new("fuzz");
    let sources = [
        ("models/tower.glb", "glb"),
        ("models/crate.gltf", "gltf"),
        ("textures/checker.png", "png"),
        ("textures/photo.jpg", "jpg"),
        ("materials/stone.material.json", "material.json"),
        ("scenes/level.scene.json", "scene.json"),
    ];
    std::fs::create_dir_all(assets.path("fuzz")).unwrap();
    let mut paths = Vec::new();
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for (src, ext) in sources {
        let bytes = std::fs::read(assets.path(src)).unwrap();
        for i in 0..24 {
            let mut b = bytes.clone();
            if i % 2 == 0 {
                b.truncate((next() as usize) % b.len());
            } else {
                for _ in 0..1 + i / 4 {
                    let at = (next() as usize) % b.len();
                    b[at] = next() as u8;
                }
            }
            let path = format!("fuzz/{}_{i}.{ext}", ext.replace('.', "_"));
            std::fs::write(assets.path(&path), b).unwrap();
            paths.push(path);
        }
    }
    let server = assets.server();
    let mut world = World::new();
    for p in &paths {
        server.load_file(&r(p)).unwrap();
    }
    settle(&server, &mut world);
    let (mut loaded, mut failed) = (0, 0);
    for p in &paths {
        let id = server.id_of(p).unwrap();
        let file = server.file_state(id).unwrap();
        let subs = server.sub_assets(id).into_iter().map(|s| s.state);
        for state in std::iter::once(file).chain(subs) {
            match state {
                LoadState::Loaded => loaded += 1,
                LoadState::Failed(m) => {
                    failed += 1;
                    assert!(!m.contains("panicked"), "{p}: {m}");
                }
                LoadState::Loading => panic!("{p} still loading"),
            }
        }
    }
    assert!(failed > 0 && loaded > 0, "{loaded} loaded, {failed} failed");
}
