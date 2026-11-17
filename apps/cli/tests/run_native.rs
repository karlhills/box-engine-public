//! `box run` and `box new --native`. The run test is skipped when
//! `BOX_SKIP_GPU_TESTS` is set or no GPU adapter exists.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value as Json;

fn box_json(cwd: &Path, args: &[&str]) -> (bool, Json) {
    let out = Command::new(env!("CARGO_BIN_EXE_box"))
        .current_dir(cwd)
        .args(args)
        .arg("--json")
        .output()
        .expect("run box");
    let json = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "box {args:?}: stdout is not JSON ({e}):\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (out.status.success(), json)
}

/// `box new <dir> --native --json`'s `next` list (the project is created).
fn json_next(base: &Path, dir: &str) -> Vec<String> {
    let (ok, json) = box_json(base, &["new", dir, "--native"]);
    assert!(ok, "{json:#}");
    json["next"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect()
}

/// `canonicalize` without Windows' verbatim prefix (`\\?\C:\...`), as box
/// writes and the player reports paths.
fn canonical(path: &Path) -> PathBuf {
    let path = std::fs::canonicalize(path).unwrap();
    match path.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => path,
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cli_run_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `box` for the stand-in projects below, building in their own target
/// folder: with a shared CARGO_TARGET_DIR their `player` would replace the
/// lean test's real one.
fn stand_in_box(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_box"));
    command.env("CARGO_TARGET_DIR", root.join("target"));
    command
}

#[test]
fn run_frames_headless_prints_a_summary() {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping box run");
        return;
    }
    let base = temp_dir("frames");
    let (ok, _) = box_json(&base, &["new", "game"]);
    assert!(ok);
    let (ok, json) = box_json(
        &base,
        &[
            "run", "game", "--frames", "10", "--size", "320x180", "--report", "r.json",
        ],
    );
    if json["error"]["code"] == "no_gpu" {
        eprintln!("no GPU adapter; skipping box run");
        return;
    }
    assert!(ok, "{json:#}");
    assert_eq!(json["frames"], 10);
    assert_eq!(json["headless"], true);
    assert_eq!(json["validation_errors"], 0);
    assert_eq!(json["scene"], "assets/scenes/main.scene.json");
    for t in ["frame_ms", "update_ms", "render_cpu_ms"] {
        assert!(json["timings"][t]["p50"].is_number(), "{t}: {json:#}");
    }
    assert!(json["adapter"].is_string());
    assert_eq!(json["captures"], serde_json::json!([]));
    assert!(base.join("r.json").is_file());

    // A scene file as the target, with a capture.
    let (ok, json) = box_json(
        &base.join("game"),
        &[
            "run",
            "assets/scenes/main.scene.json",
            "--capture",
            "shot.png",
            "--size",
            "320x180",
        ],
    );
    assert!(ok, "{json:#}");
    assert_eq!(json["frames"], 3);
    assert!(base.join("game/shot.png").is_file());
}

#[test]
fn run_errors_are_json() {
    let base = temp_dir("errors");
    let (ok, json) = box_json(&base, &["run", "--frames", "1"]);
    assert!(!ok);
    assert_eq!(json["error"]["code"], "not_a_project");
    let (ok, json) = box_json(&base, &["run", "missing", "--frames", "1"]);
    assert!(!ok);
    assert_eq!(json["error"]["code"], "not_found");
    let (ok, json) = box_json(&base, &["run", "--frames", "0"]);
    assert!(!ok);
    assert_eq!(json["error"]["code"], "usage");
}

#[test]
fn new_native_creates_a_standalone_crate() {
    let base = temp_dir("native");
    let (ok, json) = box_json(
        &base,
        &["new", "space-race", "--native", "--name", "Space Race"],
    );
    assert!(ok, "{json:#}");
    let root = base.join("space-race");
    assert_eq!(json["native"]["crate"], "space-race");
    let files: Vec<&str> = json["native"]["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f.as_str().unwrap())
        .collect();
    for f in [
        "Cargo.toml",
        "src/lib.rs",
        "src/bin/editor.rs",
        "src/bin/player.rs",
    ] {
        assert!(files.contains(&f), "{files:?}");
        assert!(root.join(f).is_file(), "{f}");
    }
    assert!(root.join("box.project.json").is_file());
    assert!(root.join("assets/scenes/main.scene.json").is_file());

    let engine = canonical(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .to_string_lossy()
        .replace('\\', "/");
    let cargo = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(cargo.contains("name = \"space-race\""), "{cargo}");
    assert!(cargo.contains("\n[workspace]\n"), "{cargo}");
    assert!(
        cargo.contains(&format!(
            "box_engine = {{ path = \"{engine}/crates/box_engine\", default-features = false }}"
        )),
        "{cargo}"
    );
    assert!(cargo.contains(&format!(
        "box_editor = {{ path = \"{engine}/crates/box_editor\", optional = true }}"
    )));
    assert!(
        cargo.contains("required-features = [\"editor\"]"),
        "{cargo}"
    );
    assert!(
        cargo.contains("\ndefault = [\"physics\", \"dynamics\", \"audio\", \"script\"]\n"),
        "every engine module by default: {cargo}"
    );
    assert!(!cargo.contains("bevy_reflect ="), "{cargo}");

    let lib = std::fs::read_to_string(root.join("src/lib.rs")).unwrap();
    assert!(lib.contains("impl Plugin for SpaceRacePlugin"));
    assert!(lib.contains("register_all::<(Spin,)>()"));
    assert!(lib.contains("add_system(Stage::Update, spin)"));
    assert!(lib.contains("Game::new().with_plugin(SpaceRacePlugin)"));
    let editor = std::fs::read_to_string(root.join("src/bin/editor.rs")).unwrap();
    assert!(
        editor.contains("box_editor::run_game(options, space_race::game())"),
        "{editor}"
    );
    assert!(editor.contains("env!(\"CARGO_MANIFEST_DIR\")"), "{editor}");
    let player = std::fs::read_to_string(root.join("src/bin/player.rs")).unwrap();
    assert!(player.contains("box_engine::player::run_game(options, space_race::game())"));
    assert!(root.join(".cargo/config.toml").is_file());
    let gitignore = std::fs::read_to_string(root.join(".gitignore")).unwrap();
    assert_eq!(gitignore, ".box/\ntarget/\n");

    // A second run refuses before writing anything.
    let (ok, json) = box_json(&base, &["new", "space-race", "--native"]);
    assert!(!ok);
    assert_eq!(json["error"]["code"], "already_exists");

    // `box run --no-cargo` prints the command that runs its own player.
    let (ok, json) = box_json(&root, &["run", "--frames", "5", "--no-cargo"]);
    assert!(ok, "{json:#}");
    assert_eq!(json["native"], true);
    assert_eq!(
        json["command"],
        serde_json::json!([
            "cargo",
            "run",
            "--bin",
            "player",
            "--",
            ".",
            "--frames",
            "5",
            "--headless"
        ])
    );
    let line = json["command_line"].as_str().unwrap();
    assert!(line.starts_with("cd "), "{line}");
    assert!(
        line.ends_with("&& cargo run --bin player -- . --frames 5 --headless"),
        "{line}"
    );

    // The next steps name the crate's binaries.
    let next = json_next(&base, "space-race-2");
    assert!(
        next[0].ends_with("&& cargo run --features editor --bin editor -- ."),
        "{next:?}"
    );

    // `--native` on an existing data project adds only the crate.
    let (ok, json) = box_json(&base, &["new", "data-game"]);
    assert!(ok);
    assert!(json["native"].is_null());
    let (ok, json) = box_json(&base, &["run", "data-game", "--no-cargo"]);
    assert!(!ok);
    assert_eq!(
        json["error"]["code"], "usage",
        "--no-cargo is for native projects"
    );
    let scene = std::fs::read(base.join("data-game/assets/scenes/main.scene.json")).unwrap();
    let (ok, json) = box_json(&base, &["new", "data-game", "--native"]);
    assert!(ok, "{json:#}");
    assert_eq!(json["created"], false);
    assert_eq!(json["native"]["crate"], "data-game");
    assert!(base.join("data-game/src/bin/player.rs").is_file());
    assert_eq!(
        std::fs::read(base.join("data-game/assets/scenes/main.scene.json")).unwrap(),
        scene,
        "the project's files are untouched"
    );
    let (ok, json) = box_json(&base, &["new", "data-game", "--native"]);
    assert!(!ok);
    assert_eq!(json["error"]["code"], "already_exists");

    // A bad engine path is refused up front.
    let (ok, json) = box_json(
        &base,
        &["new", "other", "--native", "--engine-path", "nope"],
    );
    assert!(!ok);
    assert_eq!(json["error"]["code"], "not_found");
    assert!(!base.join("other").exists());
    let (ok, json) = box_json(&base, &["new", "third", "--engine-path", "x"]);
    assert!(!ok);
    assert_eq!(json["error"]["code"], "usage");
}

/// `box run` on a native project runs its player binary through cargo in
/// the project directory: output streams (to stderr with `--json`, so
/// stdout stays one JSON result) and the player's exit code is box's. The
/// crate here is a stand-in without the engine, so it builds in a second.
#[test]
fn run_native_runs_the_projects_player_through_cargo() {
    let base = temp_dir("native_spawn");
    let (ok, _) = box_json(&base, &["new", "fake"]);
    assert!(ok);
    let root = base.join("fake");
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"fake\"\nversion = \"0.1.0\"\nedition = \"2024\"\npublish = false\n\n[[bin]]\nname = \"player\"\npath = \"src/bin/player.rs\"\n\n[workspace]\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("src/bin")).unwrap();
    std::fs::write(
        root.join("src/bin/player.rs"),
        "fn main() {\n    let args: Vec<String> = std::env::args().skip(1).collect();\n    println!(\"PLAYER ARGS: {}\", args.join(\" \"));\n    let cwd = std::env::current_dir().unwrap();\n    println!(\"PLAYER CWD: {}\", cwd.display());\n    if args.iter().any(|a| a == \"7\") {\n        std::process::exit(3);\n    }\n}\n",
    )
    .unwrap();

    let out = stand_in_box(&root)
        .current_dir(&base)
        .args(["run", "fake", "--frames", "2", "--json"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    let json: Json = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("stdout is not JSON ({e}); stderr:\n{stderr}"));
    assert!(out.status.success(), "{json:#}\n{stderr}");
    assert_eq!(json["native"], true);
    assert_eq!(json["exit_code"], 0);
    assert!(
        stderr.contains("PLAYER ARGS: . --frames 2 --headless"),
        "{stderr}"
    );
    let cwd = canonical(&root);
    assert!(
        stderr.contains(&format!("PLAYER CWD: {}", cwd.display())),
        "{stderr}"
    );

    // Text mode streams the player's output to stdout.
    let out = stand_in_box(&root)
        .current_dir(&root)
        .args(["run", "--frames", "2"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(
        stdout.contains("PLAYER ARGS: . --frames 2 --headless"),
        "{stdout}"
    );
    assert!(
        stdout.contains("Ran `fake` with its own player"),
        "{stdout}"
    );

    // A failing player's exit code is box's exit code.
    let out = stand_in_box(&root)
        .current_dir(&root)
        .args(["run", "--frames", "7", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    let json: Json = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["error"]["code"], "run_failed");
    assert_eq!(json["error"]["exit_code"], 3);
}

/// A native project whose code does not compile: `box run` reports
/// `build_failed` with the compiler errors as `compile_error` problems (file,
/// line, column), not as a failed run, and prints cargo's own rendering.
#[test]
fn run_native_reports_compile_errors() {
    let base = temp_dir("native_compile_error");
    let (ok, _) = box_json(&base, &["new", "broken"]);
    assert!(ok);
    let root = base.join("broken");
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"broken\"\nversion = \"0.1.0\"\nedition = \"2024\"\npublish = false\n\n[[bin]]\nname = \"player\"\npath = \"src/bin/player.rs\"\n\n[workspace]\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("src/bin")).unwrap();
    std::fs::write(
        root.join("src/bin/player.rs"),
        "fn main() {\n    let speed: u32 = \"fast\";\n    println!(\"{speed}\");\n}\n",
    )
    .unwrap();
    let out = stand_in_box(&root)
        .current_dir(&base)
        .args(["run", "broken", "--frames", "2", "--json"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    let json: Json = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("stdout is not JSON ({e}); stderr:\n{stderr}"));
    assert!(!out.status.success());
    let error = &json["error"];
    assert_eq!(error["code"], "build_failed", "{json:#}");
    let message = error["message"].as_str().unwrap();
    assert!(
        message.starts_with(
            "the game's code does not compile (1 error; first: src/bin/player.rs:2:22: mismatched types)"
        ),
        "{message}"
    );
    assert_eq!(error["file"], "src/bin/player.rs");
    let p = &error["problems"][0];
    assert_eq!(p["code"], "compile_error");
    assert_eq!(p["line"], 2);
    assert_eq!(p["column"], 22);
    // cargo's rendering, without colours when piped.
    assert!(
        stderr.contains("error[E0308]: mismatched types"),
        "{stderr}"
    );
    assert!(!stderr.contains('\u{1b}'), "{stderr}");
}

/// `box new --native`, then the documented opt-out (`default = ["physics",
/// "audio"]` in the generated Cargo.toml): the project builds without the
/// `dynamics` and `script` engine modules, its registry lacks their types,
/// and (GPU) its player renders a start scene that uses `RigidBody` and
/// `Script` with one warning per type instead of failing. Builds in the
/// engine's target directory, offline; skipped like
/// `generated_project_builds_with_only_engine_dependencies` (no cached
/// dependencies, or `BOX_SKIP_NATIVE_CHECK`).
#[test]
fn lean_native_project_builds_and_runs() {
    if std::env::var_os("BOX_SKIP_NATIVE_CHECK").is_some() {
        eprintln!("BOX_SKIP_NATIVE_CHECK set; skipping");
        return;
    }
    let base = temp_dir("lean");
    let (ok, json) = box_json(&base, &["new", "lean", "--native"]);
    assert!(ok, "{json:#}");
    let root = base.join("lean");
    let manifest = root.join("Cargo.toml");
    let cargo = std::fs::read_to_string(&manifest).unwrap();
    let full = "\ndefault = [\"physics\", \"dynamics\", \"audio\", \"script\"]\n";
    assert!(cargo.contains(full), "{cargo}");
    std::fs::write(
        &manifest,
        cargo.replace(full, "\ndefault = [\"physics\", \"audio\"]\n"),
    )
    .unwrap();

    // The starter cube gets a rigid body and a script.
    let scene_path = root.join("assets/scenes/main.scene.json");
    let mut scene: Json = serde_json::from_slice(&std::fs::read(&scene_path).unwrap()).unwrap();
    let cube = scene["entities"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|e| e["name"] == "Cube")
        .unwrap();
    let components = cube["components"].as_object_mut().unwrap();
    components.insert(
        "RigidBody".into(),
        serde_json::json!({ "kind": "Dynamic", "mass": 1.0 }),
    );
    components.insert(
        "Script".into(),
        serde_json::json!({ "source": "scripts/spin.luau", "properties": [] }),
    );
    std::fs::write(&scene_path, serde_json::to_vec_pretty(&scene).unwrap()).unwrap();

    let engine =
        std::fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")).unwrap();
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| engine.join("target"));
    let out = Command::new(env!("CARGO"))
        .current_dir(&root)
        .args(["build", "--offline", "--quiet", "--bin", "player"])
        .env("CARGO_TARGET_DIR", &target)
        .output()
        .expect("run cargo");
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success()
        && [
            "offline",
            "failed to download",
            "no matching package",
            "network",
        ]
        .iter()
        .any(|s| stderr.contains(s))
    {
        eprintln!("dependencies unavailable offline; skipping:\n{stderr}");
        return;
    }
    assert!(
        out.status.success(),
        "the lean project does not build:\n{stderr}"
    );

    // Neither rapier nor Luau is in its dependency graph.
    let out = Command::new(env!("CARGO"))
        .current_dir(&root)
        .args(["tree", "--offline", "-e", "normal", "--prefix", "none"])
        .output()
        .expect("run cargo tree");
    let tree = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    for name in ["rapier3d ", "mlua "] {
        assert!(
            !tree.lines().any(|l| l.starts_with(name)),
            "{name}in:\n{tree}"
        );
    }
    assert!(tree.lines().any(|l| l.starts_with("parry3d ")));

    let player = target
        .join("debug")
        .join(format!("player{}", std::env::consts::EXE_SUFFIX));
    let run = |args: &[&str]| {
        Command::new(&player)
            .arg(&root)
            .args(args)
            .env("RUST_LOG", "warn")
            .output()
            .expect("run the player")
    };

    // The registry the game writes for tools: no dynamics or script types.
    let out = run(&["--write-registry"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let registry: Json =
        serde_json::from_slice(&std::fs::read(root.join(".box/registry.json")).unwrap()).unwrap();
    let names: Vec<&str> = ["components", "resources"]
        .iter()
        .flat_map(|k| registry[k].as_array().unwrap())
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for name in ["Spin", "Collider", "AudioSource", "AudioSettings"] {
        assert!(names.contains(&name), "no {name}: {names:?}");
    }
    for name in ["RigidBody", "Velocity", "PhysicsSettings", "Script"] {
        assert!(!names.contains(&name), "{name} registered: {names:?}");
    }

    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        return;
    }
    let shot = base.join("lean.png");
    let out = run(&[
        "--capture",
        shot.to_str().unwrap(),
        "--frames",
        "10",
        "--size",
        "320x180",
    ]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    if stderr.contains("no suitable GPU adapter") {
        eprintln!("no GPU adapter; skipping the lean run");
        return;
    }
    // A capture run exits non-zero on wgpu validation errors.
    assert!(out.status.success(), "{stderr}");
    assert!(shot.is_file());
    for (name, feature) in [("RigidBody", "dynamics"), ("Script", "script")] {
        assert_eq!(
            stderr
                .lines()
                .filter(|l| l.contains(&format!("component `{name}`"))
                    && l.contains(&format!("box_engine's `{feature}` feature")))
                .count(),
            1,
            "{stderr}"
        );
    }
}
