//! Script rules (`box_script::rules`): a Luau rule function turns
//! `ScriptIntent`s into derived parts in a world that does not play (the
//! editor's edit world) and in one that does, with the same change gate,
//! keys, overrides, warnings and hot reload as Rust rules.

mod common;

use std::collections::BTreeMap;
use std::time::Duration;

use box_engine::box_core::derived::{Derived, GeneratedWarnings, PartKey};
use box_engine::box_proc::derive::parts_of;
use box_engine::box_proc::overrides::Overrides;
use box_engine::box_proc::rule::{RuleInfo, RuleRegistry};
use box_engine::box_script::{ScriptIntent, ScriptRules};
use box_engine::prelude::*;
use box_engine::{Game, app_with_world};

use common::Project;

const FENCE: &str = include_str!("../../../examples/script_rules/assets/rules/fence.luau");
const FENCE_PATH: &str = "rules/fence.luau";
const COLONNADE: &str = include_str!("../../../examples/script_rules/assets/rules/colonnade.luau");
const COLONNADE_PATH: &str = "rules/colonnade.luau";
const FENCE_SCOPE: &str = "Fence.scope = { neighbours = SNAP }";

/// The fence rule without its scope (every edit runs it over every fence);
/// the line stays, empty, so warnings name the same lines.
fn unscoped_fence() -> String {
    assert!(FENCE.contains(FENCE_SCOPE));
    FENCE.replace(FENCE_SCOPE, "")
}

/// A world like the editor's edit world: no systems run; each frame
/// propagates transforms and calls `sync_scene_meshes` (assets, rules,
/// generators).
struct Edit {
    app: App,
}

impl Edit {
    fn new(p: &Project) -> Self {
        let game = Game::new();
        let mut app = app_with_world(game.new_world());
        app.world
            .insert_resource(box_engine::asset_server(p.assets()).unwrap());
        app.world.insert_resource(game.rules());
        Self { app }
    }

    fn world(&mut self) -> &mut World {
        &mut self.app.world
    }

    fn frame(&mut self) {
        let w = &mut self.app.world;
        if let Some(s) = w
            .get_resource::<box_engine::box_assets::AssetServer>()
            .cloned()
        {
            s.update_until_idle(w, Duration::from_millis(50));
        }
        box_engine::box_core::transform::propagate_transforms(w);
        box_engine::sync_scene_meshes(w);
    }

    /// Assets, transforms and rules only (no generators): what the rule
    /// tests compare.
    fn step(&mut self) {
        let w = &mut self.app.world;
        if let Some(s) = w
            .get_resource::<box_engine::box_assets::AssetServer>()
            .cloned()
        {
            s.update_until_idle(w, Duration::from_millis(50));
        }
        box_engine::box_core::transform::propagate_transforms(w);
        box_engine::box_proc::rule::run_rules(w);
    }

    /// Frames until `done` (at most 5 s).
    fn until(&mut self, mut done: impl FnMut(&World) -> bool) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            self.frame();
            if done(&self.app.world) {
                return true;
            }
        }
        false
    }

    fn info(&self, name: &str) -> RuleInfo {
        self.app
            .world
            .resource::<RuleRegistry>()
            .get(name)
            .unwrap_or_else(|| panic!("no rule {name}"))
            .clone()
    }
}

/// An unrelated runtime component.
struct Tag;

fn spawn_intent(w: &mut World, name: &str, intent: ScriptIntent) -> Entity {
    w.spawn((Uid::new(), Name::new(name), Transform::IDENTITY, intent))
}

fn fence(points: &[Vec3]) -> ScriptIntent {
    ScriptIntent::new(FENCE_PATH).with_points(points.iter().copied())
}

fn roles(w: &World, owner: Entity) -> Vec<String> {
    let mut r: Vec<String> = parts_of(w, owner)
        .into_iter()
        .map(|(_, d)| d.key.role().to_owned())
        .collect();
    r.sort();
    r
}

fn warnings(w: &World, owner: Entity) -> Vec<String> {
    w.get::<GeneratedWarnings>(owner)
        .map(|g| g.0.iter().map(|x| x.message.clone()).collect())
        .unwrap_or_default()
}

fn part(w: &World, owner: Entity, role: &str) -> Option<Entity> {
    let key = PartKey::new(w.uid(owner).unwrap(), role);
    w.entity_by_uid(key.uid())
}

#[test]
fn a_fence_rule_builds_parts_in_a_world_that_does_not_play() {
    let p = Project::new("rules_fence", &[(FENCE_PATH, FENCE)]);
    let mut ed = Edit::new(&p);
    let f = spawn_intent(
        ed.world(),
        "Fence",
        fence(&[Vec3::ZERO, Vec3::new(4.0, 0.0, 0.0)]),
    );
    assert!(ed.until(|w| !parts_of(w, f).is_empty()));
    // Two end posts, one post between (4 m at 2 m spacing), two rails.
    assert_eq!(
        roles(ed.world(), f),
        ["bay1/post1", "corner1", "corner2", "rail1", "rail2"]
    );
    let w = &ed.app.world;
    let post = part(w, f, "bay1/post1").unwrap();
    let t = *w.get::<Transform>(post).unwrap();
    assert!(
        (t.translation - Vec3::new(2.0, 0.55, 0.0)).length() < 1e-5,
        "{t:?}"
    );
    assert!(w.get::<Primitive>(post).is_some());
    let rail = part(w, f, "rail1").unwrap();
    assert!(w.get::<box_engine::box_proc::Sweep>(rail).is_some());
    assert_eq!(
        w.get::<box_engine::box_proc::Spline>(rail)
            .unwrap()
            .points
            .len(),
        2
    );
    {
        let d = w.get::<Derived>(rail).unwrap();
        assert_eq!((d.owner, d.rule.as_str()), (f, FENCE_PATH));
    }
    // Generators made meshes for the parts.
    ed.frame();
    let w = &ed.app.world;
    assert!(w.get::<MeshRenderer>(post).is_some());
    assert!(w.get::<MeshRenderer>(rail).is_some());
    let info = ed.info(FENCE_PATH);
    assert_eq!(
        (info.intent, info.intents, info.parts, info.enabled),
        ("ScriptIntent", 1, 5, true)
    );
    assert!(info.last_run.is_some() && info.last_rule.is_some());
}

#[test]
fn the_rule_runs_only_when_an_intent_or_its_file_changed() {
    let p = Project::new("rules_gate", &[(FENCE_PATH, FENCE)]);
    let mut ed = Edit::new(&p);
    let f = spawn_intent(ed.world(), "Fence", fence(&[Vec3::ZERO, Vec3::X * 3.0]));
    assert!(ed.until(|w| !parts_of(w, f).is_empty()));
    let runs = ed.info(FENCE_PATH).runs;
    for _ in 0..10 {
        ed.frame();
    }
    assert_eq!(ed.info(FENCE_PATH).runs, runs, "idle: no runs");
    // An unrelated component on the owner (archetype move): still idle.
    ed.world().insert_one(f, Tag).unwrap();
    ed.frame();
    assert_eq!(ed.info(FENCE_PATH).runs, runs);
    // A property edit runs it once.
    ed.world()
        .component_mut::<ScriptIntent>(f)
        .unwrap()
        .set("rails", ScriptValue::Integer(3));
    ed.frame();
    ed.frame();
    assert_eq!(ed.info(FENCE_PATH).runs, runs + 1);
    assert!(roles(ed.world(), f).contains(&"rail3".to_owned()));
    // Moving the owner runs it (world transforms are part of the input).
    ed.world()
        .component_mut::<Transform>(f)
        .unwrap()
        .translation
        .x = 5.0;
    ed.frame();
    assert_eq!(ed.info(FENCE_PATH).runs, runs + 2);
}

#[test]
fn keys_are_stable_and_unchanged_parts_are_not_rewritten() {
    let p = Project::new("rules_keys", &[(FENCE_PATH, FENCE)]);
    let mut ed = Edit::new(&p);
    let f = spawn_intent(ed.world(), "Fence", fence(&[Vec3::ZERO, Vec3::X * 4.0]));
    assert!(ed.until(|w| !parts_of(w, f).is_empty()));
    let corner1 = part(ed.world(), f, "corner1").unwrap();
    // A third point: the first stretch's parts keep their entities.
    ed.world()
        .component_mut::<ScriptIntent>(f)
        .unwrap()
        .points
        .push(Vec3::new(4.0, 0.0, 4.0));
    ed.frame();
    let w = ed.world();
    assert_eq!(part(w, f, "corner1"), Some(corner1));
    assert!(part(w, f, "corner3").is_some() && part(w, f, "bay2/post1").is_some());
    let r = ed.info(FENCE_PATH).last_report;
    // corner1, corner2 and bay1/post1 unchanged; rails updated; bay2/post1
    // and corner3 spawned.
    assert_eq!((r.unchanged, r.updated, r.spawned, r.removed), (3, 2, 2, 0));
}

#[test]
fn overrides_patch_and_hide_script_rule_parts() {
    let p = Project::new("rules_overrides", &[(FENCE_PATH, FENCE)]);
    let mut ed = Edit::new(&p);
    let f = spawn_intent(ed.world(), "Fence", fence(&[Vec3::ZERO, Vec3::X * 4.0]));
    assert!(ed.until(|w| !parts_of(w, f).is_empty()));
    let runs = ed.info(FENCE_PATH).runs;
    let mut o = Overrides::default();
    o.set_field(
        FENCE_PATH,
        "rail1",
        "StandardMaterial.base_color",
        "[1.0, 0.0, 0.0, 1.0]",
    );
    o.set_hidden(FENCE_PATH, "corner2", true);
    o.set_field(FENCE_PATH, "gone", "Primitive.cast_shadows", "false");
    ed.world().insert_one(f, o).unwrap();
    ed.frame();
    assert_eq!(
        ed.info(FENCE_PATH).runs,
        runs + 1,
        "Overrides are in the gate"
    );
    let w = &ed.app.world;
    let rail = part(w, f, "rail1").unwrap();
    assert_eq!(
        w.get::<StandardMaterial>(rail).unwrap().base_color,
        Color::linear_rgba(1.0, 0.0, 0.0, 1.0)
    );
    assert!(part(w, f, "corner2").is_none(), "hidden");
    assert!(
        warnings(w, f)
            .iter()
            .any(|m| m.contains("the rule no longer makes it")),
        "{:?}",
        warnings(w, f)
    );
    // Removing the overrides restores the rule's values.
    ed.world().remove_one::<Overrides>(f).unwrap();
    ed.frame();
    let w = &ed.app.world;
    let rail = part(w, f, "rail1").unwrap();
    assert_ne!(
        w.get::<StandardMaterial>(rail).unwrap().base_color,
        Color::linear_rgba(1.0, 0.0, 0.0, 1.0)
    );
    assert!(part(w, f, "corner2").is_some());
}

#[test]
fn warnings_and_errors_name_file_and_line_and_keep_the_last_parts() {
    let p = Project::new("rules_errors", &[(FENCE_PATH, FENCE)]);
    let mut ed = Edit::new(&p);
    let short = spawn_intent(ed.world(), "Short", fence(&[Vec3::ZERO]));
    let f = spawn_intent(ed.world(), "Fence", fence(&[Vec3::ZERO, Vec3::Z * 2.0]));
    assert!(ed.until(|w| !parts_of(w, f).is_empty()));
    let line = FENCE
        .lines()
        .position(|l| l.contains("a fence needs at least two points"))
        .unwrap()
        + 1;
    assert_eq!(
        warnings(ed.world(), short),
        [format!(
            "{FENCE_PATH}:{line}: a fence needs at least two points"
        )]
    );
    let before = roles(ed.world(), f);
    // A runtime error: the parts of the last good run stay, the error is
    // on every owner with file and line.
    let broken = FENCE.replace(
        "local rail = p.post_size * 0.45",
        "local rail = p.post_size * nil",
    );
    p.write(FENCE_PATH, &broken);
    let server = ed
        .world()
        .resource::<box_engine::box_assets::AssetServer>()
        .clone();
    server.reimport(server.id_of(FENCE_PATH).unwrap()).unwrap();
    assert!(ed.until(|w| warnings(w, f).iter().any(|m| m.contains("the rule failed"))));
    let line = broken
        .lines()
        .position(|l| l.contains("p.post_size * nil"))
        .unwrap()
        + 1;
    let w = ed.world();
    assert_eq!(roles(w, f), before, "last good parts kept");
    let msg = warnings(w, f).join("\n");
    assert!(msg.contains(&format!("{FENCE_PATH}:{line}:")), "{msg}");
    // A syntax error: the previous version keeps running.
    p.write(FENCE_PATH, "local Fence = {\n");
    server.reimport(server.id_of(FENCE_PATH).unwrap()).unwrap();
    assert!(ed.until(|w| warnings(w, f).iter().any(|m| m.contains("keeps running"))));
    assert_eq!(roles(ed.world(), f), before);
    // Fixed: parts and warnings follow the file again.
    p.write(
        FENCE_PATH,
        &FENCE.replace(
            "rails = { type = \"integer\", default = 2",
            "rails = { type = \"integer\", default = 1",
        ),
    );
    server.reimport(server.id_of(FENCE_PATH).unwrap()).unwrap();
    assert!(ed.until(|w| !roles(w, f).contains(&"rail2".to_owned())));
    assert!(
        warnings(ed.world(), f).is_empty(),
        "{:?}",
        warnings(ed.world(), f)
    );
}

#[test]
fn rules_are_pure_no_world_no_waits_no_tasks() {
    let cases = [
        ("world.find(\"x\")", "world.find is not available in a rule"),
        ("wait(1)", "wait() is not available in a rule"),
        ("task.spawn(print)", "task.spawn is not available in a rule"),
        (
            "input.held(\"jump\")",
            "input.held is not available in a rule",
        ),
        ("require(\"./x\")", "require() is not available in a rule"),
    ];
    for (i, (call, expected)) in cases.iter().enumerate() {
        let src =
            format!("local R = {{}}\nfunction R.rule(intents, out)\n    {call}\nend\nreturn R\n");
        let path = format!("rules/impure{i}.luau");
        let p = Project::new("rules_pure", &[(path.as_str(), src.as_str())]);
        let mut ed = Edit::new(&p);
        let e = spawn_intent(ed.world(), "I", ScriptIntent::new(&path));
        let count = ed.world().query::<Entity>().iter().count();
        assert!(
            ed.until(|w| !warnings(w, e).is_empty()),
            "{call}: no warning"
        );
        let msg = warnings(ed.world(), e).join("\n");
        assert!(msg.contains(expected), "{call}: {msg}");
        assert!(msg.contains(&format!("{path}:3:")), "{call}: {msg}");
        assert_eq!(
            ed.world().query::<Entity>().iter().count(),
            count,
            "{call}: nothing spawned"
        );
    }
}

#[test]
fn no_state_survives_between_runs() {
    // Module locals and globals start fresh on every run, so a rule cannot
    // count its runs (its output depends only on its intents and file).
    let src = r#"
local R = {}
local runs = 0
function R.rule(intents, out)
    runs += 1
    total = (total or 0) + 1
    for _, it in intents do
        out:part(it, `run{runs}x{total}`, { Primitive = {} }, { position = it.points[1] or vector.zero })
    end
end
return R
"#;
    let p = Project::new("rules_state", &[("rules/state.luau", src)]);
    let mut ed = Edit::new(&p);
    let e = spawn_intent(
        ed.world(),
        "I",
        ScriptIntent::new("rules/state.luau").with_points([Vec3::X]),
    );
    assert!(ed.until(|w| !parts_of(w, e).is_empty()));
    for x in 2..5 {
        ed.world().component_mut::<ScriptIntent>(e).unwrap().points[0].x = x as f32;
        ed.frame();
    }
    assert_eq!(ed.info("rules/state.luau").runs, 4);
    assert_eq!(roles(ed.world(), e), ["run1x1"]);
}

#[test]
fn an_endless_rule_is_interrupted() {
    let src = "local R = {}\nfunction R.rule(intents, out)\n    while true do end\nend\nreturn R\n";
    let p = Project::new("rules_loop", &[("rules/loop.luau", src)]);
    let game = Game::new();
    let mut app = app_with_world(game.new_world());
    app.world
        .insert_resource(box_engine::asset_server(p.assets()).unwrap());
    let mut rules = RuleRegistry::default();
    rules.add_source(ScriptRules::with_budget(Duration::from_millis(100)));
    app.world.insert_resource(rules);
    let mut ed = Edit { app };
    let e = spawn_intent(ed.world(), "I", ScriptIntent::new("rules/loop.luau"));
    assert!(ed.until(|w| !warnings(w, e).is_empty()));
    let msg = warnings(ed.world(), e).join("\n");
    assert!(
        msg.contains("rules/loop.luau:3:") && msg.contains("interrupted"),
        "{msg}"
    );
}

#[test]
fn the_same_intents_give_the_same_parts_in_every_world() {
    let src = r#"
local R = {}
function R.rule(intents, out)
    for _, it in intents do
        for i = 1, 5 do
            local h = 0.5 + math.random()
            out:part(it, "box" .. i, { Primitive = { shape = { Cuboid = { size = vector.create(0.3, h, 0.3) } } } },
                { position = vector.create(i, h / 2, 0) })
        end
    end
end
return R
"#;
    let p = Project::new("rules_determinism", &[("rules/r.luau", src)]);
    let mut shapes = Vec::new();
    for _ in 0..2 {
        let mut ed = Edit::new(&p);
        let uid = Uid::from_u128(42);
        let e = ed.world().spawn((
            uid,
            Name::new("I"),
            Transform::IDENTITY,
            ScriptIntent::new("rules/r.luau"),
        ));
        assert!(ed.until(|w| parts_of(w, e).len() == 5));
        let w = ed.world();
        let mut s: Vec<(String, Primitive, Transform)> = parts_of(w, e)
            .into_iter()
            .map(|(p, d)| {
                (
                    d.key.to_string(),
                    (*w.get::<Primitive>(p).unwrap()).clone(),
                    *w.get::<Transform>(p).unwrap(),
                )
            })
            .collect();
        s.sort_by(|a, b| a.0.cmp(&b.0));
        shapes.push(s);
    }
    assert_eq!(shapes[0], shapes[1]);
}

#[test]
fn script_rules_run_in_a_playing_world_and_hot_reload_there() {
    let p = Project::new("rules_play", &[(FENCE_PATH, FENCE)]);
    let mut app = p.app();
    let f = app.world.spawn((
        Uid::new(),
        Name::new("Fence"),
        Transform::IDENTITY,
        fence(&[Vec3::ZERO, Vec3::X * 2.0]),
    ));
    assert!(common::run_until(&mut app, 5.0, |app| parts_of(
        &app.world, f
    )
    .len()
        == 4));
    assert!(roles(&app.world, f).contains(&"rail2".to_owned()));
    p.write(
        FENCE_PATH,
        &FENCE.replace(
            "rails = { type = \"integer\", default = 2",
            "rails = { type = \"integer\", default = 4",
        ),
    );
    let s = common::server(&app);
    s.reimport(s.id_of(FENCE_PATH).unwrap()).unwrap();
    assert!(common::run_until(&mut app, 5.0, |app| roles(&app.world, f)
        .contains(&"rail4".to_owned())));
}

#[test]
fn a_disabled_rule_removes_its_parts_and_comes_back() {
    let p = Project::new("rules_switch", &[(FENCE_PATH, FENCE)]);
    let mut ed = Edit::new(&p);
    // Switched off before any intent named it.
    ed.world()
        .resource_mut::<RuleRegistry>()
        .set_enabled(FENCE_PATH, false);
    let f = spawn_intent(ed.world(), "Fence", fence(&[Vec3::ZERO, Vec3::X * 2.0]));
    for _ in 0..5 {
        ed.frame();
    }
    assert!(parts_of(ed.world(), f).is_empty());
    assert!(!ed.info(FENCE_PATH).enabled);
    ed.world()
        .resource_mut::<RuleRegistry>()
        .set_enabled(FENCE_PATH, true);
    ed.frame();
    let uids: Vec<Uid> = parts_of(ed.world(), f)
        .iter()
        .map(|(e, _)| ed.app.world.uid(*e).unwrap())
        .collect();
    assert_eq!(uids.len(), 4);
    ed.world()
        .resource_mut::<RuleRegistry>()
        .set_enabled(FENCE_PATH, false);
    ed.frame();
    assert!(parts_of(ed.world(), f).is_empty());
    ed.world()
        .resource_mut::<RuleRegistry>()
        .set_enabled(FENCE_PATH, true);
    ed.frame();
    let again: Vec<Uid> = parts_of(ed.world(), f)
        .iter()
        .map(|(e, _)| ed.app.world.uid(*e).unwrap())
        .collect();
    assert_eq!(uids, again, "same uids");
}

#[test]
fn shared_fence_ends_get_one_post() {
    let p = Project::new("rules_neighbours", &[(FENCE_PATH, FENCE)]);
    let mut ed = Edit::new(&p);
    let a = ed.world().spawn((
        Uid::from_u128(1),
        Name::new("A"),
        Transform::IDENTITY,
        fence(&[Vec3::ZERO, Vec3::X * 2.0]),
    ));
    // B starts where A ends (in world space: B is moved by 2 m).
    let b = ed.world().spawn((
        Uid::from_u128(2),
        Name::new("B"),
        Transform::from_xyz(2.0, 0.0, 0.0),
        fence(&[Vec3::ZERO, Vec3::Z * 2.0]),
    ));
    assert!(ed.until(|w| !parts_of(w, a).is_empty() && !parts_of(w, b).is_empty()));
    let w = ed.world();
    assert!(
        part(w, a, "corner2").is_some(),
        "A (lower uid) builds the shared post"
    );
    assert!(part(w, b, "corner1").is_none(), "B skips it");
    assert!(part(w, b, "corner2").is_some());
}

#[test]
fn bad_parts_are_errors_with_did_you_mean() {
    let src = "local R = {}\nfunction R.rule(intents, out)\n    for _, it in intents do\n        out:part(it, \"a\", { Primitve = {} })\n    end\nend\nreturn R\n";
    let p = Project::new("rules_bad", &[("rules/bad.luau", src)]);
    let mut ed = Edit::new(&p);
    let e = spawn_intent(ed.world(), "I", ScriptIntent::new("rules/bad.luau"));
    assert!(ed.until(|w| !warnings(w, e).is_empty()));
    let msg = warnings(ed.world(), e).join("\n");
    assert!(
        msg.contains("rules/bad.luau:4:") && msg.contains("did you mean `Primitive`"),
        "{msg}"
    );
    let src2 = "local R = {}\nfunction R.rule(intents, out)\n    for _, it in intents do\n        out:part(it, \"7\", {})\n    end\nend\nreturn R\n";
    p.write("rules/bad.luau", src2);
    let s = ed
        .world()
        .resource::<box_engine::box_assets::AssetServer>()
        .clone();
    s.reimport(s.id_of("rules/bad.luau").unwrap()).unwrap();
    assert!(ed.until(|w| warnings(w, e).iter().any(|m| m.contains("bare index"))));
}

// ----- scoped rules -----

/// Every derived part (key: rule, transform, registered components) and
/// every owner's warnings, by uid. Equal snapshots: the same generated
/// world.
fn snapshot(w: &World) -> BTreeMap<String, String> {
    let mut s = BTreeMap::new();
    for (e, d) in w.query::<(Entity, &Derived)>().iter() {
        let mut v = format!("{} {:?}", d.rule, *w.get::<Transform>(e).unwrap());
        for name in w.component_names(e) {
            if matches!(name, "Uid" | "Transform" | "MeshRenderer") {
                continue;
            }
            let c = w.get_reflect(e, name).unwrap();
            v.push_str(&format!(" {name}={:?}", c.as_partial_reflect()));
        }
        s.insert(d.key.to_string(), v);
    }
    for (uid, g) in w.query::<(&Uid, &GeneratedWarnings)>().iter() {
        let mut m: Vec<String> =
            g.0.iter()
                .map(|x| format!("{} {:?} {}", x.rule, x.key, x.message))
                .collect();
        m.sort();
        s.insert(format!("warnings of {uid}"), m.join("\n"));
    }
    s
}

/// Fails with the entries that differ.
fn assert_same(a: &BTreeMap<String, String>, b: &BTreeMap<String, String>, what: &str) {
    let keys: std::collections::BTreeSet<&String> = a.keys().chain(b.keys()).collect();
    let diff: Vec<String> = keys
        .into_iter()
        .filter(|k| a.get(*k) != b.get(*k))
        .map(|k| format!("{k}:\n  {:?}\n  {:?}", a.get(k), b.get(k)))
        .collect();
    assert!(diff.is_empty(), "{what}:\n{}", diff.join("\n"));
}

/// xorshift64*: the same edits on every machine.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 33) as usize % n
    }

    fn pick<T: Copy>(&mut self, xs: &[T]) -> T {
        xs[self.below(xs.len())]
    }

    /// A point on a 2 m grid, sometimes nudged near the fence's snap
    /// distance (0.1 m), so fence ends meet, nearly meet and part.
    fn point(&mut self) -> Vec3 {
        let nudge = self.pick(&[0.0, 0.0, 0.0, 0.05, 0.09, 0.1, 0.12, 0.3]);
        Vec3::new(
            self.below(5) as f32 * 2.0 + nudge,
            0.0,
            self.below(5) as f32 * 2.0,
        )
    }

    fn points(&mut self) -> Vec<Vec3> {
        let n = 1 + self.below(4);
        (0..n).map(|_| self.point()).collect()
    }
}

/// One edit, by uid, applied the same way to several worlds.
#[derive(Clone, Debug)]
enum Op {
    SetPoint(Uid, usize, Vec3),
    AddPoint(Uid, Vec3),
    RemovePoint(Uid),
    Place(Uid, Vec3, f32),
    Prop(Uid, &'static str, ScriptValue),
    Spawn(Uid, &'static str, Vec3, Vec<Vec3>),
    Despawn(Uid),
    Hide(Uid, &'static str),
    Paint(Uid),
    ClearOverrides(Uid),
    SwitchRule(Uid),
    RemoveIntent(Uid),
}

impl Op {
    fn random(rng: &mut Rng, uids: &mut Vec<Uid>, next: &mut u128) -> Op {
        let uid = rng.pick(uids);
        match rng.below(16) {
            0..=3 => Op::SetPoint(uid, rng.below(4), rng.point()),
            4 => Op::AddPoint(uid, rng.point()),
            5 => Op::RemovePoint(uid),
            6 | 7 => Op::Place(
                uid,
                rng.point() - Vec3::new(4.0, 0.0, 4.0),
                rng.below(4) as f32 * std::f32::consts::FRAC_PI_2,
            ),
            8 => Op::Prop(uid, "rails", ScriptValue::Integer(rng.below(4) as i64)),
            9 => Op::Prop(
                uid,
                rng.pick(&["height", "spacing"]),
                ScriptValue::Number(1.0 + rng.below(4) as f64 * 0.5),
            ),
            10 => {
                *next += 1;
                let uid = Uid::from_u128(*next);
                uids.push(uid);
                let rule = rng.pick(&[FENCE_PATH, FENCE_PATH, COLONNADE_PATH]);
                Op::Spawn(uid, rule, Vec3::ZERO, rng.points())
            }
            11 => Op::Despawn(uid),
            12 => Op::Hide(
                uid,
                rng.pick(&["corner1", "corner2", "rail1", "end/column"]),
            ),
            13 => Op::Paint(uid),
            14 => {
                if rng.below(2) == 0 {
                    Op::ClearOverrides(uid)
                } else {
                    Op::RemoveIntent(uid)
                }
            }
            _ => Op::SwitchRule(uid),
        }
    }

    fn apply(&self, w: &mut World) {
        let at = |w: &World, uid: &Uid| w.entity_by_uid(*uid);
        fn intent<'a>(w: &'a mut World, uid: &Uid) -> Option<&'a mut ScriptIntent> {
            let e = w.entity_by_uid(*uid)?;
            w.component_mut::<ScriptIntent>(e)
        }
        match self {
            Op::SetPoint(uid, i, p) => {
                if let Some(it) = intent(w, uid) {
                    match it.points.len() {
                        0 => it.points.push(*p),
                        n => it.points[i % n] = *p,
                    }
                }
            }
            Op::AddPoint(uid, p) => {
                if let Some(it) = intent(w, uid) {
                    it.points.push(*p);
                }
            }
            Op::RemovePoint(uid) => {
                if let Some(it) = intent(w, uid) {
                    it.points.pop();
                }
            }
            Op::Place(uid, p, yaw) => {
                if let Some(e) = at(w, uid) {
                    *w.component_mut::<Transform>(e).unwrap() =
                        Transform::from_translation(*p).with_rotation(Quat::from_rotation_y(*yaw));
                }
            }
            Op::Prop(uid, name, value) => {
                if let Some(it) = intent(w, uid) {
                    it.set(name, value.clone());
                }
            }
            Op::Spawn(uid, rule, p, points) => {
                w.spawn((
                    *uid,
                    Name::new(format!("I{uid}")),
                    Transform::from_translation(*p),
                    ScriptIntent::new(rule).with_points(points.iter().copied()),
                ));
            }
            Op::Despawn(uid) => {
                if let Some(e) = at(w, uid) {
                    w.despawn(e);
                }
            }
            Op::Hide(uid, part) => {
                if let Some(e) = at(w, uid) {
                    let mut o = w
                        .get::<Overrides>(e)
                        .map(|o| (*o).clone())
                        .unwrap_or_default();
                    o.set_hidden(FENCE_PATH, part, true);
                    o.set_hidden(COLONNADE_PATH, part, true);
                    w.insert_one(e, o).unwrap();
                }
            }
            Op::Paint(uid) => {
                if let Some(e) = at(w, uid) {
                    let mut o = w
                        .get::<Overrides>(e)
                        .map(|o| (*o).clone())
                        .unwrap_or_default();
                    o.set_field(
                        FENCE_PATH,
                        "rail1",
                        "StandardMaterial.base_color",
                        "[1.0, 0.0, 0.0, 1.0]",
                    );
                    w.insert_one(e, o).unwrap();
                }
            }
            Op::ClearOverrides(uid) => {
                if let Some(e) = at(w, uid) {
                    let _ = w.remove_one::<Overrides>(e);
                }
            }
            Op::SwitchRule(uid) => {
                if let Some(it) = intent(w, uid) {
                    let to = if it.rule.path.as_deref() == Some(FENCE_PATH) {
                        COLONNADE_PATH
                    } else {
                        FENCE_PATH
                    };
                    it.rule = AssetRef::path(to);
                }
            }
            Op::RemoveIntent(uid) => {
                if let Some(e) = at(w, uid) {
                    let _ = w.remove_one::<ScriptIntent>(e);
                }
            }
        }
    }
}

/// The parts of `w` built from scratch: its scene saved and loaded into a
/// fresh world with the rules of `p`.
fn from_scratch(p: &Project, w: &World) -> BTreeMap<String, String> {
    let game = Game::new();
    let bytes = box_engine::box_core::scene::save(w).unwrap();
    let world = box_engine::box_core::scene::load(&bytes, game.registry()).unwrap();
    let mut app = app_with_world(world);
    app.world
        .insert_resource(box_engine::asset_server(p.assets()).unwrap());
    app.world.insert_resource(game.rules());
    assert!(box_engine::settle_assets(
        &mut app.world,
        Duration::from_secs(20)
    ));
    snapshot(&app.world)
}

#[test]
fn scoped_runs_build_what_whole_runs_and_fresh_worlds_build() {
    let scoped = Project::new(
        "rules_scoped",
        &[(FENCE_PATH, FENCE), (COLONNADE_PATH, COLONNADE)],
    );
    let whole_fence = unscoped_fence();
    let whole = Project::new(
        "rules_whole",
        &[(FENCE_PATH, &whole_fence), (COLONNADE_PATH, COLONNADE)],
    );
    for seed in [1u64, 7, 42] {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        let mut a = Edit::new(&scoped);
        let mut b = Edit::new(&whole);
        let mut uids = Vec::new();
        let mut next = 100u128;
        let mut ops = Vec::new();
        for i in 0..12 {
            next += 1;
            let uid = Uid::from_u128(next);
            uids.push(uid);
            let rule = if i % 5 == 4 {
                COLONNADE_PATH
            } else {
                FENCE_PATH
            };
            let at = rng.point() - Vec3::new(4.0, 0.0, 4.0);
            ops.push(Op::Spawn(uid, rule, at, rng.points()));
        }
        for op in &ops {
            op.apply(a.world());
            op.apply(b.world());
        }
        assert!(a.until(|w| w.query::<&Derived>().iter().count() > 20));
        assert!(b.until(|w| w.query::<&Derived>().iter().count() > 20));
        assert_same(
            &snapshot(a.world()),
            &snapshot(b.world()),
            &format!("seed {seed}: start"),
        );
        let mut scoped_runs = 0;
        for step in 0..80 {
            let op = Op::random(&mut rng, &mut uids, &mut next);
            op.apply(a.world());
            op.apply(b.world());
            a.step();
            b.step();
            let fence = a.info(FENCE_PATH);
            if fence.last_report.parts() + fence.last_report.hidden < fence.parts {
                scoped_runs += 1;
            }
            assert_same(
                &snapshot(a.world()),
                &snapshot(b.world()),
                &format!("seed {seed}, step {step}: {op:?}"),
            );
            if step % 20 == 19 {
                assert_same(
                    &snapshot(a.world()),
                    &from_scratch(&scoped, a.world()),
                    &format!("seed {seed}, step {step}: from scratch"),
                );
            }
        }
        assert!(scoped_runs > 10, "seed {seed}: {scoped_runs} scoped runs");
    }
}

/// Two fences meeting at a point, and a third far away.
fn three_fences(ed: &mut Edit) -> (Entity, Entity, Entity) {
    let w = ed.world();
    let a = w.spawn((
        Uid::from_u128(1),
        Name::new("A"),
        Transform::IDENTITY,
        fence(&[Vec3::ZERO, Vec3::X * 2.0]),
    ));
    let b = w.spawn((
        Uid::from_u128(2),
        Name::new("B"),
        Transform::from_xyz(2.0, 0.0, 0.0),
        fence(&[Vec3::ZERO, Vec3::Z * 2.0]),
    ));
    let c = w.spawn((
        Uid::from_u128(3),
        Name::new("C"),
        Transform::from_xyz(50.0, 0.0, 0.0),
        fence(&[Vec3::ZERO, Vec3::Z * 2.0]),
    ));
    assert!(ed.until(|w| [a, b, c].iter().all(|e| !parts_of(w, *e).is_empty())));
    (a, b, c)
}

#[test]
fn a_shared_post_follows_either_fence() {
    let p = Project::new("rules_shared_post", &[(FENCE_PATH, FENCE)]);
    let mut ed = Edit::new(&p);
    let (a, b, c) = three_fences(&mut ed);
    let shared = |w: &World| {
        (
            part(w, a, "corner2").is_some(),
            part(w, b, "corner1").is_some(),
        )
    };
    assert_eq!(shared(ed.world()), (true, false), "A (lower uid) builds it");
    let c_parts: Vec<Entity> = parts_of(ed.world(), c).iter().map(|(e, _)| *e).collect();
    // A's end moves away: B, unchanged itself, builds its own post now.
    ed.world().component_mut::<ScriptIntent>(a).unwrap().points[1] = Vec3::new(1.5, 0.0, 0.0);
    ed.step();
    assert_eq!(shared(ed.world()), (true, true));
    let r = ed.info(FENCE_PATH).last_report;
    assert_eq!(
        r.parts(),
        parts_of(ed.world(), a).len() + parts_of(ed.world(), b).len(),
        "only A and B were reconciled"
    );
    // Back: B's post goes again.
    ed.world().component_mut::<ScriptIntent>(a).unwrap().points[1] = Vec3::X * 2.0;
    ed.step();
    assert_eq!(shared(ed.world()), (true, false));
    // B moves away, then comes back from elsewhere, reversed: its owner
    // and points change, and its first point is A's end again.
    ed.world()
        .component_mut::<Transform>(b)
        .unwrap()
        .translation = Vec3::new(10.0, 0.0, 0.0);
    ed.step();
    assert_eq!(shared(ed.world()), (true, true));
    ed.world()
        .component_mut::<Transform>(b)
        .unwrap()
        .translation = Vec3::new(2.0, 0.0, -2.0);
    ed.world().component_mut::<ScriptIntent>(b).unwrap().points = vec![Vec3::Z * 2.0, Vec3::ZERO];
    ed.step();
    assert_eq!(shared(ed.world()), (true, false));
    assert!(part(ed.world(), b, "corner2").is_some());
    // The far fence was never touched: the same entities.
    let now: Vec<Entity> = parts_of(ed.world(), c).iter().map(|(e, _)| *e).collect();
    assert_eq!(c_parts, now);
    // A despawned: B builds the post it shared.
    ed.world().despawn(a);
    ed.step();
    assert!(part(ed.world(), b, "corner2").is_some());
}

#[test]
fn undo_gives_back_the_same_parts() {
    use box_engine::box_core::edit::EditStack;
    let p = Project::new("rules_undo", &[(FENCE_PATH, FENCE)]);
    let mut ed = Edit::new(&p);
    let (a, b, _) = three_fences(&mut ed);
    let (ua, ub) = (ed.world().uid(a).unwrap(), ed.world().uid(b).unwrap());
    let before = snapshot(ed.world());
    let saved = box_engine::box_core::scene::save(ed.world()).unwrap();
    let mut edits = EditStack::new();
    let w = ed.world();
    let path = w.field_path("ScriptIntent.points").unwrap();
    let moved: Vec<Vec3> = vec![Vec3::ZERO, Vec3::new(1.0, 0.0, 1.0)];
    edits.set_field(w, ua, &path, &moved).unwrap();
    ed.step();
    assert_ne!(snapshot(ed.world()), before);
    edits.despawn(ed.world(), ub).unwrap();
    ed.step();
    let path = ed.world().field_path("ScriptIntent.properties").unwrap();
    let props = vec![box_engine::box_script::ScriptProperty {
        name: "rails".into(),
        value: ScriptValue::Integer(4),
    }];
    let w = ed.world();
    edits.set_field(w, ua, &path, &props).unwrap();
    ed.step();
    while edits.undo(ed.world()).unwrap() {
        ed.step();
    }
    assert_eq!(snapshot(ed.world()), before);
    assert_eq!(
        box_engine::box_core::scene::save(ed.world()).unwrap(),
        saved
    );
}

#[test]
fn an_intent_scope_rebuilds_only_the_changed_intent() {
    let src = r#"
local R = {}
R.scope = "intent"
function R.rule(intents, out)
    for _, it in intents do
        out:part(it, "post", { Primitive = {} }, { position = it.points[1] or vector.zero })
    end
end
return R
"#;
    let p = Project::new("rules_intent_scope", &[("rules/r.luau", src)]);
    let mut ed = Edit::new(&p);
    let es: Vec<Entity> = (0..50)
        .map(|i| {
            spawn_intent(
                ed.world(),
                "I",
                ScriptIntent::new("rules/r.luau").with_points([Vec3::X * i as f32]),
            )
        })
        .collect();
    assert!(ed.until(|w| es.iter().all(|e| !parts_of(w, *e).is_empty())));
    ed.world()
        .component_mut::<ScriptIntent>(es[7])
        .unwrap()
        .points[0]
        .y = 3.0;
    ed.step();
    let info = ed.info("rules/r.luau");
    assert_eq!((info.parts, info.last_report.parts()), (50, 1));
    assert_eq!(info.last_report.updated, 1);
    let post = part(ed.world(), es[7], "post").unwrap();
    assert_eq!(
        ed.world().get::<Transform>(post).unwrap().translation.y,
        3.0
    );
}

#[test]
fn scoped_rules_get_no_random_numbers_and_bad_scopes_are_errors() {
    let random = "local R = {}\nR.scope = { neighbours = 1 }\nfunction R.rule(intents, out)\n    local x = math.random()\nend\nreturn R\n";
    let bad =
        "local R = {}\nR.scope = \"everything\"\nfunction R.rule(intents, out)\nend\nreturn R\n";
    let p = Project::new(
        "rules_scope_errors",
        &[("rules/random.luau", random), ("rules/bad.luau", bad)],
    );
    let mut ed = Edit::new(&p);
    let r = spawn_intent(ed.world(), "R", ScriptIntent::new("rules/random.luau"));
    let b = spawn_intent(ed.world(), "B", ScriptIntent::new("rules/bad.luau"));
    assert!(ed.until(|w| !warnings(w, r).is_empty() && !warnings(w, b).is_empty()));
    let msg = warnings(ed.world(), r).join("\n");
    assert!(
        msg.contains("rules/random.luau:4:")
            && msg.contains("math.random is not available in a rule with a `scope`"),
        "{msg}"
    );
    let msg = warnings(ed.world(), b).join("\n");
    assert!(
        msg.contains("`scope` must be \"file\", \"intent\" or { neighbours = <metres> }"),
        "{msg}"
    );
}
