//! M6 additions to rules and derived entities: `Overrides` on the owner
//! (patches applied by `reconcile`, hidden parts, stale keys kept as
//! warnings, save/load and undo), rules that read a second intent type,
//! and enabling and disabling rules.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use bevy_reflect::Reflect;
use box_core::derived::{GeneratedWarnings, PartKey, Preview};
use box_core::edit::EditStack;
use box_core::prelude::*;
use box_core::reflect::{ReflectComponent, ReflectDefault, Registry};
use box_core::scene;
use box_proc::derive::{Part, parts_of, reconcile};
use box_proc::overrides::Overrides;
use box_proc::rule::{RuleAppExt, RuleInput, RuleOutput, RuleRegistry};

/// Saved intent.
#[derive(Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default, Clone, PartialEq)]
#[type_path = "box_proc_tests"]
struct Tower {
    height: f32,
}

/// A second saved intent type that `towers` reads.
#[derive(Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default, Clone, PartialEq)]
#[type_path = "box_proc_tests"]
struct Flag {
    raised: bool,
}

/// A registered part component (so overrides can patch it).
#[derive(Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default, Clone, PartialEq)]
#[type_path = "box_proc_tests"]
struct Block {
    height: f32,
    colour: String,
}

fn registry() -> Arc<Registry> {
    let mut r = Registry::with_core_types();
    r.register::<Tower>().unwrap();
    r.register::<Flag>().unwrap();
    r.register::<Block>().unwrap();
    r.register::<Overrides>().unwrap();
    Arc::new(r)
}

fn app() -> App {
    let mut app = App::new();
    app.world.set_registry(registry());
    app
}

fn owner(world: &mut World) -> (Entity, Uid) {
    let uid = Uid::new();
    let e = world
        .spawn_saved(uid, "tower", (Transform::IDENTITY, Tower { height: 2.0 }))
        .unwrap();
    (e, uid)
}

fn parts(uid: Uid, height: f32) -> Vec<Part> {
    vec![
        Part::new(PartKey::new(uid, "body")).with(Block {
            height,
            colour: "grey".into(),
        }),
        Part::new(PartKey::new(uid, "roof"))
            .at(Transform::from_xyz(0.0, height, 0.0))
            .with(Block {
                height: 0.5,
                colour: "red".into(),
            }),
    ]
}

fn block(world: &World, owner: Entity, role: &str) -> Option<Block> {
    parts_of(world, owner)
        .into_iter()
        .find(|(_, d)| d.key.role() == role)
        .and_then(|(e, _)| world.get::<Block>(e).map(|b| (*b).clone()))
}

fn warnings(world: &World, owner: Entity) -> Vec<String> {
    world
        .get::<GeneratedWarnings>(owner)
        .map(|w| w.0.iter().map(ToString::to_string).collect())
        .unwrap_or_default()
}

fn set_overrides(world: &mut World, e: Entity, o: Overrides) {
    let _ = world.remove_one::<Overrides>(e);
    world.insert_one(e, o).unwrap();
}

#[test]
fn patches_apply_after_the_rule_and_only_rewrite_on_change() {
    let mut w = World::with_registry(registry());
    let (o, uid) = owner(&mut w);
    let mut ov = Overrides::default();
    ov.set_field("towers", "body", "Block.height", "3.5");
    ov.set_field("towers", "roof", "Block.colour", "\"gold\"");
    set_overrides(&mut w, o, ov.clone());

    let r = reconcile(&mut w, o, "towers", parts(uid, 2.0)).unwrap();
    assert_eq!(r.spawned, 2);
    assert_eq!(block(&w, o, "body").unwrap().height, 3.5);
    assert_eq!(block(&w, o, "body").unwrap().colour, "grey");
    assert_eq!(block(&w, o, "roof").unwrap().colour, "gold");

    // The same rule output and patch: nothing is written.
    let r = reconcile(&mut w, o, "towers", parts(uid, 2.0)).unwrap();
    assert_eq!((r.unchanged, r.changed()), (2, false));

    // The rule changes another field: the patch stays on top.
    let mut changed = parts(uid, 2.0);
    changed[0] = Part::new(PartKey::new(uid, "body")).with(Block {
        height: 2.0,
        colour: "white".into(),
    });
    let r = reconcile(&mut w, o, "towers", changed).unwrap();
    assert_eq!((r.updated, r.unchanged), (1, 1));
    assert_eq!(
        block(&w, o, "body").unwrap(),
        Block {
            height: 3.5,
            colour: "white".into()
        }
    );

    // Resetting the override restores the rule's value (one part changes).
    ov.reset_field("towers", "body", "Block.height");
    set_overrides(&mut w, o, ov.clone());
    let r = reconcile(&mut w, o, "towers", parts(uid, 2.0)).unwrap();
    assert_eq!((r.updated, r.unchanged), (1, 1));
    assert_eq!(block(&w, o, "body").unwrap().height, 2.0);
    assert!(warnings(&w, o).is_empty(), "{:?}", warnings(&w, o));
}

#[test]
fn hidden_parts_are_not_built_and_stale_keys_are_kept_with_a_warning() {
    let mut w = World::with_registry(registry());
    let (o, uid) = owner(&mut w);
    reconcile(&mut w, o, "towers", parts(uid, 2.0)).unwrap();
    let mut ov = Overrides::default();
    ov.set_hidden("towers", "roof", true);
    ov.set_field("towers", "spire", "Block.height", "9.0");
    ov.set_field("towers", "body", "Block.heigth", "1.0");
    ov.set_field("towers", "body", "Missing.x", "1.0");
    ov.set_field("other_rule", "spire", "Block.height", "1.0");
    set_overrides(&mut w, o, ov.clone());
    let r = reconcile(&mut w, o, "towers", parts(uid, 2.0)).unwrap();
    assert_eq!((r.hidden, r.removed), (1, 1));
    assert!(block(&w, o, "roof").is_none());
    let warned = warnings(&w, o);
    assert_eq!(warned.len(), 3, "{warned:?}");
    assert!(
        warned
            .iter()
            .any(|m| m.contains("(spire)") && m.contains("no longer makes it"))
    );
    assert!(warned.iter().any(|m| m.contains("Block.heigth")));
    assert!(warned.iter().any(|m| m.contains("has no `Missing`")));
    // Warnings persist while nothing changes; the overrides are untouched.
    reconcile(&mut w, o, "towers", parts(uid, 2.0)).unwrap();
    assert_eq!(warnings(&w, o).len(), 3);
    assert_eq!(*w.get::<Overrides>(o).unwrap(), ov);
    // Shown again: the roof comes back with the same uid.
    ov.set_hidden("towers", "roof", false);
    set_overrides(&mut w, o, ov);
    let r = reconcile(&mut w, o, "towers", parts(uid, 2.0)).unwrap();
    assert_eq!(r.spawned, 1);
    let roof = PartKey::new(uid, "roof").uid();
    assert!(w.entity_by_uid(roof).is_some());
}

#[test]
fn overrides_save_load_and_undo_like_any_field_and_parts_are_not_saved() {
    let reg = registry();
    let mut w = World::with_registry(reg.clone());
    let (o, uid) = owner(&mut w);
    reconcile(&mut w, o, "towers", parts(uid, 2.0)).unwrap();
    let mut stack = EditStack::new();
    let mut ov = Overrides::default();
    ov.set_field("towers", "body", "Block.height", "4.0");
    ov.set_hidden("towers", "roof", true);
    stack
        .add_component(&mut w, uid, "Overrides", Some(&ov))
        .unwrap();
    let saved = scene::save_string(&w).unwrap();
    assert!(saved.contains("\"Overrides\""), "{saved}");
    assert!(saved.contains("Block.height"));
    assert!(!saved.contains("\"body\":") && !saved.contains("Derived"));
    // Only the owner is saved.
    let loaded = scene::load(saved.as_bytes(), reg.clone()).unwrap();
    assert_eq!(loaded.entity_count(), 1);
    let le = loaded.entity_by_uid(uid).unwrap();
    assert_eq!(*loaded.get::<Overrides>(le).unwrap(), ov);

    // Undo removes the override, redo brings it back.
    stack.undo(&mut w).unwrap();
    assert!(!w.has::<Overrides>(o));
    stack.redo(&mut w).unwrap();
    assert_eq!(*w.get::<Overrides>(o).unwrap(), ov);
    // A field edit of the override is one undo step.
    let mut more = ov.clone();
    more.set_field("towers", "body", "Block.height", "5.0");
    let path = w.field_path("Overrides").unwrap();
    stack.set_field(&mut w, uid, &path, &more).unwrap();
    stack.undo(&mut w).unwrap();
    assert_eq!(*w.get::<Overrides>(o).unwrap(), ov);
}

fn tower_rule(runs: Arc<AtomicUsize>) -> impl Fn(&RuleInput<Tower>) -> RuleOutput {
    move |input| {
        runs.fetch_add(1, Ordering::Relaxed);
        let flags = input
            .read::<Flag>()
            .iter()
            .filter(|f| f.value.raised)
            .count();
        let mut out = RuleOutput::default();
        for t in input.intents {
            out.push(Part::new(t.key("body")).with(Block {
                height: t.value.height,
                colour: format!("{flags} flags"),
            }));
            if t.value.height > 10.0 {
                out.warn(t.uid, "very tall");
            }
        }
        out
    }
}

#[test]
fn an_override_edit_reruns_the_rule_and_hashes_see_the_patched_value() {
    let mut app = app();
    let runs = Arc::new(AtomicUsize::new(0));
    app.add_rule("towers", tower_rule(runs.clone()));
    let (o, _) = owner(&mut app.world);
    app.update();
    assert_eq!(runs.load(Ordering::Relaxed), 1);
    app.update();
    assert_eq!(runs.load(Ordering::Relaxed), 1, "idle");
    let mut ov = Overrides::default();
    ov.set_field("towers", "body", "Block.height", "7.0");
    set_overrides(&mut app.world, o, ov);
    app.update();
    assert_eq!(
        runs.load(Ordering::Relaxed),
        2,
        "an Overrides change re-runs"
    );
    assert_eq!(block(&app.world, o, "body").unwrap().height, 7.0);
    let info = app
        .world
        .resource::<RuleRegistry>()
        .get("towers")
        .unwrap()
        .clone();
    assert_eq!((info.last_report.updated, info.parts), (1, 1));
    app.update();
    assert_eq!(runs.load(Ordering::Relaxed), 2, "idle again");
}

#[test]
fn a_rule_reads_a_second_intent_type_and_changes_to_it_rerun_it() {
    let mut app = app();
    let runs = Arc::new(AtomicUsize::new(0));
    app.add_rule("towers", tower_rule(runs.clone()));
    app.rule_reads::<Flag>("towers");
    let info = app.world.resource::<RuleRegistry>().get("towers").unwrap();
    assert_eq!(info.reads, vec!["Flag"]);
    let (o, _) = owner(&mut app.world);
    let flag = app
        .world
        .spawn_saved(
            Uid::new(),
            "flag",
            (Transform::IDENTITY, Flag { raised: false }),
        )
        .unwrap();
    app.update();
    assert_eq!(block(&app.world, o, "body").unwrap().colour, "0 flags");
    app.update();
    assert_eq!(runs.load(Ordering::Relaxed), 1);
    app.world.get_mut::<Flag>(flag).unwrap().raised = true;
    app.update();
    assert_eq!(runs.load(Ordering::Relaxed), 2);
    assert_eq!(block(&app.world, o, "body").unwrap().colour, "1 flags");
    // Read-only: no parts or warnings for the flag's entity.
    assert!(parts_of(&app.world, flag).is_empty());
    // Moving the flag re-runs it too (its world transform is in the gate).
    app.world.get_mut::<Transform>(flag).unwrap().translation.x = 3.0;
    app.update();
    assert_eq!(runs.load(Ordering::Relaxed), 3);
}

#[test]
fn an_undeclared_read_is_empty() {
    let mut app = app();
    let runs = Arc::new(AtomicUsize::new(0));
    app.add_rule("towers", tower_rule(runs));
    let (o, _) = owner(&mut app.world);
    app.world
        .spawn_saved(
            Uid::new(),
            "flag",
            (Transform::IDENTITY, Flag { raised: true }),
        )
        .unwrap();
    app.update();
    assert_eq!(block(&app.world, o, "body").unwrap().colour, "0 flags");
}

#[test]
fn disabling_a_rule_removes_its_parts_and_warnings_and_enabling_restores_them() {
    let mut app = app();
    let runs = Arc::new(AtomicUsize::new(0));
    app.add_rule("towers", tower_rule(runs.clone()));
    let (o, uid) = owner(&mut app.world);
    app.world.get_mut::<Tower>(o).unwrap().height = 12.0;
    app.update();
    assert_eq!(parts_of(&app.world, o).len(), 1);
    assert_eq!(warnings(&app.world, o).len(), 1);
    assert!(
        app.world
            .resource_mut::<RuleRegistry>()
            .set_enabled("towers", false)
    );
    app.update();
    assert!(parts_of(&app.world, o).is_empty());
    assert!(warnings(&app.world, o).is_empty());
    let info = app.world.resource::<RuleRegistry>().get("towers").unwrap();
    assert!(!info.enabled);
    assert_eq!((info.parts, info.warnings), (0, 0));
    // Disabled: edits do not run it.
    app.world.get_mut::<Tower>(o).unwrap().height = 3.0;
    app.update();
    assert_eq!(runs.load(Ordering::Relaxed), 1);
    app.world
        .resource_mut::<RuleRegistry>()
        .set_enabled("towers", true);
    app.update();
    assert_eq!(runs.load(Ordering::Relaxed), 2);
    assert_eq!(block(&app.world, o, "body").unwrap().height, 3.0);
    assert!(
        app.world
            .entity_by_uid(PartKey::new(uid, "body").uid())
            .is_some()
    );
    assert!(
        !app.world
            .resource_mut::<RuleRegistry>()
            .set_enabled("nope", false)
    );
}

#[test]
fn preview_owners_are_generated_and_their_parts_build() {
    let mut app = app();
    let runs = Arc::new(AtomicUsize::new(0));
    app.add_rule("towers", tower_rule(runs));
    let e = app.world.spawn((
        Uid::new(),
        Transform::IDENTITY,
        Tower { height: 12.0 },
        Preview,
    ));
    app.update();
    assert!(box_core::prefab::is_generated(&app.world, e));
    assert_eq!(parts_of(&app.world, e).len(), 1);
    // Kept (not logged) and never saved.
    assert_eq!(warnings(&app.world, e).len(), 1);
    let saved = scene::save_string(&app.world).unwrap();
    assert!(!saved.contains("Tower"), "{saved}");
}
