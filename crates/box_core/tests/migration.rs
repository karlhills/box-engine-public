//! Scene migration: type steps, renames, `"$version"`, refusal of newer
//! data and preservation of unknown fields through load and save.

mod common;

use std::sync::Arc;

use box_core::migrate::{MigrateError, Step, VERSION_KEY};
use box_core::reflect::Registry;
use box_core::scene::{self, SceneError, UnknownComponents, UnknownFields, UnknownResourceFields};
use common::*;
use serde_json::{Value as Json, json};

const U1: &str = "00000000-0000-4000-8000-000000000001";
const U2: &str = "00000000-0000-4000-8000-000000000002";

/// The common registry plus: `Health` 1 -> 2 renamed `hp` to `current`
/// and `hp_max` to `max`; `Weather` 1 -> 2 moved `fog_density` to `fog`;
/// `Lamp` renamed to `PointLight`.
fn migrating_registry() -> Arc<Registry> {
    let mut r = Registry::with_core_types();
    r.register::<PointLight>().unwrap();
    r.register::<Health>().unwrap();
    r.register::<Npc>().unwrap();
    r.register_resource::<Weather>().unwrap();
    r.add_migration(
        "Health",
        Step::new("hp is now current, hp_max is max")
            .rename_field("hp", "current")
            .rename_field("hp_max", "max"),
    );
    r.add_migration(
        "Weather",
        Step::new("fog_density is now fog").move_field("fog_density", "fog"),
    );
    r.rename_type("Lamp", "PointLight");
    Arc::new(r)
}

/// A scene as an older game version wrote it.
fn old_scene() -> Json {
    json!({
        "version": 1,
        "resources": { "Weather": { "fog_density": 0.5, "wind": [0.0, 0.0, 1.0] } },
        "entities": [
            {
                "uid": U1, "name": "Hero", "parent": null,
                "components": {
                    "Health": { "hp": 40.0, "hp_max": 80.0 },
                    "Lamp": { "color": [1.0, 0.5, 0.0], "intensity": 200.0, "radius": 3.0 }
                }
            }
        ]
    })
}

#[test]
fn type_steps_and_renames_run_on_load_and_save_writes_versions() {
    let registry = migrating_registry();
    let bytes = serde_json::to_vec(&old_scene()).unwrap();
    let world = scene::load(&bytes, registry.clone()).unwrap();
    let hero = world.find_by_name("Hero").unwrap();
    assert_eq!(
        *world.get::<Health>(hero).unwrap(),
        Health {
            current: 40.0,
            max: 80.0
        }
    );
    assert_eq!(world.get::<PointLight>(hero).unwrap().intensity, 200.0);
    assert_eq!(world.resource::<Weather>().fog, 0.5);
    assert!(world.get::<UnknownComponents>(hero).is_none());

    let saved: Json = serde_json::from_slice(&scene::save(&world).unwrap()).unwrap();
    let health = &saved["entities"][0]["components"]["Health"];
    assert_eq!(
        health,
        &json!({ "$version": 2, "current": 40.0, "max": 80.0 })
    );
    // `$version` is the first key.
    assert_eq!(
        health.as_object().unwrap().keys().next().unwrap(),
        VERSION_KEY
    );
    assert_eq!(saved["resources"]["Weather"][VERSION_KEY], 2);
    assert!(saved["entities"][0]["components"].get("Lamp").is_none());
    // Unversioned types stay as they were.
    assert!(
        saved["entities"][0]["components"]["PointLight"]
            .get(VERSION_KEY)
            .is_none()
    );
    // A current file loads without running any step, and round-trips.
    let again = scene::load(&scene::save(&world).unwrap(), registry).unwrap();
    assert_eq!(
        scene::save(&again).unwrap(),
        scene::save(&world).unwrap(),
        "current files round-trip byte for byte"
    );
}

#[test]
fn upgrade_tree_reports_what_changes_and_is_idempotent() {
    let registry = migrating_registry();
    let mut tree = old_scene();
    let report = scene::upgrade_tree(&registry, &mut tree).unwrap();
    assert!(!report.is_empty());
    assert_eq!(report.format, None, "the envelope is current");
    let lines = report.lines().join("\n");
    assert!(
        lines.contains("`Lamp` renamed to `PointLight` (1x)"),
        "{lines}"
    );
    assert!(
        lines.contains("`Health` (1x): hp is now current"),
        "{lines}"
    );
    assert!(
        lines.contains("`Weather` (1x): fog_density is now fog"),
        "{lines}"
    );
    // Renamed keys keep their position.
    let keys: Vec<_> = tree["entities"][0]["components"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(keys, ["Health", "PointLight"]);
    assert_eq!(tree["entities"][0]["components"]["Health"]["$version"], 2);
    let mut again = tree.clone();
    assert!(
        scene::upgrade_tree(&registry, &mut again)
            .unwrap()
            .is_empty()
    );
    assert_eq!(again, tree);
}

#[test]
fn newer_data_is_refused_with_a_clear_error() {
    let registry = migrating_registry();
    let mut tree = old_scene();
    tree["entities"][0]["components"]["Health"] = json!({ "$version": 3, "current": 1.0 });
    let err = scene::load_value(&tree, registry.clone()).err().unwrap();
    let text = err.to_string();
    assert!(
        matches!(&err, SceneError::ComponentMigration { component, .. } if component == "Health")
            && matches!(
                err.migration(),
                Some(MigrateError::Newer {
                    found: 3,
                    supported: 2,
                    ..
                })
            ),
        "{err:?}"
    );
    assert!(text.contains("newer"), "{text}");
    // A newer scene envelope too.
    let mut newer = old_scene();
    newer["version"] = json!(scene::SCENE_VERSION + 1);
    assert!(matches!(
        scene::load_value(&newer, registry.clone()),
        Err(SceneError::NewerVersion { .. })
    ));
    // A malformed `$version`.
    let mut bad = old_scene();
    bad["entities"][0]["components"]["Health"]["$version"] = json!("two");
    assert!(scene::load_value(&bad, registry).is_err());
}

#[test]
fn a_failing_step_names_the_type_and_the_step() {
    let mut r = Registry::with_core_types();
    r.register::<Health>().unwrap();
    r.add_migration(
        "Health",
        Step::new("hp must be a number").map_field("hp", |v| {
            v.as_f64()
                .map(|n| json!({ "value": n }))
                .ok_or_else(|| "not a number".to_string())
        }),
    );
    let tree = json!({ "version": 1, "entities": [
        { "uid": U1, "name": null, "parent": null, "components": { "Health": { "hp": "x" } } }
    ]});
    let text = scene::load_value(&tree, Arc::new(r))
        .err()
        .unwrap()
        .to_string();
    assert!(text.contains("Health"), "{text}");
    assert!(text.contains("\"hp must be a number\""), "{text}");
    assert!(text.contains("not a number"), "{text}");
}

#[test]
fn unknown_fields_of_known_types_are_kept_and_written_back() {
    let registry = registry();
    let tree = json!({
        "version": 1,
        "resources": { "Weather": { "fog": 0.1, "wind": [0.0, 0.0, 0.0], "rain": { "rate": 3 } } },
        "entities": [
            { "uid": U1, "name": "A", "parent": null, "components": {
                "Health": { "current": 5.0, "max": 10.0, "shield": 2.5, "regen": [1, 2] },
                "FuturePlugin": { "$version": 7, "x": 1 }
            } },
            { "uid": U2, "name": "B", "parent": U1, "components": {
                "Health": { "current": 1.0, "max": 1.0 }
            } }
        ]
    });
    let world = scene::load_value(&tree, registry.clone()).unwrap();
    let a = world.find_by_name("A").unwrap();
    assert_eq!(world.get::<Health>(a).unwrap().current, 5.0);
    let extra = world.get::<UnknownFields>(a).unwrap();
    assert_eq!(
        Json::Object(extra.0["Health"].clone()),
        json!({ "shield": 2.5, "regen": [1, 2] })
    );
    assert!(
        world
            .get_resource::<UnknownResourceFields>()
            .unwrap()
            .0
            .contains_key("Weather")
    );
    let b = world.find_by_name("B").unwrap();
    assert!(world.get::<UnknownFields>(b).is_none());

    // Written back after the known fields; an unknown type keeps its version.
    let saved: Json = serde_json::from_slice(&scene::save(&world).unwrap()).unwrap();
    let health = &saved["entities"][0]["components"]["Health"];
    let keys: Vec<_> = health.as_object().unwrap().keys().cloned().collect();
    assert_eq!(keys, ["current", "max", "shield", "regen"]);
    assert_eq!(saved["resources"]["Weather"]["rain"], json!({ "rate": 3 }));
    assert_eq!(
        saved["entities"][0]["components"]["FuturePlugin"],
        json!({ "$version": 7, "x": 1 })
    );

    // Through the snapshot paths too: Play clone and copy/paste records.
    let play = scene::clone_world(&world).unwrap();
    let pa = play.find_by_name("A").unwrap();
    assert_eq!(*play.get::<UnknownFields>(pa).unwrap(), *extra);
    let records = scene::save_subtree(&world, a).unwrap();
    assert_eq!(records[0]["components"]["Health"]["shield"], 2.5);
}

#[test]
fn schema_only_types_without_their_steps_ask_for_the_games_code() {
    // A tool that knows the game's types only from the registry dump.
    let mut game = Registry::with_core_types();
    game.register::<Health>().unwrap();
    game.add_migration(
        "Health",
        Step::new("hp is current").rename_field("hp", "current"),
    );
    let dump = game.to_json();
    let health = dump["components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "Health")
        .unwrap();
    assert_eq!(health["version"], 2);

    let mut tool = Registry::with_core_types();
    tool.add_schema_types(&dump);
    let tool = Arc::new(tool);
    assert_eq!(tool.type_version("Health"), 2);
    let current = json!({ "version": 1, "entities": [
        { "uid": U1, "name": null, "parent": null,
          "components": { "Health": { "$version": 2, "current": 3.0, "max": 4.0 } } }
    ]});
    // Current data stays raw, with its version.
    let world = scene::load_value(&current, tool.clone()).unwrap();
    let e = world
        .entity_by_uid(box_core::identity::Uid::parse(U1).unwrap())
        .unwrap();
    assert_eq!(
        world.get::<UnknownComponents>(e).unwrap().0["Health"]["$version"],
        2
    );
    let saved: Json = serde_json::from_slice(&scene::save(&world).unwrap()).unwrap();
    assert_eq!(saved["entities"][0]["components"]["Health"]["$version"], 2);
    // Old data cannot be upgraded without the steps.
    let old = json!({ "version": 1, "entities": [
        { "uid": U1, "name": null, "parent": null, "components": { "Health": { "hp": 3.0 } } }
    ]});
    let err = scene::load_value(&old, tool).err().unwrap();
    assert!(
        matches!(err, SceneError::ComponentMigration { .. })
            && matches!(
                err.migration(),
                Some(MigrateError::NeedsGameCode {
                    found: 1,
                    current: 2,
                    ..
                })
            ),
        "{err:?}"
    );
}

#[test]
fn single_value_files_upgrade_split_unknown_fields_and_write_them_back() {
    let registry = migrating_registry();
    let ty = std::any::TypeId::of::<Health>();
    let file = json!({ "hp": 3.0, "hp_max": 9.0, "armor": 2 });
    let box_core::migrate::ValueFile {
        value,
        unknown: extra,
        steps,
    } = box_core::migrate::read_value_file(&registry, "Health", ty, &file).unwrap();
    assert_eq!(value, json!({ "current": 3.0, "max": 9.0 }));
    assert_eq!(Json::Object(extra.clone()), json!({ "armor": 2 }));
    assert_eq!(steps, ["hp is now current, hp_max is max"]);
    let typed = registry.value_from_json(ty, &value).unwrap();
    let json = registry.value_to_json(typed.as_ref()).unwrap();
    let written = box_core::migrate::value_file_json(&registry, "Health", json, &extra);
    assert_eq!(
        written,
        json!({ "$version": 2, "current": 3.0, "max": 9.0, "armor": 2 })
    );
    assert_eq!(
        Json::Object(box_core::migrate::unknown_fields_of(
            &registry, ty, &written
        )),
        json!({ "armor": 2 })
    );
    // A current file needs no step.
    let again = box_core::migrate::read_value_file(&registry, "Health", ty, &written).unwrap();
    assert!(again.steps.is_empty());
}

#[test]
fn renamed_and_new_name_together_is_an_error() {
    let registry = migrating_registry();
    let mut tree = old_scene();
    tree["entities"][0]["components"]["PointLight"] = json!({});
    let text = scene::load_value(&tree, registry)
        .err()
        .unwrap()
        .to_string();
    assert!(text.contains("renamed"), "{text}");
}
