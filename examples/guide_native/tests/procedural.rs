//! The procedural guide's Rust samples (`docs/guide/procedural.md`): the
//! `Cairn` generator makes the mesh the guide describes, its output is
//! pinned by a golden fixture, and the `cairn_rows` rule makes the parts.

use box_engine::box_core::derived::{Derived, GeneratedWarnings};
use box_engine::box_proc::golden::{GoldenCase, assert_golden};
use box_engine::box_proc::mesh_hash;
use box_engine::prelude::*;
use guide_native::procedural::{Cairn, CairnRow, MAX_STONES};

#[test]
fn a_cairn_is_one_box_per_stone_with_the_top_in_its_own_slot() {
    let out = Cairn::default().generate(&GenContext::new(0));
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    let mesh = out.mesh.unwrap();
    // 5 stones of 12 triangles; slot 0 (Stone) for four, slot 1 (Top) for one.
    assert_eq!(mesh.triangle_count(), 5 * 12);
    let slots: Vec<(u32, u32)> = mesh
        .groups
        .iter()
        .map(|g| (g.material_slot, g.index_count / 3))
        .collect();
    assert_eq!(slots, [(0, 4 * 12), (1, 12)]);
    // The stack stands on the origin.
    let bounds = mesh.bounds();
    assert!(bounds.min.y.abs() < 1e-5, "{bounds:?}");
    assert!(bounds.max.y > 0.5, "{bounds:?}");
}

#[test]
fn the_seed_picks_the_variation_and_nothing_else_does() {
    let hash = |seed| {
        mesh_hash(
            &Cairn::default()
                .generate(&GenContext::new(seed))
                .mesh
                .unwrap(),
        )
    };
    assert_eq!(hash(7), hash(7));
    assert_ne!(hash(7), hash(8));
}

#[test]
fn bad_parameters_warn_and_degrade() {
    let out = Cairn {
        stones: 500,
        size: -1.0,
        jitter: f32::NAN,
    }
    .generate(&GenContext::new(0));
    assert_eq!(out.warnings.len(), 2, "{:?}", out.warnings);
    assert_eq!(out.mesh.unwrap().triangle_count(), MAX_STONES as usize * 12);
}

// Re-record after a VERSION bump:
// BOX_UPDATE_GOLDEN=1 cargo test -p guide_native --test procedural
#[test]
fn cairn_golden_hashes() {
    assert_golden::<Cairn>(
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden"),
        &[
            GoldenCase::new("default", Cairn::default(), 0),
            GoldenCase::new(
                "tall",
                Cairn {
                    stones: 12,
                    ..Cairn::default()
                },
                3,
            ),
        ],
    );
}

/// The parts of the owner `uid`: (role, entity), sorted by role.
fn parts(world: &World, uid: Uid) -> Vec<(String, Entity)> {
    let mut out: Vec<(String, Entity)> = world
        .query::<(Entity, &Derived)>()
        .iter()
        .filter(|(_, d)| d.key.owner() == Some(uid))
        .map(|(e, d)| {
            assert_eq!(d.rule, "cairn_rows");
            (d.key.role().to_owned(), e)
        })
        .collect();
    out.sort();
    out
}

#[test]
fn the_rule_makes_one_cairn_per_position() {
    let mut app = guide_native::game().new_app();
    let uid = Uid::new();
    let row = app.world.spawn((
        uid,
        Transform::from_xyz(0.0, 0.0, -2.0),
        CairnRow {
            count: 3,
            spacing: 1.5,
            size: 0.6,
        },
    ));
    app.run_frames(2, 1.0 / 60.0);

    let found = parts(&app.world, uid);
    let roles: Vec<&str> = found.iter().map(|(r, _)| r.as_str()).collect();
    assert_eq!(roles, ["cairn0", "cairn1", "cairn2"]);
    for (i, (role, e)) in found.iter().enumerate() {
        // Children of the row, 1.5 m apart along its +X axis.
        let t = app.world.get::<Transform>(*e).unwrap();
        assert_eq!(t.translation, Vec3::new(i as f32 * 1.5, 0.0, 0.0), "{role}");
        let world = Vec3::from(app.world.get::<GlobalTransform>(*e).unwrap().0.translation);
        assert_eq!(world, Vec3::new(i as f32 * 1.5, 0.0, -2.0), "{role}");
        // Each its own seed, each generated.
        assert_eq!(app.world.get::<Procedural>(*e).unwrap().seed, i as u64);
        let state = app.world.get::<ProcState>(*e).unwrap();
        assert!(state.mesh().is_some(), "{role}");
    }
    assert!(
        app.world
            .get::<GeneratedWarnings>(row)
            .is_none_or(|w| w.0.is_empty())
    );

    // An edit to the intent re-runs the rule: fewer, overlapping cairns
    // despawn the extra part and warn on the row.
    let r = app.world.component_mut::<CairnRow>(row).unwrap();
    r.count = 2;
    r.spacing = 0.4;
    app.run_frames(1, 1.0 / 60.0);
    assert_eq!(parts(&app.world, uid).len(), 2);
    let warnings = app.world.get::<GeneratedWarnings>(row).unwrap();
    assert_eq!(warnings.0.len(), 1);
    assert!(
        warnings.0[0].message.contains("the cairns overlap"),
        "{:?}",
        warnings.0
    );
}
