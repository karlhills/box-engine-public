//! The game-facing conveniences: world helpers, `register_all`, readable
//! path errors, unknown scene types, range-checked commands, shorthand
//! constructors, cursor requests and the bare-`Parent` fallback.

use std::sync::Arc;

use box_core::color::Color;
use box_core::edit::{EditStack, apply_commands_with_warnings, parse_commands};
use box_core::identity::{Name, Uid};
use box_core::input::{CursorGrab, CursorOptions};
use box_core::material::StandardMaterial;
use box_core::primitive::{Primitive, Shape};
use box_core::reflect::{
    Range, Reflect, ReflectComponent, ReflectDefault, ReflectError, Registry, Unit, did_you_mean,
};
use box_core::scene;
use box_core::transform::{GlobalTransform, Parent, Transform, propagate_transforms};
use box_core::world::World;
use glam::{Vec2, Vec3};

/// Something to pick up.
#[derive(Reflect, Default, Clone)]
#[reflect(Component, Default, Clone)]
struct Coin {
    #[reflect(@Range { min: 0.0, max: 1000.0 })]
    points: u32,
    #[reflect(@Range { min: -20.0, max: 20.0 }, @Unit("rad/s"))]
    spin: f32,
}

#[derive(Reflect, Default, Clone)]
#[reflect(Component, Default, Clone)]
struct Door {
    open: bool,
}

mod other {
    use box_core::reflect::{Reflect, ReflectComponent, ReflectDefault};

    /// Same short name as the outer `Coin`.
    #[derive(Reflect, Default, Clone)]
    #[reflect(Component, Default, Clone)]
    pub struct Coin;
}

/// Not a component: no `#[reflect(Component)]`.
#[derive(Reflect, Default, Clone)]
struct Plain;

fn registry() -> Arc<Registry> {
    let mut r = Registry::with_core_types();
    assert!(r.register_all::<(Coin, Door)>().is_empty());
    Arc::new(r)
}

struct Score(u32);
struct Lives(u32);

#[test]
fn world_helpers_find_count_and_borrow() {
    let mut w = World::with_registry(registry());
    let a = w.spawn((Name::new("a"), Coin::default()));
    let b = w.spawn((Name::new("b"), Coin::default(), Door::default()));
    assert_eq!(w.find_by_name("b"), Some(b));
    assert_eq!(w.find_by_name("zzz"), None);
    assert_eq!(w.first_with::<Door>(), Some(b));
    assert_eq!(w.first_with::<Transform>(), None);
    assert_eq!(w.count::<&Coin>(), 2);
    assert_eq!(w.count::<(&Coin, &Door)>(), 1);

    w.component_mut::<Coin>(a).unwrap().points = 7;
    assert_eq!(w.get::<Coin>(a).unwrap().points, 7);
    assert!(w.component_mut::<Door>(a).is_none());

    w.insert_resource(Score(1));
    assert!(w.get_resources_mut::<(Score, Lives)>().is_none());
    w.insert_resource(Lives(3));
    let (score, lives) = w.resources_mut::<(Score, Lives)>();
    score.0 += 10;
    lives.0 -= 1;
    assert_eq!((w.resource::<Score>().0, w.resource::<Lives>().0), (11, 2));
}

#[test]
#[should_panic(expected = "missing resource")]
fn resources_mut_names_a_missing_resource() {
    let mut w = World::new();
    w.insert_resource(Score(0));
    let _ = w.resources_mut::<(Score, Lives)>();
}

#[test]
fn register_all_collects_errors_for_the_game_to_report() {
    let mut r = Registry::with_core_types();
    let errors = r.register_all::<(Coin, other::Coin, Plain, Door)>();
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(matches!(errors[0], ReflectError::DuplicateName { .. }));
    assert!(matches!(errors[1], ReflectError::NotAComponent(_)));
    assert!(r.contains("Coin") && r.contains("Door"));
    // Ignoring the result is fine: the messages are kept.
    let kept = r.take_errors();
    assert_eq!(kept.len(), 2);
    assert!(kept[0].contains("`Coin` is already registered"), "{kept:?}");
    assert!(r.take_errors().is_empty());
    let _ = r.register::<other::Coin>();
    assert_eq!(r.take_errors().len(), 1);
}

#[test]
fn did_you_mean_finds_likely_typos_only() {
    let names = ["Coin", "Collectible", "Transform"];
    assert_eq!(did_you_mean("Coinn", names), Some("Coin"));
    assert_eq!(did_you_mean("transfrom", names), Some("Transform"));
    assert_eq!(did_you_mean("Collectable", names), Some("Collectible"));
    assert_eq!(did_you_mean("Spaceship", names), None);
    assert_eq!(
        did_you_mean("Coin", names),
        None,
        "an exact name is no typo"
    );
}

#[test]
fn a_missing_field_lists_the_fields_and_a_guess() {
    let mut w = World::with_registry(registry());
    let e = w.spawn((Coin::default(),));
    let path = w.field_path("Coin.pionts").unwrap();
    let err = w.get_field(e, &path).err().unwrap().to_string();
    assert_eq!(
        err,
        "bad field path `Coin.pionts`: `Coin` has no field `pionts`; fields: points, spin \
         (did you mean `points`?)"
    );
    let value = 5u32;
    let err = w.set_field(e, &path, &value).err().unwrap().to_string();
    assert!(err.contains("did you mean `points`"), "{err}");
    // Nested: the struct that lacks the field is named.
    let t = w.spawn((Transform::default(),));
    let path = w.field_path("Transform.translation.q").unwrap();
    let err = w.get_field(t, &path).err().unwrap().to_string();
    assert!(
        err.contains("`Vec3` has no field `q`; fields: x, y, z"),
        "{err}"
    );
}

#[test]
fn unknown_scene_types_are_counted_with_a_guess() {
    let json = serde_json::json!({
        "version": 1,
        "resources": { "Wether": { "fog": 1.0 } },
        "entities": [
            { "uid": Uid::from_u128(1).to_string(), "name": "a", "components": { "Coinn": { "points": 1 } } },
            { "uid": Uid::from_u128(2).to_string(), "name": "b", "components": { "Coinn": {}, "Spaceship": {} } },
            { "uid": Uid::from_u128(3).to_string(), "name": "c", "components": { "Coin": { "points": 2, "spin": 0.0 } } }
        ]
    });
    let world = scene::load_value(&json, registry()).unwrap();
    let unknown = scene::unknown_types(&world);
    let summary: Vec<(&str, bool, usize, Option<&str>)> = unknown
        .iter()
        .map(|u| (u.name.as_str(), u.resource, u.entities, u.did_you_mean))
        .collect();
    assert_eq!(
        summary,
        vec![
            ("Coinn", false, 2, Some("Coin")),
            ("Spaceship", false, 1, None),
            ("Wether", true, 0, None),
        ]
    );
    let message = unknown[0].message("`main`");
    assert!(
        message.starts_with(
            "scene `main` uses unknown component `Coinn` on 2 entities (data kept); did you mean `Coin`?"
        ),
        "{message}"
    );
    assert!(message.contains("game's own player or editor"), "{message}");
    assert!(unknown[1].message("x").contains("on 1 entity "));

    let clean = scene::load_value(
        &serde_json::json!({ "version": 1, "entities": [] }),
        registry(),
    )
    .unwrap();
    assert!(scene::unknown_types(&clean).is_empty());
}

#[test]
fn commands_clamp_numbers_to_the_field_range_with_a_warning() {
    let mut w = World::with_registry(registry());
    let mut stack = EditStack::new();
    let commands = parse_commands(
        r#"[
            { "op": "spawn", "name": "Gem", "components": { "Coin": { "points": 1, "spin": 0.0 } } },
            { "op": "set", "entity": "Gem", "path": "Coin.points", "value": 5000 },
            { "op": "set", "entity": "Gem", "path": "Coin.spin", "value": -99.5 },
            { "op": "set", "entity": "Gem", "path": "Transform.translation.x", "value": 1e6 }
        ]"#,
    )
    .unwrap();
    let applied = apply_commands_with_warnings(&mut w, &mut stack, &commands, "t").unwrap();
    let e = w.find_by_name("Gem").unwrap();
    let coin = Coin::clone(&w.get::<Coin>(e).unwrap());
    assert_eq!((coin.points, coin.spin), (1000, -20.0));
    assert_eq!(
        w.get::<Transform>(e).unwrap().translation.x,
        1e6,
        "no range"
    );
    assert_eq!(applied.created.len(), 1);
    assert_eq!(applied.warnings.len(), 2, "{:?}", applied.warnings);
    assert_eq!(
        applied.warnings[0].message,
        "command 1 (set): `Coin.points` = 5000 is outside its range 0..=1000; clamped to 1000"
    );
    assert_eq!(applied.warnings[0].kind.code(), "value_clamped");
    assert!(
        applied.warnings[1].message.contains("clamped to -20.0"),
        "{:?}",
        applied.warnings
    );

    // A typo in a command's path reads well too.
    let bad =
        parse_commands(r#"{ "op": "set", "entity": "Gem", "path": "Coin.pionts", "value": 1 }"#)
            .unwrap();
    let err = apply_commands_with_warnings(&mut w, &mut stack, &bad, "t")
        .err()
        .unwrap()
        .to_string();
    assert!(err.contains("did you mean `points`"), "{err}");
}

#[test]
fn shorthand_constructors() {
    assert_eq!(
        Primitive::cuboid(Vec3::ONE).shape,
        Shape::Cuboid { size: Vec3::ONE }
    );
    assert!(matches!(
        Primitive::sphere(0.5).shape,
        Shape::Sphere {
            radius: 0.5,
            sectors: 32,
            stacks: 16
        }
    ));
    assert!(matches!(
        Primitive::capsule(0.4, 0.5).shape,
        Shape::Capsule {
            radius: 0.4,
            half_height: 0.5,
            ..
        }
    ));
    assert!(matches!(
        Primitive::cylinder(0.5, 2.0).shape,
        Shape::Cylinder {
            radius: 0.5,
            height: 2.0,
            ..
        }
    ));
    assert!(matches!(
        Primitive::plane(Vec2::splat(10.0)).shape,
        Shape::Plane { .. }
    ));
    assert!(!Primitive::sphere(1.0).with_shadows(false).cast_shadows);
    for p in [
        Primitive::sphere(0.5),
        Primitive::capsule(0.4, 0.5),
        Primitive::cylinder(0.5, 1.0),
        Primitive::plane(Vec2::ONE),
    ] {
        assert!(!p.mesh().indices.is_empty());
    }

    let gold = Color::hex(0xe8b04a);
    let m = StandardMaterial::color(gold)
        .with_metallic(1.0)
        .with_roughness(0.3);
    assert_eq!((m.base_color, m.metallic, m.roughness), (gold, 1.0, 0.3));
    let from: StandardMaterial = Color::RED.into();
    assert_eq!(from, StandardMaterial::color(Color::RED));
    assert_eq!(from.roughness, StandardMaterial::default().roughness);
}

#[test]
fn cursor_requests_keep_what_the_runner_applied() {
    let mut c = CursorOptions {
        active_grab: CursorGrab::Confined,
        ..CursorOptions::free()
    };
    c.lock();
    assert!(c.is_locked() && !c.visible);
    assert_eq!(c.active_grab, CursorGrab::Confined);
    c.release();
    assert!(!c.is_locked() && c.visible);
    assert_eq!(c.active_grab, CursorGrab::Confined);
}

#[test]
fn a_bare_parent_component_still_gets_a_world_transform() {
    let mut w = World::new();
    let parent = w.spawn((Transform::from_xyz(10.0, 0.0, 0.0),));
    let child = w.spawn((Transform::from_xyz(0.0, 1.0, 0.0),));
    // The mistake: Parent inserted directly instead of `set_parent`.
    w.insert_one(child, Parent(parent)).unwrap();
    propagate_transforms(&mut w);
    let g = *w.get::<GlobalTransform>(child).expect("a world transform");
    assert_eq!(g.translation(), Vec3::new(10.0, 1.0, 0.0));
    assert_eq!(g.affine(), g.0);

    // Through `set_parent` it is linked and follows its parent.
    let linked = w.spawn((Transform::from_xyz(0.0, 2.0, 0.0),));
    w.set_parent(linked, Some(parent)).unwrap();
    propagate_transforms(&mut w);
    w.component_mut::<Transform>(parent).unwrap().translation.x = 20.0;
    propagate_transforms(&mut w);
    let g = w.get::<GlobalTransform>(linked).unwrap().translation();
    assert_eq!(g, Vec3::new(20.0, 2.0, 0.0));
}
