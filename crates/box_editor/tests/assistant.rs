//! The Assistant panel (docs/AI_PANEL.md) with a scripted backend: no
//! network, no agent installed.
//!
//! - A turn's command lists become one undo step labelled after the
//!   request; the message carries the editor context; the transcript shows
//!   the agent's text and its steps in plain words.
//! - A user edit in the middle of a turn stays its own step, and then Undo
//!   Turn refuses (the turn's edits are not on top).
//! - Stop keeps what the turn changed, as one step, and Undo Turn reverts it.
//! - `undo_turn` (the agent's own) reverts the turn so far and keeps it open.
//! - Without a project there is no backend, and the panel says why.
//!
//! Capture: `target/captures/assistant/transcript.png`.

use std::path::PathBuf;

use box_core::edit::Command;
use box_editor::assistant::backend::{AgentEvent, ScriptStep, ScriptedBackend};
use box_editor::assistant::{TurnItem, TurnStatus};
use box_editor::headless::{CaptureOptions, Headless};
use box_editor::log::LogBuffer;
use box_editor::{EditorOptions, EditorState, build_editor, layout};
use serde_json::json;

fn cmds(v: serde_json::Value) -> Vec<Command> {
    serde_json::from_value(v).unwrap()
}

fn ev(e: AgentEvent) -> ScriptStep {
    ScriptStep::Event(e)
}

fn done() -> ScriptStep {
    ev(AgentEvent::TurnDone {
        cost_usd: Some(0.01),
        error: None,
    })
}

/// Polls until the running turn ends (at most 50 frames).
fn run(state: &mut EditorState) {
    for _ in 0..50 {
        if !state.poll_assistant() {
            return;
        }
    }
    panic!("the turn did not end");
}

#[test]
fn turns_are_undo_steps_with_a_plain_transcript() {
    let mut editor = build_editor(&EditorOptions::default(), LogBuffer::default()).expect("editor");
    editor.tree = layout::default_layout();

    // No project: no Claude Code backend, and the reason.
    let err = editor.state.assistant_send("hello").unwrap_err();
    assert!(err.contains("project"), "{err}");

    let turns = vec![
        // 1: two command lists, one step.
        vec![
            ev(AgentEvent::Text("Placing a pickup and a marker.".into())),
            ev(AgentEvent::ToolStart {
                id: "t1".into(),
                name: "apply_commands".into(),
                input: json!({ "commands": [{ "op": "add_preset", "preset": "pickup" }] }),
            }),
            ScriptStep::Apply(cmds(
                json!([{ "op": "add_preset", "preset": "pickup", "at": [0.0, 0.0, -3.0] }]),
            )),
            ev(AgentEvent::ToolDone {
                id: "t1".into(),
                ok: true,
                summary: "{\"created\": [..]}".into(),
            }),
            ScriptStep::Apply(cmds(json!([{ "op": "spawn", "name": "Marker" }]))),
            ev(AgentEvent::Text(
                "Done: press Play and walk into the coin.".into(),
            )),
            done(),
        ],
        // 2: interrupted by a user edit (applied by the test between polls).
        vec![
            ScriptStep::Apply(cmds(json!([{ "op": "spawn", "name": "Box A" }]))),
            ev(AgentEvent::Text("…".into())),
            ScriptStep::Apply(cmds(json!([{ "op": "spawn", "name": "Box B" }]))),
            done(),
        ],
        // 3: stopped after its first edit.
        vec![
            ScriptStep::Apply(cmds(json!([{ "op": "spawn", "name": "Half" }]))),
            ev(AgentEvent::Text("still going".into())),
            ScriptStep::Apply(cmds(json!([{ "op": "spawn", "name": "Never" }]))),
            done(),
        ],
        // 4: undoes its own first attempt.
        vec![
            ScriptStep::Apply(cmds(json!([{ "op": "spawn", "name": "Wrong" }]))),
            ev(AgentEvent::Text("That was wrong, trying again.".into())),
            done(),
        ],
    ];
    let state = &mut editor.state;
    state.select(None);
    state.set_assistant_backend(Box::new(ScriptedBackend::new(turns)), false);
    let undo_before = state.edits.undo_len();

    // Turn 1.
    state.assistant_send("add a pickup and a marker").unwrap();
    assert!(state.assistant_busy());
    assert!(state.assistant_send("again").is_err(), "one turn at a time");
    run(state);
    assert!(state.world.find_by_name("Pickup").is_some());
    assert!(state.world.find_by_name("Marker").is_some());
    assert_eq!(state.edits.undo_len(), undo_before + 1);
    assert_eq!(
        state.edits.undo_label(),
        Some("Assistant: add a pickup and a marker")
    );
    let t = &state.assistant_turns()[0];
    assert_eq!(t.status, TurnStatus::Done);
    assert_eq!(t.cost_usd, Some(0.01));
    assert!(t.items.contains(&TurnItem::Text(
        "Done: press Play and walk into the coin.".into()
    )));
    assert!(t.items.iter().any(|i| matches!(i,
        TurnItem::Step { label, ok: Some(true), .. } if label == "Placed pickup")));
    assert!(state.assistant_can_undo(0).is_ok());

    // Turn 2, with the user renaming the marker in the middle.
    state.assistant_send("add two boxes").unwrap();
    state.poll_assistant(); // Box A
    let marker = state.world.find_by_name("Marker").unwrap();
    let marker_uid = state.world.uid(marker).unwrap();
    state.rename(marker_uid, "Flag");
    run(state);
    let labels: Vec<String> = state.edits.undo_labels().map(str::to_string).collect();
    let n = labels.len();
    assert_eq!(
        labels[n - 3..],
        [
            "Assistant: add two boxes",
            "Set Name",
            "Assistant: add two boxes"
        ]
        .map(String::from)
    );
    let why = state.assistant_can_undo(1).unwrap_err();
    assert!(why.contains("later changes"), "{why}");
    assert!(
        state.assistant_can_undo(0).is_err(),
        "turn 1 is no longer on top"
    );

    // Turn 3, stopped.
    state.assistant_send("add something big").unwrap();
    state.poll_assistant(); // Half
    state.assistant_stop();
    assert_eq!(state.assistant_turns()[2].status, TurnStatus::Stopped);
    assert!(state.world.find_by_name("Half").is_some());
    assert!(state.world.find_by_name("Never").is_none());
    assert_eq!(
        state.edits.undo_label(),
        Some("Assistant: add something big")
    );
    state.assistant_undo(2).unwrap();
    assert!(state.world.find_by_name("Half").is_none());
    assert_eq!(state.assistant_turns()[2].status, TurnStatus::Undone);

    // Turn 4: the agent's undo_turn mid-turn.
    state.assistant_send("try something").unwrap();
    state.poll_assistant(); // Wrong
    assert!(state.world.find_by_name("Wrong").is_some());
    let ctx = state.editor_context();
    assert_eq!(ctx["turn"], "Assistant: try something");
    assert!(ctx["camera"]["eye"].is_array());
    assert!(state.undo_agent_turn_so_far());
    assert!(state.world.find_by_name("Wrong").is_none());
    assert!(state.edits.group_open(), "the turn goes on");
    run(state);
    assert!(!state.edits.group_open());

    // Capture the window.
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        return;
    }
    let options = CaptureOptions {
        size: [1440, 900],
        pixels_per_point: 1.0,
        frames: 1,
    };
    editor.state.set_assistant(true);
    editor
        .state
        .set_assistant_input("Make the coins spin faster");
    let mut h = match Headless::new(&mut editor, &options) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("skipped (no GPU): {e:#}");
            return;
        }
    };
    for _ in 0..4 {
        h.frame(&mut editor, Vec::new());
    }
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/assistant");
    std::fs::create_dir_all(&out).unwrap();
    h.image().unwrap().save(out.join("transcript.png")).unwrap();
}

/// The Claude Code backend's process plumbing with a stand-in `claude`: a
/// shell script that answers each stream-JSON message with a text, a tool
/// call and a result, and records its arguments. Stop kills it; the next
/// message starts it again with `--resume` and the session id.
#[cfg(unix)]
#[test]
fn claude_code_backend_talks_stream_json_to_a_process() {
    use box_editor::assistant::backend::AgentBackend;
    use box_editor::assistant::claude_code::{ClaudeCodeBackend, ClaudeCodeSetup};
    use std::os::unix::fs::PermissionsExt;

    let dir = std::env::temp_dir().join(format!("box_assistant_fake_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("claude");
    let args_file = dir.join("args.txt");
    std::fs::write(
        &script,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$@" >> '{args}'
echo '{{"type":"system","subtype":"init","session_id":"sess-42"}}'
while IFS= read -r line; do
  echo '{{"type":"assistant","message":{{"content":[{{"type":"text","text":"On it."}},{{"type":"tool_use","id":"t1","name":"mcp__box__check","input":{{}}}}]}}}}'
  echo '{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"t1","content":"ok"}}]}}}}'
  echo '{{"type":"result","subtype":"success","is_error":false,"result":"done","total_cost_usd":0.001}}'
done
"#,
            args = args_file.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let setup = ClaudeCodeSetup {
        claude: script,
        box_exe: "/bin/true".into(),
    };
    let mut b = ClaudeCodeBackend::new(setup, &dir, "rules");
    let turn = |b: &mut ClaudeCodeBackend| {
        let mut events = Vec::new();
        for _ in 0..200 {
            events.extend(b.poll());
            if events
                .iter()
                .any(|e| matches!(e, AgentEvent::TurnDone { .. }))
            {
                return events;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("no TurnDone: {events:?}");
    };
    b.send("first").unwrap();
    let events = turn(&mut b);
    assert!(events.contains(&AgentEvent::Session("sess-42".into())));
    assert!(events.contains(&AgentEvent::Text("On it.".into())));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::ToolStart { name, .. } if name == "check"))
    );
    // A second turn on the same process.
    b.send("second").unwrap();
    turn(&mut b);
    // Stop, then the next message resumes the session in a new process.
    b.cancel();
    b.send("third").unwrap();
    turn(&mut b);
    let args = std::fs::read_to_string(&args_file).unwrap();
    assert_eq!(
        args.matches("--strict-mcp-config").count(),
        2,
        "two processes"
    );
    assert!(args.contains("--resume\nsess-42"), "{args}");
    let _ = std::fs::remove_dir_all(dir);
}

/// End to end with the real Claude Code (needs `claude` installed and
/// signed in, and `cargo build -p box_cli`; uses the account's quota):
/// a fresh project in a headless editor, one request, frames and the
/// control socket pumped until the turn ends. Prints the transcript;
/// capture `target/captures/assistant/real_turn.png`.
///
/// `cargo test -p box_editor --test assistant real_claude_code -- --ignored --nocapture`
#[test]
#[ignore]
fn real_claude_code_builds_a_small_game() {
    let dir = std::env::temp_dir().join(format!("box_assistant_e2e_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let list = box_engine::templates::templates();
    let empty = list
        .iter()
        .find(|t| t.name == "empty")
        .expect("empty template");
    box_engine::templates::create_project(&dir, "E2E Game", &list, empty).expect("project");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let box_exe = root
        .join("target/debug/box")
        .canonicalize()
        .expect("build box_cli first");
    let options = box_editor::EditorOptions {
        project: Some(dir.clone()),
        ..Default::default()
    };
    let mut editor = build_editor(&options, LogBuffer::default()).expect("editor");
    editor.tree = layout::default_layout();
    let capture = CaptureOptions {
        size: [1600, 1000],
        pixels_per_point: 1.0,
        frames: 1,
    };
    let mut h = Headless::new(&mut editor, &capture).expect("GPU");
    for _ in 0..3 {
        h.frame(&mut editor, Vec::new());
    }
    // The real backend, with this checkout's `box`.
    let mut setup = box_editor::assistant::claude_code::ClaudeCodeSetup::find().expect("claude");
    setup.box_exe = box_exe;
    let project = editor.state.project.as_ref().unwrap().root().to_path_buf();
    let backend = box_editor::assistant::claude_code::ClaudeCodeBackend::new(
        setup,
        &project,
        box_editor::assistant::SYSTEM_PROMPT,
    );
    editor.state.set_assistant_backend(Box::new(backend), true);
    editor.state.set_assistant(true);
    let request = "Make a tiny game here: a player, five pickups in a row in front of the \
                   player, and a goal past them that wins once every pickup is collected. \
                   Then check the project.";
    editor.state.assistant_send(request).expect("send");
    let start = std::time::Instant::now();
    while editor.state.assistant_busy() {
        h.frame(&mut editor, Vec::new());
        std::thread::sleep(std::time::Duration::from_millis(15));
        assert!(
            start.elapsed().as_secs() < 600,
            "no answer within 10 minutes"
        );
    }
    for _ in 0..4 {
        h.frame(&mut editor, Vec::new());
    }
    let turn = editor.state.assistant_turns()[0].clone();
    println!(
        "status: {:?} in {:.0} s, cost {:?}",
        turn.status,
        start.elapsed().as_secs_f32(),
        turn.cost_usd
    );
    for item in &turn.items {
        match item {
            TurnItem::Text(t) => println!("TEXT: {t}"),
            TurnItem::Step {
                label, ok, detail, ..
            } => {
                println!(
                    "STEP {ok:?}: {label} | {}",
                    detail.chars().take(160).collect::<String>()
                )
            }
        }
    }
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/assistant");
    std::fs::create_dir_all(&out).unwrap();
    h.image().unwrap().save(out.join("real_turn.png")).unwrap();
    assert_eq!(turn.status, TurnStatus::Done, "{turn:?}");
    let w = &editor.state.world;
    let players = w
        .query::<&box_engine::behaviours::PlayerController>()
        .iter()
        .count();
    let pickups = box_engine::interact::group_progress(w, "pickups").1;
    println!(
        "players {players}, pickups {pickups}, undo {:?}",
        editor.state.edits.undo_label()
    );
    assert_eq!(players, 1);
    assert!(pickups >= 5, "{pickups} pickups");
    assert_eq!(
        editor
            .state
            .edits
            .undo_label()
            .map(|l| l.starts_with("Assistant: ")),
        Some(true)
    );
    assert!(
        editor.state.agent_control().is_none(),
        "control is off again"
    );
}
