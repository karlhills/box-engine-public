//! Assets in the editor's edit and Play worlds (M4): dropped models build
//! renderers and prefab contents through the asset glue, picking selects a
//! prefab's instance root, Play copies the loaded assets (no re-import),
//! follows hot reloads while playing, and Stop leaves the edit scene
//! byte-identical.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use box_core::assets::{AssetLookup, Assets};
use box_core::math::Ray;
use box_core::mesh::MeshData;
use box_core::prefab::Instanced;
use box_core::scene;
use box_editor::assets::AssetItem;
use box_editor::log::LogBuffer;
use box_editor::{Editor, EditorOptions, build_editor};
use box_render::{MeshRenderer, RenderError, Renderer};
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

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn project_editor(name: &str) -> (Editor, TempDir) {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "box_editor_play_assets_{name}_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    box_engine::Project::create(&dir, name).unwrap();
    for sub in ["models", "textures", "materials"] {
        copy_dir(&fixtures().join(sub), &dir.join("assets").join(sub));
    }
    let options = EditorOptions {
        project: Some(dir.clone()),
        ..Default::default()
    };
    let editor = build_editor(&options, LogBuffer::default()).unwrap();
    (editor, TempDir(dir))
}

/// What an editor frame does to the shown world (asset glue, transforms,
/// renderers), run until the assets settle.
fn settle(editor: &mut Editor) {
    assert!(editor.state.wait_assets_idle(IDLE));
    assert!(box_engine::settle_assets(&mut editor.state.world, IDLE));
    box_core::transform::propagate_transforms(&mut editor.state.world);
    box_engine::sync_scene_meshes(&mut editor.state.world);
}

fn instanced(world: &box_core::world::World) -> usize {
    world.query::<&Instanced>().iter().count()
}

fn renderer() -> Option<Renderer> {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping the captures");
        return None;
    }
    match Renderer::new_headless(640, 400) {
        Ok(r) => Some(r),
        Err(RenderError::NoAdapter) => None,
        Err(e) => panic!("{e}"),
    }
}

fn shot(r: &mut Renderer, world: &mut box_core::world::World, name: &str) -> image::RgbaImage {
    r.render(world).unwrap();
    let img = r.capture(world).unwrap();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/m4");
    std::fs::create_dir_all(&dir).unwrap();
    img.save(dir.join(format!("{name}.png"))).unwrap();
    assert_eq!(r.stats().validation_errors, 0);
    img
}

#[test]
fn dropped_models_render_pick_and_play_with_assets() {
    let (mut editor, dir) = project_editor("drop_play");
    let server = editor.state.asset_server().unwrap().clone();
    let tower = server.id_of("models/tower.glb").unwrap();
    // The browser lists a model's sub-assets once it is imported.
    editor.state.request_file(tower);
    assert!(editor.state.wait_assets_idle(IDLE));

    // A whole model (a prefab of tower.glb#Scene) and one of its meshes.
    let prefab = editor
        .state
        .drop_asset_at(&AssetItem::file(tower), Vec3::new(-3.0, 0.0, 2.0))
        .unwrap();
    let roof = editor
        .state
        .drop_asset_at(
            &AssetItem::sub(tower, "Mesh/Roof"),
            Vec3::new(3.0, 0.0, 0.0),
        )
        .unwrap();
    settle(&mut editor);
    let world = &editor.state.world;
    assert_eq!(instanced(world), 5, "tower > Tower > Base, Roof > Flag");
    let roof_e = world.entity_by_uid(roof).unwrap();
    assert!(
        world.has::<MeshRenderer>(roof_e),
        "the MeshAsset got a renderer"
    );
    assert_eq!(
        world.get::<MeshRenderer>(roof_e).unwrap().materials.len(),
        2,
        "the glTF's materials, by slot"
    );

    // Picking: a prefab's part selects the instance root; a MeshAsset
    // entity selects itself.
    let down = Vec3::NEG_Y;
    let hit = box_editor::picking::pick(world, &Ray::new(Vec3::new(-3.0, 10.0, 0.0), down))
        .expect("the tower is under the ray");
    assert_eq!(world.uid(hit.entity), Some(prefab));
    assert!(world.has::<Instanced>(hit.mesh_entity));
    let hit = box_editor::picking::pick(world, &Ray::new(Vec3::new(3.0, 10.0, 0.0), down))
        .expect("the roof is under the ray");
    assert_eq!(world.uid(hit.entity), Some(roof));

    // The hierarchy lists the instance root without its runtime contents.
    let mut rows = Vec::new();
    let expanded = [prefab].into_iter().collect();
    box_editor::panes::hierarchy::flatten(world, &expanded, "", &mut rows);
    let root_row = rows.iter().find(|r| r.uid == prefab).unwrap();
    assert_eq!(root_row.children, 0);
    assert!(rows.iter().all(|r| !world.has::<Instanced>(r.entity)));

    let mut gpu = renderer();
    let edit_img = gpu
        .as_mut()
        .map(|r| shot(r, &mut editor.state.world, "editor_edit_world_assets"));

    // Play: the snapshot holds only the roots; the play world gets the
    // loaded assets (same handles and versions) and instantiates again.
    let saved = scene::save(&editor.state.world).unwrap();
    let edit_meshes: Vec<(u64, u64)> = {
        let w = &editor.state.world;
        let lookup = w.resource::<AssetLookup>();
        let meshes = w.resource::<Assets<MeshData>>();
        lookup
            .handle_ids::<MeshData>()
            .into_iter()
            .filter_map(|id| {
                meshes
                    .iter()
                    .find(|(i, _, _)| *i == id)
                    .map(|(i, v, _)| (i, v))
            })
            .collect()
    };
    assert!(!edit_meshes.is_empty());
    let finished = server.finished_imports();
    let t0 = std::time::Instant::now();
    editor.state.start_play().unwrap();
    let play_start = t0.elapsed();
    eprintln!(
        "Play start with assets: {:.2} ms",
        play_start.as_secs_f64() * 1e3
    );
    assert_eq!(server.finished_imports(), finished, "nothing re-imported");
    let play = &editor.state.world;
    assert_eq!(instanced(play), 5);
    assert!(play.contains_resource::<box_engine::asset_glue::AssetMirror>());
    assert!(!play.contains_resource::<box_engine::box_assets::AssetServer>());
    let play_meshes = play.resource::<Assets<MeshData>>();
    for (id, version) in &edit_meshes {
        let copied = play_meshes
            .iter()
            .find(|(i, _, _)| i == id)
            .map(|(_, v, _)| v);
        assert_eq!(copied, Some(*version), "mesh {id} copied with its version");
    }
    for _ in 0..3 {
        editor.state.update_assets();
        assert!(editor.state.update_play(1.0 / 60.0).unwrap());
    }
    if let (Some(r), Some(edit_img)) = (gpu.as_mut(), &edit_img) {
        let play_img = shot(r, &mut editor.state.world, "editor_play_world_assets");
        let differing = edit_img
            .pixels()
            .zip(play_img.pixels())
            .filter(|(a, b)| a.0.iter().zip(b.0).any(|(x, y)| x.abs_diff(y) > 2))
            .count();
        assert_eq!(differing, 0, "the play world renders the same assets");
    }

    // Hot reload during Play: the server keeps serving the parked edit
    // world, and its changes reach the play world.
    let prefab_scene = serde_json::json!({
        "version": 1,
        "entities": [
            { "uid": "60000000-0000-4000-8000-000000000001", "name": "A", "parent": null,
              "components": { "Primitive": { "shape": { "Cuboid": { "size": [1.0, 1.0, 1.0] } }, "cast_shadows": true } } }
        ]
    });
    std::fs::create_dir_all(dir.0.join("assets/prefabs")).unwrap();
    std::fs::write(
        dir.0.join("assets/prefabs/p.scene.json"),
        serde_json::to_vec(&prefab_scene).unwrap(),
    )
    .unwrap();
    // The game spawns an instance at runtime (loaded through the mirror).
    let runtime = editor.state.world.spawn((
        box_core::transform::Transform::default(),
        box_core::prefab::PrefabInstance {
            scene: box_core::asset_ref::AssetRef::path("prefabs/p.scene.json"),
        },
    ));
    for _ in 0..200 {
        editor.state.update_assets();
        editor.state.update_play(1.0 / 60.0).unwrap();
        if editor.state.world.children(runtime).len() == 1 {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        editor.state.world.children(runtime).len(),
        1,
        "loaded during Play"
    );
    let mut two = prefab_scene.clone();
    two["entities"].as_array_mut().unwrap().push(serde_json::json!(
        { "uid": "60000000-0000-4000-8000-000000000002", "name": "B", "parent": null,
          "components": { "Primitive": { "shape": { "Sphere": { "radius": 0.5, "sectors": 8, "stacks": 4 } }, "cast_shadows": true } } }
    ));
    std::fs::write(
        dir.0.join("assets/prefabs/p.scene.json"),
        serde_json::to_vec_pretty(&two).unwrap(),
    )
    .unwrap();
    server.rescan();
    for _ in 0..200 {
        editor.state.update_assets();
        editor.state.update_play(1.0 / 60.0).unwrap();
        if editor.state.world.children(runtime).len() == 2 {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        editor.state.world.children(runtime).len(),
        2,
        "hot reload reached the play world"
    );

    // Stop: the edit scene is byte-identical.
    editor.state.stop_play();
    assert_eq!(scene::save(&editor.state.world).unwrap(), saved);
    assert_eq!(instanced(&editor.state.world), 5);
}

/// Play-start cost with assets (printed; run with `--release --nocapture`
/// for the numbers in docs/measurements): the collector sample's arena,
/// and a scene holding a large loaded model (a 1M-vertex mesh and a
/// 2048x2048 texture registered as asset-server assets).
#[test]
fn play_start_cost_with_assets() {
    use box_core::asset_ref::{AssetId, AssetRef};
    use box_core::image::ImageData;
    use box_core::mesh_asset::MeshAsset;

    // The collector sample (primitives only), opened as a project.
    let dir = std::env::temp_dir().join(format!("box_play_cost_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/collector");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(
        sample.join("box.project.json"),
        dir.join("box.project.json"),
    )
    .unwrap();
    copy_dir(&sample.join("assets"), &dir.join("assets"));
    let _cleanup = TempDir(dir.clone());
    let options = EditorOptions {
        project: Some(dir.clone()),
        ..Default::default()
    };
    let mut editor = build_editor(&options, LogBuffer::default()).unwrap();
    settle(&mut editor);
    let n = editor.state.world.entity_count();
    let t = std::time::Instant::now();
    editor.state.start_play().unwrap();
    let collector = t.elapsed();
    editor.state.stop_play();

    // A large model: what the asset server would have stored.
    let mesh = MeshData::grid(100.0, 100.0, 999, 999);
    let vertices = mesh.positions.len();
    let mut image = ImageData {
        width: 2048,
        height: 2048,
        color_space: box_core::image::ColorSpace::Srgb,
        mips: vec![vec![128u8; 2048 * 2048 * 4]],
    };
    image.generate_mips();
    let id = AssetId::new();
    {
        let w = &mut editor.state.world;
        let mh = w.resource_or_default::<Assets<MeshData>>().add(mesh);
        let ih = w.resource_or_default::<Assets<ImageData>>().add(image);
        let lookup = w.resource_or_default::<AssetLookup>();
        lookup.insert(id, Some("Mesh/Big"), Some("models/big.glb"), mh);
        lookup.insert(id, Some("Texture/0"), Some("models/big.glb"), ih);
        w.spawn_saved(
            box_core::identity::Uid::new(),
            "Big",
            (
                box_core::transform::Transform::default(),
                MeshAsset {
                    mesh: AssetRef::id(id).with_label("Mesh/Big"),
                    materials: Vec::new(),
                    cast_shadows: true,
                },
            ),
        )
        .unwrap();
    }
    box_engine::sync_scene_meshes(&mut editor.state.world);
    let t = std::time::Instant::now();
    editor.state.start_play().unwrap();
    let large = t.elapsed();
    let play = &editor.state.world;
    assert_eq!(
        play.query::<(&MeshRenderer, &MeshAsset)>().iter().count(),
        1
    );
    editor.state.stop_play();
    eprintln!(
        "Play start: collector arena ({n} entities) {:.2} ms; with a {vertices}-vertex mesh and a 2048^2 texture {:.2} ms",
        collector.as_secs_f64() * 1e3,
        large.as_secs_f64() * 1e3
    );
}
