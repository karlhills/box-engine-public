//! Playtests (W1): the file format, deterministic headless replays of the
//! Luau collector, a recording that replays to the same place, a check that
//! fails when the scene changes, and live recording and replay over the
//! control socket.

use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use box_engine::box_assets::AssetServer;
use box_engine::box_core::action::{Actions, InjectedInput};
use box_engine::box_core::input::Input;
use box_engine::control::{
    ControlClient, ControlHost, ControlServer, Discovery, FIXED_DT, HeadlessHost, HostKind,
};
use box_engine::player::{PlayerOptions, build_app};
use box_engine::playtest::{
    self, Check, Playtest, PlaytestRecorder, PlaytestReport, ReplayOptions, Step,
};
use box_engine::project::starter_scene;
use box_engine::{Game, settle_assets};
use glam::Vec2;
use serde_json::{Value as Json, json};

fn example() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/collector_luau")
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if entry.file_name() == ".box" {
            continue;
        }
        if path.is_dir() {
            copy_tree(&path, &to.join(entry.file_name()));
        } else {
            std::fs::copy(&path, to.join(entry.file_name())).unwrap();
        }
    }
}

/// A copy of the Luau collector in the test's temp folder.
fn collector(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("playtest_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    copy_tree(&example(), &dir);
    dir
}

/// The example's route (examples/collector_luau/playtests/route.playtest.json):
/// run to the middle gem, back south, then east to the south-east gem
/// (camera-relative movement from the scene's camera pose).
fn route_text() -> String {
    std::fs::read_to_string(example().join("playtests/route.playtest.json")).unwrap()
}

fn replay(project: &Path, playtest: &Playtest) -> PlaytestReport {
    let player = PlayerOptions {
        project: Some(project.to_path_buf()),
        ..Default::default()
    };
    playtest::replay(
        &Game::new(),
        &player,
        playtest,
        "route",
        &ReplayOptions::default(),
    )
    .unwrap()
}

#[test]
fn the_format_parses_round_trips_and_names_bad_steps() {
    let p = Playtest::parse(&route_text()).unwrap();
    assert_eq!(p.frames(), 305);
    assert_eq!(p.checks(), 7);
    assert!(matches!(&p.steps[1], Step::Wait(20)));
    assert!(matches!(
        &p.steps[4],
        Step::Check(Check::EntityNear { radius, horizontal: true, .. }) if *radius == 1.0
    ));
    let again = Playtest::parse(&p.to_json_pretty()).unwrap();
    assert_eq!(again, p);

    let err = |text: &str| Playtest::parse(text).unwrap_err().to_string();
    let e = err(r#"{"steps": [{"wait": 1}, {"jump": true}]}"#);
    assert!(
        e.contains("step 1") && e.contains("`input`, `wait` or `check`"),
        "{e}"
    );
    let e = err(r#"{"steps": [{"check": "entity_far", "entity": "x"}]}"#);
    assert!(e.contains("step 0") && e.contains("entity_far"), "{e}");
    let e = err(r#"{"steps": [{"check": "entity_gone", "entity": "x", "radius": 2}]}"#);
    assert!(e.contains("radius"), "{e}");
    let e = err(r#"{"steps": [{"input": {"move": [0, 1]}, "frames": 0}]}"#);
    assert!(e.contains("at least 1"), "{e}");
    let e = err(r#"{"steps": [{"input": {"move": [0, 1]}}]}"#);
    assert!(e.contains("frames"), "{e}");
    let e = err(r#"{"version": 9, "steps": []}"#);
    assert!(e.contains("newer"), "{e}");
    let e = err(r#"{"steps": [], "sead": 3}"#);
    assert!(e.contains("sead"), "{e}");
}

/// The acceptance case: a route that picks gems up replays twice with the
/// same report and end state, and a check fails once the scene changes.
#[test]
fn a_route_replays_identically_and_fails_when_the_scene_changes() {
    box_engine::init_logging();
    let project = collector("route");
    let route = Playtest::parse(&route_text()).unwrap();
    let first = replay(&project, &route);
    assert!(first.passed, "{}", first.text());
    assert_eq!(first.frames, 305);
    let second = replay(&project, &route);
    assert_eq!(first, second, "a replay is deterministic");

    // An unknown action is refused before anything runs.
    let mut typo = route.clone();
    if let Step::Input(s) = &mut typo.steps[0] {
        let v = s.input.remove("move").unwrap();
        s.input.insert("mov".into(), v);
    }
    let player = PlayerOptions {
        project: Some(project.clone()),
        ..Default::default()
    };
    let e = playtest::replay(
        &Game::new(),
        &player,
        &typo,
        "typo",
        &ReplayOptions::default(),
    )
    .unwrap_err();
    assert_eq!(e.code, "unknown_action");
    assert!(e.message.contains("did you mean `move`"), "{}", e.message);

    // Move the middle gem out of the way: the route no longer collects it.
    let scene_path = project.join("assets/scenes/arena.scene.json");
    let mut scene: Json =
        serde_json::from_str(&std::fs::read_to_string(&scene_path).unwrap()).unwrap();
    let gem = scene["entities"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|e| e["name"] == "Gem 1")
        .unwrap();
    gem["components"]["Transform"]["translation"] = json!([4.0, 0.9, 0.0]);
    std::fs::write(&scene_path, serde_json::to_string_pretty(&scene).unwrap()).unwrap();
    let changed = replay(&project, &route);
    assert!(!changed.passed);
    assert!(changed.finished);
    let failed: Vec<&str> = changed
        .checks
        .iter()
        .filter(|c| !c.passed)
        .map(|c| c.check.as_str())
        .collect();
    assert!(failed.contains(&"entity_gone"), "{}", changed.text());
    let gone = changed.checks.iter().find(|c| c.step == 2).unwrap();
    assert_eq!(gone.frame, 90);
    assert!(gone.message.contains("still there"), "{}", gone.message);
    assert_ne!(changed.fingerprint, first.fingerprint);
}

/// A run driven through named actions (as `send_input` does) and recorded
/// in a player's world becomes a playtest whose headless replay ends in the
/// same place, with the recording's own checks passing.
#[test]
fn a_recorded_run_replays_to_the_same_place() {
    box_engine::init_logging();
    let project = collector("record");
    let options = PlayerOptions {
        project: Some(project.clone()),
        watch: false,
        ..Default::default()
    };
    let (mut app, _) = build_app(&Game::new(), &options).unwrap();
    settle_assets(&mut app.world, Duration::from_secs(60));
    let recorder = PlaytestRecorder::new(&app.world, None, false, None).unwrap();
    app.world.insert_resource(recorder);
    let drive = |app: &mut box_engine::prelude::App, frames: u32, mv: Option<Vec2>, jump: bool| {
        {
            let actions = app.world.resource_mut::<Actions>();
            if let Some(v) = mv {
                actions.inject("move", InjectedInput::axis(v, frames));
            }
            if jump {
                actions.inject("jump", InjectedInput::press(1));
            }
        }
        for _ in 0..frames {
            app.tick(FIXED_DT);
            app.world.resource_mut::<Input>().end_frame();
            // As a replay does, so the scripts start on the same frame.
            if let Some(server) = app.world.get_resource::<AssetServer>().cloned() {
                server.update_until_idle(&mut app.world, Duration::from_secs(60));
            }
        }
    };
    drive(&mut app, 75, Some(Vec2::new(0.0, 1.0)), false);
    drive(&mut app, 10, None, true);
    drive(&mut app, 120, Some(Vec2::new(-0.7, 0.7)), false);
    drive(&mut app, 30, None, false);
    let recorder = app.world.remove_resource::<PlaytestRecorder>().unwrap();
    assert_eq!(recorder.frames(), 235);
    let recorded = recorder.finish(&app.world);
    assert_eq!(
        recorded.scene.as_deref(),
        Some("assets/scenes/arena.scene.json")
    );
    assert_eq!(recorded.frames(), 235);
    let checks: Vec<&Check> = recorded
        .steps
        .iter()
        .filter_map(|s| match s {
            Step::Check(c) => Some(c),
            _ => None,
        })
        .collect();
    assert!(
        checks
            .iter()
            .any(|c| matches!(c, Check::EntityGone { entity } if entity == "Gem 1")),
        "{}",
        recorded.to_json_pretty()
    );
    let Some(Check::EntityNear { point, .. }) = checks
        .iter()
        .find(|c| matches!(c, Check::EntityNear { entity, .. } if entity == "Player"))
    else {
        panic!("no player check: {}", recorded.to_json_pretty());
    };
    // Equal frames merged: move, the jump press, idle, move, idle.
    let inputs = recorded
        .steps
        .iter()
        .filter(|s| !matches!(s, Step::Check(_)))
        .count();
    assert!(inputs <= 6, "{}", recorded.to_json_pretty());

    let path = playtest::path_in_project(&project, "recorded");
    playtest::write(&path, &recorded).unwrap();
    let report = replay(&project, &Playtest::read(&path).unwrap());
    assert!(report.passed, "{}", report.text());
    let near = report
        .checks
        .iter()
        .find(|c| c.check == "entity_near")
        .unwrap();
    let distance = near.actual["distance"].as_f64().unwrap();
    assert!(
        distance < 0.01,
        "the replay ends where the run ended ({point:?}): {}",
        near.message
    );
}

/// Run `client` on a thread while polling `server` with `host` (frames run
/// only through `step_frames`).
fn drive<T: Send + 'static>(
    server: &mut ControlServer,
    host: &mut dyn ControlHost,
    client: impl FnOnce(Discovery) -> T + Send + 'static,
) -> T {
    let discovery = server.discovery();
    let handle = thread::spawn(move || client(discovery));
    let start = Instant::now();
    while !handle.is_finished() {
        server.poll(host);
        thread::sleep(Duration::from_millis(1));
        assert!(start.elapsed() < Duration::from_secs(120), "client hung");
    }
    server.poll(host);
    handle.join().unwrap()
}

/// A game that walks the entity named "Walker" with the `move` action
/// (2 m/s, +y forward = -Z).
struct WalkerGame;

impl box_engine::prelude::Plugin for WalkerGame {
    fn build(&self, app: &mut box_engine::prelude::App) {
        use box_engine::prelude::*;
        app.world
            .insert_resource(ActionMap::new().axis2("move", [Axis2Binding::wasd()]));
        app.add_system(Stage::FixedUpdate, |w: &mut World| {
            let (v, dt) = (
                w.resource::<Actions>().axis2("move"),
                w.resource::<Time>().delta,
            );
            if let Some(e) = w.find_by_name("Walker")
                && let Some(t) = w.component_mut::<Transform>(e)
            {
                t.translation += Vec3::new(v.x, 0.0, -v.y) * 2.0 * dt;
            }
        });
    }
}

/// Live: `record_start` restarts the game and records what `send_input`
/// injects, `record_stop` returns the playtest, `playtest` replays it in the
/// running game and `playtest_status` reports it.
#[test]
fn live_recording_and_replay_over_the_control_socket() {
    let mut server = ControlServer::bind(HostKind::Player, None).unwrap();
    let mut host = HeadlessHost::new(Game::new().with_plugin(WalkerGame), starter_scene());
    let r = drive(&mut server, &mut host, |d| {
        let mut c = ControlClient::connect(d, Duration::from_secs(10)).unwrap();
        let mut call = |m: &str, p: Json| {
            c.call(m, p)
                .unwrap_or_else(|e| json!({ "error": e.to_control_error().to_json() }))
        };
        let mut r = serde_json::Map::new();
        call(
            "apply_commands",
            json!({ "commands": [{ "op": "spawn", "name": "Walker" }] }),
        );
        r.insert("not_playing".into(), call("record_stop", json!({})));
        r.insert("start".into(), call("record_start", json!({})));
        r.insert(
            "again".into(),
            call("record_start", json!({ "restart": false })),
        );
        call(
            "send_input",
            json!({ "action": "move", "value": [1, 0], "frames": 30 }),
        );
        call("step_frames", json!({ "frames": 40 }));
        let stop = call("record_stop", json!({}));
        r.insert("stop".into(), stop.clone());
        let at = call("world_query", json!({ "name": "Walker" }));
        r.insert("at".into(), at.clone());
        let mut recorded = stop["playtest"].clone();
        let end = at["entities"][0]["components"]["Transform"]["translation"].clone();
        recorded["steps"].as_array_mut().unwrap().push(
            json!({ "check": "entity_near", "entity": "Walker", "point": end, "radius": 0.001 }),
        );
        // Wander off first: the replay restarts from the scene anyway.
        call(
            "send_input",
            json!({ "action": "move", "value": [0, 1], "frames": 20 }),
        );
        call("step_frames", json!({ "frames": 20 }));
        r.insert(
            "replay".into(),
            call("playtest", json!({ "playtest": recorded })),
        );
        r.insert("running".into(), call("playtest_status", json!({})));
        call("step_frames", json!({ "frames": 45 }));
        r.insert("done".into(), call("playtest_status", json!({})));
        r.insert(
            "bad".into(),
            call(
                "playtest",
                json!({ "playtest": { "steps": [{ "input": { "fly": true }, "frames": 1 }] } }),
            ),
        );
        Json::Object(r)
    });
    let code = |k: &str| r[k]["error"]["code"].as_str().unwrap_or("").to_string();
    assert_eq!(code("not_playing"), "not_playing", "{r:#}");
    assert_eq!(r["start"]["recording"], true);
    assert_eq!(r["start"]["restarted"], true);
    assert_eq!(code("again"), "invalid_params");
    assert_eq!(r["stop"]["frames"], 40, "{r:#}");
    let steps = &r["stop"]["playtest"]["steps"];
    assert_eq!(
        steps[0],
        json!({ "input": { "move": [1.0, 0.0] }, "frames": 30 })
    );
    assert_eq!(steps[1], json!({ "wait": 10 }));
    let x = r["at"]["entities"][0]["components"]["Transform"]["translation"][0]
        .as_f64()
        .unwrap();
    assert!((x - 1.0).abs() < 0.05, "walked 1 m: {x}");
    assert_eq!(r["replay"]["started"], true);
    assert_eq!(r["replay"]["total_frames"], 40);
    assert_eq!(r["running"]["finished"], false);
    assert_eq!(r["done"]["finished"], true, "{r:#}");
    assert_eq!(r["done"]["passed"], true, "{r:#}");
    assert_eq!(code("bad"), "unknown_action");
}
