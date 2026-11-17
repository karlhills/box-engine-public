//! Project upgrades against the data projects as they were first committed
//! (M7 Collector Luau, M8 Cinder Keep), vendored under
//! `tests/fixtures/upgrade/` (box.project.json and every JSON file under
//! `assets/` with its `.meta`, exactly as committed then).
//!
//! No shipped format has changed incompatibly since those commits, so the
//! fixtures open as they are; the upgrade machinery is exercised with a
//! test-only type migration and an older `engine_version`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use box_engine::box_core::migrate::Step;
use box_engine::box_core::scene;
use box_engine::player::{PlayerOptions, build_app};
use box_engine::prelude::*;
use box_engine::project::ENGINE_VERSION;
use box_engine::savegame::{load_game, save_game};
use box_engine::upgrade::{self, FileKind};
use box_engine::{Game, Project};
use serde_json::{Value as Json, json};

mod upgrade_support;
use upgrade_support::*;

const DATA_PROJECTS: [&str; 2] = ["collector_luau_m7", "cinder_keep_m8"];

#[test]
fn vendored_data_projects_are_current_and_load_without_losing_data() {
    for name in DATA_PROJECTS {
        check_data_project(
            name,
            Expect {
                engine_types_only: true,
                materials: None,
            },
        );
    }
}

/// A test-only plugin: version 2 of the `Script` component (no data change;
/// the step checks the shape it upgrades), as a game or the engine would
/// add for a real change.
struct ScriptV2;

impl Plugin for ScriptV2 {
    fn register(&self, registry: &mut Registry) {
        registry.add_migration(
            "Script",
            Step::new("test-only: Script v2 (no data change)").custom(|v| {
                v.get("source").ok_or("a Script has a source")?;
                Ok(())
            }),
        );
    }

    fn build(&self, _app: &mut App) {}
}

/// Saves of every scene loaded with `registry`, by file.
fn loaded_bytes(root: &Path, registry: &Arc<Registry>) -> Vec<(PathBuf, Vec<u8>)> {
    files_with(root, ".scene.json")
        .into_iter()
        .map(|p| {
            let world = scene::load(&std::fs::read(&p).unwrap(), registry.clone()).unwrap();
            (p, scene::save(&world).unwrap())
        })
        .collect()
}

#[test]
fn an_older_project_upgrades_after_a_backup_and_loads_identically() {
    let root = copy_fixture("cinder_keep_m8", "apply");
    // As if made with an earlier engine release (every release so far is
    // 0.1.0, so the vendored file says that).
    let project_file = root.join("box.project.json");
    let mut pf = read_json(&project_file);
    pf["engine_version"] = json!("0.0.9");
    std::fs::write(
        &project_file,
        serde_json::to_string_pretty(&pf).unwrap() + "\n",
    )
    .unwrap();

    let game = Game::new().with_plugin(ScriptV2);
    let registry = game.registry();
    let originals: Vec<(PathBuf, Vec<u8>)> = files_with(&root, ".json")
        .into_iter()
        .chain([project_file.clone()])
        .map(|p| {
            let bytes = std::fs::read(&p).unwrap();
            (p, bytes)
        })
        .collect();
    // In memory, before any file changes: the reference.
    let before = loaded_bytes(&root, &registry);

    let plan = upgrade::plan(&root, &registry).unwrap();
    assert!(plan.needed() && !plan.blocked(), "{}", plan.summary());
    let kinds: Vec<(String, FileKind)> = plan
        .files
        .iter()
        .map(|f| (f.path.clone(), f.kind))
        .collect();
    assert_eq!(
        kinds,
        [
            ("assets/scenes/keep.scene.json".to_string(), FileKind::Scene),
            (
                "assets/scenes/outdoor.scene.json".to_string(),
                FileKind::Scene
            ),
            ("box.project.json".to_string(), FileKind::Project),
        ],
        "{}",
        plan.summary()
    );
    let summary = plan.summary();
    assert!(summary.contains("box-engine 0.0.9 to"), "{summary}");
    assert!(summary.contains("test-only: Script v2"), "{summary}");
    assert!(summary.contains("engine_version 0.0.9 -> "), "{summary}");
    // Planning wrote nothing.
    for (p, bytes) in &originals {
        assert_eq!(&std::fs::read(p).unwrap(), bytes, "{}", p.display());
    }
    assert!(!root.join(".box/backups").exists());

    let report = upgrade::apply(&plan).unwrap();
    assert_eq!(report.written.len(), 3);
    assert!(report.backup.starts_with(root.join(".box/backups")));
    // The backup holds the originals, byte for byte, and a manifest.
    for written in &report.written {
        let original = &originals
            .iter()
            .find(|(p, _)| p == &root.join(written))
            .unwrap()
            .1;
        assert_eq!(
            &std::fs::read(report.backup.join(written)).unwrap(),
            original,
            "{written}"
        );
    }
    let manifest = read_json(&report.backup.join("upgrade.json"));
    assert_eq!(manifest["files"].as_array().unwrap().len(), 3);
    assert_eq!(manifest["engine_version"], "0.0.9");
    // Files no step touched are untouched.
    for (p, bytes) in &originals {
        let rel = p
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if !report.written.contains(&rel) {
            assert_eq!(&std::fs::read(p).unwrap(), bytes, "{rel}");
        }
    }

    // The upgraded files: Scripts at version 2, this engine's version.
    let keep = read_json(&root.join("assets/scenes/keep.scene.json"));
    let scripts: Vec<&Json> = keep["entities"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["components"].get("Script"))
        .collect();
    assert!(!scripts.is_empty());
    assert!(scripts.iter().all(|s| s["$version"] == 2), "{scripts:?}");
    assert_eq!(read_json(&project_file)["engine_version"], ENGINE_VERSION);
    Project::open(&root).unwrap();

    // They load to exactly the worlds the old files loaded to.
    assert_eq!(loaded_bytes(&root, &registry), before);
    assert!(upgrade::plan(&root, &registry).unwrap().is_current());
    // Applying a current plan does nothing (no new backup).
    let again = upgrade::apply(&upgrade::plan(&root, &registry).unwrap()).unwrap();
    assert!(again.written.is_empty());

    // An engine without the step is older than the files: refused, with a
    // clear message, by the planner, the scene loader and the player.
    let older = engine_game();
    let plan = upgrade::plan(&root, &older.registry()).unwrap();
    assert!(
        plan.blocked() && !plan.newer.is_empty(),
        "{}",
        plan.summary()
    );
    assert!(plan.summary().contains("newer"), "{}", plan.summary());
    let e = scene::load(
        &std::fs::read(root.join("assets/scenes/keep.scene.json")).unwrap(),
        older.registry(),
    )
    .err()
    .unwrap();
    assert!(e.to_string().contains("newer"), "{e}");
    let options = PlayerOptions {
        project: Some(root.clone()),
        ..Default::default()
    };
    let refused = build_app(&older, &options).err().unwrap();
    assert!(format!("{refused:#}").contains("newer"), "{refused:#}");
}

#[test]
fn newer_files_block_the_upgrade() {
    let root = copy_fixture("collector_luau_m7", "newer");
    let registry = engine_game().registry();
    let set = |rel: &str, key: &str, value: Json| {
        let path = root.join(rel);
        let mut j = read_json(&path);
        j[key] = value;
        std::fs::write(&path, serde_json::to_vec_pretty(&j).unwrap()).unwrap();
    };
    set(
        "assets/scenes/arena.scene.json",
        "version",
        json!(scene::SCENE_VERSION + 1),
    );
    set("assets/scenes/arena.scene.json.meta", "version", json!(99));
    let plan = upgrade::plan(&root, &registry).unwrap();
    let newer: Vec<&str> = plan.newer.iter().map(|p| p.path.as_str()).collect();
    assert_eq!(
        newer,
        [
            "assets/scenes/arena.scene.json",
            "assets/scenes/arena.scene.json.meta"
        ]
    );
    assert!(plan.blocked());
    let err = upgrade::apply(&plan).unwrap_err().to_string();
    assert!(err.contains("newer box-engine"), "{err}");
    assert!(!root.join(".box/backups").exists());

    set("box.project.json", "engine_version", json!("99.0.0"));
    let plan = upgrade::plan(&root, &registry).unwrap();
    assert!(plan.newer.iter().any(|p| p.path == "box.project.json"));
}

/// A test-only game component and its version 2 (`hp` renamed `current`).
#[derive(Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default, Clone)]
struct Health {
    current: f32,
}

struct HealthGame;

impl Plugin for HealthGame {
    fn register(&self, registry: &mut Registry) {
        registry.register::<Health>().unwrap();
        registry.add_migration(
            "Health",
            Step::new("hp is now current").rename_field("hp", "current"),
        );
    }

    fn build(&self, _app: &mut App) {}
}

#[test]
fn save_games_run_type_steps_and_write_versions() {
    let game = Game::new().with_plugin(HealthGame);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("upgrade_saves");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // A save written by version 1 of the game.
    let old = dir.join("old.save.json");
    let uid = "00000000-0000-4000-8000-0000000000aa";
    std::fs::write(
        &old,
        serde_json::to_vec_pretty(&json!({
            "format": 1, "game": "", "version": 1, "saved_at": 0,
            "scenes": [], "resources": {},
            "entities": [{ "uid": uid, "name": "Hero", "parent": null, "scene": null,
                           "components": { "Health": { "hp": 42.0 } } }],
            "hooks": {}
        }))
        .unwrap(),
    )
    .unwrap();
    let mut world = game.new_world();
    load_game(&mut world, &old).unwrap();
    let hero = world.find_by_name("Hero").unwrap();
    assert_eq!(world.get::<Health>(hero).unwrap().current, 42.0);

    let new = dir.join("new.save.json");
    save_game(&mut world, &new).unwrap();
    let saved = read_json(&new);
    assert_eq!(
        saved["entities"][0]["components"]["Health"],
        json!({ "$version": 2, "current": 42.0 })
    );
}

#[test]
fn settings_files_keep_unknown_fields_and_versions() {
    let root = copy_fixture("collector_luau_m7", "settings");
    let path = root.join("assets/settings/AudioSettings.json");
    let mut file = read_json(&path);
    file["future_bus"] = json!({ "volume": 0.5 });
    std::fs::write(&path, serde_json::to_vec_pretty(&file).unwrap()).unwrap();

    let game = engine_game();
    let mut world = game.new_world();
    box_engine::settings::load_settings_from(
        &mut world,
        &root.join("assets"),
        None,
        box_engine::settings::Missing::UseDefault,
    );
    let written = box_engine::settings::settings_file_json(&world, "AudioSettings", &path).unwrap();
    assert_eq!(written["future_bus"], json!({ "volume": 0.5 }), "{written}");
    assert_preserved(&file, &written, "AudioSettings");
}
