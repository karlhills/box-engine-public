//! Error reporting in the editor: the "crashed last time" notice, Copy and
//! Show File on error toasts, actionable startup and save errors. Captures
//! go to `target/captures/errors/` (skipped without a GPU or with
//! `BOX_SKIP_GPU_TESTS`).
//!
//! The crash notice reads `BOX_CRASH_DIR`, which a test cannot set in its
//! own process (`set_var` is unsafe): that test runs again as a child
//! process with the variable set.

use std::path::{Path, PathBuf};
use std::process::Command;

use box_editor::log::{LogBuffer, Severity};
use box_editor::{Dialog, Editor, EditorOptions, build_editor};
use egui::{Event, PointerButton, Pos2, Rect, vec2};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("box_editor_errors_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// What one UI frame produced: text it copied and its shapes.
struct Frame {
    copied: Vec<String>,
    shapes: Vec<egui::epaint::ClippedShape>,
}

/// Runs the editor UI one frame (no rendering).
fn frame(ctx: &egui::Context, editor: &mut Editor, time: f64, events: Vec<Event>) -> Frame {
    let raw = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1440.0, 900.0))),
        time: Some(time),
        predicted_dt: 1.0 / 60.0,
        focused: true,
        events,
        ..Default::default()
    };
    let out = ctx.run_ui(raw, |ui| editor.ui(ui));
    let copied = out
        .platform_output
        .commands
        .into_iter()
        .filter_map(|c| match c {
            egui::OutputCommand::CopyText(t) => Some(t),
            _ => None,
        })
        .collect();
    Frame {
        copied,
        shapes: out.shapes,
    }
}

/// The centre of the first text shape reading `label`.
fn find_text(shapes: &[egui::epaint::ClippedShape], label: &str) -> Option<Pos2> {
    shapes.iter().find_map(|c| match &c.shape {
        egui::Shape::Text(t) if t.galley.text() == label => Some(t.visual_bounding_rect().center()),
        _ => None,
    })
}

/// Renders `editor` headlessly and writes `target/captures/errors/<name>.png`.
fn capture(editor: &mut Editor, name: &str) -> bool {
    use box_editor::headless::{CaptureOptions, Headless};
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        return false;
    }
    let options = CaptureOptions {
        size: [1440, 900],
        pixels_per_point: 1.0,
        // Enough for the modal fade-in to finish.
        frames: 30,
    };
    let mut h = match Headless::new(editor, &options) {
        Ok(h) => h,
        Err(e)
            if matches!(
                e.downcast_ref::<box_render::RenderError>(),
                Some(box_render::RenderError::NoAdapter)
            ) =>
        {
            eprintln!("no GPU adapter; skipping the {name} capture");
            return false;
        }
        Err(e) => panic!("{e:#}"),
    };
    for _ in 0..options.frames {
        h.frame(editor, Vec::new());
    }
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/errors");
    std::fs::create_dir_all(&out).unwrap();
    h.image()
        .unwrap()
        .save(out.join(format!("{name}.png")))
        .unwrap();
    true
}

const CHILD: &str = "BOX_EDITOR_ERRORS_CHILD";

/// Runs the test `name` of this binary again in a child process with
/// `BOX_CRASH_DIR=dir`; panics with its output if it failed.
fn run_as_child(name: &str, dir: &Path) {
    let out = Command::new(std::env::current_exe().unwrap())
        .args([name, "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .env(box_engine::crash::CRASH_DIR_ENV, dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "child {name} failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The editor crashed last time: the next start shows the notice (after a
/// dialog that was already open), Open/Dismiss forget it and keep the file.
#[test]
fn the_next_start_offers_the_last_crash_report() {
    if std::env::var_os(CHILD).is_none() {
        let dir = temp_dir("crash");
        let report = dir.join("box-editor-crash-20261116-120000-1.txt");
        std::fs::write(&report, "box-engine crash report (panic)\n").unwrap();
        std::fs::write(dir.join("editor.last-crash"), report.display().to_string()).unwrap();
        run_as_child("the_next_start_offers_the_last_crash_report", &dir);
        assert!(report.is_file(), "the report is kept");
        assert!(
            !dir.join("editor.last-crash").exists(),
            "the notice is dismissed"
        );
        return;
    }
    let dir = box_engine::crash::crash_dir().unwrap();
    let report = box_engine::crash::last_crash("editor").expect("the marker names the report");
    assert!(report.starts_with(&dir));

    let mut editor = build_editor(&EditorOptions::default(), LogBuffer::default()).unwrap();
    editor.offer_crash_report();
    assert!(
        matches!(editor.dialog(), Some(Dialog::CrashReport { report: r }) if *r == report),
        "{:?}",
        editor.dialog()
    );
    capture(&mut editor, "crash_prompt");

    // Escape dismisses it; it is not offered again.
    let ctx = egui::Context::default();
    editor.apply_theme(&ctx);
    frame(&ctx, &mut editor, 0.0, Vec::new());
    frame(
        &ctx,
        &mut editor,
        0.1,
        vec![Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }],
    );
    assert!(editor.dialog().is_none(), "{:?}", editor.dialog());
    assert_eq!(box_engine::crash::last_crash("editor"), None);

    // Behind another dialog, the notice waits until that one is closed.
    std::fs::write(dir.join("editor.last-crash"), report.display().to_string()).unwrap();
    let mut editor = build_editor(&EditorOptions::default(), LogBuffer::default()).unwrap();
    editor.save(); // a scratch scene without a file: Save Scene As opens
    assert!(matches!(editor.dialog(), Some(Dialog::SaveAs { .. })));
    editor.offer_crash_report();
    assert!(matches!(editor.dialog(), Some(Dialog::SaveAs { .. })));
    editor.answer_save_as(None);
    frame(&ctx, &mut editor, 0.2, Vec::new());
    assert!(matches!(editor.dialog(), Some(Dialog::CrashReport { .. })));
    // Open (headless: logs the path instead of opening a viewer).
    editor.answer_crash_report(true);
    assert!(editor.dialog().is_none());
    assert_eq!(box_engine::crash::last_crash("editor"), None);
}

/// Without a crash marker nothing is shown.
#[test]
fn no_crash_no_notice() {
    if std::env::var_os(CHILD).is_none() {
        let dir = temp_dir("no_crash");
        run_as_child("no_crash_no_notice", &dir);
        return;
    }
    let mut editor = build_editor(&EditorOptions::default(), LogBuffer::default()).unwrap();
    editor.offer_crash_report();
    assert!(editor.dialog().is_none());
}

fn project_editor(name: &str, log: LogBuffer) -> (Editor, PathBuf) {
    let dir = temp_dir(name);
    box_engine::Project::create(&dir, name).unwrap();
    let options = EditorOptions {
        project: Some(dir.clone()),
        ..Default::default()
    };
    (build_editor(&options, log).unwrap(), dir)
}

/// Warning and error toasts carry the whole message (Copy) and the file it
/// names when that file exists (Show File); infos have neither.
#[test]
fn error_toasts_copy_the_message_and_know_its_file() {
    let log = LogBuffer::default();
    let (mut editor, dir) = project_editor("toast_file", log.clone());
    std::fs::create_dir_all(dir.join("assets/scripts")).unwrap();
    std::fs::write(dir.join("assets/scripts/door.luau"), "return {}\n").unwrap();
    let long = "scripts/door.luau:5: attempt to index nil with 'open'\n  in on_update of \"Door\" (scripts/door.luau); this entity's script is disabled until the file is reloaded";
    log.push(tracing::Level::ERROR, "box_script", long.into());
    log.push(
        tracing::Level::WARN,
        "box_engine::asset_glue",
        "mesh models/gone.glb#Mesh/A: no such file in assets/".into(),
    );
    editor.state.poll_log();
    let toasts = editor.state.toasts().items();
    assert_eq!(toasts.len(), 2);
    let script = &toasts[0];
    assert_eq!(script.severity, Severity::Error);
    assert_eq!(script.full, long, "Copy copies every line");
    assert_eq!(
        script.file.as_deref(),
        Some(dir.join("assets/scripts/door.luau").as_path()),
        "found relative to assets/, past the :line suffix"
    );
    assert_eq!(toasts[1].file, None, "a missing file has no Show File");

    // Clicking Copy copies the message.
    let ctx = egui::Context::default();
    editor.apply_theme(&ctx);
    // The first frame lays out the dock; toasts anchor to the viewport.
    frame(&ctx, &mut editor, 0.0, Vec::new());
    let first = frame(&ctx, &mut editor, 0.05, Vec::new());
    let copy = find_text(&first.shapes, "Copy").expect("a Copy button");
    // A script error's file button opens the line in the Script pane.
    assert!(find_text(&first.shapes, "Edit Script").is_some());
    let press = |pressed| Event::PointerButton {
        pos: copy,
        button: PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    frame(&ctx, &mut editor, 0.1, vec![Event::PointerMoved(copy)]);
    frame(&ctx, &mut editor, 0.2, vec![press(true)]);
    let mut copied = frame(&ctx, &mut editor, 0.3, vec![press(false)]).copied;
    copied.extend(frame(&ctx, &mut editor, 0.4, Vec::new()).copied);
    assert!(copied.iter().any(|t| t == long), "{copied:?}");
    assert!(
        editor
            .state
            .toasts()
            .items()
            .iter()
            .any(|t| t.text == "Copied the message")
    );
    // Where it is now (the "Copied" notice moved the toasts).
    let now = frame(&ctx, &mut editor, 0.45, Vec::new());
    let edit = find_text(&now.shapes, "Edit Script").expect("an Edit Script button");
    let click = |pressed| Event::PointerButton {
        pos: edit,
        button: PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    frame(&ctx, &mut editor, 0.5, vec![Event::PointerMoved(edit)]);
    frame(&ctx, &mut editor, 0.6, vec![click(true)]);
    frame(&ctx, &mut editor, 0.7, vec![click(false)]);
    let tabs = &editor.state.script_editor().tabs;
    assert_eq!(tabs.len(), 1, "the script opened");
    assert_eq!(tabs[0].path, "scripts/door.luau");
}

/// Toasts with actions over the viewport, for review.
#[test]
fn error_toast_capture() {
    let log = LogBuffer::default();
    let (mut editor, dir) = project_editor("toast_capture", log.clone());
    std::fs::create_dir_all(dir.join("assets/shaders")).unwrap();
    std::fs::write(dir.join("assets/shaders/water.wgsl"), "fn surface() {}\n").unwrap();
    log.push(
        tracing::Level::ERROR,
        "box_render::material_shader",
        "material shader shaders/water.wgsl failed to compile; drawing the error material\nshaders/water.wgsl:4: unknown identifier `undefined_thing`".into(),
    );
    log.push(
        tracing::Level::ERROR,
        "box_engine::asset_glue",
        "mesh models/tower.glb#Mesh/Roof: no such file in assets/ (moved, renamed or deleted?); fix the reference in the inspector or put the file back; drawing the placeholder".into(),
    );
    log.push(
        tracing::Level::WARN,
        "box_engine::save",
        "save slot `slot1`: /Users/me/game/.box/saves/slot1.json.tmp: Permission denied (os error 13); no permission to write here: check the file's and its folder's permissions (or whether another program locks it), or choose another location)".into(),
    );
    if capture(&mut editor, "toasts") {
        let toasts = editor.state.toasts().items();
        assert_eq!(toasts.len(), 3);
        assert!(toasts[0].file.is_some(), "the shader file exists");
    }
}

/// Saving into a read-only folder says what to do about it.
#[cfg(unix)]
#[test]
fn a_read_only_scene_file_says_what_to_do() {
    use std::os::unix::fs::PermissionsExt;
    let (mut editor, dir) = project_editor("read_only", LogBuffer::default());
    let scenes = dir.join("assets/scenes");
    let mut perms = std::fs::metadata(&scenes).unwrap().permissions();
    perms.set_mode(0o555);
    std::fs::set_permissions(&scenes, perms.clone()).unwrap();
    editor
        .state
        .add_entity(box_editor::NewEntity::Cube, None)
        .unwrap();
    editor.save();
    perms.set_mode(0o755);
    std::fs::set_permissions(&scenes, perms).unwrap();
    match editor.dialog() {
        Some(Dialog::Error { title, message }) => {
            assert_eq!(title, "Could not save the scene");
            assert!(message.contains("Permission denied"), "{message}");
            assert!(
                message.contains("check the file's and its folder's permissions"),
                "{message}"
            );
        }
        other => panic!("{other:?}"),
    }
}

/// A project whose start scene is missing or malformed opens with a message
/// that names the file and says what to do (no repeated causes).
#[test]
fn start_scene_errors_name_the_file_and_the_fix() {
    let dir = temp_dir("start_scene");
    let project = box_engine::Project::create(&dir, "start").unwrap();
    let start = project.start_scene_path();
    let text = std::fs::read_to_string(&start).unwrap();
    std::fs::remove_file(&start).unwrap();
    let options = EditorOptions {
        project: Some(dir.clone()),
        ..Default::default()
    };
    let editor = build_editor(&options, LogBuffer::default()).unwrap();
    match editor.dialog() {
        Some(Dialog::Error { title, message }) => {
            assert_eq!(title, "Could not load the start scene");
            assert!(
                message.starts_with(
                    "The start scene `assets/scenes/main.scene.json` does not exist (set by `start_scene` in "
                ),
                "{message}"
            );
            assert!(
                message.contains("Save it as assets/scenes/main.scene.json (File ▸ Save Scene)"),
                "{message}"
            );
        }
        other => panic!("{other:?}"),
    }

    std::fs::write(
        &start,
        text.replacen("\"version\": 1,", "\"version\": 1,,", 1),
    )
    .unwrap();
    let editor = build_editor(&options, LogBuffer::default()).unwrap();
    match editor.dialog() {
        Some(Dialog::Error { message, .. }) => {
            let cause = message.split("\n\n").next().unwrap();
            assert!(cause.ends_with("at line 2 column 16"), "{cause}");
            assert_eq!(
                cause.matches("line 2 column 16").count(),
                1,
                "the parse error once: {cause}"
            );
        }
        other => panic!("{other:?}"),
    }
}
