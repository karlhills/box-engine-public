//! M9 export: the data examples exported for macOS (release and
//! development) with this `box-player` as the template, then run from a copy
//! of the `.app` with no project folder around; pak round trip, cook
//! determinism, corruption, the Windows layout with resources patched into a
//! sample PE, and an `open` launch. GPU runs are skipped with
//! `BOX_SKIP_GPU_TESTS`. Captures: `target/captures/export/`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use box_engine::Project;
use box_engine::box_assets::{AssetServer, Pak, vfs};
use box_engine::box_core::identity::Uid;
use box_engine::box_render::golden;
use box_engine::packed::{ExportManifest, MANIFEST_FILE, PAK_FILE, Profile};
use box_engine::player::{PlayerOptions, build_app};
use box_export::{ExportOptions, ExportReport, Target, cook, windows};

const EXAMPLES: [&str; 3] = ["collector_luau", "cinder_keep", "script_demo"];

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn example(name: &str) -> PathBuf {
    repo().join("examples").join(name)
}

fn temp(name: &str) -> PathBuf {
    let dir =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("export_{name}_{}", Uid::new()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn captures() -> PathBuf {
    let dir = repo().join("target/captures/export");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn skip_gpu() -> bool {
    let skip = std::env::var_os("BOX_SKIP_GPU_TESTS").is_some();
    if skip {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping the GPU part");
    }
    skip
}

fn export(
    project: &Path,
    target: Target,
    profile: Profile,
    out: &Path,
    player: &Path,
) -> ExportReport {
    let mut options = ExportOptions::new(project, target, profile, out);
    options.player = Some(player.to_path_buf());
    box_export::export(&options, &mut |_| {})
        .unwrap_or_else(|e| panic!("export {}: {e}", project.display()))
}

fn template() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_box-player"))
}

/// Runs an exported executable from `cwd` with a private HOME (its save
/// folder), headless for `frames` frames with a capture.
fn run_exported(exe: &Path, cwd: &Path, capture: &Path, frames: u32) -> Output {
    Command::new(exe)
        .current_dir(cwd)
        .env("HOME", cwd)
        .env_remove("RUST_LOG")
        .args([
            "--frames",
            &frames.to_string(),
            "--size",
            "640x360",
            "--capture",
        ])
        .arg(capture)
        .output()
        .unwrap()
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let p = e.path();
        if e.file_type().unwrap().is_dir() {
            copy_dir(&p, &to.join(e.file_name()));
        } else {
            std::fs::copy(&p, to.join(e.file_name())).unwrap();
        }
    }
}

#[test]
fn data_examples_export_for_macos_and_run_from_the_pak() {
    for name in EXAMPLES {
        let project = example(name);
        let mut folder_capture = None;
        for profile in [Profile::Release, Profile::Development] {
            let out = temp(&format!("{name}_{}", profile.name()));
            let report = export(&project, Target::MacOs, profile, &out, &template());
            assert!(
                report.output.ends_with(format!("{}.app", report.name)),
                "{report:?}"
            );
            assert!(report.signed, "ad-hoc signed");
            let pak = Pak::open(&report.pak).unwrap();
            pak.verify().unwrap();
            let manifest: ExportManifest =
                serde_json::from_slice(&pak.read(MANIFEST_FILE).unwrap()).unwrap();
            assert_eq!(manifest.profile, profile);
            assert_eq!(manifest.target, "macos");
            // Only game files: no editor state, authoring or docs.
            for e in pak.entries() {
                assert!(
                    e.path == "box.project.json"
                        || e.path == MANIFEST_FILE
                        || e.path.starts_with("assets/"),
                    "{name}: {} packed",
                    e.path
                );
                assert!(!e.path.split('/').any(|p| p.starts_with('.')), "{}", e.path);
            }
            let plist = std::fs::read_to_string(report.output.join("Contents/Info.plist")).unwrap();
            assert!(plist.contains(&format!("<string>{}</string>", manifest.bundle_id)));
            assert!(
                report
                    .output
                    .join("Contents/Resources/AppIcon.icns")
                    .is_file()
            );
            let verify = Command::new("codesign")
                .args(["--verify", "--strict"])
                .arg(&report.output)
                .output()
                .unwrap();
            assert!(
                verify.status.success(),
                "{}",
                String::from_utf8_lossy(&verify.stderr)
            );
            if skip_gpu() {
                continue;
            }
            // Run a copy far from the project: the pak is all it has.
            let elsewhere = temp(&format!("{name}_{}_run", profile.name()));
            let app = elsewhere.join(report.output.file_name().unwrap());
            copy_dir(&report.output, &app);
            let exe = app
                .join("Contents/MacOS")
                .join(report.executable.file_name().unwrap());
            let shot = captures().join(format!("{name}_{}.png", profile.name()));
            let run = run_exported(&exe, &elsewhere, &shot, 20);
            let log = String::from_utf8_lossy(&run.stderr);
            assert!(run.status.success(), "{name} {}: {log}", profile.name());
            assert!(
                log.contains(&format!("({} build)", profile.name())),
                "{log}"
            );
            assert!(log.contains("Resources/game.pak"), "{log}");
            assert!(!log.contains("validation error"), "{log}");
            assert!(!log.contains(" ERROR "), "{name}: {log}");
            // No warnings either: the pak is read like a folder (a format
            // check reading the disk once warned about every export).
            assert!(!log.contains(" WARN "), "{name}: {log}");
            // Same frames from the project folder: the same picture.
            let actual = image::open(&shot).unwrap().to_rgba8();
            let expected = folder_capture.get_or_insert_with(|| {
                let path = captures().join(format!("{name}_folder.png"));
                let run = Command::new(template())
                    .arg(&project)
                    .current_dir(&elsewhere)
                    .env("HOME", &elsewhere)
                    .args(["--frames", "20", "--size", "640x360", "--capture"])
                    .arg(&path)
                    .output()
                    .unwrap();
                assert!(
                    run.status.success(),
                    "{}",
                    String::from_utf8_lossy(&run.stderr)
                );
                image::open(&path).unwrap().to_rgba8()
            });
            golden::compare(&actual, expected, &golden::DEFAULT_TOLERANCE).unwrap_or_else(|e| {
                panic!(
                    "{name} {}: pak run differs from folder run: {e}",
                    profile.name()
                )
            });
            std::fs::remove_dir_all(&elsewhere).unwrap();
            std::fs::remove_dir_all(&out).unwrap();
        }
    }
}

#[test]
fn cooking_is_deterministic_and_round_trips() {
    let project = Project::open(example("script_demo")).unwrap();
    let manifest = ExportManifest {
        format: 1,
        name: "Script Demo".into(),
        bundle_id: "com.example.script-demo".into(),
        version: "1.0.0".into(),
        profile: Profile::Release,
        engine_version: "0.1.0".into(),
        target: "macos".into(),
    };
    let dir = temp("cook");
    let (a, b) = (dir.join("a.pak"), dir.join("b.pak"));
    let first = cook::cook(&project, &manifest, &a, &mut |_| {}).unwrap();
    let second = cook::cook(&project, &manifest, &b, &mut |_| {}).unwrap();
    assert_eq!(
        std::fs::read(&a).unwrap(),
        std::fs::read(&b).unwrap(),
        "same bytes"
    );
    assert_eq!(first.pak_hash, second.pak_hash);
    let pak = Pak::open(&a).unwrap();
    let files = cook::packed_files(&project).unwrap();
    assert_eq!(pak.entries().len(), files.len() + 1, "files + manifest");
    for rel in &files {
        assert_eq!(
            pak.read(rel).unwrap(),
            std::fs::read(project.root().join(rel)).unwrap(),
            "{rel}"
        );
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_mounted_pak_plays_read_only_without_hot_reload() {
    let out = temp("readonly");
    let report = export(
        &example("collector_luau"),
        Target::MacOs,
        Profile::Release,
        &out,
        &template(),
    );
    // A windowed, watching run as far as options go: still no watcher.
    let options = PlayerOptions {
        project: Some(report.pak.clone()),
        watch: true,
        ..Default::default()
    };
    let (app, project) = build_app(&box_engine::Game::new(), &options).unwrap();
    assert!(project.is_packed());
    assert_eq!(project.name(), "Collector (Luau)");
    let server = app.world.resource::<AssetServer>();
    assert!(server.is_read_only());
    assert!(!server.is_watching(), "no hot reload from a pak");
    assert!(server.files().iter().any(|(_, p)| p.ends_with(".luau")));
    drop(app);
    vfs::unmount(project.root());
    std::fs::remove_dir_all(&out).unwrap();
}

#[test]
fn a_damaged_pak_is_reported() {
    let out = temp("damaged");
    let report = export(
        &example("script_demo"),
        Target::MacOs,
        Profile::Release,
        &out,
        &template(),
    );
    let pak = Pak::open(&report.pak).unwrap();
    let scene = pak
        .entries()
        .iter()
        .find(|e| e.path.ends_with(".scene.json"))
        .unwrap()
        .clone();
    drop(pak);
    let good = std::fs::read(&report.pak).unwrap();
    let exe = &report.executable;
    // No project folder to serve a control socket from.
    let run = Command::new(exe)
        .current_dir(&out)
        .env("HOME", &out)
        .args(["--headless", "--control"])
        .output()
        .unwrap();
    assert!(!run.status.success());
    assert!(
        String::from_utf8_lossy(&run.stderr)
            .contains("--control is not available in an exported game")
    );
    // A flipped byte in a scene: that file fails its hash when read.
    let mut bytes = good.clone();
    bytes[scene.offset as usize + 1] ^= 0x20;
    std::fs::write(&report.pak, &bytes).unwrap();
    let run = Command::new(exe)
        .current_dir(&out)
        .env("HOME", &out)
        .args(["--headless", "--frames", "2"])
        .output()
        .unwrap();
    let log = String::from_utf8_lossy(&run.stderr);
    assert!(log.contains("content hash mismatch"), "{log}");
    // A flipped byte in the index: refused at startup.
    let mut bytes = good.clone();
    let n = bytes.len();
    bytes[n - 2] ^= 1;
    std::fs::write(&report.pak, &bytes).unwrap();
    let run = Command::new(exe)
        .current_dir(&out)
        .env("HOME", &out)
        .args(["--headless", "--frames", "2"])
        .output()
        .unwrap();
    let log = String::from_utf8_lossy(&run.stderr);
    assert!(!run.status.success());
    assert!(log.contains("damaged pak (index hash mismatch)"), "{log}");
    std::fs::remove_dir_all(&out).unwrap();
}

#[test]
fn windows_bundle_layout_and_resources() {
    // A tiny real PE (spikes/s7_export/pe_sample) stands in for the Windows
    // player template, which is built on Windows.
    let pe = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/box_export/tests/fixtures/tiny.exe");
    for (profile, subsystem) in [
        (Profile::Release, windows::SUBSYSTEM_GUI),
        (Profile::Development, windows::SUBSYSTEM_CONSOLE),
    ] {
        let out = temp("windows");
        let report = export(&example("cinder_keep"), Target::Windows, profile, &out, &pe);
        assert_eq!(report.output, out.join("Cinder Keep"));
        assert_eq!(report.executable, out.join("Cinder Keep/Cinder Keep.exe"));
        assert_eq!(report.pak, out.join("Cinder Keep").join(PAK_FILE));
        assert!(!report.signed);
        let names: Vec<String> = std::fs::read_dir(&report.output)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
        let summary = windows::read_back(&report.executable).unwrap();
        assert_eq!(summary.subsystem, subsystem);
        assert!(summary.icon_bytes.unwrap() > 1000);
        assert_eq!(summary.product_name.as_deref(), Some("Cinder Keep"));
        assert_eq!(summary.file_version.as_deref(), Some("1.0.0"));
        let pak = Pak::open(&report.pak).unwrap();
        pak.verify().unwrap();
        let manifest: ExportManifest =
            serde_json::from_slice(&pak.read(MANIFEST_FILE).unwrap()).unwrap();
        assert_eq!(manifest.target, "windows");
        // Exporting again replaces the earlier export.
        let again = export(&example("cinder_keep"), Target::Windows, profile, &out, &pe);
        assert_eq!(again.cook.pak_hash, report.cook.pak_hash);
        std::fs::remove_dir_all(&out).unwrap();
    }
    // A Mach-O is not a Windows template, and a stranger's folder is kept.
    let out = temp("windows_bad");
    let mut options = ExportOptions::new(
        example("script_demo"),
        Target::Windows,
        Profile::Release,
        &out,
    );
    options.player = Some(template());
    let err = box_export::export(&options, &mut |_| {}).unwrap_err();
    assert_eq!(err.code(), "no_template", "{err}");
    std::fs::create_dir_all(out.join("Script Demo")).unwrap();
    std::fs::write(out.join("Script Demo/notes.txt"), "mine").unwrap();
    options.player = Some(pe);
    let err = box_export::export(&options, &mut |_| {}).unwrap_err();
    assert_eq!(err.code(), "output_exists", "{err}");
    assert!(out.join("Script Demo/notes.txt").is_file());
    std::fs::remove_dir_all(&out).unwrap();
}

/// LaunchServices (`open`, as Finder) starts the exported app; it runs
/// headless and writes a report.
#[cfg(target_os = "macos")]
#[test]
fn open_launches_the_exported_app() {
    if skip_gpu() {
        return;
    }
    // In the system temp folder: an app started by LaunchServices has its
    // own privacy permissions, and files on other volumes would make macOS
    // ask the user for access.
    let out = std::env::temp_dir().join(format!("box_export_open_{}", Uid::new()));
    std::fs::create_dir_all(&out).unwrap();
    let report = export(
        &example("script_demo"),
        Target::MacOs,
        Profile::Release,
        &out,
        &template(),
    );
    let json = out.join("report.json");
    let mut child = Command::new("open")
        .args(["-W", "-n", "--env"])
        .arg(format!("HOME={}", out.display()))
        .arg(&report.output)
        .args(["--args", "--headless", "--frames", "5", "--report"])
        .arg(&json)
        .spawn()
        .unwrap();
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if start.elapsed() > Duration::from_secs(120) {
            let _ = child.kill();
            panic!("open did not return");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let text = std::fs::read_to_string(&json).expect("the app ran and wrote its report");
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["frames"], 5);
    assert_eq!(value["title"], "Script Demo");
    std::fs::remove_dir_all(&out).unwrap();
}

/// A native project (examples/collector) exports with its own player, built
/// by `cargo build --release`; the bundle plays the pak with the game code.
#[test]
#[ignore = "builds the collector player in release mode (minutes on a clean target dir); run with --ignored"]
fn native_project_exports_through_cargo() {
    let out = temp("native");
    let options = ExportOptions::new(example("collector"), Target::MacOs, Profile::Release, &out);
    let report = box_export::export(&options, &mut |_| {}).unwrap();
    assert_eq!(report.player_source, "cargo");
    assert!(
        report.player.ends_with("release/collector-player"),
        "{}",
        report.player.display()
    );
    assert!(report.signed);
    if skip_gpu() {
        return;
    }
    let elsewhere = temp("native_run");
    let app = elsewhere.join("Collector.app");
    copy_dir(&report.output, &app);
    let shot = captures().join("collector_native_release.png");
    let run = run_exported(&app.join("Contents/MacOS/Collector"), &elsewhere, &shot, 20);
    let log = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "{log}");
    assert!(
        log.contains("(release build)") && log.contains("Resources/game.pak"),
        "{log}"
    );
    std::fs::remove_dir_all(&elsewhere).unwrap();
    std::fs::remove_dir_all(&out).unwrap();
}
