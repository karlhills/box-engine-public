//! File ▸ Export…: settings saved into box.project.json, validation shown
//! per field, an export run in the background ending in a toast, failures as
//! error toasts; a capture of the window (`target/captures/export/`).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use box_editor::headless::{CaptureOptions, capture};
use box_editor::log::{LogBuffer, Severity};
use box_editor::{Editor, EditorOptions, build_editor};
use box_engine::Project;
use box_engine::packed::Profile;
use box_engine::project::ExportSettings;
use box_export::Target;
use tracing_subscriber::layer::SubscriberExt;

fn temp_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("editor_export_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn project_editor(dir: &Path, log: LogBuffer) -> Editor {
    Project::create(dir, "Export Me").unwrap();
    let options = EditorOptions {
        project: Some(dir.to_path_buf()),
        ..Default::default()
    };
    build_editor(&options, log).unwrap()
}

fn tiny_pe() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../box_export/tests/fixtures/tiny.exe")
}

fn wait(editor: &mut Editor) {
    let start = Instant::now();
    while editor.state.poll_export() {
        assert!(
            start.elapsed() < Duration::from_secs(120),
            "export did not finish"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    editor.state.poll_log();
}

#[test]
fn export_window_saves_settings_and_exports_in_the_background() {
    let dir = temp_dir("run");
    // Logged errors reach the editor's console (and become toasts).
    let log = LogBuffer::default();
    let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(log.layer()));
    let mut editor = project_editor(&dir, log);
    let state = &mut editor.state;
    assert!(!state.export_open());
    state.run_action("file.export").unwrap();
    assert!(state.export_open());

    // Invalid fields are reported per field and block saving.
    state.set_export_fields(&ExportSettings {
        app_name: Some("Exported Game".into()),
        bundle_id: Some("not a bundle id".into()),
        version: Some("1.x".into()),
        icon: None,
        splash: None,
    });
    let fields: Vec<&str> = state
        .export_settings_problems()
        .iter()
        .map(|p| p.0)
        .collect();
    assert_eq!(fields, ["bundle_id", "version"]);
    assert!(state.save_export_settings().is_err());
    let settings = ExportSettings {
        app_name: Some("Exported Game".into()),
        bundle_id: Some("com.example.exported".into()),
        version: Some("2.1.0".into()),
        icon: None,
        // The splash check box, turned off: written as `"splash": false`.
        splash: Some(false),
    };
    state.set_export_fields(&settings);
    assert_eq!(state.export_settings_edited(), settings);
    assert!(state.export_settings_problems().is_empty());

    // Export (Windows layout with a sample PE: no GPU, no template needed).
    let mut options = state.export_options().unwrap();
    assert!(options.out.ends_with("export"), "{}", options.out.display());
    assert_eq!(options.target, Target::host());
    options.target = Target::Windows;
    options.profile = Profile::Release;
    options.out = dir.join("out");
    options.player = Some(tiny_pe());
    state.start_export(options.clone()).unwrap();
    assert!(state.export_progress().is_some());
    assert!(state.start_export(options).is_err(), "one at a time");
    wait(&mut editor);
    let state = &mut editor.state;
    assert!(state.export_progress().is_none());
    // Saved before exporting.
    let reopened = Project::open(&dir).unwrap();
    assert_eq!(reopened.file().export, settings);
    let exe = dir.join("out/Exported Game/Exported Game.exe");
    assert!(exe.is_file());
    let toasts = state.toasts().items();
    assert!(
        toasts
            .iter()
            .any(|t| t.severity == Severity::Info && t.text.starts_with("Exported ")),
        "{toasts:?}"
    );

    // A failure (a Mach-O is no Windows player) is an error toast.
    let mut bad = state.export_options().unwrap();
    bad.target = Target::Windows;
    bad.out = dir.join("out2");
    bad.player = Some(std::env::current_exe().unwrap());
    state.start_export(bad).unwrap();
    wait(&mut editor);
    let toasts = editor.state.toasts().items();
    assert!(
        toasts
            .iter()
            .any(|t| t.severity == Severity::Error && t.text.contains("export failed")),
        "{toasts:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn export_window_capture() {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping");
        return;
    }
    let dir = temp_dir("capture");
    let mut editor = project_editor(&dir, LogBuffer::default());
    editor.state.open_export();
    editor.state.set_export_fields(&ExportSettings {
        app_name: Some("Export Me".into()),
        bundle_id: Some("com.example.export me".into()),
        version: None,
        icon: None,
        splash: None,
    });
    let image = match capture(&mut editor, &CaptureOptions::default()) {
        Ok(image) => image,
        Err(e) => {
            eprintln!("no GPU ({e}); skipping");
            return;
        }
    };
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/export");
    std::fs::create_dir_all(&out).unwrap();
    image.save(out.join("editor_export.png")).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
