//! The editor's asset side (M4b), headless: the browser listing, rename and
//! move with reference repair, files dropped from the OS, drops into the
//! scene, material assignment, import settings, the material inspector and
//! shader headers; plus captures of the browser, the asset picker, import
//! settings and the material inspector (skipped without a GPU).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use box_assets::{AssetEvent, AssetEventKind, LoadState};
use box_core::asset_ref::AssetRef;
use box_core::assets::{AssetLookup, Assets};
use box_core::event::{EventCursor, Events};
use box_core::image::ImageData;
use box_core::material::StandardMaterial;
use box_core::mesh::MeshData;
use box_core::mesh_asset::MeshAsset;
use box_core::prefab::PrefabInstance;
use box_core::prelude::Transform;
use box_core::scene;
use box_editor::assets::{AssetItem, Picker, RefStatus, RefType, SettingsEdit};
use box_editor::headless::{CaptureOptions, capture};
use box_editor::log::LogBuffer;
use box_editor::{Editor, EditorOptions, NewEntity, build_editor, layout};
use box_render::RenderError;
use glam::Vec3;

const IDLE: Duration = Duration::from_secs(30);

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../box_assets/tests/fixtures")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            copy_dir(&p, &to.join(e.file_name()));
        } else {
            std::fs::copy(&p, to.join(e.file_name())).unwrap();
        }
    }
}

fn temp_dir(name: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "box_editor_assets_{name}_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A project whose assets are the box_assets fixture corpus.
fn project_editor(name: &str) -> (Editor, PathBuf) {
    let dir = temp_dir(name);
    box_engine::Project::create(&dir, name).unwrap();
    for sub in ["models", "textures", "materials", "bad"] {
        copy_dir(&fixtures().join(sub), &dir.join("assets").join(sub));
    }
    let options = EditorOptions {
        project: Some(dir.clone()),
        ..Default::default()
    };
    let editor = build_editor(&options, LogBuffer::default()).unwrap();
    let root = editor.state.project.as_ref().unwrap().root().to_path_buf();
    (editor, root)
}

fn id_of(editor: &Editor, path: &str) -> box_core::asset_ref::AssetId {
    editor
        .state
        .asset_server()
        .unwrap()
        .id_of(path)
        .unwrap_or_else(|| panic!("{path} is not indexed"))
}

fn load_and_wait(editor: &mut Editor, path: &str) -> box_core::asset_ref::AssetId {
    let id = id_of(editor, path);
    editor.state.request_file(id);
    assert!(editor.state.wait_assets_idle(IDLE));
    id
}

fn events(editor: &Editor) -> Vec<AssetEvent> {
    let mut cursor = EventCursor::<AssetEvent>::default();
    editor
        .state
        .edit_world()
        .get_resource::<Events<AssetEvent>>()
        .map(|e| cursor.read(e).cloned().collect())
        .unwrap_or_default()
}

#[test]
fn the_server_follows_the_project() {
    let mut editor = build_editor(&EditorOptions::default(), LogBuffer::default()).unwrap();
    assert!(
        editor.state.asset_server().is_none(),
        "no project, no server"
    );
    assert!(editor.state.asset_files().is_empty());
    let (mut editor, root) = project_editor("follows");
    let server = editor.state.asset_server().unwrap().clone();
    assert!(server.is_watching());
    assert_eq!(
        server.assets_dir(),
        std::fs::canonicalize(root.join("assets")).unwrap()
    );
    assert!(
        editor
            .state
            .world
            .contains_resource::<box_assets::AssetServer>()
    );
    assert!(editor.state.world.contains_resource::<AssetLookup>());
    // Switching the scene replaces the world and the server.
    let main = root.join("assets/scenes/main.scene.json");
    editor.state.open_scene(&main, false).unwrap();
    assert!(
        editor
            .state
            .world
            .contains_resource::<box_assets::AssetServer>()
    );
    // During Play the server keeps driving the parked edit world.
    editor.state.start_play().unwrap();
    assert!(
        !editor
            .state
            .world
            .contains_resource::<box_assets::AssetServer>()
    );
    let id = load_and_wait(&mut editor, "textures/checker.png");
    let h = editor
        .state
        .asset_server()
        .unwrap()
        .get_handle::<ImageData>(&AssetRef::id(id))
        .unwrap();
    assert!(
        editor
            .state
            .edit_world()
            .resource::<Assets<ImageData>>()
            .get(h)
            .is_some()
    );
    editor.state.stop_play();
}

#[test]
fn the_browser_lists_files_sub_assets_and_failures() {
    let (mut editor, _root) = project_editor("listing");
    let files = editor.state.asset_files();
    let paths: Vec<&str> = files.iter().map(|f| f.1.as_str()).collect();
    for p in [
        "models/tower.glb",
        "models/crate.gltf",
        "textures/checker.png",
        "materials/stone.material.json",
        "scenes/main.scene.json",
    ] {
        assert!(paths.contains(&p), "{p} missing from {paths:?}");
    }
    let folders = editor.state.asset_folders();
    for f in ["bad", "materials", "models", "scenes", "textures"] {
        assert!(
            folders.contains(&f.to_string()),
            "{f} missing from {folders:?}"
        );
    }
    let tower = load_and_wait(&mut editor, "models/tower.glb");
    let subs = editor.state.asset_server().unwrap().sub_assets(tower);
    let labels: HashSet<String> = subs.iter().filter_map(|s| s.label.clone()).collect();
    for l in [
        "Mesh/Roof",
        "Mesh/Base",
        "Material/Stone",
        "Scene",
        "Texture/0",
    ] {
        assert!(labels.contains(l), "{l} missing from {labels:?}");
    }
    assert_eq!(
        editor.state.item_type(&AssetItem::sub(tower, "Mesh/Roof")),
        RefType::Mesh
    );
    // A broken file shows its error.
    let bad = load_and_wait(&mut editor, "bad/truncated.png");
    assert!(matches!(
        editor.state.asset_server().unwrap().file_state(bad),
        Some(LoadState::Failed(_))
    ));
    let status = editor
        .state
        .ref_status(&AssetRef::path("bad/truncated.png"));
    assert!(matches!(status, RefStatus::Failed(_)), "{status:?}");
    assert_eq!(
        editor.state.ref_status(&AssetRef::path("nope.png")),
        RefStatus::Missing
    );
    // The picker lists compatible assets only.
    let meshes = editor.state.compatible_assets(RefType::Mesh);
    assert!(
        meshes
            .iter()
            .any(|(i, _)| *i == AssetItem::sub(tower, "Mesh/Roof"))
    );
    assert!(
        meshes
            .iter()
            .all(|(i, _)| i.label.as_deref().is_some_and(|l| l.starts_with("Mesh/")))
    );
    let materials: Vec<String> = editor
        .state
        .compatible_assets(RefType::Material)
        .into_iter()
        .map(|m| m.1)
        .collect();
    for m in [
        "materials/stone.material.json",
        "models/tower.glb#Material/Stone",
        "models/tower.glb#Material/Roof_Tiles",
    ] {
        assert!(
            materials.iter().any(|n| n == m),
            "{m} missing from {materials:?}"
        );
    }
    let images = editor.state.compatible_assets(RefType::Image);
    assert!(images.iter().any(|(_, n)| n == "textures/checker.png"));
    assert!(
        images
            .iter()
            .any(|(_, n)| n == "models/tower.glb#Texture/0")
    );
}

#[test]
fn renaming_in_the_browser_keeps_references_and_repairs_path_hints() {
    let (mut editor, root) = project_editor("rename");
    let tower = load_and_wait(&mut editor, "models/tower.glb");
    let uid = editor
        .state
        .drop_asset_at(
            &AssetItem::sub(tower, "Mesh/Roof"),
            Vec3::new(1.0, 0.0, 2.0),
        )
        .unwrap();
    let mesh_of = |editor: &Editor| -> MeshAsset {
        let e = editor.state.world.entity_by_uid(uid).unwrap();
        editor
            .state
            .world
            .get::<MeshAsset>(e)
            .map(|r| MeshAsset::clone(&r))
            .unwrap()
    };
    let before = mesh_of(&editor);
    assert_eq!(before.mesh.path.as_deref(), Some("models/tower.glb"));
    let steps = editor.state.edits.undo_len();

    // Move into a new folder, then rename (inline rename keeps the suffix).
    editor.state.move_asset_to_folder(tower, "props").unwrap();
    editor.state.rename_asset_stem(tower, "keep").unwrap();
    assert!(root.join("assets/props/keep.glb").is_file());
    assert!(root.join("assets/props/keep.glb.meta").is_file());
    assert!(!root.join("assets/models/tower.glb").exists());
    assert!(editor.state.rename_asset_stem(tower, "a/b").is_err());
    assert!(
        editor.state.rename_asset(tower, "props/keep.png").is_err(),
        "type change"
    );

    let after = mesh_of(&editor);
    assert_eq!(after.mesh.id, before.mesh.id, "the id never changes");
    assert_eq!(
        after.mesh.path.as_deref(),
        Some("props/keep.glb"),
        "hint repaired"
    );
    assert!(
        after
            .materials
            .iter()
            .all(|m| m.path.as_deref() == Some("props/keep.glb"))
    );
    // Each move repaired the hints as one undo step.
    assert_eq!(editor.state.edits.undo_len(), steps + 2);
    assert_eq!(editor.state.edits.undo_label(), Some("Update asset paths"));
    // The reference still resolves to the loaded mesh, by id and by the new path.
    let lookup = editor.state.edit_world().resource::<AssetLookup>();
    let h = lookup.get::<MeshData>(&after.mesh).expect("resolves");
    assert_eq!(
        lookup.get::<MeshData>(&AssetRef::path("props/keep.glb#Mesh/Roof")),
        Some(h)
    );
    // Undo puts the old hint back; the id still resolves.
    editor.state.undo();
    let stale = mesh_of(&editor);
    assert_eq!(stale.mesh.path.as_deref(), Some("props/tower.glb"));
    let lookup = editor.state.edit_world().resource::<AssetLookup>();
    assert_eq!(lookup.get::<MeshData>(&stale.mesh), Some(h));
}

#[test]
fn renaming_the_open_scene_keeps_it_open() {
    let (mut editor, root) = project_editor("rename_scene");
    let id = id_of(&editor, "scenes/main.scene.json");
    editor.state.rename_asset_stem(id, "level").unwrap();
    let new = root.join("assets/scenes/level.scene.json");
    assert_eq!(
        editor
            .state
            .scene
            .path
            .as_deref()
            .map(std::fs::canonicalize)
            .map(Result::unwrap),
        Some(std::fs::canonicalize(&new).unwrap())
    );
    assert_eq!(editor.state.scene.name, "level");
    editor.state.save_scene().unwrap();
    assert!(new.is_file());
}

#[test]
fn files_dropped_from_the_os_are_copied_into_the_folder() {
    let (mut editor, root) = project_editor("os_drop");
    let outside = temp_dir("os_drop_src");
    std::fs::create_dir_all(&outside).unwrap();
    let glb = outside.join("tower.glb");
    std::fs::copy(fixtures().join("models/tower.glb"), &glb).unwrap();
    let png = outside.join("checker.png");
    std::fs::copy(fixtures().join("textures/checker.png"), &png).unwrap();
    std::fs::write(outside.join("checker.png.meta"), b"not copied").unwrap();
    let original = std::fs::read(&glb).unwrap();

    editor.state.assets.folder = "models".into();
    let added = editor
        .state
        .import_files(&[glb.clone(), png.clone(), outside.join("checker.png.meta")])
        .unwrap();
    // tower.glb is taken in models/: the copy gets a number.
    assert_eq!(added, ["models/tower 2.glb", "models/checker.png"]);
    assert_eq!(
        std::fs::read(&glb).unwrap(),
        original,
        "the original is untouched"
    );
    assert!(glb.is_file() && png.is_file());
    let files = editor.state.asset_files();
    assert!(
        files.iter().any(|f| f.1 == "models/tower 2.glb"),
        "shown in the browser"
    );
    // A copy gets its own id.
    assert_ne!(
        id_of(&editor, "models/tower 2.glb"),
        id_of(&editor, "models/tower.glb")
    );
    let meta = std::fs::read_to_string(root.join("assets/models/checker.png.meta")).unwrap();
    assert!(meta.contains("\"importer\": \"texture\""));
    let id = load_and_wait(&mut editor, "models/tower 2.glb");
    assert_eq!(
        editor.state.asset_server().unwrap().file_state(id),
        Some(LoadState::Loaded)
    );
    let _ = std::fs::remove_dir_all(&outside);
}

#[test]
fn dropping_into_the_viewport_is_one_undoable_entity() {
    let (mut editor, _root) = project_editor("drop");
    let tower = load_and_wait(&mut editor, "models/tower.glb");
    let before = scene::save(&editor.state.world).unwrap();
    let steps = editor.state.edits.undo_len();

    // The model: a PrefabInstance of its #Scene at the drop point.
    let at = Vec3::new(3.0, 0.0, -2.0);
    let uid = editor
        .state
        .drop_asset_at(&AssetItem::file(tower), at)
        .unwrap();
    assert_eq!(editor.state.edits.undo_len(), steps + 1);
    assert_eq!(editor.state.primary(), Some(uid));
    let e = editor.state.world.entity_by_uid(uid).unwrap();
    let p = editor
        .state
        .world
        .get::<PrefabInstance>(e)
        .map(|r| PrefabInstance::clone(&r))
        .unwrap();
    assert_eq!(p.scene.id, Some(tower));
    assert_eq!(p.scene.label.as_deref(), Some("Scene"));
    assert_eq!(p.scene.path.as_deref(), Some("models/tower.glb"));
    // At the drop point, standing on it by its lowest point (the tower's
    // origin is above its base).
    let placed = editor.state.world.get::<Transform>(e).unwrap().translation;
    assert_eq!((placed.x, placed.z), (at.x, at.z));
    box_core::transform::propagate_transforms(&mut editor.state.world);
    let bottom = box_editor::entity_bounds(&editor.state.world, e)
        .unwrap()
        .min
        .y;
    assert!((bottom - at.y).abs() < 1e-4, "{bottom}");
    assert_eq!(editor.state.display_name(e), "tower");
    editor.state.undo();
    assert_eq!(
        scene::save(&editor.state.world).unwrap(),
        before,
        "byte-identical"
    );

    // A mesh sub-asset: MeshAsset with the glTF's materials for it.
    let uid = editor
        .state
        .drop_asset_at(&AssetItem::sub(tower, "Mesh/Roof"), Vec3::ZERO)
        .unwrap();
    let e = editor.state.world.entity_by_uid(uid).unwrap();
    let m = editor
        .state
        .world
        .get::<MeshAsset>(e)
        .map(|r| MeshAsset::clone(&r))
        .unwrap();
    assert_eq!(m.mesh.label.as_deref(), Some("Mesh/Roof"));
    assert!(!m.materials.is_empty(), "materials from the glTF scene");
    assert!(m.materials.iter().all(|r| r.id == Some(tower)));
    editor.state.undo();
    assert_eq!(scene::save(&editor.state.world).unwrap(), before);

    // A .scene.json.
    let level = id_of(&editor, "scenes/main.scene.json");
    let uid = editor
        .state
        .drop_asset_at(&AssetItem::file(level), Vec3::ZERO)
        .unwrap();
    let e = editor.state.world.entity_by_uid(uid).unwrap();
    let p = editor
        .state
        .world
        .get::<PrefabInstance>(e)
        .map(|r| PrefabInstance::clone(&r))
        .unwrap();
    assert_eq!((p.scene.id, p.scene.label), (Some(level), None));
    editor.state.undo();

    // Textures and materials cannot be placed.
    let tex = id_of(&editor, "textures/checker.png");
    assert!(
        editor
            .state
            .drop_asset_at(&AssetItem::file(tex), Vec3::ZERO)
            .is_none()
    );
    assert_eq!(scene::save(&editor.state.world).unwrap(), before);

    // The drop point: the ground plane under the pointer, or the camera target.
    let p = editor
        .state
        .viewport_drop_point(glam::Vec2::new(0.0, -0.9), 16.0 / 9.0);
    assert!(
        p.y.abs() < 1e-3 || p.y > 0.0,
        "on the ground or a surface: {p}"
    );
}

#[test]
fn materials_are_assigned_by_dropping_them_on_entities() {
    let (mut editor, _root) = project_editor("assign");
    let stone = load_and_wait(&mut editor, "materials/stone.material.json");
    let tower = load_and_wait(&mut editor, "models/tower.glb");
    // A MeshAsset entity gets the material in slot 0.
    let uid = editor
        .state
        .drop_asset_at(&AssetItem::sub(tower, "Mesh/Roof"), Vec3::ZERO)
        .unwrap();
    let steps = editor.state.edits.undo_len();
    assert!(editor.state.assign_material(uid, &AssetItem::file(stone)));
    assert_eq!(editor.state.edits.undo_len(), steps + 1);
    let e = editor.state.world.entity_by_uid(uid).unwrap();
    let m = editor
        .state
        .world
        .get::<MeshAsset>(e)
        .map(|r| MeshAsset::clone(&r))
        .unwrap();
    assert_eq!(m.materials[0].id, Some(stone));
    // A Primitive entity gets a link (MaterialRef); its inline material is
    // kept.
    let cube = editor.state.add_entity(NewEntity::Cube, None).unwrap();
    let steps = editor.state.edits.undo_len();
    assert!(editor.state.assign_material(cube, &AssetItem::file(stone)));
    assert_eq!(editor.state.edits.undo_len(), steps + 1);
    let e = editor.state.world.entity_by_uid(cube).unwrap();
    let link = editor
        .state
        .world
        .get::<box_core::mesh_asset::MaterialRef>(e)
        .map(|r| r.material.clone())
        .unwrap();
    assert_eq!(link.id, Some(stone));
    assert_eq!(
        editor
            .state
            .world
            .get::<StandardMaterial>(e)
            .unwrap()
            .roughness,
        0.5
    );
    editor.state.undo();
    let e = editor.state.world.entity_by_uid(cube).unwrap();
    assert!(
        !editor
            .state
            .world
            .has::<box_core::mesh_asset::MaterialRef>(e)
    );
    // Not a material: refused.
    let tex = id_of(&editor, "textures/checker.png");
    assert!(!editor.state.assign_material(cube, &AssetItem::file(tex)));
}

#[test]
fn import_settings_apply_reimports() {
    let (mut editor, root) = project_editor("settings");
    let id = load_and_wait(&mut editor, "textures/checker.png");
    let server = editor.state.asset_server().unwrap().clone();
    let h = server.get_handle::<ImageData>(&AssetRef::id(id)).unwrap();
    let version = |e: &Editor| {
        e.state
            .edit_world()
            .resource::<Assets<ImageData>>()
            .version(h)
    };
    let v1 = version(&editor).unwrap();
    editor.state.select_asset(Some(AssetItem::file(id)));
    {
        let s = editor
            .state
            .assets
            .inspect
            .as_mut()
            .unwrap()
            .settings
            .as_mut()
            .expect("textures have import settings");
        assert_eq!(s.importer, "texture");
        let SettingsEdit::Texture(t) = &mut s.edit else {
            panic!("texture settings expected")
        };
        t.mipmaps = false;
        assert!(s.is_dirty());
    }
    // Revert drops the edit; Apply writes the .meta and re-imports.
    editor.state.revert_import_settings();
    assert!(
        !editor
            .state
            .assets
            .inspect
            .as_ref()
            .unwrap()
            .settings
            .as_ref()
            .unwrap()
            .is_dirty()
    );
    if let Some(SettingsEdit::Texture(t)) = editor
        .state
        .assets
        .inspect
        .as_mut()
        .and_then(|i| i.settings.as_mut())
        .map(|s| &mut s.edit)
    {
        t.mipmaps = false;
    }
    editor.state.apply_import_settings().unwrap();
    assert_eq!(server.file_state(id), Some(LoadState::Loading));
    assert!(editor.state.wait_assets_idle(IDLE));
    assert_eq!(server.file_state(id), Some(LoadState::Loaded));
    assert!(version(&editor).unwrap() > v1);
    let image = editor
        .state
        .edit_world()
        .resource::<Assets<ImageData>>()
        .get(h)
        .unwrap()
        .clone();
    assert_eq!(image.mips.len(), 1);
    let meta = std::fs::read_to_string(root.join("assets/textures/checker.png.meta")).unwrap();
    assert!(meta.contains("\"mipmaps\": false"), "{meta}");
    assert!(
        !editor
            .state
            .assets
            .inspect
            .as_ref()
            .unwrap()
            .settings
            .as_ref()
            .unwrap()
            .is_dirty()
    );
    // Selecting an entity closes the asset inspector.
    editor.state.select(None);
    assert!(editor.state.assets.inspect.is_none());
}

#[test]
fn the_material_inspector_writes_the_file_and_it_reloads() {
    let (mut editor, root) = project_editor("material");
    let id = editor.state.new_material_asset("materials").unwrap();
    let path = root.join("assets/materials/New Material.material.json");
    assert!(path.is_file());
    let original = std::fs::read(&path).unwrap();
    assert!(editor.state.wait_assets_idle(IDLE));
    assert_eq!(editor.state.assets.selected, Some(AssetItem::file(id)));
    let steps = editor.state.edits.undo_len();
    let handle = editor
        .state
        .asset_server()
        .unwrap()
        .get_handle::<StandardMaterial>(&AssetRef::id(id))
        .unwrap();

    editor.state.edit_material(1.0, |m| m.roughness = 0.2);
    // Not written while a drag goes on, until the delay passes.
    editor.state.flush_material_edit(1.1, false);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    editor.state.flush_material_edit(1.4, false);
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(written.contains("\"roughness\": 0.2"), "{written}");
    assert!(editor.state.wait_assets_idle(IDLE));
    let reloaded = events(&editor)
        .iter()
        .any(|e| e.id == id && e.kind == AssetEventKind::Reloaded);
    assert!(reloaded, "the reload event arrives");
    let value = editor
        .state
        .edit_world()
        .resource::<Assets<StandardMaterial>>()
        .get(handle)
        .unwrap()
        .roughness;
    assert!((value - 0.2).abs() < 1e-6);
    assert_eq!(
        editor.state.edits.undo_len(),
        steps,
        "file edits are not undo steps"
    );

    // Revert puts the file back byte for byte.
    editor.state.revert_material();
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(editor.state.wait_assets_idle(IDLE));
    let m = editor
        .state
        .assets
        .inspect
        .as_ref()
        .unwrap()
        .material
        .as_ref()
        .unwrap();
    assert_eq!(m.value.roughness, StandardMaterial::default().roughness);

    // glTF materials are read-only.
    let tower = load_and_wait(&mut editor, "models/tower.glb");
    editor
        .state
        .select_asset(Some(AssetItem::sub(tower, "Material/Stone")));
    let m = editor
        .state
        .assets
        .inspect
        .as_ref()
        .unwrap()
        .material
        .as_ref()
        .unwrap();
    assert!(m.read_only);
    assert_eq!(m.value.roughness, 0.9);
}

#[test]
fn shader_headers_are_read_from_wgsl_files() {
    let (mut editor, root) = project_editor("shader");
    let wgsl = root.join("assets/shaders/stripes.wgsl");
    std::fs::create_dir_all(wgsl.parent().unwrap()).unwrap();
    std::fs::write(
        &wgsl,
        "@material struct Params {\n    /// Stripe colour.\n    @color tint: vec3f = vec3f(0.9, 0.3, 0.2),\n    @range(1.0, 40.0) frequency: f32 = 10.0,\n}\nfn surface() {}\n",
    )
    .unwrap();
    let info = editor
        .state
        .shader_info(&AssetRef::path("shaders/stripes.wgsl"))
        .unwrap();
    let header = info.header.unwrap().unwrap();
    let names: Vec<&str> = header.params.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["tint", "frequency"]);
    assert_eq!(header.params[1].range, Some((1.0, 40.0)));
    assert_eq!(header.params[0].doc.as_deref(), Some("Stripe colour."));
    std::fs::write(&wgsl, "@material struct Params { x: mat4x4f }\n").unwrap();
    // Re-read at most once a second.
    std::thread::sleep(Duration::from_millis(1100));
    let info = editor
        .state
        .shader_info(&AssetRef::path("shaders/stripes.wgsl"))
        .unwrap();
    assert!(info.header.is_err(), "{:?}", info.header);
    assert!(editor.state.shader_info(&AssetRef::NONE).is_none());
    let missing = editor
        .state
        .shader_info(&AssetRef::path("shaders/none.wgsl"))
        .unwrap();
    assert!(missing.header.is_err());
}

// ------------------------------------------------------------- captures ----

fn try_capture(editor: &mut Editor, name: &str) -> Option<image::RgbaImage> {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping {name}");
        return None;
    }
    let options = CaptureOptions {
        size: [1600, 1000],
        pixels_per_point: 1.0,
        frames: 6,
    };
    let image = match capture(editor, &options) {
        Ok(i) => i,
        Err(e)
            if matches!(
                e.downcast_ref::<RenderError>(),
                Some(RenderError::NoAdapter)
            ) =>
        {
            eprintln!("no GPU adapter; skipping {name}");
            return None;
        }
        Err(e) => panic!("{e:#}"),
    };
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/captures");
    std::fs::create_dir_all(&out).unwrap();
    image.save(out.join(format!("{name}.png"))).unwrap();
    Some(image)
}

#[test]
fn capture_the_browser_picker_settings_and_material_inspector() {
    let (mut editor, root) = project_editor("captures");
    editor.tree = layout::default_layout();
    // A broken image next to good ones, and the tower's sub-assets shown.
    std::fs::copy(
        fixtures().join("bad/truncated.png"),
        root.join("assets/models/broken.png"),
    )
    .unwrap();
    editor.state.asset_server().unwrap().rescan();
    let tower = load_and_wait(&mut editor, "models/tower.glb");
    editor.state.assets.folder = "models".into();
    editor.state.assets.expanded.insert(tower);

    // 1. Browser with thumbnails and a failed badge; the image's import
    // settings in the inspector, edited (Apply enabled).
    let color = id_of(&editor, "models/crate_color.png");
    editor.state.select_asset(Some(AssetItem::file(color)));
    if let Some(SettingsEdit::Texture(t)) = editor
        .state
        .assets
        .inspect
        .as_mut()
        .and_then(|i| i.settings.as_mut())
        .map(|s| &mut s.edit)
    {
        t.color_space = box_assets::meta::ColorSpaceSetting::Linear;
    }
    if try_capture(&mut editor, "assets_browser").is_none() {
        return;
    }
    let bad = id_of(&editor, "models/broken.png");
    assert!(matches!(
        editor.state.asset_server().unwrap().file_state(bad),
        Some(LoadState::Failed(_))
    ));
    // A model's import settings, edited (Apply enabled).
    editor.state.select_asset(Some(AssetItem::file(tower)));
    if let Some(SettingsEdit::Model(m)) = editor
        .state
        .assets
        .inspect
        .as_mut()
        .and_then(|i| i.settings.as_mut())
        .map(|s| &mut s.edit)
    {
        m.scale = 0.01;
    }
    try_capture(&mut editor, "assets_import_settings");

    // 2. The material inspector.
    let stone = id_of(&editor, "materials/stone.material.json");
    editor.state.assets.folder = "materials".into();
    editor.state.select_asset(Some(AssetItem::file(stone)));
    try_capture(&mut editor, "assets_material_inspector");

    // 3. The asset picker on a MeshAsset material slot.
    editor.state.assets.folder = "models".into();
    let uid = editor
        .state
        .drop_asset_at(&AssetItem::sub(tower, "Mesh/Roof"), Vec3::ZERO)
        .unwrap();
    editor.state.select(Some(uid));
    editor.state.assets.picker = Some(Picker {
        path: "MeshAsset.materials[0]".into(),
        filter: String::new(),
    });
    try_capture(&mut editor, "assets_picker");
    assert!(
        editor.state.assets.picker.is_some(),
        "the picker stayed open"
    );
}

/// A `ShaderMaterial` whose parameters come from the shader's header, with
/// the renderer's compile error under it.
#[test]
fn capture_shader_material_params_and_error() {
    use box_core::shader_material::{ShaderMaterial, ShaderParamValue, ShaderSource};
    let (mut editor, root) = project_editor("shader_capture");
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../box_engine/examples/assets/stripes.wgsl"),
    )
    .unwrap();
    let path = "shaders/stripes.wgsl";
    std::fs::create_dir_all(root.join("assets/shaders")).unwrap();
    // A shader with a bug in its body (its header is valid, so it imports).
    let broken = text.replace(
        "s.roughness = mix(0.8, 0.35, stripe);",
        "s.roughness = no_such_function(stripe);",
    );
    std::fs::write(root.join("assets").join(path), &broken).unwrap();
    let uid = editor.state.add_entity(NewEntity::Cube, None).unwrap();
    let material = ShaderMaterial::new(AssetRef::path(path))
        .with_param("frequency", ShaderParamValue::Float(20.0));
    editor.state.finish_edit();
    let r = editor.state.edits.add_component(
        &mut editor.state.world,
        uid,
        "ShaderMaterial",
        Some(&material),
    );
    r.unwrap();
    editor.state.select(Some(uid));
    // The scene glue makes the material an asset, then the asset glue
    // requests its shader and the server imports it.
    box_engine::sync_scene_meshes(&mut editor.state.world);
    box_engine::sync_assets(&mut editor.state.world);
    assert!(editor.state.wait_assets_idle(Duration::from_secs(30)));
    let h = editor
        .state
        .asset_server()
        .unwrap()
        .get_handle::<ShaderSource>(&AssetRef::path(path))
        .expect("the glue loaded the material's shader");
    let info = editor.state.shader_info(&AssetRef::path(path)).unwrap();
    assert_eq!(info.handle, Some(h), "the loaded source wins over the file");
    assert_eq!(info.header.unwrap().unwrap().params.len(), 5);
    if try_capture(&mut editor, "assets_shader_material").is_none() {
        return;
    }
    let error = editor.state.shader_error(h);
    assert!(
        error
            .as_deref()
            .is_some_and(|e| e.contains("no_such_function")),
        "{error:?}"
    );
}

#[test]
fn double_clicking_a_scene_opens_it() {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        return;
    }
    let (mut editor, root) = project_editor("open_scene");
    let level = root.join("assets/scenes/level.scene.json");
    std::fs::copy(fixtures().join("scenes/level.scene.json"), &level).unwrap();
    editor.state.asset_server().unwrap().rescan();
    let id = id_of(&editor, "scenes/level.scene.json");
    editor.state.open_asset(&AssetItem::file(id));
    // The request is handled by the next frame (no unsaved changes: no prompt).
    let options = CaptureOptions {
        size: [640, 400],
        pixels_per_point: 1.0,
        frames: 1,
    };
    match capture(&mut editor, &options) {
        Ok(_) => {}
        Err(e)
            if matches!(
                e.downcast_ref::<RenderError>(),
                Some(RenderError::NoAdapter)
            ) =>
        {
            return;
        }
        Err(e) => panic!("{e:#}"),
    }
    assert_eq!(editor.state.scene.name, "level");
}

/// Saving writes asset references with ids and current paths, even when
/// they were typed as paths, as one undo step before the write.
#[test]
fn saving_writes_asset_references_with_ids() {
    let (mut editor, root) = project_editor("save_refs");
    let tower = id_of(&editor, "models/tower.glb");
    let uid = editor.state.add_entity(NewEntity::Empty, None).unwrap();
    let path_only = MeshAsset {
        mesh: AssetRef::path("models/tower.glb#Mesh/Roof"),
        ..Default::default()
    };
    editor.state.add_component(uid, "MeshAsset");
    assert!(editor.state.set_field(uid, "MeshAsset", &path_only));
    let steps = editor.state.edits.undo_len();
    editor.save();
    assert!(!editor.state.is_dirty(), "saved");
    assert_eq!(editor.state.edits.undo_len(), steps + 1, "one repair step");
    let file = root.join("assets/scenes/main.scene.json");
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
    let mesh = json["entities"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|e| e["components"].get("MeshAsset"))
        .expect("the MeshAsset entity is saved")["mesh"]
        .clone();
    assert_eq!(mesh["id"], serde_json::json!(tower.to_string()));
    assert_eq!(mesh["label"], "Mesh/Roof");
    assert_eq!(mesh["path"], "models/tower.glb");
}

/// A shader-material file dropped on a primitive is linked and drawn as a
/// ShaderMaterial, without importing it as the wrong type: the file stays
/// loaded and nothing is logged as failed.
#[test]
fn a_shader_material_file_is_not_loaded_as_a_standard_material() {
    let (mut editor, root) = project_editor("shader_drop");
    let shaders = root.join("assets/shaders");
    std::fs::create_dir_all(&shaders).unwrap();
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../box_engine/examples/assets/stripes.wgsl"),
        shaders.join("stripes.wgsl"),
    )
    .unwrap();
    std::fs::write(
        root.join("assets/materials/stripes.material.json"),
        r#"{"shader": "shaders/stripes.wgsl", "params": [], "alpha_mode": "Opaque", "double_sided": false}"#,
    )
    .unwrap();
    editor.state.asset_server().unwrap().rescan();
    // Dropped before it was ever loaded: the link resolves it when drawn.
    let id = id_of(&editor, "materials/stripes.material.json");
    let cube = editor.state.add_entity(NewEntity::Cube, None).unwrap();
    let steps = editor.state.edits.undo_len();
    assert!(editor.state.assign_material(cube, &AssetItem::file(id)));
    assert_eq!(editor.state.edits.undo_len(), steps + 1);
    for _ in 0..3 {
        box_engine::sync_scene_meshes(&mut editor.state.world);
        assert!(editor.state.wait_assets_idle(IDLE));
    }
    // Loaded as a ShaderMaterial, which the cube now draws with.
    let e = editor.state.world.entity_by_uid(cube).unwrap();
    let r = editor
        .state
        .world
        .get::<box_render::MeshRenderer>(e)
        .map(|r| r.materials.clone())
        .unwrap();
    assert!(
        matches!(r.as_slice(), [box_render::MaterialHandle::Shader(_)]),
        "{r:?}"
    );
    let failed: Vec<_> = events(&editor)
        .into_iter()
        .filter(|e| e.id == id && e.kind == AssetEventKind::Failed)
        .map(|e| e.message)
        .collect();
    assert!(failed.is_empty(), "{failed:?}");
    let server = editor.state.asset_server().unwrap();
    assert_eq!(server.file_state(id), Some(LoadState::Loaded));
}
