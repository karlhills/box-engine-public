//! Scene JSON round trip, ordering, unknown data, snapshots and Play-mode clone.

mod common;

use box_core::identity::{Name, Uid};
use box_core::primitive::{Primitive, Shape};
use box_core::scene::{self, SceneError, UnknownComponents, UnknownResources};
use box_core::transform::Transform;
use box_core::world::{Entity, Placement, World};
use common::*;
use serde_json::Value as Json;

#[test]
fn round_trip_is_byte_identical() {
    let world = build_world(300, 42);
    let a = scene::save(&world).unwrap();
    let loaded = scene::load(&a, registry()).unwrap();
    let b = scene::save(&loaded).unwrap();
    assert_eq!(loaded.entity_count(), 300);
    assert!(a == b, "round trip changed the file");
    assert_eq!(a, scene::save(&world).unwrap(), "saving is deterministic");
}

#[test]
fn hierarchy_and_root_order_survive() {
    let mut world = build_world(300, 7);
    // Reorder roots: move the last root to the front.
    let last = *world.roots().last().unwrap();
    world
        .place(
            last,
            Placement {
                parent: None,
                index: 0,
            },
        )
        .unwrap();
    let loaded = scene::load(&scene::save(&world).unwrap(), registry()).unwrap();
    let uids = |w: &World, es: &[Entity]| es.iter().map(|e| w.uid(*e)).collect::<Vec<_>>();
    assert_eq!(uids(&world, world.roots()), uids(&loaded, loaded.roots()));
    for (e, uid) in world.query::<(Entity, &Uid)>().iter() {
        let le = loaded.entity_by_uid(*uid).unwrap();
        let p = world.parent(e).and_then(|p| world.uid(p));
        let lp = loaded.parent(le).and_then(|p| loaded.uid(p));
        assert_eq!(p, lp);
        assert_eq!(
            uids(&world, &world.children(e)),
            uids(&loaded, &loaded.children(le)),
            "child order"
        );
    }
}

#[test]
fn format_sample() {
    let mut world = build_world(3, 1);
    let e = world.entity_by_uid(uid(1, 0)).unwrap();
    world
        .insert_one(
            e,
            Primitive::new(Shape::Sphere {
                radius: 0.5,
                sectors: 16,
                stacks: 8,
            }),
        )
        .unwrap();
    let text = scene::save_string(&world).unwrap();
    println!("{text}");
    let v: Json = serde_json::from_str(&text).unwrap();
    let top: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
    assert_eq!(top, ["version", "resources", "entities"]);
    let first = &v["entities"][0];
    let keys: Vec<_> = first.as_object().unwrap().keys().cloned().collect();
    assert_eq!(keys, ["uid", "name", "parent", "components"]);
    assert_eq!(v["resources"]["Weather"]["fog"], 0.02);
    // Short numeric arrays are on one line.
    assert!(text.contains("\"scale\": ["), "{text}");
    assert!(!text.contains("\"scale\": [\n"), "{text}");
    // NotSaved components are skipped; derived data never appears.
    assert!(
        !text.contains("Cooldown")
            && !text.contains("GlobalTransform")
            && !text.contains("Children")
    );
    assert!(text.contains("\"Sphere\": {"), "{text}");
}

#[test]
fn not_saved_and_unregistered_are_skipped() {
    let mut world = World::with_registry(registry());
    let e = world
        .spawn_saved(Uid::from_u128(1), "a", (Cooldown(3.0), 7u8))
        .unwrap();
    world
        .insert_one(e, box_core::transform::GlobalTransform::default())
        .unwrap();
    let v: Json = serde_json::from_slice(&scene::save(&world).unwrap()).unwrap();
    assert_eq!(v["entities"][0]["components"], serde_json::json!({}));
    // An entity without a Uid is not saved; its saved child is, as a root.
    let mut world = World::with_registry(registry());
    let group = world.spawn((Transform::IDENTITY,));
    let child = world.spawn_saved(Uid::from_u128(2), "child", ()).unwrap();
    world.set_parent(child, Some(group)).unwrap();
    let v: Json = serde_json::from_slice(&scene::save(&world).unwrap()).unwrap();
    let entities = v["entities"].as_array().unwrap();
    assert_eq!(entities.len(), 1);
    assert_eq!(entities[0]["name"], "child");
    assert_eq!(entities[0]["parent"], Json::Null);
}

const WITH_UNKNOWN: &str = r#"{
  "version": 1,
  "resources": {
    "PluginSky": { "turbidity": 3, "list": [1, 2, 3, 4, 5, 6] },
    "Weather": { "fog": 0.5, "wind": [0.0, 0.0, 1.0] }
  },
  "entities": [
    {
      "uid": "00000000-0000-0000-0000-000000000001",
      "name": "Root",
      "parent": null,
      "components": {
        "AaaPluginThing": { "zeta": 1, "alpha": [true, null, "x"], "nested": { "b": 2.5, "a": 1e-7 } },
        "Health": { "current": 5.0, "max": 10.0 },
        "Waterfall": { "flow": 3 }
      }
    },
    {
      "uid": "00000000-0000-0000-0000-000000000002",
      "name": null,
      "parent": "00000000-0000-0000-0000-000000000001",
      "components": { "Zzz": null }
    }
  ]
}"#;

fn canonical(text: &str) -> Vec<u8> {
    scene::to_pretty_json(&serde_json::from_str::<Json>(text).unwrap()).unwrap()
}

#[test]
fn unknown_components_and_resources_are_preserved() {
    let canonical = canonical(WITH_UNKNOWN);
    let world = scene::load(&canonical, registry()).unwrap();
    let root = world.entity_by_uid(Uid::from_u128(1)).unwrap();
    let unknown = world.get::<UnknownComponents>(root).unwrap();
    assert_eq!(
        unknown.0.keys().collect::<Vec<_>>(),
        ["AaaPluginThing", "Waterfall"]
    );
    drop(unknown);
    assert_eq!(
        *world.get::<Health>(root).unwrap(),
        Health {
            current: 5.0,
            max: 10.0
        }
    );
    assert_eq!(world.resource::<Weather>().fog, 0.5);
    assert!(
        world
            .resource::<UnknownResources>()
            .0
            .contains_key("PluginSky")
    );
    let child = world.entity_by_uid(Uid::from_u128(2)).unwrap();
    assert!(world.get::<Name>(child).is_none());
    let out = scene::save(&world).unwrap();
    assert_eq!(
        String::from_utf8(out).unwrap(),
        String::from_utf8(canonical).unwrap()
    );
}

#[test]
fn play_clone_maps_uids() {
    let mut edit = build_world(300, 9);
    // Churn entity ids so that edit and play Entity values differ.
    for _ in 0..50 {
        let e = edit.spawn((1u8,));
        edit.despawn(e);
    }
    let edit_bytes = scene::save(&edit).unwrap();
    let mut play = scene::clone_world(&edit).unwrap();
    assert!(std::sync::Arc::ptr_eq(play.registry(), edit.registry()));
    assert_eq!(scene::save(&play).unwrap(), edit_bytes);
    let mut differing = 0;
    for (e, uid) in edit.query::<(Entity, &Uid)>().iter() {
        let p = play.entity_by_uid(*uid).unwrap();
        differing += usize::from(p != e);
        assert_eq!(play.uid(p), Some(*uid));
    }
    assert!(differing > 0, "test should exercise differing Entity ids");
    // Mutating play leaves edit untouched.
    let p = play.entity_by_uid(uid(9, 0)).unwrap();
    let path = play.field_path("Transform.translation.y").unwrap();
    play.set_field(p, &path, &99.0f32).unwrap();
    assert_eq!(scene::save(&edit).unwrap(), edit_bytes);
    assert_ne!(scene::save(&play).unwrap(), edit_bytes);
}

#[test]
fn version_checks() {
    let err = scene::load(br#"{"version": 99, "entities": []}"#, registry())
        .err()
        .unwrap();
    assert!(
        matches!(err, SceneError::NewerVersion { found: 99, .. }),
        "{err}"
    );
    assert!(err.to_string().contains("newer"));
    let err = scene::load(br#"{"version": 0, "entities": []}"#, registry())
        .err()
        .unwrap();
    assert!(matches!(
        err,
        SceneError::UnsupportedVersion { found: 0, .. }
    ));
    let err = scene::load(br#"{"entities": []}"#, registry())
        .err()
        .unwrap();
    assert!(matches!(err, SceneError::Format(_)));
}

#[test]
fn malformed_input_is_an_error_not_a_panic() {
    let cases = [
        "not json",
        r#"{"version": 1}"#,
        r#"{"version": 1, "entities": [5]}"#,
        r#"{"version": 1, "entities": [{"uid": "nope"}]}"#,
        r#"{"version": 1, "entities": [{"uid": "00000000-0000-0000-0000-000000000001", "parent": "00000000-0000-0000-0000-000000000009"}]}"#,
        r#"{"version": 1, "entities": [{"uid": "00000000-0000-0000-0000-000000000001"}, {"uid": "00000000-0000-0000-0000-000000000001"}]}"#,
        r#"{"version": 1, "entities": [{"uid": "00000000-0000-0000-0000-000000000001", "components": {"Health": {"current": "x"}}}]}"#,
        r#"{"version": 1, "entities": [{"uid": "00000000-0000-0000-0000-000000000001", "components": {"Uid": "x"}}]}"#,
        r#"{"version": 1, "resources": {"Weather": 3}, "entities": []}"#,
        r#"{"version": 1, "resources": [], "entities": []}"#,
    ];
    for c in cases {
        let err = scene::load(c.as_bytes(), registry())
            .err()
            .unwrap_or_else(|| panic!("accepted {c}"));
        println!("{err}");
    }
    let err = scene::load(cases[6].as_bytes(), registry()).err().unwrap();
    let msg = err.to_string();
    assert!(
        msg.contains("00000000-0000-0000-0000-000000000001") && msg.contains("Health"),
        "{msg}"
    );
}

#[test]
fn subtree_snapshot_and_instantiate() {
    let mut world = build_world(40, 3);
    let root = world.entity_by_uid(uid(3, 0)).unwrap();
    let records = scene::save_subtree(&world, root).unwrap();
    let subtree_len = records.len();
    assert!(subtree_len > 1, "fixture should give entity 0 children");
    // Paste a copy with fresh uids under the last root.
    let mut copy = records.clone();
    let map = scene::remap_uids(&mut copy);
    assert_eq!(map.len(), subtree_len);
    let target = world.uid(*world.roots().last().unwrap());
    let before = world.entity_count();
    let spawned = scene::instantiate(&mut world, &copy, Some(Placement::last(target))).unwrap();
    assert_eq!(world.entity_count(), before + subtree_len as u32);
    assert_eq!(world.parent(spawned[0]).and_then(|p| world.uid(p)), target);
    // References inside the copy point at the copy.
    let new_root = map[&uid(3, 0)];
    for (old, new) in &map {
        let o = world.entity_by_uid(*old).unwrap();
        let n = world.entity_by_uid(*new).unwrap();
        if *old != uid(3, 0) {
            let op = world.parent(o).and_then(|p| world.uid(p)).unwrap();
            let np = world.parent(n).and_then(|p| world.uid(p)).unwrap();
            assert_eq!(map[&op], np);
        }
    }
    assert!(world.entity_by_uid(new_root).is_some());
    // Instantiating the originals again fails (duplicate uids) and leaves nothing behind.
    let count = world.entity_count();
    assert!(scene::instantiate(&mut world, &records, None).is_err());
    assert_eq!(world.entity_count(), count);
}

/// A scene file may give only some fields of an enum variant: the rest come
/// from the enum's `VariantDefaults` (else the field type's default), and the
/// variant is kept instead of falling back to the type's default one.
#[test]
fn partial_enum_variants_load_with_variant_defaults() {
    let text = r#"{
  "version": 1,
  "entities": [
    {
      "uid": "00000000-0000-0000-0000-000000000001",
      "name": "Ball",
      "components": {
        "Primitive": { "shape": { "Sphere": { "radius": 0.3 } } },
        "Npc": { "behaviour": { "Patrol": { "waypoints": [[1.0, 0.0, 0.0]] } } }
      }
    },
    {
      "uid": "00000000-0000-0000-0000-000000000002",
      "name": "Pill",
      "components": { "Primitive": { "shape": "Capsule", "cast_shadows": false } }
    }
  ]
}"#;
    let world = scene::load(text.as_bytes(), registry()).unwrap();
    let ball = world.entity_by_uid(Uid::from_u128(1)).unwrap();
    assert_eq!(
        *world.get::<Primitive>(ball).unwrap(),
        Primitive::sphere(0.3)
    );
    assert_eq!(
        world.get::<Npc>(ball).unwrap().behaviour,
        Behaviour::Patrol {
            speed: 0.0,
            waypoints: vec![[1.0, 0.0, 0.0]]
        }
    );
    let pill = world.entity_by_uid(Uid::from_u128(2)).unwrap();
    assert_eq!(
        *world.get::<Primitive>(pill).unwrap(),
        Primitive::capsule(0.4, 0.4).with_shadows(false)
    );
    // Saving writes every field.
    let saved: Json = serde_json::from_slice(&scene::save(&world).unwrap()).unwrap();
    assert_eq!(
        saved["entities"][0]["components"]["Primitive"]["shape"]["Sphere"]["sectors"],
        32
    );
}
