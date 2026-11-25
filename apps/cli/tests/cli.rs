//! End-to-end tests of the `box` binary: new -> apply -> show -> check -> render.
//!
//! The render test is skipped when `BOX_SKIP_GPU_TESTS` is set or no GPU
//! adapter exists.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{Value as Json, json};

fn box_cmd(cwd: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_box"));
    c.current_dir(cwd).args(args);
    c
}

fn run(cwd: &Path, args: &[&str]) -> (Output, Json) {
    let out = box_cmd(cwd, args).output().expect("run box");
    let json = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "box {args:?}: stdout is not JSON ({e}):\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (out, json)
}

/// Run with `--json`, expect success, return the JSON result.
fn ok(cwd: &Path, args: &[&str]) -> Json {
    let mut all = args.to_vec();
    all.push("--json");
    let (out, json) = run(cwd, &all);
    assert!(out.status.success(), "box {args:?} failed: {json:#}");
    json
}

/// Run with `--json`, expect failure, return the `error` object.
fn fails(cwd: &Path, args: &[&str]) -> Json {
    let mut all = args.to_vec();
    all.push("--json");
    let (out, json) = run(cwd, &all);
    assert!(!out.status.success(), "box {args:?} should fail: {json:#}");
    json["error"].clone()
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cli_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn new_project(name: &str) -> PathBuf {
    let base = temp_dir(name);
    let created = ok(&base, &["new", "game", "--name", "Test Game"]);
    assert_eq!(created["name"], "Test Game");
    assert_eq!(created["start_scene"], "assets/scenes/main.scene.json");
    base.join("game")
}

const SCENE: &str = "assets/scenes/main.scene.json";

#[test]
fn playtest_log_checks_ignore_terminal_log_filter() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = box_cmd(
        &root,
        &["playtest", "examples/collector_luau", "route", "--json"],
    )
    .env("RUST_LOG", "warn")
    .output()
    .expect("run playtest");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}");
    let report: Json = serde_json::from_str(&stdout).expect("playtest report");
    assert!(
        report["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| { check["check"] == "log_contains" && check["passed"] == true })
    );
    assert!(!stderr.contains("score 20"), "{stderr}");
}

fn write(dir: &Path, name: &str, value: &Json) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, serde_json::to_string_pretty(value).unwrap()).unwrap();
    p
}

fn entity<'a>(show: &'a Json, name: &str) -> &'a Json {
    show["entities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == name)
        .unwrap_or_else(|| panic!("no entity `{name}` in {show:#}"))
}

fn gpu_tests_enabled() -> bool {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping render");
        return false;
    }
    true
}

/// Render through the CLI. `None` when the machine has no GPU adapter.
fn render(cwd: &Path, args: &[&str]) -> Option<Json> {
    let mut all = vec!["render"];
    all.extend_from_slice(args);
    all.push("--json");
    let (out, json) = run(cwd, &all);
    if json["error"]["code"] == "no_gpu" {
        eprintln!("no GPU adapter; skipping render");
        return None;
    }
    assert!(out.status.success(), "render failed: {json:#}");
    assert_eq!(json["validation_errors"], 0);
    Some(json)
}

fn color(r: f32, g: f32, b: f32) -> Json {
    json!([r, g, b])
}

/// M2 acceptance: an agent using only the CLI creates a project, spawns
/// entities with Primitive + StandardMaterial, moves them and renders a PNG.
#[test]
fn agent_workflow_new_apply_show_check_render() {
    let project = new_project("workflow");

    // Spawn two shapes with materials, all in one transaction.
    let spawn = json!([
        { "op": "spawn", "name": "Tower", "components": {
            "Primitive": { "shape": { "Cylinder": { "radius": 0.6, "height": 3.0, "segments": 32 } }, "cast_shadows": true },
            "StandardMaterial": { "base_color": color(0.25, 0.4, 0.75), "roughness": 0.7, "metallic": 0.0, "emissive": color(0.0, 0.0, 0.0) }
        } },
        { "op": "spawn", "name": "Ball", "components": {
            "Transform": { "translation": [0.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [1.0, 1.0, 1.0] },
            "Primitive": { "shape": { "Sphere": { "radius": 0.5, "sectors": 32, "stacks": 16 } }, "cast_shadows": true },
            "StandardMaterial": { "base_color": color(0.9, 0.7, 0.2), "roughness": 0.3, "metallic": 1.0, "emissive": color(0.0, 0.0, 0.0) }
        } }
    ]);
    let commands = write(&project, "spawn.json", &spawn);
    let applied = ok(
        &project,
        &["scene", "apply", SCENE, commands.to_str().unwrap()],
    );
    assert_eq!(applied["commands"], 2);
    let created = applied["created"].as_array().unwrap().clone();
    assert_eq!(created.len(), 2);
    let tower_uid = created[0].as_str().unwrap().to_string();

    // Move them (by uid and by name, from stdin).
    let moves = json!([
        { "op": "set", "entity": tower_uid, "path": "Transform.translation", "value": [-1.8, 1.5, -0.8] },
        { "op": "set", "entity": "Ball", "path": "Transform.translation", "value": [1.6, 0.5, 0.6] },
        { "op": "set", "entity": "Cube", "path": "Transform.rotation", "value": [0.0, 0.3826834, 0.0, 0.9238795] },
        { "op": "set", "entity": "Cube", "path": "StandardMaterial.roughness", "value": 0.35 }
    ]);
    let mut child = box_cmd(&project, &["scene", "apply", SCENE, "-", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(moves.to_string().as_bytes()).unwrap();
    }
    let out = child.wait_with_output().unwrap();
    let moved: Json = serde_json::from_slice(&out.stdout).unwrap();
    assert!(out.status.success(), "{moved:#}");
    assert_eq!(moved["created"], json!([]));

    // Show reflects the edits.
    let show = ok(&project, &["scene", "show", SCENE]);
    assert_eq!(show["entities"].as_array().unwrap().len(), 6);
    assert_eq!(
        entity(&show, "Tower")["components"]["Transform"]["translation"],
        json!([-1.8, 1.5, -0.8])
    );
    assert_eq!(
        entity(&show, "Cube")["components"]["StandardMaterial"]["roughness"],
        json!(0.35)
    );
    assert!(show["resources"]["Environment"].is_object());
    let one = ok(&project, &["scene", "show", SCENE, "--entity", &tower_uid]);
    assert_eq!(one["entity"]["name"], "Tower");
    assert!(
        one["entity"]["components"]["Primitive"]["shape"]["Cylinder"].is_object(),
        "{one:#}"
    );

    // The project is valid.
    let check = ok(&project, &["check"]);
    assert_eq!(check["ok"], true, "{check:#}");
    assert_eq!(check["scenes"][0]["entities"], 6);
    let list = ok(&project, &["scene", "list"]);
    assert_eq!(list["scenes"][0]["path"], SCENE);

    // Render with the scene camera.
    if !gpu_tests_enabled() {
        return;
    }
    let out =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/cli_render.png");
    let Some(rendered) = render(
        &project,
        &[SCENE, "--out", out.to_str().unwrap(), "--size", "960x540"],
    ) else {
        return;
    };
    assert_eq!(rendered["camera"]["name"], "Camera");
    assert_eq!(rendered["objects"], 4);
    let img = image::open(&out).unwrap().to_rgba8();
    assert_eq!(img.dimensions(), (960, 540));
    // The gold ball and the blue tower are in view: count their tints
    // (the ground is green-grey, the cube terracotta, the sky blue-grey).
    let count = |f: &dyn Fn(f32, f32, f32) -> bool| {
        img.pixels()
            .filter(|p| f(f32::from(p[0]), f32::from(p[1]), f32::from(p[2])))
            .count()
    };
    let gold = count(&|r, g, b| r > 90.0 && g > 0.8 * r && g <= 1.05 * r && b < 0.75 * g);
    let blue = count(&|r, g, b| b > r + 40.0 && b > g + 20.0 && r < 140.0);
    assert!(gold > 1000, "gold ball not visible ({gold} pixels)");
    assert!(blue > 3000, "blue tower not visible ({blue} pixels)");
}

#[test]
fn render_frames_scene_without_camera_and_honours_camera_flag() {
    if !gpu_tests_enabled() {
        return;
    }
    let project = new_project("framing");
    let edits = json!([
        { "op": "despawn", "entity": "Camera" },
        { "op": "spawn", "name": "Top", "components": {
            "Transform": { "translation": [0.0, 12.0, 0.01], "rotation": [-0.70710677, 0.0, 0.0, 0.70710677], "scale": [1.0, 1.0, 1.0] },
            "Camera": { "fov_y": 0.9, "near": 0.1, "active": false }
        } }
    ]);
    let commands = write(&project, "edits.json", &edits);
    ok(
        &project,
        &["scene", "apply", SCENE, commands.to_str().unwrap()],
    );

    let captures = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let auto = captures.join("cli_auto.png");
    // The only camera is inactive: an automatic camera frames the scene.
    let Some(r) = render(
        &project,
        &[SCENE, "--out", auto.to_str().unwrap(), "--size", "320x180"],
    ) else {
        return;
    };
    assert_eq!(r["camera"]["automatic"], true);
    assert!(auto.is_file());

    let top = captures.join("cli_top.png");
    let r = render(
        &project,
        &[
            ".",
            "--out",
            top.to_str().unwrap(),
            "--size",
            "320x180",
            "--camera",
            "Top",
        ],
    )
    .unwrap();
    assert_eq!(r["camera"]["name"], "Top");

    let err = fails(
        &project,
        &[
            "render",
            SCENE,
            "--out",
            top.to_str().unwrap(),
            "--camera",
            "Cube",
        ],
    );
    assert_eq!(err["code"], "not_a_camera");
    assert!(err["entity"].is_string());
}

#[test]
fn failed_apply_is_reported_and_leaves_the_scene_untouched() {
    let project = new_project("apply_errors");
    let scene_path = project.join(SCENE);
    let before = std::fs::read(&scene_path).unwrap();

    // The second command fails: the first must not be written either.
    let bad = json!([
        { "op": "spawn", "name": "Lamp" },
        { "op": "set", "entity": "Nobody", "path": "Transform.translation.x", "value": 1.0 }
    ]);
    let commands = write(&project, "bad.json", &bad);
    let err = fails(
        &project,
        &["scene", "apply", SCENE, commands.to_str().unwrap()],
    );
    assert_eq!(err["code"], "entity_not_found", "{err:#}");
    assert_eq!(err["entity"], "Nobody");
    assert_eq!(err["command"], 1);
    assert_eq!(err["op"], "set");
    assert!(err["file"].as_str().unwrap().ends_with("main.scene.json"));
    assert_eq!(std::fs::read(&scene_path).unwrap(), before);

    let bad_path = json!({ "op": "set", "entity": "Cube", "path": "Transform.nope", "value": 1.0 });
    let commands = write(&project, "bad_path.json", &bad_path);
    let err = fails(
        &project,
        &["scene", "apply", SCENE, commands.to_str().unwrap()],
    );
    assert_eq!(err["code"], "bad_path", "{err:#}");

    let commands = write(&project, "garbage.json", &json!([{ "op": "explode" }]));
    let err = fails(
        &project,
        &["scene", "apply", SCENE, commands.to_str().unwrap()],
    );
    assert_eq!(err["code"], "command_parse");
    assert_eq!(err["command"], 0);

    // Dry run validates without writing.
    let good = json!({ "op": "rename", "entity": "Cube", "name": "Box" });
    let commands = write(&project, "good.json", &good);
    let r = ok(
        &project,
        &[
            "scene",
            "apply",
            SCENE,
            commands.to_str().unwrap(),
            "--dry-run",
        ],
    );
    assert_eq!(r["written"], false);
    assert_eq!(std::fs::read(&scene_path).unwrap(), before);

    // Paths relative to the project root work from a subdirectory.
    let sub = project.join("assets");
    ok(&sub, &["scene", "apply", SCENE, commands.to_str().unwrap()]);
    let show = ok(&sub, &["scene", "show", SCENE, "--entity", "Box"]);
    assert_eq!(show["entity"]["name"], "Box");
}

#[test]
fn script_docs_writes_the_reference() {
    let project = new_project("script_docs");
    let first = ok(&project, &["script", "docs"]);
    assert_eq!(first["undocumented"], 0, "{first:#}");
    assert!(first["items"].as_u64().unwrap() > 400, "{first:#}");
    let dir = project.join(".box/reference");
    let index = std::fs::read_to_string(dir.join("README.md")).unwrap();
    assert!(index.contains("[`world`](world.md#world-1)"), "{index}");
    let world = std::fs::read_to_string(dir.join("world.md")).unwrap();
    assert!(world.contains("### `world.spawn`"));
    // Unchanged pages are not rewritten; stale generated pages are removed,
    // other files are kept.
    std::fs::write(
        dir.join("old.md"),
        format!(
            "{}\n",
            "<!-- Generated by `box script docs` from the engine source. Do not edit. -->"
        ),
    )
    .unwrap();
    std::fs::write(dir.join("notes.md"), "mine\n").unwrap();
    let second = ok(&project, &["script", "docs"]);
    assert_eq!(second["written"], 0, "{second:#}");
    assert!(!dir.join("old.md").exists());
    assert!(dir.join("notes.md").exists());
    // Outside a project, --out is required.
    let outside = temp_dir("script_docs_outside");
    assert_eq!(fails(&outside, &["script", "docs"])["code"], "usage");
    let out = ok(&outside, &["script", "docs", "--out", "ref"]);
    assert_eq!(out["pages"], first["pages"]);
}

#[test]
fn script_types_writes_definitions_and_vscode_settings() {
    let project = new_project("script_types");
    let first = ok(&project, &["script", "types"]);
    assert_eq!(first["file"], ".box/types/box.d.luau");
    assert_eq!(first["vscode_settings"], "written");
    assert!(first["types"].as_u64().unwrap() > 20, "{first:#}");
    let text = std::fs::read_to_string(project.join(".box/types/box.d.luau")).unwrap();
    assert!(text.contains("declare extern type Entity with"));
    assert!(text.contains("function get(self, component: \"Transform\"): Transform?"));
    assert!(text.contains("declare scenes: {"));
    let settings: Json = serde_json::from_str(
        &std::fs::read_to_string(project.join(".vscode/settings.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        settings["luau-lsp.types.definitionFiles"],
        json!([".box/types/box.d.luau"])
    );
    // Stable: a second run writes the same text and keeps the settings.
    let second = ok(&project, &["script", "types"]);
    assert_eq!(second["vscode_settings"], "kept");
    assert_eq!(
        std::fs::read_to_string(project.join(".box/types/box.d.luau")).unwrap(),
        text
    );
}

#[test]
fn check_reports_broken_scenes_with_file_and_entity() {
    let project = new_project("check");
    let scene_path = project.join(SCENE);
    let mut tree: Json = serde_json::from_slice(&std::fs::read(&scene_path).unwrap()).unwrap();
    let cube = tree["entities"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|e| e["name"] == "Cube")
        .unwrap();
    let uid = cube["uid"].as_str().unwrap().to_string();
    cube["components"]["StandardMaterial"]["roughness"] = json!("very");
    cube["components"]["Glow"] = json!({ "power": 3 });
    write(&project, SCENE, &tree);
    std::fs::write(project.join("assets/scenes/broken.scene.json"), "{ nope").unwrap();

    let mut args = vec!["check", "--json"];
    let (out, report) = run(&project, &args);
    assert!(!out.status.success());
    assert_eq!(report["ok"], false);
    assert_eq!(report["errors"], 2, "{report:#}");
    let problems = report["problems"].as_array().unwrap();
    let broken = problems
        .iter()
        .find(|p| p["file"] == "assets/scenes/broken.scene.json")
        .unwrap();
    assert_eq!(broken["code"], "scene_parse");
    let main = problems
        .iter()
        .find(|p| p["file"] == SCENE && p["severity"] == "error")
        .unwrap();
    assert_eq!(main["entity"], uid.as_str());
    assert_eq!(main["component"], "StandardMaterial");

    // Fix the field: the unknown component is only a warning.
    cube_fix(&project, &uid);
    std::fs::remove_file(project.join("assets/scenes/broken.scene.json")).unwrap();
    args.pop();
    let report = ok(&project, &args);
    assert_eq!(report["ok"], true);
    assert_eq!(report["warnings"], 1);
    assert_eq!(report["problems"][0]["code"], "unknown_component");
    assert_eq!(report["problems"][0]["entity"], uid.as_str());
}

fn cube_fix(project: &Path, uid: &str) {
    let path = project.join(SCENE);
    let mut tree: Json = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for e in tree["entities"].as_array_mut().unwrap() {
        if e["uid"] == uid {
            e["components"]["StandardMaterial"]["roughness"] = json!(0.5);
        }
    }
    write(project, SCENE, &tree);
}

#[test]
fn project_errors_are_json() {
    let base = temp_dir("project_errors");
    let err = fails(&base, &["check"]);
    assert_eq!(err["code"], "not_a_project");

    let project = new_project("project_errors_newer");
    let err = fails(project.parent().unwrap(), &["new", "game"]);
    assert_eq!(err["code"], "already_exists");
    let file = project.join("box.project.json");
    let mut p: Json = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    p["engine_version"] = json!("999.0.0");
    write(&project, "box.project.json", &p);
    let err = fails(&project, &["scene", "list"]);
    assert_eq!(err["code"], "project_too_new");
    assert!(
        err["message"]
            .as_str()
            .unwrap()
            .contains("update box-engine")
    );
    assert!(err["file"].as_str().unwrap().ends_with("box.project.json"));

    let err = fails(&base, &["scene", "show", "missing.scene.json"]);
    assert_eq!(err["code"], "not_found");

    let (out, json) = run(&base, &["render", "--json"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(json["error"]["code"], "usage");
}

#[test]
fn registry_lists_engine_types() {
    let base = temp_dir("registry");
    let r = ok(&base, &["registry"]);
    let names = |list: &str| -> Vec<String> {
        r[list]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect()
    };
    let components = names("components");
    for c in [
        "AudioSource",
        "Camera",
        "DirectionalLight",
        "Name",
        "Primitive",
        "StandardMaterial",
        "Transform",
    ] {
        assert!(components.contains(&c.to_string()), "{c} in {components:?}");
    }
    assert!(!components.contains(&"MeshRenderer".to_string()));
    // The audio and physics settings resources come with the default
    // `audio` and `dynamics` features; `GameVars` holds interaction state,
    // `GameRules`, `GameState` and `HudSettings` the behaviours' rules,
    // `NavSettings` the navmesh's agent size (`navigation`).
    assert_eq!(
        names("resources"),
        vec![
            "AudioSettings",
            "Environment",
            "GameRules",
            "GameState",
            "GameVars",
            "HudSettings",
            "NavSettings",
            "PhysicsSettings"
        ]
    );
    for c in ["RigidBody", "Joint", "Velocity", "Collider"] {
        assert!(components.contains(&c.to_string()), "{c} in {components:?}");
    }
    let env = r["resources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "Environment")
        .unwrap();
    let fog = env["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "fog_density")
        .unwrap();
    assert_eq!(fog["unit"], "1/m");
    assert!(fog["doc"].is_string());
    assert_eq!(env["default"]["tonemapper"], "AgX");

    // Human-readable output works too.
    let out = box_cmd(&base, &["registry"]).output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("component StandardMaterial"));
}

fn set_start_scene(project: &Path, start: &str) {
    let file = project.join("box.project.json");
    let mut p: Json = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    p["start_scene"] = json!(start);
    write(project, "box.project.json", &p);
}

#[test]
fn start_scene_outside_the_project_is_refused() {
    let project = new_project("escape");
    let base = project.parent().unwrap();
    let outside = base.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let victim = outside.join("x.scene.json");
    std::fs::copy(project.join(SCENE), &victim).unwrap();
    let before = std::fs::read(&victim).unwrap();
    let commands = write(
        base,
        "spawn.json",
        &json!([{ "op": "spawn", "name": "Intruder" }]),
    );
    let commands = commands.to_str().unwrap();

    for start in ["../outside/x.scene.json", victim.to_str().unwrap()] {
        set_start_scene(&project, start);
        let err = fails(base, &["scene", "apply", "game", commands]);
        assert_eq!(err["code"], "path_outside_project", "{start}: {err:#}");
        assert!(err["file"].as_str().unwrap().ends_with("box.project.json"));
        assert_eq!(std::fs::read(&victim).unwrap(), before, "{start}");
        let err = fails(base, &["check", "game"]);
        assert_eq!(err["code"], "path_outside_project", "{start}: {err:#}");
        let err = fails(base, &["scene", "show", "game"]);
        assert_eq!(err["code"], "path_outside_project", "{start}: {err:#}");
    }
}

#[test]
fn file_fields_are_project_relative() {
    let project = new_project("relative_files");
    let base = project.parent().unwrap();
    let head = base.join("head");
    std::fs::create_dir_all(&head).unwrap();
    let root = project.to_str().unwrap();

    let shown = ok(&head, &["scene", "show", "../game"]);
    assert_eq!(shown["file"], SCENE);
    assert_eq!(shown["project"], root);
    let shown = ok(
        &head,
        &[
            "scene",
            "show",
            "../game/assets/../assets/scenes/main.scene.json",
        ],
    );
    assert_eq!(shown["file"], SCENE);

    let commands = write(
        base,
        "rename.json",
        &json!({ "op": "rename", "entity": "Cube", "name": "Box" }),
    );
    let applied = ok(
        &head,
        &["scene", "apply", "../game", commands.to_str().unwrap()],
    );
    assert_eq!(applied["file"], SCENE);
    assert_eq!(applied["project"], root);

    let checked = ok(&head, &["check", "../game"]);
    assert_eq!(checked["project"], root);
    assert_eq!(checked["scenes"][0]["file"], SCENE);

    let err = fails(&head, &["scene", "show", "../game", "--entity", "Nobody"]);
    assert_eq!(err["code"], "entity_not_found");
    assert_eq!(err["file"], SCENE);
    assert_eq!(err["project"], root);

    // Outside any project the path stays as given.
    let loose = base.join("loose.scene.json");
    std::fs::copy(project.join(SCENE), &loose).unwrap();
    let shown = ok(base, &["scene", "show", "loose.scene.json"]);
    assert_eq!(shown["file"], "loose.scene.json");
    assert_eq!(shown["project"], Json::Null);
}

#[test]
fn closed_stdout_is_not_a_panic() {
    let base = temp_dir("broken_pipe");
    for args in [&["registry", "--json"][..], &["registry"][..]] {
        let mut child = box_cmd(&base, args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // Close the read end before the child writes anything.
        drop(child.stdout.take());
        let out = child.wait_with_output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!stderr.contains("panicked"), "{args:?}: {stderr}");
        assert_eq!(out.status.code(), Some(0), "{args:?}: {stderr}");
    }
}

#[test]
fn usage_errors_exit_with_2_wherever_they_are_detected() {
    let base = temp_dir("usage");
    // Detected by clap.
    let (out, json) = run(&base, &["new", "--json"]);
    assert_eq!(json["error"]["code"], "usage");
    assert_eq!(out.status.code(), Some(2));
    // Detected by the library.
    let (out, json) = run(&base, &["new", "x", "--name", "", "--json"]);
    assert_eq!(json["error"]["code"], "usage");
    assert_eq!(out.status.code(), Some(2));
    assert!(!base.join("x").exists());
    // Other failures stay 1.
    let (out, json) = run(&base, &["check", "--json"]);
    assert_eq!(json["error"]["code"], "not_a_project");
    assert_eq!(out.status.code(), Some(1));
}

/// W1: views chosen without editing the scene: --eye/--look-at, --overview
/// and several poses from a --views file in one run.
#[test]
fn render_poses_overview_and_views() {
    if !gpu_tests_enabled() {
        return;
    }
    let project = new_project("poses");
    let before = std::fs::read(project.join(SCENE)).unwrap();
    let captures = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("cli_poses");
    let eye = captures.join("eye.png");
    let Some(r) = render(
        &project,
        &[
            ".",
            "--out",
            eye.to_str().unwrap(),
            "--size",
            "320x180",
            "--eye",
            "0,8,-6",
            "--look-at",
            "0,0,0",
            "--fov",
            "50",
        ],
    ) else {
        return;
    };
    assert_eq!(r["camera"]["pose"]["eye"], json!([0.0, 8.0, -6.0]));
    assert_eq!(r["camera"]["pose"]["fov"], 50.0);
    assert!(eye.is_file());

    let top = captures.join("top.png");
    let r = render(
        &project,
        &[
            ".",
            "--out",
            top.to_str().unwrap(),
            "--size",
            "320x180",
            "--overview",
        ],
    )
    .unwrap();
    assert_eq!(r["camera"]["overview"], true);

    let views = write(
        &project,
        "views.json",
        &json!([
            { "name": "front", "eye": [0.0, 2.0, 8.0], "look_at": [0.0, 0.5, 0.0] },
            { "name": "above", "overview": true },
            { "name": "scene_camera", "camera": "Camera" }
        ]),
    );
    let shots = captures.join("shot.png");
    let r = render(
        &project,
        &[
            ".",
            "--out",
            shots.to_str().unwrap(),
            "--size",
            "320x180",
            "--views",
            views.to_str().unwrap(),
        ],
    )
    .unwrap();
    let list = r["views"].as_array().unwrap();
    assert_eq!(list.len(), 3);
    for name in ["front", "above", "scene_camera"] {
        let file = captures.join(format!("shot_{name}.png"));
        assert!(file.is_file(), "{name}");
    }
    assert_eq!(list[2]["camera"]["name"], "Camera");
    let a = std::fs::read(captures.join("shot_front.png")).unwrap();
    let b = std::fs::read(captures.join("shot_above.png")).unwrap();
    assert_ne!(a, b, "different poses, different images");

    // Bad poses are usage errors; the scene file never changes.
    let err = fails(
        &project,
        &[
            "render",
            ".",
            "--out",
            "x.png",
            "--eye",
            "1,1,1",
            "--look-at",
            "1,1,1",
        ],
    );
    assert_eq!(err["code"], "usage", "{err:#}");
    let bad = write(
        &project,
        "bad.json",
        &json!([{ "name": "a b", "overview": true }]),
    );
    let err = fails(
        &project,
        &[
            "render",
            ".",
            "--out",
            "x.png",
            "--views",
            bad.to_str().unwrap(),
        ],
    );
    assert_eq!(err["code"], "usage", "{err:#}");
    assert_eq!(std::fs::read(project.join(SCENE)).unwrap(), before);
}

#[test]
fn render_refuses_outputs_that_are_not_png() {
    let project = new_project("render_out");

    let (out, json) = run(&project, &["render", SCENE, "--out", "shot.txt", "--json"]);
    assert_eq!(json["error"]["code"], "usage", "{json:#}");
    assert_eq!(out.status.code(), Some(2));
    assert!(!project.join("shot.txt").exists());

    // The scene file itself and a mislabelled text file both survive.
    let scene_before = std::fs::read(project.join(SCENE)).unwrap();
    let err = fails(&project, &["render", SCENE, "--out", SCENE]);
    assert_eq!(err["code"], "usage", "{err:#}");
    assert_eq!(std::fs::read(project.join(SCENE)).unwrap(), scene_before);

    std::fs::write(project.join("notes.png"), "not an image").unwrap();
    let err = fails(&project, &["render", SCENE, "--out", "notes.png"]);
    assert_eq!(err["code"], "already_exists", "{err:#}");
    assert_eq!(
        std::fs::read_to_string(project.join("notes.png")).unwrap(),
        "not an image"
    );
}

#[test]
fn render_replaces_an_existing_png_and_leaves_no_temporary_file() {
    if !gpu_tests_enabled() {
        return;
    }
    let project = new_project("render_replace");
    if render(
        &project,
        &[SCENE, "--out", "shots/a.PNG", "--size", "64x36"],
    )
    .is_none()
    {
        return;
    }
    render(
        &project,
        &[SCENE, "--out", "shots/a.PNG", "--size", "32x18"],
    )
    .unwrap();
    let img = image::open(project.join("shots/a.PNG")).unwrap();
    assert_eq!((img.width(), img.height()), (32, 18));
    let names: Vec<_> = std::fs::read_dir(project.join("shots"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["a.PNG"]);
}

#[test]
fn empty_command_list_does_not_rewrite_the_scene() {
    let project = new_project("apply_empty");
    let scene_path = project.join(SCENE);
    // Valid, but not formatted the way the engine writes it.
    let tree: Json = serde_json::from_slice(&std::fs::read(&scene_path).unwrap()).unwrap();
    let compact = serde_json::to_vec(&tree).unwrap();
    std::fs::write(&scene_path, &compact).unwrap();
    let commands = write(&project, "none.json", &json!([]));
    let applied = ok(
        &project,
        &["scene", "apply", SCENE, commands.to_str().unwrap()],
    );
    assert_eq!(applied["commands"], 0);
    assert_eq!(applied["written"], false);
    assert_eq!(std::fs::read(&scene_path).unwrap(), compact);
}

/// M4: an agent imports a model, places it as a prefab with `scene apply`
/// (the path-only reference is saved with its id) and renders it.
#[test]
fn import_place_and_render_a_model() {
    let project = new_project("import");
    let fixtures =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/box_assets/tests/fixtures");
    let glb = fixtures.join("models/tower.glb");
    let imported = ok(&project, &["import", glb.to_str().unwrap()]);
    assert_eq!(imported["file"], "assets/models/tower.glb");
    assert_eq!(imported["state"], "loaded");
    assert_eq!(imported["importer"], "gltf");
    let labels: Vec<&str> = imported["sub_assets"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["label"].as_str())
        .collect();
    assert!(
        labels.contains(&"Scene") && labels.contains(&"Mesh/Roof"),
        "{labels:?}"
    );
    assert!(project.join("assets/models/tower.glb.meta").is_file());
    let id = imported["id"].as_str().unwrap().to_string();
    // Importing again refuses to overwrite; a .gltf brings its files along.
    let again = fails(&project, &["import", glb.to_str().unwrap()]);
    assert_eq!(again["code"], "already_exists");
    let gltf = ok(
        &project,
        &[
            "import",
            fixtures.join("models/crate.gltf").to_str().unwrap(),
            "--to",
            "props",
        ],
    );
    assert_eq!(gltf["state"], "loaded", "{gltf:#}");
    assert!(project.join("assets/props/crate.bin").is_file());
    assert!(project.join("assets/props/crate_color.png").is_file());

    let commands = json!([
        { "op": "spawn", "name": "Tower", "components": {
            "PrefabInstance": { "scene": "models/tower.glb#Scene" },
            "Transform": { "translation": [0.0, 0.0, 2.0], "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [1.0, 1.0, 1.0] }
        } },
        { "op": "despawn", "entity": "Cube" }
    ]);
    let cmds = write(&project, "place.json", &commands);
    let applied = ok(&project, &["scene", "apply", SCENE, cmds.to_str().unwrap()]);
    assert_eq!(applied["refs_repaired"], 1);
    let show = ok(&project, &["scene", "show", SCENE]);
    let tower = entity(&show, "Tower");
    assert_eq!(
        tower["components"]["PrefabInstance"]["scene"],
        json!({ "id": id, "label": "Scene", "path": "models/tower.glb" })
    );
    assert_eq!(
        show["entities"].as_array().unwrap().len(),
        4,
        "only the root is saved"
    );

    if !gpu_tests_enabled() {
        return;
    }
    let out = project.join("tower.png");
    let Some(json) = render(
        &project,
        &[SCENE, "--out", out.to_str().unwrap(), "--size", "640x400"],
    ) else {
        return;
    };
    // Ground, the tower's base and roof.
    assert!(json["objects"].as_u64().unwrap() >= 3, "{json:#}");
    let captures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/m4");
    std::fs::create_dir_all(&captures).unwrap();
    std::fs::copy(&out, captures.join("cli_import_render.png")).unwrap();
    let img = image::open(&out).unwrap().to_rgba8();
    let red = img
        .pixels()
        .filter(|p| p.0[0] as i32 - p.0[1] as i32 > 50)
        .count();
    assert!(red > 200, "the tower's red/white checker and roof: {red}");
}
