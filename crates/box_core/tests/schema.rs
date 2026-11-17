//! Schema-only types: a game's component types known only from its dumped
//! registry (what `box scene apply` uses for a native project), edited
//! through the command layer as validated raw JSON.

mod common;

use std::sync::Arc;

use box_core::edit::{EditStack, apply_commands_with_warnings, parse_commands};
use box_core::reflect::Registry;
use box_core::scene::{self, unknown_types};
use box_core::world::World;
use common::{Behaviour, Health, Npc, Weather};

/// The engine's core types plus the test game's types as schemas only, as
/// the CLI builds it from `.box/registry.json`.
fn schema_registry() -> Arc<Registry> {
    let dump = common::registry().to_json();
    let mut r = Registry::with_core_types();
    let added = r.add_schema_types(&dump);
    for name in ["Health", "Npc", "PointLight", "Weather"] {
        assert!(added.iter().any(|a| a == name), "{added:?}");
    }
    assert!(
        !added.iter().any(|a| a == "Transform"),
        "registered types stay"
    );
    Arc::new(r)
}

const SCRIPT: &str = r#"[
  { "op": "spawn", "name": "Guard", "components": {
      "Health": { "current": 50.0 },
      "Npc": { "behaviour": "Idle", "tags": ["guard"], "target": null, "loot_table": null } } },
  { "op": "set", "entity": "Guard", "path": "Health.max", "value": 20000 },
  { "op": "set", "entity": "Guard", "path": "Npc.behaviour", "value": { "Patrol": { "speed": 2.0, "waypoints": [[1.0, 2.0, 3.0]] } } },
  { "op": "add_component", "entity": "Guard", "component": "PointLight" },
  { "op": "set", "entity": "Guard", "path": "PointLight.intensity", "value": 1500.0 },
  { "op": "remove_component", "entity": "Guard", "component": "PointLight" },
  { "op": "set_resource", "path": "Weather.fog", "value": 0.5 },
  { "op": "set", "entity": "Guard", "path": "Transform.translation.y", "value": 1.0 }
]"#;

#[test]
fn schema_only_components_are_edited_and_load_with_the_real_types() {
    let mut world = World::with_registry(schema_registry());
    let before = scene::save(&world).unwrap();
    let mut stack = EditStack::new();
    let commands = parse_commands(SCRIPT).unwrap();
    let applied = apply_commands_with_warnings(&mut world, &mut stack, &commands, "agent").unwrap();
    assert_eq!(applied.warnings.len(), 1, "{:?}", applied.warnings);
    assert!(
        applied.warnings[0].contains("Health.max") && applied.warnings[0].contains("clamped"),
        "{:?}",
        applied.warnings
    );
    assert!(unknown_types(&world).is_empty(), "schema types are known");
    assert_eq!(stack.undo_len(), 1, "one transaction");

    // The game, with its real types, reads what the schema-only edit wrote.
    let bytes = scene::save(&world).unwrap();
    let game = scene::load(&bytes, common::registry()).unwrap();
    let guard = game.find_by_name("Guard").unwrap();
    let health: Health = (*game.get::<Health>(guard).unwrap()).clone();
    assert_eq!(
        health,
        Health {
            current: 50.0,
            max: 10_000.0
        }
    );
    let npc: Npc = (*game.get::<Npc>(guard).unwrap()).clone();
    assert_eq!(npc.tags, vec!["guard".to_string()]);
    assert_eq!(
        npc.behaviour,
        Behaviour::Patrol {
            speed: 2.0,
            waypoints: vec![[1.0, 2.0, 3.0]]
        }
    );
    assert!(!game.has::<common::PointLight>(guard), "removed again");
    let weather = game.resource::<Weather>();
    assert_eq!(weather.fog, 0.5);
    assert_eq!(weather.wind, Weather::default().wind, "default filled in");

    // Undo restores the scene exactly.
    assert!(stack.undo(&mut world).unwrap());
    assert_eq!(scene::save(&world).unwrap(), before);
    assert!(stack.redo(&mut world).unwrap());
    assert_eq!(scene::save(&world).unwrap(), bytes);
}

fn apply_err(world: &mut World, json: &str) -> String {
    let mut stack = EditStack::new();
    let commands = parse_commands(json).unwrap();
    let before = scene::save(world).unwrap();
    let err = apply_commands_with_warnings(world, &mut stack, &commands, "agent")
        .unwrap_err()
        .to_string();
    assert_eq!(scene::save(world).unwrap(), before, "all or nothing");
    err
}

#[test]
fn schema_only_values_are_validated() {
    let mut world = World::with_registry(schema_registry());
    let mut stack = EditStack::new();
    let spawn = parse_commands(
        r#"{ "op": "spawn", "name": "Guard", "components": { "Health": { "current": 1.0 } } }"#,
    )
    .unwrap();
    apply_commands_with_warnings(&mut world, &mut stack, &spawn, "spawn").unwrap();

    let e = apply_err(
        &mut world,
        r#"{ "op": "set", "entity": "Guard", "path": "Health.max", "value": "lots" }"#,
    );
    assert!(e.contains("Health.max") && e.contains("f32"), "{e}");
    let e = apply_err(
        &mut world,
        r#"{ "op": "set", "entity": "Guard", "path": "Health.maxx", "value": 1.0 }"#,
    );
    assert!(e.contains("did you mean `max`"), "{e}");
    let e = apply_err(
        &mut world,
        r#"{ "op": "add_component", "entity": "Guard", "component": "Npc", "value": { "behaviour": "Sleep" } }"#,
    );
    assert!(e.contains("no variant `Sleep`"), "{e}");
    let e = apply_err(
        &mut world,
        r#"{ "op": "add_component", "entity": "Guard", "component": "Health" }"#,
    );
    assert!(e.contains("already"), "{e}");
    let e = apply_err(
        &mut world,
        r#"{ "op": "set", "entity": "Guard", "path": "Npc.tags", "value": [] }"#,
    );
    assert!(e.contains("no `Npc`"), "{e}");
    let e = apply_err(
        &mut world,
        r#"{ "op": "set", "entity": "Guard", "path": "Weather.fog", "value": 1.0 }"#,
    );
    assert!(e.contains("resource"), "{e}");
    let e = apply_err(
        &mut world,
        r#"{ "op": "add_component", "entity": "Guard", "component": "Healthy" }"#,
    );
    assert!(
        e.contains("unknown component or resource type `Healthy`"),
        "{e}"
    );
}
