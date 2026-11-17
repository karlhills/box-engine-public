//! Undo/redo: drag merging, subtree despawn, and a property test over random edit sequences.

mod common;

use box_core::edit::{Edit, EditStack, Keep};
use box_core::glam::{Quat, Vec3};
use box_core::identity::{Name, Uid};
use box_core::reflect::PartialReflect;
use box_core::scene::{self, UnknownComponents};
use box_core::transform::{GlobalTransform, Transform};
use box_core::world::{Placement, World};
use common::*;
use proptest::prelude::*;

fn save(w: &World) -> Vec<u8> {
    scene::save(w).unwrap()
}

fn set(
    stack: &mut EditStack,
    w: &mut World,
    uid: Uid,
    path: &str,
    v: &dyn PartialReflect,
) -> Result<(), box_core::edit::EditError> {
    let p = w.field_path(path)?;
    stack.set_field(w, uid, &p, v)
}

#[test]
fn drag_merges_into_one_entry() {
    let mut w = build_world(10, 3);
    let mut stack = EditStack::new();
    let before = save(&w);
    let u = uid(3, 0);
    stack.begin("Drag X");
    for i in 0..60 {
        set(
            &mut stack,
            &mut w,
            u,
            "Transform.translation.x",
            &(i as f32 * 0.5),
        )
        .unwrap();
    }
    // A different field in the same drag is a separate edit in the same step.
    set(&mut stack, &mut w, u, "Transform.translation.y", &1.0f32).unwrap();
    assert_eq!(stack.undo_label(), Some("Drag X"));
    assert!(stack.commit());
    assert_eq!(stack.undo_len(), 1);
    let after = save(&w);
    assert!(stack.undo(&mut w).unwrap());
    assert_eq!(save(&w), before);
    assert_eq!(stack.redo_label(), Some("Drag X"));
    assert!(stack.redo(&mut w).unwrap());
    assert_eq!(save(&w), after);
    // Outside a transaction every edit is its own step.
    set(&mut stack, &mut w, u, "Transform.translation.x", &1.0f32).unwrap();
    set(&mut stack, &mut w, u, "Transform.translation.x", &2.0f32).unwrap();
    assert_eq!(stack.undo_len(), 3);
    assert!(!stack.can_redo());
}

#[test]
fn despawn_subtree_and_undo_restores_order_and_unknown_data() {
    let mut w = build_world(60, 5);
    let mut stack = EditStack::new();
    let e0 = w.entity_by_uid(uid(5, 0)).unwrap();
    let target_e = *w
        .children(e0)
        .first()
        .expect("fixture gives entity 0 children");
    w.insert_one(
        target_e,
        UnknownComponents([("PluginThing".to_string(), serde_json::json!({"a": 1}))].into()),
    )
    .unwrap();
    let target = w.uid(target_e).unwrap();
    let before = save(&w);
    stack.despawn(&mut w, target).unwrap();
    assert!(w.entity_by_uid(target).is_none());
    stack.undo(&mut w).unwrap();
    assert_eq!(save(&w), before);
    stack.redo(&mut w).unwrap();
    assert!(w.entity_by_uid(target).is_none());
}

#[test]
fn spawn_rename_reparent_duplicate_and_resources() {
    let mut w = build_world(5, 11);
    let mut stack = EditStack::new();
    let before = save(&w);
    let light = PointLight::default();
    let u = Uid::from_u128(1000);
    let e = stack
        .spawn(
            &mut w,
            u,
            "Lamp",
            Placement {
                parent: None,
                index: 0,
            },
            &[("PointLight", &light)],
        )
        .unwrap();
    assert_eq!(w.roots()[0], e);
    assert_eq!(
        w.component_names(e),
        ["Name", "PointLight", "Transform", "Uid"]
    );
    stack.rename(&mut w, u, "Ceiling lamp").unwrap();
    assert_eq!(w.get::<Name>(e).unwrap().as_str(), "Ceiling lamp");
    stack.reparent(&mut w, u, Some(uid(11, 0)), None).unwrap();
    let copy = stack.duplicate(&mut w, u).unwrap();
    let ce = w.entity_by_uid(copy).unwrap();
    assert_eq!(w.placement(ce).index, w.placement(e).index + 1);
    let fog = w.field_path("Weather.fog").unwrap();
    stack.set_resource_field(&mut w, &fog, &0.7f32).unwrap();
    assert_eq!(w.resource::<Weather>().fog, 0.7);
    let edited = save(&w);
    let steps = stack.undo_len();
    assert_eq!(steps, 5);
    while stack.undo(&mut w).unwrap() {}
    assert_eq!(save(&w), before);
    while stack.redo(&mut w).unwrap() {}
    assert_eq!(save(&w), edited);
}

#[test]
fn failed_edits_record_nothing_and_rollback_reverts() {
    let mut w = build_world(5, 12);
    let mut stack = EditStack::new();
    let before = save(&w);
    let u = uid(12, 0);
    assert!(set(&mut stack, &mut w, u, "Transform.nope", &1.0f32).is_err());
    assert!(stack.add_component(&mut w, u, "Transform", None).is_err());
    assert!(stack.remove_component(&mut w, u, "Uid").is_err());
    assert!(
        stack
            .spawn(&mut w, u, "dup", Placement::last(None), &[])
            .is_err()
    );
    assert!(stack.reparent(&mut w, u, Some(u), None).is_err());
    assert!(stack.despawn(&mut w, Uid::from_u128(1)).is_err());
    assert_eq!(stack.undo_len(), 0);
    assert_eq!(save(&w), before);
    // A spawn whose components fail leaves nothing behind.
    let bad = 5u32;
    assert!(
        stack
            .spawn(
                &mut w,
                Uid::from_u128(77),
                "x",
                Placement::last(None),
                &[("Health", &bad)]
            )
            .is_err()
    );
    assert!(w.entity_by_uid(Uid::from_u128(77)).is_none());
    stack.begin("Several");
    set(&mut stack, &mut w, u, "Transform.translation.x", &4.0f32).unwrap();
    stack.add_component(&mut w, u, "Health", None).ok();
    stack.rollback(&mut w).unwrap();
    assert_eq!(save(&w), before);
    assert!(!stack.can_undo());
}

fn world_pos(w: &World, u: Uid) -> Vec3 {
    let e = w.entity_by_uid(u).unwrap();
    box_core::transform::compute_global(w, e).translation.into()
}

fn close(a: Vec3, b: Vec3) -> bool {
    (a - b).length() < 1e-4
}

/// Parent `P` (moved, rotated 90° about Y, scaled 2), group `G` without a
/// transform, and entity `E` at world (1, 2, 3).
fn reparent_world() -> (World, Uid, Uid, Uid) {
    let mut w = World::with_registry(registry());
    let (p, g, e) = (Uid::from_u128(1), Uid::from_u128(2), Uid::from_u128(3));
    let pt = Transform {
        translation: Vec3::new(10.0, 0.0, 0.0),
        rotation: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
        scale: Vec3::splat(2.0),
    };
    w.spawn_saved(p, "P", (pt,)).unwrap();
    w.spawn_saved(g, "G", ()).unwrap();
    w.spawn_saved(e, "E", (Transform::from_xyz(1.0, 2.0, 3.0),))
        .unwrap();
    (w, p, g, e)
}

#[test]
fn reparent_keeps_world_position_as_one_step() {
    let (mut w, p, g, e) = reparent_world();
    let mut stack = EditStack::new();
    let before = save(&w);
    let local = |w: &World| *w.get::<Transform>(w.entity_by_uid(e).unwrap()).unwrap();

    stack.reparent(&mut w, e, Some(p), None).unwrap();
    assert_eq!(stack.undo_len(), 1, "Reparent + SetField is one step");
    assert_eq!(stack.undo_label(), Some("Reparent"));
    assert!(close(world_pos(&w, e), Vec3::new(1.0, 2.0, 3.0)));
    let l = local(&w);
    assert!(close(l.scale, Vec3::splat(0.5)), "{l:?}");
    // GlobalTransform agrees after propagation.
    box_core::transform::propagate_transforms(&mut w);
    let ge = w.entity_by_uid(e).unwrap();
    let gt = w.get::<GlobalTransform>(ge).unwrap().translation();
    assert!(close(gt, Vec3::new(1.0, 2.0, 3.0)), "{gt}");
    let moved = save(&w);

    // Undo restores placement and the local transform byte for byte.
    stack.undo(&mut w).unwrap();
    assert_eq!(save(&w), before);
    stack.redo(&mut w).unwrap();
    assert_eq!(save(&w), moved);

    // Back to the roots: world position kept, local ≈ original.
    stack.reparent(&mut w, e, None, None).unwrap();
    assert!(close(local(&w).translation, Vec3::new(1.0, 2.0, 3.0)));
    assert!(close(local(&w).scale, Vec3::ONE));

    // Under a transform-less group (identity world transform): the local
    // transform is kept exactly and only the Reparent is recorded.
    let l = local(&w);
    stack.reparent(&mut w, e, Some(g), None).unwrap();
    assert_eq!(local(&w), l);
    stack.undo(&mut w).unwrap();
    assert_eq!(local(&w), l);

    // Inside an open transaction both edits join it.
    let steps = stack.undo_len();
    stack.begin("Several");
    stack.reparent(&mut w, e, Some(p), None).unwrap();
    stack.rename(&mut w, e, "Moved").unwrap();
    stack.commit();
    assert_eq!(stack.undo_len(), steps + 1);
    assert_eq!(stack.undo_label(), Some("Several"));
}

#[test]
fn reparent_keep_local_and_edge_cases() {
    let (mut w, p, g, e) = reparent_world();
    let mut stack = EditStack::new();
    let local = |w: &World| *w.get::<Transform>(w.entity_by_uid(e).unwrap()).unwrap();

    // Keep::Local moves the entity with its new parent.
    stack
        .reparent_keeping(&mut w, e, Some(p), None, Keep::Local)
        .unwrap();
    assert_eq!(local(&w), Transform::from_xyz(1.0, 2.0, 3.0));
    assert!(close(world_pos(&w, e), Vec3::new(16.0, 4.0, -2.0)));
    stack.undo(&mut w).unwrap();

    // A transform-less entity keeps its (absent) transform; its children move.
    let c = Uid::from_u128(4);
    w.spawn_saved(c, "C", (Transform::from_xyz(0.0, 1.0, 0.0),))
        .unwrap();
    stack.reparent(&mut w, c, Some(g), None).unwrap();
    let steps = stack.undo_len();
    stack.reparent(&mut w, g, Some(p), None).unwrap();
    assert_eq!(stack.undo_len(), steps + 1);
    assert!(!w.has::<Transform>(w.entity_by_uid(g).unwrap()));
    assert!(close(world_pos(&w, c), Vec3::new(10.0, 2.0, 0.0)));

    // A singular new parent (zero scale) cannot hold a world position: local kept.
    let pe = w.entity_by_uid(p).unwrap();
    w.get_mut::<Transform>(pe).unwrap().scale = Vec3::new(1.0, 0.0, 1.0);
    stack.reparent(&mut w, e, Some(p), None).unwrap();
    assert_eq!(local(&w), Transform::from_xyz(1.0, 2.0, 3.0));

    // Non-uniform scale with a rotated child: the world position stays exact.
    w.get_mut::<Transform>(pe).unwrap().scale = Vec3::new(1.0, 3.0, 0.5);
    stack.reparent(&mut w, e, None, None).unwrap();
    let ee = w.entity_by_uid(e).unwrap();
    w.get_mut::<Transform>(ee).unwrap().rotation = Quat::from_rotation_z(0.7);
    let before = world_pos(&w, e);
    stack.reparent(&mut w, e, Some(p), None).unwrap();
    assert!(close(world_pos(&w, e), before));
    let l = local(&w);
    assert!(l.rotation.is_normalized() && l.scale.is_finite(), "{l:?}");

    // Cycles are rejected without changing the transform or recording a step.
    let steps = stack.undo_len();
    let pl = *w.get::<Transform>(pe).unwrap();
    assert!(stack.reparent(&mut w, p, Some(e), None).is_err());
    assert_eq!(*w.get::<Transform>(pe).unwrap(), pl);
    assert_eq!(stack.undo_len(), steps);
}

#[test]
fn plugin_edits_through_apply() {
    let mut w = build_world(3, 13);
    let mut stack = EditStack::new();
    let u = uid(13, 1);
    let e = w.entity_by_uid(u).unwrap();
    let path = w.field_path("Transform.translation.y").unwrap();
    let old = w.get_field(e, &path).unwrap();
    let edit = Edit::SetField {
        uid: u,
        path,
        old,
        new: Box::new(42.0f32),
    };
    stack.apply(&mut w, "Brush", edit).unwrap();
    assert_eq!(
        w.get::<box_core::transform::Transform>(e)
            .unwrap()
            .translation
            .y,
        42.0
    );
    assert_eq!(stack.undo_label(), Some("Brush"));
    stack.undo(&mut w).unwrap();
    assert_ne!(
        w.get::<box_core::transform::Transform>(e)
            .unwrap()
            .translation
            .y,
        42.0
    );
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
        index: usize,
    },
    Despawn {
        who: usize,
    },
    Reparent {
        who: usize,
        parent: Option<usize>,
        index: usize,
    },
    Rename {
        who: usize,
        n: usize,
    },
    Duplicate {
        who: usize,
    },
    SetFog {
        v: f32,
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
const COMPONENTS: [&str; 5] = ["Transform", "PointLight", "Health", "Npc", "Primitive"];

fn op() -> impl Strategy<Value = Op> {
    let f = -100.0f32..100.0;
    prop_oneof![
        4 => (any::<usize>(), 0..FLOAT_FIELDS.len(), f.clone()).prop_map(|(who, field, v)| Op::SetFloat { who, field, v }),
        2 => (any::<usize>(), 0..FLOAT_FIELDS.len(), prop::collection::vec(f.clone(), 1..8))
            .prop_map(|(who, field, vs)| Op::Drag { who, field, vs }),
        1 => (any::<usize>(), 0usize..4).prop_map(|(who, n)| Op::SetTags { who, n }),
        1 => (any::<usize>(), 0usize..3, f.clone(), 0usize..4).prop_map(|(who, kind, v, len)| Op::SetBehaviour { who, kind, v, len }),
        1 => (any::<usize>(), prop::option::of(any::<usize>())).prop_map(|(who, other)| Op::SetTarget { who, other }),
        1 => (any::<usize>(), f.clone()).prop_map(|(who, v)| Op::SetColor { who, v }),
        2 => (any::<usize>(), 0..COMPONENTS.len()).prop_map(|(who, comp)| Op::Add { who, comp }),
        2 => (any::<usize>(), 0..COMPONENTS.len()).prop_map(|(who, comp)| Op::Remove { who, comp }),
        1 => (prop::option::of(any::<usize>()), 0usize..5).prop_map(|(parent, index)| Op::Spawn { parent, index }),
        1 => any::<usize>().prop_map(|who| Op::Despawn { who }),
        2 => (any::<usize>(), prop::option::of(any::<usize>()), 0usize..5)
            .prop_map(|(who, parent, index)| Op::Reparent { who, parent, index }),
        1 => (any::<usize>(), 0usize..3).prop_map(|(who, n)| Op::Rename { who, n }),
        1 => any::<usize>().prop_map(|who| Op::Duplicate { who }),
        1 => f.prop_map(|v| Op::SetFog { v }),
    ]
}

fn live_uids(w: &World) -> Vec<Uid> {
    let mut v: Vec<Uid> = w.query::<&Uid>().iter().copied().collect();
    v.sort();
    v
}

/// Apply one op. Ops that do not fit the current world (missing component,
/// cycle, ...) are rejected by the stack and record nothing.
fn apply(w: &mut World, stack: &mut EditStack, op: &Op, next_uid: &mut u128) {
    let uids = live_uids(w);
    if uids.is_empty() {
        return;
    }
    let pick = |i: usize| uids[i % uids.len()];
    let _ = match op {
        Op::SetFloat { who, field, v } => set(stack, w, pick(*who), FLOAT_FIELDS[*field], v),
        Op::Drag { who, field, vs } => {
            stack.begin("Drag");
            for v in vs {
                let _ = set(stack, w, pick(*who), FLOAT_FIELDS[*field], v);
            }
            stack.commit();
            Ok(())
        }
        Op::SetTags { who, n } => {
            let tags: Vec<String> = (0..*n).map(|i| format!("t{i}")).collect();
            set(stack, w, pick(*who), "Npc.tags", &tags)
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
            set(stack, w, pick(*who), "Npc.behaviour", &b)
        }
        Op::SetTarget { who, other } => set(stack, w, pick(*who), "Npc.target", &other.map(pick)),
        Op::SetColor { who, v } => set(stack, w, pick(*who), "PointLight.color", &[*v, 0.5, 0.25]),
        Op::Add { who, comp } => stack.add_component(w, pick(*who), COMPONENTS[*comp], None),
        Op::Remove { who, comp } => stack.remove_component(w, pick(*who), COMPONENTS[*comp]),
        Op::Spawn { parent, index } => {
            *next_uid += 1;
            let at = Placement {
                parent: parent.map(pick),
                index: *index,
            };
            stack
                .spawn(w, Uid::from_u128(*next_uid), "Spawned", at, &[])
                .map(drop)
        }
        Op::Despawn { who } => {
            if uids.len() > 3 {
                stack.despawn(w, pick(*who))
            } else {
                Ok(())
            }
        }
        Op::Reparent { who, parent, index } => {
            stack.reparent(w, pick(*who), parent.map(pick), Some(*index))
        }
        Op::Rename { who, n } => stack.rename(w, pick(*who), &format!("Name {n}")),
        Op::Duplicate { who } => {
            if uids.len() < 60 {
                stack.duplicate(w, pick(*who)).map(drop)
            } else {
                Ok(())
            }
        }
        Op::SetFog { v } => {
            let p = w.field_path("Weather.fog").unwrap();
            stack.set_resource_field(w, &p, v)
        }
    };
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn undo_all_then_redo_all(seed in 0u64..1000, ops in prop::collection::vec(op(), 1..40)) {
        let mut w = build_world(25, seed);
        let mut stack = EditStack::new();
        let original = save(&w);
        let mut next_uid = u128::MAX / 2;
        for op in &ops {
            apply(&mut w, &mut stack, op, &mut next_uid);
        }
        let edited = save(&w);
        let steps = stack.undo_len();
        for _ in 0..steps {
            prop_assert!(stack.undo(&mut w).unwrap());
        }
        prop_assert!(!stack.undo(&mut w).unwrap());
        prop_assert_eq!(String::from_utf8(save(&w)).unwrap(), String::from_utf8(original).unwrap());
        for _ in 0..steps {
            prop_assert!(stack.redo(&mut w).unwrap());
        }
        prop_assert_eq!(String::from_utf8(save(&w)).unwrap(), String::from_utf8(edited).unwrap());
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
        let mut w = build_world(25, seed);
        let mut stack = EditStack::new();
        let mut next_uid = u128::MAX / 2;
        for _ in 0..40 {
            let o = op().new_tree(&mut runner).unwrap().current();
            apply(&mut w, &mut stack, &o, &mut next_uid);
            ops_total += 1;
        }
        steps_total += stack.undo_len();
    }
    println!("{steps_total} undo steps from {ops_total} generated ops");
    assert!(steps_total * 2 > ops_total, "{steps_total} of {ops_total}");
}
