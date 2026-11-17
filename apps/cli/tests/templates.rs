//! Project templates and the examples gallery through the `box` binary
//! (M9: "project templates and at least three example projects"; "the
//! example projects open and run"):
//!
//! - `box new --list-templates` and the error for an unknown template;
//! - every data template creates a project that `box check` passes with no
//!   problems and that runs 60 frames headless with no warnings, plus a
//!   capture in `target/captures/templates/new_<name>.png`;
//! - every example of the gallery (`templates/examples/examples.json`),
//!   copied as the editor copies it, checks and runs headless.
//!
//! The runs are skipped when `BOX_SKIP_GPU_TESTS` is set or there is no GPU
//! adapter. The native template builds in `run_native.rs` and in
//! `native.rs`'s unit test.

use std::path::{Path, PathBuf};
use std::process::Command;

use box_engine::templates;
use serde_json::Value as Json;

fn run(cwd: &Path, args: &[&str]) -> (bool, Option<i32>, Json, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_box"))
        .current_dir(cwd)
        .args(args)
        .arg("--json")
        .output()
        .expect("run box");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let json = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "box {args:?}: stdout is not JSON ({e}):\n{}\nstderr:\n{stderr}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    (out.status.success(), out.status.code(), json, stderr)
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("templates_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn gpu_tests_enabled() -> bool {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping the headless runs");
        return false;
    }
    true
}

/// `box check` passes with no errors and no warnings.
fn check_clean(base: &Path, project: &Path) {
    let p = project.to_str().unwrap();
    let (ok, _, json, stderr) = run(base, &["check", p]);
    assert!(ok, "box check {p}: {json:#}\n{stderr}");
    assert_eq!(json["errors"], 0, "{json:#}");
    assert_eq!(json["warnings"], 0, "{json:#}");
    assert_eq!(json["problems"], Json::Array(Vec::new()), "{json:#}");
}

/// `box run --frames 60` (optionally with a capture): success, no wgpu
/// validation errors, no problems. `false` without a GPU adapter.
fn runs_clean(base: &Path, project: &Path, capture: Option<&Path>) -> bool {
    let p = project.to_str().unwrap();
    let mut args = vec!["run", p, "--frames", "60"];
    let capture_text;
    if let Some(c) = capture {
        capture_text = c.to_str().unwrap().to_string();
        args.extend(["--capture", &capture_text]);
    }
    let (ok, _, json, stderr) = run(base, &args);
    if json["error"]["code"] == "no_gpu" {
        eprintln!("no GPU adapter; skipping");
        return false;
    }
    assert!(ok, "box run {p}: {json:#}\n{stderr}");
    assert_eq!(json["validation_errors"], 0, "{json:#}");
    assert_eq!(json["problems"], Json::Array(Vec::new()), "{p}: {json:#}");
    assert!(
        !stderr.contains(" WARN ") && !stderr.contains(" ERROR "),
        "{p} logged warnings:\n{stderr}"
    );
    true
}

#[test]
fn list_templates_names_every_template() {
    let base = temp_dir("list");
    let (ok, _, json, _) = run(&base, &["new", "--list-templates"]);
    assert!(ok, "{json:#}");
    assert_eq!(json["default"], "empty");
    let list = json["templates"].as_array().unwrap();
    let names: Vec<&str> = list.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["empty", "third-person", "procedural", "native"]);
    for t in list {
        assert!(!t["title"].as_str().unwrap().is_empty(), "{t:#}");
        assert!(!t["description"].as_str().unwrap().is_empty(), "{t:#}");
        let preview = t["preview"].as_str().expect("every template has a preview");
        assert!(Path::new(preview).is_file(), "{preview}");
        assert!(Path::new(t["folder"].as_str().unwrap()).is_dir(), "{t:#}");
    }
    assert_eq!(list[3]["native"], true);
    // Nothing else may come with it.
    let (ok, code, json, _) = run(&base, &["new", "x", "--list-templates"]);
    assert!(!ok);
    assert_eq!(code, Some(2));
    assert_eq!(json["error"]["code"], "usage");
    assert!(!base.join("x").exists());
}

#[test]
fn unknown_templates_and_conflicting_flags_are_usage_errors() {
    let base = temp_dir("unknown");
    let (ok, code, json, _) = run(&base, &["new", "game", "--template", "platformer"]);
    assert!(!ok);
    assert_eq!(code, Some(2));
    assert_eq!(json["error"]["code"], "usage");
    assert_eq!(
        json["error"]["message"],
        "unknown template `platformer`; available: empty, third-person, procedural, native"
    );
    assert_eq!(
        json["error"]["available"],
        serde_json::json!(["empty", "third-person", "procedural", "native"])
    );
    assert!(!base.join("game").exists(), "nothing is created");

    let (ok, code, json, _) = run(
        &base,
        &["new", "game", "--native", "--template", "procedural"],
    );
    assert!(!ok);
    assert_eq!(code, Some(2));
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("--native means `--template native`"),
        "{json:#}"
    );
    let (ok, _, json, _) = run(
        &base,
        &[
            "new",
            "game",
            "--template",
            "procedural",
            "--engine-path",
            ".",
        ],
    );
    assert!(!ok);
    assert_eq!(json["error"]["code"], "usage");
    assert!(!base.join("game").exists());
}

#[test]
fn every_template_creates_a_project_that_checks_and_runs() {
    let base = temp_dir("each");
    let gpu = gpu_tests_enabled();
    let captures =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/templates");
    std::fs::create_dir_all(&captures).unwrap();
    for (template, entity) in [
        ("empty", "Cube"),
        ("third-person", "Player"),
        ("procedural", "Garden Path"),
    ] {
        let dir = base.join(format!("my-{template}"));
        let (ok, _, json, stderr) = run(&base, &["new", dir.to_str().unwrap(), "-t", template]);
        assert!(ok, "{template}: {json:#}\n{stderr}");
        assert_eq!(json["template"], template);
        assert_eq!(json["name"], format!("my-{template}"));
        // The template's own files stay behind; its README comes along.
        assert!(dir.join("README.md").is_file(), "{template}");
        assert!(!dir.join("template.json").exists(), "{template}");
        assert!(!dir.join("preview.png").exists(), "{template}");
        let project: Json =
            serde_json::from_slice(&std::fs::read(dir.join("box.project.json")).unwrap()).unwrap();
        assert_eq!(project["name"], format!("my-{template}"));
        assert_eq!(
            project["engine_version"],
            box_engine::project::ENGINE_VERSION
        );

        let (ok, _, show, _) = run(&base, &["scene", "show", dir.to_str().unwrap()]);
        assert!(ok, "{show:#}");
        assert!(
            show["entities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["name"] == entity),
            "{template}: no `{entity}`"
        );
        check_clean(&base, &dir);
        if gpu {
            let shot = captures.join(format!("new_{template}.png"));
            if !runs_clean(&base, &dir, Some(&shot)) {
                return;
            }
            assert!(shot.is_file(), "{}", shot.display());
        }
        // Checking and running wrote no file into the template itself.
        let source = templates::find_template(&templates::templates(), template)
            .unwrap()
            .dir
            .clone()
            .unwrap();
        assert!(!source.join(".box").exists(), "{}", source.display());
    }
}

/// The gallery's examples, copied the way the editor's Open Example copies
/// them, pass `box check` without errors and run 60 frames headless
/// without warnings. The originals are only read.
#[test]
fn every_gallery_example_opens_and_runs() {
    let base = temp_dir("gallery");
    let examples = templates::examples();
    let names: Vec<&str> = examples.iter().map(|e| e.name.as_str()).collect();
    for required in ["collector_luau", "cinder_keep", "script_rules"] {
        assert!(
            names.contains(&required),
            "{required} missing from {names:?}"
        );
    }
    let gpu = gpu_tests_enabled();
    for example in &examples {
        assert!(example.preview.is_some(), "{} has no preview", example.name);
        let copy = base.join(&example.name);
        let project = templates::copy_example(example, &copy).unwrap();
        assert!(!copy.join(".box/editor.json").exists());
        let (ok, _, json, stderr) = run(&base, &["check", project.root().to_str().unwrap()]);
        assert!(ok, "{}: {json:#}\n{stderr}", example.name);
        assert_eq!(json["errors"], 0, "{}: {json:#}", example.name);
        if gpu && !runs_clean(&base, project.root(), None) {
            return;
        }
    }
}
