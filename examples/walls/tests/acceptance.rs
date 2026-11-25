//! The `examples/walls` acceptance criteria of ROADMAP M5 and
//! docs/PROCEDURAL_DIRECTION.md §7, one test each, headless (no GPU):
//! touching walls connect; dragging one regenerates only it and the walls
//! it touches or stopped touching; undo restores a byte-identical scene and
//! identical derived mesh hashes; save and reload reproduce the meshes and
//! the file holds only paths; the fence uses the same adjacency with no
//! engine code; a second scene works; idle frames run no rule and no
//! generator.

mod common;

use std::collections::BTreeSet;

use box_engine::box_core::derived::Derived;
use box_engine::box_core::edit::EditStack;
use box_engine::box_core::scene;
use box_engine::prelude::*;
use common::*;
use walls::adjacency::{PathShape, analyse};
use walls::{FenceGen, FencePath, WallEnd, WallPath, fence, plan};

#[test]
fn two_touching_walls_connect() {
    let game = game();
    let mut w = empty_world(&game);
    let a = spawn_wall(&mut w, "A", &[[0.0, 0.0], [4.0, 0.0]], 0.3);
    let b = spawn_wall(&mut w, "B", &[[4.0, 0.0], [4.0, -3.0]], 0.3);
    settle(&mut w);
    let (ga, gb) = (wall_gen(&w, a), wall_gen(&w, b));
    assert_eq!(ga.start, WallEnd::Cap);
    assert!(matches!(ga.end, WallEnd::Mitre { .. }), "{:?}", ga.end);
    assert!(matches!(gb.start, WallEnd::Mitre { .. }), "{:?}", gb.start);
    assert_eq!(gb.end, WallEnd::Cap);
    // The two mitred ends share their corner vertices exactly.
    let meshes = w.resource::<Assets<MeshData>>();
    let mesh_of = |uid: Uid| {
        let part = w
            .query::<(&Derived, &MeshRenderer)>()
            .iter()
            .find(|(d, _)| d.key.owner() == Some(uid))
            .map(|(_, r)| r.mesh)
            .unwrap();
        meshes.get(part).unwrap().clone()
    };
    let (ma, mb) = (mesh_of(a), mesh_of(b));
    let at_corner = |m: &MeshData| -> BTreeSet<[u32; 3]> {
        m.positions
            .iter()
            .filter(|p| (p.x - 4.0).abs() < 0.2 && p.z.abs() < 0.2)
            .map(|p| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()])
            .collect()
    };
    assert_eq!(at_corner(&ma), at_corner(&mb));
    assert_eq!(at_corner(&ma).len(), 4, "the mitre line's four corners");

    // A thinner wall touching the end: a butt, the lower uid owning.
    let c = spawn_wall(&mut w, "C", &[[0.0, 0.0], [0.0, 3.0]], 0.2);
    settle(&mut w);
    let (ga, gc) = (wall_gen(&w, a), wall_gen(&w, c));
    let (owner, other) = if a < c {
        (ga.start, gc.start)
    } else {
        (gc.start, ga.start)
    };
    assert!(
        matches!(owner, WallEnd::Butt { cap: true, .. }),
        "{owner:?}"
    );
    assert!(
        matches!(other, WallEnd::Butt { cap: false, .. }),
        "{other:?}"
    );
    assert!(warnings(&w, a).is_empty() && warnings(&w, c).is_empty());
}

/// Neighbours of `uid` before or after: the walls it shares a junction or
/// T contact with, by the adjacency function itself.
fn touching(w: &World, uid: Uid) -> BTreeSet<Uid> {
    let mut shapes = Vec::new();
    for (u, p, g) in w.query::<(&Uid, &WallPath, &GlobalTransform)>().iter() {
        if p.points.len() >= 2 {
            shapes.push(PathShape {
                uid: *u,
                points: p.points.iter().map(|q| g.0.transform_point3(*q)).collect(),
                half_width: p.thickness * 0.5,
                height: p.height,
            });
        }
    }
    let adj = analyse(&shapes);
    let i = shapes.iter().position(|s| s.uid == uid).unwrap();
    let mut out: BTreeSet<Uid> = adj
        .neighbours(i)
        .into_iter()
        .map(|j| shapes[j].uid)
        .collect();
    // Fences read walls (M6): a fence end in the wall's footprint butts it.
    let wall = &shapes[i];
    for (u, p, g) in w.query::<(&Uid, &FencePath, &GlobalTransform)>().iter() {
        let ends = [p.points.first(), p.points.last()];
        let touches = ends.into_iter().flatten().any(|q| {
            let q = g.0.transform_point3(*q);
            wall.points.windows(2).any(|s| {
                let seg = box_engine::box_proc::snap::SnapSegment::new(s[0], s[1], 0.0);
                seg.closest(q).distance(q) <= wall.half_width + walls::adjacency::SNAP + 1e-4
            })
        });
        if touches {
            out.insert(*u);
        }
    }
    out
}

fn owner_of(key: &str) -> Uid {
    Uid::parse(key.split('/').next().unwrap()).unwrap()
}

/// Drags "L Mitre B" of the yard 4 m east in 40 steps through the edit
/// stack (one transaction, as a drag is): away from its L partner, then
/// onto "L Butt Owner". Each step regenerates the dragged wall and only
/// walls it touched before or touches after.
fn drag_l_mitre_b(w: &mut World, edits: &mut EditStack) -> Vec<Vec<String>> {
    let uid = uid_by_name(w, "L Mitre B");
    let start = wall_path(w, uid).points;
    let path = w.field_path("WallPath.points").unwrap();
    edits.begin("Drag wall");
    let mut per_step = Vec::new();
    for step in 1..=40 {
        let before_touch = touching(w, uid);
        let before = generations(w);
        let report_runs = rule(w, plan::RULE).runs;
        let moved: Vec<Vec3> = start
            .iter()
            .map(|p| *p + Vec3::new(step as f32 * 0.1, 0.0, 0.0))
            .collect();
        edits.set_field(w, uid, &path, &moved).unwrap();
        settle(w);
        let after_touch = touching(w, uid);
        let regen = regenerated(&before, &generations(w));
        let info = rule(w, plan::RULE);
        let fences = rule(w, fence::RULE);
        assert_eq!(info.runs, report_runs + 1, "the rule ran once for the edit");
        let allowed: BTreeSet<Uid> = before_touch
            .union(&after_touch)
            .copied()
            .chain([uid])
            .collect();
        for key in &regen {
            assert!(
                allowed.contains(&owner_of(key)),
                "step {step}: {key} regenerated but is not the dragged wall or a neighbour"
            );
        }
        assert!(
            regen.iter().any(|k| owner_of(k) == uid),
            "step {step}: the dragged wall regenerated"
        );
        // Reconcile rewrote exactly the parts that regenerated (the fence
        // rule reads walls, so it runs for every wall edit too).
        assert_eq!(
            info.last_report.updated + fences.last_report.updated,
            regen.len(),
            "step {step}"
        );
        per_step.push(regen);
    }
    edits.commit();
    per_step
}

#[test]
fn dragging_a_wall_regenerates_only_it_and_its_neighbours() {
    let game = game();
    let mut w = load_scene(&game, "yard");
    let mut edits = EditStack::new();
    let walls = w.count::<&WallPath>();
    let steps = drag_l_mitre_b(&mut w, &mut edits);
    let counts: Vec<usize> = steps.iter().map(Vec::len).collect();
    println!("parts regenerated per drag step (of {walls} walls): {counts:?}");
    // Leaving the L (mitre, then a T against the partner's end, then
    // free) and arriving at the butt owner (T, then butt) also regenerate
    // the partner; in between only the dragged wall regenerates. Fence A
    // butts the wall's face for the first steps (until the face is beyond
    // its end), so it regenerates with it.
    assert!(counts.iter().all(|c| (1..=3).contains(c)), "{counts:?}");
    assert_eq!(counts[0], 3, "leaves its L partner, moves Fence A's butt");
    assert_eq!(*counts.last().unwrap(), 2, "joins L Butt Owner");
    assert!(counts.iter().filter(|c| **c == 1).count() >= 30);
    assert_eq!(edits.undo_len(), 1, "the drag is one undo step");
    let a = uid_by_name(&w, "L Mitre A");
    assert_eq!(wall_gen(&w, a).end, WallEnd::Cap);
    let b = uid_by_name(&w, "L Mitre B");
    assert!(matches!(wall_gen(&w, b).start, WallEnd::Butt { .. }));
}

#[test]
fn undo_restores_a_byte_identical_scene_and_identical_mesh_hashes() {
    let game = game();
    let mut w = load_scene(&game, "yard");
    let scene_before = scene::save_string(&w).unwrap();
    let hashes_before = part_hashes(&w);
    assert_eq!(
        hashes_before.len(),
        w.count::<&WallPath>() + w.count::<&FencePath>()
    );
    let mut edits = EditStack::new();
    drag_l_mitre_b(&mut w, &mut edits);
    let hashes_dragged = part_hashes(&w);
    assert_ne!(hashes_dragged, hashes_before);

    edits.undo(&mut w).unwrap();
    settle(&mut w);
    assert_eq!(scene::save_string(&w).unwrap(), scene_before);
    assert_eq!(part_hashes(&w), hashes_before);

    edits.redo(&mut w).unwrap();
    settle(&mut w);
    assert_eq!(part_hashes(&w), hashes_dragged);
}

#[test]
fn save_and_reload_reproduce_the_meshes_and_the_file_holds_only_paths() {
    let game = game();
    for name in walls::authoring::SCENES {
        let mut w = load_scene(&game, name);
        // An edit first, so the file is not just the authored one.
        let uid = w
            .query::<(&Uid, &WallPath)>()
            .iter()
            .map(|(u, _)| *u)
            .min()
            .unwrap();
        let mut edits = EditStack::new();
        let path = w.field_path("WallPath.height").unwrap();
        edits.set_field(&mut w, uid, &path, &3.25f32).unwrap();
        settle(&mut w);
        let hashes = part_hashes(&w);
        let bytes = scene::save(&w).unwrap();

        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let entities = json["entities"].as_array().unwrap();
        let saved: usize = entities.len();
        let derived = w.count::<&Derived>();
        assert!(derived > 0);
        assert_eq!(saved, w.entity_count() as usize - derived, "{name}");
        let allowed = [
            "Transform",
            "WallPath",
            "FencePath",
            "Camera",
            "DirectionalLight",
            "Primitive",
            "StandardMaterial",
        ];
        let mut procedural = 0;
        for e in entities {
            let comps = e["components"].as_object().unwrap();
            for c in comps.keys() {
                assert!(allowed.contains(&c.as_str()), "{name}: {c} saved");
            }
            if comps.contains_key("WallPath") || comps.contains_key("FencePath") {
                procedural += 1;
                assert!(e["parent"].is_null());
            }
        }
        assert_eq!(procedural, w.count::<&WallPath>() + w.count::<&FencePath>());
        let text = String::from_utf8(bytes.clone()).unwrap();
        for word in ["WallGen", "FenceGen", "Derived", "MeshRenderer", "/wall"] {
            assert!(!text.contains(word), "{name}: `{word}` in the file");
        }

        let reloaded = load_bytes(&game, &bytes);
        assert_eq!(part_hashes(&reloaded), hashes, "{name}");
    }
}

#[test]
fn the_fence_shares_the_adjacency_and_needs_no_engine_code() {
    // The fence is this crate's code only: its rule calls the same
    // `adjacency::analyse` as walls and registers with the same
    // `app.add_rule`; the engine has no fence (or wall) type.
    let game = game();
    let engine_rules = Game::new().rules();
    let rules: Vec<(&str, &str)> = game
        .rules()
        .rules()
        .map(|r| (r.name, r.intent))
        .filter(|(name, _)| engine_rules.get(name).is_none())
        .collect();
    assert_eq!(
        rules,
        [(plan::RULE, "WallPath"), (fence::RULE, "FencePath")]
    );
    for t in ["WallPath", "FencePath", "WallGen", "FenceGen"] {
        assert!(
            box_engine::engine_registry().component(t).is_none(),
            "{t} is not an engine type"
        );
        assert!(game.registry().component(t).is_some());
    }
    let mut w = load_scene(&game, "yard");
    let posts = |name: &str, w: &World| {
        let uid = uid_by_name(w, name);
        w.query::<(&Derived, &FenceGen)>()
            .iter()
            .find(|(d, _)| d.key.owner() == Some(uid))
            .map(|(_, g)| (g.start_post, g.end_post))
            .unwrap()
    };
    // Fence A's end and Fence B's start meet: one shared corner post,
    // built by A (the lower uid). Fence A starts on the face of the wall
    // "L Mitre B": it butts the wall with no post (the fence rule reads
    // walls, `rule_reads`).
    assert_eq!(posts("Fence A", &w), (false, true));
    assert_eq!(posts("Fence B", &w), (false, true));
    // Fence C starts on Fence B's line (a T): its own post, snapped.
    assert_eq!(posts("Fence C", &w), (true, true));
    // Moving Fence B's start away: it gets its own post again.
    let b = uid_by_name(&w, "Fence B");
    let path = w.field_path("FencePath.points[0]").unwrap();
    let mut edits = EditStack::new();
    edits
        .set_field(&mut w, b, &path, &Vec3::new(-0.5, 0.0, 1.0))
        .unwrap();
    settle(&mut w);
    assert_eq!(posts("Fence B", &w), (true, true));
}

#[test]
fn a_second_scene_works_with_no_code_change() {
    let game = game();
    for (name, expected_warnings) in [("yard", 2), ("court", 0)] {
        // Through the player's app (rules as App systems), not the edit world.
        let options = box_engine::player::PlayerOptions {
            project: Some(walls::authoring::project_dir()),
            scene: Some(name.into()),
            ..Default::default()
        };
        let (mut app, _) = box_engine::player::build_app(&game, &options).unwrap();
        app.run_frames(2, 1.0 / 60.0);
        let w = &app.world;
        let paths = w.count::<&WallPath>() + w.count::<&FencePath>();
        assert_eq!(
            part_hashes(w).len(),
            paths,
            "{name}: one drawn part per path"
        );
        let meshes = w.resource::<Assets<MeshData>>();
        for (_, r) in w.query::<(&Derived, &MeshRenderer)>().iter() {
            assert!(meshes.get(r.mesh).unwrap().triangle_count() > 0, "{name}");
        }
        for (_, s) in w.query::<(&Derived, &ProcState)>().iter() {
            assert!(s.warnings().is_empty(), "{name}: {:?}", s.warnings());
        }
        let owners_warned: usize = w
            .query::<&box_engine::box_core::derived::GeneratedWarnings>()
            .iter()
            .count();
        assert_eq!(owners_warned, expected_warnings, "{name}");
    }
}

#[test]
fn idle_frames_run_no_rule_and_no_generator() {
    let game = game();
    let mut w = empty_world(&game);
    spawn_rooms(&mut w, 100);
    settle(&mut w);
    let runs = rule(&w, plan::RULE).runs;
    let generated = generated_total(&w);
    let start = std::time::Instant::now();
    for _ in 0..100 {
        advance(&mut w, FRAME);
        frame(&mut w);
    }
    let per_frame = start.elapsed() / 100;
    assert_eq!(rule(&w, plan::RULE).runs, runs, "no rule run");
    assert_eq!(generated_total(&w), generated, "no generation");
    println!(
        "idle frame, 100 walls (propagate + rules gate + flush stamp + renderers): {per_frame:?}"
    );
}

#[test]
fn scenes_match_authoring() {
    let game = game();
    for name in walls::authoring::SCENES {
        let built =
            walls::authoring::build_scene(&game, &walls::authoring::commands_path(name)).unwrap();
        let saved = std::fs::read(walls::authoring::scene_path(name)).unwrap();
        assert!(
            built == saved,
            "{name}.scene.json is out of date with authoring/{name}.commands.json; \
             run `cargo run -p walls --example walls_author`"
        );
    }
}
