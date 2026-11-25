//! M5 generator runtime in the engine: renderers and materials by slot,
//! scenes that store only parameters, golden output hashes, bake to mesh,
//! and the latency benchmark (parameter edit through `box_core::edit` to a
//! mesh the renderer will upload).
//!
//! `cargo test --release -p box_engine --test procedural -- --nocapture`
//! prints the latency numbers.

#[allow(dead_code)]
#[path = "../examples/common/steps.rs"]
mod steps;

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use box_engine::box_assets::AssetServer;
use box_engine::box_core::asset_ref::AssetId;
use box_engine::box_core::edit::EditStack;
use box_engine::box_core::scene;
use box_engine::box_proc::golden::{GoldenCase, assert_golden};
use box_engine::box_proc::{ProcClock, ProcRuntime, mesh_hash};
use box_engine::prelude::*;
use box_engine::{Game, Project, settle_assets, sync_scene_meshes};
use steps::{Steps, StepsPlugin};

fn game() -> Game {
    Game::new().with_plugin(StepsPlugin)
}

/// A world with the game's registry and a manual throttle clock.
fn world(game: &Game) -> World {
    let mut w = game.new_world();
    w.insert_resource(ProcSettings {
        clock: ProcClock::Manual(Duration::ZERO),
        ..ProcSettings::default()
    });
    w
}

fn advance(w: &mut World, by: Duration) {
    let s = w.resource_mut::<ProcSettings>();
    if let ProcClock::Manual(t) = &mut s.clock {
        *t += by;
    }
}

fn renderer(w: &World, e: Entity) -> MeshRenderer {
    (*w.get::<MeshRenderer>(e).expect("MeshRenderer")).clone()
}

fn drawn_hash(w: &World, e: Entity) -> u64 {
    let r = renderer(w, e);
    mesh_hash(w.resource::<Assets<MeshData>>().get(r.mesh).unwrap())
}

#[test]
fn generator_in_a_native_project_is_registered_and_drawn() {
    let game = game();
    let registry = game.registry();
    let g = registry.generator("Steps").expect("listed");
    assert_eq!(g.material_slots(), ["Tread", "Riser"]);
    assert_eq!(g.category(), "Procedural");
    assert!(registry.component("Procedural").is_some());

    let mut app = game.new_app();
    let e = app.world.spawn((Transform::default(), Steps::default()));
    app.tick(1.0 / 60.0);
    let r = renderer(&app.world, e);
    assert_eq!(r.materials.len(), 2, "one material per slot");
    assert!(r.cast_shadows);
    let mesh = app
        .world
        .resource::<Assets<MeshData>>()
        .get(r.mesh)
        .unwrap();
    assert_eq!(mesh.triangle_count(), 6 * 6 * 2);
}

#[test]
fn materials_follow_slots_entity_material_and_links() {
    let game = game();
    let mut w = world(&game);
    let red = StandardMaterial {
        base_color: Color::RED,
        ..Default::default()
    };
    let e = w.spawn((Steps::default(), red.clone()));
    sync_scene_meshes(&mut w);
    let r = renderer(&w, e);
    let [MaterialHandle::Standard(inline), b] = r.materials[..] else {
        panic!("{:?}", r.materials)
    };
    assert_eq!(
        b,
        MaterialHandle::Standard(inline),
        "both slots: the inline material"
    );
    assert_eq!(
        w.resource::<Assets<StandardMaterial>>().get(inline),
        Some(&red)
    );

    // Idle frames change nothing.
    let v = w.resource::<Assets<StandardMaterial>>().version(inline);
    for _ in 0..3 {
        sync_scene_meshes(&mut w);
    }
    assert_eq!(renderer(&w, e), r);
    assert_eq!(w.resource::<Assets<StandardMaterial>>().version(inline), v);

    // Slot 1 gets a material asset; slot 0 keeps the entity's material.
    let stone = w
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            base_color: Color::GREEN,
            ..Default::default()
        });
    let id = AssetId::from_u128(7);
    let mut lookup = AssetLookup::default();
    lookup.insert(id, None, Some("materials/stone.material.json"), stone);
    w.insert_resource(lookup);
    w.insert_one(
        e,
        Procedural {
            materials: vec![
                AssetRef::NONE,
                AssetRef::path("materials/stone.material.json"),
            ],
            ..Procedural::default()
        },
    )
    .unwrap();
    sync_scene_meshes(&mut w);
    assert_eq!(
        renderer(&w, e).materials,
        [
            MaterialHandle::Standard(inline),
            MaterialHandle::Standard(stone)
        ]
    );
    let mesh_before = renderer(&w, e).mesh;
    let mesh_version = w.resource::<Assets<MeshData>>().version(mesh_before);

    // A MaterialRef replaces the entity material in every unassigned slot.
    w.insert_one(e, MaterialRef::new(AssetRef::id(id))).unwrap();
    sync_scene_meshes(&mut w);
    assert_eq!(
        renderer(&w, e).materials,
        [
            MaterialHandle::Standard(stone),
            MaterialHandle::Standard(stone)
        ]
    );

    // A slot asset that does not exist: the placeholder, one for all.
    w.get_mut::<Procedural>(e).unwrap().materials[1] = AssetRef::path("missing.material.json");
    sync_scene_meshes(&mut w);
    let MaterialHandle::Standard(placeholder) = renderer(&w, e).materials[1] else {
        panic!()
    };
    assert_ne!(placeholder, stone);
    assert_ne!(placeholder, inline);

    // Inline edits rewrite the owned asset in place, never the mesh.
    w.remove_one::<MaterialRef>(e).unwrap();
    w.get_mut::<StandardMaterial>(e).unwrap().roughness = 0.2;
    sync_scene_meshes(&mut w);
    assert_eq!(
        renderer(&w, e).materials[0],
        MaterialHandle::Standard(inline)
    );
    assert_eq!(
        w.resource::<Assets<StandardMaterial>>()
            .get(inline)
            .unwrap()
            .roughness,
        0.2
    );
    assert_eq!(
        w.resource::<Assets<MeshData>>().version(mesh_before),
        mesh_version,
        "material edits never regenerate"
    );

    // Shadows.
    w.get_mut::<Procedural>(e).unwrap().cast_shadows = false;
    sync_scene_meshes(&mut w);
    assert!(!renderer(&w, e).cast_shadows);

    // Removing the generator removes the renderer and frees what the glue
    // and the runtime own (the linked asset and the placeholder stay).
    w.remove_one::<Steps>(e).unwrap();
    sync_scene_meshes(&mut w);
    assert!(!w.has::<MeshRenderer>(e));
    assert!(w.resource::<Assets<MeshData>>().is_empty());
    let materials = w.resource::<Assets<StandardMaterial>>();
    assert!(materials.get(inline).is_none());
    assert!(materials.get(stone).is_some());
    assert_eq!(materials.len(), 2, "the linked asset and the placeholder");
}

#[test]
fn despawning_frees_the_mesh_and_material() {
    let game = game();
    let mut w = world(&game);
    let e = w.spawn((Steps::default(),));
    sync_scene_meshes(&mut w);
    assert_eq!(w.resource::<Assets<MeshData>>().len(), 1);
    w.despawn(e);
    sync_scene_meshes(&mut w);
    assert!(w.resource::<Assets<MeshData>>().is_empty());
    assert!(w.resource::<Assets<StandardMaterial>>().is_empty());
}

#[test]
fn scenes_store_parameters_only_and_reload_the_same_geometry() {
    let game = game();
    let mut w = world(&game);
    let e = w
        .spawn_saved(
            Uid::new(),
            "Stairs",
            (
                Transform::default(),
                Steps {
                    count: 9,
                    wobble: 0.05,
                    ..Steps::default()
                },
                Procedural::with_seed(11),
            ),
        )
        .unwrap();
    sync_scene_meshes(&mut w);
    let before = drawn_hash(&w, e);
    let text = scene::save_string(&w).unwrap();
    assert!(text.contains("\"Steps\"") && text.contains("\"wobble\""));
    assert!(text.contains("\"seed\": 11"));
    for runtime in ["ProcState", "MeshRenderer", "positions", "indices"] {
        assert!(!text.contains(runtime), "{runtime} saved:\n{text}");
    }
    let mut loaded = scene::load(text.as_bytes(), game.registry()).unwrap();
    sync_scene_meshes(&mut loaded);
    let e2 = loaded.find_by_name("Stairs").unwrap();
    assert_eq!(drawn_hash(&loaded, e2), before);
}

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/generators")
}

/// Changing `Steps` output without bumping `Steps::VERSION` fails here.
#[test]
fn steps_golden_hashes() {
    assert_golden::<Steps>(
        golden_dir(),
        &[
            GoldenCase::new("default", Steps::default(), 0),
            GoldenCase::new(
                "wobbly",
                Steps {
                    count: 10,
                    width: 1.2,
                    wobble: 0.04,
                    ..Steps::default()
                },
                3,
            ),
            GoldenCase::new(
                "clamped",
                Steps {
                    count: 500,
                    rise: -1.0,
                    ..Steps::default()
                },
                0,
            ),
        ],
    );
}

// ----------------------------------------------------------------- bake ----

struct TempProject(PathBuf);

impl TempProject {
    fn new(name: &str) -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "box_engine_proc_{name}_{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        Project::create(&root, "Proc Test").unwrap();
        Self(root)
    }

    fn assets(&self) -> PathBuf {
        self.0.join("assets")
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn bake_writes_a_glb_and_swaps_the_generator_in_one_undo_step() {
    let project = TempProject::new("bake");
    let server = AssetServer::new(project.assets()).unwrap();
    let game = game();
    let mut w = world(&game);
    w.insert_resource(server.clone());
    let uid = Uid::new();
    let e = w
        .spawn_saved(
            uid,
            "Front Stairs",
            (
                Transform::default(),
                Steps {
                    count: 4,
                    wobble: 0.03,
                    ..Steps::default()
                },
                Procedural::with_seed(5),
                StandardMaterial {
                    base_color: Color::hex(0xb07a5a),
                    roughness: 0.6,
                    ..Default::default()
                },
            ),
        )
        .unwrap();
    sync_scene_meshes(&mut w);
    let generated = drawn_hash(&w, e);
    let scene_before = scene::save_string(&w).unwrap();

    let mut edits = EditStack::new();
    let path = box_engine::bake::bake_path(&w, &server, uid);
    assert_eq!(path, "baked/Front Stairs.glb");
    let baked = box_engine::bake::bake_to_mesh(&mut w, &mut edits, &server, uid, &path).unwrap();
    assert!(baked.warnings.is_empty(), "{:?}", baked.warnings);
    assert!(project.assets().join(&path).is_file());
    assert!(!w.has::<Steps>(e) && !w.has::<Procedural>(e));
    let asset = (*w.get::<MeshAsset>(e).unwrap()).clone();
    assert_eq!(asset.mesh.label.as_deref(), Some("Mesh/Front_Stairs"));
    assert_eq!(
        asset
            .materials
            .iter()
            .map(|m| m.label.clone().unwrap_or_default())
            .collect::<Vec<_>>(),
        ["Material/Tread", "Material/Riser"]
    );
    assert_eq!(edits.undo_len(), 1, "one undo step");
    assert_eq!(edits.undo_label(), Some("Bake to Mesh"));

    // Reload: the asset server imports the file; the drawn mesh is the
    // generated one (quantized hash), with the baked materials.
    assert!(settle_assets(&mut w, Duration::from_secs(30)));
    let r = renderer(&w, e);
    let loaded = w
        .resource::<Assets<MeshData>>()
        .get(r.mesh)
        .expect("imported mesh");
    assert_eq!(
        mesh_hash(loaded),
        generated,
        "bake -> reload changes the mesh"
    );
    let MaterialHandle::Standard(m) = r.materials[0] else {
        panic!()
    };
    let m = w.resource::<Assets<StandardMaterial>>().get(m).unwrap();
    assert!((m.roughness - 0.6).abs() < 1e-6);

    // Undo brings the generator back, byte-identical, and it draws again.
    edits.undo(&mut w).unwrap();
    assert!(w.has::<Steps>(e) && !w.has::<MeshAsset>(e));
    assert_eq!(scene::save_string(&w).unwrap(), scene_before);
    advance(&mut w, Duration::from_secs(1));
    sync_scene_meshes(&mut w);
    assert_eq!(drawn_hash(&w, e), generated);
    edits.redo(&mut w).unwrap();
    assert!(w.has::<MeshAsset>(e) && !w.has::<Steps>(e));

    // Something without a generator cannot be baked.
    let plain = Uid::new();
    w.spawn_saved(plain, "Plain", (Transform::default(),))
        .unwrap();
    assert!(matches!(
        box_engine::bake::bake_to_mesh(&mut w, &mut edits, &server, plain, "baked/plain.glb"),
        Err(box_engine::bake::BakeError::NotProcedural)
    ));
}

/// Primitives and lathes go through the generator runtime, so they bake
/// like any generator: the reloaded `.glb` draws the generated mesh (same
/// quantized hash), a primitive keeps its shadow flag, and a collider that
/// followed the primitive keeps its shape.
#[test]
fn primitives_and_lathes_bake_and_reload_the_same_mesh() {
    let project = TempProject::new("bake_builtin");
    let server = AssetServer::new(project.assets()).unwrap();
    let game = game();
    let mut w = world(&game);
    w.insert_resource(server.clone());
    let red = StandardMaterial {
        base_color: Color::hex(0xc04030),
        ..Default::default()
    };
    let column = Uid::new();
    let ball = Uid::new();
    w.spawn_saved(
        column,
        "Column",
        (
            Transform::default(),
            Lathe {
                shape: LatheShape::Column {
                    height: 2.0,
                    radius: 0.2,
                },
                sweep_angle: 4.0,
                ..Lathe::default()
            },
            red.clone(),
        ),
    )
    .unwrap();
    w.spawn_saved(
        ball,
        "Ball",
        (
            Transform::from_xyz(2.0, 0.0, 0.0),
            Primitive::sphere(0.4).with_shadows(false),
            red,
        ),
    )
    .unwrap();
    #[cfg(feature = "physics")]
    {
        let e = w.entity_by_uid(ball).unwrap();
        w.insert_one(e, Collider::from_primitive()).unwrap();
    }
    sync_scene_meshes(&mut w);
    let mut edits = EditStack::new();
    let mut generated = Vec::new();
    for (uid, slots) in [(column, 2), (ball, 1)] {
        let e = w.entity_by_uid(uid).unwrap();
        generated.push(drawn_hash(&w, e));
        let path = box_engine::bake::bake_path(&w, &server, uid);
        let baked =
            box_engine::bake::bake_to_mesh(&mut w, &mut edits, &server, uid, &path).unwrap();
        assert!(baked.warnings.is_empty(), "{:?}", baked.warnings);
        assert_eq!(baked.mesh_asset.materials.len(), slots, "{path}");
    }
    let (ce, be) = (
        w.entity_by_uid(column).unwrap(),
        w.entity_by_uid(ball).unwrap(),
    );
    assert!(!w.has::<Lathe>(ce) && !w.has::<Primitive>(be));
    assert!(!w.get::<MeshAsset>(be).unwrap().cast_shadows);
    #[cfg(feature = "physics")]
    assert_eq!(
        w.get::<Collider>(be).unwrap().shape,
        ColliderShape::Sphere { radius: 0.4 },
        "the collider keeps the primitive's shape"
    );
    assert!(settle_assets(&mut w, Duration::from_secs(30)));
    for (e, hash) in [ce, be].into_iter().zip(&generated) {
        let r = renderer(&w, e);
        let loaded = w.resource::<Assets<MeshData>>().get(r.mesh).unwrap();
        assert_eq!(mesh_hash(loaded), *hash, "bake -> reload changes the mesh");
    }
    assert!(!renderer(&w, be).cast_shadows);

    // Undo (one step per bake) restores both, collider included.
    edits.undo(&mut w).unwrap();
    edits.undo(&mut w).unwrap();
    assert!(w.has::<Lathe>(ce) && w.has::<Primitive>(be));
    #[cfg(feature = "physics")]
    assert_eq!(
        w.get::<Collider>(be).unwrap().shape,
        ColliderShape::Primitive
    );
}

// -------------------------------------------------------------- latency ----

/// Edits `Steps.count` of `uid` through the edit stack (as the inspector
/// does) and returns the frames (sync calls) and wall time until the
/// renderer's mesh asset has a new version.
fn edit_to_visible(
    w: &mut World,
    edits: &mut EditStack,
    uids: &[Uid],
    value: u32,
) -> (u32, Duration) {
    let entities: Vec<Entity> = uids.iter().map(|u| w.entity_by_uid(*u).unwrap()).collect();
    let versions: Vec<u64> = entities
        .iter()
        .map(|e| {
            let r = renderer(w, *e);
            w.resource::<Assets<MeshData>>().version(r.mesh).unwrap()
        })
        .collect();
    let path = w.field_path("Steps.count").unwrap();
    let start = Instant::now();
    for uid in uids {
        edits.set_field(w, *uid, &path, &value).unwrap();
    }
    for frame in 1..=600 {
        sync_scene_meshes(w);
        let visible = entities.iter().zip(&versions).all(|(e, v)| {
            let r = renderer(w, *e);
            w.resource::<Assets<MeshData>>().version(r.mesh) > Some(*v)
        });
        if visible {
            return (frame, start.elapsed());
        }
        advance(w, Duration::from_micros(16_667));
    }
    panic!("never visible");
}

fn spawn_flights(w: &mut World, n: usize) -> Vec<Uid> {
    (0..n)
        .map(|i| {
            let uid = Uid::new();
            w.spawn_saved(
                uid,
                &format!("Steps {i}"),
                (
                    Transform::from_xyz((i % 32) as f32 * 2.0, 0.0, (i / 32) as f32 * 3.0),
                    Steps::default(),
                ),
            )
            .unwrap();
            uid
        })
        .collect()
}

/// The M5 latency benchmark. Prints a table; asserts the properties the
/// acceptance criteria need (one frame for an edit after a pause, a drag
/// regenerating about every other 60 Hz frame with the last value landing,
/// frames under 16 ms, one stall allowed: `FrameTimes`).
#[test]
fn latency_benchmark() {
    let game = game();
    println!(
        "\nM5 latency: parameter edit (EditStack::set_field) -> regenerated mesh visible to the renderer"
    );
    println!("{:>6} {:>10} {:>8} {:>10}", "N", "edited", "frames", "ms");
    for n in [1usize, 100, 1000] {
        let mut w = world(&game);
        let uids = spawn_flights(&mut w, n);
        box_engine::box_proc::settle(&mut w);
        sync_scene_meshes(&mut w);
        let mut edits = EditStack::new();

        // One object edited after a pause.
        advance(&mut w, Duration::from_secs(1));
        let (frames, time) = edit_to_visible(&mut w, &mut edits, &uids[..1], 9);
        println!(
            "{n:>6} {:>10} {frames:>8} {:>10.3}",
            1,
            time.as_secs_f64() * 1e3
        );
        assert_eq!(frames, 1, "an edit after a pause shows in the same frame");

        // Every object edited at once (a multi-selection edit).
        advance(&mut w, Duration::from_secs(1));
        let (frames, time) = edit_to_visible(&mut w, &mut edits, &uids, 7);
        println!(
            "{n:>6} {n:>10} {frames:>8} {:>10.3}",
            time.as_secs_f64() * 1e3
        );

        // Idle cost of the whole sync (stamp + renderers) at this size.
        let start = Instant::now();
        for _ in 0..100 {
            sync_scene_meshes(&mut w);
        }
        println!(
            "{n:>6} {:>10} {:>8} {:>10.3}   (idle sync_scene_meshes per frame)",
            0,
            "-",
            start.elapsed().as_secs_f64() * 1e3 / 100.0
        );
    }

    // A one-second slider drag at 60 Hz on one object among 1,000, through
    // the edit stack in one transaction, as the inspector drags.
    let mut w = world(&game);
    let uids = spawn_flights(&mut w, 1000);
    box_engine::box_proc::settle(&mut w);
    sync_scene_meshes(&mut w);
    let e = w.entity_by_uid(uids[0]).unwrap();
    let path = w.field_path("Steps.rise").unwrap();
    let mut edits = EditStack::new();
    edits.begin("Drag rise");
    let before = w.get::<ProcState>(e).unwrap().generations();
    let mut times = FrameTimes::default();
    let mut last = 0.0f32;
    advance(&mut w, Duration::from_secs(1));
    for frame in 0..60 {
        last = 0.18 + frame as f32 * 0.002;
        let start = Instant::now();
        edits.set_field(&mut w, uids[0], &path, &last).unwrap();
        sync_scene_meshes(&mut w);
        times.push(start.elapsed());
        advance(&mut w, Duration::from_micros(16_667));
    }
    edits.commit();
    let during = w.get::<ProcState>(e).unwrap().generations() - before;
    let mut landed_after = None;
    for frame in 1..=3 {
        sync_scene_meshes(&mut w);
        let top = {
            let r = renderer(&w, e);
            w.resource::<Assets<MeshData>>()
                .get(r.mesh)
                .unwrap()
                .bounds()
                .max
                .y
        };
        if (top - last * 6.0).abs() < 1e-4 {
            landed_after = Some(frame);
            break;
        }
        advance(&mut w, Duration::from_micros(16_667));
    }
    println!(
        "drag: 60 frames, {during} regenerations, worst frame {:.3} ms (next {:.3} ms), last value visible {} frame(s) after the drag",
        times.worst().as_secs_f64() * 1e3,
        times.second_worst().as_secs_f64() * 1e3,
        landed_after.map_or("never".into(), |f| f.to_string())
    );
    assert!((29..=31).contains(&during), "{during}");
    assert!(landed_after.is_some(), "the last value lands");
    assert!(!box_engine::box_proc::pending(&w), "nothing left queued");
    assert_eq!(edits.undo_len(), 1, "the drag is one undo step");
    times.assert_within_budget("drag rise");
    assert!(w.resource::<ProcRuntime>().stats().generated >= 1000);
}

/// Frame times of a drag. One frame may miss the budget: a test thread can
/// be descheduled for a frame while other test binaries load the machine. A
/// slow regeneration slows every regenerating frame (about half of them), so
/// the second-worst frame still catches it.
#[derive(Default)]
struct FrameTimes(Vec<Duration>);

impl FrameTimes {
    const BUDGET: Duration = Duration::from_millis(16);

    fn push(&mut self, t: Duration) {
        self.0.push(t);
    }

    fn sorted(&self) -> Vec<Duration> {
        let mut v = self.0.clone();
        v.sort_unstable_by(|a, b| b.cmp(a));
        v
    }

    fn worst(&self) -> Duration {
        self.sorted().first().copied().unwrap_or_default()
    }

    fn second_worst(&self) -> Duration {
        self.sorted().get(1).copied().unwrap_or_default()
    }

    fn assert_within_budget(&self, what: &str) {
        let over: Vec<Duration> = self
            .sorted()
            .into_iter()
            .take_while(|t| *t >= Self::BUDGET)
            .collect();
        assert!(
            over.len() <= 1,
            "{what}: {} drag frames took {:?} or more: {over:?}",
            over.len(),
            Self::BUDGET
        );
    }
}

/// The M5 acceptance drag on the built-in generators: one second of a 60 Hz
/// slider drag (one `EditStack` transaction, as the inspector drags) on a
/// lathe or an extrusion among 200 others. Prints the worst frame, the
/// regenerations and the generation time; asserts frames under 16 ms (one stall
/// allowed: `FrameTimes`) and that the last value lands.
#[test]
fn lathe_and_extrude_drag_latency() {
    enum Shape {
        Lathe(Lathe),
        Extrude(Extrude),
    }
    struct Case {
        name: &'static str,
        shape: Shape,
        path: &'static str,
        values: fn(usize) -> f32,
    }
    let cases = [
        Case {
            name: "lathe vase, sweep_angle",
            shape: Shape::Lathe(Lathe::default()),
            path: "Lathe.sweep_angle",
            values: |f| 1.0 + f as f32 * 0.08,
        },
        Case {
            name: "lathe vase, height",
            shape: Shape::Lathe(Lathe::default()),
            path: "Lathe.shape.height",
            values: |f| 1.0 + f as f32 * 0.01,
        },
        Case {
            name: "lathe vase 256 segments, height",
            shape: Shape::Lathe(Lathe {
                segments: 256,
                ..Lathe::default()
            }),
            path: "Lathe.shape.height",
            values: |f| 1.0 + f as f32 * 0.01,
        },
        Case {
            name: "extrude hole + bevel, depth",
            shape: Shape::Extrude(Extrude {
                hole: Hole::Circle {
                    radius: 0.25,
                    segments: 32,
                    offset: Vec2::ZERO,
                },
                bevel_width: 0.05,
                bevel_segments: 4,
                ..Extrude::default()
            }),
            path: "Extrude.depth",
            values: |f| 0.3 + f as f32 * 0.01,
        },
    ];
    println!("\nM5 drag latency (60 Hz for 1 s, 1 of 200 objects)");
    println!(
        "{:<34} {:>6} {:>10} {:>10} {:>8} {:>7}",
        "case", "regens", "worst ms", "gen ms", "tris", "landed"
    );
    let game = game();
    for case in cases {
        let mut w = world(&game);
        let mut uids = Vec::new();
        for i in 0..200 {
            let uid = Uid::new();
            let t = Transform::from_xyz((i % 20) as f32 * 2.0, 0.0, (i / 20) as f32 * 2.0);
            let name = format!("Obj {i}");
            match &case.shape {
                Shape::Lathe(l) => w.spawn_saved(uid, &name, (t, l.clone())),
                Shape::Extrude(x) => w.spawn_saved(uid, &name, (t, x.clone())),
            }
            .unwrap();
            uids.push(uid);
        }
        box_engine::box_proc::settle(&mut w);
        sync_scene_meshes(&mut w);
        let e = w.entity_by_uid(uids[0]).unwrap();
        let path = w.field_path(case.path).unwrap();
        let mut edits = EditStack::new();
        edits.begin("Drag");
        let before = w.get::<ProcState>(e).unwrap().generations();
        let mut times = FrameTimes::default();
        let mut gen_worst = Duration::ZERO;
        advance(&mut w, Duration::from_secs(1));
        for frame in 0..60 {
            let value = (case.values)(frame);
            let generations = w.get::<ProcState>(e).unwrap().generations();
            let start = Instant::now();
            edits.set_field(&mut w, uids[0], &path, &value).unwrap();
            sync_scene_meshes(&mut w);
            times.push(start.elapsed());
            let s = w.get::<ProcState>(e).unwrap();
            if s.generations() > generations {
                gen_worst = gen_worst.max(s.last_generate().unwrap_or_default());
            }
            drop(s);
            advance(&mut w, Duration::from_micros(16_667));
        }
        edits.commit();
        let during = w.get::<ProcState>(e).unwrap().generations() - before;
        // What the last dragged value generates.
        let ctx = GenContext::new(0);
        let expected = match (w.get::<Lathe>(e), w.get::<Extrude>(e)) {
            (Some(l), _) => l.generate(&ctx),
            (None, Some(x)) => x.generate(&ctx),
            _ => unreachable!(),
        }
        .mesh
        .map(|m| mesh_hash(&m));
        let mut landed = None;
        for frame in 1..=3 {
            sync_scene_meshes(&mut w);
            if Some(drawn_hash(&w, e)) == expected {
                landed = Some(frame);
                break;
            }
            advance(&mut w, Duration::from_micros(16_667));
        }
        let tris = {
            let r = renderer(&w, e);
            w.resource::<Assets<MeshData>>()
                .get(r.mesh)
                .unwrap()
                .triangle_count()
        };
        println!(
            "{:<34} {during:>6} {:>10.3} {:>10.3} {tris:>8} {:>7}",
            case.name,
            times.worst().as_secs_f64() * 1e3,
            gen_worst.as_secs_f64() * 1e3,
            landed.map_or("never".into(), |f| f.to_string())
        );
        assert!((29..=31).contains(&during), "{}: {during}", case.name);
        assert!(landed.is_some(), "{}: the last value lands", case.name);
        assert_eq!(edits.undo_len(), 1, "the drag is one undo step");
        times.assert_within_budget(case.name);
    }
}

// ---------------------------------------------------------------- rules ----

/// Saved intent: a landing with a flight of steps in front of it.
#[derive(Clone, Debug, PartialEq)]
struct Landing {
    steps: u32,
}

fn landing_rule(
    input: &box_engine::box_proc::rule::RuleInput<Landing>,
) -> box_engine::box_proc::rule::RuleOutput {
    use box_engine::box_proc::derive::Part;
    let mut out = box_engine::box_proc::rule::RuleOutput::default();
    for i in input.intents {
        out.push(Part::new(i.key("flight")).with(Steps {
            count: i.value.steps,
            ..Steps::default()
        }));
    }
    out
}

struct Landings;

impl Plugin for Landings {
    fn build(&self, app: &mut App) {
        use box_engine::box_proc::rule::RuleAppExt;
        app.add_rule("landings", landing_rule);
    }
}

/// The rule's part and its triangle count.
fn part_steps(w: &World) -> (Entity, usize) {
    let e = w
        .query::<(Entity, &box_engine::box_core::derived::Derived)>()
        .iter()
        .next()
        .map(|(e, _)| e)
        .expect("a part");
    let r = renderer(w, e);
    let tris = w
        .resource::<Assets<MeshData>>()
        .get(r.mesh)
        .unwrap()
        .triangle_count();
    (e, tris)
}

/// Parts a rule produces generate in the same frame, in an app.
#[test]
fn rule_parts_generate_in_the_same_frame() {
    let game = Game::new().with_plugin(StepsPlugin).with_plugin(Landings);
    let mut app = game.new_app();
    app.world.insert_resource(ProcSettings {
        clock: ProcClock::Manual(Duration::ZERO),
        ..ProcSettings::default()
    });
    let owner = app
        .world
        .spawn((Uid::new(), Transform::default(), Landing { steps: 3 }));
    app.update();
    assert_eq!(part_steps(&app.world).1, 3 * 12, "drawn on the first frame");
    app.world.get_mut::<Landing>(owner).unwrap().steps = 5;
    advance(&mut app.world, Duration::from_secs(1));
    app.update();
    assert_eq!(
        part_steps(&app.world).1,
        5 * 12,
        "regenerated the same frame"
    );
}

/// A rule that places a marker part at each landing's world position.
fn marker_rule(
    input: &box_engine::box_proc::rule::RuleInput<Landing>,
) -> box_engine::box_proc::rule::RuleOutput {
    use box_engine::box_proc::derive::Part;
    let mut out = box_engine::box_proc::rule::RuleOutput::default();
    for i in input.intents {
        let at = i.transform.translation;
        out.push(Part::new(i.key("marker")).with(Transform::from_xyz(at.x, at.y, at.z)));
    }
    out
}

struct Markers;

impl Plugin for Markers {
    fn build(&self, app: &mut App) {
        use box_engine::box_proc::rule::RuleAppExt;
        app.add_rule("markers", marker_rule);
    }
}

/// `settle_step` (the startup splash's loading, and `settle_assets`) runs
/// rules on current world transforms: a child intent spawned since the last
/// propagation is seen where it is, not at the origin.
#[test]
fn settle_step_runs_rules_on_current_world_transforms() {
    let game = Game::new().with_plugin(Markers);
    let mut app = game.new_app();
    let parent = app
        .world
        .spawn((Uid::new(), Transform::from_xyz(4.0, 2.0, 0.0)));
    let child = app.world.spawn((
        Uid::new(),
        Transform::from_xyz(1.0, 0.0, 0.0),
        Landing { steps: 1 },
    ));
    app.world.set_parent(child, Some(parent)).unwrap();
    box_engine::settle_step(&mut app.world);
    let marker = app
        .world
        .query::<(&box_engine::box_core::derived::Derived, &Transform)>()
        .iter()
        .map(|(_, t)| t.translation)
        .next()
        .expect("a marker part");
    assert_eq!(marker, Vec3::new(5.0, 2.0, 0.0));
}

/// A world without an `App` (the editor's edit world) runs the game's rules
/// through `sync_scene_meshes` once `Game::rules` is installed.
#[test]
fn rules_run_in_a_world_without_an_app() {
    let game = Game::new().with_plugin(StepsPlugin).with_plugin(Landings);
    let mut w = world(&game);
    w.insert_resource(game.rules());
    // The game's rule and the two engine rules for terrain.
    assert_eq!(
        w.resource::<box_engine::box_proc::rule::RuleRegistry>()
            .len(),
        3
    );
    w.spawn((Uid::new(), Transform::default(), Landing { steps: 2 }));
    box_engine::box_core::transform::propagate_transforms(&mut w);
    sync_scene_meshes(&mut w);
    let (e, tris) = part_steps(&w);
    assert_eq!(tris, 2 * 12);
    // Nothing changed: the part is neither rewritten nor regenerated.
    let generations = w.get::<ProcState>(e).unwrap().generations();
    for _ in 0..3 {
        advance(&mut w, Duration::from_secs(1));
        sync_scene_meshes(&mut w);
    }
    assert_eq!(w.get::<ProcState>(e).unwrap().generations(), generations);
    // A game without rule plugins still has the engine's terrain rules.
    let names: Vec<&str> = Game::new().rules().rules().map(|r| r.name).collect();
    assert_eq!(names, ["terrain", "terrain_paths"]);
}
