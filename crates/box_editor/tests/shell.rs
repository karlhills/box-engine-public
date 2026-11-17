//! Discoverability and fast iteration across sessions (docs/DX_REVIEW.md,
//! Batch 2): the command palette, recent projects and the Welcome card,
//! what a launch without a project opens, session restore, registration
//! errors at startup and Edit ▸ History.
//!
//! Headless and driven by egui events where the UI matters; widgets are
//! found by their text in egui's accessibility tree. No test reads the
//! user's config folder: each passes its own `config_dir`. The last test
//! writes captures and needs a GPU.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use box_core::identity::{Name, Uid};
use box_core::prelude::Transform;
use box_editor::actions::Action;
use box_editor::layout::Pane;
use box_editor::log::LogBuffer;
use box_editor::start::{self, Launch, MAX_RECENT, RecentProjects};
use box_editor::{Dialog, DocAction, Editor, EditorOptions, NewEntity, build_editor};
use egui::accesskit::{Role, TreeUpdate};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use glam::Vec3;

// ----- an egui driver that reads the accessibility tree -----

#[derive(Clone, Debug)]
struct Node {
    role: Role,
    text: String,
    rect: Rect,
}

fn nodes_of(update: Option<&TreeUpdate>) -> Vec<Node> {
    let Some(u) = update else {
        return Vec::new();
    };
    u.nodes
        .iter()
        .filter_map(|(_, n)| {
            let b = n.bounds()?;
            Some(Node {
                role: n.role(),
                text: n.label().or(n.value()).unwrap_or_default().to_string(),
                rect: Rect::from_min_max(
                    pos2(b.x0 as f32, b.y0 as f32),
                    pos2(b.x1 as f32, b.y1 as f32),
                ),
            })
        })
        .collect()
}

struct Driver {
    ctx: egui::Context,
    time: f64,
    nodes: Vec<Node>,
}

impl Driver {
    fn new(editor: &Editor) -> Self {
        let ctx = egui::Context::default();
        editor.apply_theme(&ctx);
        ctx.enable_accesskit();
        Self {
            ctx,
            time: 0.0,
            nodes: Vec::new(),
        }
    }

    fn frame(&mut self, editor: &mut Editor, events: Vec<Event>) {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0))),
            time: Some(self.time),
            predicted_dt: 1.0 / 60.0,
            focused: true,
            events,
            ..Default::default()
        };
        let out = self.ctx.run_ui(raw, |ui| editor.ui(ui));
        self.nodes = nodes_of(out.platform_output.accesskit_update.as_ref());
        self.time += 1.0 / 60.0;
    }

    fn frames(&mut self, editor: &mut Editor, n: usize) {
        for _ in 0..n {
            self.frame(editor, Vec::new());
        }
    }

    fn find(&self, text: &str) -> Option<Node> {
        self.nodes
            .iter()
            .rev()
            .find(|n| n.text == text && n.role != Role::TextInput)
            .cloned()
    }

    fn get(&self, text: &str) -> Node {
        self.find(text).unwrap_or_else(|| {
            let all: Vec<&str> = self.nodes.iter().map(|n| n.text.as_str()).collect();
            panic!("no widget `{text}` among {all:?}")
        })
    }

    /// Clicks the widget labelled `text`, or whose label starts with
    /// `text` and a space (a button with text on its right).
    fn click(&mut self, editor: &mut Editor, text: &str) {
        let pos = match self.find(text) {
            Some(n) => n.rect.center(),
            None => self
                .nodes
                .iter()
                .rev()
                .find(|n| n.text.starts_with(&format!("{text} ")))
                .unwrap_or_else(|| panic!("no widget `{text}`"))
                .rect
                .center(),
        };
        let b = |pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        self.frame(editor, vec![Event::PointerMoved(pos)]);
        self.frame(editor, vec![b(true)]);
        self.frame(editor, vec![b(false)]);
        self.frames(editor, 3);
    }

    fn key(&mut self, editor: &mut Editor, key: Key, modifiers: Modifiers) {
        let e = |pressed| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        };
        self.frame(
            editor,
            vec![Event::ModifiersChanged(modifiers), e(true), e(false)],
        );
        self.frame(editor, vec![Event::ModifiersChanged(Modifiers::NONE)]);
        self.frame(editor, Vec::new());
    }

    fn type_text(&mut self, editor: &mut Editor, text: &str) {
        self.frame(editor, vec![Event::Text(text.into())]);
        self.frame(editor, Vec::new());
    }
}

// ----- projects and editors -----

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("box_shell_{name}_{}", Uid::new()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn temp_project(name: &str) -> PathBuf {
    let dir = temp_dir(name).join(name);
    box_engine::Project::create(&dir, name).unwrap();
    dir.canonicalize().unwrap()
}

/// A scene file with a sun, a ground, a camera and a cube.
fn scene_bytes() -> Vec<u8> {
    box_core::scene::save(&box_engine::project::starter_scene()).unwrap()
}

fn editor_with(options: EditorOptions) -> Editor {
    let mut editor = build_editor(&options, LogBuffer::default()).unwrap();
    editor.tree = box_editor::layout::default_layout();
    editor
}

fn open(dir: &Path) -> Editor {
    editor_with(EditorOptions {
        project: Some(dir.to_path_buf()),
        ..Default::default()
    })
}

fn by_name(editor: &Editor, name: &str) -> Option<Uid> {
    editor
        .state
        .world
        .query::<(&Uid, &Name)>()
        .iter()
        .find(|(_, n)| n.0 == name)
        .map(|(u, _)| *u)
}

fn row_index(editor: &Editor, id: &str) -> usize {
    editor
        .state
        .palette_results()
        .iter()
        .position(|r| r.id == id)
        .unwrap_or_else(|| panic!("no palette row `{id}`"))
}

// ----- E5b/W33: command palette -----

#[test]
fn palette_finds_actions_fuzzily_and_runs_them() {
    let dir = temp_project("palette");
    let mut ed = open(&dir);
    let before = ed.state.world.entity_count();

    ed.state.open_palette();
    ed.state.set_palette_query("add cub");
    let rows = ed.state.palette_results();
    assert_eq!(
        (rows[0].category.as_str(), rows[0].label.as_str()),
        ("Add", "Cube")
    );
    ed.state.palette_run_selected().unwrap();
    assert!(!ed.state.palette_open(), "running closes the palette");
    assert!(ed.state.world.entity_count() > before, "a cube was added");

    // Letters in order: "svas" is Save Scene As, with its shortcut.
    ed.state.open_palette();
    ed.state.set_palette_query("svas");
    let rows = ed.state.palette_results();
    assert_eq!(rows[0].id, "file.save_as");
    assert_eq!(
        rows[0].shortcut,
        Some(box_editor::shortcuts::Command::SaveAs)
    );

    // Every menu action is searchable.
    for id in [
        "file.new_project",
        "edit.undo",
        "view.grid",
        "play.toggle",
        "help.shortcuts",
    ] {
        assert!(ed.state.actions().get(id).is_some(), "{id}");
    }
    std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
}

#[test]
fn palette_keyboard_cmd_k_arrows_enter_and_escape() {
    let dir = temp_project("palette_keys");
    let mut ed = open(&dir);
    let mut d = Driver::new(&ed);
    d.frames(&mut ed, 3);

    d.key(&mut ed, Key::K, Modifiers::COMMAND);
    assert!(ed.state.palette_open(), "Cmd+K opens it");
    d.key(&mut ed, Key::Escape, Modifiers::NONE);
    assert!(!ed.state.palette_open(), "Escape closes it");
    assert!(ed.dialog().is_none());

    d.key(&mut ed, Key::F1, Modifiers::NONE);
    assert!(ed.state.palette_open(), "F1 opens it too");
    d.type_text(&mut ed, "scene settings");
    assert_eq!(ed.state.palette_results()[0].id, "view.scene_settings");
    d.find("Scene Settings").expect("the row is drawn");
    // Down then Up comes back to the first row; Enter runs it.
    d.key(&mut ed, Key::ArrowDown, Modifiers::NONE);
    d.key(&mut ed, Key::ArrowUp, Modifiers::NONE);
    let cube = by_name(&ed, "Cube").unwrap();
    ed.state.select(Some(cube));
    d.key(&mut ed, Key::Enter, Modifiers::NONE);
    assert!(!ed.state.palette_open());
    assert!(
        ed.state.scene_settings_shown(),
        "Scene Settings ran (selects nothing)"
    );

    // Help ▸ Command Palette… opens it as well.
    d.click(&mut ed, "Help");
    d.click(&mut ed, "Command Palette…");
    assert!(ed.state.palette_open());
    std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
}

#[test]
fn palette_shows_why_an_action_is_disabled() {
    let dir = temp_project("palette_disabled");
    let mut ed = open(&dir);
    ed.state.select(None);
    ed.state.open_palette();
    ed.state.set_palette_query("duplicate");
    let row = &ed.state.palette_results()[0];
    assert_eq!(row.id, "edit.duplicate");
    assert_eq!(row.enabled, Err("Select something first".to_string()));
    assert_eq!(
        ed.state.palette_run_selected(),
        Err("Select something first".to_string())
    );
    assert!(
        ed.state.palette_open(),
        "a disabled row keeps the palette open"
    );

    ed.state.set_palette_query("step one frame");
    assert_eq!(
        ed.state.palette_results()[0].enabled,
        Err("Start Play first".to_string())
    );

    // The reason is drawn in the row.
    let mut d = Driver::new(&ed);
    d.frames(&mut ed, 3);
    d.get("Start Play first");
    std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
}

#[test]
fn palette_lists_recently_used_first() {
    let dir = temp_project("palette_recent");
    let mut ed = open(&dir);
    for id in ["view.grid", "view.reset_camera"] {
        ed.state.open_palette();
        ed.state.set_palette_query(
            ed.state
                .actions()
                .get(id)
                .unwrap()
                .label()
                .to_owned()
                .as_str(),
        );
        let i = row_index(&ed, id);
        ed.state.palette_move(i as isize);
        ed.state.palette_run_selected().unwrap();
    }
    assert!(!ed.state.show_grid, "Grid was toggled off");
    ed.state.open_palette();
    let ids: Vec<String> = ed
        .state
        .palette_results()
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(ids[..2], ["view.reset_camera", "view.grid"]);
    // With a query, a recently used match is lifted above an equal one.
    ed.state.set_palette_query("reset");
    assert_eq!(ed.state.palette_results()[0].id, "view.reset_camera");
    std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
}

#[test]
fn registered_actions_appear_in_the_palette() {
    let dir = temp_project("palette_plugin");
    let mut ed = open(&dir);
    let runs = Arc::new(AtomicUsize::new(0));
    let r = runs.clone();
    ed.register_action(
        Action::new("game.respawn", "Game", "Respawn Player", move |s| {
            r.fetch_add(1, Ordering::Relaxed);
            s.notify("respawned");
        })
        .keywords("reset player")
        .enabled_when(|s| {
            if s.selection.is_empty() {
                Ok(())
            } else {
                Err("Deselect first".into())
            }
        }),
    );
    ed.state.select(None);
    ed.state.open_palette();
    ed.state.set_palette_query("respawn");
    let row = &ed.state.palette_results()[0];
    assert_eq!(
        (row.id.as_str(), row.category.as_str()),
        ("game.respawn", "Game")
    );
    ed.state.palette_run_selected().unwrap();
    assert_eq!(runs.load(Ordering::Relaxed), 1);
    assert_eq!(ed.state.run_action("game.respawn"), Ok(()));
    assert_eq!(runs.load(Ordering::Relaxed), 2);
    assert!(ed.state.run_action("game.nothing").is_err());
    std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
}

#[test]
fn palette_has_scenes_components_assets_and_history() {
    let dir = temp_project("palette_context");
    std::fs::write(dir.join("assets/scenes/level2.scene.json"), scene_bytes()).unwrap();
    std::fs::create_dir_all(dir.join("assets/materials")).unwrap();
    std::fs::write(dir.join("assets/materials/rock.material.json"), "{}").unwrap();
    let mut ed = open(&dir);

    ed.state.open_palette();
    // Open Scene ▸ another scene of the project.
    ed.state.set_palette_query("level2");
    let row = ed.state.palette_results()[0].clone();
    assert_eq!(row.category, "Open Scene");
    ed.state.palette_run_selected().unwrap();
    let mut d = Driver::new(&ed);
    d.frames(&mut ed, 2);
    assert_eq!(
        ed.state.scene.name, "level2",
        "the scene opened (it was clean)"
    );

    // Back in main: Add Component for the selection, by friendly name.
    ed.begin(DocAction::OpenScene(
        dir.join("assets/scenes/main.scene.json"),
    ));
    d.frames(&mut ed, 2);
    let cube = by_name(&ed, "Cube").unwrap();
    ed.state.select(Some(cube));
    ed.state.open_palette();
    ed.state.set_palette_query("point light");
    let rows = ed.state.palette_results();
    let i = rows
        .iter()
        .position(|r| r.category == "Add Component")
        .expect("Add Component ▸ Point Light");
    ed.state.palette_move(i as isize);
    ed.state.palette_run_selected().unwrap();
    let e = ed.state.world.entity_by_uid(cube).unwrap();
    assert!(ed.state.world.component_names(e).contains(&"PointLight"));

    // Assets ▸ a file selects it in the browser (Assets tab in front).
    ed.state.open_palette();
    ed.state.set_palette_query("rock material");
    let i = row_index(&ed, "asset:materials/rock.material.json");
    ed.state.palette_move(i as isize);
    ed.state.palette_run_selected().unwrap();
    d.frames(&mut ed, 2);
    assert_eq!(ed.state.assets.folder, "materials");
    assert!(ed.state.assets.selected.is_some());
    assert!(ed.state.assets.inspect.is_some(), "shown in the inspector");

    // History ▸ an earlier state.
    ed.state.open_palette();
    ed.state
        .set_palette_query(box_editor::history::HISTORY_START);
    let row = ed.state.palette_results()[0].clone();
    assert_eq!(row.category, "History");
    ed.state.palette_run_selected().unwrap();
    assert_eq!(ed.state.edits.undo_len(), 0);
    let e = ed.state.world.entity_by_uid(cube).unwrap();
    assert!(!ed.state.world.component_names(e).contains(&"PointLight"));
    std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
}

// ----- E6/W11: recent projects, launch and the Welcome card -----

#[test]
fn recent_projects_dedupe_cap_persist_and_clear() {
    let config = temp_dir("config");
    let mut r = RecentProjects::load(Some(&config));
    assert!(r.entries().is_empty());
    let a = temp_project("recent_a");
    let b = temp_project("recent_b");
    r.add(&a, "a");
    r.add(&b, "b");
    // The same folder again, through another spelling: moved to the front once.
    r.add(&a.join("assets/.."), "a");
    let paths: Vec<&Path> = r.entries().iter().map(|e| e.path.as_path()).collect();
    assert_eq!(paths, [a.as_path(), b.as_path()]);
    for i in 0..MAX_RECENT + 3 {
        r.add(&config.join(format!("p{i}")), &format!("p{i}"));
    }
    assert_eq!(r.entries().len(), MAX_RECENT);
    assert_eq!(r.entries()[0].name, format!("p{}", MAX_RECENT + 2));

    let again = RecentProjects::load(Some(&config));
    assert_eq!(again.entries(), r.entries(), "written to recent.json");
    assert!(config.join(start::RECENT_FILE).is_file());
    r.clear();
    assert!(RecentProjects::load(Some(&config)).entries().is_empty());

    // The config folder comes from BOX_EDITOR_CONFIG_DIR when set.
    let env = |over: Option<&'static str>| {
        move |k: &str| match k {
            start::CONFIG_DIR_ENV => over.map(Into::into),
            "HOME" => Some("/home/me".into()),
            // Windows reads the config base from APPDATA.
            "APPDATA" => Some(r"C:\Users\me\AppData\Roaming".into()),
            _ => None,
        }
    };
    assert_eq!(
        start::config_dir_from(env(Some("/tmp/cfg"))),
        Some(PathBuf::from("/tmp/cfg"))
    );
    assert!(
        start::config_dir_from(env(None))
            .unwrap()
            .ends_with("box-editor")
    );
    for d in [
        config,
        a.parent().unwrap().into(),
        b.parent().unwrap().into(),
    ] {
        std::fs::remove_dir_all(d).unwrap();
    }
}

#[test]
fn a_launch_without_a_project_picks_the_working_folder_then_the_last_project() {
    let config = temp_dir("launch");
    let a = temp_project("launch_a");
    let b = temp_project("launch_b");
    let mut recent = RecentProjects::load(Some(&config));
    assert_eq!(start::choose_launch(None, &recent, false), Launch::Welcome);
    recent.add(&b, "b");
    assert_eq!(
        start::choose_launch(Some(&a.join("assets")), &recent, false),
        Launch::WorkingFolder(a.clone()),
        "inside a project wins"
    );
    assert_eq!(
        start::choose_launch(Some(&config), &recent, false),
        Launch::LastProject(b.clone())
    );
    assert_eq!(
        start::choose_launch(Some(&config), &recent, true),
        Launch::Welcome
    );
    // A last project that is gone is skipped.
    std::fs::remove_file(b.join("box.project.json")).unwrap();
    assert_eq!(
        start::choose_launch(Some(&config), &recent, false),
        Launch::Welcome
    );

    // Through the editor (the test's working folder is not a project).
    let cwd = std::env::current_dir().unwrap();
    if box_engine::Project::find_root(&cwd).is_none() {
        let options = EditorOptions {
            config_dir: Some(config.clone()),
            ..Default::default()
        };
        let ed = editor_with(options.clone());
        assert!(
            ed.state.project.is_none() && ed.state.welcome,
            "Welcome card"
        );
        recent.add(&a, "a");
        let ed = editor_with(options.clone());
        assert_eq!(
            ed.state.project.as_ref().map(|p| p.root().to_path_buf()),
            Some(a.clone())
        );
        assert!(!ed.state.welcome);
        let ed = editor_with(EditorOptions {
            no_restore: true,
            ..options
        });
        assert!(
            ed.state.project.is_none() && ed.state.welcome,
            "--no-restore"
        );
    }
    // Without a config folder (tests, captures): no user state at all.
    let ed = editor_with(EditorOptions::default());
    assert!(ed.state.project.is_none() && !ed.state.welcome);
    for d in [
        config,
        a.parent().unwrap().into(),
        b.parent().unwrap().into(),
    ] {
        std::fs::remove_dir_all(d).unwrap();
    }
}

#[test]
fn welcome_card_and_open_recent_open_projects() {
    let config = temp_dir("welcome");
    let a = temp_project("welcome_a");
    let b = temp_project("welcome_b");
    let mut recent = RecentProjects::load(Some(&config));
    recent.add(&a, "welcome_a");
    let mut ed = editor_with(EditorOptions {
        config_dir: Some(config.clone()),
        no_restore: true,
        ..Default::default()
    });
    assert!(ed.state.welcome);
    let mut d = Driver::new(&ed);
    d.frames(&mut ed, 3);
    d.get("New Project…");
    d.get("Open Project…");
    d.click(&mut ed, "welcome_a");
    assert_eq!(ed.state.project.as_ref().unwrap().root(), a.as_path());
    assert!(!ed.state.welcome, "a project opened: the card is gone");

    // File ▸ Open Recent lists both after opening b; Clear empties it.
    ed.open_project(&b).unwrap();
    d.frames(&mut ed, 2);
    d.click(&mut ed, "File");
    d.click(&mut ed, "Open Recent");
    d.get("welcome_b");
    d.click(&mut ed, "welcome_a");
    assert_eq!(ed.state.project.as_ref().unwrap().root(), a.as_path());
    let names: Vec<&str> = ed
        .state
        .recent
        .entries()
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    assert_eq!(names, ["welcome_a", "welcome_b"]);
    ed.state.run_action("file.clear_recent").unwrap();
    assert!(RecentProjects::load(Some(&config)).entries().is_empty());

    // "Explore the sample scene" closes the card.
    let mut ed = editor_with(EditorOptions {
        config_dir: Some(config.clone()),
        ..Default::default()
    });
    let mut d = Driver::new(&ed);
    d.frames(&mut ed, 3);
    d.click(&mut ed, "Explore the sample scene");
    assert!(!ed.state.welcome);
    for dir in [
        config,
        a.parent().unwrap().into(),
        b.parent().unwrap().into(),
    ] {
        std::fs::remove_dir_all(dir).unwrap();
    }
}

// ----- A15: session restore -----

#[test]
fn a_project_reopens_with_its_scene_camera_and_selection() {
    let dir = temp_project("session");
    std::fs::write(dir.join("assets/scenes/level2.scene.json"), scene_bytes()).unwrap();
    // The saved layout too (no `editor_with`, which resets it).
    let restoring = || {
        let options = EditorOptions {
            project: Some(dir.clone()),
            restore: true,
            ..Default::default()
        };
        build_editor(&options, LogBuffer::default()).unwrap()
    };
    let mut ed = restoring();
    ed.state
        .open_scene(&dir.join("assets/scenes/level2.scene.json"), false)
        .unwrap();
    let ground = by_name(&ed, "Ground").expect("the new scene has a ground");
    let sun = by_name(&ed, "Sun").expect("and a sun");
    ed.state.selection = vec![sun, ground];
    ed.state.camera.orbit.target = Vec3::new(3.0, 1.0, -2.0);
    ed.state.camera.orbit.distance = 7.5;
    ed.state.camera.orbit.yaw = 1.25;
    ed.state.camera.orbit.pitch = 0.3;
    ed.focus_console(None);
    ed.on_quit();

    let ed = restoring();
    assert_eq!(ed.state.scene.name, "level2", "the last scene reopened");
    assert_eq!(ed.state.selection, vec![sun, ground]);
    let o = ed.state.camera.orbit;
    assert_eq!(
        (o.target, o.distance, o.yaw, o.pitch),
        (Vec3::new(3.0, 1.0, -2.0), 7.5, 1.25, 0.3)
    );
    // The active dock tab is part of the saved layout.
    let id = ed.tree.tiles.find_pane(&Pane::Console).unwrap();
    assert!(
        ed.tree.active_tiles().contains(&id),
        "the Console tab is in front"
    );

    // Entities that are gone are skipped; without restore the start scene opens.
    let mut prefs: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join(".box/editor.json")).unwrap())
            .unwrap();
    prefs["session"]["selection"] = serde_json::json!([Uid::new(), ground]);
    std::fs::write(dir.join(".box/editor.json"), prefs.to_string()).unwrap();
    assert_eq!(restoring().state.selection, vec![ground]);
    let plain = open(&dir);
    assert_eq!(plain.state.scene.name, "main");
    assert!(plain.state.selection.is_empty());

    // A deleted scene falls back to the start scene.
    std::fs::remove_file(dir.join("assets/scenes/level2.scene.json")).unwrap();
    assert_eq!(restoring().state.scene.name, "main");
    std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
}

// ----- registration errors at startup -----

mod clash {
    use box_engine::prelude::*;

    pub mod one {
        use box_engine::prelude::*;
        #[derive(Reflect, Default, Clone)]
        #[reflect(Component, Default, Clone)]
        pub struct Coin;
    }
    pub mod two {
        use box_engine::prelude::*;
        #[derive(Reflect, Default, Clone)]
        #[reflect(Component, Default, Clone)]
        pub struct Coin;
    }

    pub struct Clashing;
    impl Plugin for Clashing {
        fn register(&self, registry: &mut Registry) {
            registry.register_all::<(one::Coin, two::Coin)>();
        }
        fn build(&self, _app: &mut App) {}
    }
}

#[test]
fn registration_errors_are_shown_at_startup() {
    use tracing_subscriber::layer::SubscriberExt;
    let log = LogBuffer::default();
    let subscriber = tracing_subscriber::registry().with(log.layer());
    let game = box_engine::Game::new().with_plugin(clash::Clashing);
    let mut ed = tracing::subscriber::with_default(subscriber, || {
        box_editor::build_game_editor(&EditorOptions::default(), log.clone(), game).unwrap()
    });
    ed.state.poll_log();
    let toast = ed
        .state
        .toasts()
        .items()
        .iter()
        .find(|t| t.text.contains("Clashing"))
        .expect("an error toast names the plugin");
    assert_eq!(toast.severity, box_editor::log::Severity::Error);
    assert!(toast.text.contains("Coin"), "{}", toast.text);
    // Logged once: the first Play does not repeat it.
    assert_eq!(ed.state.game().report_registration_errors(), 0);
}

// ----- E20: undo history -----

#[test]
fn history_lists_steps_and_jumps() {
    let dir = temp_project("history");
    let mut ed = open(&dir);
    let cube = by_name(&ed, "Cube").unwrap();
    ed.state.rename(cube, "A");
    ed.state.rename(cube, "B");
    ed.state.add_entity(NewEntity::Sphere, None);
    assert_eq!(ed.state.edits.undo_len(), 3);
    ed.state.undo();
    let h = ed.state.history_entries();
    let labels: Vec<(&str, bool, bool)> = h
        .iter()
        .map(|e| (e.label.as_str(), e.current, e.redo))
        .collect();
    assert_eq!(labels.len(), 4);
    assert!(labels[0].2, "the undone step is first, as redo");
    assert!(labels[1].1, "then the current state");
    assert_eq!(
        labels[3],
        (box_editor::history::HISTORY_START, false, false)
    );

    ed.state.jump_history(0);
    assert_eq!(ed.state.edits.undo_len(), 0);
    assert_eq!(
        ed.state
            .world
            .get::<Name>(ed.state.world.entity_by_uid(cube).unwrap())
            .unwrap()
            .0,
        "Cube"
    );
    ed.state.jump_history(3);
    assert_eq!(ed.state.edits.undo_len(), 3);
    assert!(by_name(&ed, "Sphere").is_some());

    // Through Edit ▸ History.
    let mut d = Driver::new(&ed);
    d.frames(&mut ed, 3);
    d.click(&mut ed, "Edit");
    d.click(&mut ed, "History");
    let first = h[3].label.clone();
    d.click(&mut ed, &first);
    assert_eq!(ed.state.edits.undo_len(), 0, "clicked the first state");
    let _ = Transform::default();
    std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
}

#[test]
fn a_failed_recent_project_shows_an_error_and_leaves_the_list() {
    let dir = temp_project("recent_gone");
    let config = temp_dir("recent_gone_cfg");
    let mut ed = editor_with(EditorOptions {
        project: Some(dir.clone()),
        config_dir: Some(config.clone()),
        ..Default::default()
    });
    let gone = temp_project("recent_gone_b");
    ed.state.recent.add(&gone, "gone");
    std::fs::remove_dir_all(gone.parent().unwrap()).unwrap();
    ed.begin(DocAction::OpenRecent(gone.clone()));
    let mut d = Driver::new(&ed);
    d.frames(&mut ed, 2);
    assert!(matches!(ed.dialog(), Some(Dialog::Error { .. })));
    assert!(ed.state.recent.entries().iter().all(|e| e.path != gone));
    std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    std::fs::remove_dir_all(config).unwrap();
}

// ----- captures -----

fn skip_gpu() -> bool {
    std::env::var_os("BOX_SKIP_GPU_TESTS").is_some()
}

/// Captures for review in `target/captures/shell/`: the palette with
/// results, the Welcome card, File ▸ Open Recent and Edit ▸ History.
#[test]
fn shell_captures() {
    use box_editor::headless::{CaptureOptions, Headless};
    if skip_gpu() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping captures");
        return;
    }
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/shell");
    std::fs::create_dir_all(&out).unwrap();
    let config = temp_dir("captures_cfg");
    let a = temp_project("Meadow");
    let b = temp_project("Harbour");
    let mut recent = RecentProjects::load(Some(&config));
    recent.add(&b, "Harbour");
    recent.add(&a, "Meadow");
    let mut ed = editor_with(EditorOptions {
        config_dir: Some(config.clone()),
        no_restore: true,
        ..Default::default()
    });
    let options = CaptureOptions {
        size: [2560, 1600],
        pixels_per_point: 2.0,
        frames: 1,
    };
    let mut h = match Headless::new(&mut ed, &options) {
        Ok(h) => h,
        Err(e)
            if matches!(
                e.downcast_ref::<box_render::RenderError>(),
                Some(box_render::RenderError::NoAdapter)
            ) =>
        {
            eprintln!("no GPU adapter; skipping captures");
            return;
        }
        Err(e) => panic!("{e:#}"),
    };
    h.enable_accesskit();
    let run = |h: &mut Headless, ed: &mut Editor, events: Vec<Event>| {
        h.frame(ed, events);
        nodes_of(h.accesskit_update())
    };
    let find = |nodes: &[Node], text: &str| -> Pos2 {
        nodes
            .iter()
            .rev()
            .find(|n| n.text == text || n.text.starts_with(&format!("{text} ")))
            .unwrap_or_else(|| panic!("no `{text}`"))
            .rect
            .center()
    };
    let click = |h: &mut Headless, ed: &mut Editor, pos: Pos2| {
        let press = |pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        run(h, ed, vec![Event::PointerMoved(pos)]);
        run(h, ed, vec![press(true), press(false)]);
        let mut n = Vec::new();
        for _ in 0..20 {
            n = run(h, ed, Vec::new());
        }
        n
    };
    let save = |h: &Headless, name: &str| {
        h.image().unwrap().save(out.join(name)).unwrap();
    };
    for _ in 0..3 {
        run(&mut h, &mut ed, Vec::new());
    }
    save(&h, "welcome.png");

    // Open Meadow from the card, make a few edits, then the menus.
    let nodes = run(&mut h, &mut ed, Vec::new());
    let nodes = click(&mut h, &mut ed, find(&nodes, "Meadow"));
    assert!(ed.state.project.is_some());
    let cube = by_name(&ed, "Cube").unwrap();
    ed.state.select(Some(cube));
    ed.state.rename(cube, "Crate");
    ed.state.add_entity(NewEntity::Sphere, None);
    ed.state.add_entity(NewEntity::PointLight, None);
    ed.state.undo();
    let _ = nodes;
    let mut nodes = Vec::new();
    for _ in 0..3 {
        nodes = run(&mut h, &mut ed, Vec::new());
    }
    let n = click(&mut h, &mut ed, find(&nodes, "File"));
    let n = click(&mut h, &mut ed, find(&n, "Open Recent"));
    let _ = n;
    save(&h, "open_recent.png");
    let key = |key, pressed| Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: Modifiers::NONE,
    };
    run(
        &mut h,
        &mut ed,
        vec![key(Key::Escape, true), key(Key::Escape, false)],
    );
    let mut nodes = Vec::new();
    for _ in 0..5 {
        nodes = run(&mut h, &mut ed, Vec::new());
    }
    let n = click(&mut h, &mut ed, find(&nodes, "Edit"));
    click(&mut h, &mut ed, find(&n, "History"));
    save(&h, "history.png");
    run(
        &mut h,
        &mut ed,
        vec![key(Key::Escape, true), key(Key::Escape, false)],
    );
    for _ in 0..5 {
        run(&mut h, &mut ed, Vec::new());
    }

    // The palette: everything, then a search.
    ed.state.select(Some(cube));
    ed.state.open_palette();
    for _ in 0..3 {
        run(&mut h, &mut ed, Vec::new());
    }
    save(&h, "palette.png");
    run(&mut h, &mut ed, vec![Event::Text("li".into())]);
    for _ in 0..3 {
        run(&mut h, &mut ed, Vec::new());
    }
    save(&h, "palette_search.png");
    eprintln!("captures in {}", out.display());
    for d in [
        config,
        a.parent().unwrap().into(),
        b.parent().unwrap().into(),
    ] {
        std::fs::remove_dir_all(d).unwrap();
    }
}
