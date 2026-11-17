//! Save games: round trips with scene and runtime entities, resources and
//! hook data; versioned migration; atomic writes with a backup; bad files
//! change nothing; `on_add` hooks run for restored entities.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use box_engine::prelude::*;
use box_engine::savegame::{SaveError, list_slots, load_game, read_save, save_game, slot_path};
use box_engine::scenes::SceneInstance;

#[derive(Reflect, Default, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone)]
struct Health {
    hp: i32,
}

#[derive(Reflect, Default, Clone, Debug, PartialEq)]
#[reflect(Resource, Default, Clone)]
struct Score {
    points: u32,
}

/// Runtime-only state, saved by a hook.
#[derive(Default, Debug, PartialEq)]
struct Inventory(Vec<String>);

/// Counts `on_add` hook runs for `Health`.
#[derive(Default)]
struct Added(u32);

struct TestGame;

impl Plugin for TestGame {
    fn register(&self, registry: &mut Registry) {
        registry.register_all::<(Health,)>();
        let _ = registry.register_resource::<Score>();
    }

    fn build(&self, app: &mut App) {
        app.world.resource_or_default::<Score>();
        app.world.resource_or_default::<Inventory>();
        app.world.resource_or_default::<Added>();
        app.on_add::<Health>(|w, _| w.resource_mut::<Added>().0 += 1);
        app.world.resource_mut::<SaveHooks>().add(
            "inventory",
            |w| serde_json::json!(w.resource::<Inventory>().0),
            |w, v| {
                w.resource_mut::<Inventory>().0 = v
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|s| s.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
            },
        );
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "box_savegame_{name}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// An app whose world has two "scene" entities (with uids, adopted as the
/// main scene instance), configured to save into `dir`.
fn app(dir: &Path) -> App {
    let game = Game::new().with_plugin(TestGame);
    let mut world = game.new_world();
    world.spawn((
        Uid::new(),
        Name::new("Hero"),
        Transform::from_xyz(1.0, 0.0, 0.0),
        Health { hp: 10 },
    ));
    let door = world.spawn((Uid::new(), Name::new("Door"), Transform::default()));
    world.spawn((Uid::new(), Name::new("Hinge"), Transform::default()));
    let hinge = world.find_by_name("Hinge").unwrap();
    world.set_parent(hinge, Some(door)).unwrap();
    let mut app = game.app(world);
    app.world.resource_mut::<SaveConfig>().dir = Some(dir.to_path_buf());
    app.world.resource_mut::<SaveConfig>().game = "test".into();
    app.tick(1.0 / 60.0);
    app
}

fn hero(world: &World) -> Entity {
    world.find_by_name("Hero").unwrap()
}

#[test]
fn a_saved_game_restores_entities_resources_and_hook_state() {
    let dir = temp_dir("round_trip");
    let mut app = app(&dir);
    // Runtime state: a spawned saveable enemy, an unsaved effect, a resource,
    // a hook's data.
    let enemy = app.world.spawn((
        Saveable,
        Name::new("Enemy"),
        Transform::from_xyz(5.0, 0.0, 0.0),
        Health { hp: 3 },
    ));
    app.world.spawn((Name::new("Spark"), Transform::default()));
    app.world.resource_mut::<Score>().points = 120;
    app.world.resource_mut::<Inventory>().0 = vec!["key".into(), "map".into()];
    app.tick(1.0 / 60.0);
    let path = slot_path(&app.world, "slot1").unwrap();
    save_game(&mut app.world, &path).unwrap();
    let enemy_uid = app.world.uid(enemy).expect("Saveable got a uid at save");
    let hero_uid = app.world.uid(hero(&app.world)).unwrap();

    // Play on: move, damage, kill the enemy, spawn another, change state.
    let h = hero(&app.world);
    app.world
        .component_mut::<Transform>(h)
        .unwrap()
        .translation
        .x = 9.0;
    app.world.component_mut::<Health>(h).unwrap().hp = 1;
    app.world.despawn(enemy);
    app.world.spawn((
        Saveable,
        Name::new("Enemy2"),
        Transform::default(),
        Health { hp: 7 },
    ));
    app.world.resource_mut::<Score>().points = 999;
    app.world.resource_mut::<Inventory>().0.clear();
    app.tick(1.0 / 60.0);
    let added_before = app.world.resource::<Added>().0;

    let n = load_game(&mut app.world, &path).unwrap();
    app.tick(1.0 / 60.0);
    assert_eq!(n, 4, "hero, door, hinge, enemy");
    let w = &app.world;
    let h = hero(w);
    assert_eq!(w.uid(h), Some(hero_uid));
    assert_eq!(w.get::<Transform>(h).unwrap().translation.x, 1.0);
    assert_eq!(*w.get::<Health>(h).unwrap(), Health { hp: 10 });
    let e = w
        .entity_by_uid(enemy_uid)
        .expect("the runtime enemy is back");
    assert_eq!(*w.get::<Health>(e).unwrap(), Health { hp: 3 });
    assert!(w.find_by_name("Enemy2").is_none(), "spawned after the save");
    assert!(
        w.find_by_name("Spark").is_some(),
        "runtime entities without a uid are kept (KeepAll)"
    );
    let hinge = w.find_by_name("Hinge").unwrap();
    assert_eq!(
        w.parent(hinge),
        w.find_by_name("Door"),
        "hierarchy restored"
    );
    assert_eq!(w.resource::<Score>().points, 120);
    assert_eq!(w.resource::<Inventory>().0, ["key", "map"]);
    // Scene bookkeeping: scene entities belong to the restored main instance.
    let main = w.resource::<Scenes>().main().unwrap();
    assert_eq!(w.get::<SceneInstance>(h).map(|s| s.0), Some(main));
    assert!(
        w.get::<SceneInstance>(e).is_none(),
        "runtime entities stay runtime"
    );
    // on_add hooks ran for the restored entities (hero and enemy).
    assert_eq!(w.resource::<Added>().0, added_before + 2);
}

#[test]
fn excluded_entities_are_neither_saved_nor_replaced() {
    let dir = temp_dir("exclude");
    let mut app = app(&dir);
    let cam = app.world.spawn((
        Uid::new(),
        Name::new("CameraRig"),
        SaveExclude,
        Transform::default(),
    ));
    let path = slot_path(&app.world, "s").unwrap();
    save_game(&mut app.world, &path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("CameraRig"));
    app.world
        .component_mut::<Transform>(cam)
        .unwrap()
        .translation
        .y = 4.0;
    load_game(&mut app.world, &path).unwrap();
    assert_eq!(app.world.get::<Transform>(cam).unwrap().translation.y, 4.0);
}

/// Transient entities (M8 gap report item 10) are never saved, with their
/// subtrees, and a load despawns them, even under the keep-all runtime
/// policy and even with a uid.
#[test]
fn transient_entities_are_not_saved_and_go_on_load() {
    let dir = temp_dir("transient");
    let mut app = app(&dir);
    assert_eq!(
        app.world.resource::<Scenes>().runtime_entities(),
        RuntimeEntities::KeepAll
    );
    let spawn_wave = |w: &mut World| {
        let enemy = w.spawn((
            Uid::new(),
            Name::new("Cinderling"),
            Transform::default(),
            Health { hp: 1 },
            Transient,
        ));
        let eye = w.spawn((Uid::new(), Name::new("Eye"), Transform::default()));
        w.set_parent(eye, Some(enemy)).unwrap();
        (enemy, eye)
    };
    let (enemy, eye) = spawn_wave(&mut app.world);
    app.tick(1.0 / 60.0);
    let path = slot_path(&app.world, "s").unwrap();
    save_game(&mut app.world, &path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        !text.contains("Cinderling") && !text.contains("\"Eye\""),
        "{text}"
    );
    assert!(text.contains("Hero"));
    let (later, _) = spawn_wave(&mut app.world);
    load_game(&mut app.world, &path).unwrap();
    for e in [enemy, eye, later] {
        assert!(!app.world.contains(e));
    }
    assert!(app.world.find_by_name("Cinderling").is_none());
    assert!(app.world.find_by_name("Hero").is_some());
}

#[test]
fn writes_are_atomic_with_a_backup_and_a_broken_slot_falls_back() {
    let dir = temp_dir("atomic");
    let mut app = app(&dir);
    let path = slot_path(&app.world, "a").unwrap();
    save_game(&mut app.world, &path).unwrap();
    app.world.resource_mut::<Score>().points = 5;
    save_game(&mut app.world, &path).unwrap();
    let bak = PathBuf::from(format!("{}.bak", path.display()));
    assert!(bak.exists(), "the previous save is kept");
    assert!(!PathBuf::from(format!("{}.tmp", path.display())).exists());
    let read_points = |p: &Path| {
        let j: serde_json::Value = serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap();
        j["resources"]["Score"]["points"].as_u64()
    };
    assert_eq!(read_points(&path), Some(5));
    assert_eq!(read_points(&bak), Some(0));
    // A torn write of the slot: loading uses the backup.
    std::fs::write(&path, b"{ \"format\": 1, \"entit").unwrap();
    app.world.resource_mut::<Score>().points = 77;
    load_game(&mut app.world, &path).unwrap();
    assert_eq!(app.world.resource::<Score>().points, 0);
    assert_eq!(list_slots(&app.world).len(), 0, "the torn slot is skipped");
}

/// A damaged save without a backup, and a slot folder that cannot be
/// written: the errors say what to do.
#[test]
fn damaged_and_unwritable_saves_say_what_to_do() {
    let dir = temp_dir("damaged");
    let mut app = app(&dir);
    let path = slot_path(&app.world, "one").unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"{ \"format\": 1, \"entit").unwrap();
    let err = load_game(&mut app.world, &path).unwrap_err().to_string();
    assert!(err.starts_with("not a save file: "), "{err}");
    assert!(
        err.contains("the save file is damaged and there is no backup"),
        "{err}"
    );
    assert!(err.contains("delete it to start over"), "{err}");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let folder = path.parent().unwrap();
        std::fs::remove_file(&path).unwrap();
        let mut perms = std::fs::metadata(folder).unwrap().permissions();
        perms.set_mode(0o555);
        std::fs::set_permissions(folder, perms.clone()).unwrap();
        let err = save_game(&mut app.world, &path).unwrap_err().to_string();
        perms.set_mode(0o755);
        std::fs::set_permissions(folder, perms).unwrap();
        assert!(err.contains("Permission denied"), "{err}");
        assert!(
            err.contains("check the file's and its folder's permissions"),
            "{err}"
        );
    }
}

#[test]
fn a_bad_file_changes_nothing() {
    let dir = temp_dir("bad");
    let mut app = app(&dir);
    let path = slot_path(&app.world, "bad").unwrap();
    save_game(&mut app.world, &path).unwrap();
    let mut j: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    // A component value of the wrong type in the last record.
    let last = j["entities"].as_array().unwrap().len() - 1;
    j["entities"][last]["components"]["Transform"] = serde_json::json!("not a transform");
    std::fs::write(&path, serde_json::to_vec(&j).unwrap()).unwrap();
    let before = app.world.entity_count();
    let err = load_game(&mut app.world, &path).unwrap_err();
    assert!(matches!(err, SaveError::Scene(_)), "{err}");
    assert_eq!(app.world.entity_count(), before);
    assert!(app.world.find_by_name("Hero").is_some());
}

#[test]
fn older_versions_migrate_and_newer_or_foreign_saves_are_refused() {
    let dir = temp_dir("versions");
    let mut app = app(&dir);
    let path = slot_path(&app.world, "v").unwrap();
    // Version 1 stored health under the hook "hp_bonus"; version 2 adds 5.
    app.world.resource_mut::<Score>().points = 10;
    save_game(&mut app.world, &path).unwrap();

    app.world.resource_mut::<SaveConfig>().version = 2;
    let err = read_save(&app.world, &path).unwrap_err();
    assert!(matches!(err, SaveError::Migration { from: 1, .. }), "{err}");
    app.world.resource_mut::<SaveConfig>().migrate = Some(Arc::new(|from, json| {
        assert_eq!(from, 1);
        let p = json["resources"]["Score"]["points"]
            .as_u64()
            .ok_or("no score")?;
        json["resources"]["Score"]["points"] = serde_json::json!(p * 10);
        Ok(())
    }));
    load_game(&mut app.world, &path).unwrap();
    assert_eq!(app.world.resource::<Score>().points, 100, "migrated");

    // Saved by version 2 now; a version 1 build refuses it.
    save_game(&mut app.world, &path).unwrap();
    app.world.resource_mut::<SaveConfig>().version = 1;
    assert!(matches!(
        read_save(&app.world, &path).unwrap_err(),
        SaveError::NewerVersion {
            found: 2,
            supported: 1
        }
    ));
    app.world.resource_mut::<SaveConfig>().version = 2;
    app.world.resource_mut::<SaveConfig>().game = "another".into();
    assert!(matches!(
        read_save(&app.world, &path).unwrap_err(),
        SaveError::OtherGame { .. }
    ));
    assert!(matches!(
        slot_path(&app.world, "../escape"),
        Err(SaveError::BadSlot(_))
    ));
}

#[test]
fn save_and_load_requests_apply_next_frame_and_report_events() {
    let dir = temp_dir("requests");
    let mut app = app(&dir);
    let mut cursor = EventCursor::<SaveEvent>::default();
    app.world.resource_mut::<Score>().points = 3;
    app.world.resource_mut::<SaveGames>().save("quick");
    app.tick(1.0 / 60.0);
    app.world.resource_mut::<Score>().points = 4;
    app.world.resource_mut::<SaveGames>().load("quick");
    app.world.resource_mut::<SaveGames>().load("missing");
    app.tick(1.0 / 60.0);
    let events: Vec<SaveEvent> = cursor
        .read(app.world.resource::<Events<SaveEvent>>())
        .cloned()
        .collect();
    assert!(
        matches!(&events[0], SaveEvent::Saved { slot, .. } if slot == "quick"),
        "{events:?}"
    );
    assert!(matches!(&events[1], SaveEvent::Loaded { slot, .. } if slot == "quick"));
    assert!(matches!(&events[2], SaveEvent::Failed { slot, .. } if slot == "missing"));
    assert_eq!(app.world.resource::<Score>().points, 3);
    let slots = list_slots(&app.world);
    assert_eq!(slots.len(), 1);
    assert_eq!(
        (slots[0].slot.as_str(), slots[0].game.as_str()),
        ("quick", "test")
    );
}
