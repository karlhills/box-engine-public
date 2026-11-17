//! UI assets from files: string tables (loading by language, fallback,
//! placeholders, the missing-key report, hot reload, a new table file),
//! theme files (applied, hot-reloaded, a bad edit keeps the last good one)
//! with font assets, and the M7 acceptance case: a menu navigated by
//! gamepad whose text comes from a string table.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use box_engine::box_ui::{Strings, Ui, UiEvent, UiEventKind, UiNode};
use box_engine::prelude::*;

fn temp_project(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "box_ui_assets_{name}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    for sub in ["strings", "ui", "fonts"] {
        std::fs::create_dir_all(d.join(sub)).unwrap();
    }
    d
}

fn write(dir: &Path, rel: &str, text: &str) {
    std::fs::write(dir.join(rel), text).unwrap();
}

fn app_with_assets(dir: &Path) -> App {
    let mut app = box_engine::default_app();
    *app.world.resource_mut::<box_engine::WindowSize>() = box_engine::WindowSize {
        width: 1920,
        height: 1080,
        scale_factor_milli: 1000,
    };
    let server = box_engine::asset_server(dir).unwrap();
    server.watch(true).unwrap();
    app.world.insert_resource(server);
    app
}

/// Runs frames until `done` or 10 s pass.
fn until(app: &mut App, mut done: impl FnMut(&mut App) -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        app.tick(1.0 / 60.0);
        app.world.resource_mut::<Input>().end_frame();
        if done(app) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

#[test]
fn string_tables_load_by_language_and_hot_reload() {
    let dir = temp_project("reload");
    write(
        &dir,
        "strings/en.strings.json",
        r#"{"greeting": "Hello {name}", "only_en": "English"}"#,
    );
    write(
        &dir,
        "strings/de.strings.json",
        r#"{"greeting": "Hallo {name}"}"#,
    );
    let mut app = app_with_assets(&dir);
    let label = app
        .world
        .resource_mut::<Ui>()
        .add(None, UiNode::label_key("greeting").with_arg("name", "Ada"));
    let only_en = app
        .world
        .resource_mut::<Ui>()
        .add(None, UiNode::label_key("only_en"));
    let text = |app: &mut App, n| app.world.resource::<Ui>().text(n).unwrap_or_default();

    assert!(
        until(&mut app, |a| text(a, label) == "Hello Ada"),
        "{}",
        text(&mut app, label)
    );
    app.world.resource_mut::<Strings>().set_language("de");
    assert!(until(&mut app, |a| text(a, label) == "Hallo Ada"));
    assert_eq!(text(&mut app, only_en), "English", "fallback language");

    // Hot reload of a table.
    write(
        &dir,
        "strings/de.strings.json",
        r#"{"greeting": "Servus {name}"}"#,
    );
    assert!(
        until(&mut app, |a| text(a, label) == "Servus Ada"),
        "reloaded: {}",
        text(&mut app, label)
    );
    // A new table file for the language joins the merge.
    write(
        &dir,
        "strings/extra.de.strings.json",
        r#"{"menu": {"bye": "Tschüss"}}"#,
    );
    assert!(until(&mut app, |a| a
        .world
        .resource::<Strings>()
        .has("menu.bye")));

    // Missing keys show the key and are reported once.
    let missing = app
        .world
        .resource_mut::<Ui>()
        .add(None, UiNode::label_key("menu.typo"));
    app.tick(1.0 / 60.0);
    assert_eq!(text(&mut app, missing), "menu.typo");
    assert_eq!(
        app.world.resource::<Strings>().missing_keys(),
        ["menu.typo"]
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn theme_files_apply_hot_reload_and_load_their_fonts() {
    let dir = temp_project("theme");
    let mono = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../box_editor/assets/fonts/SourceCodePro-Regular.ttf");
    std::fs::copy(mono, dir.join("fonts/mono.ttf")).unwrap();
    write(
        &dir,
        "ui/game.theme.json",
        r##"{"colors": {"accent": "#112233"}, "fonts": {"mono": "fonts/mono.ttf"},
            "styles": {"code": {"font": "mono"}}}"##,
    );
    let mut app = app_with_assets(&dir);
    app.world
        .resource_mut::<Ui>()
        .set_theme("ui/game.theme.json");
    let body = app.world.resource_mut::<Ui>().add(
        None,
        UiNode::text("iiiiiiii").anchored([0.0, 0.0], [0.0, 0.0]),
    );
    let code = app.world.resource_mut::<Ui>().add(
        None,
        UiNode::text("iiiiiiii")
            .with_class("code")
            .anchored([0.0, 0.0], [0.0, 100.0]),
    );
    let accent = |app: &App| {
        app.world
            .resource::<Ui>()
            .theme()
            .colors
            .get("accent")
            .cloned()
    };
    assert!(until(&mut app, |a| accent(a).as_deref() == Some("#112233")));
    // The monospace font makes the i's as wide as any letter.
    let width = |app: &mut App, n| app.world.resource::<Ui>().rect(n).map_or(0.0, |r| r[2]);
    assert!(
        until(&mut app, |a| width(a, code) > width(a, body) * 1.5),
        "mono {} vs body {}",
        width(&mut app, code),
        width(&mut app, body)
    );
    // Hot reload.
    write(
        &dir,
        "ui/game.theme.json",
        r##"{"colors": {"accent": "#445566"}}"##,
    );
    assert!(until(&mut app, |a| accent(a).as_deref() == Some("#445566")));
    // A broken edit keeps the last good theme.
    write(&dir, "ui/game.theme.json", r##"{"colours": {}}"##);
    for _ in 0..30 {
        app.tick(1.0 / 60.0);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(accent(&app).as_deref(), Some("#445566"));
    let _ = std::fs::remove_dir_all(dir);
}

/// M7 acceptance: "A menu can be navigated by gamepad, and its text comes
/// from a string table."
#[test]
fn a_menu_navigated_by_gamepad_with_text_from_a_string_table() {
    let dir = temp_project("acceptance");
    write(
        &dir,
        "strings/en.strings.json",
        r#"{"menu": {"start": "Start", "options": "Options"}}"#,
    );
    write(
        &dir,
        "strings/de.strings.json",
        r#"{"menu": {"start": "Starten", "options": "Optionen"}}"#,
    );
    let mut app = app_with_assets(&dir);
    app.world.resource_mut::<Strings>().set_language("de");
    let menu = app.world.resource_mut::<Ui>().add(
        None,
        UiNode::panel()
            .modal()
            .anchored([0.5, 0.5], [0.0, 0.0])
            .children([
                UiNode::button("start", "menu.start"),
                UiNode::button("options", "menu.options"),
            ]),
    );
    let options = app
        .world
        .resource::<Ui>()
        .find(Some(menu), "options")
        .unwrap();
    assert!(until(&mut app, |a| {
        a.world.resource::<Ui>().text(options).as_deref() == Some("Optionen")
    }));
    let pad = GamepadId(0);
    app.world
        .resource_mut::<Input>()
        .connect_gamepad(pad, "pad");
    let mut cursor = EventCursor::<UiEvent>::default();
    let mut clicked = Vec::new();
    for b in [GamepadButton::DPadDown, GamepadButton::South] {
        for down in [true, false] {
            let input = app.world.resource_mut::<Input>();
            if down {
                input.press_gamepad_button(pad, b);
            } else {
                input.release_gamepad_button(pad, b);
            }
            app.tick(1.0 / 60.0);
            app.world.resource_mut::<Input>().end_frame();
            clicked.extend(
                cursor
                    .read(app.world.resource::<Events<UiEvent>>())
                    .filter(|e| e.kind == UiEventKind::Click)
                    .filter_map(|e| e.id.clone()),
            );
        }
    }
    assert_eq!(clicked, ["options"]);
    assert_eq!(app.world.resource::<Ui>().focused(), Some(options));
    let _ = std::fs::remove_dir_all(dir);
}
