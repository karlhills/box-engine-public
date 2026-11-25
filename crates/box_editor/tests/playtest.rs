//! Playtests in the editor over the live control socket: recording,
//! saving and replaying (W1).

mod control_harness;

use std::thread;
use std::time::{Duration, Instant};

use box_editor::log::LogBuffer;
use box_engine::control::ControlClient;
use control_harness::{Frames, SCENE, project_editor};
use serde_json::{Value as Json, json};

/// Playtests in the editor: `record_start` restarts Play and records,
/// `record_stop` saves the playtest into the project, `playtest` replays it
/// in a fresh Play session and `playtest_status` reports it.
#[test]
fn the_editor_records_and_replays_playtests() {
    let (mut editor, root) = project_editor("playtest", LogBuffer::default());
    let mut frames = Frames::new(&mut editor);
    frames.frame(&mut editor);
    let project = root.clone();
    let conversation = thread::spawn(move || {
        let mut c = ControlClient::connect_project(&project, Duration::from_secs(60)).unwrap();
        let mut call = |method: &str, params: Json| {
            c.call(method, params)
                .unwrap_or_else(|e| json!({ "error": e.to_control_error().to_json() }))
        };
        let mut r = serde_json::Map::new();
        r.insert("start".into(), call("record_start", json!({ "seed": 3 })));
        call("pause", json!({}));
        call("step_frames", json!({ "frames": 12 }));
        r.insert(
            "stop".into(),
            call(
                "record_stop",
                json!({ "save": "smoke", "description": "idle" }),
            ),
        );
        r.insert(
            "elsewhere".into(),
            call(
                "playtest",
                json!({ "playtest": { "scene": "level9", "steps": [{ "wait": 1 }] } }),
            ),
        );
        r.insert(
            "replay".into(),
            call("playtest", json!({ "file": "smoke" })),
        );
        let start = Instant::now();
        loop {
            let status = call("playtest_status", json!({}));
            if status["finished"] == true || start.elapsed() > Duration::from_secs(60) {
                r.insert("done".into(), status);
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        Json::Object(r)
    });
    let r = frames.drive(&mut editor, conversation);
    assert_eq!(r["start"]["restarted"], true, "{r:#}");
    assert_eq!(r["start"]["scene"], SCENE);
    assert_eq!(r["stop"]["file"], "playtests/smoke.playtest.json", "{r:#}");
    assert!(r["stop"]["frames"].as_u64().unwrap() >= 12, "{r:#}");
    let saved =
        box_engine::playtest::Playtest::read(&root.join("playtests/smoke.playtest.json")).unwrap();
    assert_eq!(saved.seed, Some(3));
    assert_eq!(saved.description.as_deref(), Some("idle"));
    assert_eq!(saved.scene.as_deref(), Some(SCENE));
    assert_eq!(r["elsewhere"]["error"]["code"], "invalid_params");
    assert_eq!(r["replay"]["started"], true, "{r:#}");
    assert_eq!(r["done"]["finished"], true, "{r:#}");
    assert_eq!(r["done"]["passed"], true, "{r:#}");
    assert!(editor.state.is_playing());
}
