//! `box mcp` end to end: the built binary with piped stdio, driven like an
//! MCP client. Project tools on a project made with `box new`, both protocol
//! eras, and live tools against a control server in this process.
//!
//! Renders and screenshots are skipped when `BOX_SKIP_GPU_TESTS` is set or no
//! GPU adapter exists.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use box_engine::box_render::{RenderError, Renderer};
use box_engine::control::{ControlServer, Discovery, HeadlessHost, HostKind, discovery_path};
use box_engine::{Game, Project};
use serde_json::{Value as Json, json};

const SCENE: &str = "assets/scenes/main.scene.json";
const MODERN: &str = "2026-07-28";

fn temp_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("mcp_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn new_project(name: &str) -> PathBuf {
    let base = temp_dir(name);
    let out = Command::new(env!("CARGO_BIN_EXE_box"))
        .current_dir(&base)
        .args(["new", "game", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "box new failed");
    base.join("game")
}

fn gpu_tests_enabled() -> bool {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping GPU checks");
        return false;
    }
    true
}

/// A `box mcp` child process and a minimal MCP client.
struct Mcp {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    /// `Some` for modern (per-request `_meta`) requests.
    modern: bool,
}

impl Mcp {
    fn spawn(cwd: &Path, args: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_box"))
            .current_dir(cwd)
            .arg("mcp")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn box mcp");
        let stdin = child.stdin.take();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            stdin,
            stdout,
            next_id: 1,
            modern: false,
        }
    }

    fn send_line(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(line.as_bytes()).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
    }

    fn read(&mut self) -> Json {
        let mut line = String::new();
        let n = self.stdout.read_line(&mut line).unwrap();
        assert!(n > 0, "box mcp closed stdout");
        serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("stdout is not JSON-RPC ({e}): {line}"))
    }

    /// Send a request; return the whole response.
    fn request(&mut self, method: &str, mut params: Json) -> Json {
        let id = self.next_id;
        self.next_id += 1;
        if self.modern {
            params["_meta"] = json!({
                "io.modelcontextprotocol/protocolVersion": MODERN,
                "io.modelcontextprotocol/clientInfo": { "name": "box-test", "version": "1" },
                "io.modelcontextprotocol/clientCapabilities": {},
            });
        }
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.send_line(&msg.to_string());
        let response = self.read();
        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["id"], id, "{response:#}");
        response
    }

    fn notify(&mut self, method: &str) {
        self.send_line(&json!({ "jsonrpc": "2.0", "method": method }).to_string());
    }

    /// `tools/call`; returns the result (a tool result, possibly isError).
    fn call(&mut self, tool: &str, args: Json) -> Json {
        let r = self.request("tools/call", json!({ "name": tool, "arguments": args }));
        assert!(r.get("error").is_none(), "{tool}: protocol error {r:#}");
        let result = r["result"].clone();
        assert!(result["content"].is_array(), "{tool}: {result:#}");
        // The text item always repeats the structured result.
        let text = result["content"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["type"] == "text")
            .unwrap()["text"]
            .as_str()
            .unwrap()
            .to_string();
        let parsed: Json = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed, result["structuredContent"]);
        result
    }

    fn ok(&mut self, tool: &str, args: Json) -> Json {
        let r = self.call(tool, args);
        assert_eq!(r["isError"], false, "{tool} failed: {r:#}");
        r
    }

    fn fails(&mut self, tool: &str, args: Json) -> Json {
        let r = self.call(tool, args);
        assert_eq!(r["isError"], true, "{tool} should fail: {r:#}");
        r["structuredContent"]["error"].clone()
    }

    /// Close stdin and expect a clean exit.
    fn finish(mut self) {
        drop(self.stdin.take());
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "box mcp exited with {status}");
                // Nothing else was written to stdout.
                let mut rest = String::new();
                self.stdout.read_line(&mut rest).unwrap();
                assert!(rest.is_empty(), "stray stdout: {rest}");
                return;
            }
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "box mcp did not exit on EOF"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
}

fn image_png(result: &Json) -> Vec<u8> {
    let image = result["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["type"] == "image")
        .unwrap_or_else(|| panic!("no image content: {result:#}"));
    assert_eq!(image["mimeType"], "image/png");
    box_engine::control::base64_decode(image["data"].as_str().unwrap()).unwrap()
}

const TOOL_NAMES: [&str; 17] = [
    "check",
    "registry",
    "scene_list",
    "scene_show",
    "scene_apply",
    "render",
    "import",
    "status",
    "world_query",
    "apply_commands",
    "screenshot",
    "play",
    "stop",
    "pause",
    "step_frames",
    "read_log",
    "frame_stats",
];

/// The acceptance conversation over the legacy handshake (2025-11-25), the
/// one most MCP clients use today.
#[test]
fn mcp_conversation_with_project_tools() {
    let project = new_project("conversation");
    let mut mcp = Mcp::spawn(&project, &[]);

    let init = mcp.request(
        "initialize",
        json!({
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": { "name": "box-test", "version": "1" }
        }),
    );
    assert_eq!(init["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(init["result"]["serverInfo"]["name"], "box");
    assert!(init["result"]["capabilities"]["tools"].is_object());
    assert!(
        init["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("registry")
    );
    mcp.notify("notifications/initialized");
    assert_eq!(mcp.request("ping", json!({}))["result"], json!({}));

    let list = mcp.request("tools/list", json!({}));
    let tools = list["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, TOOL_NAMES);
    for t in tools {
        assert_eq!(t["inputSchema"]["type"], "object", "{t:#}");
        assert!(t["description"].as_str().unwrap().len() > 40);
        assert!(t["inputSchema"]["properties"].is_object());
    }
    let apply = tools.iter().find(|t| t["name"] == "scene_apply").unwrap();
    assert_eq!(apply["inputSchema"]["required"], json!(["commands"]));
    assert_eq!(apply["annotations"]["readOnlyHint"], false);

    // Registry, one type.
    let reg = mcp.ok("registry", json!({ "name": "Transform" }));
    let comps = reg["structuredContent"]["components"].as_array().unwrap();
    assert_eq!(comps.len(), 1);
    assert_eq!(comps[0]["name"], "Transform");
    assert_eq!(
        mcp.fails("registry", json!({ "name": "Nope" }))["code"],
        "unknown_type"
    );

    // Show the start scene (default scene, project = cwd).
    let show = mcp.ok("scene_show", json!({}));
    assert_eq!(
        show["structuredContent"]["entities"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    let cube = mcp.ok("scene_show", json!({ "scene": SCENE, "entity": "Cube" }));
    assert_eq!(cube["structuredContent"]["entity"]["name"], "Cube");
    assert_eq!(
        mcp.ok("scene_list", json!({}))["structuredContent"]["scenes"][0]["path"],
        SCENE
    );

    // Apply: spawn and move, then a failing transaction that changes nothing.
    let spawn = json!([
        { "op": "spawn", "name": "Ball", "components": {
            "Primitive": { "shape": { "Sphere": { "radius": 0.5, "sectors": 32, "stacks": 16 } }, "cast_shadows": true },
            "StandardMaterial": { "base_color": [0.9, 0.7, 0.2], "roughness": 0.3, "metallic": 1.0, "emissive": [0.0, 0.0, 0.0] } } },
        { "op": "set", "entity": "Ball", "path": "Transform.translation", "value": [1.5, 0.5, 0.5] }
    ]);
    let dry = mcp.ok("scene_apply", json!({ "commands": spawn, "dry_run": true }));
    assert_eq!(dry["structuredContent"]["written"], false);
    let applied = mcp.ok("scene_apply", json!({ "commands": spawn, "scene": SCENE }));
    assert_eq!(
        applied["structuredContent"]["created"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let err = mcp.fails(
        "scene_apply",
        json!({ "commands": { "op": "set", "entity": "Nobody", "path": "Transform.translation.x", "value": 1.0 } }),
    );
    assert_eq!(err["code"], "entity_not_found");
    assert_eq!(err["command"], 0);
    let show = mcp.ok("scene_show", json!({ "entity": "Ball" }));
    assert_eq!(
        show["structuredContent"]["entity"]["components"]["Transform"]["translation"],
        json!([1.5, 0.5, 0.5])
    );
    assert_eq!(mcp.ok("check", json!({}))["structuredContent"]["ok"], true);

    // Bad arguments are tool errors; unknown tools and methods are protocol errors.
    assert_eq!(
        mcp.fails("scene_show", json!({ "entitee": "Cube" }))["code"],
        "invalid_arguments"
    );
    assert_eq!(
        mcp.fails("scene_apply", json!({}))["code"],
        "invalid_arguments"
    );
    let unknown_tool = mcp.request("tools/call", json!({ "name": "fly", "arguments": {} }));
    assert_eq!(unknown_tool["error"]["code"], -32602);
    let unknown = mcp.request("resources/list", json!({}));
    assert_eq!(unknown["error"]["code"], -32601);
    assert!(
        unknown["error"]["message"]
            .as_str()
            .unwrap()
            .contains("resources/list")
    );

    // Live tools without a running editor: a clear error.
    let live = mcp.fails("status", json!({}));
    assert_eq!(live["code"], "not_running");
    assert!(
        live["message"].as_str().unwrap().contains("control.json"),
        "{live:#}"
    );
    // It says how to start one with live control.
    for how in [
        "--control` (or File ▸ Allow Agent Control in an open editor)",
        "`box-player ",
        "`box run ",
    ] {
        assert!(
            live["message"].as_str().unwrap().contains(how),
            "{how}: {live:#}"
        );
    }

    // Render returns the PNG as image content.
    if gpu_tests_enabled() {
        let r = mcp.call("render", json!({ "width": 480, "height": 270 }));
        if r["structuredContent"]["error"]["code"] == "no_gpu" {
            eprintln!("no GPU adapter; skipping render");
        } else {
            assert_eq!(r["isError"], false, "{r:#}");
            assert_eq!(r["structuredContent"]["objects"], 3);
            assert_eq!(r["structuredContent"]["validation_errors"], 0);
            assert!(r["structuredContent"].get("out").is_none());
            let png = image_png(&r);
            let img = image::load_from_memory(&png).unwrap();
            assert_eq!((img.width(), img.height()), (480, 270));
            let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/captures/mcp_render.png");
            std::fs::create_dir_all(out.parent().unwrap()).unwrap();
            std::fs::write(&out, &png).unwrap();
        }
    }
    mcp.finish();
}

/// The stateless 2026-07-28 revision: `server/discover` and per-request `_meta`.
#[test]
fn mcp_modern_protocol() {
    let project = new_project("modern");
    let elsewhere = temp_dir("modern_cwd");
    let mut mcp = Mcp::spawn(&elsewhere, &["--project", project.to_str().unwrap()]);

    // Without _meta and without initialize: invalid params.
    let r = mcp.request("tools/list", json!({}));
    assert_eq!(r["error"]["code"], -32602);

    mcp.modern = true;
    let d = mcp.request("server/discover", json!({}));
    assert_eq!(d["result"]["resultType"], "complete");
    assert_eq!(d["result"]["supportedVersions"], json!([MODERN]));
    assert!(d["result"]["capabilities"]["tools"].is_object());
    assert_eq!(
        d["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "box"
    );

    let list = mcp.request("tools/list", json!({}));
    assert_eq!(list["result"]["resultType"], "complete");
    assert_eq!(
        list["result"]["tools"].as_array().unwrap().len(),
        TOOL_NAMES.len()
    );
    assert!(list["result"]["ttlMs"].is_number());

    let show = mcp.ok("scene_show", json!({}));
    assert_eq!(show["resultType"], "complete");
    assert_eq!(
        show["structuredContent"]["entities"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    // `project` argument overrides --project; a non-project path is an error.
    let err = mcp.fails(
        "scene_list",
        json!({ "project": elsewhere.to_str().unwrap() }),
    );
    assert_eq!(err["code"], "not_a_project");

    let mut bad = json!({ "jsonrpc": "2.0", "id": 99, "method": "tools/list", "params": { "_meta": {
        "io.modelcontextprotocol/protocolVersion": "2025-11-25",
        "io.modelcontextprotocol/clientCapabilities": {} } } });
    mcp.send_line(&bad.to_string());
    let r = mcp.read();
    assert_eq!(r["error"]["code"], -32022);
    assert_eq!(r["error"]["data"]["supported"], json!([MODERN]));
    bad["params"]["_meta"] = json!({ "io.modelcontextprotocol/protocolVersion": MODERN });
    mcp.send_line(&bad.to_string());
    assert_eq!(
        mcp.read()["error"]["code"],
        -32602,
        "clientCapabilities is required"
    );
    mcp.send_line("{ this is not json");
    let r = mcp.read();
    assert_eq!(r["error"]["code"], -32700);
    assert_eq!(r["id"], Json::Null);
    mcp.finish();
}

/// `box mcp` drives a live host (a HeadlessHost behind a real control
/// server in this process): status, world_query, apply_commands, screenshot,
/// play, pause, step_frames, read_log, frame_stats and stop.
#[test]
fn mcp_live_tools_end_to_end() {
    let project = new_project("live");
    let scene = Project::open(&project).unwrap().start_scene_path();
    let world = box_engine::box_core::scene::load(
        &std::fs::read(&scene).unwrap(),
        box_engine::shared_engine_registry(),
    )
    .unwrap();
    let mut host = HeadlessHost::new(Game::new(), world).with_location(Some(&project), Some(SCENE));
    let gpu = gpu_tests_enabled()
        && match Renderer::new_headless(320, 180) {
            Ok(r) => {
                host = host.with_renderer(r);
                true
            }
            Err(RenderError::NoAdapter) => false,
            Err(e) => panic!("{e}"),
        };
    let mut server = ControlServer::start(&project, HostKind::Editor, Some(SCENE)).unwrap();

    let project_arg = project.to_str().unwrap().to_string();
    let conversation = thread::spawn(move || {
        let mut mcp = Mcp::spawn(Path::new(&project_arg), &[]);
        mcp.request(
            "initialize",
            json!({ "protocolVersion": "2025-06-18", "capabilities": {} }),
        );
        mcp.notify("notifications/initialized");
        let mut r = serde_json::Map::new();
        r.insert(
            "status".into(),
            mcp.ok("status", json!({}))["structuredContent"].clone(),
        );
        r.insert(
            "cube".into(),
            mcp.ok("world_query", json!({ "name": "cube" }))["structuredContent"].clone(),
        );
        r.insert(
            "apply".into(),
            mcp.ok("apply_commands", json!({ "commands": [
                { "op": "spawn", "name": "Lamp", "components": {
                    "Primitive": { "shape": { "Sphere": { "radius": 0.3, "sectors": 16, "stacks": 8 } }, "cast_shadows": false } } },
                { "op": "set", "entity": "Lamp", "path": "Transform.translation.y", "value": 2.0 }
            ] }))["structuredContent"]
                .clone(),
        );
        r.insert(
            "apply_err".into(),
            mcp.fails(
                "apply_commands",
                json!({ "commands": { "op": "rename", "entity": "Nobody", "name": "x" } }),
            ),
        );
        if gpu {
            let shot = mcp.ok("screenshot", json!({ "width": 320, "height": 180 }));
            let png = image_png(&shot);
            r.insert(
                "shot".into(),
                json!({ "meta": shot["structuredContent"], "len": png.len() }),
            );
            let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/captures/mcp_live_screenshot.png");
            std::fs::create_dir_all(out.parent().unwrap()).unwrap();
            std::fs::write(&out, &png).unwrap();
        } else {
            r.insert("shot".into(), mcp.fails("screenshot", json!({})));
        }
        r.insert(
            "play".into(),
            mcp.ok("play", json!({}))["structuredContent"].clone(),
        );
        r.insert(
            "pause".into(),
            mcp.ok("pause", json!({}))["structuredContent"].clone(),
        );
        r.insert(
            "step".into(),
            mcp.ok("step_frames", json!({ "frames": 10 }))["structuredContent"].clone(),
        );
        r.insert(
            "stats".into(),
            mcp.ok("frame_stats", json!({}))["structuredContent"].clone(),
        );
        r.insert(
            "stop".into(),
            mcp.ok("stop", json!({}))["structuredContent"].clone(),
        );
        r.insert(
            "log".into(),
            mcp.ok("read_log", json!({ "since": 0 }))["structuredContent"].clone(),
        );
        (mcp, r)
    });
    let start = Instant::now();
    while !conversation.is_finished() {
        server.poll(&mut host);
        thread::sleep(Duration::from_millis(1));
        assert!(
            start.elapsed() < Duration::from_secs(120),
            "conversation hung"
        );
    }
    let (mut mcp, r) = conversation.join().unwrap();

    assert_eq!(r["status"]["kind"], "editor");
    assert_eq!(r["status"]["mode"], "edit");
    assert_eq!(r["status"]["scene"], SCENE);
    assert_eq!(r["cube"]["entities"][0]["name"], "Cube");
    assert_eq!(r["apply"]["created"].as_array().unwrap().len(), 1);
    assert_eq!(r["apply_err"]["code"], "entity_not_found");
    if gpu {
        assert_eq!(r["shot"]["meta"]["width"], 320);
        assert!(r["shot"]["meta"].get("png_base64").is_none());
        assert!(r["shot"]["len"].as_u64().unwrap() > 1000);
    } else {
        assert_eq!(r["shot"]["code"], "unsupported");
    }
    assert_eq!(r["play"]["mode"], "play");
    assert_eq!(r["pause"]["mode"], "paused");
    assert_eq!(r["step"]["frame"], 11);
    assert_eq!(r["stats"]["frame"], 11);
    assert_eq!(r["stop"]["mode"], "edit");
    assert!(r["log"]["entries"].as_array().unwrap().len() >= 4);

    // The edit is in the host's world and undo stack.
    assert_eq!(host.edit_stack().undo_len(), 1);
    let lamp = box_engine::box_core::edit::resolve_entity(host.edit_world(), "Lamp").unwrap();
    assert!(host.edit_world().entity_by_uid(lamp).is_some());

    // The host goes away: the file is removed, live tools say not_running.
    drop(server);
    assert!(!discovery_path(&project).exists());
    assert_eq!(mcp.fails("status", json!({}))["code"], "not_running");

    // A crashed host leaves a stale file: still not_running, with a hint.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let stale = Discovery {
        version: 1,
        pid: 999_999,
        port,
        token: "x".into(),
        kind: HostKind::Player,
        scene: None,
    };
    std::fs::write(
        discovery_path(&project),
        serde_json::to_vec(&stale).unwrap(),
    )
    .unwrap();
    let err = mcp.fails("world_query", json!({}));
    assert_eq!(err["code"], "not_running");
    assert!(
        err["message"].as_str().unwrap().contains("stale"),
        "{err:#}"
    );
    mcp.finish();
}

/// Kills a child process if the test fails before it exits on its own.
struct ChildGuard(Option<Child>);

impl ChildGuard {
    fn child(&mut self) -> &mut Child {
        self.0.as_mut().expect("child already exited")
    }

    fn wait_exit(&mut self, timeout: Duration) -> std::process::ExitStatus {
        let start = Instant::now();
        loop {
            if let Some(status) = self.child().try_wait().unwrap() {
                self.0 = None;
                return status;
            }
            assert!(start.elapsed() < timeout, "the player did not exit");
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// A headless player with `--control` on `project`: the `box-player` binary
/// when it was built next to `box` (always under `cargo test --workspace`),
/// else `box run --control`, which runs the same player library in-process.
fn live_player(project: &Path, stderr: &Path) -> Command {
    let player = Path::new(env!("CARGO_BIN_EXE_box"))
        .with_file_name(format!("box-player{}", std::env::consts::EXE_SUFFIX));
    let mut cmd = if player.is_file() {
        let mut cmd = Command::new(player);
        cmd.arg(project)
            .args(["--headless", "--control", "--size", "320x180"]);
        cmd
    } else {
        eprintln!("box-player is not built; using `box run --control`");
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_box"));
        cmd.arg("run")
            .arg(project)
            .args(["--control", "--frames", "1000000", "--size", "320x180"]);
        cmd
    };
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(stderr).unwrap());
    cmd
}

/// The real `box mcp` binary drives a real headless player started with
/// `--control`: status, world_query, screenshot, pause, step_frames, play,
/// read_log and stop (which ends the player). Needs a GPU for the player.
#[test]
fn mcp_drives_a_running_player() {
    if !gpu_tests_enabled() {
        return;
    }
    let project = new_project("player");
    let stderr = project.join("player.log");
    let mut player = ChildGuard(Some(live_player(&project, &stderr).spawn().unwrap()));
    let start = Instant::now();
    while !discovery_path(&project).is_file() {
        if let Some(status) = player.child().try_wait().unwrap() {
            let log = std::fs::read_to_string(&stderr).unwrap_or_default();
            if log.contains("no suitable GPU adapter") {
                eprintln!("no GPU adapter; skipping");
                return;
            }
            panic!("the player exited with {status}:\n{log}");
        }
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "no discovery file"
        );
        thread::sleep(Duration::from_millis(20));
    }

    let mut mcp = Mcp::spawn(&project, &[]);
    mcp.request(
        "initialize",
        json!({ "protocolVersion": "2025-06-18", "capabilities": {} }),
    );
    mcp.notify("notifications/initialized");

    let status = mcp.ok("status", json!({}))["structuredContent"].clone();
    assert_eq!(status["kind"], "player");
    assert_eq!(status["mode"], "play");
    assert_eq!(status["scene"], SCENE);

    let cube = mcp.ok("world_query", json!({ "name": "cube" }))["structuredContent"].clone();
    assert_eq!(cube["entities"][0]["name"], "Cube");

    let shot = mcp.ok("screenshot", json!({}));
    assert_eq!(shot["structuredContent"]["width"], 320);
    assert_eq!(shot["structuredContent"]["height"], 180);
    let png = image_png(&shot);
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");

    let paused = mcp.ok("pause", json!({}))["structuredContent"].clone();
    assert_eq!(paused["mode"], "paused");
    let frame = paused["frame"].as_u64().unwrap();
    let step = mcp.ok("step_frames", json!({ "frames": 5 }))["structuredContent"].clone();
    assert_eq!(step, json!({ "frames": 5, "frame": frame + 5 }));
    assert_eq!(
        mcp.ok("status", json!({}))["structuredContent"]["frame"],
        frame + 5
    );
    assert_eq!(
        mcp.ok("play", json!({}))["structuredContent"]["mode"],
        "play"
    );

    let log = mcp.ok("read_log", json!({ "since": 0 }))["structuredContent"].clone();
    assert!(
        log["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["message"].as_str().unwrap().contains("control socket")),
        "{log:#}"
    );

    mcp.ok("stop", json!({}));
    let status = player.wait_exit(Duration::from_secs(30));
    assert!(
        status.success(),
        "exit {status}:\n{}",
        std::fs::read_to_string(&stderr).unwrap_or_default()
    );
    assert!(!discovery_path(&project).exists());
    assert_eq!(mcp.fails("status", json!({}))["code"], "not_running");
    mcp.finish();
}

/// M3 acceptance (ROADMAP): an MCP client connected to `box mcp` queries the
/// running editor's world, applies a command, takes a viewport screenshot,
/// and presses Play and Stop. The editor is a real headless `box_editor`
/// with Allow Agent Control on, running frames on this thread; `box mcp` is
/// the built binary. Without a GPU the editor runs its UI only and the
/// screenshot answers `unsupported`.
#[test]
fn mcp_drives_the_running_editor() {
    use box_editor::headless::{CaptureOptions, Headless};

    let project = new_project("editor");
    let options = box_editor::EditorOptions {
        project: Some(project.clone()),
        control: true,
        ..Default::default()
    };
    let mut editor =
        box_editor::build_editor(&options, box_editor::log::LogBuffer::default()).unwrap();
    editor.tree = box_editor::layout::default_layout();
    editor.state.autosave_dir = project.join(".box/autosave");
    assert!(discovery_path(&project).exists());
    let before = box_engine::box_core::scene::save(&editor.state.world).unwrap();

    let mut headless = None;
    if gpu_tests_enabled() {
        let capture = CaptureOptions {
            size: [1280, 720],
            pixels_per_point: 1.0,
            frames: 1,
        };
        match Headless::new(&mut editor, &capture) {
            Ok(h) => headless = Some(h),
            Err(e)
                if matches!(
                    e.downcast_ref::<RenderError>(),
                    Some(RenderError::NoAdapter)
                ) => {}
            Err(e) => panic!("{e:#}"),
        }
    }
    let gpu = headless.is_some();
    let ctx = egui::Context::default();
    let mut frame = 0u32;
    let mut run_frame = |editor: &mut box_editor::Editor| {
        match &mut headless {
            Some(h) => {
                h.frame(editor, Vec::new());
            }
            None => {
                editor.poll_control(None);
                let raw = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1280.0, 720.0),
                    )),
                    time: Some(f64::from(frame) / 60.0),
                    focused: true,
                    ..Default::default()
                };
                let _ = ctx.run_ui(raw, |ui| editor.ui(ui));
            }
        }
        frame += 1;
    };
    run_frame(&mut editor);

    let project_arg = project.to_str().unwrap().to_string();
    let conversation = thread::spawn(move || {
        let mut mcp = Mcp::spawn(Path::new(&project_arg), &[]);
        mcp.request(
            "initialize",
            json!({ "protocolVersion": "2025-06-18", "capabilities": {} }),
        );
        mcp.notify("notifications/initialized");
        let mut r = serde_json::Map::new();
        let ok = |mcp: &mut Mcp,
                  r: &mut serde_json::Map<String, Json>,
                  name: &str,
                  tool: &str,
                  args: Json| {
            let result = mcp.ok(tool, args);
            r.insert(name.into(), result["structuredContent"].clone());
            result
        };
        ok(&mut mcp, &mut r, "status", "status", json!({}));
        ok(
            &mut mcp,
            &mut r,
            "query",
            "world_query",
            json!({ "name": "cube" }),
        );
        ok(
            &mut mcp,
            &mut r,
            "apply",
            "apply_commands",
            json!({ "commands": [
                { "op": "spawn", "name": "Agent Lamp", "components": {
                    "Primitive": { "shape": { "Sphere": { "radius": 0.3, "sectors": 16, "stacks": 8 } } } } },
                { "op": "set", "entity": "Agent Lamp", "path": "Transform.translation.y", "value": 2.0 }
            ] }),
        );
        let png = if gpu {
            let shot = ok(
                &mut mcp,
                &mut r,
                "shot",
                "screenshot",
                json!({ "width": 480, "height": 270 }),
            );
            Some(image_png(&shot))
        } else {
            r.insert("shot".into(), mcp.fails("screenshot", json!({})));
            None
        };
        ok(&mut mcp, &mut r, "play", "play", json!({}));
        ok(
            &mut mcp,
            &mut r,
            "play_query",
            "world_query",
            json!({ "name": "agent lamp" }),
        );
        ok(&mut mcp, &mut r, "stop", "stop", json!({}));
        ok(
            &mut mcp,
            &mut r,
            "after",
            "world_query",
            json!({ "name": "agent lamp" }),
        );
        (mcp, r, png)
    });
    let start = Instant::now();
    while !conversation.is_finished() {
        run_frame(&mut editor);
        thread::sleep(Duration::from_millis(1));
        assert!(
            start.elapsed() < Duration::from_secs(120),
            "conversation hung"
        );
    }
    let (mcp, r, png) = conversation.join().unwrap();

    assert_eq!(r["status"]["kind"], "editor");
    assert_eq!(r["status"]["mode"], "edit");
    assert_eq!(r["status"]["scene"], SCENE);
    assert_eq!(r["query"]["world"], "edit");
    assert_eq!(r["query"]["entities"][0]["name"], "Cube");
    assert_eq!(r["apply"]["created"].as_array().unwrap().len(), 1);
    assert_eq!(r["apply"]["undoable"], true);
    match png {
        Some(png) => {
            assert_eq!(r["shot"]["width"], 480);
            assert_eq!(r["shot"]["height"], 270);
            let img = image::load_from_memory(&png).unwrap();
            assert_eq!((img.width(), img.height()), (480, 270));
            let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/captures/mcp_editor_screenshot.png");
            std::fs::create_dir_all(out.parent().unwrap()).unwrap();
            std::fs::write(&out, &png).unwrap();
        }
        None => assert_eq!(r["shot"]["code"], "unsupported"),
    }
    assert_eq!(r["play"]["mode"], "play");
    assert_eq!(r["play_query"]["world"], "play");
    assert_eq!(r["play_query"]["total"], 1);
    assert_eq!(r["stop"]["mode"], "edit");
    assert_eq!(r["after"]["world"], "edit");
    assert_eq!(r["after"]["total"], 1);

    // The agent's edit is one undo step in the editor; undoing it restores
    // the scene as loaded, so Play and Stop left it unchanged.
    assert!(editor.state.is_dirty());
    assert_eq!(editor.state.edits.undo_label(), Some("Agent edit"));
    editor.state.undo();
    assert_eq!(
        box_engine::box_core::scene::save(&editor.state.world).unwrap(),
        before
    );
    editor.on_quit();
    assert!(!discovery_path(&project).exists());
    mcp.finish();
}
