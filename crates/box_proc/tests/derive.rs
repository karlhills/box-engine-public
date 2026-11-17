//! Derived entities: `reconcile`, the `Derived` marker's shared behaviour
//! with prefab contents (save, edit, selection), part keys and uids,
//! warnings, and the `add_rule` wrapper with its change gating.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use bevy_reflect::Reflect;
use box_core::derived::{Derived, GeneratedRevision, GeneratedWarnings, PartKey};
use box_core::edit::EditStack;
use box_core::prelude::*;
use box_core::reflect::{ReflectComponent, ReflectDefault, Registry};
use box_core::scene;
use box_proc::derive::{Part, ReconcileError, parts_of, reconcile};
use box_proc::rule::{RuleAppExt, RuleInput, RuleOutput, RuleRegistry, run_rules};

/// Saved intent: a row of posts along +X.
#[derive(Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default, Clone, PartialEq)]
#[type_path = "box_proc_tests"]
struct Row {
    posts: u32,
    spacing: f32,
}

/// What each derived part carries (runtime only, not registered).
#[derive(Clone, Debug, PartialEq)]
struct Post {
    height: f32,
}

#[derive(Clone, Debug, PartialEq)]
struct Cap;

fn registry() -> Arc<Registry> {
    let mut r = Registry::with_core_types();
    r.register::<Row>().unwrap();
    Arc::new(r)
}

fn world() -> World {
    World::with_registry(registry())
}

fn owner(world: &mut World, name: &str) -> (Entity, Uid) {
    let uid = Uid::new();
    let e = world
        .spawn_saved(uid, name, (Transform::IDENTITY,))
        .unwrap();
    (e, uid)
}

/// Semantic roles: `start`, `end`, and `mid` + the post's distance in cm,
/// which does not shift when a post is added at the end.
fn row_parts(uid: Uid, row: &Row) -> Vec<Part> {
    (0..row.posts)
        .map(|i| {
            let role = match i {
                0 => "start".to_string(),
                i if i + 1 == row.posts => "end".to_string(),
                i => format!("mid{}", (i as f32 * row.spacing * 100.0).round() as i64),
            };
            Part::new(PartKey::new(uid, role))
                .at(Transform::from_xyz(i as f32 * row.spacing, 0.0, 0.0))
                .with(Post { height: 1.0 })
        })
        .collect()
}

fn uids_of(world: &World, owner: Entity) -> Vec<Uid> {
    let mut v: Vec<Uid> = parts_of(world, owner)
        .iter()
        .map(|(e, _)| world.uid(*e).unwrap())
        .collect();
    v.sort();
    v
}

// ----- reconcile -----

#[test]
fn reconcile_spawns_updates_only_on_change_and_removes() {
    let mut w = world();
    let (o, uid) = owner(&mut w, "Row");
    let row = Row {
        posts: 3,
        spacing: 2.0,
    };
    let r = reconcile(&mut w, o, "row", row_parts(uid, &row)).unwrap();
    assert_eq!((r.spawned, r.updated, r.unchanged, r.removed), (3, 0, 0, 0));
    let parts = parts_of(&w, o);
    assert_eq!(parts.len(), 3);
    for (e, d) in &parts {
        assert_eq!(w.parent(*e), Some(o), "parts are children of the owner");
        assert_eq!(d.rule, "row");
        assert_eq!(w.uid(*e), Some(d.key.uid()), "uid from the key");
        assert_eq!(w.get::<Name>(*e).unwrap().as_str(), d.key.role());
    }
    let revision = w.resource::<GeneratedRevision>().0;

    // The same parts again: nothing is written (a component replaced by
    // the same value would still be written by a naive insert).
    let end = w.entity_by_uid(PartKey::new(uid, "end").uid()).unwrap();
    w.get_mut::<Post>(end).unwrap().height = 1.0;
    let r = reconcile(&mut w, o, "row", row_parts(uid, &row)).unwrap();
    assert_eq!((r.unchanged, r.changed()), (3, false));
    assert_eq!(w.resource::<GeneratedRevision>().0, revision);

    // One post's component changes: only it is updated.
    let mut parts = row_parts(uid, &row);
    parts[1] = parts[1].clone().with(Post { height: 2.5 });
    let r = reconcile(&mut w, o, "row", parts).unwrap();
    assert_eq!((r.updated, r.unchanged), (1, 2));
    let mid = w.entity_by_uid(PartKey::new(uid, "mid200").uid()).unwrap();
    assert_eq!(w.get::<Post>(mid).unwrap().height, 2.5);
    assert!(w.resource::<GeneratedRevision>().0 > revision);

    // A component dropped from a part is removed; one added is inserted.
    let mut parts = row_parts(uid, &row);
    parts[0] = Part::new(PartKey::new(uid, "start")).with(Cap);
    let r = reconcile(&mut w, o, "row", parts).unwrap();
    assert_eq!(r.updated, 2, "start changed, mid200 went back to 1.0");
    let start = w.entity_by_uid(PartKey::new(uid, "start").uid()).unwrap();
    assert!(w.has::<Cap>(start) && !w.has::<Post>(start));

    // A longer row: the existing keys keep their entities and uids.
    let before: Vec<Entity> = ["start", "mid200"]
        .iter()
        .map(|r| w.entity_by_uid(PartKey::new(uid, r).uid()).unwrap())
        .collect();
    let longer = Row {
        posts: 4,
        spacing: 2.0,
    };
    let r = reconcile(&mut w, o, "row", row_parts(uid, &longer)).unwrap();
    // `end` moved to x = 6 (updated), `mid400` is new, `start` lost its cap.
    assert_eq!((r.spawned, r.removed), (1, 0));
    for (role, e) in ["start", "mid200"].iter().zip(&before) {
        assert_eq!(w.entity_by_uid(PartKey::new(uid, role).uid()), Some(*e));
    }

    // Shorter: the vanished keys are despawned with their subtrees.
    let child = w.spawn((Transform::IDENTITY,));
    let mid400 = w.entity_by_uid(PartKey::new(uid, "mid400").uid()).unwrap();
    w.set_parent(child, Some(mid400)).unwrap();
    let r = reconcile(&mut w, o, "row", row_parts(uid, &row)).unwrap();
    assert_eq!(r.removed, 1);
    assert!(!w.contains(mid400) && !w.contains(child));
    assert_eq!(parts_of(&w, o).len(), 3);

    // No parts: everything goes.
    let r = reconcile(&mut w, o, "row", Vec::new()).unwrap();
    assert_eq!(r.removed, 3);
    assert!(parts_of(&w, o).is_empty());
}

#[test]
fn reconcile_never_touches_other_owners_or_rules() {
    let mut w = world();
    let (a, ua) = owner(&mut w, "A");
    let (b, ub) = owner(&mut w, "B");
    let row = Row {
        posts: 2,
        spacing: 1.0,
    };
    reconcile(&mut w, a, "row", row_parts(ua, &row)).unwrap();
    reconcile(&mut w, b, "row", row_parts(ub, &row)).unwrap();
    let caps = vec![Part::new(PartKey::new(ua, "cap")).with(Cap)];
    reconcile(&mut w, a, "caps", caps).unwrap();
    // Clearing A's rows leaves A's caps and B's rows.
    let r = reconcile(&mut w, a, "row", Vec::new()).unwrap();
    assert_eq!(r.removed, 2);
    let a_rules: Vec<String> = parts_of(&w, a).into_iter().map(|(_, d)| d.rule).collect();
    assert_eq!(a_rules, vec!["caps".to_string()]);
    assert_eq!(parts_of(&w, b).len(), 2);
    // A key another rule of the owner already uses is refused, with a
    // warning, and the other rule's part is left alone.
    let clash = vec![Part::new(PartKey::new(ua, "cap")).with(Post { height: 3.0 })];
    let r = reconcile(&mut w, a, "row", clash).unwrap();
    assert_eq!((r.rejected, r.spawned), (1, 0));
    let cap = w.entity_by_uid(PartKey::new(ua, "cap").uid()).unwrap();
    assert!(w.has::<Cap>(cap) && !w.has::<Post>(cap));
    let warnings = w.get::<GeneratedWarnings>(a).unwrap().0.clone();
    assert!(warnings[0].message.contains("rule `caps`"), "{warnings:?}");
    // Keys of another owner, bare indices and duplicates are refused.
    let bad = vec![
        Part::new(PartKey::new(ub, "post")),
        Part::new(PartKey::new(ua, "7")),
        Part::new(PartKey::new(ua, "post")),
        Part::new(PartKey::new(ua, "post")),
    ];
    let r = reconcile(&mut w, a, "row", bad).unwrap();
    assert_eq!((r.rejected, r.spawned), (3, 1));
    assert_eq!(parts_of(&w, b).len(), 2);
}

#[test]
fn owner_errors_are_reported() {
    let mut w = world();
    let no_uid = w.spawn((Transform::IDENTITY,));
    assert_eq!(
        reconcile(&mut w, no_uid, "row", Vec::new()),
        Err(ReconcileError::OwnerWithoutUid(no_uid))
    );
    w.despawn(no_uid);
    assert_eq!(
        reconcile(&mut w, no_uid, "row", Vec::new()),
        Err(ReconcileError::NoOwner(no_uid))
    );
}

#[test]
fn despawning_the_owner_despawns_its_parts() {
    let mut w = world();
    let (o, uid) = owner(&mut w, "Row");
    let row = Row {
        posts: 3,
        spacing: 1.0,
    };
    reconcile(&mut w, o, "row", row_parts(uid, &row)).unwrap();
    let parts: Vec<Entity> = parts_of(&w, o).into_iter().map(|(e, _)| e).collect();
    w.despawn(o);
    assert!(parts.iter().all(|e| !w.contains(*e)));
    assert_eq!(w.query::<&Derived>().iter().count(), 0);
    assert!(w.entity_by_uid(PartKey::new(uid, "start").uid()).is_none());
}

// ----- shared behaviour with prefab contents -----

#[test]
fn derived_parts_are_not_saved_not_editable_and_select_their_owner() {
    let mut w = world();
    let (o, uid) = owner(&mut w, "Row");
    w.insert_one(
        o,
        Row {
            posts: 2,
            spacing: 1.0,
        },
    )
    .unwrap();
    let before = scene::save(&w).unwrap();
    let row = Row {
        posts: 2,
        spacing: 1.0,
    };
    reconcile(&mut w, o, "row", row_parts(uid, &row)).unwrap();
    // Save: byte-identical with and without the parts.
    assert_eq!(scene::save(&w).unwrap(), before);
    assert_eq!(scene::save_subtree(&w, o).unwrap().len(), 1);
    // Picking maps a part to its owner; the predicate covers it.
    let part = w.entity_by_uid(PartKey::new(uid, "end").uid()).unwrap();
    assert!(box_core::prefab::is_generated(&w, part));
    assert!(!box_core::prefab::is_instanced(&w, part));
    assert!(!box_core::prefab::is_generated(&w, o));
    assert_eq!(box_core::prefab::editable_ancestor(&w, part), o);
    // A saved child of a part (made by raw world access) is saved under the
    // owner, not under the part.
    let stray_uid = Uid::new();
    let stray = w.spawn_saved(stray_uid, "Stray", ()).unwrap();
    w.set_parent(stray, Some(part)).unwrap();
    assert_eq!(w.placement(stray).parent, Some(uid));
    w.despawn(stray);
    // Edits refuse the part (and placing under it), naming the owner.
    let mut edits = EditStack::new();
    let part_uid = w.uid(part).unwrap();
    let path = w.field_path("Transform").unwrap();
    let err = edits
        .set_field(&mut w, part_uid, &path, &Transform::from_xyz(1.0, 0.0, 0.0))
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("rule `row`") && err.contains(&uid.to_string()),
        "{err}"
    );
    assert!(edits.despawn(&mut w, part_uid).is_err());
    assert!(edits.rename(&mut w, part_uid, "x").is_err());
    let (child, child_uid) = owner(&mut w, "Child");
    let err = edits
        .reparent(&mut w, child_uid, Some(part_uid), None)
        .unwrap_err()
        .to_string();
    assert!(err.contains("generated by rule"), "{err}");
    assert_eq!(w.parent(child), None);
    // Names of parts never make a name ambiguous for commands.
    assert_eq!(box_core::edit::resolve_entity(&w, "Row").unwrap(), uid);
    let _ = w.spawn((Uid::new(), Name::new("end")));
    assert!(box_core::edit::resolve_entity(&w, "end").is_ok());
}

/// Delete the owner, undo: the scene is byte-identical and the next rule
/// pass rebuilds the parts with the same uids.
#[test]
fn undo_restores_the_scene_and_the_same_part_uids() {
    let mut app = App::new();
    app.world.set_registry(registry());
    app.add_rule("row", rule_row);
    let uid = Uid::new();
    app.world
        .spawn_saved(
            uid,
            "Row",
            (
                Transform::IDENTITY,
                Row {
                    posts: 4,
                    spacing: 1.0,
                },
            ),
        )
        .unwrap();
    app.update();
    let o = app.world.entity_by_uid(uid).unwrap();
    let uids = uids_of(&app.world, o);
    assert_eq!(uids.len(), 4);
    let saved = scene::save(&app.world).unwrap();

    let mut edits = EditStack::new();
    edits.despawn(&mut app.world, uid).unwrap();
    app.update();
    assert_eq!(app.world.query::<&Derived>().iter().count(), 0);
    edits.undo(&mut app.world).unwrap();
    app.update();
    let o = app.world.entity_by_uid(uid).unwrap();
    assert_eq!(uids_of(&app.world, o), uids);
    assert_eq!(scene::save(&app.world).unwrap(), saved);

    // A loaded copy of the scene derives the same uids too.
    let mut again = App::new();
    again.world = scene::load(&saved, registry()).unwrap();
    again.add_rule("row", rule_row);
    again.update();
    let o2 = again.world.entity_by_uid(uid).unwrap();
    assert_eq!(uids_of(&again.world, o2), uids);
}

// ----- warnings -----

// ----- add_rule -----

/// The rule as a plain function (what `add_rule` usually gets).
fn rule_row(input: &RuleInput<Row>) -> RuleOutput {
    let mut out = RuleOutput::default();
    for i in input.intents {
        for p in row_parts(i.uid, &i.value) {
            out.push(p);
        }
        if i.value.posts > 5 {
            out.warn(i.uid, "more than 5 posts");
        }
    }
    out
}

/// A rule that counts its own runs (tests run in parallel, so each test
/// gets its own counter).
fn counting(runs: Arc<AtomicUsize>) -> impl Fn(&RuleInput<Row>) -> RuleOutput {
    move |input| {
        runs.fetch_add(1, Ordering::Relaxed);
        let mut out = RuleOutput::default();
        for i in input.intents {
            for p in row_parts(i.uid, &i.value) {
                out.push(p);
            }
            if i.value.posts > 5 {
                out.warn(i.uid, "more than 5 posts");
            }
        }
        out
    }
}

#[test]
fn add_rule_runs_only_when_an_intent_changed() {
    let runs = Arc::new(AtomicUsize::new(0));
    let mut app = App::new();
    app.add_rule("row", counting(runs.clone()));
    app.update();
    assert_eq!(runs.load(Ordering::Relaxed), 1, "primed once");
    let uid = Uid::new();
    let e = app.world.spawn((
        uid,
        Name::new("Row"),
        Transform::IDENTITY,
        Row {
            posts: 3,
            spacing: 1.0,
        },
    ));
    app.update();
    assert_eq!(runs.load(Ordering::Relaxed), 2);
    assert_eq!(parts_of(&app.world, e).len(), 3);
    // Parts have their world transforms in the frame they appear.
    let end = app
        .world
        .entity_by_uid(PartKey::new(uid, "end").uid())
        .unwrap();
    let gt = app.world.get::<GlobalTransform>(end).unwrap().0;
    assert_eq!(Vec3::from(gt.translation), Vec3::new(2.0, 0.0, 0.0));
    // Idle: no runs.
    app.run_frames(10, 1.0 / 60.0);
    assert_eq!(runs.load(Ordering::Relaxed), 2);
    // Moving the owner reruns the rule (world transforms are input), but
    // the parts are relative to it, so none is rewritten.
    app.world.get_mut::<Transform>(e).unwrap().translation = Vec3::new(5.0, 0.0, 0.0);
    app.update();
    assert_eq!(runs.load(Ordering::Relaxed), 3);
    let info = app
        .world
        .resource::<RuleRegistry>()
        .get("row")
        .unwrap()
        .clone();
    assert_eq!(info.last_report.unchanged, 3);
    assert!(!info.last_report.changed());
    let gt = app.world.get::<GlobalTransform>(end).unwrap().0;
    assert_eq!(Vec3::from(gt.translation), Vec3::new(7.0, 0.0, 0.0));
    // Changing the intent: one more run; warnings land on the owner.
    app.world.get_mut::<Row>(e).unwrap().posts = 6;
    app.update();
    assert_eq!(runs.load(Ordering::Relaxed), 4);
    assert_eq!(parts_of(&app.world, e).len(), 6);
    let info = app
        .world
        .resource::<RuleRegistry>()
        .get("row")
        .unwrap()
        .clone();
    assert_eq!((info.parts, info.warnings, info.owners), (6, 1, 1));
    assert_eq!(info.intent, "Row");
    assert!(info.last_run.is_some() && info.last_rule.is_some());
    // Removing the intent removes the parts and the warnings.
    app.world.remove_one::<Row>(e).unwrap();
    app.update();
    assert!(parts_of(&app.world, e).is_empty());
    assert!(!app.world.has::<GeneratedWarnings>(e));
    let info = app
        .world
        .resource::<RuleRegistry>()
        .get("row")
        .unwrap()
        .clone();
    assert_eq!((info.parts, info.owners, info.intents), (0, 0, 0));
}

/// An unrelated component on an owner moves it to another archetype, which
/// changes query order; that is not an intent change.
#[test]
fn add_rule_ignores_query_order_changes() {
    struct Unrelated;
    let runs = Arc::new(AtomicUsize::new(0));
    let mut app = App::new();
    app.add_rule("row", counting(runs.clone()));
    let mut owners = Vec::new();
    for _ in 0..3 {
        owners.push(app.world.spawn((
            Uid::new(),
            Name::new("Row"),
            Transform::IDENTITY,
            Row {
                posts: 2,
                spacing: 1.0,
            },
        )));
    }
    app.update();
    assert_eq!(runs.load(Ordering::Relaxed), 1);
    app.world.insert_one(owners[0], Unrelated).unwrap();
    app.run_frames(3, 1.0 / 60.0);
    assert_eq!(runs.load(Ordering::Relaxed), 1, "no intent changed");
    // A real change after the reorder still runs the rule.
    app.world.get_mut::<Row>(owners[0]).unwrap().posts = 4;
    app.update();
    assert_eq!(runs.load(Ordering::Relaxed), 2);
    assert_eq!(parts_of(&app.world, owners[0]).len(), 4);
}

#[test]
fn rule_registry_lists_rules_and_a_plain_system_also_works() {
    let mut app = App::new();
    app.add_rule("row", rule_row);
    app.add_rule("row_again", counting(Arc::new(AtomicUsize::new(0))));
    let names: Vec<&str> = app
        .world
        .resource::<RuleRegistry>()
        .rules()
        .map(|r| r.name)
        .collect();
    assert_eq!(names, vec!["row", "row_again"]);
    assert_eq!(
        app.system_names(Stage::PostUpdate)
            .iter()
            .filter(|n| n.ends_with("::run_rules"))
            .count(),
        1,
        "one system for every rule"
    );

    // The plain path: a hand-written system calling reconcile.
    let mut plain = App::new();
    plain.add_system(Stage::Update, |w: &mut World| {
        let rows: Vec<(Entity, Uid, Row)> = w
            .query::<(Entity, &Uid, &Row)>()
            .iter()
            .map(|(e, u, r)| (e, *u, r.clone()))
            .collect();
        for (e, uid, row) in rows {
            reconcile(w, e, "plain_row", row_parts(uid, &row)).unwrap();
        }
    });
    let e = plain.world.spawn((
        Uid::new(),
        Transform::IDENTITY,
        Row {
            posts: 2,
            spacing: 1.0,
        },
    ));
    plain.update();
    let parts = parts_of(&plain.world, e);
    assert_eq!(parts.len(), 2);
    assert!(parts.iter().all(|(_, d)| d.rule == "plain_row"));
}

/// 1,000 intents: the idle cost of the change gate (no rule run). Run with
/// `--release -- --nocapture` for the numbers.
#[test]
fn idle_cost_with_a_thousand_intents() {
    let runs = Arc::new(AtomicUsize::new(0));
    let mut world = World::new();
    world.insert_resource(RuleRegistry::default());
    world
        .resource_mut::<RuleRegistry>()
        .add("row", counting(runs.clone()));
    for k in 0..1000 {
        world.spawn((
            Uid::new(),
            Transform::from_xyz(k as f32, 0.0, 0.0),
            Row {
                posts: 2,
                spacing: 1.0,
            },
        ));
    }
    box_core::transform::propagate_transforms(&mut world);
    let t0 = Instant::now();
    run_rules(&mut world);
    let first = t0.elapsed();
    assert_eq!(runs.load(Ordering::Relaxed), 1);
    assert_eq!(world.query::<&Derived>().iter().count(), 2000);
    let n = 200;
    let t0 = Instant::now();
    for _ in 0..n {
        run_rules(&mut world);
    }
    let idle = t0.elapsed().as_secs_f64() * 1e6 / n as f64;
    assert_eq!(runs.load(Ordering::Relaxed), 1, "idle: the rule never ran");
    // One intent changes: the rule runs once, one owner's parts change.
    let some = world.query::<(Entity, &Row)>().iter().next().unwrap().0;
    world.get_mut::<Row>(some).unwrap().posts = 3;
    let t0 = Instant::now();
    run_rules(&mut world);
    let edit = t0.elapsed();
    let info = world.resource::<RuleRegistry>().get("row").unwrap().clone();
    assert_eq!(runs.load(Ordering::Relaxed), 2);
    assert_eq!(
        (info.last_report.spawned, info.last_report.updated),
        (1, 1),
        "the new post and the moved end"
    );
    assert_eq!(info.last_report.unchanged, 2000 - 1);
    eprintln!(
        "1,000 intents, 2,000 parts: first run {:.2} ms; idle pass {idle:.1} µs; \
         one edit (rule + reconcile of all owners) {:.2} ms (rule alone {:.3} ms)",
        first.as_secs_f64() * 1e3,
        edit.as_secs_f64() * 1e3,
        info.last_rule.unwrap().as_secs_f64() * 1e3,
    );
    if !cfg!(debug_assertions) {
        assert!(idle < 500.0, "idle pass took {idle} µs");
    }
}
