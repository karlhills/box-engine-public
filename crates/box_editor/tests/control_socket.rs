//! Every live-control method over the socket against a headless editor.
//! A binary of its own: it captures the editor's log with a scoped
//! `tracing` subscriber, and `tracing` caches callsite interest globally, so
//! another test in the same binary that logs first without the subscriber
//! would hide these messages (AGENTS.md, Gotchas).

mod control_harness;

use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use box_core::identity::Uid;
use box_core::scene;
use box_editor::log::LogBuffer;
use box_editor::{Editor, PlayState};
use box_engine::control::{self, ControlClient};
use control_harness::{Frames, SCENE, project_editor};
use serde_json::{Value as Json, json};
use tracing_subscriber::layer::SubscriberExt;

/// Every method over the socket, with the editor's bookkeeping checked after.
#[test]
fn a_client_queries_edits_screenshots_and_plays_the_editor() {
    let log = LogBuffer::default();
    let _subscriber =
        tracing::subscriber::set_default(tracing_subscriber::registry().with(log.layer()));
    let (mut editor, root) = project_editor("socket", log);
    let initial = scene::save(&editor.state.world).unwrap();
    let initial_count = editor.state.world.entity_count();
    let mut frames = Frames::new(&mut editor);
    let gpu = frames.gpu();
    // Lay the viewport out so screenshots have a current size.
    frames.frame(&mut editor);

    let conversation = thread::spawn(move || {
        let mut c = ControlClient::connect_project(&root, Duration::from_secs(60)).unwrap();
        let mut r = serde_json::Map::new();
        let mut call = |name: &str, method: &str, params: Json| {
            let v = match c.call(method, params) {
                Ok(v) => v,
                Err(e) => json!({ "error": e.to_control_error().to_json() }),
            };
            r.insert(name.into(), v);
        };
        call("status", "status", json!({}));
        call("context", "editor_context", json!({}));
        call("undo_turn_none", "undo_turn", json!({}));
        call(
            "query",
            "world_query",
            json!({ "include_components": false }),
        );
        call(
            "apply",
            "apply_commands",
            json!({ "commands": [
                { "op": "spawn", "name": "Lamp", "components": {
                    "Primitive": { "shape": { "Sphere": { "radius": 0.3, "sectors": 16, "stacks": 8 } } } } },
                { "op": "set", "entity": "Lamp", "path": "Transform.translation.y", "value": 2.0 }
            ] }),
        );
        call(
            "dry",
            "apply_commands",
            json!({ "commands": { "op": "despawn", "entity": "Lamp" }, "dry_run": true }),
        );
        call(
            "apply_err",
            "apply_commands",
            json!({ "commands": { "op": "rename", "entity": "Nobody", "name": "x" } }),
        );
        call("lamp", "world_query", json!({ "name": "lamp" }));
        call("shot", "screenshot", json!({ "width": 320, "height": 180 }));
        call("shot_default", "screenshot", json!({}));
        call("pause_edit", "pause", json!({}));
        call("play", "play", json!({}));
        call("play_query", "world_query", json!({ "name": "lamp" }));
        call(
            "play_apply",
            "apply_commands",
            json!({ "commands": { "op": "rename", "entity": "Lamp", "name": "Play Lamp" } }),
        );
        call("pause", "pause", json!({}));
        call("step", "step_frames", json!({ "frames": 5 }));
        call("paused", "status", json!({}));
        call(
            "play_shot",
            "screenshot",
            json!({ "width": 64, "height": 48 }),
        );
        call("stats", "frame_stats", json!({}));
        call("stop", "stop", json!({}));
        call("log", "read_log", json!({ "since": 0, "limit": 10000 }));
        r
    });
    let r = frames.drive(&mut editor, conversation);
    let shot_size = |v: &Json| {
        let png = control::base64_decode(v["png_base64"].as_str().unwrap()).unwrap();
        let img = image::load_from_memory(&png).unwrap();
        assert_eq!(
            (img.width() as u64, img.height() as u64),
            (v["width"].as_u64().unwrap(), v["height"].as_u64().unwrap())
        );
        (img.width(), img.height())
    };

    assert_eq!(r["status"]["kind"], "editor");
    assert_eq!(r["status"]["mode"], "edit");
    assert_eq!(r["status"]["scene"], SCENE);
    assert_eq!(r["status"]["undo"], 0);
    assert_eq!(r["context"]["scene"], SCENE);
    assert_eq!(r["context"]["mode"], "edit");
    assert!(r["context"]["camera"]["eye"].is_array(), "{}", r["context"]);
    assert!(r["context"]["turn"].is_null());
    // No assistant turn is open: nothing to undo.
    assert_eq!(r["undo_turn_none"]["undone"], false);
    assert_eq!(r["query"]["world"], "edit");
    assert_eq!(r["query"]["total"], initial_count);

    assert_eq!(r["apply"]["world"], "edit");
    assert_eq!(r["apply"]["undoable"], true);
    assert_eq!(r["apply"]["created"].as_array().unwrap().len(), 1);
    assert_eq!(r["dry"]["dry_run"], true);
    assert_eq!(r["apply_err"]["error"]["code"], "entity_not_found");
    assert_eq!(r["lamp"]["total"], 1, "the dry run changed nothing");
    assert_eq!(
        r["lamp"]["entities"][0]["components"]["Transform"]["translation"][1],
        2.0
    );

    if gpu {
        assert_eq!(shot_size(&r["shot"]), (320, 180));
        // No size: the viewport at its current size.
        let viewport = editor.state.viewport.target_px;
        assert!(viewport[0] > 0);
        assert_eq!(shot_size(&r["shot_default"]), (viewport[0], viewport[1]));
        assert_eq!(shot_size(&r["play_shot"]), (64, 48));
        let png = control::base64_decode(r["shot"]["png_base64"].as_str().unwrap()).unwrap();
        let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("editor_control_screenshot.png"), png).unwrap();
        assert_eq!(r["stats"]["renderer"]["validation_errors"], 0);
    } else {
        assert_eq!(r["shot"]["error"]["code"], "unsupported");
        assert_eq!(r["stats"]["renderer"], Json::Null);
    }

    assert_eq!(r["pause_edit"]["error"]["code"], "not_playing");
    assert_eq!(r["play"]["mode"], "play");
    assert_eq!(r["play_query"]["world"], "play");
    assert_eq!(r["play_query"]["total"], 1, "the play world has the edit");
    assert_eq!(r["play_apply"]["world"], "play");
    assert_eq!(r["play_apply"]["undoable"], false);
    assert_eq!(r["play_apply"]["discarded_on_stop"], true);
    assert_eq!(r["pause"]["mode"], "paused");
    let before_step = r["pause"]["frame"].as_u64().unwrap();
    assert_eq!(r["step"]["frames"], 5);
    assert_eq!(r["step"]["frame"].as_u64().unwrap(), before_step + 5);
    assert_eq!(r["paused"]["mode"], "paused", "stepping keeps the pause");
    assert_eq!(r["paused"]["frame"], r["step"]["frame"]);
    assert_eq!(r["paused"]["undo"], 1, "the edit world's history");
    assert_eq!(r["stats"]["world"], "play");
    assert_eq!(r["stop"]["mode"], "edit");
    let messages: Vec<&str> = r["log"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["message"].as_str().unwrap())
        .collect();
    assert!(
        messages
            .iter()
            .any(|m| m.contains("agent applied 2 command(s) to the edit world")),
        "{messages:?}"
    );
    assert!(messages.iter().any(|m| m.contains("agent pressed Play")));
    assert!(r["log"]["next"].as_u64().unwrap() >= messages.len() as u64);

    // Editing again, with the agent's edit as one undo step on the edit world.
    assert_eq!(editor.state.play_state(), PlayState::Editing);
    assert_eq!(editor.state.edits.undo_len(), 1);
    assert_eq!(editor.state.edits.undo_label(), Some("Agent edit"));
    assert!(editor.state.is_dirty());
    assert!(editor.title().contains(" •"), "{}", editor.title());
    let lamp = box_core::edit::resolve_entity(&editor.state.world, "Lamp").unwrap();
    assert!(
        box_core::edit::resolve_entity(&editor.state.world, "Play Lamp").is_err(),
        "the Play rename was discarded"
    );
    frames.frame(&mut editor);
    assert!(
        hierarchy_has(&editor, lamp),
        "the hierarchy shows the agent's entity"
    );
    // Cmd+Z undoes it: back to the scene as loaded.
    editor.state.undo();
    assert_eq!(scene::save(&editor.state.world).unwrap(), initial);
}

fn hierarchy_has(editor: &Editor, uid: Uid) -> bool {
    editor.state.hierarchy.rows.iter().any(|r| r.uid == uid)
}
