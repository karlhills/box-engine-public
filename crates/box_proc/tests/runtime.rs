//! Generator runtime tests: registration, stamping and the throttled
//! flush, cache keys, inputs, panics, cleanup and the golden harness.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use box_core::app::App;
use box_core::assets::Assets;
use box_core::identity::Uid;
use box_core::mesh::{MaterialGroup, MeshData};
use box_core::reflect::{Reflect, ReflectComponent, ReflectDefault, Registry, bevy_reflect};
use box_core::world::{Entity, World};
use box_proc::golden::{GoldenCase, GoldenError, check_golden, fixture_path};
use box_proc::{
    AppGeneratorExt, GenContext, GenOutput, Generator, GeneratorRegistry, Inputs, ProcClock,
    ProcRuntime, ProcSettings, ProcState, Procedural, flush, mesh_hash, settle,
};
use glam::Vec3;

// ------------------------------------------------------------ generators ----

/// A box split into a body (slot 0) and a top (slot 1), with an optional
/// seeded jitter.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone)]
struct Block {
    size: f32,
    jitter: f32,
}

impl Default for Block {
    fn default() -> Self {
        Self {
            size: 1.0,
            jitter: 0.0,
        }
    }
}

impl Generator for Block {
    const VERSION: u32 = 1;
    const MATERIAL_SLOTS: &'static [&'static str] = &["Body", "Top"];
    const CATEGORY: &'static str = "Test";

    fn generate(&self, ctx: &GenContext) -> GenOutput {
        let mut rng = ctx.rng();
        let h = self.size + rng.range(0.0, self.jitter);
        let mut m = MeshData::cuboid(Vec3::new(self.size, h, self.size));
        // The cuboid's +Y face is its third (indices 12..18).
        m.groups = vec![
            MaterialGroup {
                first_index: 0,
                index_count: 12,
                material_slot: 0,
            },
            MaterialGroup {
                first_index: 12,
                index_count: 6,
                material_slot: 1,
            },
            MaterialGroup {
                first_index: 18,
                index_count: 18,
                material_slot: 0,
            },
        ];
        GenOutput::mesh(m)
    }
}

/// A component other generators read.
#[derive(Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default, Clone)]
struct Anchor {
    height: f32,
}

/// A column as tall as the `Anchor` of `target`.
#[derive(Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default, Clone)]
struct Column {
    target: Option<Uid>,
}

impl Generator for Column {
    const VERSION: u32 = 1;

    fn inputs(&self, inputs: &mut Inputs) {
        if let Some(uid) = self.target {
            inputs.component::<Anchor>(uid);
        }
    }

    fn generate(&self, ctx: &GenContext) -> GenOutput {
        let height = self
            .target
            .and_then(|uid| ctx.component::<Anchor>(uid))
            .map_or(1.0, |a| a.height);
        GenOutput::mesh(MeshData::cuboid(Vec3::new(0.2, height.max(0.01), 0.2)))
    }
}

/// Panics for negative sizes (a buggy generator).
#[derive(Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default, Clone)]
struct Fragile {
    size: f32,
}

impl Generator for Fragile {
    const VERSION: u32 = 1;
    fn generate(&self, _ctx: &GenContext) -> GenOutput {
        assert!(self.size >= 0.0, "negative size");
        GenOutput::mesh(MeshData::cuboid(Vec3::splat(self.size)))
    }
}

/// Takes `millis` to generate.
#[derive(Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default, Clone)]
struct Slow {
    millis: u32,
    size: f32,
}

impl Generator for Slow {
    const VERSION: u32 = 1;
    fn generate(&self, _ctx: &GenContext) -> GenOutput {
        std::thread::sleep(Duration::from_millis(u64::from(self.millis)));
        GenOutput::mesh(MeshData::cuboid(Vec3::splat(self.size.max(0.1))))
    }
}

/// Parameters in a map.
#[derive(Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default, Clone)]
struct Tagged {
    sizes: HashMap<String, f32>,
}

impl Generator for Tagged {
    const VERSION: u32 = 1;
    fn generate(&self, _ctx: &GenContext) -> GenOutput {
        let mut keys: Vec<_> = self.sizes.iter().collect();
        keys.sort_by(|a, b| a.0.cmp(b.0));
        let total: f32 = keys.iter().map(|(_, v)| **v).sum();
        GenOutput::mesh(MeshData::cuboid(Vec3::splat(total.max(0.1))))
    }
}

fn registry() -> Arc<Registry> {
    let mut r = Registry::with_core_types();
    box_proc::register_proc_types(&mut r);
    r.register_generator::<Block>().unwrap();
    r.register_generator::<Column>().unwrap();
    r.register_generator::<Fragile>().unwrap();
    r.register_generator::<Slow>().unwrap();
    r.register_generator::<Tagged>().unwrap();
    r.register::<Anchor>().unwrap();
    Arc::new(r)
}

/// A world with a manual clock at 0 (so the throttle is deterministic).
fn world() -> World {
    let mut w = World::with_registry(registry());
    w.insert_resource(ProcSettings {
        clock: ProcClock::Manual(Duration::ZERO),
        ..ProcSettings::default()
    });
    w
}

fn set_clock(w: &mut World, ms: u64) {
    w.resource_mut::<ProcSettings>().clock = ProcClock::Manual(Duration::from_millis(ms));
}

fn generations(w: &World, e: Entity) -> u64 {
    w.get::<ProcState>(e).map_or(0, |s| s.generations())
}

fn mesh(w: &World, e: Entity) -> Option<MeshData> {
    let h = w.get::<ProcState>(e)?.mesh()?;
    w.resource::<Assets<MeshData>>().get(h).cloned()
}

fn height(w: &World, e: Entity) -> f32 {
    let m = mesh(w, e).expect("mesh");
    let b = m.bounds();
    b.max.y - b.min.y
}

// ----------------------------------------------------------------- tests ----

#[test]
fn registry_lists_generators_with_names_categories_and_slots() {
    let r = registry();
    let names: Vec<_> = r.generators().iter().map(|g| g.name()).collect();
    // Ordered by category, then label: "Procedural", the engine's
    // "Shapes" (Extrude, Lathe, Primitive, Sweep), "Terrain" (the hidden
    // ground and path chunk generators), then "Test".
    assert_eq!(
        names,
        [
            "Column",
            "Fragile",
            "Slow",
            "Tagged",
            "Extrude",
            "Lathe",
            "Primitive",
            "Sweep",
            "TerrainChunk",
            "TerrainPathChunk",
            "Block"
        ]
    );
    let block = r.generator("Block").unwrap();
    assert_eq!(block.label(), "Block");
    assert_eq!(block.category(), "Test");
    assert_eq!(block.version(), 1);
    assert_eq!(block.material_slots(), ["Body", "Top"]);
    assert!(
        r.generator("Anchor").is_none(),
        "plain components are not generators"
    );
    assert!(r.component("Procedural").is_some());

    // Type-erased, from a dynamic value (as tools hold them).
    let dynamic = block.default_value().unwrap().to_dynamic().unwrap();
    let out = block
        .reflect()
        .generate(dynamic.as_ref(), &GenContext::new(0))
        .unwrap();
    assert_eq!(out.mesh.unwrap().triangle_count(), 12);
    assert!(
        block
            .reflect()
            .generate(&Anchor::default(), &GenContext::new(0))
            .is_none(),
        "a value of another type"
    );
}

#[test]
fn new_generator_generates_once_then_idles() {
    let mut w = world();
    let e = w.spawn((Block::default(),));
    flush(&mut w);
    assert_eq!(generations(&w, e), 1);
    let s = w.get::<ProcState>(e).unwrap();
    assert_eq!(s.generator(), "Block");
    assert_eq!(s.slot_count(), 2);
    assert!(s.key().is_some() && !s.is_dirty() && s.warnings().is_empty());
    let handle = s.mesh().unwrap();
    drop(s);
    let v = w.resource::<Assets<MeshData>>().version(handle);
    for _ in 0..5 {
        flush(&mut w);
    }
    assert_eq!(generations(&w, e), 1, "idle frames regenerate nothing");
    assert_eq!(w.resource::<Assets<MeshData>>().version(handle), v);
    assert_eq!(w.resource::<ProcRuntime>().queued(), 0);
}

#[test]
fn edit_regenerates_in_place_and_equal_keys_are_skipped() {
    let mut w = world();
    let e = w.spawn((Block::default(),));
    flush(&mut w);
    let handle = w.get::<ProcState>(e).unwrap().mesh().unwrap();
    let v = w.resource::<Assets<MeshData>>().version(handle).unwrap();

    set_clock(&mut w, 100);
    w.get_mut::<Block>(e).unwrap().size = 2.0;
    flush(&mut w);
    assert_eq!(generations(&w, e), 2);
    assert_eq!(
        w.get::<ProcState>(e).unwrap().mesh(),
        Some(handle),
        "same handle"
    );
    assert_eq!(
        w.resource::<Assets<MeshData>>().version(handle),
        Some(v + 1),
        "version bumped once: the renderer re-uploads"
    );
    assert!((height(&w, e) - 2.0).abs() < 1e-6);

    // Edited and edited back within one throttle interval: stamped, but
    // the cache key is unchanged, so nothing is rebuilt.
    set_clock(&mut w, 200);
    w.get_mut::<Block>(e).unwrap().size = 3.0;
    flush(&mut w);
    assert_eq!(generations(&w, e), 3);
    set_clock(&mut w, 210);
    w.get_mut::<Block>(e).unwrap().size = 4.0;
    flush(&mut w); // throttled: stays queued
    w.get_mut::<Block>(e).unwrap().size = 3.0;
    set_clock(&mut w, 240);
    let skipped = w.resource::<ProcRuntime>().stats().skipped;
    flush(&mut w);
    assert_eq!(generations(&w, e), 3);
    assert_eq!(w.resource::<ProcRuntime>().stats().skipped, skipped + 1);

    // The seed is part of the key.
    w.get_mut::<Block>(e).unwrap().jitter = 1.0;
    set_clock(&mut w, 300);
    flush(&mut w);
    let key_seed0 = w.get::<ProcState>(e).unwrap().key();
    w.insert_one(e, Procedural::with_seed(7)).unwrap();
    set_clock(&mut w, 400);
    flush(&mut w);
    assert_ne!(w.get::<ProcState>(e).unwrap().key(), key_seed0);
    assert_eq!(generations(&w, e), 5);

    // Materials and shadows do not touch the mesh.
    w.get_mut::<Procedural>(e).unwrap().cast_shadows = false;
    set_clock(&mut w, 500);
    flush(&mut w);
    assert_eq!(generations(&w, e), 5);
}

#[test]
fn same_parameters_and_seed_give_the_same_hash() {
    let a = Block {
        size: 1.5,
        jitter: 0.5,
    };
    let hash = |seed| mesh_hash(&a.generate(&GenContext::new(seed)).mesh.unwrap());
    assert_eq!(hash(3), hash(3));
    assert_ne!(hash(3), hash(4), "the seed drives the jitter");
}

#[test]
fn map_order_does_not_change_the_cache_key_but_values_do() {
    let mut w = world();
    let mut forward = HashMap::new();
    let mut backward = HashMap::new();
    let names: Vec<String> = (0..16).map(|i| format!("k{i}")).collect();
    for (i, n) in names.iter().enumerate() {
        forward.insert(n.clone(), i as f32);
    }
    for (i, n) in names.iter().enumerate().rev() {
        backward.insert(n.clone(), i as f32);
    }
    let a = w.spawn((Tagged { sizes: forward },));
    let b = w.spawn((Tagged {
        sizes: backward.clone(),
    },));
    flush(&mut w);
    let key = |w: &World, e| w.get::<ProcState>(e).unwrap().key().unwrap();
    assert_eq!(key(&w, a), key(&w, b));
    backward.insert("k3".into(), 99.0);
    w.get_mut::<Tagged>(b).unwrap().sizes = backward;
    set_clock(&mut w, 100);
    flush(&mut w);
    assert_ne!(key(&w, a), key(&w, b));
}

#[test]
fn throttle_coalesces_a_drag_and_the_last_value_lands() {
    let mut w = world();
    let e = w.spawn((Block::default(),));
    flush(&mut w); // t = 0: first generation, immediate.
    assert_eq!(generations(&w, e), 1);

    // A 1 s drag at 60 Hz, one edit per frame.
    let frame_ms = 1000.0 / 60.0;
    let mut last = 0.0;
    for frame in 1..=60u32 {
        let t = (f64::from(frame) * frame_ms) as u64 + 1000;
        set_clock(&mut w, t);
        last = 1.0 + frame as f32 * 0.01;
        w.get_mut::<Block>(e).unwrap().size = last;
        flush(&mut w);
    }
    let during = generations(&w, e) - 1;
    // 30 ms throttle at 16.7 ms frames: every other frame.
    assert!((28..=31).contains(&during), "{during} regenerations");
    // The drag stopped; the last edit may still be queued, and lands at the
    // next flush the throttle allows.
    for extra in 1..=3u64 {
        set_clock(&mut w, 2000 + extra * 17);
        flush(&mut w);
    }
    assert!((height(&w, e) - last).abs() < 1e-5, "last value landed");
    assert!(!w.get::<ProcState>(e).unwrap().is_dirty());

    // The first edit after a pause is not delayed.
    let before = generations(&w, e);
    set_clock(&mut w, 5000);
    w.get_mut::<Block>(e).unwrap().size = 4.0;
    flush(&mut w);
    assert_eq!(generations(&w, e), before + 1);
}

#[test]
fn new_objects_are_not_throttled_but_edits_are() {
    let mut w = world();
    let a = w.spawn((Block::default(),));
    flush(&mut w);
    // Within the throttle interval of that flush: an edit waits...
    w.get_mut::<Block>(a).unwrap().size = 2.0;
    flush(&mut w);
    assert_eq!(generations(&w, a), 1, "edit throttled");
    // ...but a new object shows in the frame it appears (and the waiting
    // edit lands with it).
    let b = w.spawn((Block::default(),));
    flush(&mut w);
    assert_eq!(generations(&w, b), 1, "new object generated at once");
    assert_eq!(generations(&w, a), 2);
}

#[test]
fn an_identical_output_is_not_uploaded_again() {
    /// The size shapes the mesh; `label` does not.
    #[derive(Reflect, Clone, Debug, PartialEq, Default)]
    #[reflect(Component, Default, Clone)]
    struct Labelled {
        size: f32,
        label: String,
    }
    impl Generator for Labelled {
        const VERSION: u32 = 1;
        fn generate(&self, _ctx: &GenContext) -> GenOutput {
            GenOutput::mesh(MeshData::cuboid(Vec3::splat(self.size.max(0.1))))
        }
    }
    let mut app = App::new();
    app.add_generator::<Labelled>();
    let w = &mut app.world;
    w.insert_resource(ProcSettings {
        throttle: Duration::ZERO,
        ..ProcSettings::default()
    });
    let e = w.spawn((Labelled::default(),));
    flush(w);
    let h = w.get::<ProcState>(e).unwrap().mesh().unwrap();
    let v = w.resource::<Assets<MeshData>>().version(h);
    w.get_mut::<Labelled>(e).unwrap().label = "renamed".into();
    flush(w);
    assert_eq!(generations(w, e), 2, "the key changed, so it regenerated");
    assert_eq!(
        w.resource::<Assets<MeshData>>().version(h),
        v,
        "same mesh: not written, not re-uploaded"
    );
}

#[test]
fn primitives_generate_in_a_core_registry_world() {
    use box_core::primitive::Primitive;
    let mut w = World::new(); // box_core's registry: no generator type data
    let e = w.spawn((Primitive::sphere(0.5),));
    flush(&mut w);
    let s = w.get::<ProcState>(e).expect("Primitive is a generator");
    assert_eq!(s.generator(), "Primitive");
    drop(s);
    assert_eq!(mesh(&w, e), Some(Primitive::sphere(0.5).mesh()));
}

#[test]
fn budget_defers_work_without_starving() {
    let mut w = world();
    w.resource_mut::<ProcSettings>().budget = Duration::from_millis(5);
    let entities: Vec<Entity> = (0..8)
        .map(|_| {
            w.spawn((Slow {
                millis: 2,
                size: 1.0,
            },))
        })
        .collect();
    flush(&mut w);
    let first = entities
        .iter()
        .filter(|e| generations(&w, **e) == 1)
        .count();
    assert!(
        (1..8).contains(&first),
        "{first} generated in the first flush"
    );
    assert!(w.resource::<ProcRuntime>().stats().deferred > 0);
    // Leftovers continue on the next frames without waiting for the
    // throttle (the clock does not move).
    for _ in 0..8 {
        flush(&mut w);
    }
    assert!(entities.iter().all(|e| generations(&w, *e) == 1));

    // A zero budget still processes one object per flush.
    w.resource_mut::<ProcSettings>().budget = Duration::ZERO;
    set_clock(&mut w, 1000);
    for e in &entities {
        w.get_mut::<Slow>(*e).unwrap().size = 2.0;
    }
    flush(&mut w);
    let done = entities
        .iter()
        .filter(|e| generations(&w, **e) == 2)
        .count();
    assert_eq!(done, 1);
    settle(&mut w);
    assert!(
        entities.iter().all(|e| generations(&w, *e) == 2),
        "settle finishes"
    );
}

#[test]
fn idle_generators_cost_little_per_frame() {
    let mut w = world();
    for i in 0..1000 {
        w.spawn((Block {
            size: 1.0 + i as f32 * 0.001,
            jitter: 0.0,
        },));
    }
    settle(&mut w);
    assert_eq!(w.resource::<ProcRuntime>().stats().generated, 1000);
    let frames = 200;
    let start = Instant::now();
    for _ in 0..frames {
        flush(&mut w);
    }
    let per_frame = start.elapsed() / frames;
    println!("1,000 idle generators: {per_frame:?} per flush");
    assert_eq!(w.resource::<ProcRuntime>().stats().generated, 1000);
    // Generous: unoptimized test builds on a loaded machine.
    assert!(per_frame < Duration::from_millis(5), "{per_frame:?}");
}

#[test]
fn input_changes_regenerate_dependents_and_missing_inputs_warn() {
    let mut w = world();
    let anchor_uid = Uid::new();
    let anchor = w.spawn((anchor_uid, Anchor { height: 2.0 }));
    let a = w.spawn((Column {
        target: Some(anchor_uid),
    },));
    let b = w.spawn((Column {
        target: Some(anchor_uid),
    },));
    let other = w.spawn((Block::default(),));
    flush(&mut w);
    assert!((height(&w, a) - 2.0).abs() < 1e-6);
    assert_eq!(
        w.resource::<ProcRuntime>().watched_inputs(),
        1,
        "one watcher, two dependents"
    );

    set_clock(&mut w, 100);
    w.get_mut::<Anchor>(anchor).unwrap().height = 3.0;
    flush(&mut w);
    assert!((height(&w, a) - 3.0).abs() < 1e-6);
    assert!((height(&w, b) - 3.0).abs() < 1e-6);
    assert_eq!(generations(&w, other), 1, "unrelated objects are untouched");

    // The input entity goes away: a warning, and the fallback shape.
    set_clock(&mut w, 200);
    w.despawn(anchor);
    flush(&mut w);
    let s = w.get::<ProcState>(a).unwrap();
    assert_eq!(s.warnings().len(), 1);
    assert!(s.warnings()[0].contains("`Anchor`") && s.warnings()[0].contains("missing"));
    drop(s);
    assert!((height(&w, a) - 1.0).abs() < 1e-6);

    // Pointing elsewhere drops the watch.
    set_clock(&mut w, 300);
    w.get_mut::<Column>(a).unwrap().target = None;
    w.get_mut::<Column>(b).unwrap().target = None;
    flush(&mut w);
    assert_eq!(w.resource::<ProcRuntime>().watched_inputs(), 0);
    assert!(w.get::<ProcState>(a).unwrap().warnings().is_empty());
}

#[test]
fn a_panicking_generator_becomes_a_warning() {
    let mut w = world();
    let e = w.spawn((Fragile { size: 1.0 },));
    flush(&mut w);
    assert!(mesh(&w, e).is_some());
    set_clock(&mut w, 100);
    w.get_mut::<Fragile>(e).unwrap().size = -1.0;
    flush(&mut w);
    let s = w.get::<ProcState>(e).unwrap();
    assert!(s.mesh().is_none(), "draws nothing");
    assert!(
        s.warnings()[0].contains("generator panicked: negative size"),
        "{:?}",
        s.warnings()
    );
    drop(s);
    assert!(
        w.resource::<Assets<MeshData>>().is_empty(),
        "old mesh freed"
    );
    // Not retried every frame: the key is recorded.
    set_clock(&mut w, 200);
    flush(&mut w);
    assert_eq!(generations(&w, e), 2);
    // Fixing the parameter recovers.
    set_clock(&mut w, 300);
    w.get_mut::<Fragile>(e).unwrap().size = 1.0;
    flush(&mut w);
    assert!(mesh(&w, e).is_some());
    assert!(w.get::<ProcState>(e).unwrap().warnings().is_empty());
}

#[test]
fn removing_the_generator_or_despawning_frees_the_mesh() {
    let mut w = world();
    let a = w.spawn((Block::default(),));
    let b = w.spawn((Block::default(),));
    flush(&mut w);
    assert_eq!(w.resource::<Assets<MeshData>>().len(), 2);
    w.remove_one::<Block>(a).unwrap();
    flush(&mut w);
    assert!(!w.has::<ProcState>(a));
    assert_eq!(w.resource::<Assets<MeshData>>().len(), 1);
    w.despawn(b);
    flush(&mut w);
    assert!(w.resource::<Assets<MeshData>>().is_empty());
    // Adding it back starts over.
    w.insert_one(a, Block::default()).unwrap();
    set_clock(&mut w, 100);
    flush(&mut w);
    assert_eq!(generations(&w, a), 1);
}

#[test]
fn a_second_generator_on_one_entity_waits_until_the_first_goes() {
    let mut w = world();
    let e = w.spawn((Block::default(), Fragile { size: 0.5 }));
    flush(&mut w);
    let first = w.get::<ProcState>(e).unwrap().generator();
    let second = if first == "Block" {
        w.remove_one::<Block>(e).unwrap();
        "Fragile"
    } else {
        w.remove_one::<Fragile>(e).unwrap();
        "Block"
    };
    flush(&mut w); // the first generator's state is released
    set_clock(&mut w, 100);
    flush(&mut w); // the second one takes over
    assert_eq!(w.get::<ProcState>(e).unwrap().generator(), second);
    assert!(mesh(&w, e).is_some());
}

#[test]
fn add_generator_runs_types_missing_from_the_registry() {
    let mut app = App::new(); // core registry: no Block
    app.add_generator::<Block>();
    let e = app.world.spawn((Block::default(),));
    flush(&mut app.world);
    assert!(mesh(&app.world, e).is_some());
    // `Primitive` runs in every world, whatever its registry lists.
    assert_eq!(
        app.world.resource::<ProcRuntime>().generator_names(),
        ["Primitive", "Block"]
    );
}

// --------------------------------------------------------------- golden ----

static TWEAK: AtomicU32 = AtomicU32::new(0);

/// A generator whose output a test can change without touching `VERSION`.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone)]
struct Pinned {
    size: f32,
}

impl Default for Pinned {
    fn default() -> Self {
        Self { size: 1.0 }
    }
}

impl Generator for Pinned {
    const VERSION: u32 = 3;
    fn generate(&self, _ctx: &GenContext) -> GenOutput {
        let grow = TWEAK.load(Ordering::SeqCst) as f32 * 0.01;
        GenOutput::mesh(MeshData::cuboid(Vec3::splat(self.size + grow)))
    }
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("box_proc_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[test]
fn golden_fails_on_an_output_change_without_a_version_bump() {
    let dir = temp_dir("golden");
    let cases = [
        GoldenCase::new("default", Pinned::default(), 0),
        GoldenCase::new("big", Pinned { size: 4.0 }, 9),
    ];
    // No fixture yet.
    assert!(matches!(
        check_golden(&dir, &cases, false),
        Err(GoldenError::Missing { .. })
    ));
    let report = check_golden(&dir, &cases, true).unwrap();
    assert!(report.recorded && report.cases == 2);
    let text = std::fs::read_to_string(fixture_path::<Pinned>(&dir)).unwrap();
    assert!(text.contains("\"version\": 3") && text.contains("\"big\""));
    // Unchanged output passes and writes nothing.
    let report = check_golden(&dir, &cases, false).unwrap();
    assert!(!report.recorded);

    // The output changes, VERSION does not: fails, even when re-recording.
    TWEAK.store(1, Ordering::SeqCst);
    for update in [false, true] {
        match check_golden(&dir, &cases, update) {
            Err(e @ GoldenError::Unversioned { .. }) => {
                let msg = e.to_string();
                assert!(msg.contains("Bump VERSION"), "{msg}");
                assert!(msg.contains("`default`"), "{msg}");
            }
            other => panic!("expected an unversioned change, got {other:?}"),
        }
    }
    TWEAK.store(0, Ordering::SeqCst);

    // Fixtures recorded at another version fail until re-recorded.
    let path = fixture_path::<Pinned>(&dir);
    let old = text.replace("\"version\": 3", "\"version\": 2");
    std::fs::write(&path, &old).unwrap();
    assert!(matches!(
        check_golden(&dir, &cases, false),
        Err(GoldenError::VersionChanged { recorded: 2, .. })
    ));
    assert!(check_golden(&dir, &cases, true).unwrap().recorded);

    // A new case in code must be recorded.
    let more = [
        GoldenCase::new("default", Pinned::default(), 0),
        GoldenCase::new("small", Pinned { size: 0.5 }, 0),
    ];
    assert!(matches!(
        check_golden(&dir, &more, false),
        Err(GoldenError::Unrecorded { .. })
    ));
    check_golden(&dir, &more, true).unwrap();
    assert_eq!(check_golden(&dir, &more, false).unwrap().cases, 3);
    let _ = std::fs::remove_dir_all(&dir);
}
