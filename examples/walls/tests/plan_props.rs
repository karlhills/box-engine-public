//! Property tests of the pure `walls` and `fences` rules (no engine):
//! random paths on a coarse lattice (so ends often meet, cross and
//! overlap) with sub-snap jitter.
//!
//! Positions are not compared after moving every owner: float rounding of
//! moved points can flip a decision that sits exactly at the 10 cm
//! threshold. The court scene and `tests/acceptance.rs` cover moved and
//! rotated owners.

use box_engine::prelude::*;
use proptest::prelude::*;
use walls::WallEnd;
use walls::adjacency::SNAP;
use walls::fence::{FenceIntent, plan_fences};
use walls::plan::{WallIntent, plan_walls};
use walls::{FencePath, WallPath};

fn point() -> impl Strategy<Value = Vec3> {
    (0i32..5, 0i32..5, -0.06f32..0.06, -0.06f32..0.06)
        .prop_map(|(x, z, jx, jz)| Vec3::new(x as f32 * 2.0 + jx, 0.0, z as f32 * 2.0 + jz))
}

fn wall_path() -> impl Strategy<Value = WallPath> {
    (
        prop::collection::vec(point(), 1..5),
        prop::sample::select(vec![0.2f32, 0.3, 0.3, 0.45]),
        prop::sample::select(vec![2.5f32, 2.5, 3.0]),
    )
        .prop_map(|(points, thickness, height)| WallPath {
            points,
            height,
            thickness,
        })
}

fn intents<'a>(walls: &'a [WallPath], ids: &[u128]) -> Vec<WallIntent<'a>> {
    walls
        .iter()
        .zip(ids)
        .map(|(p, id)| WallIntent {
            uid: Uid::from_u128(*id),
            path: p,
            transform: Affine3A::IDENTITY,
        })
        .collect()
}

fn finite_end(e: WallEnd) -> bool {
    match e {
        WallEnd::Cap | WallEnd::Open => true,
        WallEnd::Mitre { neighbour_dir } => neighbour_dir.is_finite(),
        WallEnd::Butt { point, normal, .. } => point.is_finite() && normal.is_finite(),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn walls_rule_is_pure_order_free_and_builds_finite_meshes(
        walls in prop::collection::vec(wall_path(), 1..7),
    ) {
        let ids: Vec<u128> = (1..=walls.len() as u128).collect();
        let out = plan_walls(&intents(&walls, &ids));
        prop_assert_eq!(out.len(), walls.len());
        // Deterministic.
        prop_assert_eq!(&plan_walls(&intents(&walls, &ids)), &out);
        // Independent of the input order.
        let mut rev: Vec<(u128, WallPath)> = ids.iter().copied().zip(walls.iter().cloned()).collect();
        rev.reverse();
        let (rids, rwalls): (Vec<u128>, Vec<WallPath>) = rev.into_iter().unzip();
        prop_assert_eq!(&plan_walls(&intents(&rwalls, &rids)), &out);

        for w in &out {
            let Some(g) = &w.generator else { continue };
            prop_assert!(g.points.iter().all(|p| p.is_finite()));
            prop_assert!(finite_end(g.start) && finite_end(g.end));
            prop_assert!(g.fills.iter().all(|p| p.is_finite()) && g.fills.len() % 3 == 0);
            let built = g.generate(&GenContext::new(0));
            let mesh = built.mesh.expect("a mesh");
            prop_assert!(mesh.positions.iter().all(|p| p.is_finite()));
            prop_assert!(!built.warnings.iter().any(|m| m.contains("panicked")));
        }
    }

    #[test]
    fn fences_rule_is_pure_and_builds_each_post_once(
        paths in prop::collection::vec(prop::collection::vec(point(), 2..4), 1..6),
    ) {
        let fences: Vec<FencePath> = paths
            .into_iter()
            .map(|points| FencePath { points, ..FencePath::default() })
            .collect();
        let ids: Vec<FenceIntent> = fences
            .iter()
            .enumerate()
            .map(|(i, p)| FenceIntent {
                uid: Uid::from_u128(i as u128 + 1),
                path: p,
                transform: Affine3A::IDENTITY,
            })
            .collect();
        let out = plan_fences(&ids);
        let mut rev = ids.clone();
        rev.reverse();
        prop_assert_eq!(&plan_fences(&rev), &out);
        // Every fence end has an end post within SNAP of it, and no two end
        // posts are that close (a shared post is built once, never twice).
        let mut ends: Vec<(Vec3, bool)> = Vec::new();
        for f in &out {
            let Some(g) = &f.generator else { continue };
            ends.push((g.points[0], g.start_post));
            ends.push((*g.points.last().unwrap(), g.end_post));
            let built = g.generate(&GenContext::new(0));
            prop_assert!(built.mesh.unwrap().positions.iter().all(|p| p.is_finite()));
        }
        for (p, _) in &ends {
            let posts = ends
                .iter()
                .filter(|(q, post)| *post && q.distance(*p) <= SNAP)
                .count();
            prop_assert!(posts >= 1, "no post near {:?}", p);
        }
        let posts: Vec<Vec3> = ends.iter().filter(|e| e.1).map(|e| e.0).collect();
        for (i, a) in posts.iter().enumerate() {
            for b in &posts[i + 1..] {
                prop_assert!(a.distance(*b) > SNAP, "posts at {:?} and {:?}", a, b);
            }
        }
    }
}
