//! The asset server on a mounted pak: same imports as from the folder, no
//! `.meta` writes, no watching, writes refused.

mod common;

use box_assets::{AssetError, AssetServer, LoadState, PakWriter, vfs};
use box_core::asset_ref::AssetRef;
use box_core::assets::{Assets, Handle};
use box_core::image::ImageData;
use box_core::material::StandardMaterial;
use box_core::mesh::MeshData;
use box_core::world::World;
use common::{TempAssets, settle};

/// The fixture corpus (good files only) packed as `assets/**` of a pak.
fn pack(assets: &TempAssets, out: &std::path::Path) {
    let mut writer = PakWriter::new();
    let mut stack = vec![assets.dir.clone()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let p = e.path();
            let rel = p
                .strip_prefix(&assets.dir)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if rel.starts_with("bad") {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
            } else {
                writer.add_file(&format!("assets/{rel}"), p).unwrap();
            }
        }
    }
    writer.write(out).unwrap();
}

#[test]
fn serves_imports_from_a_mounted_pak() {
    let assets = TempAssets::new("pak");
    // Folder server first: creates the missing .meta files the pak needs.
    let disk = assets.server();
    let files_on_disk = disk.files();
    drop(disk);
    let out_dir = std::env::temp_dir().join(format!("box_assets_pak_out_{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let pak_path = out_dir.join("game.pak");
    pack(&assets, &pak_path);
    let root = vfs::mount_file(&pak_path).unwrap();

    let server = AssetServer::new(root.join("assets")).unwrap();
    assert!(server.is_read_only());
    let good: Vec<_> = files_on_disk
        .iter()
        .filter(|(_, p)| !p.starts_with("bad"))
        .cloned()
        .collect();
    assert_eq!(server.files(), good, "same ids and paths as the folder");

    let mut world = World::new();
    let image: Handle<ImageData> = server
        .load(&AssetRef::path("textures/checker.png"))
        .unwrap();
    let mesh: Handle<MeshData> = server
        .load(&AssetRef::path("models/crate.gltf#Mesh/Crate"))
        .unwrap();
    let material: Handle<StandardMaterial> = server
        .load(&AssetRef::path("materials/stone.material.json"))
        .unwrap();
    settle(&server, &mut world);
    for (what, state) in [
        ("image", server.handle_state(image)),
        ("mesh", server.handle_state(mesh)),
        ("material", server.handle_state(material)),
    ] {
        assert_eq!(state, Some(LoadState::Loaded), "{what}");
    }
    assert!(world.resource::<Assets<ImageData>>().get(image).is_some());
    // The glTF's external buffer and images came from the pak too.
    assert!(
        world
            .resource::<Assets<MeshData>>()
            .get(mesh)
            .unwrap()
            .vertex_count()
            > 0
    );

    assert!(matches!(server.watch(true), Err(AssetError::ReadOnly(_))));
    assert!(matches!(
        server.create("x.material.json", b"{}"),
        Err(AssetError::ReadOnly(_))
    ));
    let id = server.id_of("textures/checker.png").unwrap();
    assert!(matches!(
        server.write(id, b"x"),
        Err(AssetError::ReadOnly(_))
    ));
    assert!(matches!(
        server.rename("textures/checker.png", "x.png"),
        Err(AssetError::ReadOnly(_))
    ));
    assert!(server.file_exists("models/crate.bin"));
    assert_eq!(
        server.read_file("models/crate.bin").unwrap(),
        std::fs::read(assets.path("models/crate.bin")).unwrap()
    );
    assert!(!server.file_exists("nope.bin"));

    vfs::unmount(&root);
    std::fs::remove_dir_all(&out_dir).unwrap();
}

#[test]
fn a_pak_file_without_meta_files_is_reported() {
    let out_dir =
        std::env::temp_dir().join(format!("box_assets_pak_nometa_{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let pak_path = out_dir.join("game.pak");
    let mut writer = PakWriter::new();
    writer
        .add_file(
            "assets/checker.png",
            common::fixtures_dir().join("textures/checker.png"),
        )
        .unwrap();
    writer.write(&pak_path).unwrap();
    let root = vfs::mount_file(&pak_path).unwrap();
    let server = AssetServer::new(root.join("assets")).unwrap();
    assert!(server.files().is_empty());
    let broken = server.broken_files();
    assert_eq!(broken.len(), 1);
    assert!(broken[0].1.contains("missing"), "{broken:?}");
    vfs::unmount(&root);
    std::fs::remove_dir_all(&out_dir).unwrap();
}
