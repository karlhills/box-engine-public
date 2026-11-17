//! Undo/redo: drag merging, and a property test over random edit sequences.

use proptest::prelude::*;
use s2_reflection::components::*;
use s2_reflection::fixtures::build_world;
use s2_reflection::undo::Editor;
use s2_reflection::{default_registry, ops, scene};

fn editor(n: usize, seed: u64) -> Editor {
    Editor::new(build_world(n, seed), default_registry())
}

fn save(ed: &Editor) -> Vec<u8> {
    scene::save(&ed.world, &ed.registry).unwrap()
}

fn uid(seed: u64, i: u128) -> Uid {
    Uid::from_u128(((seed as u128) << 64) | (i + 1))
}

#[test]
fn drag_merges_into_one_entry() {
    let mut ed = editor(10, 3);
    let before = save(&ed);
    let u = uid(3, 0);
    ed.begin("Drag X");
    for i in 0..60 {
        ed.set_field(u, "Transform.translation.x", &(i as f32 * 0.5))
            .unwrap();
    }
    // A different field in the same drag is a separate edit in the same step.
    ed.set_field(u, "Transform.translation.y", &1.0f32).unwrap();
    ed.commit();
    assert_eq!(ed.undo_len(), 1);
    let after = save(&ed);
    assert!(ed.undo().unwrap());
    assert_eq!(save(&ed), before);
    assert!(ed.redo().unwrap());
    assert_eq!(save(&ed), after);
}

#[test]
fn despawn_subtree_and_undo_restores_order() {
    let mut ed = editor(60, 5);
    let before = save(&ed);
    // Despawn every root-level child chain of entity 0's first child, then undo.
    let e0 = ops::entity(&ed.world, uid(5, 0)).unwrap();
    let target = ed
        .world
        .children(e0)
        .first()
        .and_then(|c| ops::uid_of(&ed.world, *c))
        .unwrap_or(uid(5, 0));
    ed.despawn(target).unwrap();
    assert!(ops::entity(&ed.world, target).is_err());
    ed.undo().unwrap();
    assert_eq!(save(&ed), before);
}

/// An abstract edit, resolved against the live world when applied, so any
/// generated sequence is meaningful.
#[derive(Clone, Debug)]
enum Op {
    SetFloat {
        who: usize,
        field: usize,
        v: f32,
    },
    Drag {
        who: usize,
        field: usize,
        vs: Vec<f32>,
    },
    SetTags {
        who: usize,
        n: usize,
    },
    SetBehaviour {
        who: usize,
        kind: usize,
        v: f32,
        len: usize,
    },
    SetTarget {
        who: usize,
        other: Option<usize>,
    },
    SetColor {
        who: usize,
        v: f32,
    },
    Add {
        who: usize,
        comp: usize,
    },
    Remove {
        who: usize,
        comp: usize,
    },
    Spawn {
        parent: Option<usize>,
    },
    Despawn {
        who: usize,
    },
    Reparent {
        who: usize,
        parent: Option<usize>,
    },
}

const FLOAT_FIELDS: [&str; 6] = [
    "Transform.translation.x",
    "Transform.scale.z",
    "Transform.rotation.y",
    "PointLight.intensity",
    "Health.current",
    "Npc.behaviour.speed",
];
const COMPONENTS: [&str; 4] = ["Transform", "PointLight", "Health", "Npc"];

fn op() -> impl Strategy<Value = Op> {
    let f = -100.0f32..100.0;
    prop_oneof![
        4 => (any::<usize>(), 0..FLOAT_FIELDS.len(), f.clone()).prop_map(|(who, field, v)| Op::SetFloat { who, field, v }),
        2 => (any::<usize>(), 0..FLOAT_FIELDS.len(), prop::collection::vec(f.clone(), 1..8))
            .prop_map(|(who, field, vs)| Op::Drag { who, field, vs }),
        1 => (any::<usize>(), 0usize..4).prop_map(|(who, n)| Op::SetTags { who, n }),
        1 => (any::<usize>(), 0usize..3, f.clone(), 0usize..4).prop_map(|(who, kind, v, len)| Op::SetBehaviour { who, kind, v, len }),
        1 => (any::<usize>(), prop::option::of(any::<usize>())).prop_map(|(who, other)| Op::SetTarget { who, other }),
        1 => (any::<usize>(), f).prop_map(|(who, v)| Op::SetColor { who, v }),
        2 => (any::<usize>(), 0..COMPONENTS.len()).prop_map(|(who, comp)| Op::Add { who, comp }),
        2 => (any::<usize>(), 0..COMPONENTS.len()).prop_map(|(who, comp)| Op::Remove { who, comp }),
        1 => prop::option::of(any::<usize>()).prop_map(|parent| Op::Spawn { parent }),
        1 => any::<usize>().prop_map(|who| Op::Despawn { who }),
        2 => (any::<usize>(), prop::option::of(any::<usize>())).prop_map(|(who, parent)| Op::Reparent { who, parent }),
    ]
}

fn live_uids(ed: &Editor) -> Vec<Uid> {
    let mut v: Vec<Uid> = ed.world.query::<&Uid>().iter().copied().collect();
    v.sort();
    v
}

/// Apply one op through the editor. Ops that do not fit the current world
/// (missing component, cycle, ...) are rejected by the editor and record nothing.
fn apply(ed: &mut Editor, op: &Op, next_uid: &mut u128) {
    let uids = live_uids(ed);
    if uids.is_empty() {
        return;
    }
    let pick = |i: usize| uids[i % uids.len()];
    let _ = match op {
        Op::SetFloat { who, field, v } => ed.set_field(pick(*who), FLOAT_FIELDS[*field], v),
        Op::Drag { who, field, vs } => {
            ed.begin("Drag");
            for v in vs {
                let _ = ed.set_field(pick(*who), FLOAT_FIELDS[*field], v);
            }
            ed.commit();
            Ok(())
        }
        Op::SetTags { who, n } => {
            let tags: Vec<String> = (0..*n).map(|i| format!("t{i}")).collect();
            ed.set_field(pick(*who), "Npc.tags", &tags)
        }
        Op::SetBehaviour { who, kind, v, len } => {
            let b = match kind {
                0 => Behaviour::Idle,
                1 => Behaviour::Patrol {
                    speed: *v,
                    waypoints: vec![[*v, 0.0, 1.0]; *len],
                },
                _ => Behaviour::Follow(Some(pick(*len))),
            };
            ed.set_field(pick(*who), "Npc.behaviour", &b)
        }
        Op::SetTarget { who, other } => ed.set_field(pick(*who), "Npc.target", &other.map(pick)),
        Op::SetColor { who, v } => ed.set_field(pick(*who), "PointLight.color", &[*v, 0.5, 0.25]),
        Op::Add { who, comp } => ed.add_component(pick(*who), COMPONENTS[*comp]),
        Op::Remove { who, comp } => ed.remove_component(pick(*who), COMPONENTS[*comp]),
        Op::Spawn { parent } => {
            *next_uid += 1;
            ed.spawn(Uid::from_u128(*next_uid), "Spawned", parent.map(pick))
        }
        Op::Despawn { who } => {
            if uids.len() > 3 {
                ed.despawn(pick(*who))
            } else {
                Ok(())
            }
        }
        Op::Reparent { who, parent } => ed.reparent(pick(*who), parent.map(pick)),
    };
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn undo_all_then_redo_all(seed in 0u64..1000, ops in prop::collection::vec(op(), 1..40)) {
        let mut ed = editor(25, seed);
        let original = save(&ed);
        let mut next_uid = u128::MAX / 2;
        for op in &ops {
            apply(&mut ed, op, &mut next_uid);
        }
        let edited = save(&ed);
        let steps = ed.undo_len();
        for _ in 0..steps {
            prop_assert!(ed.undo().unwrap());
        }
        prop_assert!(!ed.undo().unwrap());
        prop_assert_eq!(String::from_utf8(save(&ed)).unwrap(), String::from_utf8(original).unwrap());
        for _ in 0..steps {
            prop_assert!(ed.redo().unwrap());
        }
        prop_assert_eq!(String::from_utf8(save(&ed)).unwrap(), String::from_utf8(edited).unwrap());
    }
}

/// Guards against a generator whose ops are mostly rejected (which would make
/// the property test pass vacuously).
#[test]
fn generated_ops_mostly_apply() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;
    let mut runner = TestRunner::deterministic();
    let (mut ops_total, mut steps_total) = (0, 0);
    for seed in 0..50 {
        let mut ed = editor(25, seed);
        let mut next_uid = u128::MAX / 2;
        for _ in 0..40 {
            let o = op().new_tree(&mut runner).unwrap().current();
            apply(&mut ed, &o, &mut next_uid);
            ops_total += 1;
        }
        steps_total += ed.undo_len();
    }
    println!("{steps_total} undo steps from {ops_total} generated ops");
    assert!(steps_total * 2 > ops_total, "{steps_total} of {ops_total}");
}
