//! JSON commands (`box scene apply`) through the edit stack.

mod common;

use box_core::edit::{Command, EditError, EditStack, apply_commands, parse_commands};
use box_core::identity::{Name, Uid};
use box_core::primitive::{Primitive, Shape};
use box_core::scene;
use box_core::transform::Transform;
use box_core::world::World;
use common::*;

const SCRIPT: &str = r#"[
  { "op": "spawn", "name": "Room", "uid": "00000000-0000-0000-0000-00000000aaaa" },
  { "op": "spawn", "name": "Lamp", "parent": "Room",
    "components": {
      "Transform": { "translation": [0.0, 2.0, 0.0], "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [1.0, 1.0, 1.0] },
      "Primitive": { "shape": { "Sphere": { "radius": 0.2, "sectors": 16, "stacks": 8 } }, "cast_shadows": false },
      "PointLight": { "color": [1.0, 0.9, 0.8], "intensity": 1200.0, "radius": 6.0 }
    } },
  { "op": "set", "entity": "Lamp", "path": "Transform.translation.y", "value": 2.5 },
  { "op": "set", "entity": "Lamp", "path": "PointLight.color", "value": [0.5, 0.5, 0.5] },
  { "op": "set", "entity": "Lamp", "path": "Primitive.shape", "value": { "Cuboid": { "size": [1.0, 2.0, 3.0] } } },
  { "op": "add_component", "entity": "Lamp", "component": "Health" },
  { "op": "add_component", "entity": "Room", "component": "Npc", "value": { "behaviour": "Idle", "tags": ["x"], "target": null, "loot_table": null } },
  { "op": "set", "entity": "Room", "path": "Npc.target", "value": "00000000-0000-0000-0000-00000000aaaa" },
  { "op": "remove_component", "entity": "Lamp", "component": "Health" },
  { "op": "rename", "entity": "Lamp", "name": "Ceiling lamp" },
  { "op": "duplicate", "entity": "Ceiling lamp" },
  { "op": "set_resource", "path": "Weather.fog", "value": 0.25 },
  { "op": "reparent", "entity": "00000000-0000-0000-0000-00000000aaaa", "parent": "Entity 0", "index": 0 }
]"#;

#[test]
fn script_applies_as_one_undo_step() {
    let mut w = build_world(4, 21);
    let mut stack = EditStack::new();
    let before = scene::save(&w).unwrap();
    let commands = parse_commands(SCRIPT).unwrap();
    assert_eq!(commands.len(), 13);
    let created = apply_commands(&mut w, &mut stack, &commands, "Apply script").unwrap();
    assert_eq!(created.len(), 3, "two spawns and a duplicate");
    assert_eq!(
        created[0],
        Uid::parse("00000000-0000-0000-0000-00000000aaaa").unwrap()
    );

    let lamp = w.entity_by_uid(created[1]).unwrap();
    assert_eq!(w.get::<Name>(lamp).unwrap().as_str(), "Ceiling lamp");
    assert_eq!(w.get::<Transform>(lamp).unwrap().translation.y, 2.5);
    assert_eq!(w.get::<PointLight>(lamp).unwrap().color, [0.5, 0.5, 0.5]);
    assert_eq!(
        w.get::<Primitive>(lamp).unwrap().shape,
        Shape::Cuboid {
            size: box_core::glam::Vec3::new(1.0, 2.0, 3.0)
        }
    );
    assert!(!w.has::<Health>(lamp));
    let room = w.entity_by_uid(created[0]).unwrap();
    assert_eq!(w.get::<Npc>(room).unwrap().target, Some(created[0]));
    assert_eq!(w.parent(lamp), Some(room));
    assert_eq!(w.children(room).len(), 2, "lamp and its duplicate");
    assert_eq!(w.parent(room).and_then(|p| w.uid(p)), Some(uid(21, 0)));
    assert_eq!(w.resource::<Weather>().fog, 0.25);

    assert_eq!(stack.undo_len(), 1);
    assert_eq!(stack.undo_label(), Some("Apply script"));
    let after = scene::save(&w).unwrap();
    stack.undo(&mut w).unwrap();
    assert_eq!(scene::save(&w).unwrap(), before);
    stack.redo(&mut w).unwrap();
    assert_eq!(scene::save(&w).unwrap(), after);
}

#[test]
fn failure_rolls_back_and_names_entity_and_path() {
    let mut w = build_world(4, 22);
    let mut stack = EditStack::new();
    let before = scene::save(&w).unwrap();
    let commands = parse_commands(
        r#"[
          { "op": "spawn", "name": "Lamp" },
          { "op": "set", "entity": "Lamp", "path": "Transform.translation.y", "value": 1.0 },
          { "op": "set", "entity": "Lamp", "path": "Transform.translaton.y", "value": 1.0 }
        ]"#,
    )
    .unwrap();
    let err = apply_commands(&mut w, &mut stack, &commands, "x").unwrap_err();
    let msg = err.to_string();
    println!("{msg}");
    assert_eq!((err.index, err.op), (2, "set"));
    assert!(
        msg.contains("entity `Lamp`") && msg.contains("Transform.translaton.y"),
        "{msg}"
    );
    assert_eq!(scene::save(&w).unwrap(), before, "rolled back");
    assert_eq!(stack.undo_len(), 0);
}

#[test]
fn error_cases() {
    let mut w = build_world(4, 23);
    let mut stack = EditStack::new();
    // Two entities with the same name.
    let dup =
        parse_commands(r#"[{"op":"spawn","name":"Twin"},{"op":"spawn","name":"Twin"}]"#).unwrap();
    apply_commands(&mut w, &mut stack, &dup, "twins").unwrap();
    type Check = fn(&EditError) -> bool;
    let cases: [(&str, Check); 7] = [
        (r#"{"op":"despawn","entity":"Twin"}"#, |e| {
            matches!(e, EditError::AmbiguousName { count: 2, .. })
        }),
        (r#"{"op":"despawn","entity":"Nobody"}"#, |e| {
            matches!(e, EditError::UnknownEntity(_))
        }),
        (
            r#"{"op":"despawn","entity":"00000000-0000-0000-0000-000000000099"}"#,
            |e| matches!(e, EditError::NoEntity(_)),
        ),
        (
            r#"{"op":"set","entity":"Entity 0","path":"Transform.translation.x","value":"high"}"#,
            |e| matches!(e, EditError::Reflect(_)),
        ),
        (
            r#"{"op":"set","entity":"Entity 0","path":"Weather.fog","value":1.0}"#,
            |e| matches!(e, EditError::Invalid(_)),
        ),
        (
            r#"{"op":"add_component","entity":"Entity 0","component":"Bogus"}"#,
            |e| matches!(e, EditError::Reflect(_)),
        ),
        (
            r#"{"op":"reparent","entity":"Entity 0","parent":"Entity 0"}"#,
            |e| matches!(e, EditError::Reflect(_)),
        ),
    ];
    for (json, check) in cases {
        let cmds = parse_commands(json).unwrap();
        let err = apply_commands(&mut w, &mut stack, &cmds, "x").unwrap_err();
        println!("{err}");
        assert!(check(&err.source), "{json}: {err}");
    }
}

#[test]
fn parse_errors_are_reported_per_command() {
    assert!(parse_commands("[").is_err());
    let err =
        parse_commands(r#"[{"op":"spawn","name":"a"},{"op":"sett","entity":"a"}]"#).unwrap_err();
    assert!(err.to_string().starts_with("command 1:"), "{err}");
    let err = parse_commands(r#"{"op":"rename","entity":"a","name":"b","extra":1}"#).unwrap_err();
    assert!(err.to_string().contains("extra"), "{err}");
    let single = parse_commands(r#"{"op":"despawn","entity":"a"}"#).unwrap();
    assert_eq!(single, [Command::Despawn { entity: "a".into() }]);
}

#[test]
fn commands_work_on_a_core_only_world() {
    let mut w = World::new();
    let mut stack = EditStack::new();
    let cmds = parse_commands(
        r#"[{"op":"spawn","name":"Box","components":{"Primitive":{"shape":{"Cuboid":{"size":[2.0,2.0,2.0]}},"cast_shadows":true}}},
            {"op":"set","entity":"Box","path":"Primitive.shape.size.y","value":4.0}]"#,
    )
    .unwrap();
    let created = apply_commands(&mut w, &mut stack, &cmds, "Box").unwrap();
    let e = w.entity_by_uid(created[0]).unwrap();
    let mesh = w.get::<Primitive>(e).unwrap().mesh();
    assert!((mesh.bounds().max.y - 2.0).abs() < 1e-5);
}

#[test]
fn reparent_command_keeps_world_position_unless_keep_local() {
    use box_core::glam::Vec3;
    let mut w = World::with_registry(registry());
    let mut stack = EditStack::new();
    let script = r#"[
      { "op": "spawn", "name": "Shelf", "components": { "Transform": { "translation": [5.0, 1.0, 0.0], "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [2.0, 2.0, 2.0] } } },
      { "op": "spawn", "name": "Cup", "components": { "Transform": { "translation": [1.0, 3.0, 0.0], "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [1.0, 1.0, 1.0] } } },
      { "op": "spawn", "name": "Plate", "components": { "Transform": { "translation": [1.0, 3.0, 0.0], "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [1.0, 1.0, 1.0] } } },
      { "op": "reparent", "entity": "Cup", "parent": "Shelf" },
      { "op": "reparent", "entity": "Plate", "parent": "Shelf", "keep": "local" }
    ]"#;
    let commands = parse_commands(script).unwrap();
    let created = apply_commands(&mut w, &mut stack, &commands, "Shelve").unwrap();
    let global = |u: Uid| {
        let e = w.entity_by_uid(u).unwrap();
        Vec3::from(box_core::transform::compute_global(&w, e).translation)
    };
    assert!((global(created[1]) - Vec3::new(1.0, 3.0, 0.0)).length() < 1e-5);
    let cup = w.entity_by_uid(created[1]).unwrap();
    assert_eq!(
        *w.get::<Transform>(cup).unwrap(),
        Transform::from_xyz(-2.0, 1.0, 0.0).with_scale(Vec3::splat(0.5))
    );
    assert!((global(created[2]) - Vec3::new(7.0, 7.0, 0.0)).length() < 1e-5);
    assert_eq!(stack.undo_len(), 1);
    assert!(parse_commands(r#"{"op":"reparent","entity":"Cup","keep":"sideways"}"#).is_err());
}

/// A spawn may give only some fields of an enum variant (the rest take that
/// variant's defaults): `{ "Sphere": { "radius": 0.5 } }` makes a sphere,
/// as `world.spawn` in Luau does, never the default variant.
#[test]
fn spawn_with_a_partial_enum_variant_keeps_the_variant() {
    let mut w = build_world(1, 3);
    let mut stack = EditStack::new();
    let commands = parse_commands(
        r#"[
      { "op": "spawn", "name": "Ball",
        "components": { "Primitive": { "shape": { "Sphere": { "radius": 0.5 } } } } },
      { "op": "spawn", "name": "Box",
        "components": { "Primitive": { "shape": { "Cuboid": {} } } } }
    ]"#,
    )
    .unwrap();
    let created = apply_commands(&mut w, &mut stack, &commands, "Spawn").unwrap();
    let ball = w.entity_by_uid(created[0]).unwrap();
    match w.get::<Primitive>(ball).unwrap().shape {
        Shape::Sphere { radius, .. } => assert_eq!(radius, 0.5),
        ref other => panic!("expected a sphere, got {other:?}"),
    }
    let boxed = w.entity_by_uid(created[1]).unwrap();
    assert!(matches!(
        w.get::<Primitive>(boxed).unwrap().shape,
        Shape::Cuboid { .. }
    ));
}
