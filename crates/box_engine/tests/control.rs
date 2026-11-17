//! The live control socket over real localhost TCP: discovery file
//! lifecycle, authentication, malformed and oversized input, and every method
//! against a [`HeadlessHost`].
//!
//! The screenshot round trip needs a GPU; it is skipped when
//! `BOX_SKIP_GPU_TESTS` is set or no adapter exists.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use box_engine::Game;
use box_engine::box_core::edit::Command;
use box_engine::box_render::{RenderError, Renderer};
use box_engine::control::{
    ClientError, ControlClient, ControlError, ControlHost, ControlServer, Discovery, HeadlessHost,
    HostKind, MAX_REQUEST_BYTES, Mode, Status, base64_decode, discovery_path, read_discovery,
};
use box_engine::project::starter_scene;
use serde_json::{Value as Json, json};

const TIMEOUT: Duration = Duration::from_secs(10);

fn temp_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("control_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Run `client` on a thread while polling `server` with `host` on this one
/// (the host is not `Send`, like the editor's world).
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
    // Flush anything still queued.
    server.poll(host);
    match handle.join() {
        Ok(v) => v,
        Err(p) => std::panic::resume_unwind(p),
    }
}

fn host() -> HeadlessHost {
    HeadlessHost::new(Game::new(), starter_scene())
        .with_location(None, Some("assets/scenes/main.scene.json"))
}

/// One raw request line, one raw response line.
fn raw(stream: &mut BufReader<TcpStream>, line: &[u8]) -> Json {
    stream.get_mut().write_all(line).unwrap();
    let mut out = String::new();
    stream.read_line(&mut out).unwrap();
    serde_json::from_str(&out).unwrap_or_else(|e| panic!("bad response {out:?}: {e}"))
}

#[test]
fn discovery_file_lifecycle() {
    let project = temp_dir("discovery");
    let path = discovery_path(&project);
    assert!(matches!(
        read_discovery(&project),
        Err(ClientError::NotRunning(_))
    ));
    {
        let mut server =
            ControlServer::start(&project, HostKind::Editor, Some("a.scene.json")).unwrap();
        let d = read_discovery(&project).unwrap();
        assert_eq!(d, server.discovery());
        assert_eq!(d.version, 1);
        assert_eq!(d.pid, std::process::id());
        assert_eq!(d.kind, HostKind::Editor);
        assert_eq!(d.scene.as_deref(), Some("a.scene.json"));
        assert_eq!(d.token.len(), 64);
        assert_ne!(d.port, 0);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "token file must be private");
        }
        server.set_scene(Some("b.scene.json")).unwrap();
        assert_eq!(
            read_discovery(&project).unwrap().scene.as_deref(),
            Some("b.scene.json")
        );
    }
    assert!(
        !path.exists(),
        "dropping the server removes the discovery file"
    );

    // A newer instance replaced the file: the old one leaves it alone.
    let old = ControlServer::start(&project, HostKind::Player, None).unwrap();
    let new = ControlServer::start(&project, HostKind::Editor, None).unwrap();
    drop(old);
    assert_eq!(read_discovery(&project).unwrap().token, new.token());
    drop(new);
    assert!(!path.exists());
}

#[test]
fn rejects_bad_tokens_malformed_and_oversized_requests() {
    let mut server = ControlServer::bind(HostKind::Editor, None).unwrap();
    let mut host = host();
    let responses = drive(&mut server, &mut host, |d| {
        let connect = || {
            let s = TcpStream::connect(("127.0.0.1", d.port)).unwrap();
            s.set_read_timeout(Some(TIMEOUT)).unwrap();
            BufReader::new(s)
        };
        let mut s = connect();
        let token = d.token.clone();
        let mut out = vec![
            raw(
                &mut s,
                b"{\"id\":1,\"token\":\"wrong\",\"method\":\"status\"}\n",
            ),
            raw(&mut s, b"{nope\n"),
            raw(&mut s, b"[1,2]\n"),
        ];
        let line = |v: Json| format!("{v}\n").into_bytes();
        out.push(raw(
            &mut s,
            &line(json!({"id": 5, "token": token, "method": 7})),
        ));
        out.push(raw(
            &mut s,
            &line(json!({"id": 6, "token": token, "method": "status", "params": [1]})),
        ));
        out.push(raw(
            &mut s,
            &line(json!({"id": 7, "token": token, "method": "explode"})),
        ));
        out.push(raw(
            &mut s,
            &line(json!({"id": "s", "token": token, "method": "status"})),
        ));
        out.push(raw(
            &mut s,
            &line(
                json!({"id": 9, "token": token, "method": "step_frames", "params": {"frames": 0}}),
            ),
        ));
        out.push(raw(
            &mut s,
            &line(
                json!({"id": 10, "token": token, "method": "world_query", "params": {"bogus": 1}}),
            ),
        ));

        // Oversized: the server answers too_large and closes the connection.
        let mut big = connect();
        let blob = vec![b'x'; MAX_REQUEST_BYTES + 4096];
        let _ = big.get_mut().write_all(&blob);
        let mut resp = String::new();
        big.read_line(&mut resp).unwrap();
        out.push(serde_json::from_str(&resp).unwrap());
        let mut rest = String::new();
        assert_eq!(
            big.read_line(&mut rest).unwrap_or(0),
            0,
            "connection closed"
        );

        // The server still serves new clients.
        let mut client = ControlClient::connect(d, TIMEOUT).unwrap();
        out.push(client.call("status", json!({})).unwrap());
        out
    });
    let code = |i: usize| {
        responses[i]["error"]["code"]
            .as_str()
            .unwrap_or("")
            .to_string()
    };
    assert_eq!(code(0), "unauthorized");
    assert_eq!(responses[0]["id"], 1);
    assert_eq!(code(1), "bad_request", "{:#}", responses[1]);
    assert_eq!(responses[1]["id"], Json::Null);
    assert_eq!(code(2), "bad_request");
    assert_eq!(code(3), "bad_request", "method must be a string");
    assert_eq!(code(4), "invalid_params", "{:#}", responses[4]);
    assert_eq!(responses[4]["id"], 6);
    assert_eq!(code(5), "unknown_method");
    assert!(responses[5]["error"]["methods"].is_array());
    assert_eq!(responses[6]["id"], "s");
    assert_eq!(responses[6]["result"]["mode"], "edit");
    assert_eq!(code(7), "invalid_params");
    assert_eq!(code(8), "invalid_params");
    assert_eq!(code(9), "too_large");
    assert_eq!(responses[10]["kind"], "editor");
    assert_eq!(server.connections(), 0, "closed connections are dropped");
}

#[test]
fn wrong_and_missing_tokens_are_unauthorized() {
    let mut server = ControlServer::bind(HostKind::Player, None).unwrap();
    let mut host = host();
    let (wrong, missing) = drive(&mut server, &mut host, |d| {
        let mut bad = d.clone();
        bad.token = "0".repeat(64);
        let wrong = ControlClient::connect(bad, TIMEOUT)
            .unwrap()
            .call("status", json!({}));
        let s = TcpStream::connect(("127.0.0.1", d.port)).unwrap();
        s.set_read_timeout(Some(TIMEOUT)).unwrap();
        let missing = raw(
            &mut BufReader::new(s),
            b"{\"id\":1,\"method\":\"status\"}\n",
        );
        (wrong, missing)
    });
    match wrong {
        Err(ClientError::Remote(e)) => assert_eq!(e.code, "unauthorized"),
        other => panic!("expected unauthorized, got {other:?}"),
    }
    assert_eq!(missing["error"]["code"], "unauthorized");
    assert_eq!(missing["id"], 1);
}

fn gpu_renderer() -> Option<Renderer> {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping screenshot");
        return None;
    }
    match Renderer::new_headless(320, 180) {
        Ok(r) => Some(r),
        Err(RenderError::NoAdapter) => {
            eprintln!("no GPU adapter; skipping screenshot");
            None
        }
        Err(e) => panic!("headless renderer: {e}"),
    }
}

#[test]
fn every_method_round_trips() {
    let mut server =
        ControlServer::bind(HostKind::Editor, Some("assets/scenes/main.scene.json")).unwrap();
    let renderer = gpu_renderer();
    let has_gpu = renderer.is_some();
    let mut host = host();
    if let Some(r) = renderer {
        host = host.with_renderer(r);
    }
    let r = drive(&mut server, &mut host, move |d| {
        let mut c = ControlClient::connect(d, TIMEOUT).unwrap();
        let mut call = |m: &str, p: Json| c.call(m, p).map_err(|e| e.to_control_error());
        let mut r = serde_json::Map::new();
        let mut put = |k: &str, v: Result<Json, ControlError>| {
            r.insert(
                k.into(),
                v.unwrap_or_else(|e| json!({ "error": e.to_json() })),
            );
        };
        put("status", call("status", json!({})));
        put("all", call("world_query", json!({})));
        put("cube", call("world_query", json!({ "name": "CUBE" })));
        put(
            "cameras",
            call(
                "world_query",
                json!({ "component": "Camera", "include_components": false }),
            ),
        );
        put(
            "limited",
            call(
                "world_query",
                json!({ "limit": 1, "include_resources": true }),
            ),
        );
        put(
            "bad_component",
            call("world_query", json!({ "component": "Nope" })),
        );
        put("bad_uid", call("world_query", json!({ "uid": "x" })));
        let spawn = json!([
            { "op": "spawn", "name": "Ball", "components": {
                "Primitive": { "shape": { "Sphere": { "radius": 0.5, "sectors": 16, "stacks": 8 } }, "cast_shadows": true } } },
            { "op": "set", "entity": "Ball", "path": "Transform.translation", "value": [0.0, 2.0, 0.0] }
        ]);
        put(
            "dry",
            call(
                "apply_commands",
                json!({ "commands": spawn, "dry_run": true }),
            ),
        );
        put("after_dry", call("world_query", json!({ "name": "Ball" })));
        put(
            "apply",
            call("apply_commands", json!({ "commands": spawn })),
        );
        put("ball", call("world_query", json!({ "name": "Ball" })));
        put(
            "bad_cmd",
            call(
                "apply_commands",
                json!({ "commands": [
            { "op": "set", "entity": "Ball", "path": "Transform.translation.y", "value": 3.0 },
            { "op": "set", "entity": "Nobody", "path": "Transform.translation.y", "value": 1.0 }
        ] }),
            ),
        );
        put(
            "parse_cmd",
            call("apply_commands", json!({ "commands": [{ "op": "fly" }] })),
        );
        put("pause_edit", call("pause", json!({})));
        put("step_edit", call("step_frames", json!({ "frames": 2 })));
        put(
            "shot",
            call("screenshot", json!({ "width": 160, "height": 90 })),
        );
        put("shot_half", call("screenshot", json!({ "width": 160 })));
        put("play", call("play", json!({})));
        put(
            "play_edit",
            call(
                "apply_commands",
                json!({ "commands": { "op": "despawn", "entity": "Ball" } }),
            ),
        );
        put("play_ball", call("world_query", json!({ "name": "Ball" })));
        put("pause", call("pause", json!({})));
        put("step", call("step_frames", json!({ "frames": 5 })));
        put("paused_status", call("status", json!({})));
        put("stats", call("frame_stats", json!({})));
        put("stop", call("stop", json!({})));
        put("edit_ball", call("world_query", json!({ "name": "Ball" })));
        put("log", call("read_log", json!({})));
        put(
            "log_tail",
            call("read_log", json!({ "since": 3, "limit": 1 })),
        );
        r
    });
    let err = |k: &str| r[k]["error"]["code"].as_str().unwrap_or("<ok>").to_string();

    assert_eq!(r["status"]["mode"], "edit");
    assert_eq!(r["status"]["kind"], "editor");
    assert_eq!(r["status"]["scene"], "assets/scenes/main.scene.json");
    assert_eq!(r["status"]["frame"], 0);

    // The starter scene: Camera, Sun, Ground, Cube (4 saved entities).
    let all = r["all"]["entities"].as_array().unwrap();
    assert_eq!(r["all"]["total"], 4, "{:#}", r["all"]);
    assert!(all.iter().all(|e| e["uid"].is_string()));
    let cube = &r["cube"]["entities"][0];
    assert_eq!(r["cube"]["total"], 1);
    assert_eq!(cube["name"], "Cube");
    assert!(
        cube["components"]["Transform"]["translation"].is_array(),
        "{cube:#}"
    );
    assert!(cube["components"]["Primitive"]["shape"].is_object());
    assert!(cube["components"].get("Uid").is_none());
    assert_eq!(r["cameras"]["total"], 1);
    assert!(
        r["cameras"]["entities"][0]["components"]
            .as_array()
            .unwrap()
            .contains(&json!("Camera"))
    );
    assert_eq!(r["limited"]["entities"].as_array().unwrap().len(), 1);
    assert_eq!(r["limited"]["truncated"], true);
    assert!(
        r["limited"]["resources"]["Environment"].is_object(),
        "{:#}",
        r["limited"]
    );
    assert_eq!(err("bad_component"), "unknown_type");
    assert_eq!(err("bad_uid"), "invalid_params");

    assert_eq!(r["dry"]["dry_run"], true);
    assert_eq!(r["dry"]["created"].as_array().unwrap().len(), 1);
    assert_eq!(r["after_dry"]["total"], 0, "a dry run changes nothing");
    assert_eq!(r["apply"]["undoable"], true);
    let ball_uid = r["apply"]["created"][0].as_str().unwrap().to_string();
    assert_eq!(r["ball"]["entities"][0]["uid"], ball_uid.as_str());
    assert_eq!(
        r["ball"]["entities"][0]["components"]["Transform"]["translation"],
        json!([0.0, 2.0, 0.0])
    );
    assert_eq!(err("bad_cmd"), "entity_not_found");
    assert_eq!(r["bad_cmd"]["error"]["command"], 1);
    assert_eq!(r["bad_cmd"]["error"]["op"], "set");
    assert_eq!(err("parse_cmd"), "command_parse");
    assert_eq!(err("pause_edit"), "not_playing");
    assert_eq!(err("step_edit"), "not_playing");
    assert_eq!(err("shot_half"), "invalid_params");
    if has_gpu {
        assert_eq!(r["shot"]["width"], 160);
        assert_eq!(r["shot"]["mime_type"], "image/png");
        let png = base64_decode(r["shot"]["png_base64"].as_str().unwrap()).unwrap();
        let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/captures/control_screenshot.png");
        std::fs::create_dir_all(out.parent().unwrap()).unwrap();
        std::fs::write(&out, &png).unwrap();
        let img = image::load_from_memory(&png).unwrap();
        assert_eq!((img.width(), img.height()), (160, 90));
    } else {
        assert_eq!(err("shot"), "unsupported");
    }

    assert_eq!(r["play"]["mode"], "play");
    assert_eq!(r["play"]["frame"], 1);
    assert_eq!(r["play_edit"]["undoable"], true);
    assert_eq!(r["play_ball"]["total"], 0, "play-world edit applied");
    assert_eq!(r["pause"]["mode"], "paused");
    assert_eq!(r["step"], json!({ "frames": 5, "frame": 6 }));
    assert_eq!(r["paused_status"]["frame"], 6);
    assert_eq!(r["stats"]["mode"], "paused");
    assert_eq!(r["stats"]["frame"], 6);
    assert_eq!(r["stop"]["mode"], "edit");
    assert_eq!(r["edit_ball"]["total"], 1, "stop restores the edit world");

    let log: Vec<&str> = r["log"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["message"].as_str().unwrap())
        .collect();
    assert_eq!(
        log,
        [
            "applied 2 command(s)",
            "play",
            "applied 1 command(s)",
            "pause",
            "stop"
        ]
    );
    assert_eq!(r["log"]["next"], 5);
    assert_eq!(r["log_tail"]["entries"][0]["message"], "pause");
    assert_eq!(r["log_tail"]["next"], 4);

    // The edit happened through the host's undo stack.
    assert_eq!(host.edit_stack().undo_len(), 1);
    assert_eq!(host.mode(), Mode::Edit);
}

/// A host that implements only `status`.
struct Minimal;
impl ControlHost for Minimal {
    fn status(&mut self) -> Status {
        Status {
            mode: Mode::Play,
            scene: None,
            project: None,
            frame: 0,
            undo: None,
        }
    }
}

#[test]
fn unimplemented_methods_are_unsupported_and_clients_can_share_a_server() {
    let mut server = ControlServer::bind(HostKind::Player, None).unwrap();
    let results = drive(&mut server, &mut Minimal, |d| {
        let threads: Vec<_> = (0..3)
            .map(|_| {
                let d = d.clone();
                thread::spawn(move || {
                    let mut c = ControlClient::connect(d, TIMEOUT).unwrap();
                    let status = c.call("status", json!({})).unwrap();
                    let play = c.call("play", json!({})).unwrap_err();
                    let query = c.call("world_query", json!({})).unwrap_err();
                    (status, play.code().to_string(), query.code().to_string())
                })
            })
            .collect();
        threads
            .into_iter()
            .map(|t| t.join().unwrap())
            .collect::<Vec<_>>()
    });
    for (status, play, query) in results {
        assert_eq!(status["kind"], "player");
        assert_eq!(status["mode"], "play");
        assert_eq!(play, "unsupported");
        assert_eq!(query, "unsupported");
    }
}

#[test]
fn stale_discovery_files_are_reported() {
    let project = temp_dir("stale");
    match ControlClient::connect_project(&project, TIMEOUT) {
        Err(e @ ClientError::NotRunning(_)) => assert_eq!(e.code(), "not_running"),
        other => panic!("expected NotRunning, got {:?}", other.err()),
    }
    // A port nothing listens on: bind, note it, release it.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let stale = Discovery {
        version: 1,
        pid: 999_999,
        port,
        token: "t".into(),
        kind: HostKind::Editor,
        scene: None,
    };
    let path = discovery_path(&project);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, serde_json::to_vec(&stale).unwrap()).unwrap();
    match ControlClient::connect_project(&project, TIMEOUT) {
        Err(e @ ClientError::Stale { .. }) => {
            assert_eq!(e.code(), "not_running");
            assert!(e.to_string().contains("stale"), "{e}");
        }
        other => panic!("expected Stale, got {:?}", other.err()),
    }
    std::fs::write(&path, b"{ not json").unwrap();
    assert!(matches!(
        read_discovery(&project),
        Err(ClientError::Protocol(_))
    ));
}

#[test]
fn commands_reach_the_host_parsed() {
    // The apply params are parsed with the shared command parser.
    struct Recorder(Vec<Command>);
    impl ControlHost for Recorder {
        fn status(&mut self) -> Status {
            Minimal.status()
        }
        fn apply_commands(
            &mut self,
            commands: &[Command],
            dry_run: bool,
        ) -> Result<Json, ControlError> {
            self.0.extend_from_slice(commands);
            Ok(json!({ "dry_run": dry_run }))
        }
    }
    let mut server = ControlServer::bind(HostKind::Editor, None).unwrap();
    let mut host = Recorder(Vec::new());
    let out = drive(&mut server, &mut host, |d| {
        ControlClient::connect(d, TIMEOUT)
            .unwrap()
            .call(
                "apply_commands",
                json!({ "commands": { "op": "despawn", "entity": "Cube" }, "dry_run": true }),
            )
            .unwrap()
    });
    assert_eq!(out["dry_run"], true);
    assert_eq!(host.0.len(), 1);
    assert_eq!(host.0[0].op(), "despawn");
}
