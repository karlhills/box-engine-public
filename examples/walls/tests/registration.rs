//! The registration experiment of docs/PROCEDURAL_DIRECTION.md §10: the
//! same `walls` rule registered as a plain system that calls `reconcile`
//! itself, next to the `app.add_rule` path the example uses.
//!
//! Findings (asserted below):
//!
//! - Both give identical parts and mesh hashes in an `App` (the player,
//!   Play).
//! - The plain system has to re-implement the change gate (a cached copy
//!   of every intent and its world transform), the per-owner grouping and
//!   the clean-up of owners that lost their parts, and must be ordered
//!   before `sync_scene_meshes` by name, or its parts generate a frame late.
//! - The editor's edit world has no `App`: it runs the rules the game adds
//!   with `add_rule` (`Game::rules`), not plain systems. A plain-system
//!   rule therefore builds nothing while editing.

mod common;

use std::collections::{BTreeMap, HashMap};

use box_engine::box_core::app::Stage;
use box_engine::box_core::derived::Derived;
use box_engine::box_proc::derive::{Part, parts_of, reconcile_with};
use box_engine::player::{PlayerOptions, build_app};
use box_engine::prelude::*;
use box_engine::{Game, sync_scene_meshes};
use common::*;
use walls::plan::{WallIntent, plan_walls, to_output};
use walls::{FenceGen, FencePath, WallGen, WallPath};

/// The walls rule as a plain `PostUpdate` system: everything `add_rule`
/// does, by hand.
fn plain_walls_system() -> impl FnMut(&mut World) {
    let mut cache: Vec<(Uid, WallPath, Affine3A)> = Vec::new();
    let mut primed = false;
    move |world: &mut World| {
        let mut now: Vec<(Uid, WallPath, Affine3A)> = world
            .query::<(&Uid, &WallPath, Option<&GlobalTransform>, Option<&Derived>)>()
            .iter()
            .filter(|(.., d)| d.is_none())
            .map(|(u, p, g, _)| (*u, p.clone(), g.map_or(Affine3A::IDENTITY, |g| g.0)))
            .collect();
        now.sort_by_key(|(u, ..)| *u);
        if primed && now == cache {
            return;
        }
        primed = true;
        let intents: Vec<WallIntent> = now
            .iter()
            .map(|(uid, path, transform)| WallIntent {
                uid: *uid,
                path,
                transform: *transform,
            })
            .collect();
        let out = to_output(plan_walls(&intents));
        let mut parts: BTreeMap<Uid, Vec<Part>> = BTreeMap::new();
        let mut warnings: BTreeMap<Uid, Vec<String>> = BTreeMap::new();
        for p in out.parts {
            parts.entry(p.key.owner().unwrap()).or_default().push(p);
        }
        for (uid, m) in out.warnings {
            warnings.entry(uid).or_default().push(m);
        }
        // Owners that had parts last time and have none now must be
        // reconciled to nothing as well.
        let mut owners: Vec<Uid> = now.iter().map(|(u, ..)| *u).collect();
        owners.extend(cache.iter().map(|(u, ..)| *u));
        owners.sort();
        owners.dedup();
        for uid in owners {
            if let Some(e) = world.entity_by_uid(uid) {
                let p = parts.remove(&uid).unwrap_or_default();
                let w = warnings.remove(&uid).unwrap_or_default();
                let _ = reconcile_with(world, e, walls::plan::RULE, p, w);
            }
        }
        box_engine::box_core::transform::propagate_transforms(world);
        cache = now;
    }
}

/// The walls example with its wall rule as a plain system (fences keep
/// `add_rule`).
struct PlainWalls;

impl Plugin for PlainWalls {
    fn register(&self, registry: &mut Registry) {
        walls::WallsPlugin.register(registry);
    }

    fn build(&self, app: &mut App) {
        use box_engine::box_proc::rule::RuleAppExt;
        app.system(Stage::PostUpdate, plain_walls_system())
            .before("box_engine::scene_glue::sync_scene_meshes");
        app.add_rule(walls::fence::RULE, walls::fence::plan);
        app.rule_reads::<walls::WallPath>(walls::fence::RULE);
    }
}

fn played(game: &Game, scene: &str) -> App {
    let options = PlayerOptions {
        project: Some(walls::authoring::project_dir()),
        scene: Some(scene.into()),
        ..Default::default()
    };
    let (mut app, _) = build_app(game, &options).unwrap();
    app.check_order().unwrap();
    app.run_frames(1, 1.0 / 60.0);
    app
}

#[test]
fn a_plain_system_gives_the_same_parts_in_an_app() {
    let rule = game();
    let plain = Game::new().with_plugin(PlainWalls);
    for scene in walls::authoring::SCENES {
        let a = played(&rule, scene);
        let b = played(&plain, scene);
        let ha = part_hashes(&a.world);
        assert!(!ha.is_empty());
        assert_eq!(
            ha,
            part_hashes(&b.world),
            "{scene}: drawn on the first frame"
        );
        let warnings = |w: &World| -> HashMap<Uid, usize> {
            w.query::<(&Uid, &box_engine::box_core::derived::GeneratedWarnings)>()
                .iter()
                .map(|(u, g)| (*u, g.0.len()))
                .collect()
        };
        assert_eq!(warnings(&a.world), warnings(&b.world), "{scene}");
    }
}

#[test]
fn a_plain_system_rule_builds_nothing_in_the_editors_edit_world() {
    let plain = Game::new().with_plugin(PlainWalls);
    let mut w = scene_world(&plain);
    assert_eq!(w.count::<&WallGen>(), 0, "no wall parts while editing");
    assert!(w.count::<&FenceGen>() > 0, "add_rule fences still build");

    let rule = game();
    w = scene_world(&rule);
    assert_eq!(w.count::<&WallGen>(), w.count::<&WallPath>());
    let owner = w
        .query::<(Entity, &WallPath)>()
        .iter()
        .map(|(e, _)| e)
        .next()
        .unwrap();
    assert_eq!(parts_of(&w, owner).len(), 1);
    assert!(w.count::<&FencePath>() > 0);
}

/// The yard scene in a world without an `App`, as the editor runs it.
fn scene_world(game: &Game) -> World {
    let bytes = std::fs::read(walls::authoring::scene_path("yard")).unwrap();
    let mut w = box_engine::box_core::scene::load(&bytes, game.registry()).unwrap();
    prepare(&mut w, game);
    box_engine::box_core::transform::propagate_transforms(&mut w);
    sync_scene_meshes(&mut w);
    box_engine::box_proc::settle(&mut w);
    w
}
