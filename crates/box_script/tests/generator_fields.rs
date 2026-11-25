//! Every registered generator can be built from Luau (what a script rule's
//! `out:part` and `world.spawn` do): every variant of every enum field, with
//! no payload and with list payloads such as custom Sweep and Lathe
//! profiles (docs/WORLD_BUILDING.md W0).

use bevy_reflect::TypeInfo;
use box_engine::box_proc::GeneratorRegistry;
use box_engine::box_script::convert::{apply_lua, default_of};
use box_engine::prelude::*;
use mlua::{Lua, Value};

fn eval(lua: &Lua, src: &str) -> Value {
    lua.load(src).eval().unwrap()
}

#[test]
fn every_variant_of_every_generator_enum_field_builds_from_luau() {
    let lua = Lua::new();
    let registry = box_engine::engine_registry();
    let generators = registry.generators();
    assert!(generators.len() >= 4, "{}", generators.len());
    let mut built = 0;
    for g in generators {
        let c = g.component();
        let TypeInfo::Struct(info) = c.type_info() else {
            continue;
        };
        for field in info.iter() {
            let Some(TypeInfo::Enum(e)) = field.type_info() else {
                continue;
            };
            if e.type_path().starts_with("core::option::Option") {
                continue;
            }
            for variant in e.variant_names() {
                let mut value = default_of(&registry, c.type_id()).expect("default");
                let patch = eval(
                    &lua,
                    &format!("return {{ {} = {{ {variant} = {{}} }} }}", field.name()),
                );
                let path = format!("{}.{}", c.name(), field.name());
                apply_lua(&lua, &registry, value.as_mut(), &patch, &path)
                    .unwrap_or_else(|e| panic!("{path} = {variant}: {e}"));
                built += 1;
            }
        }
    }
    assert!(built >= 10, "{built} variants");
}

#[test]
fn custom_sweep_and_lathe_profiles_build_from_luau_lists() {
    let lua = Lua::new();
    let registry = box_engine::engine_registry();

    let mut sweep = Sweep::default();
    let patch = eval(
        &lua,
        "return { profile = { Custom = { points = { vector.create(-1, 0, 0), vector.create(0, 0.5, 0), vector.create(1, 0, 0) }, closed = false } } }",
    );
    apply_lua(&lua, &registry, &mut sweep, &patch, "Sweep").unwrap();
    assert_eq!(
        sweep.profile,
        SweepProfile::Custom {
            points: vec![
                Vec2::new(-1.0, 0.0),
                Vec2::new(0.0, 0.5),
                Vec2::new(1.0, 0.0)
            ],
            closed: false,
        }
    );

    // Tables of numbers work as points too.
    let mut lathe = Lathe::default();
    let patch = eval(
        &lua,
        "return { shape = { Custom = { profile = { {0, 0}, {1, 0}, {0.6, 1}, {0, 1.2} } } } }",
    );
    apply_lua(&lua, &registry, &mut lathe, &patch, "Lathe").unwrap();
    let LatheShape::Custom { profile, .. } = &lathe.shape else {
        panic!("{:?}", lathe.shape);
    };
    assert_eq!(profile.len(), 4);
    assert_eq!(profile[2], Vec2::new(0.6, 1.0));
}
