//! Convert to Script (docs/BEHAVIOURS.md, B3): each behaviour with a script
//! version runs side by side with its converted copy, and they move alike.
//!
//! - `eject_replaces_the_component_as_one_undo_step`: the command swaps the
//!   component for a `Script` with the values as properties, lists the
//!   script file, keeps a file that exists; undo brings the component back;
//!   behaviours without a script version and too many Mover points are
//!   refused with a reason.
//! - `converted_behaviours_move_like_the_components`: Spinner, Bobber and
//!   three Movers (PingPong with a wait, Loop, Once), each next to its
//!   converted copy, compared over 6 s of play.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use box_engine::behaviours::{Bobber, Mover, Spinner};
use box_engine::box_assets::AssetServer;
use box_engine::box_core::edit::{Command, EditStack, apply_commands_with_warnings};
use box_engine::box_script::{Script, ScriptRuntime};
use box_engine::prelude::*;

struct Project {
    root: PathBuf,
}

impl Project {
    fn new(name: &str) -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "box_b3_{name}_{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("assets")).unwrap();
        Self { root }
    }

    fn assets(&self) -> PathBuf {
        self.root.join("assets")
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn commands(json: serde_json::Value) -> Vec<Command> {
    serde_json::from_value(json).unwrap()
}

#[test]
fn eject_replaces_the_component_as_one_undo_step() {
    let game = Game::new();
    let mut world = game.new_world();
    let mut stack = EditStack::default();
    apply_commands_with_warnings(
        &mut world,
        &mut stack,
        &commands(serde_json::json!([
            { "op": "spawn", "name": "Lift", "components": {
                "Transform": {},
                "Mover": { "points": [[0, 0, 0], [0, 3, 0], [2, 3, 0]], "speed": 1.5, "mode": "Loop", "wait": 0.25 } } },
            { "op": "spawn", "name": "Long", "components": {
                "Transform": {},
                "Mover": { "points": [[0,0,0],[1,0,0],[2,0,0],[3,0,0],[4,0,0],[5,0,0],[6,0,0],[7,0,0],[8,0,0]] } } },
            { "op": "spawn", "name": "Hurt", "components": { "Transform": {}, "Health": {} } }
        ])),
        "level",
    )
    .unwrap();
    let undo_before = stack.undo_len();
    let applied = apply_commands_with_warnings(
        &mut world,
        &mut stack,
        &commands(serde_json::json!([{ "op": "eject_behaviour", "entity": "Lift", "component": "Mover" }])),
        "convert",
    )
    .unwrap();
    assert_eq!(stack.undo_len(), undo_before + 1, "one undo step");
    assert_eq!(applied.files.len(), 1);
    assert_eq!(applied.files[0].0, "scripts/behaviours/mover.luau");
    let lift = world.find_by_name("Lift").unwrap();
    assert!(!world.has::<Mover>(lift));
    let script = (*world.get::<Script>(lift).unwrap()).clone();
    assert_eq!(
        script.source.path.as_deref(),
        Some("scripts/behaviours/mover.luau")
    );
    let prop = |n: &str| {
        script
            .properties
            .iter()
            .find(|p| p.name == n)
            .map(|p| p.value.clone())
            .unwrap_or_else(|| panic!("no property {n}"))
    };
    use box_engine::box_script::ScriptValue;
    assert_eq!(prop("speed"), ScriptValue::Number(1.5));
    assert_eq!(prop("mode"), ScriptValue::String("Loop".into()));
    assert_eq!(prop("point_count"), ScriptValue::Integer(3));
    assert_eq!(
        prop("point_3"),
        ScriptValue::Vector(Vec3::new(2.0, 3.0, 0.0))
    );
    // Undo puts the Mover back.
    stack.undo(&mut world).unwrap();
    let lift = world.find_by_name("Lift").unwrap();
    assert!(world.has::<Mover>(lift));
    assert!(!world.has::<Script>(lift));

    // Refusals say why.
    let err = apply_commands_with_warnings(
        &mut world,
        &mut stack,
        &commands(serde_json::json!([{ "op": "eject_behaviour", "entity": "Long", "component": "Mover" }])),
        "convert",
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("at most 8"), "{err}");
    let err = apply_commands_with_warnings(
        &mut world,
        &mut stack,
        &commands(serde_json::json!([{ "op": "eject_behaviour", "entity": "Hurt", "component": "Health" }])),
        "convert",
    )
    .unwrap_err()
    .to_string();
    assert!(
        err.contains("no script version") && err.contains("Mover"),
        "{err}"
    );

    // An existing (edited) file is kept.
    let p = Project::new("keep");
    std::fs::create_dir_all(p.assets().join("scripts/behaviours")).unwrap();
    std::fs::write(p.assets().join("scripts/behaviours/mover.luau"), "-- mine").unwrap();
    let written = box_engine::behaviours::scripts::write_missing_files(
        &p.assets(),
        &[
            ("scripts/behaviours/mover.luau".into(), "new".into()),
            ("scripts/behaviours/spinner.luau".into(), "spin".into()),
        ],
    )
    .unwrap();
    assert_eq!(written, ["scripts/behaviours/spinner.luau"]);
    assert_eq!(
        std::fs::read_to_string(p.assets().join("scripts/behaviours/mover.luau")).unwrap(),
        "-- mine"
    );
}

/// Pairs side by side: the component on `<name>`, its converted copy on
/// `<name> Script`, `dy` metres higher.
fn pairs() -> serde_json::Value {
    let mut out = Vec::new();
    let mut add = |name: &str, x: f32, components: serde_json::Value| {
        for (suffix, y) in [("", 0.0), (" Script", 10.0)] {
            let mut c = components.clone();
            c["Transform"] = serde_json::json!({ "translation": [x, y, 0.0] });
            out.push(serde_json::json!({ "op": "spawn", "name": format!("{name}{suffix}"), "components": c }));
        }
    };
    add(
        "Spin",
        0.0,
        serde_json::json!({ "Spinner": { "axis": [0.3, 1.0, 0.0], "speed": 2.5 } }),
    );
    add(
        "Bob",
        4.0,
        serde_json::json!({ "Bobber": { "height": 0.4, "period": 1.7 } }),
    );
    add(
        "PingPong",
        8.0,
        serde_json::json!({ "Mover": { "points": [[0, 0, 0], [3, 0, 0], [3, 0, 2]], "speed": 2.0, "wait": 0.5 } }),
    );
    add(
        "Loop",
        12.0,
        serde_json::json!({ "Mover": { "points": [[0, 0, 0], [1, 0, 0], [1, 0, 1], [0, 0, 1]], "speed": 1.0, "mode": "Loop", "wait": 0.0 } }),
    );
    add(
        "Once",
        16.0,
        serde_json::json!({ "Mover": { "points": [[0, 0, 0], [0, 0, 4]], "speed": 3.0, "mode": "Once" } }),
    );
    serde_json::Value::Array(out)
}

#[test]
fn converted_behaviours_move_like_the_components() {
    let p = Project::new("equiv");
    let game = Game::new();
    let mut world = game.new_world();
    let mut stack = EditStack::default();
    apply_commands_with_warnings(&mut world, &mut stack, &commands(pairs()), "level").unwrap();
    let mut files = Vec::new();
    for (name, component) in [
        ("Spin Script", "Spinner"),
        ("Bob Script", "Bobber"),
        ("PingPong Script", "Mover"),
        ("Loop Script", "Mover"),
        ("Once Script", "Mover"),
    ] {
        let applied = apply_commands_with_warnings(
            &mut world,
            &mut stack,
            &commands(serde_json::json!([{ "op": "eject_behaviour", "entity": name, "component": component }])),
            "convert",
        )
        .unwrap();
        files.extend(applied.files);
    }
    box_engine::behaviours::scripts::write_missing_files(&p.assets(), &files).unwrap();
    world.insert_resource(box_engine::asset_server(p.assets()).unwrap());
    let mut app = game.app(world);
    // Scripts load on worker threads; wait until every instance started.
    let start = std::time::Instant::now();
    loop {
        let s = app.world.resource::<AssetServer>().clone();
        s.update_until_idle(&mut app.world, Duration::from_millis(50));
        app.run_frames(1, 1.0 / 60.0);
        let started = app
            .world
            .get_resource::<ScriptRuntime>()
            .map_or(0, |rt| rt.stats().instances);
        if started >= 5 {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "scripts did not start"
        );
    }
    let rt = app.world.resource::<ScriptRuntime>();
    assert_eq!(rt.stats().errors, 0);
    // Compare as they go (the scripts start a frame or two later: one
    // frame of travel is 3–5 cm).
    let tf = |app: &App, n: &str| {
        let e = app.world.find_by_name(n).unwrap();
        *app.world.get::<Transform>(e).unwrap()
    };
    let mut worst_move: f32 = 0.0;
    let mut worst_turn: f32 = 0.0;
    for _ in 0..360 {
        app.run_frames(1, 1.0 / 60.0);
        for n in ["Bob", "PingPong", "Loop", "Once"] {
            let a = tf(&app, n).translation;
            let b = tf(&app, &format!("{n} Script")).translation - Vec3::Y * 10.0;
            worst_move = worst_move.max(a.distance(b));
        }
        let a = tf(&app, "Spin").rotation;
        let b = tf(&app, "Spin Script").rotation;
        worst_turn = worst_turn.max(a.angle_between(b));
    }
    println!("worst difference: {worst_move:.4} m, {worst_turn:.4} rad");
    assert!(
        worst_move < 0.11,
        "positions differ by up to {worst_move} m"
    );
    assert!(
        worst_turn < 0.1,
        "rotations differ by up to {worst_turn} rad"
    );
    // The one-way mover stopped at its end in both.
    assert!((tf(&app, "Once").translation.z - 4.0).abs() < 1e-3);
    assert!((tf(&app, "Once Script").translation.z - 4.0).abs() < 1e-3);
    let _ = (Spinner::default(), Bobber::default());
}
