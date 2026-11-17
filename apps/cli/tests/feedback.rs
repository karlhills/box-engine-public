//! What the `box` binary tells people and agents: next steps, help texts,
//! readable registry output, the `problems` list of results, `box check`
//! on a project with broken assets, and import errors. Render parts are
//! skipped when `BOX_SKIP_GPU_TESTS` is set or no GPU adapter exists.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value as Json, json};

fn box_out(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_box"))
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("run box")
}

/// Run with `--json`: (success, JSON result, stderr).
fn run_json(cwd: &Path, args: &[&str]) -> (bool, Json, String) {
    let mut all = args.to_vec();
    all.push("--json");
    let out = box_out(cwd, &all);
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let json = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "box {args:?}: stdout is not JSON ({e}):\n{}\nstderr:\n{stderr}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    (out.status.success(), json, stderr)
}

/// Run without `--json`: (success, stdout, stderr).
fn run_text(cwd: &Path, args: &[&str]) -> (bool, String, String) {
    let out = box_out(cwd, args);
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cli_feedback_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

const SCENE: &str = "assets/scenes/main.scene.json";

fn codes(problems: &Json) -> Vec<String> {
    problems
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["code"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn new_prints_next_steps() {
    let base = temp_dir("next");
    let (ok, text, _) = run_text(&base, &["new", "my game"]);
    assert!(ok);
    assert!(
        text.ends_with(
            "Next: box-editor 'my game' · box run 'my game' · box render 'my game' --out shot.png\n"
        ),
        "{text}"
    );
    let (ok, json, _) = run_json(&base, &["new", "other"]);
    assert!(ok);
    assert_eq!(
        json["next"],
        json!([
            "box-editor other",
            "box run other",
            "box render other --out shot.png"
        ])
    );
    assert_eq!(json["created"], true);
    // An existing project: the error says what to do instead.
    let (ok, json, _) = run_json(&base, &["new", "other"]);
    assert!(!ok);
    assert_eq!(json["error"]["code"], "already_exists");
    assert!(
        json["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("box new --native")
    );
}

#[test]
fn help_shows_the_command_schema_and_mcp_setup() {
    let base = temp_dir("help");
    let (ok, apply, _) = run_text(&base, &["scene", "apply", "--help"]);
    assert!(ok);
    for op in [
        "spawn",
        "set_resource",
        "add_component",
        "remove_component",
        "reparent",
        "despawn",
        "rename",
        "duplicate",
    ] {
        assert!(apply.contains(&format!("  {op} ")), "{op}: {apply}");
    }
    assert!(apply.contains("Example:\n  [{\"op\": \"spawn\""), "{apply}");

    let (ok, mcp, _) = run_text(&base, &["mcp", "--help"]);
    assert!(ok);
    // The help shell-quotes the path (on Windows `'C:\...\box.exe'`).
    let canonical = std::fs::canonicalize(env!("CARGO_BIN_EXE_box")).unwrap();
    let candidates = [
        canonical.display().to_string(),
        env!("CARGO_BIN_EXE_box").to_string(),
    ];
    assert!(
        candidates.iter().any(|exe| {
            let quoted = box_engine::player::shell_line(std::slice::from_ref(exe));
            [exe.as_str(), quoted.as_str()]
                .iter()
                .any(|shown| mcp.contains(&format!("claude mcp add box -- {shown} mcp --project")))
        }),
        "{mcp}"
    );
    assert!(mcp.contains("box-editor <project> --control"), "{mcp}");
    assert!(mcp.contains("File ▸ Allow Agent Control"), "{mcp}");
    assert!(mcp.contains("box-player <project> --control"), "{mcp}");
}

#[test]
fn registry_text_is_readable() {
    let base = temp_dir("registry_text");
    let (ok, text, _) = run_text(&base, &["registry"]);
    assert!(ok);
    assert!(
        !text.contains("alloc::") && !text.contains("glam::"),
        "{text}"
    );
    assert!(text.contains("translation: vec3 [x, y, z] [m]"), "{text}");
    assert!(
        text.contains("materials: list of asset: null or {\"path\""),
        "{text}"
    );
    assert!(text.contains("tonemapper: Tonemapper: \"AgX\" | \"AcesFitted\" | \"None\""));
    // Docs are whole first sentences, not cut at a line break.
    assert!(
        text.contains("A perspective camera. Uses the entity's `GlobalTransform` (looks down −Z).")
            || text.contains("    A perspective camera.\n"),
        "{text}"
    );
    assert!(!text.contains("Saved in\n"), "{text}");
}

/// A project with a broken image, a shader with a bad header, a reference
/// to a missing model, a glTF that is not JSON and a scene with a mistyped
/// component.
fn broken_project(name: &str) -> PathBuf {
    let base = temp_dir(name);
    let (ok, _, _) = run_json(&base, &["new", "game"]);
    assert!(ok);
    let root = base.join("game");
    let assets = root.join("assets");
    std::fs::create_dir_all(assets.join("textures")).unwrap();
    std::fs::create_dir_all(assets.join("shaders")).unwrap();
    std::fs::write(assets.join("textures/bad.png"), "not a png").unwrap();
    std::fs::write(
        assets.join("shaders/bad.wgsl"),
        "@material struct P {\n    @bogus x: f32 = 1.0,\n}\nfn surface(in: SurfaceInput, p: P) -> Surface { return default_surface(in); }\n",
    )
    .unwrap();
    std::fs::write(assets.join("bad.gltf"), "{ \"asset\": ").unwrap();
    let commands = json!([
        { "op": "spawn", "name": "Shaded", "components": {
            "Primitive": { "shape": { "Sphere": { "radius": 0.6, "sectors": 24, "stacks": 12 } }, "cast_shadows": true },
            "ShaderMaterial": { "shader": { "path": "shaders/bad.wgsl" }, "params": [], "alpha_mode": "Opaque", "double_sided": false }
        } },
        { "op": "set", "entity": "Cube", "path": "StandardMaterial.base_color_texture", "value": { "path": "textures/bad.png" } },
        { "op": "spawn", "name": "Ghost", "components": {
            "MeshAsset": { "mesh": { "path": "models/nope.glb", "label": "Mesh/A" }, "materials": [], "cast_shadows": true }
        } }
    ]);
    std::fs::write(root.join("cmds.json"), commands.to_string()).unwrap();

    // `scene apply` writes the dangling reference, with a warning.
    let (ok, applied, stderr) = run_json(&root, &["scene", "apply", SCENE, "cmds.json"]);
    assert!(ok, "{applied:#}");
    assert_eq!(applied["written"], true);
    let problems = applied["problems"].as_array().unwrap();
    assert_eq!(problems.len(), 1, "{applied:#}");
    assert_eq!(problems[0]["code"], "asset_not_found");
    assert_eq!(problems[0]["severity"], "warning");
    assert_eq!(problems[0]["entity_name"], "Ghost");
    assert_eq!(problems[0]["asset"], "models/nope.glb#Mesh/A");
    assert!(
        !stderr.contains("WARN"),
        "reported once, in the result: {stderr}"
    );
    let (_, text, _) = run_text(&root, &["scene", "apply", SCENE, "cmds.json", "--dry-run"]);
    assert!(
        text.contains("warning[asset_not_found]: entity `Ghost`: MeshAsset references `models/nope.glb#Mesh/A`, which is not in assets/"),
        "{text}"
    );

    // A scene whose `Transform` is mistyped everywhere.
    let main = std::fs::read_to_string(root.join(SCENE)).unwrap();
    std::fs::write(
        root.join("assets/scenes/typo.scene.json"),
        main.replace("\"Transform\":", "\"Tranform\":"),
    )
    .unwrap();
    root
}

#[test]
fn check_imports_assets_and_resolves_references() {
    let root = broken_project("check");
    let (ok, report, _) = run_json(&root, &["check"]);
    assert!(!ok, "errors exit with 1");
    assert_eq!(report["ok"], false);
    let problems = report["problems"].as_array().unwrap();
    let find = |code: &str, asset: &str| {
        problems
            .iter()
            .find(|p| p["code"] == code && p["asset"].as_str().is_some_and(|a| a.contains(asset)))
            .unwrap_or_else(|| panic!("no {code} for {asset}: {report:#}"))
    };
    let png = find("asset_import_failed", "textures/bad.png");
    assert_eq!(png["file"], "assets/textures/bad.png");
    assert_eq!(png["severity"], "error");
    find("asset_import_failed", "bad.gltf");
    let shader = find("shader_header", "shaders/bad.wgsl");
    assert!(
        shader["message"]
            .as_str()
            .unwrap()
            .contains("shaders/bad.wgsl:2: bad @material header"),
        "{shader:#}"
    );
    let missing = find("asset_not_found", "models/nope.glb");
    assert_eq!(missing["entity_name"], "Ghost");
    assert_eq!(missing["component"], "MeshAsset");
    let typo = problems
        .iter()
        .find(|p| p["code"] == "unknown_component")
        .unwrap();
    assert_eq!(typo["file"], "assets/scenes/typo.scene.json");
    assert_eq!(typo["suggestion"], "Transform");
    assert!(
        typo["message"].as_str().unwrap().contains(
            "unknown component `Tranform` on 6 entities (data kept); did you mean `Transform`?"
        ),
        "{typo:#}"
    );
    assert_eq!(typo["entities"].as_array().unwrap().len(), 6);
    assert_eq!(
        problems
            .iter()
            .filter(|p| p["code"] == "unknown_component")
            .count(),
        1,
        "one problem per type"
    );
    // Each failure once: no log records repeating the import errors.
    assert!(
        !codes(&report["problems"]).contains(&"log".to_string()),
        "{report:#}"
    );
    assert_eq!(report["errors"], 5, "{report:#}");
    assert!(report["assets"].as_u64().unwrap() >= 5);

    let (ok, text, stderr) = run_text(&root, &["check"]);
    assert!(!ok);
    assert!(
        text.contains("error[shader_header]: assets/shaders/bad.wgsl: "),
        "{text}"
    );
    assert!(
        text.lines()
            .last()
            .unwrap()
            .starts_with("failed: 2 scene(s), "),
        "{text}"
    );
    assert!(
        !stderr.contains("ERROR"),
        "not repeated on stderr: {stderr}"
    );
}

/// The error-reporting pass (M9): common failures carry the problem, the
/// place and a fix, with stable codes.
#[test]
fn common_failures_say_what_to_do() {
    let base = temp_dir("common");
    // A project folder that does not exist (typo, moved, renamed).
    let (ok, json, _) = run_json(&base, &["check", "gone"]);
    assert!(!ok);
    assert_eq!(json["error"]["code"], "not_found");
    let message = json["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("gone does not exist; check the path (was the project moved or renamed?)"),
        "{message}"
    );

    // A start scene that does not exist.
    let (ok, _, _) = run_json(&base, &["new", "game"]);
    assert!(ok);
    let root = base.join("game");
    std::fs::rename(
        root.join(SCENE),
        root.join("assets/scenes/level.scene.json"),
    )
    .unwrap();
    let (ok, json, _) = run_json(&base, &["run", "game", "--frames", "1"]);
    assert!(!ok);
    assert_eq!(json["error"]["code"], "start_scene_missing", "{json:#}");
    let message = json["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("the start scene `assets/scenes/main.scene.json` does not exist (set by `start_scene` in "),
        "{message}"
    );
    assert!(
        message.contains("point `start_scene` at an existing one"),
        "{message}"
    );

    // A material shader that does not compile is found without a GPU, and a
    // damaged binary file says to export it again.
    let assets = root.join("assets");
    std::fs::create_dir_all(assets.join("shaders")).unwrap();
    std::fs::create_dir_all(assets.join("audio")).unwrap();
    std::fs::write(
        assets.join("shaders/wave.wgsl"),
        "fn surface(in: SurfaceInput, p: MaterialParams) -> Surface {\n    var s = default_surface(in);\n    s.base_color = vec4<f32>(1.0, 0.0, 0.0, undefined_alpha);\n    return s;\n}\n",
    )
    .unwrap();
    std::fs::write(assets.join("audio/hit.wav"), "RIFFxxxxWAVEjunk").unwrap();
    let (ok, report, _) = run_json(&root, &["check"]);
    assert!(!ok);
    let problems = report["problems"].as_array().unwrap();
    let shader = problems
        .iter()
        .find(|p| p["code"] == "shader_invalid")
        .unwrap_or_else(|| panic!("{report:#}"));
    assert_eq!(shader["file"], "assets/shaders/wave.wgsl");
    let m = shader["message"].as_str().unwrap();
    assert!(m.contains("shaders/wave.wgsl:3"), "{m}");
    assert!(m.contains("undefined_alpha"), "{m}");
    let wav = problems
        .iter()
        .find(|p| p["asset"] == "audio/hit.wav")
        .unwrap_or_else(|| panic!("{report:#}"));
    assert_eq!(wav["code"], "asset_import_failed");
    assert!(
        wav["message"].as_str().unwrap().ends_with(
            "; the file is damaged, incomplete or not really WAV: export or download it again"
        ),
        "{wav:#}"
    );
    assert!(
        problems.iter().any(|p| p["code"] == "start_scene_missing"
            && p["message"]
                .as_str()
                .unwrap()
                .contains("point `start_scene` at an existing one")),
        "{report:#}"
    );
}

#[test]
fn render_reports_failed_assets_in_problems() {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        return;
    }
    let root = broken_project("render");
    let (ok, json, stderr) = run_json(
        &root,
        &["render", ".", "--out", "shot.png", "--size", "320x180"],
    );
    if json["error"]["code"] == "no_gpu" {
        return;
    }
    assert!(ok, "the image is still rendered: {json:#}");
    assert!(root.join("shot.png").is_file());
    let mut found = codes(&json["problems"]);
    found.sort();
    assert_eq!(
        found,
        ["asset_import_failed", "asset_not_found", "shader_header"],
        "{json:#}"
    );
    assert!(!stderr.contains("ERROR"), "{stderr}");
    let (_, text, _) = run_text(
        &root,
        &["render", ".", "--out", "shot.png", "--size", "320x180"],
    );
    assert!(
        text.contains("3 problem(s): 3 error(s), 0 warning(s)"),
        "{text}"
    );
    assert!(
        text.contains("error[asset_import_failed]: entity `Cube`: textures/bad.png: "),
        "{text}"
    );
}

#[test]
fn import_errors_say_what_happened_once() {
    let base = temp_dir("import_errors");
    let (ok, _, _) = run_json(&base, &["new", "game"]);
    assert!(ok);
    let root = base.join("game");
    std::fs::write(base.join("broken.png"), "xx").unwrap();
    let (ok, text, stderr) = run_text(&root, &["import", "../broken.png"]);
    assert!(!ok);
    assert!(
        text.starts_with(
            "Copied to assets/textures/broken.png; import failed: not a PNG or JPEG image"
        ),
        "{text}"
    );
    assert!(
        text.contains("The file was kept: fix it (hot reload re-imports it) or delete it."),
        "{text}"
    );
    assert!(!text.contains("Imported"), "{text}");
    assert!(!stderr.contains("import failed"), "printed once: {stderr}");
    assert!(root.join("assets/textures/broken.png").is_file());

    let (ok, json, _) = run_json(&root, &["import", "../broken.png"]);
    assert!(!ok);
    assert_eq!(json["error"]["code"], "already_exists");
    assert!(json["error"]["hint"].as_str().unwrap().contains("--to"));
    let (_, _, stderr) = run_text(&root, &["import", "../broken.png"]);
    assert!(
        stderr.contains("\n  hint: import it into another folder with --to"),
        "{stderr}"
    );

    std::fs::write(base.join("ship.fbx"), "fbx").unwrap();
    let (ok, json, _) = run_json(&root, &["import", "../ship.fbx"]);
    assert!(!ok);
    assert_eq!(json["error"]["code"], "unsupported_format");
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("FBX is not supported: convert it to glTF")
    );
    assert!(!root.join("assets/ship.fbx").exists());
}

#[test]
fn scene_apply_reports_clamped_values() {
    let base = temp_dir("clamped");
    let (ok, _, _) = run_json(&base, &["new", "game"]);
    assert!(ok);
    let root = base.join("game");
    std::fs::write(
        root.join("cmds.json"),
        json!({ "op": "set", "entity": "Cube", "path": "StandardMaterial.roughness", "value": 5.0 })
            .to_string(),
    )
    .unwrap();
    let (ok, json, stderr) = run_json(&root, &["scene", "apply", SCENE, "cmds.json"]);
    assert!(ok, "{json:#}");
    assert_eq!(codes(&json["problems"]), ["value_clamped"], "{json:#}");
    assert!(!stderr.contains("WARN"), "once, in the result: {stderr}");
}

/// A glTF's external ORM image gets a linear `.meta` when the import creates
/// it, so later runs do not warn.
#[test]
fn gltf_import_makes_new_orm_metas_linear() {
    let base = temp_dir("orm");
    let (ok, _, _) = run_json(&base, &["new", "game"]);
    assert!(ok);
    let root = base.join("game");
    let fixtures =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/box_assets/tests/fixtures/models");
    let (ok, json, _) = run_json(
        &root,
        &["import", fixtures.join("crate.gltf").to_str().unwrap()],
    );
    assert!(ok, "{json:#}");
    assert_eq!(json["problems"], json!([]), "{json:#}");
    let meta: Json = serde_json::from_slice(
        &std::fs::read(root.join("assets/models/crate_mr.png.meta")).unwrap(),
    )
    .unwrap();
    assert_eq!(meta["settings"]["color_space"], "linear");
    let (ok, report, _) = run_json(&root, &["check"]);
    assert!(ok, "{report:#}");
    assert_eq!(report["problems"], json!([]), "{report:#}");
}
