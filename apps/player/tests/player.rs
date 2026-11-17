//! `box-player` on a fresh project, headless, also as a live control host
//! (`--control`). GPU tests are skipped with `BOX_SKIP_GPU_TESTS`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use box_engine::Project;
use box_engine::box_core::identity::Uid;
use box_engine::control::{ControlClient, HostKind, base64_decode, discovery_path};
use serde_json::json;

fn player() -> Command {
    Command::new(env!("CARGO_BIN_EXE_box-player"))
}

fn temp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("box_player_app_{name}_{}", Uid::new()))
}

#[test]
fn runs_a_project_headless_for_n_frames() {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping");
        return;
    }
    let root = temp("headless");
    Project::create(&root, "Headless").unwrap();
    let report = root.join("report.json");
    let capture = root.join("shot.png");
    let out = player()
        .arg(&root)
        .args(["--headless", "--frames", "10", "--size", "320x180"])
        .arg("--report")
        .arg(&report)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Quiet logs without colour codes when stderr is not a terminal.
    let log = String::from_utf8_lossy(&out.stderr);
    assert!(!log.contains('\u{1b}'), "no ANSI escapes when piped: {log}");
    assert!(!log.contains("Features"), "no feature dump: {log}");
    assert!(out.stdout.is_empty(), "logs go to stderr");
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report).unwrap()).unwrap();
    assert_eq!(json["frames"], 10);
    assert_eq!(json["title"], "Headless");
    // Ground and cube.
    assert!(json["max_draws"].as_u64().unwrap() >= 2, "{json}");
    assert!(!capture.exists());

    // From inside the project, with a scene name and a capture.
    let out = player()
        .current_dir(root.join("assets"))
        .args(["--scene", "main", "--frames", "2", "--size", "320x180"])
        .arg("--capture")
        .arg(&capture)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(capture.is_file());
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn outside_a_project_is_a_clear_error() {
    let dir = temp("none");
    std::fs::create_dir_all(&dir).unwrap();
    let out = player()
        .current_dir(&dir)
        .args(["--headless"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("not inside a box project"), "{err}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn help_and_flag_errors() {
    let out = player().arg("--help").output().unwrap();
    assert!(out.status.success());
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(
        help.starts_with("Usage: box-player [project_dir | game.pak] [options]"),
        "{help}"
    );
    for flag in [
        "--scene",
        "--frames",
        "--capture",
        "--control",
        "--force-engine-only",
    ] {
        assert!(help.contains(flag), "{flag}: {help}");
    }

    // The version `box --version` and `box-editor --version` print too.
    for flag in ["-V", "--version"] {
        let out = player().arg(flag).output().unwrap();
        assert!(out.status.success());
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            format!("box-player {}", env!("CARGO_PKG_VERSION"))
        );
    }

    let out = player().args(["--framez", "3"]).output().unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("unknown flag `--framez`; did you mean `--frames`?"),
        "{err}"
    );

    let root = temp("scenes");
    Project::create(&root, "Scenes").unwrap();
    let out = player()
        .arg(&root)
        .args(["--scene", "nope", "--headless", "--frames", "1"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("scene `nope` not found")
            && err.contains("available scenes: main (assets/scenes/main.scene.json)"),
        "{err}"
    );
    std::fs::remove_dir_all(&root).unwrap();
}

/// A native project's game code is not in box-player: it refuses with the
/// command that runs the project's own player, unless forced.
#[test]
fn refuses_native_projects() {
    let root = temp("native");
    Project::create(&root, "Native").unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"native\"\n\n[[bin]]\nname = \"native-player\"\npath = \"src/bin/p.rs\"\n\n[[bin]]\nname = \"editor\"\npath = \"src/bin/e.rs\"\n",
    )
    .unwrap();
    let out = player()
        .arg(&root)
        .args(["--headless", "--frames", "1"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("`Native` is a native project"), "{err}");
    assert!(
        err.contains("&& cargo run --bin native-player -- . --headless --frames 1"),
        "the player binary is read from Cargo.toml: {err}"
    );
    assert!(err.contains("--force-engine-only"), "{err}");

    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_none() {
        let out = player()
            .arg(&root)
            .args(["--headless", "--frames", "1", "--force-engine-only"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    std::fs::remove_dir_all(&root).unwrap();
}

/// Kills the player if the test fails before it exits on its own.
struct ChildGuard(Option<std::process::Child>);

impl ChildGuard {
    fn child(&mut self) -> &mut std::process::Child {
        self.0.as_mut().expect("child already exited")
    }

    /// Wait for the child to exit (it is then no longer killed on drop).
    fn wait_exit(&mut self, timeout: Duration) -> std::process::ExitStatus {
        let start = Instant::now();
        loop {
            if let Some(status) = self.child().try_wait().unwrap() {
                self.0 = None;
                return status;
            }
            assert!(start.elapsed() < timeout, "the player did not exit");
            std::thread::sleep(Duration::from_millis(20));
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

/// Wait for the discovery file of a starting player.
fn wait_for_discovery(guard: &mut ChildGuard, root: &Path, stderr: &Path) {
    let start = Instant::now();
    while !discovery_path(root).is_file() {
        if let Some(status) = guard.child().try_wait().unwrap() {
            panic!(
                "the player exited with {status} before opening its control socket:\n{}",
                std::fs::read_to_string(stderr).unwrap_or_default()
            );
        }
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "no discovery file"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// `box-player --headless --control` on a fresh project, driven over the
/// control socket: status, world_query, apply_commands, screenshot, pause,
/// step_frames, play, frame_stats, read_log and stop.
#[test]
fn headless_player_serves_the_control_socket() {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping");
        return;
    }
    let root = temp("control");
    Project::create(&root, "Live").unwrap();
    let stderr = root.join("stderr.log");
    let mut guard = ChildGuard(Some(
        player()
            .arg(&root)
            .args(["--headless", "--control", "--size", "320x180"])
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&stderr).unwrap())
            .spawn()
            .unwrap(),
    ));
    wait_for_discovery(&mut guard, &root, &stderr);
    let mut c = ControlClient::connect_project(&root, Duration::from_secs(30)).unwrap();
    assert_eq!(c.discovery().kind, HostKind::Player);
    assert_eq!(c.discovery().pid, guard.child().id());

    // The first request, right after the discovery file appears, already
    // sees the scene: the bottom half of the view is the green ground and
    // the orange cube, not sky.
    let first = c.call("screenshot", json!({})).unwrap();
    let png = base64_decode(first["png_base64"].as_str().unwrap()).unwrap();
    let img = image::load_from_memory(&png).unwrap().to_rgb8();
    let (w, h) = img.dimensions();
    let not_sky = (0..w)
        .flat_map(|x| (h / 2..h).map(move |y| (x, y)))
        .filter(|&(x, y)| {
            let [r, g, b] = img.get_pixel(x, y).0;
            g > b || r > b
        })
        .count();
    assert!(
        not_sky > (w * h / 2 * 9 / 10) as usize,
        "first screenshot: {not_sky} of {} bottom pixels are scene",
        w * h / 2
    );

    let status = c.call("status", json!({})).unwrap();
    assert_eq!(status["kind"], "player");
    assert_eq!(status["mode"], "play");
    assert_eq!(status["scene"], "assets/scenes/main.scene.json");

    let cube = c.call("world_query", json!({ "name": "cube" })).unwrap();
    assert_eq!(cube["total"], 1, "{cube:#}");
    assert_eq!(cube["entities"][0]["name"], "Cube");

    let applied = c
        .call(
            "apply_commands",
            json!({ "commands": [{ "op": "spawn", "name": "Lamp", "components": {
                "Primitive": { "shape": { "Sphere": { "radius": 0.4, "sectors": 24, "stacks": 12 } } },
                "Transform": { "translation": [1.5, 1.2, 0.0] } } }] }),
        )
        .unwrap();
    assert_eq!(applied["saved"], false, "{applied:#}");
    let uid = applied["created"][0].as_str().unwrap().to_string();
    let lamp = c.call("world_query", json!({ "uid": uid })).unwrap();
    assert_eq!(lamp["total"], 1);
    assert_eq!(lamp["entities"][0]["name"], "Lamp");
    // Live only: the scene file is untouched.
    let scene = std::fs::read_to_string(root.join("assets/scenes/main.scene.json")).unwrap();
    assert!(!scene.contains("Lamp"));

    // The window (here `--size`) size by default, or the requested size.
    let shot = c.call("screenshot", json!({})).unwrap();
    assert_eq!(
        (shot["width"].as_u64(), shot["height"].as_u64()),
        (Some(320), Some(180))
    );
    let shot = c
        .call("screenshot", json!({ "width": 640, "height": 360 }))
        .unwrap();
    assert_eq!(
        (shot["width"].as_u64(), shot["height"].as_u64()),
        (Some(640), Some(360))
    );
    let png = base64_decode(shot["png_base64"].as_str().unwrap()).unwrap();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/captures/player_control_screenshot.png");
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    std::fs::write(&out, &png).unwrap();

    // Pause freezes the frame count; step_frames advances it exactly.
    let paused = c.call("pause", json!({})).unwrap();
    assert_eq!(paused["mode"], "paused");
    let frame = paused["frame"].as_u64().unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(c.call("status", json!({})).unwrap()["frame"], frame);
    let step = c.call("step_frames", json!({ "frames": 3 })).unwrap();
    assert_eq!(step, json!({ "frames": 3, "frame": frame + 3 }));
    assert_eq!(c.call("status", json!({})).unwrap()["frame"], frame + 3);
    assert_eq!(c.call("play", json!({})).unwrap()["mode"], "play");
    let start = Instant::now();
    while c.call("status", json!({})).unwrap()["frame"]
        .as_u64()
        .unwrap()
        <= frame + 3
    {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "play did not resume"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    let stats = c.call("frame_stats", json!({})).unwrap();
    assert!(
        stats["renderer"]["draws"].as_u64().unwrap() >= 2,
        "{stats:#}"
    );
    assert_eq!(stats["renderer"]["validation_errors"], 0);
    assert_eq!(stats["size"], json!([320, 180]));

    let log = c.call("read_log", json!({ "since": 0 })).unwrap();
    let messages: Vec<&str> = log["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["message"].as_str().unwrap())
        .collect();
    assert!(
        messages
            .iter()
            .any(|m| m.contains("control socket listening")),
        "{messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|m| m.starts_with("applied 1 command(s)")),
        "{messages:?}"
    );
    let next = log["next"].as_u64().unwrap();
    let newer = c.call("read_log", json!({ "since": next })).unwrap();
    assert!(
        newer["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["seq"].as_u64().unwrap() >= next)
    );

    // stop ends the process cleanly and removes the discovery file.
    c.call("stop", json!({})).unwrap();
    let status = guard.wait_exit(Duration::from_secs(30));
    assert!(
        status.success(),
        "exit {status}:\n{}",
        std::fs::read_to_string(&stderr).unwrap_or_default()
    );
    assert!(!discovery_path(&root).exists());
    std::fs::remove_dir_all(&root).unwrap();
}
