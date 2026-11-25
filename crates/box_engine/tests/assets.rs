//! M4 acceptance tests for the runtime asset glue: prefabs, `MeshAsset`
//! renderers, textures and shaders through the asset server, hot reload,
//! renames, placeholders and saving scenes with prefab instances.
//!
//! Rendering tests are skipped when `BOX_SKIP_GPU_TESTS` is set or no GPU
//! adapter exists; their captures are written to `target/captures/m4/`.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use box_engine::box_assets::{AssetServer, LoadState};
use box_engine::box_core::edit::EditStack;
use box_engine::box_core::prefab::{Instanced, PrefabInstance};
use box_engine::box_core::scene;
use box_engine::prelude::*;
use box_engine::{Project, WindowSize, app_with_world, settle_assets, shared_engine_registry};
use box_render::RenderError;
use serde_json::{Value as Json, json};

const W: u32 = 480;
const H: u32 = 320;
const SETTLE: Duration = Duration::from_secs(30);

// ------------------------------------------------------------- helpers ----

/// Every log line of this test binary, for "logs one error" checks (each
/// test filters by a file name of its own).
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn log() -> &'static Captured {
    static LOG: OnceLock<Captured> = OnceLock::new();
    LOG.get_or_init(|| {
        let log = Captured::default();
        let writer = log.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .finish();
        let _ = tracing::subscriber::set_global_default(subscriber);
        log
    })
}

/// Error lines logged so far that mention `needle`.
fn errors_about(needle: &str) -> usize {
    let text = String::from_utf8_lossy(&log().0.lock().unwrap()).into_owned();
    text.lines()
        .filter(|l| l.contains("ERROR") && l.contains(needle))
        .count()
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../box_assets/tests/fixtures")
}

fn captures_dir() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/m4");
    std::fs::create_dir_all(&dir).unwrap();
    dir
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

/// A project whose `assets/` holds the box_assets fixture corpus (models,
/// textures, materials, scenes), removed on drop.
struct TempProject {
    root: PathBuf,
}

impl TempProject {
    fn new(name: &str) -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        log();
        let root = std::env::temp_dir().join(format!(
            "box_engine_assets_{name}_{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        Project::create(&root, "Assets Test").unwrap();
        for dir in ["models", "textures", "materials", "scenes"] {
            copy_dir(&fixtures().join(dir), &root.join("assets").join(dir));
        }
        Self { root }
    }

    fn assets(&self) -> PathBuf {
        self.root.join("assets")
    }

    fn write(&self, rel: &str, bytes: impl AsRef<[u8]>) {
        let path = self.assets().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn server(&self) -> AssetServer {
        AssetServer::new(self.assets()).unwrap()
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn png(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
    let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba(f(x, y)));
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

/// A world with a camera looking at `target` from `eye` and a sun.
fn base_world(eye: Vec3, target: Vec3) -> World {
    let mut w = World::with_registry(shared_engine_registry());
    w.spawn_saved(
        Uid::new(),
        "Camera",
        (
            Transform::from_translation(eye).looking_at(target, Vec3::Y),
            Camera::default(),
        ),
    )
    .unwrap();
    w.spawn_saved(
        Uid::new(),
        "Sun",
        (
            Transform::from_xyz(0.0, 0.0, 0.0).looking_at(Vec3::new(-0.4, -1.0, -0.6), Vec3::Y),
            DirectionalLight::default(),
        ),
    )
    .unwrap();
    w
}

/// An app around `world` serving `project`'s assets, settled.
fn app(project: &TempProject, world: World) -> (App, AssetServer) {
    let server = project.server();
    let mut world = world;
    world.insert_resource(server.clone());
    let mut app = app_with_world(world);
    app.world.insert_resource(WindowSize {
        width: W,
        height: H,
        scale_factor_milli: 1000,
    });
    assert!(
        settle_assets(&mut app.world, SETTLE),
        "assets did not settle"
    );
    app.tick(1.0 / 60.0);
    (app, server)
}

fn renderer() -> Option<Renderer> {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping");
        return None;
    }
    match Renderer::new_headless(W, H) {
        Ok(r) => Some(r),
        Err(RenderError::NoAdapter) => {
            eprintln!("no GPU adapter; skipping");
            None
        }
        Err(e) => panic!("renderer creation failed: {e}"),
    }
}

/// Two frames, then a capture saved as `target/captures/m4/<name>.png`.
fn capture(app: &mut App, r: &mut Renderer, name: &str) -> image::RgbaImage {
    for _ in 0..2 {
        app.tick(1.0 / 60.0);
        r.render(&mut app.world).unwrap();
    }
    app.tick(1.0 / 60.0);
    let img = r.capture(&mut app.world).unwrap();
    img.save(captures_dir().join(format!("{name}.png")))
        .unwrap();
    assert_eq!(
        r.stats().validation_errors,
        0,
        "{name}: wgpu validation errors"
    );
    img
}

/// Pixels whose colour differs by more than `tol` in any channel.
fn differing(a: &image::RgbaImage, b: &image::RgbaImage, tol: u8) -> usize {
    a.pixels()
        .zip(b.pixels())
        .filter(|(p, q)| p.0.iter().zip(q.0).any(|(x, y)| x.abs_diff(y) > tol))
        .count()
}

fn count(img: &image::RgbaImage, f: impl Fn([u8; 4]) -> bool) -> usize {
    img.pixels().filter(|p| f(p.0)).count()
}

fn instanced(world: &World) -> usize {
    world.query::<&Instanced>().iter().count()
}

/// Reloads after changing files on disk without the watcher: rescan, then
/// settle and run a frame.
fn reload(app: &mut App, server: &AssetServer) {
    server.rescan();
    assert!(settle_assets(&mut app.world, SETTLE));
    app.tick(1.0 / 60.0);
}

fn tower_world(server: Option<&AssetServer>) -> (World, Uid) {
    let mut w = base_world(Vec3::new(2.2, 2.0, 1.2), Vec3::new(0.0, 0.7, -2.0));
    let mut scene = AssetRef::path("models/tower.glb#Scene");
    if let Some(s) = server {
        scene = s.canonical_ref(&scene).unwrap();
    }
    let uid = Uid::new();
    w.spawn_saved(
        uid,
        "Tower",
        (Transform::IDENTITY, PrefabInstance { scene }),
    )
    .unwrap();
    (w, uid)
}

// --------------------------------------------------------------- tests ----

/// A `.glb` instanced as a prefab renders with its materials and embedded
/// textures: instanced entities appear beneath the root, the meshes get
/// renderers, and the checker texture is visible.
#[test]
fn glb_prefab_renders_with_materials_and_textures() {
    let project = TempProject::new("glb");
    let (world, root_uid) = tower_world(None);
    let (mut app, server) = app(&project, world);
    // tower (glTF root) > Tower > Base, Roof > Flag
    assert_eq!(instanced(&app.world), 5);
    let root = app.world.entity_by_uid(root_uid).unwrap();
    let renderers: Vec<MeshRenderer> = app
        .world
        .query::<(&MeshRenderer, &Instanced)>()
        .iter()
        .map(|(r, _)| r.clone())
        .collect();
    assert_eq!(renderers.len(), 2, "Base and Roof");
    for r in &renderers {
        assert!(!r.materials.is_empty());
        for m in &r.materials {
            assert!(matches!(m, MaterialHandle::Standard(_)));
        }
    }
    // The embedded checker is requested and loaded.
    let tex = AssetRef::path("models/tower.glb#Texture/0");
    assert_eq!(server.state(&tex), Some(LoadState::Loaded));
    assert!(
        app.world
            .resource::<AssetLookup>()
            .get::<ImageData>(&tex)
            .is_some()
    );
    // Instanced uids are derived: the same after a rebuild.
    let before: Vec<Uid> = sorted_instanced_uids(&app.world);
    assert!(
        app.world
            .children(root)
            .iter()
            .all(|c| app.world.get::<Instanced>(*c).unwrap().instance == root)
    );

    let Some(mut r) = renderer() else { return };
    let img = capture(&mut app, &mut r, "glb_prefab");
    // The red/white checker on the base and the red roof. (Bloom spreads
    // some of the white texels' light into their neighbours: 188 white
    // pixels with the default bloom.)
    let red = count(&img, |p| p[0] as i32 - p[1] as i32 > 50);
    let white = count(&img, |p| p.iter().take(3).all(|c| *c > 200));
    assert!(red > 1500 && white > 120, "{red} red, {white} white pixels");
    assert_eq!(sorted_instanced_uids(&app.world), before);
}

/// A `MeshAsset` with no materials draws the imported mesh with the
/// materials it was imported with, in slot order (M8 gap report item 12).
#[test]
fn empty_mesh_asset_materials_use_the_imported_ones() {
    let project = TempProject::new("own_materials");
    let (mut world, _) = tower_world(None);
    let spawned = Uid::new();
    world
        .spawn_saved(
            spawned,
            "Loose roof",
            (
                Transform::from_xyz(3.0, 0.0, 0.0),
                MeshAsset {
                    mesh: AssetRef::path("models/tower.glb#Mesh/Roof"),
                    ..Default::default()
                },
            ),
        )
        .unwrap();
    let (app, _server) = app(&project, world);
    let e = app.world.entity_by_uid(spawned).unwrap();
    let own = app
        .world
        .get::<MeshRenderer>(e)
        .expect("a renderer")
        .clone();
    // The prefab's roof lists its materials explicitly (the importer wrote
    // them into the model's scene).
    let listed = app
        .world
        .query::<(&MeshRenderer, &MeshAsset, &Instanced)>()
        .iter()
        .find(|(_, m, _)| m.mesh.label.as_deref() == Some("Mesh/Roof"))
        .map(|(r, m, _)| {
            assert!(!m.materials.is_empty());
            r.clone()
        })
        .expect("the instanced roof");
    assert_eq!(own.mesh, listed.mesh);
    assert_eq!(own.materials, listed.materials);
    assert!(
        own.materials
            .iter()
            .all(|m| matches!(m, MaterialHandle::Standard(_)))
    );
}

/// The glTF `colliders` import setting gives mesh nodes a triangle-mesh
/// collider, except nodes named `*-nocol` (M8 gap report item 1).
#[cfg(feature = "physics")]
#[test]
fn gltf_collider_setting_skips_nocol_nodes() {
    use box_engine::box_proc::glb::{GlbMaterial, GlbNode, write_scene_glb};
    let project = TempProject::new("nocol");
    let cube = MeshData::cuboid(Vec3::ONE);
    let stone = StandardMaterial::default();
    let node = |name, x: f32| GlbNode {
        name,
        parent: None,
        translation: Vec3::new(x, 0.0, 0.0),
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
        mesh: Some(&cube),
    };
    let glb = write_scene_glb(
        "level",
        &[
            node("Wall", 0.0),
            node("Grass-NoCol", 2.0),
            node("Rock", 4.0),
        ],
        &[GlbMaterial {
            name: "Stone",
            material: &stone,
        }],
    )
    .unwrap();
    project.write("models/level.glb", &glb.bytes);
    project.write(
        "models/level.glb.meta",
        r#"{ "version": 1, "id": "10000000-0000-4000-8000-0000000000c1", "importer": "gltf",
  "settings": { "colliders": true } }"#,
    );
    let mut w = base_world(Vec3::new(0.0, 3.0, 6.0), Vec3::ZERO);
    w.spawn_saved(
        Uid::new(),
        "Level",
        (
            Transform::IDENTITY,
            PrefabInstance {
                scene: AssetRef::path("models/level.glb#Scene"),
            },
        ),
    )
    .unwrap();
    let (app, _server) = app(&project, w);
    let mut with_collider: Vec<String> = app
        .world
        .query::<(&Name, &MeshAsset, &Instanced)>()
        .iter()
        .map(|(n, _, _)| n.as_str().to_string())
        .collect::<Vec<_>>()
        .into_iter()
        .filter(|n| {
            let e = app.world.find_by_name(n).unwrap();
            app.world.has::<box_engine::box_physics::Collider>(e)
        })
        .collect();
    with_collider.sort();
    assert_eq!(with_collider, ["Rock", "Wall"]);
    assert!(app.world.find_by_name("Grass-NoCol").is_some());
}

fn sorted_instanced_uids(world: &World) -> Vec<Uid> {
    let mut v: Vec<Uid> = world
        .query::<(&Uid, &Instanced)>()
        .iter()
        .map(|(u, _)| *u)
        .collect();
    v.sort();
    v
}

/// Saving writes only the `PrefabInstance` entity, never its contents; the
/// saved scene loads and instantiates again with the same uids. Edits to
/// instanced entities are refused. (No GPU needed.)
#[test]
fn saving_a_prefab_instance_writes_only_the_root() {
    let project = TempProject::new("save");
    let (world, root_uid) = tower_world(None);
    let (mut app, _server) = app(&project, world);
    assert_eq!(instanced(&app.world), 5);
    let uids = sorted_instanced_uids(&app.world);

    let text = scene::save_string(&app.world).unwrap();
    let json: Json = serde_json::from_str(&text).unwrap();
    let names: Vec<&str> = json["entities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Camera", "Sun", "Tower"], "{text}");
    let tower = &json["entities"][2];
    assert_eq!(tower["uid"], json!(root_uid.to_string()));
    assert!(tower["components"]["PrefabInstance"].is_object());
    for e in &uids {
        assert!(!text.contains(&e.to_string()), "instanced {e} saved");
    }

    // Edits through box_core::edit refuse instanced entities.
    let mut edits = EditStack::new();
    let child = uids[0];
    let err = edits
        .rename(&mut app.world, child, "Mine")
        .unwrap_err()
        .to_string();
    assert!(err.contains("prefab instance"), "{err}");
    assert!(edits.despawn(&mut app.world, child).is_err());
    assert!(
        edits
            .spawn(
                &mut app.world,
                Uid::new(),
                "Under",
                box_engine::box_core::world::Placement::last(Some(child)),
                &[]
            )
            .is_err()
    );
    // Undoable edits of the root still work and rebuild the contents.
    let path = app.world.field_path("PrefabInstance.scene").unwrap();
    edits
        .set_field(
            &mut app.world,
            root_uid,
            &path,
            &AssetRef::path("scenes/level.scene.json"),
        )
        .unwrap();
    assert!(settle_assets(&mut app.world, SETTLE));
    assert_eq!(instanced(&app.world), 1, "level.scene.json has one entity");
    edits.undo(&mut app.world).unwrap();
    assert!(settle_assets(&mut app.world, SETTLE));
    assert_eq!(sorted_instanced_uids(&app.world), uids);

    // Loading the saved scene instantiates again, with the same uids.
    let loaded = scene::load(text.as_bytes(), shared_engine_registry()).unwrap();
    assert_eq!(loaded.entity_count(), 3);
    let (again, _) = self::app(&project, loaded);
    assert_eq!(sorted_instanced_uids(&again.world), uids);
}

/// Editing a texture on disk updates the rendered image without a restart.
#[test]
fn editing_a_texture_updates_the_render() {
    let project = TempProject::new("texture");
    let mut w = base_world(Vec3::new(1.6, 1.4, 2.2), Vec3::ZERO);
    w.spawn_saved(
        Uid::new(),
        "Crate",
        (
            Transform::IDENTITY,
            Primitive::default(),
            StandardMaterial {
                base_color: Color::WHITE,
                base_color_texture: AssetRef::path("textures/checker.png"),
                ..Default::default()
            },
        ),
    )
    .unwrap();
    let (mut app, server) = app(&project, w);
    assert_eq!(
        server.state(&AssetRef::path("textures/checker.png")),
        Some(LoadState::Loaded),
        "the inline material's texture was requested"
    );
    let Some(mut r) = renderer() else { return };
    let before = capture(&mut app, &mut r, "texture_before");
    project.write(
        "textures/checker.png",
        png(64, 64, |_, _| [220, 30, 30, 255]),
    );
    reload(&mut app, &server);
    let after = capture(&mut app, &mut r, "texture_after");
    let changed = differing(&before, &after, 24);
    assert!(changed > 2000, "only {changed} pixels changed");
    let red = count(&after, |p| p[0] as i32 - p[1] as i32 > 60);
    assert!(red > 2000, "the cube is red now ({red} px)");
}

const STRIPES_MATERIAL: &str = r#"{
  "shader": "shaders/hot.wgsl",
  "params": [
    { "name": "tint", "value": { "Vec3": [0.1, 0.8, 0.15] } },
    { "name": "frequency", "value": { "Float": 3.0 } }
  ]
}"#;

fn stripes_source() -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/assets/stripes.wgsl"),
    )
    .unwrap()
}

/// A custom shader material authored as files (`.wgsl` + material file with
/// parameters) applied to a `MeshAsset` renders; editing the shader on disk
/// updates the render; a broken edit keeps the previous version and logs
/// one error.
#[test]
fn shader_material_from_files_renders_and_hot_reloads() {
    let project = TempProject::new("shader");
    project.write("shaders/hot.wgsl", stripes_source());
    project.write("materials/hot.material.json", STRIPES_MATERIAL);
    let mut w = base_world(Vec3::new(1.5, 1.3, 1.9), Vec3::new(0.0, 0.5, 0.0));
    w.spawn_saved(
        Uid::new(),
        "Block",
        (
            Transform::IDENTITY,
            MeshAsset {
                mesh: AssetRef::path("models/tower.glb#Mesh/Base"),
                materials: vec![AssetRef::path("materials/hot.material.json")],
                cast_shadows: true,
            },
        ),
    )
    .unwrap();
    let (mut app, server) = app(&project, w);
    let renderer_component = app
        .world
        .query::<(&MeshRenderer, &MeshAsset)>()
        .iter()
        .map(|(r, _)| r.clone())
        .next()
        .expect("the MeshAsset got a renderer");
    assert!(matches!(
        renderer_component.materials[..],
        [MaterialHandle::Shader(_)]
    ));
    let shader = server
        .get_handle::<ShaderSource>(&AssetRef::path("shaders/hot.wgsl"))
        .expect("the material's shader was requested");

    let Some(mut r) = renderer() else { return };
    let green = capture(&mut app, &mut r, "shader_material");
    let greenish = count(&green, |p| p[1] as i32 - p[0] as i32 > 40);
    assert!(greenish > 1000, "green stripes: {greenish}");
    assert!(r.shader_error(shader).is_none());

    // Edit: stripes become blue.
    let blue_src = stripes_source().replace(
        "s.base_color = mix(vec3f(0.92, 0.9, 0.85), p.tint, stripe);",
        "s.base_color = mix(vec3f(0.92, 0.9, 0.85), vec3f(0.1, 0.15, 0.9), stripe);",
    );
    assert_ne!(blue_src, stripes_source());
    project.write("shaders/hot.wgsl", &blue_src);
    reload(&mut app, &server);
    let blue = capture(&mut app, &mut r, "shader_material_edited");
    assert!(differing(&green, &blue, 24) > 1000);
    let bluish = count(&blue, |p| p[2] as i32 - p[0] as i32 > 40);
    assert!(bluish > 1000, "blue stripes: {bluish}");

    // A broken body: the renderer keeps the previous pipeline, one error.
    let errors = errors_about("hot.wgsl");
    project.write(
        "shaders/hot.wgsl",
        blue_src.replace(
            "s.roughness = mix(0.8, 0.35, stripe);",
            "s.roughness = nope(stripe);",
        ),
    );
    reload(&mut app, &server);
    let broken = capture(&mut app, &mut r, "shader_material_broken_body");
    assert_eq!(
        differing(&blue, &broken, 8),
        0,
        "the previous version stays"
    );
    assert!(r.shader_error(shader).is_some_and(|e| e.contains("nope")));
    assert_eq!(errors_about("hot.wgsl") - errors, 1, "one error logged");

    // A broken header: the import fails and the last good source stays.
    let errors = errors_about("hot.wgsl");
    project.write(
        "shaders/hot.wgsl",
        blue_src.replace(
            "@range(1.0, 40.0) frequency: f32",
            "@range(1.0) frequency: f32",
        ),
    );
    reload(&mut app, &server);
    assert!(matches!(
        server.state(&AssetRef::path("shaders/hot.wgsl")),
        Some(LoadState::Failed(_))
    ));
    let kept = capture(&mut app, &mut r, "shader_material_broken_header");
    assert_eq!(differing(&blue, &kept, 8), 0, "the last good source stays");
    assert_eq!(errors_about("hot.wgsl") - errors, 1, "one error logged");
}

/// Renaming an asset keeps the scene rendering identically: references by
/// id still resolve (the prefab, its meshes, materials and textures).
#[test]
fn renaming_an_asset_keeps_the_scene_rendering() {
    let project = TempProject::new("rename");
    let server0 = project.server();
    let (world, _) = tower_world(Some(&server0));
    drop(server0);
    let (mut app, server) = app(&project, world);
    assert_eq!(instanced(&app.world), 5);
    let gpu = renderer();
    let before = gpu.map(|mut r| (capture(&mut app, &mut r, "rename_before"), r));
    server
        .rename("models/tower.glb", "props/old_tower.glb")
        .unwrap();
    app.tick(1.0 / 60.0);
    assert!(settle_assets(&mut app.world, SETTLE));
    assert_eq!(instanced(&app.world), 5);
    let lookup = app.world.resource::<AssetLookup>();
    assert!(
        lookup
            .get::<SceneAsset>(&AssetRef::path("props/old_tower.glb#Scene"))
            .is_some()
    );
    let Some((before, mut r)) = before else {
        return;
    };
    let after = capture(&mut app, &mut r, "rename_after");
    assert_eq!(differing(&before, &after, 2), 0, "renders identically");
}

use box_engine::box_assets::SceneAsset;

const LAMP: &str = r#"{
  "version": 1,
  "entities": [
    { "uid": "30000000-0000-4000-8000-000000000001", "name": "Post", "parent": null,
      "components": {
        "Transform": { "translation": [0.0, 0.5, 0.0], "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [0.3, 1.0, 0.3] },
        "Primitive": { "shape": { "Cuboid": { "size": [1.0, 1.0, 1.0] } }, "cast_shadows": true },
        "StandardMaterial": { "base_color": [0.1, 0.3, 0.9, 1.0], "roughness": 0.5, "metallic": 0.0, "emissive": [0.0, 0.0, 0.0, 1.0] }
      } },
    { "uid": "30000000-0000-4000-8000-000000000002", "name": "Bulb", "parent": "30000000-0000-4000-8000-000000000001",
      "components": {
        "Transform": { "translation": [0.0, 0.7, 0.0], "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [1.5, 0.4, 1.5] },
        "Primitive": { "shape": { "Sphere": { "radius": 0.5, "sectors": 16, "stacks": 8 } }, "cast_shadows": true }
      } }
  ]
}"#;

/// A `.scene.json` instanced three times updates every instance when the
/// source file changes.
#[test]
fn scene_instanced_three_times_follows_its_source() {
    let project = TempProject::new("lamps");
    project.write("prefabs/lamp.scene.json", LAMP);
    let mut w = base_world(Vec3::new(0.0, 2.5, 5.0), Vec3::new(0.0, 0.6, 0.0));
    let mut roots = Vec::new();
    for x in [-1.6f32, 0.0, 1.6] {
        let uid = Uid::new();
        w.spawn_saved(
            uid,
            "Lamp",
            (
                Transform::from_xyz(x, 0.0, 0.0),
                PrefabInstance {
                    scene: AssetRef::path("prefabs/lamp.scene.json"),
                },
            ),
        )
        .unwrap();
        roots.push(uid);
    }
    let (mut app, server) = app(&project, w);
    assert_eq!(instanced(&app.world), 6);
    let first = sorted_instanced_uids(&app.world);
    let gpu = renderer();
    let mut gpu = gpu.map(|mut r| (capture(&mut app, &mut r, "lamps_before"), r));

    // The source gains a red cap on every post.
    let mut json: Json = serde_json::from_str(LAMP).unwrap();
    json["entities"].as_array_mut().unwrap().push(json!({
        "uid": "30000000-0000-4000-8000-000000000003", "name": "Cap",
        "parent": "30000000-0000-4000-8000-000000000001",
        "components": {
            "Transform": { "translation": [0.0, 1.05, 0.0], "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [2.0, 0.15, 2.0] },
            "Primitive": { "shape": { "Cuboid": { "size": [1.0, 1.0, 1.0] } }, "cast_shadows": true },
            "StandardMaterial": { "base_color": [0.9, 0.05, 0.05, 1.0], "roughness": 0.5, "metallic": 0.0, "emissive": [0.0, 0.0, 0.0, 1.0] }
        }
    }));
    project.write(
        "prefabs/lamp.scene.json",
        serde_json::to_vec_pretty(&json).unwrap(),
    );
    reload(&mut app, &server);
    assert_eq!(instanced(&app.world), 9, "every instance rebuilt");
    let after = sorted_instanced_uids(&app.world);
    assert!(first.iter().all(|u| after.contains(u)), "uids kept");
    for uid in &roots {
        let root = app.world.entity_by_uid(*uid).unwrap();
        let post = app.world.children(root)[0];
        assert_eq!(app.world.children(post).len(), 2, "Bulb and Cap");
    }
    if let Some((before, r)) = &mut gpu {
        let img = capture(&mut app, r, "lamps_after");
        assert!(differing(before, &img, 24) > 300);
        let red = count(&img, |p| p[0] as i32 - p[1] as i32 > 50);
        assert!(red > 100, "red caps: {red}");
    }
}

/// A malformed asset logs an error and draws the magenta placeholder; the
/// app keeps running.
#[test]
fn malformed_asset_draws_the_placeholder() {
    let project = TempProject::new("bad");
    std::fs::copy(
        fixtures().join("bad/garbage.glb"),
        project.assets().join("models/broken_model.glb"),
    )
    .unwrap();
    let mut w = base_world(Vec3::new(1.6, 1.4, 2.2), Vec3::ZERO);
    w.spawn_saved(
        Uid::new(),
        "Broken",
        (
            Transform::IDENTITY,
            MeshAsset {
                mesh: AssetRef::path("models/broken_model.glb#Mesh/Thing"),
                materials: vec![AssetRef::path("models/broken_model.glb#Material/Thing")],
                cast_shadows: true,
            },
        ),
    )
    .unwrap();
    let (mut app, server) = app(&project, w);
    assert!(matches!(
        server.state(&AssetRef::path("models/broken_model.glb#Mesh/Thing")),
        Some(LoadState::Failed(_))
    ));
    assert!(errors_about("broken_model.glb") >= 1);
    assert_eq!(app.world.query::<&MeshRenderer>().iter().count(), 1);
    let Some(mut r) = renderer() else { return };
    let img = capture(&mut app, &mut r, "malformed_placeholder");
    let magenta = count(&img, |p| {
        p[0] > 160 && p[2] > 160 && (p[0].min(p[2]) as i32 - p[1] as i32) > 50
    });
    assert!(magenta > 2000, "magenta placeholder: {magenta}");
}

/// Save repair: path-only references get their ids, stale path hints are
/// updated, as one undoable step. (No GPU needed.)
#[test]
fn canonicalize_scene_refs_fills_ids_and_fixes_paths() {
    let project = TempProject::new("canon");
    let server = project.server();
    let mut w = World::with_registry(shared_engine_registry());
    let a = Uid::new();
    w.spawn_saved(
        a,
        "A",
        (MeshAsset {
            mesh: AssetRef::path("models/tower.glb#Mesh/Roof"),
            materials: vec![
                AssetRef::NONE,
                AssetRef::path("materials/stone.material.json"),
            ],
            cast_shadows: true,
        },),
    )
    .unwrap();
    let id = server.id_of("textures/checker.png").unwrap();
    let b = Uid::new();
    w.spawn_saved(
        b,
        "B",
        (StandardMaterial {
            base_color_texture: AssetRef::id(id).with_path("old/place.png"),
            ..Default::default()
        },),
    )
    .unwrap();
    let mut edits = EditStack::new();
    edits.begin("Repair asset references");
    let n = box_engine::asset_glue::canonicalize_scene_refs(&mut w, &mut edits, &server).unwrap();
    edits.commit();
    assert_eq!(n, 2);
    let e = w.entity_by_uid(a).unwrap();
    let m = (*w.get::<MeshAsset>(e).unwrap()).clone();
    assert_eq!(m.mesh.id, server.id_of("models/tower.glb"));
    assert_eq!(m.mesh.label.as_deref(), Some("Mesh/Roof"));
    assert!(m.materials[0].is_none());
    assert_eq!(
        m.materials[1].id,
        server.id_of("materials/stone.material.json")
    );
    let e = w.entity_by_uid(b).unwrap();
    let t = w
        .get::<StandardMaterial>(e)
        .unwrap()
        .base_color_texture
        .clone();
    assert_eq!(t.path.as_deref(), Some("textures/checker.png"));
    assert_eq!(t.id, Some(id));
    // Already canonical: nothing to do.
    assert_eq!(
        box_engine::asset_glue::canonicalize_scene_refs(&mut w, &mut edits, &server).unwrap(),
        0
    );
    assert_eq!(edits.undo_len(), 1);
    edits.undo(&mut w).unwrap();
    let e = w.entity_by_uid(a).unwrap();
    assert_eq!(w.get::<MeshAsset>(e).unwrap().mesh.id, None);
}

/// Nested prefabs instantiate; a prefab that contains itself is refused
/// with an error instead of recursing. (No GPU needed.)
#[test]
fn nested_prefabs_and_cycles() {
    let project = TempProject::new("nested");
    project.write("prefabs/lamp.scene.json", LAMP);
    let pair = json!({
        "version": 1,
        "entities": [
            { "uid": "40000000-0000-4000-8000-000000000001", "name": "Left", "parent": null,
              "components": { "PrefabInstance": { "scene": "prefabs/lamp.scene.json" } } },
            { "uid": "40000000-0000-4000-8000-000000000002", "name": "Right", "parent": null,
              "components": { "PrefabInstance": { "scene": "prefabs/lamp.scene.json" } } }
        ]
    });
    project.write(
        "prefabs/pair.scene.json",
        serde_json::to_vec(&pair).unwrap(),
    );
    let looped = json!({
        "version": 1,
        "entities": [
            { "uid": "50000000-0000-4000-8000-000000000001", "name": "Again", "parent": null,
              "components": { "PrefabInstance": { "scene": "prefabs/loop.scene.json" } } }
        ]
    });
    project.write(
        "prefabs/loop.scene.json",
        serde_json::to_vec(&looped).unwrap(),
    );
    let mut w = World::with_registry(shared_engine_registry());
    w.spawn_saved(
        Uid::new(),
        "Pair",
        (
            Transform::IDENTITY,
            PrefabInstance {
                scene: AssetRef::path("prefabs/pair.scene.json"),
            },
        ),
    )
    .unwrap();
    w.spawn_saved(
        Uid::new(),
        "Loop",
        (
            Transform::IDENTITY,
            PrefabInstance {
                scene: AssetRef::path("prefabs/loop.scene.json"),
            },
        ),
    )
    .unwrap();
    let (app, _) = app(&project, w);
    // Pair: Left, Right (depth 1) and two posts with bulbs (depth 2).
    // Loop: Again (depth 1), refused one level down.
    let depths: Vec<u32> = app
        .world
        .query::<&Instanced>()
        .iter()
        .map(|i| i.depth)
        .collect();
    assert_eq!(depths.iter().filter(|d| **d == 1).count(), 3);
    assert_eq!(depths.iter().filter(|d| **d == 2).count(), 4);
    assert_eq!(depths.len(), 7);
    assert!(errors_about("contains itself") >= 1);
}
