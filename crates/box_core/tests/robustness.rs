//! Failure atomicity of the edit stack, value validation, and scene/world
//! invariants under unusual input.

mod common;

use std::time::{Duration, Instant};

use box_core::edit::{EditStack, apply_commands, parse_commands};
use box_core::identity::{Name, Uid};
use box_core::reflect::Registry;
use box_core::scene::{self, UnknownComponents};
use box_core::transform::Transform;
use box_core::world::{Entity, Placement, World};
use common::*;

const NAMES: [&str; 3] = ["a", "b", "c"];

/// Three saved roots `a`, `b`, `c` (uids 1..=3) with default transforms.
fn flat_world() -> World {
    let mut w = World::with_registry(registry());
    w.insert_resource(Weather::default());
    for (i, name) in NAMES.iter().enumerate() {
        w.spawn_saved(id(i as u128 + 1), name, (Transform::default(),))
            .unwrap();
    }
    w
}

fn id(i: u128) -> Uid {
    Uid::from_u128(i)
}

fn x(w: &World, uid: Uid) -> f32 {
    let e = w.entity_by_uid(uid).unwrap();
    w.get::<Transform>(e).unwrap().translation.x
}

fn set_x(stack: &mut EditStack, w: &mut World, uid: Uid, v: f32) {
    let p = w.field_path("Transform.translation.x").unwrap();
    stack.set_field(w, uid, &p, &v).unwrap();
}

fn raw_despawn(w: &mut World, uid: Uid) {
    let e = w.entity_by_uid(uid).unwrap();
    w.despawn(e);
}

// ----- defect 1: atomic undo / redo / rollback -----

#[test]
fn failed_undo_changes_nothing_and_keeps_the_step() {
    let mut w = flat_world();
    let mut stack = EditStack::new();
    stack.begin("Move both");
    set_x(&mut stack, &mut w, id(1), 100.0);
    set_x(&mut stack, &mut w, id(2), 200.0);
    stack.commit();
    // Behind the stack's back: the first edit can no longer be reverted.
    raw_despawn(&mut w, id(1));
    assert!(stack.undo(&mut w).is_err());
    assert_eq!(
        x(&w, id(2)),
        200.0,
        "the edit reverted first was re-applied"
    );
    assert_eq!((stack.undo_len(), stack.redo_len()), (1, 0));
    assert_eq!(stack.undo_label(), Some("Move both"));
}

#[test]
fn failed_redo_changes_nothing_and_keeps_the_step() {
    let mut w = flat_world();
    let mut stack = EditStack::new();
    stack.begin("Move both");
    set_x(&mut stack, &mut w, id(1), 100.0);
    set_x(&mut stack, &mut w, id(2), 200.0);
    stack.commit();
    assert!(stack.undo(&mut w).unwrap());
    raw_despawn(&mut w, id(2));
    assert!(stack.redo(&mut w).is_err());
    assert_eq!(x(&w, id(1)), 0.0, "the edit redone first was reverted");
    assert_eq!((stack.undo_len(), stack.redo_len()), (0, 1));
}

#[test]
fn failed_rollback_changes_nothing_and_keeps_the_transaction_open() {
    let mut w = flat_world();
    let mut stack = EditStack::new();
    stack.begin("Move both");
    set_x(&mut stack, &mut w, id(1), 100.0);
    set_x(&mut stack, &mut w, id(2), 200.0);
    raw_despawn(&mut w, id(1));
    assert!(stack.rollback(&mut w).is_err());
    assert_eq!(x(&w, id(2)), 200.0);
    assert!(stack.is_open());
    assert_eq!(stack.undo_label(), Some("Move both"));
}

// ----- defect 2: redo history and no-op edits -----

#[test]
fn rolled_back_batch_keeps_redo_history() {
    let mut w = flat_world();
    let mut stack = EditStack::new();
    set_x(&mut stack, &mut w, id(1), 7.0);
    assert!(stack.undo(&mut w).unwrap());
    let before = scene::save(&w).unwrap();
    let commands = parse_commands(
        r#"[
          { "op": "set", "entity": "b", "path": "Transform.translation.x", "value": 5.0 },
          { "op": "set", "entity": "nobody", "path": "Transform.translation.x", "value": 5.0 }
        ]"#,
    )
    .unwrap();
    assert!(apply_commands(&mut w, &mut stack, &commands, "Batch").is_err());
    assert_eq!(scene::save(&w).unwrap(), before);
    assert_eq!((stack.undo_len(), stack.redo_len()), (0, 1));
    assert!(stack.redo(&mut w).unwrap());
    assert_eq!(x(&w, id(1)), 7.0);
    // A committed edit still discards redo.
    assert!(stack.undo(&mut w).unwrap());
    stack.begin("Drag");
    set_x(&mut stack, &mut w, id(2), 1.0);
    assert!(!stack.can_redo());
    stack.commit();
    assert_eq!((stack.undo_len(), stack.redo_len()), (1, 0));
}

#[test]
fn identical_value_edits_record_nothing() {
    let mut w = flat_world();
    let mut stack = EditStack::new();
    set_x(&mut stack, &mut w, id(1), 7.0);
    assert!(stack.undo(&mut w).unwrap());
    set_x(&mut stack, &mut w, id(1), 0.0);
    stack.rename(&mut w, id(1), "a").unwrap();
    let whole = w.field_path("Transform").unwrap();
    stack
        .set_field(&mut w, id(2), &whole, &Transform::default())
        .unwrap();
    let fog = w.field_path("Weather.fog").unwrap();
    stack
        .set_resource_field(&mut w, &fog, &Weather::default().fog)
        .unwrap();
    assert_eq!((stack.undo_len(), stack.redo_len()), (0, 1));
    // A real change still records.
    stack.rename(&mut w, id(1), "renamed").unwrap();
    assert_eq!((stack.undo_len(), stack.redo_len()), (1, 0));
}

// ----- defect 3: non-finite values -----

#[test]
fn non_finite_values_are_rejected_by_the_edit_layer() {
    let scripts = [
        r#"{ "op": "set", "entity": "a", "path": "Transform.translation.x", "value": 1e39 }"#,
        r#"{ "op": "set", "entity": "a", "path": "Transform.translation", "value": [0.0, 1e39, 0.0] }"#,
        r#"{ "op": "set", "entity": "a", "path": "Transform",
             "value": { "translation": [0.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0, -1e39], "scale": [1.0, 1.0, 1.0] } }"#,
        r#"{ "op": "add_component", "entity": "a", "component": "Health", "value": { "current": 1e39, "max": 1.0 } }"#,
        r#"{ "op": "add_component", "entity": "a", "component": "PointLight",
             "value": { "color": [1.0, 1e39, 1.0], "intensity": 1.0, "radius": 1.0 } }"#,
        r#"{ "op": "add_component", "entity": "a", "component": "Npc",
             "value": { "behaviour": { "Patrol": { "speed": 1.0, "waypoints": [[0.0, 0.0, 1e39]] } },
                        "tags": [], "target": null, "loot_table": null } }"#,
        r#"{ "op": "spawn", "name": "d", "components": { "Health": { "current": 1.0, "max": 1e39 } } }"#,
        r#"{ "op": "set_resource", "path": "Weather.wind", "value": [1e39, 0.0, 0.0] }"#,
    ];
    let mut w = flat_world();
    let mut stack = EditStack::new();
    let before = scene::save(&w).unwrap();
    for script in scripts {
        let commands = parse_commands(script).unwrap();
        let result = apply_commands(&mut w, &mut stack, &commands, "Bad");
        assert!(result.is_err(), "accepted: {script}");
    }
    // Typed values, a prepared edit and pasted records.
    let px = w.field_path("Transform.translation.x").unwrap();
    assert!(stack.set_field(&mut w, id(1), &px, &f32::NAN).is_err());
    assert!(
        stack
            .add_component(
                &mut w,
                id(1),
                "Health",
                Some(&Health {
                    current: f32::INFINITY,
                    max: 1.0
                })
            )
            .is_err()
    );
    let edit = box_core::edit::Edit::SetField {
        uid: id(1),
        path: px.clone(),
        old: Box::new(0.0f32),
        new: Box::new(f32::NEG_INFINITY),
    };
    assert!(stack.apply(&mut w, "Plugin", edit).is_err());
    let record = serde_json::json!({
        "uid": id(9).to_string(), "name": "pasted", "parent": null,
        "components": { "Health": { "current": 1e39, "max": 1.0 } }
    });
    assert!(
        stack
            .instantiate(&mut w, &[record], Placement::last(None))
            .is_err()
    );
    assert!(w.entity_by_uid(id(9)).is_none());

    assert_eq!(stack.undo_len(), 0);
    let saved = scene::save(&w).unwrap();
    assert_eq!(saved, before);
    scene::load(&saved, registry()).unwrap();
}

// ----- defect 4: saved entities under unsaved parents -----

#[test]
fn saved_entities_under_unsaved_parents_are_saved_under_the_nearest_saved_ancestor() {
    let mut w = World::with_registry(registry());
    let a = w.spawn_saved(id(1), "a", ()).unwrap();
    let unsaved = w.spawn((Transform::default(),));
    let c = w.spawn_saved(id(2), "c", ()).unwrap();
    let d = w.spawn_saved(id(3), "d", ()).unwrap();
    let unsaved_root = w.spawn((Transform::default(),));
    let e = w.spawn_saved(id(4), "e", ()).unwrap();
    w.set_parent(unsaved, Some(a)).unwrap();
    w.set_parent(c, Some(unsaved)).unwrap();
    w.set_parent(d, Some(c)).unwrap();
    w.set_parent(e, Some(unsaved_root)).unwrap();

    assert_eq!(w.placement(c).parent, Some(id(1)));
    assert_eq!(w.placement(e).parent, None);
    assert_eq!(scene::save_subtree(&w, a).unwrap().len(), 3);

    let bytes = scene::save(&w).unwrap();
    for loaded in [
        scene::load(&bytes, registry()).unwrap(),
        scene::clone_world(&w).unwrap(),
    ] {
        let get = |i| loaded.entity_by_uid(id(i)).expect("entity was dropped");
        assert_eq!(loaded.parent(get(2)), Some(get(1)));
        assert_eq!(loaded.parent(get(3)), Some(get(2)));
        assert_eq!(loaded.parent(get(4)), None);
        assert_eq!(loaded.roots(), &[get(1), get(4)]);
        assert_eq!(scene::save(&loaded).unwrap(), bytes);
    }
}

// ----- defect 5: duplicate uids -----

fn assert_index_consistent(w: &World) {
    let mut seen = std::collections::HashSet::new();
    for (e, uid) in w.query::<(Entity, &Uid)>().iter() {
        assert!(seen.insert(*uid), "two entities share uid {uid}");
        assert_eq!(w.entity_by_uid(*uid), Some(e), "stale index for {uid}");
    }
    let bytes = scene::save(w).unwrap();
    scene::load(&bytes, w.registry().clone()).unwrap();
}

#[test]
fn duplicate_uids_cannot_be_created() {
    let mut w = World::with_registry(registry());
    let first = w.spawn((id(1), Name::new("first")));
    let second = w.spawn((id(1), Name::new("second")));
    assert_eq!(w.entity_by_uid(id(1)), Some(first));
    assert_ne!(w.uid(second), Some(id(1)));
    assert_index_consistent(&w);

    // An entity keeps its own uid when asked to take one that is in use.
    let third = w.spawn((id(3),));
    w.insert_one(third, id(1)).unwrap();
    assert_eq!(w.uid(third), Some(id(3)));
    w.insert(third, (id(1), Name::new("third"))).unwrap();
    assert_eq!(w.uid(third), Some(id(3)));
    // An unsaved entity gets a fresh one.
    let fourth = w.spawn((Name::new("fourth"),));
    w.insert_one(fourth, id(1)).unwrap();
    assert!(w.uid(fourth).is_some_and(|u| u != id(1)));
    assert_eq!(w.entity_by_uid(id(1)), Some(first));
    assert_index_consistent(&w);
    assert_eq!(w.roots(), &[first, second, third, fourth]);
}

#[test]
fn spawn_saved_with_a_uid_in_the_bundle_keeps_the_index_exact() {
    let mut w = World::with_registry(registry());
    let e = w
        .spawn_saved(id(1), "e", (id(2), Transform::default()))
        .unwrap();
    let uid = w.uid(e).unwrap();
    assert_eq!(w.entity_by_uid(uid), Some(e));
    let other = if uid == id(1) { id(2) } else { id(1) };
    assert_eq!(w.entity_by_uid(other), None);
    assert_index_consistent(&w);
}

// ----- defect 7: many children under one parent -----

/// A parent (uid 1) and `children` entities under it, or as roots when `flat`.
fn records(children: usize, flat: bool) -> Vec<serde_json::Value> {
    let parent = id(1).to_string();
    let mut records = vec![serde_json::json!({ "uid": parent, "name": "parent", "parent": null })];
    records.extend((0..children).map(|i| {
        let uid = id(i as u128 + 2).to_string();
        if flat {
            serde_json::json!({ "uid": uid, "name": "child", "parent": null })
        } else {
            serde_json::json!({ "uid": uid, "name": "child", "parent": parent })
        }
    }));
    records
}

fn time_load(records: &[serde_json::Value]) -> (World, Duration) {
    let scene = serde_json::json!({ "version": 1, "entities": records });
    let bytes = serde_json::to_vec(&scene).unwrap();
    let start = Instant::now();
    let w = scene::load(&bytes, registry()).unwrap();
    (w, start.elapsed())
}

#[test]
fn twenty_thousand_children_load_paste_and_unload_in_linear_time() {
    const CHILDREN: usize = 20_000;
    // The same number of entities without a shared parent: linear by construction.
    let (_, baseline) = time_load(&records(CHILDREN, true));

    let wide = records(CHILDREN, false);
    let (w, load) = time_load(&wide);
    let parent = w.entity_by_uid(id(1)).unwrap();
    let children = w.children(parent);
    assert_eq!(children.len(), CHILDREN);
    let last = id(CHILDREN as u128 + 1);
    assert_eq!(w.uid(children[CHILDREN - 1]), Some(last));

    // Paste through the edit stack.
    let mut w = World::with_registry(registry());
    let mut stack = EditStack::new();
    let start = Instant::now();
    stack
        .instantiate(&mut w, &wide, Placement::last(None))
        .unwrap();
    let paste = start.elapsed();

    // A load that fails at the end despawns everything again, last first.
    let mut bad = wide.clone();
    bad.push(serde_json::json!({ "uid": "not a uid" }));
    let mut w = World::with_registry(registry());
    let start = Instant::now();
    assert!(scene::instantiate(&mut w, &bad, None).is_err());
    let cleanup = start.elapsed();
    assert_eq!(w.entity_count(), 0);

    // Quadratic sibling scans cost 20 to 40 baselines here.
    let limit = baseline * 10 + Duration::from_millis(150);
    println!("baseline {baseline:?}, load {load:?}, paste {paste:?}, failed load {cleanup:?}");
    assert!(load < limit, "load took {load:?} (limit {limit:?})");
    assert!(paste < limit, "paste took {paste:?} (limit {limit:?})");
    assert!(
        cleanup < limit,
        "failed load took {cleanup:?} (limit {limit:?})"
    );
}

// ----- defect 8: a component that was unknown and is now registered -----

#[test]
fn registered_component_wins_over_its_stale_unknown_copy() {
    let text = format!(
        r#"{{ "version": 1, "entities": [ {{ "uid": "{}", "name": "a", "parent": null,
            "components": {{ "Health": {{ "current": 1.0, "max": 2.0 }}, "Zeta": {{ "k": 1 }} }} }} ] }}"#,
        id(1)
    );
    // Loaded before the game registered `Health`.
    let mut w = scene::load(
        text.as_bytes(),
        std::sync::Arc::new(Registry::with_core_types()),
    )
    .unwrap();
    let e = w.entity_by_uid(id(1)).unwrap();
    assert!(
        w.get::<UnknownComponents>(e)
            .unwrap()
            .0
            .contains_key("Health")
    );
    w.set_registry(registry());
    w.insert_one(
        e,
        Health {
            current: 50.0,
            max: 60.0,
        },
    )
    .unwrap();

    let saved = scene::save_string(&w).unwrap();
    assert_eq!(saved.matches("\"Health\"").count(), 1, "{saved}");
    let loaded = scene::load(saved.as_bytes(), registry()).unwrap();
    let e = loaded.entity_by_uid(id(1)).unwrap();
    assert_eq!(loaded.get::<Health>(e).unwrap().current, 50.0);
    // Still-unknown components are kept.
    assert!(
        loaded
            .get::<UnknownComponents>(e)
            .unwrap()
            .0
            .contains_key("Zeta")
    );
}
