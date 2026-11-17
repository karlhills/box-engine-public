//! Helpers shared by the walls tests: worlds that run the game's rules the
//! way the editor's edit world does (`Game::rules` + `sync_scene_meshes`),
//! a manual throttle clock, and per-part hashes and generation counts keyed
//! by part key.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::time::Duration;

use box_engine::box_core::derived::Derived;
use box_engine::box_core::scene;
use box_engine::box_core::transform::propagate_transforms;
use box_engine::box_proc::rule::{RuleInfo, RuleRegistry};
use box_engine::box_proc::{ProcClock, ProcRuntime, mesh_hash};
use box_engine::prelude::*;
use box_engine::{Game, sync_scene_meshes};
use walls::{FencePath, WallGen, WallPath, authoring};

pub const FRAME: Duration = Duration::from_micros(16_667);

pub fn game() -> Game {
    authoring::game()
}

/// Manual throttle clock and the game's rules, as in the editor's world.
pub fn prepare(w: &mut World, game: &Game) {
    w.insert_resource(ProcSettings {
        clock: ProcClock::Manual(Duration::ZERO),
        ..ProcSettings::default()
    });
    w.insert_resource(game.rules());
}

pub fn empty_world(game: &Game) -> World {
    let mut w = game.new_world();
    prepare(&mut w, game);
    w
}

/// A scene from `assets/scenes`, loaded and settled.
pub fn load_scene(game: &Game, name: &str) -> World {
    let bytes = std::fs::read(authoring::scene_path(name)).unwrap();
    load_bytes(game, &bytes)
}

pub fn load_bytes(game: &Game, bytes: &[u8]) -> World {
    let mut w = scene::load(bytes, game.registry()).unwrap();
    prepare(&mut w, game);
    settle(&mut w);
    w
}

/// One frame of the edit world: propagate, rules, generation (throttled).
pub fn frame(w: &mut World) {
    propagate_transforms(w);
    sync_scene_meshes(w);
}

/// A frame after the throttle interval has passed, then anything still
/// queued: every pending change is visible afterwards.
pub fn settle(w: &mut World) {
    advance(w, Duration::from_secs(1));
    frame(w);
    box_engine::box_proc::settle(w);
    frame(w);
}

pub fn advance(w: &mut World, by: Duration) {
    if let ProcClock::Manual(t) = &mut w.resource_mut::<ProcSettings>().clock {
        *t += by;
    }
}

/// `mesh_hash` of every derived part's drawn mesh, by part key.
pub fn part_hashes(w: &World) -> BTreeMap<String, u64> {
    let meshes = w.resource::<Assets<MeshData>>();
    w.query::<(&Derived, &MeshRenderer)>()
        .iter()
        .map(|(d, r)| {
            (
                d.key.to_string(),
                mesh_hash(meshes.get(r.mesh).expect("a generated mesh")),
            )
        })
        .collect()
}

/// How often each part was generated, by part key.
pub fn generations(w: &World) -> BTreeMap<String, u64> {
    w.query::<(&Derived, &ProcState)>()
        .iter()
        .map(|(d, s)| (d.key.to_string(), s.generations()))
        .collect()
}

/// Part keys whose generation count grew.
pub fn regenerated(before: &BTreeMap<String, u64>, after: &BTreeMap<String, u64>) -> Vec<String> {
    after
        .iter()
        .filter(|(k, g)| before.get(*k).is_none_or(|b| b < *g))
        .map(|(k, _)| k.clone())
        .collect()
}

pub fn rule(w: &World, name: &str) -> RuleInfo {
    w.resource::<RuleRegistry>().get(name).unwrap().clone()
}

pub fn generated_total(w: &World) -> u64 {
    w.resource::<ProcRuntime>().stats().generated
}

pub fn uid_by_name(w: &World, name: &str) -> Uid {
    w.query::<(&Uid, &Name)>()
        .iter()
        .find(|(_, n)| n.as_str() == name)
        .map(|(u, _)| *u)
        .unwrap_or_else(|| panic!("no entity named {name}"))
}

/// The `WallGen` of `owner`'s wall part.
pub fn wall_gen(w: &World, owner: Uid) -> WallGen {
    w.query::<(&Derived, &WallGen)>()
        .iter()
        .find(|(d, _)| d.key.owner() == Some(owner))
        .map(|(_, g)| g.clone())
        .unwrap_or_else(|| panic!("no wall part for {owner}"))
}

pub fn wall_path(w: &World, owner: Uid) -> WallPath {
    let e = w.entity_by_uid(owner).unwrap();
    (*w.get::<WallPath>(e).unwrap()).clone()
}

pub fn warnings(w: &World, owner: Uid) -> Vec<String> {
    let e = w.entity_by_uid(owner).unwrap();
    w.get::<box_engine::box_core::derived::GeneratedWarnings>(e)
        .map(|g| g.0.iter().map(|x| x.message.clone()).collect())
        .unwrap_or_default()
}

pub fn flat_points(pts: &[[f32; 2]]) -> Vec<Vec3> {
    pts.iter().map(|p| Vec3::new(p[0], 0.0, p[1])).collect()
}

/// Spawns a saved wall entity directly (tests that are not about edits).
pub fn spawn_wall(w: &mut World, name: &str, pts: &[[f32; 2]], thickness: f32) -> Uid {
    let uid = Uid::new();
    w.spawn_saved(
        uid,
        name,
        (
            Transform::default(),
            WallPath {
                points: flat_points(pts),
                thickness,
                ..WallPath::default()
            },
        ),
    )
    .unwrap();
    uid
}

pub fn spawn_fence(w: &mut World, name: &str, pts: &[[f32; 2]]) -> Uid {
    let uid = Uid::new();
    w.spawn_saved(
        uid,
        name,
        (
            Transform::default(),
            FencePath {
                points: flat_points(pts),
                ..FencePath::default()
            },
        ),
    )
    .unwrap();
    uid
}

/// `n` walls laid out as square rooms of four mitred walls (4 m sides, 6 m
/// apart), the last room possibly incomplete. Returns the uids in order.
pub fn spawn_rooms(w: &mut World, n: usize) -> Vec<Uid> {
    let per_row = 16;
    (0..n)
        .map(|i| {
            let room = i / 4;
            let (ox, oz) = ((room % per_row) as f32 * 6.0, (room / per_row) as f32 * 6.0);
            let c = [[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]];
            let (a, b) = (c[i % 4], c[(i + 1) % 4]);
            spawn_wall(
                w,
                &format!("Wall {i}"),
                &[[ox + a[0], oz + a[1]], [ox + b[0], oz + b[1]]],
                0.3,
            )
        })
        .collect()
}
