//! `box upgrade` and the upgrade problems of `box check`, end to end.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value as Json, json};

fn run(cwd: &Path, args: &[&str]) -> (Output, Json) {
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
    (out, json)
}

fn ok(cwd: &Path, args: &[&str]) -> Json {
    let (out, json) = run(cwd, args);
    assert!(out.status.success(), "box {args:?} failed: {json:#}");
    json
}

fn new_project(name: &str) -> PathBuf {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cli_upgrade_{name}"));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    ok(&base, &["new", "game"]);
    base.join("game")
}

fn read_json(path: &Path) -> Json {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn dry_run_reports_apply_backs_up_and_newer_projects_are_refused() {
    let project = new_project("flow");
    let project_file = project.join("box.project.json");
    let mut pf = read_json(&project_file);
    pf["engine_version"] = json!("0.0.9");
    let original = serde_json::to_string_pretty(&pf).unwrap() + "\n";
    std::fs::write(&project_file, &original).unwrap();
    let dir = project.to_str().unwrap();

    // `box check` warns, and that is all.
    let check = ok(&project, &["check"]);
    assert_eq!(check["ok"], true, "{check:#}");
    let problems = check["problems"].as_array().unwrap();
    assert_eq!(problems.len(), 1, "{check:#}");
    assert_eq!(problems[0]["code"], "upgrade_needed");
    assert_eq!(problems[0]["severity"], "warning");
    assert_eq!(problems[0]["file"], "box.project.json");

    let dry = ok(&project, &["upgrade", "--project", dir, "--dry-run"]);
    let target = dry["target_engine_version"].as_str().unwrap().to_string();
    assert_eq!(dry["dry_run"], true);
    assert_eq!(dry["needed"], true);
    assert_eq!(dry["blocked"], false);
    assert_eq!(dry["applied"], false);
    assert_eq!(dry["engine_version"], "0.0.9");
    assert_eq!(
        dry["files"],
        json!([{
            "file": "box.project.json",
            "kind": "project",
            "changes": [format!("engine_version 0.0.9 -> {target}")]
        }])
    );
    assert_eq!(dry["newer"], json!([]));
    assert_eq!(dry["errors"], json!([]));
    assert_eq!(std::fs::read_to_string(&project_file).unwrap(), original);
    assert!(!project.join(".box/backups").exists());

    let done = ok(&project, &["upgrade", "--project", dir]);
    assert_eq!(done["applied"], true);
    assert_eq!(done["written"], json!(["box.project.json"]));
    let backup = project.join(done["backup"].as_str().unwrap());
    assert!(
        done["backup"]
            .as_str()
            .unwrap()
            .starts_with(".box/backups/")
    );
    assert_eq!(
        std::fs::read_to_string(backup.join("box.project.json")).unwrap(),
        original
    );
    assert_eq!(
        read_json(&backup.join("upgrade.json"))["engine_version"],
        "0.0.9"
    );
    assert_eq!(read_json(&project_file)["engine_version"], target.as_str());
    let again = ok(&project, &["upgrade", "--project", dir]);
    assert_eq!(
        (again["needed"].clone(), again["applied"].clone()),
        (json!(false), json!(false))
    );
    let check = ok(&project, &["check"]);
    assert_eq!(check["problems"], json!([]), "{check:#}");

    // A newer scene blocks the upgrade (exit code 1, the plan as JSON).
    let scene = project.join("assets/scenes/main.scene.json");
    let mut tree = read_json(&scene);
    tree["version"] = json!(99);
    std::fs::write(&scene, serde_json::to_vec_pretty(&tree).unwrap()).unwrap();
    let (out, blocked) = run(&project, &["upgrade", "--project", dir]);
    assert_eq!(out.status.code(), Some(1), "{blocked:#}");
    assert_eq!(blocked["blocked"], true);
    assert_eq!(blocked["newer"][0]["file"], "assets/scenes/main.scene.json");
    let (out, check) = run(&project, &["check"]);
    assert!(!out.status.success());
    assert!(
        check["problems"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["code"] == "newer_format"),
        "{check:#}"
    );

    // A project from a newer engine is refused before anything else.
    pf["engine_version"] = json!("99.0.0");
    std::fs::write(&project_file, serde_json::to_vec_pretty(&pf).unwrap()).unwrap();
    let (out, err) = run(&project, &["upgrade", "--project", dir]);
    assert!(!out.status.success());
    assert_eq!(err["error"]["code"], "project_too_new", "{err:#}");
}
