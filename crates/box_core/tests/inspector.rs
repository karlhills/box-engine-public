//! Registry, field metadata, registry dump and generic get/set/add/remove by name and path.

mod common;

use box_core::glam::{Quat, Vec3};
use box_core::identity::{Name, Uid};
use box_core::reflect::bevy_reflect::structs::DynamicStruct;
use box_core::reflect::{FieldPath, Range, ReflectError, Registry, Typed};
use box_core::transform::Transform;
use box_core::world::World;
use common::*;

fn world_with_lamp() -> (World, Uid) {
    let mut w = World::with_registry(registry());
    let uid = Uid::from_u128(7);
    let t = Transform::from_xyz(1.0, 2.0, 3.0);
    w.spawn_saved(uid, "Lamp", (t, PointLight::default()))
        .unwrap();
    (w, uid)
}

fn path(w: &World, p: &str) -> FieldPath {
    w.field_path(p).unwrap()
}

fn set(
    w: &mut World,
    uid: Uid,
    p: &str,
    v: &dyn box_core::reflect::PartialReflect,
) -> Result<(), ReflectError> {
    let e = w.entity_by_uid(uid).unwrap();
    let p = w.field_path(p)?;
    w.set_field(e, &p, v)
}

#[test]
fn registry_names_are_stable_short_paths() {
    let r = registry();
    let names: Vec<_> = r.components().map(|c| c.name()).collect();
    assert_eq!(
        names,
        [
            "AnimationPlayer",
            "AnimationStateMachine",
            "Cooldown",
            "Health",
            "Interpolated",
            "MaterialRef",
            "MeshAsset",
            "Name",
            "Npc",
            "PointLight",
            "PrefabInstance",
            "Primitive",
            "ShaderMaterial",
            "SkinBinding",
            "StandardMaterial",
            "Transform",
            "Transient",
            "Uid",
            "Visible"
        ]
    );
    assert_eq!(
        r.component("Transform").unwrap().type_path(),
        "box_engine::Transform"
    );
    assert_eq!(r.component("Health").unwrap().type_path(), "game::Health");
    let res: Vec<_> = r.resources().map(|c| c.name()).collect();
    assert_eq!(res, ["Weather"]);
    // Components and resources do not answer for each other.
    assert!(r.component("Weather").is_none() && r.resource("Health").is_none());
    assert!(r.component("Uid").unwrap().is_hidden());
    assert!(!r.component("Cooldown").unwrap().is_saved());
    assert!(r.component("Transform").unwrap().is_saved());
}

#[test]
fn duplicate_short_name_is_rejected() {
    mod other {
        use box_core::reflect::{Reflect, ReflectComponent};
        #[derive(Reflect, Default)]
        #[reflect(Component)]
        #[type_path = "other_plugin"]
        pub struct Health(pub u32);

        #[derive(Reflect, Default)]
        #[type_path = "other_plugin"]
        pub struct NotComponent(pub u32);
    }
    let mut r = Registry::with_core_types();
    r.register::<Health>().unwrap();
    r.register::<Health>().unwrap(); // idempotent
    let err = r.register::<other::Health>().unwrap_err();
    assert!(
        matches!(&err, ReflectError::DuplicateName { name, .. } if name == "Health"),
        "{err}"
    );
    println!("{err}");
    // Resources share the namespace.
    assert!(matches!(
        r.register_resource::<Health>(),
        Err(ReflectError::DuplicateName { .. })
    ));
    let err = r.register::<other::NotComponent>().unwrap_err();
    assert!(matches!(err, ReflectError::NotAComponent(_)));
}

#[test]
fn field_metadata_reads_back() {
    let w = World::with_registry(registry());
    let r = w.registry();
    let m = r.field_meta(&path(&w, "PointLight.intensity")).unwrap();
    assert_eq!(m.type_path, "f32");
    assert_eq!(m.doc, Some("Luminous power."));
    assert_eq!(
        m.range,
        Some(Range {
            min: 0.0,
            max: 100_000.0
        })
    );
    assert_eq!(m.unit, Some("lm"));
    assert_eq!(m.step, Some(10.0));

    // Nested fields inherit unit and doc from their parent field.
    let t = r.field_meta(&path(&w, "Transform.translation.x")).unwrap();
    assert_eq!(
        (t.name.as_ref(), t.type_path, t.unit),
        ("x", "f32", Some("m"))
    );
    assert_eq!(t.doc, Some("Position relative to the parent."));
    let t = r.field_meta(&path(&w, "Transform.translation")).unwrap();
    assert_eq!(t.type_path, "glam::Vec3");

    // Fields inside enum variants.
    let s = r.field_meta(&path(&w, "Primitive.shape.radius")).unwrap();
    assert_eq!((s.unit, s.range.map(|r| r.min)), (Some("m"), Some(0.001)));
    let s = r
        .field_meta(&path(&w, "Npc.behaviour.waypoints[0]"))
        .unwrap();
    assert_eq!(s.type_path, "[f32; 3]");
    assert!(r.field_meta(&path(&w, "PointLight.nope")).is_none());

    let light = r.component("PointLight").unwrap();
    assert_eq!(light.doc(), Some("An omnidirectional light."));
    let rows: Vec<_> = light.fields().into_iter().map(|f| f.name).collect();
    assert_eq!(rows, ["color", "intensity", "radius"]);
    let name_rows = r.component("Name").unwrap().fields();
    assert_eq!(
        (name_rows[0].name.as_ref(), name_rows[0].type_path),
        ("0", "alloc::string::String")
    );
    assert!(r.component("Health").unwrap().default_value().is_some());
    assert!(r.component("Uid").unwrap().default_value().is_none());
}

#[test]
fn field_path_parsing() {
    let w = World::with_registry(registry());
    let p = path(&w, "Npc.behaviour.waypoints[0][2]");
    assert_eq!(
        (p.target(), p.field(), p.is_whole()),
        ("Npc", "behaviour.waypoints[0][2]", false)
    );
    let p = path(&w, "Transform");
    assert!(p.is_whole() && p.field().is_empty());
    let p = path(&w, "Weather.fog");
    assert!(p.is_resource());
    assert!(matches!(w.field_path("Nope.x"), Err(ReflectError::UnknownType(n)) if n == "Nope"));
    assert!(matches!(
        w.field_path("Transform.translation..x"),
        Err(ReflectError::Path { .. })
    ));
}

#[test]
fn component_names_on_entity() {
    let (mut w, uid) = world_with_lamp();
    let e = w.entity_by_uid(uid).unwrap();
    // Derived and unregistered components are not listed.
    w.insert_one(e, box_core::transform::GlobalTransform::default())
        .unwrap();
    w.insert_one(e, 5u8).unwrap();
    assert_eq!(
        w.component_names(e),
        ["Name", "PointLight", "Transform", "Uid"]
    );
}

#[test]
fn get_and_set_by_path() {
    let (mut w, uid) = world_with_lamp();
    let e = w.entity_by_uid(uid).unwrap();
    let v = w.get_field(e, &path(&w, "PointLight.intensity")).unwrap();
    assert_eq!(v.try_downcast_ref::<f32>(), Some(&800.0));
    set(&mut w, uid, "PointLight.intensity", &1234.5f32).unwrap();
    set(&mut w, uid, "Transform.translation.x", &-4.0f32).unwrap();
    set(&mut w, uid, "PointLight.color[1]", &0.25f32).unwrap();
    set(&mut w, uid, "Transform.scale", &Vec3::splat(2.0)).unwrap();
    set(
        &mut w,
        uid,
        "Transform.rotation",
        &Quat::from_xyzw(0.0, 0.0, 0.0, 0.5),
    )
    .unwrap();
    set(&mut w, uid, "Name", &Name::new("Renamed")).unwrap();

    let light = w.get::<PointLight>(e).unwrap().clone();
    assert_eq!(light.intensity, 1234.5);
    assert_eq!(light.color, [1.0, 0.25, 1.0]);
    let t = *w.get::<Transform>(e).unwrap();
    assert_eq!(t.translation, Vec3::new(-4.0, 2.0, 3.0));
    assert_eq!(t.scale, Vec3::splat(2.0));
    assert_eq!(t.rotation, Quat::from_xyzw(0.0, 0.0, 0.0, 0.5));
    assert_eq!(w.get::<Name>(e).unwrap().as_str(), "Renamed");
    // The reflective borrow sees the same data.
    let r = w.get_reflect(e, "Transform").unwrap();
    assert_eq!(r.downcast_ref::<Transform>(), Some(&t));
}

#[test]
fn set_whole_component_from_dynamic_value() {
    let (mut w, uid) = world_with_lamp();
    // What a script or the deserializer would hand us: a dynamic struct.
    let mut d = DynamicStruct::default();
    d.insert("current", 5.0f32);
    d.insert("max", 50.0f32);
    d.set_represented_type(Some(<Health as Typed>::type_info()));
    let e = w.entity_by_uid(uid).unwrap();
    w.insert_default(e, "Health").unwrap();
    set(&mut w, uid, "Health", &d).unwrap();
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
    let (mut w, uid) = world_with_lamp();
    let e = w.entity_by_uid(uid).unwrap();
    w.insert_default(e, "Npc").unwrap();
    let patrol = Behaviour::Patrol {
        speed: 2.0,
        waypoints: vec![[1.0, 0.0, 0.0]],
    };
    set(&mut w, uid, "Npc.behaviour", &patrol).unwrap();
    set(&mut w, uid, "Npc.behaviour.speed", &3.0f32).unwrap();
    set(&mut w, uid, "Npc.behaviour.waypoints[0][2]", &9.0f32).unwrap();
    set(
        &mut w,
        uid,
        "Npc.tags",
        &vec!["a".to_string(), "b".to_string()],
    )
    .unwrap();
    // Shrinking a Vec works because set_field replaces instead of applying.
    set(&mut w, uid, "Npc.tags", &vec!["c".to_string()]).unwrap();
    set(&mut w, uid, "Npc.target", &Some(Uid::from_u128(9))).unwrap();
    set(&mut w, uid, "Npc.loot_table", &Option::<String>::None).unwrap();
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
    let (mut w, uid) = world_with_lamp();
    let e = w.entity_by_uid(uid).unwrap();
    w.insert_default(e, "Health").unwrap();
    assert!(matches!(
        w.insert_default(e, "Health"),
        Err(ReflectError::AlreadyPresent(_))
    ));
    let removed = w.remove_reflect(e, "PointLight").unwrap();
    assert_eq!(
        removed.try_downcast_ref::<PointLight>(),
        Some(&PointLight::default())
    );
    assert_eq!(w.component_names(e), ["Health", "Name", "Transform", "Uid"]);
    assert!(matches!(
        w.insert_default(e, "Nope"),
        Err(ReflectError::UnknownType(_))
    ));
    assert!(matches!(
        w.insert_default(e, "Weather"),
        Err(ReflectError::NotAComponent(_))
    ));
    assert!(matches!(
        w.remove_reflect(e, "PointLight"),
        Err(ReflectError::Missing(_))
    ));
    assert!(matches!(
        w.insert_default(e, "Uid"),
        Err(ReflectError::NoDefault(_))
    ));
}

#[test]
fn uid_is_protected_and_indexed() {
    let (mut w, uid) = world_with_lamp();
    let e = w.entity_by_uid(uid).unwrap();
    assert!(matches!(
        set(&mut w, uid, "Uid", &Uid::from_u128(1)),
        Err(ReflectError::ReadOnly(_))
    ));
    // Moving a uid through remove/insert keeps the index right.
    w.remove_reflect(e, "Uid").unwrap();
    assert_eq!(w.entity_by_uid(uid), None);
    let other = w.spawn_saved(Uid::from_u128(8), "Other", ()).unwrap();
    assert!(matches!(
        w.insert_reflect(e, "Uid", &Uid::from_u128(8)),
        Err(ReflectError::DuplicateUid(_))
    ));
    w.insert_reflect(e, "Uid", &Uid::from_u128(9)).unwrap();
    assert_eq!(w.entity_by_uid(Uid::from_u128(9)), Some(e));
    assert_eq!(w.entity_by_uid(Uid::from_u128(8)), Some(other));
}

#[test]
fn resources_by_name() {
    let mut w = World::with_registry(registry());
    assert!(w.resource_names().is_empty());
    assert!(matches!(
        w.get_resource_field(&path(&w, "Weather.fog")),
        Err(ReflectError::MissingResource(_))
    ));
    w.insert_resource_reflect("Weather", &Weather::default())
        .unwrap();
    assert_eq!(w.resource_names(), ["Weather"]);
    w.set_resource_field(&path(&w, "Weather.fog"), &0.5f32)
        .unwrap();
    assert_eq!(w.resource::<Weather>().fog, 0.5);
    let v = w.get_resource_field(&path(&w, "Weather.wind.x")).unwrap();
    assert_eq!(v.try_downcast_ref::<f32>(), Some(&1.0));
    let old = w.remove_resource_reflect("Weather").unwrap();
    assert_eq!(old.try_downcast_ref::<Weather>().map(|w| w.fog), Some(0.5));
}

#[test]
fn json_values_round_trip_through_the_registry() {
    let w = World::with_registry(registry());
    let r = w.registry();
    let json = serde_json::json!({ "current": 3.0, "max": 9.0 });
    let ty = std::any::TypeId::of::<Health>();
    let v = r.value_from_json(ty, &json).unwrap();
    assert_eq!(r.value_to_json(v.as_ref()).unwrap(), json);
    let err = r
        .value_from_json(ty, &serde_json::json!({ "current": "x" }))
        .unwrap_err();
    assert!(err.to_string().contains("game::Health"), "{err}");
}

#[test]
fn error_messages_name_the_path() {
    let (mut w, uid) = world_with_lamp();
    let errs = [
        set(&mut w, uid, "PointLight.intensty", &1.0f32).unwrap_err(),
        set(&mut w, uid, "PointLight.intensity", &1.0f64).unwrap_err(),
        set(&mut w, uid, "Transform.translation", &[1.0f32, 2.0, 3.0]).unwrap_err(),
        set(&mut w, uid, "PointLight.color[7]", &1.0f32).unwrap_err(),
    ];
    for e in &errs {
        println!("{e}");
    }
    assert!(errs[0].to_string().contains("PointLight.intensty"));
    assert!(matches!(errs[1], ReflectError::Shape { .. }));
    assert!(matches!(errs[2], ReflectError::Shape { .. }));
    assert!(matches!(errs[3], ReflectError::Path { .. }));
}

#[test]
fn registry_dump_is_stable_and_complete() {
    let r = registry();
    let a = r.to_json();
    assert_eq!(a, registry().to_json(), "dump is deterministic");
    let comps = a["components"].as_array().unwrap();
    let names: Vec<_> = comps.iter().map(|c| c["name"].as_str().unwrap()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
    let light = comps.iter().find(|c| c["name"] == "PointLight").unwrap();
    assert_eq!(light["type_path"], "game::PointLight");
    assert_eq!(light["doc"], "An omnidirectional light.");
    assert_eq!(light["fields"][1]["name"], "intensity");
    assert_eq!(light["fields"][1]["unit"], "lm");
    assert_eq!(light["fields"][1]["range"]["max"], 100_000.0);
    assert_eq!(light["default"]["intensity"], 800.0);
    let prim = comps.iter().find(|c| c["name"] == "Primitive").unwrap();
    let variants: Vec<_> = prim["fields"][0]["variants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        variants,
        ["Cuboid", "Sphere", "Cylinder", "Capsule", "Plane"]
    );
    assert_eq!(prim["fields"][0]["variants"][1]["fields"][0]["unit"], "m");
    assert_eq!(a["resources"][0]["name"], "Weather");
    assert_eq!(a["resources"][0]["fields"][0]["doc"], "Fog density.");
    let uid = comps.iter().find(|c| c["name"] == "Uid").unwrap();
    assert_eq!(
        (uid["hidden"].as_bool(), uid["default"].is_null()),
        (Some(true), true)
    );
}
