//! `box export --json`: template lookup through `BOX_TEMPLATES`, the JSON
//! report, and errors (no template, a template of another engine version,
//! bad flags). The template here is a copy of `box` itself (any Mach-O);
//! running exported games is tested in `apps/player/tests/export.rs`.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value as Json;

fn temp(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cli_export_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn example(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
}

fn box_export(templates: &Path, args: &[&str]) -> (bool, Json) {
    let out = Command::new(env!("CARGO_BIN_EXE_box"))
        .env("BOX_TEMPLATES", templates)
        .arg("export")
        .args(args)
        .arg("--json")
        .output()
        .unwrap();
    let json = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON ({e}): {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (out.status.success(), json)
}

#[test]
fn export_finds_templates_and_reports_json() {
    let dir = temp("json");
    let templates = dir.join("templates");
    std::fs::create_dir_all(templates.join("macos")).unwrap();
    std::fs::copy(
        env!("CARGO_BIN_EXE_box"),
        templates.join("macos/box-player"),
    )
    .unwrap();
    let project = example("script_demo");
    let out = dir.join("out");
    let p = project.to_str().unwrap();
    let o = out.to_str().unwrap();

    let (ok, json) = box_export(
        &templates,
        &["--project", p, "--target", "macos", "--out", o, "--no-sign"],
    );
    assert!(ok, "{json:#}");
    assert_eq!(json["ok"], true);
    assert_eq!(json["target"], "macos");
    assert_eq!(json["profile"], "release");
    assert_eq!(json["player_source"], "template");
    assert_eq!(json["bundle_id"], "com.example.script-demo");
    assert_eq!(json["version"], "1.0.0");
    assert_eq!(json["splash"], true);
    assert!(
        json["player"]
            .as_str()
            .unwrap()
            .ends_with("templates/macos/box-player")
    );
    assert!(json["cook"]["pak_bytes"].as_u64().unwrap() > 1000);
    assert!(
        json["warnings"][0].as_str().unwrap().contains("not signed"),
        "{json:#}"
    );
    assert!(
        out.join("Script Demo.app/Contents/Resources/game.pak")
            .is_file()
    );

    // A template of another engine version is refused.
    std::fs::write(templates.join("VERSION"), "0.0.1\n").unwrap();
    let (ok, json) = box_export(&templates, &["--project", p, "--out", o]);
    assert!(!ok);
    assert_eq!(json["error"]["code"], "no_template");
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("box-engine 0.0.1")
    );

    // No Windows template anywhere.
    let (ok, json) = box_export(
        &templates,
        &["--project", p, "--target", "windows", "--out", o],
    );
    assert!(!ok);
    assert_eq!(json["error"]["code"], "no_template");
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("built on Windows")
    );

    // Bad flags.
    let (ok, json) = box_export(
        &templates,
        &["--project", p, "--target", "linux", "--out", o],
    );
    assert!(!ok);
    assert_eq!(json["error"]["code"], "usage");
    let (ok, json) = box_export(
        &templates,
        &["--project", p, "--profile", "debug", "--out", o],
    );
    assert!(!ok);
    assert_eq!(json["error"]["code"], "usage");
    let _ = std::fs::remove_dir_all(&dir);
}
