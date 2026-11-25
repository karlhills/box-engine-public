//! The Script pane: `.luau` files opened from the Assets pane (double-click)
//! or at a line (a Console error), edited and saved; files changed on disk
//! reload (or are flagged while edited); log errors mark their lines; a
//! dirty script joins the unsaved-changes prompt when quitting.
//!
//! Capture: `target/captures/script_editor/pane.png` (a script with an
//! error line, the find bar and the Script tab in front).

use std::path::PathBuf;
use std::time::Duration;

use box_editor::assets::AssetItem;
use box_editor::headless::{CaptureOptions, Headless};
use box_editor::log::LogBuffer;
use box_editor::{EditorOptions, UnsavedChoice, build_editor, layout};

const SCRIPT: &str = "-- Spins the entity it is on.\nlocal Spin = {}\n\nSpin.properties = {\n    speed = { type = \"number\", default = 2 },\n}\n\nfunction Spin.on_update(self, dt: number)\n    self.entity:rotate_y(self.props.speed * dt)\nend\n\nreturn Spin\n";

fn project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("box_script_editor_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    box_engine::Project::create(&dir, name).unwrap();
    std::fs::create_dir_all(dir.join("assets/scripts")).unwrap();
    std::fs::write(dir.join("assets/scripts/spin.luau"), SCRIPT).unwrap();
    dir
}

#[test]
fn scripts_open_edit_save_reload_and_ask_before_quitting() {
    let dir = project("edit");
    let mut editor = build_editor(
        &EditorOptions {
            project: Some(dir.clone()),
            ..Default::default()
        },
        LogBuffer::default(),
    )
    .unwrap();
    editor.tree = layout::default_layout();
    let file = dir.join("assets/scripts/spin.luau");

    // Double-click in the Assets pane opens it in the Script pane.
    let server = editor.state.asset_server().cloned().expect("asset server");
    let id = server.id_of("scripts/spin.luau").expect("indexed");
    editor.state.open_asset(&AssetItem::file(id));
    let tabs = &editor.state.script_editor().tabs;
    assert_eq!(tabs.len(), 1);
    assert_eq!(tabs[0].path, "scripts/spin.luau");
    assert_eq!(tabs[0].text, SCRIPT);
    // Opening it again (a Console line, at a line) reuses the tab.
    editor
        .state
        .open_script("assets/scripts/spin.luau", Some(9))
        .unwrap();
    assert_eq!(editor.state.script_editor().tabs.len(), 1);

    // Edit and save: written to disk, no longer dirty.
    let edited = SCRIPT.replace("default = 2", "default = 5");
    editor.state.script_editor_mut().tabs[0].text = edited.clone();
    assert_eq!(editor.state.dirty_scripts(), ["scripts/spin.luau"]);
    editor.state.save_script(0).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), edited);
    assert!(editor.state.dirty_scripts().is_empty());

    // Changed on disk without edits: reloads.
    std::thread::sleep(Duration::from_millis(1100));
    let outside = edited.replace("Spins", "Turns");
    std::fs::write(&file, &outside).unwrap();
    editor.state.check_scripts_on_disk();
    assert_eq!(editor.state.script_editor().tabs[0].text, outside);
    // ... with edits: flagged, the edits kept.
    editor.state.script_editor_mut().tabs[0]
        .text
        .push_str("-- mine\n");
    std::thread::sleep(Duration::from_millis(1100));
    std::fs::write(&file, SCRIPT).unwrap();
    editor.state.check_scripts_on_disk();
    let tab = &editor.state.script_editor().tabs[0];
    assert!(tab.changed_on_disk);
    assert!(tab.text.ends_with("-- mine\n"));

    // A log error naming a line marks it.
    editor.state.console.log.push(
        tracing::Level::ERROR,
        "box_script",
        "scripts/spin.luau:9: attempt to call a nil value (method 'rotate_y')".into(),
    );
    let problems = editor.state.script_problems("scripts/spin.luau");
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].0, 9);

    // Capture: the pane with the error line and the find bar.
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_none() {
        editor.state.script_editor_mut().tabs[0].changed_on_disk = false;
        let options = CaptureOptions {
            size: [1440, 900],
            pixels_per_point: 1.0,
            frames: 1,
        };
        match Headless::new(&mut editor, &options) {
            Ok(mut h) => {
                editor
                    .state
                    .open_script("scripts/spin.luau", Some(9))
                    .unwrap();
                editor.state.script_editor_mut().find = Some("speed".into());
                for _ in 0..4 {
                    h.frame(&mut editor, Vec::new());
                }
                let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../target/captures/script_editor");
                std::fs::create_dir_all(&out).unwrap();
                h.image().unwrap().save(out.join("pane.png")).unwrap();
            }
            Err(e) => eprintln!("capture skipped (no GPU): {e:#}"),
        }
    }

    // Quitting with an edited script asks; Save writes it, then quits.
    assert!(!editor.request_quit(), "the unsaved prompt shows");
    assert!(editor.dialog().is_some());
    editor.answer_unsaved(UnsavedChoice::Save);
    assert!(editor.dialog().is_none());
    assert!(
        std::fs::read_to_string(&file)
            .unwrap()
            .ends_with("-- mine\n")
    );
    assert!(editor.state.dirty_scripts().is_empty());
    let _ = std::fs::remove_dir_all(dir);
}
