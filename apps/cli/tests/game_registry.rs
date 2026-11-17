//! A native project's game types in the `box` CLI: `.box/registry.json`
//! (what the game's editor and player write) is merged into `registry`,
//! `check`, `scene show` and `scene apply`, so game components can be edited
//! without the game's binary.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

use serde_json::{Value as Json, json};

fn run(cwd: &Path, args: &[&str]) -> (Output, Json) {
    let mut all = args.to_vec();
    all.push("--json");
    let out = Command::new(env!("CARGO_BIN_EXE_box"))
        .current_dir(cwd)
        .args(&all)
        .output()
        .expect("run box");
    let json = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "box {args:?}: stdout is not JSON ({e}):\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (out, json)
}

fn ok(cwd: &Path, args: &[&str]) -> Json {
    let (out, json) = run(cwd, args);
    assert!(out.status.success(), "box {args:?} failed: {json:#}");
    json
}

fn fails(cwd: &Path, args: &[&str]) -> Json {
    let (out, json) = run(cwd, args);
    assert!(!out.status.success(), "box {args:?} should fail: {json:#}");
    json["error"].clone()
}

/// A project with a Cargo.toml (native) and no registry file yet.
fn native_project(name: &str) -> PathBuf {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("game_registry_{name}"));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    ok(&base, &["new", "game"]);
    let root = base.join("game");
    std::fs::create_dir_all(root.join("src/bin")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"game\"\n\n[[bin]]\nname = \"player\"\npath = \"src/bin/player.rs\"\n",
    )
    .unwrap();
    std::fs::write(root.join("src/lib.rs"), "// the game\n").unwrap();
    std::fs::write(root.join("src/bin/player.rs"), "fn main() {}\n").unwrap();
    root
}

/// What a native game's binary writes: its types in the `box registry
/// --json` format (engine types left out here; tools skip them anyway).
fn registry_file() -> Json {
    json!({
        "format": 2,
        "engine_version": env!("CARGO_PKG_VERSION"),
        "modules": ["physics", "dynamics", "audio", "script"],
        "hash": "0123456789abcdef",
        "components": [{
            "name": "Spin",
            "type_path": "game::Spin",
            "kind": "struct",
            "doc": "Spins its entity around the Y axis.",
            "saved": true,
            "hidden": false,
            "fields": [{
                "name": "speed", "type_path": "f32", "doc": "Turns per second.",
                "range": { "min": -4.0, "max": 4.0 }, "unit": "turn/s", "step": null, "hidden": false
            }, {
                "name": "axis", "type_path": "glam::Vec3", "doc": null,
                "range": null, "unit": null, "step": null, "hidden": false
            }],
            "default": { "speed": 0.25, "axis": [0.0, 1.0, 0.0] }
        }],
        "resources": [{
            "name": "CharacterSettings",
            "type_path": "game::CharacterSettings",
            "kind": "struct",
            "doc": "Character tunables.",
            "saved": false,
            "hidden": false,
            "settings": true,
            "file": "assets/settings/CharacterSettings.json",
            "fields": [{
                "name": "skin", "type_path": "f32", "doc": null,
                "range": { "min": 0.0, "max": 0.2 }, "unit": "m", "step": null, "hidden": false
            }],
            "default": { "skin": 0.02 }
        }]
    })
}

fn set_mtime(path: &Path, t: SystemTime) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(t)
        .unwrap();
}

/// Writes the registry file, newer than the sources unless `stale`.
fn write_registry(root: &Path, stale: bool) -> PathBuf {
    let path = root.join(".box/registry.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&registry_file()).unwrap(),
    )
    .unwrap();
    let now = SystemTime::now();
    let (registry, sources) = if stale {
        (now - Duration::from_secs(60), now)
    } else {
        (now, now - Duration::from_secs(60))
    };
    set_mtime(&path, registry);
    for f in ["Cargo.toml", "src/lib.rs", "src/bin/player.rs"] {
        set_mtime(&root.join(f), sources);
    }
    path
}

fn codes(json: &Json) -> Vec<String> {
    json["problems"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p["code"].as_str().map(str::to_string))
        .collect()
}

const SCENE: &str = "assets/scenes/main.scene.json";

#[test]
fn scene_apply_edits_game_components_without_the_game_binary() {
    let root = native_project("apply");
    // Without the registry file: game types are unknown, and check says why.
    let check = ok(&root, &["check"]);
    assert!(
        codes(&check).contains(&"registry_missing".to_string()),
        "{check:#}"
    );
    let missing = check["problems"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["code"] == "registry_missing")
        .unwrap();
    assert!(
        missing["hint"]
            .as_str()
            .unwrap()
            .contains("--bin player -- --write-registry"),
        "{missing:#}"
    );
    let commands = root.join("spin.json");
    std::fs::write(
        &commands,
        r#"[{ "op": "spawn", "name": "Spinner", "components": { "Spin": { "speed": 1.0 } } },
            { "op": "set", "entity": "Spinner", "path": "Spin.speed", "value": 9.0 },
            { "op": "set", "entity": "Spinner", "path": "Spin.axis.x", "value": 1.0 },
            { "op": "set_resource", "path": "CharacterSettings.skin", "value": 0.05 }]"#,
    )
    .unwrap();
    let e = fails(&root, &["scene", "apply", SCENE, "spin.json"]);
    assert!(e["message"].as_str().unwrap().contains("Spin"), "{e:#}");

    write_registry(&root, false);
    // The registry lists the game's types, marked.
    let registry = ok(&root, &["registry"]);
    let spin = registry["components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "Spin")
        .unwrap_or_else(|| panic!("no Spin in {registry:#}"));
    assert_eq!(spin["game"], true);
    assert!(
        registry["components"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["name"] == "Transform"),
        "engine types too"
    );
    assert_eq!(
        registry["game_registry"]["types"],
        json!(["CharacterSettings", "Spin"])
    );
    assert_eq!(registry["game_registry"]["stale"], false);
    let text = Command::new(env!("CARGO_BIN_EXE_box"))
        .current_dir(&root)
        .arg("registry")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(text.contains("component Spin (game)"), "{text}");
    assert!(
        text.contains("resource CharacterSettings (game, settings file)"),
        "{text}"
    );
    // Outside the project, --project selects it.
    let other = ok(root.parent().unwrap(), &["registry", "--project", "game"]);
    assert_eq!(other["game_registry"]["file"], ".box/registry.json");

    // Applied without the game binary, with the range clamp reported.
    let applied = ok(&root, &["scene", "apply", SCENE, "spin.json"]);
    assert_eq!(applied["written"], true);
    assert_eq!(codes(&applied), ["value_clamped"], "{applied:#}");
    let show = ok(&root, &["scene", "show", SCENE, "--entity", "Spinner"]);
    assert_eq!(
        show["entity"]["components"]["Spin"],
        json!({ "speed": 4.0, "axis": [1.0, 1.0, 0.0] }),
        "{show:#}"
    );
    let show = ok(&root, &["scene", "show", SCENE]);
    assert_eq!(
        show["resources"]["CharacterSettings"],
        json!({ "skin": 0.05 })
    );
    // Check: no unknown-component warnings for game types any more.
    let check = ok(&root, &["check"]);
    assert_eq!(check["ok"], true, "{check:#}");
    assert!(codes(&check).is_empty(), "{check:#}");

    // Wrong types and fields are refused, naming the field.
    let bad = |cmd: &str| {
        std::fs::write(root.join("bad.json"), cmd).unwrap();
        fails(&root, &["scene", "apply", SCENE, "bad.json"])["message"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let m = bad(r#"{ "op": "set", "entity": "Spinner", "path": "Spin.speed", "value": "fast" }"#);
    assert!(m.contains("`Spin.speed`") && m.contains("f32"), "{m}");
    let m = bad(r#"{ "op": "set", "entity": "Spinner", "path": "Spin.sped", "value": 1.0 }"#);
    assert!(m.contains("did you mean `speed`"), "{m}");
    let m = bad(
        r#"{ "op": "add_component", "entity": "Cube", "component": "Spin", "value": { "axis": [1, 2] } }"#,
    );
    assert!(m.contains("`Spin.axis`"), "{m}");
    // A scene edited by hand into a wrong shape is a check error.
    let path = root.join(SCENE);
    let mut tree: Json = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for e in tree["entities"].as_array_mut().unwrap() {
        if e["name"] == "Spinner" {
            e["components"]["Spin"]["speed"] = json!("x");
        }
    }
    std::fs::write(&path, serde_json::to_vec_pretty(&tree).unwrap()).unwrap();
    let (out, check) = run(&root, &["check"]);
    assert!(!out.status.success());
    let p = check["problems"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["code"] == "invalid_component")
        .unwrap_or_else(|| panic!("{check:#}"));
    assert_eq!(p["entity_name"], "Spinner");
    assert_eq!(p["component"], "Spin");
}

#[test]
fn a_stale_registry_is_a_warning_with_the_refresh_command() {
    let root = native_project("stale");
    write_registry(&root, true);
    let check = ok(&root, &["check"]);
    let stale = check["problems"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["code"] == "registry_stale")
        .unwrap_or_else(|| panic!("{check:#}"));
    assert_eq!(stale["severity"], "warning");
    assert!(
        stale["message"]
            .as_str()
            .unwrap()
            .contains("sources changed"),
        "{stale:#}"
    );
    assert!(
        stale["hint"].as_str().unwrap().contains("--write-registry"),
        "{stale:#}"
    );
    assert_eq!(check["game_registry"]["stale"], true);
    // The game types still work, with the warning attached.
    std::fs::write(
        root.join("add.json"),
        r#"{ "op": "add_component", "entity": "Cube", "component": "Spin" }"#,
    )
    .unwrap();
    let applied = ok(&root, &["scene", "apply", SCENE, "add.json"]);
    assert_eq!(codes(&applied), ["registry_stale"], "{applied:#}");

    // Another engine version is stale too.
    let path = root.join(".box/registry.json");
    let mut file = registry_file();
    file["engine_version"] = json!("0.0.1");
    std::fs::write(&path, file.to_string()).unwrap();
    let registry = ok(&root, &["registry"]);
    let p = &registry["problems"][0];
    assert_eq!(p["code"], "registry_stale");
    assert!(
        p["message"].as_str().unwrap().contains("engine 0.0.1"),
        "{p:#}"
    );

    // A file that is not a registry: a warning, engine types only.
    std::fs::write(&path, "{ not json").unwrap();
    let registry = ok(&root, &["registry"]);
    assert_eq!(registry["problems"][0]["code"], "registry_invalid");
}

#[test]
fn check_validates_settings_files_of_game_settings_types() {
    let root = native_project("settings");
    write_registry(&root, false);
    let settings = root.join("assets/settings");
    std::fs::create_dir_all(&settings).unwrap();
    std::fs::write(
        settings.join("CharacterSettings.json"),
        r#"{ "skin": 0.05 }"#,
    )
    .unwrap();
    let check = ok(&root, &["check"]);
    assert!(codes(&check).is_empty(), "{check:#}");
    std::fs::write(
        settings.join("CharacterSettings.json"),
        r#"{ "skin": "thin" }"#,
    )
    .unwrap();
    let (out, check) = run(&root, &["check"]);
    assert!(!out.status.success());
    let p = check["problems"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["code"] == "invalid_settings")
        .unwrap_or_else(|| panic!("{check:#}"));
    assert_eq!(p["file"], "assets/settings/CharacterSettings.json");
    assert!(
        p["message"]
            .as_str()
            .unwrap()
            .contains("CharacterSettings.skin"),
        "{p:#}"
    );
    // A file that is not JSON is the importer's error.
    std::fs::write(settings.join("CharacterSettings.json"), "{").unwrap();
    let (_, check) = run(&root, &["check"]);
    assert!(
        codes(&check).contains(&"asset_import_failed".to_string()),
        "{check:#}"
    );
}

/// A lean native game (`modules` without `dynamics` and `script`): `box
/// check` reports scene types and settings files of the left-out modules as
/// `module_disabled` with the feature to enable (M8 gap report items 20
/// and 21), and `registry` lists the modules.
#[test]
fn check_reports_types_and_settings_of_left_out_modules() {
    let root = native_project("lean");
    let path = write_registry(&root, false);
    let mut file = registry_file();
    file["modules"] = json!(["physics", "audio"]);
    std::fs::write(&path, serde_json::to_string_pretty(&file).unwrap()).unwrap();
    set_mtime(&path, SystemTime::now());
    let check = ok(&root, &["check"]);
    assert!(codes(&check).is_empty(), "{check:#}");
    assert_eq!(
        check["game_registry"]["modules"],
        json!(["physics", "audio"]),
        "{check:#}"
    );

    // A scene using a rigid body, a script and a collider (physics is in).
    let scene_path = root.join(SCENE);
    let mut scene: Json =
        serde_json::from_str(&std::fs::read_to_string(&scene_path).unwrap()).unwrap();
    scene["entities"].as_array_mut().unwrap().push(json!({
        "uid": "6a1b8f0e-1111-4c3e-9a55-0123456789ab",
        "name": "Crate",
        "components": {
            "Transform": {},
            "Collider": { "shape": { "Cuboid": { "size": [1, 1, 1] } } },
            "RigidBody": { "kind": "Dynamic" },
            "Script": { "source": "scripts/crate.luau" }
        }
    }));
    std::fs::write(&scene_path, serde_json::to_string_pretty(&scene).unwrap()).unwrap();
    let settings = root.join("assets/settings");
    std::fs::create_dir_all(&settings).unwrap();
    std::fs::write(settings.join("PhysicsSettings.json"), "{}").unwrap();
    let check = ok(&root, &["check"]);
    let disabled: Vec<&Json> = check["problems"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["code"] == "module_disabled")
        .collect();
    let mut found: Vec<(String, String)> = disabled
        .iter()
        .map(|p| {
            (
                p["module"].as_str().unwrap().to_string(),
                p["component"]
                    .as_str()
                    .or(p["file"].as_str())
                    .unwrap()
                    .to_string(),
            )
        })
        .collect();
    found.sort();
    assert_eq!(
        found,
        [
            ("dynamics".into(), "RigidBody".into()),
            (
                "dynamics".into(),
                "assets/settings/PhysicsSettings.json".into()
            ),
            ("script".into(), "Script".into()),
        ],
        "{check:#}"
    );
    assert!(
        disabled
            .iter()
            .all(|p| p["severity"] == "warning" && p["hint"].as_str().is_some()),
        "{check:#}"
    );
    assert!(!codes(&check).contains(&"unknown_component".to_string()));
}
