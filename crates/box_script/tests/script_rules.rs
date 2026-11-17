//! The `examples/script_rules` data project (no Rust): Luau rules build its
//! fences and colonnade. Golden mesh hashes of every part (re-record with
//! `BOX_UPDATE_GOLDEN=1 cargo test -p box_script --test script_rules`,
//! then review the editor captures), the same parts in the player and in a
//! world that does not play, and after saving and loading the scene.
//!
//! `cargo test --release -p box_script --test script_rules -- --nocapture`
//! prints the cost of an idle pass, one edit and a drag over 100, 1,000 and
//! 5,000 fences, with the fence's `scope` and without it.

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use box_engine::box_core::derived::Derived;
use box_engine::box_core::scene;
use box_engine::box_proc::mesh_hash;
use box_engine::box_proc::rule::RuleRegistry;
use box_engine::box_script::{ScriptIntent, ScriptValue};
use box_engine::player::{PlayerOptions, build_app};
use box_engine::prelude::*;
use box_engine::{Game, app_with_world, settle_assets};

fn example() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/script_rules")
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/script_rules.json")
}

/// Every part's key, components and mesh hash, sorted by key.
fn parts(w: &World) -> BTreeMap<String, String> {
    let meshes = w.resource::<Assets<MeshData>>();
    w.query::<(Entity, &Derived)>()
        .iter()
        .map(|(e, d)| {
            let hash = w
                .get::<MeshRenderer>(e)
                .and_then(|r| meshes.get(r.mesh).map(mesh_hash))
                .map_or_else(|| "no mesh".to_owned(), |h| format!("{h:#018x}"));
            let t = *w.get::<Transform>(e).unwrap();
            let p = t.translation;
            (
                d.key.to_string(),
                format!("{hash} at ({:.3}, {:.3}, {:.3}) {}", p.x, p.y, p.z, d.rule),
            )
        })
        .collect()
}

/// The example's scene in a world that does not play (as the editor's edit
/// world and `box render`), settled.
fn edit_world(bytes: &[u8]) -> App {
    let game = Game::new();
    let world = scene::load(bytes, game.registry()).unwrap();
    let mut app = app_with_world(world);
    app.world
        .insert_resource(box_engine::asset_server(example().join("assets")).unwrap());
    app.world.insert_resource(game.rules());
    settle_assets(&mut app.world, Duration::from_secs(20));
    app
}

#[test]
fn the_example_builds_golden_parts_everywhere() {
    let bytes = std::fs::read(example().join("assets/scenes/main.scene.json")).unwrap();
    let edit = edit_world(&bytes);
    let got = parts(&edit.world);
    assert!(got.len() > 20, "{got:#?}");
    assert!(got.values().all(|v| !v.starts_with("no mesh")), "{got:#?}");
    let rules = edit.world.resource::<RuleRegistry>();
    for name in ["rules/fence.luau", "rules/colonnade.luau"] {
        let r = rules.get(name).unwrap();
        assert_eq!(r.warnings, 0, "{name}");
    }

    let path = fixture();
    if std::env::var_os("BOX_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(&got).unwrap() + "\n").unwrap();
        eprintln!("recorded {}", path.display());
    }
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "no fixture {}; record it with BOX_UPDATE_GOLDEN=1",
            path.display()
        )
    });
    let want: BTreeMap<String, String> = serde_json::from_str(&text).unwrap();
    assert_eq!(
        got, want,
        "parts changed; re-record with BOX_UPDATE_GOLDEN=1 if intended"
    );

    // The player (a world that plays) builds the same parts.
    let options =
        PlayerOptions::from_args([example().display().to_string(), "--headless".to_owned()])
            .unwrap();
    let (mut app, _) = build_app(&Game::new(), &options).unwrap();
    for _ in 0..3 {
        settle_assets(&mut app.world, Duration::from_secs(20));
        app.run_frames(1, 1.0 / 60.0);
    }
    assert_eq!(parts(&app.world), want, "player");

    // Saved and loaded again: no parts in the file, the same parts back.
    let saved = scene::save(&edit.world).unwrap();
    assert!(!String::from_utf8_lossy(&saved).contains("Derived"));
    assert_eq!(parts(&edit_world(&saved).world), want, "save and load");
}

/// The example's fence rule, as shipped (`scope = { neighbours = SNAP }`)
/// or without its scope line (every edit runs it over every fence).
fn fence_source(scoped: bool) -> String {
    let fence = std::fs::read_to_string(example().join("assets/rules/fence.luau")).unwrap();
    if scoped {
        return fence;
    }
    let line = "Fence.scope = { neighbours = SNAP }\n";
    assert!(fence.contains(line));
    fence.replace(line, "")
}

/// Release timings of the fence rule over `n` fences in rows of 32, each
/// fence's end meeting the next one's start (a shared post): 7 parts each.
struct Cost {
    parts: usize,
    /// One pass with nothing changed (the change gate).
    idle: Duration,
    /// One property edit of one fence: the rule function, and the whole
    /// pass (rule plus reconcile).
    edit_rule: Duration,
    edit: Duration,
    /// A drag of one fence's middle point, one move per pass (60 passes):
    /// mean and slowest pass.
    drag: Duration,
    drag_max: Duration,
}

fn cost(n: usize, scoped: bool) -> Cost {
    let fence = fence_source(scoped);
    let p = common::Project::new("rules_cost", &[("rules/fence.luau", fence.as_str())]);
    let game = Game::new();
    let mut app = app_with_world(game.new_world());
    app.world
        .insert_resource(box_engine::asset_server(p.assets()).unwrap());
    app.world.insert_resource(game.rules());
    let mut fences = Vec::new();
    for i in 0..n {
        let x = (i % 32) as f32 * 4.0;
        let z = (i / 32) as f32 * 6.0;
        fences.push(app.world.spawn((
            Uid::new(),
            Name::new(format!("Fence {i}")),
            Transform::from_xyz(x, 0.0, z),
            ScriptIntent::new("rules/fence.luau").with_points([
                Vec3::ZERO,
                Vec3::new(2.0, 0.0, 2.0),
                Vec3::new(4.0, 0.0, 0.0),
            ]),
        )));
    }
    settle_assets(&mut app.world, Duration::from_secs(120));
    let info = |app: &App| {
        app.world
            .resource::<RuleRegistry>()
            .get("rules/fence.luau")
            .unwrap()
            .clone()
    };
    let parts = info(&app).parts;
    assert_eq!(
        parts,
        n * 7 - (n - n.div_ceil(32)),
        "shared posts built once"
    );
    let pass = |app: &mut App| {
        let t = Instant::now();
        box_engine::box_proc::rule::run_rules(&mut app.world);
        t.elapsed()
    };
    // Idle passes: the change gate alone.
    let idle = (0..100).map(|_| pass(&mut app)).sum::<Duration>() / 100;
    let runs = info(&app).runs;
    // One edit: one rule run.
    let e = fences[n / 2];
    let (mut edit, mut edit_rule) = (Duration::ZERO, Duration::ZERO);
    let edits = 10;
    for k in 0..edits {
        app.world
            .component_mut::<ScriptIntent>(e)
            .unwrap()
            .set("height", ScriptValue::Number(1.0 + k as f64 * 0.05));
        edit += pass(&mut app);
        edit_rule += info(&app).last_rule.unwrap();
    }
    assert_eq!(info(&app).runs, runs + edits);
    // A 60 Hz drag of the middle point.
    let frames = 60;
    let mut drag = Vec::new();
    for k in 1..=frames {
        app.world.component_mut::<ScriptIntent>(e).unwrap().points[1] =
            Vec3::new(2.0 + 0.01 * k as f32, 0.0, 2.0 + 0.01 * k as f32);
        drag.push(pass(&mut app));
    }
    assert_eq!(info(&app).runs, runs + edits + frames);
    assert_eq!(info(&app).parts, parts);
    Cost {
        parts,
        idle,
        edit_rule: edit_rule / edits as u32,
        edit: edit / edits as u32,
        drag: drag.iter().sum::<Duration>() / frames as u32,
        drag_max: drag.into_iter().max().unwrap(),
    }
}

#[test]
fn rule_run_cost() {
    // Debug builds (`cargo test --workspace`) check the benchmark runs;
    // the numbers mean something in release.
    let sizes: &[usize] = if cfg!(debug_assertions) {
        &[100]
    } else {
        &[100, 1000, 5000]
    };
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    for scoped in [true, false] {
        for &n in sizes {
            let c = cost(n, scoped);
            eprintln!(
                "fence rule ({}), {n} intents ({} parts): idle pass {:.1} µs; one edit: rule function {:.3} ms, \
                 rule + reconcile {:.3} ms; 60 Hz drag {:.3} ms per pass (slowest {:.3} ms)",
                if scoped {
                    "neighbours scope"
                } else {
                    "file scope"
                },
                c.parts,
                c.idle.as_secs_f64() * 1e6,
                ms(c.edit_rule),
                ms(c.edit),
                ms(c.drag),
                ms(c.drag_max),
            );
        }
    }
}
