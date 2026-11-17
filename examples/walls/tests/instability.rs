//! The instability cases of docs/PROCEDURAL_DIRECTION.md §7, each with a
//! rule or a warning, tested on the pure rule (`plan_walls`) and the
//! generator's output. Captures of the same cases: `tests/captures.rs`
//! (`instability_*.png`).

use box_engine::box_proc::mesh_builder::signed_volume;
use box_engine::prelude::*;
use walls::adjacency::SNAP;
use walls::plan::{PlannedWall, WallIntent, min_mitre_angle, plan_walls};
use walls::snap::{SNAP_RELEASE, Snapper};
use walls::{WallGen, WallPath};

fn wall(points: &[[f32; 2]], thickness: f32) -> WallPath {
    WallPath {
        points: points.iter().map(|p| Vec3::new(p[0], 0.0, p[1])).collect(),
        thickness,
        ..WallPath::default()
    }
}

fn plan(walls: &[WallPath]) -> Vec<PlannedWall> {
    let intents: Vec<WallIntent> = walls
        .iter()
        .enumerate()
        .map(|(i, p)| WallIntent {
            uid: Uid::from_u128(i as u128 + 1),
            path: p,
            transform: Affine3A::IDENTITY,
        })
        .collect();
    plan_walls(&intents)
}

fn generator(w: &PlannedWall) -> &WallGen {
    w.generator.as_ref().expect("a wall part")
}

fn kinds(w: &PlannedWall) -> [&'static str; 2] {
    let g = generator(w);
    [g.start.kind(), g.end.kind()]
}

/// The part's mesh: no generator warnings, every position finite.
fn mesh(w: &PlannedWall) -> MeshData {
    let out = generator(w).generate(&GenContext::new(0));
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    let m = out.mesh.unwrap();
    assert!(m.positions.iter().all(|p| p.is_finite()));
    assert!(m.triangle_count() > 0);
    m
}

fn has(w: &PlannedWall, text: &str) -> bool {
    w.warnings.iter().any(|m| m.contains(text))
}

/// An end hovering around the 10 cm threshold: written raw, the junction
/// flips between a mitre and a T every step; through a `Snapper` it
/// connects once and stays connected until the end is pulled beyond 15 cm.
#[test]
fn snap_threshold_flicker() {
    let a = wall(&[[0.0, 0.0], [4.0, 0.0]], 0.3);
    let target = Vec3::new(4.0, 0.0, 0.0);
    let jitter = [
        0.12f32, 0.09, 0.11, 0.095, 0.105, 0.09, 0.13, 0.08, 0.14, 0.11,
    ];
    let a_end = |b_start: Vec3| {
        let b = WallPath {
            points: vec![b_start, b_start + Vec3::new(0.0, 0.0, -3.0)],
            ..WallPath::default()
        };
        plan(&[a.clone(), b])[0]
            .generator
            .as_ref()
            .unwrap()
            .end
            .kind()
    };
    let toggles = |kinds: &[&str]| kinds.windows(2).filter(|w| w[0] != w[1]).count();

    let raw: Vec<&str> = jitter
        .iter()
        .map(|d| a_end(target + Vec3::new(*d, 0.0, 0.0)))
        .collect();
    let mut snapper = Snapper::new();
    let snapped: Vec<&str> = jitter
        .iter()
        .map(|d| a_end(snapper.snap(target + Vec3::new(*d, 0.0, 0.0), &[target])))
        .collect();
    println!("raw: {raw:?}\nsnapped: {snapped:?}");
    assert!(toggles(&raw) >= 6, "{raw:?}");
    assert_eq!(toggles(&snapped), 1, "{snapped:?}");
    assert_eq!(snapped.last(), Some(&"mitre"));
    // Pulled beyond SNAP_RELEASE, it lets go.
    let away = target + Vec3::new(SNAP_RELEASE + 0.01, 0.0, 0.0);
    assert_eq!(snapper.snap(away, &[target]), away);
    const { assert!(SNAP < SNAP_RELEASE) };
}

/// Three or more ends at one point: the longest pair joins, the others
/// butt against its faces; an end that still overlaps gets a warning.
#[test]
fn three_or_more_walls_at_a_point() {
    // A T made of three ends: the straight pair mitres, the stem butts.
    let out = plan(&[
        wall(&[[0.0, 0.0], [4.0, 0.0]], 0.3),
        wall(&[[0.0, 0.0], [-4.0, 0.0]], 0.3),
        wall(&[[0.0, 0.0], [0.0, 3.0]], 0.3),
    ]);
    assert_eq!(kinds(&out[0])[0], "mitre");
    assert_eq!(kinds(&out[1])[0], "mitre");
    assert_eq!(kinds(&out[2])[0], "butt-owner", "capped butt");
    assert!(out.iter().all(|w| w.warnings.is_empty()), "{out:?}");
    out.iter().for_each(|w| {
        mesh(w);
    });

    // An X of four ends: two straight pairs.
    let out = plan(&[
        wall(&[[0.0, 0.0], [4.0, 0.0]], 0.3),
        wall(&[[0.0, 0.0], [-4.0, 0.0]], 0.3),
        wall(&[[0.0, 0.0], [0.0, 3.0]], 0.3),
        wall(&[[0.0, 0.0], [0.0, -3.0]], 0.3),
    ]);
    assert!(out.iter().all(|w| w.warnings.is_empty()), "{out:?}");

    // Two stems 10° apart on the same side: they overlap each other.
    let out = plan(&[
        wall(&[[0.0, 0.0], [4.0, 0.0]], 0.3),
        wall(&[[0.0, 0.0], [-4.0, 0.0]], 0.3),
        wall(&[[0.0, 0.0], [0.0, 3.0]], 0.3),
        wall(&[[0.0, 0.0], [0.52, 2.95]], 0.3),
    ]);
    assert!(
        has(&out[2], "unresolved") || has(&out[3], "unresolved"),
        "{out:?}"
    );
}

/// Corners between two walls below the mitre limit are bevelled (a fill
/// triangle on the owner, a warning on both); above it they mitre.
#[test]
fn acute_angles() {
    let limit = min_mitre_angle().to_degrees();
    assert!((limit - 28.955).abs() < 0.01, "{limit}");
    for deg in [
        5.0f32, 10.0, 20.0, 28.0, 30.0, 45.0, 60.0, 90.0, 120.0, 150.0, 170.0,
    ] {
        let r = deg.to_radians();
        let out = plan(&[
            wall(&[[4.0, 0.0], [0.0, 0.0]], 0.3),
            wall(&[[0.0, 0.0], [4.0 * r.cos(), 4.0 * r.sin()]], 0.3),
        ]);
        let bevelled = deg < limit;
        let fills = generator(&out[0]).fills.len() + generator(&out[1]).fills.len();
        if bevelled {
            assert_eq!(kinds(&out[0])[1], "cap", "{deg}°");
            assert_eq!(fills, 3, "{deg}°: one fill triangle");
            assert!(has(&out[0], "sharper than the mitre limit"), "{deg}°");
            assert!(has(&out[1], "sharper than the mitre limit"), "{deg}°");
        } else {
            assert_eq!(kinds(&out[0])[1], "mitre", "{deg}°");
            assert_eq!(fills, 0);
            assert!(out.iter().all(|w| w.warnings.is_empty()), "{deg}°");
        }
        let volume: f32 = out.iter().map(|w| signed_volume(&mesh(w))).sum();
        assert!(volume > 0.0, "{deg}°");
    }
    // Inside one wall the sweep bevels; the rule warns on the owner.
    let out = plan(&[wall(&[[0.0, 0.0], [4.0, 0.0], [0.5, 0.8]], 0.3)]);
    assert!(has(&out[0], "sharper than the mitre limit"), "{out:?}");
}

/// Walls of different thickness meeting end to end butt at any angle: the
/// subordinate stops on the owner's inner face and never enters it.
#[test]
fn mixed_thickness() {
    for deg in [60.0f32, 90.0, 120.0] {
        let r = deg.to_radians();
        let out = plan(&[
            wall(&[[-4.0, 0.0], [0.0, 0.0]], 0.4),
            wall(&[[0.0, 0.0], [-3.0 * r.cos(), 3.0 * r.sin()]], 0.2),
        ]);
        assert_eq!(kinds(&out[0])[1], "butt-owner", "{deg}°");
        assert_eq!(kinds(&out[1])[0], "butt", "{deg}°");
        assert!(out.iter().all(|w| w.warnings.is_empty()), "{deg}°");
        // The thin wall's vertices are all on or beyond the owner's inner
        // face (z >= 0.2).
        let thin = mesh(&out[1]);
        let min_z = thin.positions.iter().map(|p| p.z).fold(f32::MAX, f32::min);
        assert!((min_z - 0.2).abs() < 1e-4, "{deg}°: {min_z}");
        mesh(&out[0]);
    }
    // In line: the bigger section caps, the other is open against it.
    let out = plan(&[
        wall(&[[-4.0, 0.0], [0.0, 0.0]], 0.4),
        wall(&[[0.0, 0.0], [4.0, 0.0]], 0.2),
    ]);
    assert_eq!(kinds(&out[0])[1], "cap");
    assert_eq!(kinds(&out[1])[0], "open");
    // Different heights: the taller one owns the butt even with the higher uid.
    let mut tall = wall(&[[0.0, 0.0], [0.0, 3.0]], 0.3);
    tall.height = 4.0;
    let out = plan(&[wall(&[[-4.0, 0.0], [0.0, 0.0]], 0.3), tall]);
    assert_eq!(kinds(&out[1])[0], "butt-owner");
    assert_eq!(kinds(&out[0])[1], "butt");
}

/// An end meeting a body mid-side is cut on the near face, wherever within
/// reach it ends (on the centreline, on the face, 8 cm short of it).
#[test]
fn t_junction_mid_side() {
    for z in [0.0f32, 0.15, 0.23] {
        let out = plan(&[
            wall(&[[-4.0, 0.0], [4.0, 0.0]], 0.3),
            wall(&[[0.5, z], [0.5, 4.0]], 0.2),
        ]);
        assert_eq!(kinds(&out[0]), ["cap", "cap"], "the body is untouched");
        assert_eq!(kinds(&out[1])[0], "butt", "{z}");
        assert!(out.iter().all(|w| w.warnings.is_empty()), "{z}: {out:?}");
        let stem = mesh(&out[1]);
        let min_z = stem.positions.iter().map(|p| p.z).fold(f32::MAX, f32::min);
        assert!((min_z - 0.15).abs() < 1e-4, "{z}: {min_z}");
    }
    // Grazing angles cannot be cut cleanly: capped with a warning.
    let out = plan(&[
        wall(&[[-4.0, 0.0], [4.0, 0.0]], 0.3),
        wall(&[[0.0, 0.2], [4.0, 1.5]], 0.3),
    ]);
    assert_eq!(kinds(&out[1])[0], "cap");
    assert!(has(&out[1], "sharp angle"), "{out:?}");
}

#[test]
fn a_wall_crossing_another_warns_on_both() {
    let out = plan(&[
        wall(&[[-4.0, 0.0], [4.0, 0.0]], 0.3),
        wall(&[[0.0, -3.0], [0.0, 3.0]], 0.3),
    ]);
    for w in &out {
        assert!(has(w, "crosses another wall at (0.00, 0.00)"), "{out:?}");
        mesh(w);
    }
}

#[test]
fn zero_length_segments() {
    let out = plan(&[wall(
        &[[0.0, 0.0], [0.0, 0.0], [4.0, 0.0], [4.0, 0.0005]],
        0.3,
    )]);
    assert_eq!(generator(&out[0]).points.len(), 2);
    assert!(has(&out[0], "2 zero-length segment(s)"), "{out:?}");
    mesh(&out[0]);
    let out = plan(&[wall(&[[1.0, 1.0], [1.0, 1.0]], 0.3)]);
    assert!(out[0].generator.is_none());
    assert!(has(&out[0], "at least two distinct points"));
    // A wall shorter than the snap distance between two others collapses.
    let out = plan(&[
        wall(&[[0.0, 0.0], [4.0, 0.0]], 0.3),
        wall(&[[4.0, 0.0], [4.05, 0.0]], 0.3),
    ]);
    assert!(out[1].generator.is_none(), "{out:?}");
    assert!(has(&out[1], "collapses"));
}

/// A stem dragged through a body in 5 cm steps: free, T on the near face,
/// a crossing past the far side; every step builds finite geometry.
#[test]
fn dragging_through_a_junction() {
    let mut seen: Vec<String> = Vec::new();
    for step in 0..=40 {
        let z = 1.0 - step as f32 * 0.05;
        let out = plan(&[
            wall(&[[-4.0, 0.0], [4.0, 0.0]], 0.3),
            wall(&[[0.5, z], [0.5, 4.0]], 0.2),
        ]);
        mesh(&out[0]);
        mesh(&out[1]);
        let state = if has(&out[1], "crosses") {
            "crossing".to_string()
        } else {
            kinds(&out[1])[0].to_string()
        };
        if seen.last() != Some(&state) {
            seen.push(state);
        }
    }
    println!("{seen:?}");
    assert_eq!(seen, ["cap", "butt", "crossing"]);
}

#[test]
fn collinear_overlapping_walls_warn() {
    let out = plan(&[
        wall(&[[0.0, 0.0], [6.0, 0.0]], 0.3),
        wall(&[[3.0, 0.0], [9.0, 0.0]], 0.3),
    ]);
    for w in &out {
        assert!(has(w, "runs along another wall for 3.00 m"), "{out:?}");
    }
    // Starting at the same point, folded back over each other.
    let out = plan(&[
        wall(&[[0.0, 0.0], [6.0, 0.0]], 0.3),
        wall(&[[0.0, 0.0], [4.0, 0.0]], 0.3),
    ]);
    for w in &out {
        assert!(has(w, "doubles back"), "{out:?}");
        assert_eq!(kinds(w)[0], "cap");
    }
}

/// Snapping moves partner ends to the junction point, which is the first
/// end's position in uid order. Moving a wall that owns its junction
/// points therefore also moves its partners' ends, and their other
/// junctions change: one step further than "the walls it touches". A
/// small move of a wall that owns no junction point is absorbed entirely.
#[test]
fn snapped_partners_carry_a_move_one_step_further() {
    let room = |nudge: [f32; 2], which: usize| -> Vec<WallPath> {
        let c = [[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]];
        (0..4)
            .map(|i| {
                let d = if i == which { nudge } else { [0.0, 0.0] };
                let (a, b) = (c[i], c[(i + 1) % 4]);
                wall(
                    &[[a[0] + d[0], a[1] + d[1]], [b[0] + d[0], b[1] + d[1]]],
                    0.3,
                )
            })
            .collect()
    };
    let changed = |which: usize| -> Vec<usize> {
        let before = plan(&room([0.0, 0.0], which));
        let after = plan(&room([0.03, 0.0], which));
        (0..4).filter(|i| before[*i] != after[*i]).collect()
    };
    // Wall 0 owns both its junctions (lowest uid): all four change.
    assert_eq!(changed(0), [0, 1, 2, 3]);
    // Wall 2 owns its junction with wall 3: it, both partners and wall 0
    // (whose mitre with wall 3 follows wall 3's moved end).
    assert_eq!(changed(2), [0, 1, 2, 3]);
    // Wall 3 owns neither junction: both its ends stay snapped to points
    // its partners own, so a 3 cm nudge changes nothing at all.
    assert_eq!(changed(3), Vec::<usize>::new());
    // Coincident data (as the Snapper writes it) never moves a partner.
    let mut walls = room([0.0, 0.0], 0);
    let before = plan(&walls);
    walls[2].height = 2.6; // a non-geometric edit of a non-owner
    let after = plan(&walls);
    let changed: Vec<usize> = (0..4).filter(|i| before[*i] != after[*i]).collect();
    assert_eq!(changed, [1, 2, 3], "it and the walls it touches");
}
