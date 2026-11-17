//! Registry, field metadata and generic get/set/add/remove by name and path.

use bevy_reflect::structs::DynamicStruct;
use box_core::world::World;
use glam::{Quat, Vec3};
use s2_reflection::components::*;
use s2_reflection::math::Vec3Def;
use s2_reflection::{Error, default_registry, meta, ops};

fn world_with_lamp() -> (World, Uid) {
    let mut w = World::new();
    let uid = Uid::from_u128(7);
    let e = ops::spawn_with_uid(&mut w, uid, Some("Lamp"));
    w.insert(
        e,
        (
            Transform {
                translation: Vec3::new(1.0, 2.0, 3.0),
                ..Default::default()
            },
            PointLight::default(),
        ),
    )
    .unwrap();
    (w, uid)
}

#[test]
fn registry_names_are_stable_short_paths() {
    let r = default_registry();
    let names: Vec<_> = r.components().map(|c| c.name).collect();
    assert_eq!(
        names,
        ["Health", "Name", "Npc", "PointLight", "Transform", "Uid"]
    );
    assert_eq!(
        r.component("PointLight")
            .unwrap()
            .registration
            .type_info()
            .type_path(),
        "box_engine::PointLight"
    );
    assert_eq!(
        r.component("Health")
            .unwrap()
            .registration
            .type_info()
            .type_path(),
        "game::Health"
    );
}

#[test]
fn duplicate_short_name_is_rejected() {
    mod other {
        use bevy_reflect::Reflect;
        use s2_reflection::registry::ReflectComponent;
        #[derive(Reflect, Default)]
        #[reflect(Component)]
        #[type_path = "other_plugin"]
        pub struct Health(pub u32);
    }
    let mut r = default_registry();
    let err = r.register_component::<other::Health>().unwrap_err();
    assert!(matches!(err, Error::DuplicateName(n) if n == "Health"));
}

#[test]
fn field_metadata_reads_back() {
    let r = default_registry();
    let m = meta::field(&r, "PointLight.intensity").unwrap();
    assert_eq!(m.type_path, "f32");
    assert_eq!(m.doc.as_deref(), Some("Luminous power."));
    assert_eq!(
        m.range,
        Some(meta::Range {
            min: 0.0,
            max: 100_000.0
        })
    );
    assert_eq!(m.unit, Some("lm"));
    assert_eq!(m.step, Some(10.0));

    let t = meta::field(&r, "Transform.translation.x").unwrap();
    assert_eq!(t.unit, Some("m"));
    assert_eq!(
        t.doc.as_deref(),
        Some("Position relative to the parent, in metres.")
    );
    // The remote wrapper is what reflection sees as the field type.
    assert_eq!(t.type_path, "s2_reflection::math::Vec3Def");

    assert_eq!(
        meta::component_doc(&r, "PointLight").as_deref(),
        Some("An omnidirectional light.")
    );
    let rows: Vec<_> = meta::component_fields(&r, "PointLight")
        .unwrap()
        .into_iter()
        .map(|f| f.name)
        .collect();
    assert_eq!(rows, ["color", "intensity", "radius"]);
}

#[test]
fn list_components_on_entity() {
    let r = default_registry();
    let (w, uid) = world_with_lamp();
    let e = ops::entity(&w, uid).unwrap();
    assert_eq!(
        ops::list_components(&w, &r, e),
        ["Name", "PointLight", "Transform", "Uid"]
    );
}

#[test]
fn get_and_set_by_path() {
    let r = default_registry();
    let (mut w, uid) = world_with_lamp();

    let v = ops::get_field(&w, &r, uid, "PointLight.intensity").unwrap();
    assert_eq!(v.try_downcast_ref::<f32>(), Some(&800.0));
    ops::set_field(&mut w, &r, uid, "PointLight.intensity", &1234.5f32).unwrap();
    ops::set_field(&mut w, &r, uid, "Transform.translation.x", &-4.0f32).unwrap();
    ops::set_field(&mut w, &r, uid, "PointLight.color[1]", &0.25f32).unwrap();
    // Whole remote field: the value must be the wrapper type, not glam::Vec3.
    ops::set_field(
        &mut w,
        &r,
        uid,
        "Transform.scale",
        &Vec3Def(Vec3::splat(2.0)),
    )
    .unwrap();
    ops::set_field(&mut w, &r, uid, "Transform.rotation.w", &0.5f32).unwrap();

    let e = ops::entity(&w, uid).unwrap();
    let light = w.get::<PointLight>(e).unwrap();
    assert_eq!(light.intensity, 1234.5);
    assert_eq!(light.color, [1.0, 0.25, 1.0]);
    let t = *w.get::<Transform>(e).unwrap();
    assert_eq!(t.translation, Vec3::new(-4.0, 2.0, 3.0));
    assert_eq!(t.scale, Vec3::splat(2.0));
    assert_eq!(t.rotation, Quat::from_xyzw(0.0, 0.0, 0.0, 0.5));
}

#[test]
fn set_whole_component_from_dynamic_value() {
    let r = default_registry();
    let (mut w, uid) = world_with_lamp();
    // What a script or the deserializer would hand us: a dynamic struct.
    let mut d = DynamicStruct::default();
    d.insert("current", 5.0f32);
    d.insert("max", 50.0f32);
    d.set_represented_type(Some(<Health as bevy_reflect::Typed>::type_info()));
    ops::add_default(&mut w, &r, uid, "Health").unwrap();
    ops::set_field(&mut w, &r, uid, "Health", &d).unwrap();
    let e = ops::entity(&w, uid).unwrap();
    assert_eq!(
        *w.get::<Health>(e).unwrap(),
        Health {
            current: 5.0,
            max: 50.0
        }
    );
}

#[test]
fn enum_vec_option_fields() {
    let r = default_registry();
    let (mut w, uid) = world_with_lamp();
    ops::add_default(&mut w, &r, uid, "Npc").unwrap();
    let patrol = Behaviour::Patrol {
        speed: 2.0,
        waypoints: vec![[1.0, 0.0, 0.0]],
    };
    ops::set_field(&mut w, &r, uid, "Npc.behaviour", &patrol).unwrap();
    // Path into an enum variant field, then into a Vec element.
    ops::set_field(&mut w, &r, uid, "Npc.behaviour.speed", &3.0f32).unwrap();
    ops::set_field(&mut w, &r, uid, "Npc.behaviour.waypoints[0][2]", &9.0f32).unwrap();
    ops::set_field(
        &mut w,
        &r,
        uid,
        "Npc.tags",
        &vec!["a".to_string(), "b".to_string()],
    )
    .unwrap();
    // Shrinking a Vec works because set_field replaces instead of applying.
    ops::set_field(&mut w, &r, uid, "Npc.tags", &vec!["c".to_string()]).unwrap();
    ops::set_field(&mut w, &r, uid, "Npc.target", &Some(Uid::from_u128(9))).unwrap();
    ops::set_field(&mut w, &r, uid, "Npc.loot_table", &Option::<String>::None).unwrap();
    let e = ops::entity(&w, uid).unwrap();
    let npc = w.get::<Npc>(e).unwrap().clone();
    assert_eq!(
        npc.behaviour,
        Behaviour::Patrol {
            speed: 3.0,
            waypoints: vec![[1.0, 0.0, 9.0]]
        }
    );
    assert_eq!(npc.tags, ["c"]);
    assert_eq!(npc.target, Some(Uid::from_u128(9)));
}

#[test]
fn add_and_remove_by_name() {
    let r = default_registry();
    let (mut w, uid) = world_with_lamp();
    ops::add_default(&mut w, &r, uid, "Health").unwrap();
    assert!(matches!(
        ops::add_default(&mut w, &r, uid, "Health"),
        Err(Error::AlreadyPresent(_))
    ));
    let removed = ops::remove(&mut w, &r, uid, "PointLight").unwrap();
    assert_eq!(
        removed.downcast_ref::<PointLight>(),
        Some(&PointLight::default())
    );
    let e = ops::entity(&w, uid).unwrap();
    assert_eq!(
        ops::list_components(&w, &r, e),
        ["Health", "Name", "Transform", "Uid"]
    );
    assert!(matches!(
        ops::add_default(&mut w, &r, uid, "Nope"),
        Err(Error::UnknownComponent(_))
    ));
}

/// Records what bevy_reflect's errors look like (printed with --nocapture).
#[test]
fn error_messages() {
    let r = default_registry();
    let (mut w, uid) = world_with_lamp();
    let errs = [
        ops::set_field(&mut w, &r, uid, "PointLight.intensty", &1.0f32).unwrap_err(),
        ops::set_field(&mut w, &r, uid, "PointLight.intensity", &1.0f64).unwrap_err(),
        ops::set_field(
            &mut w,
            &r,
            uid,
            "Transform.translation",
            &[1.0f32, 2.0, 3.0],
        )
        .unwrap_err(),
        ops::set_field(&mut w, &r, uid, "PointLight.color[7]", &1.0f32).unwrap_err(),
        ops::set_field(&mut w, &r, uid, "PointLight.color..", &1.0f32).unwrap_err(),
    ];
    for e in &errs {
        println!("{e}");
    }
    assert!(matches!(errs[1], Error::Shape { .. }));
}

/// A remote wrapper reports the wrapper's type path but downcasts to the remote type.
#[test]
fn remote_wrapper_downcasts_to_remote_type() {
    let r = default_registry();
    let (w, uid) = world_with_lamp();
    let v = ops::get_field(&w, &r, uid, "Transform.translation").unwrap();
    assert_eq!(v.reflect_type_path(), "s2_reflection::math::Vec3Def");
    let v = v.try_into_reflect().unwrap();
    assert!(!v.is::<Vec3Def>());
    assert_eq!(v.downcast_ref::<Vec3>(), Some(&Vec3::new(1.0, 2.0, 3.0)));
}
