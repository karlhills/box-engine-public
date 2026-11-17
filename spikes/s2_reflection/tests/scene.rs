//! Scene JSON round trip, unknown component preservation, Play-mode clone.

use box_core::world::Entity;
use s2_reflection::components::*;
use s2_reflection::fixtures::build_world;
use s2_reflection::scene::{self, UnknownComponents};
use s2_reflection::{default_registry, ops};

#[test]
fn round_trip_is_byte_identical() {
    let r = default_registry();
    let world = build_world(300, 42);
    let a = scene::save(&world, &r).unwrap();
    let loaded = scene::load(&a, &r).unwrap();
    let b = scene::save(&loaded, &r).unwrap();
    assert_eq!(loaded.entity_count(), 300);
    assert!(a == b, "round trip changed the file");
    // Saving is deterministic for the same world, too.
    assert_eq!(a, scene::save(&world, &r).unwrap());
}

#[test]
fn hierarchy_survives() {
    let r = default_registry();
    let world = build_world(300, 7);
    let loaded = scene::load(&scene::save(&world, &r).unwrap(), &r).unwrap();
    for (e, uid) in world.query::<(Entity, &Uid)>().iter() {
        let le = ops::entity(&loaded, *uid).unwrap();
        let p = world.parent(e).and_then(|p| ops::uid_of(&world, p));
        let lp = loaded.parent(le).and_then(|p| ops::uid_of(&loaded, p));
        assert_eq!(p, lp);
        let c: Vec<_> = world
            .children(e)
            .iter()
            .map(|c| ops::uid_of(&world, *c))
            .collect();
        let lc: Vec<_> = loaded
            .children(le)
            .iter()
            .map(|c| ops::uid_of(&loaded, *c))
            .collect();
        assert_eq!(c, lc, "child order");
    }
}

#[test]
fn format_sample() {
    let r = default_registry();
    let world = build_world(3, 1);
    let text = scene::save_string(&world, &r).unwrap();
    println!("{text}");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    let first = &v["entities"][0];
    let keys: Vec<_> = first.as_object().unwrap().keys().cloned().collect();
    assert_eq!(keys, ["uid", "name", "parent", "components"]);
    assert!(first["components"]["Transform"]["translation"].is_array());
}

const WITH_UNKNOWN: &str = r#"{
  "version": 1,
  "entities": [
    {
      "uid": "00000000-0000-0000-0000-000000000001",
      "name": "Root",
      "parent": null,
      "components": {
        "AaaPluginThing": {
          "zeta": 1,
          "alpha": [true, null, "x"],
          "nested": { "b": 2.5, "a": 1e-7 }
        },
        "Health": {
          "current": 5.0,
          "max": 10.0
        },
        "Waterfall": {
          "flow": 3
        }
      }
    },
    {
      "uid": "00000000-0000-0000-0000-000000000002",
      "name": null,
      "parent": "00000000-0000-0000-0000-000000000001",
      "components": {
        "Zzz": null
      }
    }
  ]
}"#;

#[test]
fn unknown_components_are_preserved() {
    let r = default_registry();
    // Normalise through serde_json's own pretty printer so that number
    // formatting (1e-7 -> 1e-7) and whitespace match what save() writes.
    let canonical = serde_json::to_vec_pretty(
        &serde_json::from_str::<serde_json::Value>(WITH_UNKNOWN).unwrap(),
    )
    .unwrap();
    let world = scene::load(&canonical, &r).unwrap();
    let root = ops::entity(&world, Uid::from_u128(1)).unwrap();
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
    let out = scene::save(&world, &r).unwrap();
    assert_eq!(
        String::from_utf8(out).unwrap(),
        String::from_utf8(canonical).unwrap()
    );
}

#[test]
fn play_clone_maps_uids() {
    let r = default_registry();
    let mut edit = build_world(300, 9);
    // Churn entity ids so that edit and play Entity values differ.
    for _ in 0..50 {
        let e = edit.spawn((1u8,));
        edit.despawn(e);
    }
    let edit_bytes = scene::save(&edit, &r).unwrap();
    let mut play = scene::clone_world(&edit, &r).unwrap();
    assert_eq!(scene::save(&play, &r).unwrap(), edit_bytes);

    let mut differing = 0;
    for (e, uid) in edit.query::<(Entity, &Uid)>().iter() {
        let p = ops::entity(&play, *uid).unwrap();
        differing += usize::from(p != e);
        assert_eq!(ops::uid_of(&play, p), Some(*uid));
    }
    assert!(differing > 0, "test should exercise differing Entity ids");

    // Mutating play leaves edit untouched.
    let uid = Uid::from_u128((9u128 << 64) | 1);
    ops::set_field(&mut play, &r, uid, "Transform.translation.y", &99.0f32).unwrap();
    assert_eq!(scene::save(&edit, &r).unwrap(), edit_bytes);
    assert_ne!(scene::save(&play, &r).unwrap(), edit_bytes);
    let _ = &mut edit;
}

#[test]
fn version_mismatch_is_an_error() {
    let r = default_registry();
    let err = scene::load(br#"{"version": 99, "entities": []}"#, &r)
        .err()
        .unwrap();
    assert!(err.to_string().contains("unsupported version 99"));
}

#[test]
fn enum_option_vec_format() {
    let r = default_registry();
    let mut w = box_core::world::World::new();
    let e = ops::spawn_with_uid(&mut w, Uid::from_u128(1), None);
    let npc = Npc {
        behaviour: Behaviour::Patrol {
            speed: 2.5,
            waypoints: vec![[1.0, 0.0, 0.0]],
        },
        tags: vec!["a".into()],
        target: Some(Uid::from_u128(2)),
        loot_table: None,
    };
    w.insert(e, (npc.clone(), Health::default())).unwrap();
    let text = scene::save_string(&w, &r).unwrap();
    println!("{text}");
    let back = scene::load(text.as_bytes(), &r).unwrap();
    let be = ops::entity(&back, Uid::from_u128(1)).unwrap();
    assert_eq!(*back.get::<Npc>(be).unwrap(), npc);
    for b in [
        Behaviour::Idle,
        Behaviour::Follow(None),
        Behaviour::Follow(Some(Uid::from_u128(3))),
    ] {
        let mut n = npc.clone();
        n.behaviour = b;
        w.insert_one(e, n.clone()).unwrap();
        let back = scene::load(&scene::save(&w, &r).unwrap(), &r).unwrap();
        assert_eq!(
            *back
                .get::<Npc>(ops::entity(&back, Uid::from_u128(1)).unwrap())
                .unwrap(),
            n
        );
    }
}
