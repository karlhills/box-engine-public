//! Property tests for the 2D polygon kernel (docs/ROADMAP.md M5 test strategy).

use box_proc::poly::{self, Join, Polygon};
use glam::Vec2;
use proptest::prelude::*;

/// A simple (star-shaped) polygon: sorted angles, random radii, random centre.
fn star(max_points: usize) -> impl Strategy<Value = Polygon> {
    star_with_centre(max_points).prop_map(|(p, _)| p)
}

/// [`star`] and its centre: a point inside from which every edge is visible
/// (the polygon's kernel), so a disc around it smaller than the distance to
/// the nearest edge is inside.
fn star_with_centre(max_points: usize) -> impl Strategy<Value = (Polygon, Vec2)> {
    (
        -5.0f32..5.0,
        -5.0f32..5.0,
        // At least 4 points: with spread angles the largest gap is then
        // 1.9/4 of a turn (under half), so the centre is inside and sees
        // every edge. With 3 points a gap can exceed half a turn and the
        // centre falls outside.
        prop::collection::vec((0.0f32..1.0, 0.5f32..3.0), 4..max_points),
    )
        .prop_map(|(cx, cy, mut pts)| {
            pts.sort_by(|a, b| a.0.total_cmp(&b.0));
            let n = pts.len() as f32;
            let polygon = Polygon::new(
                pts.iter()
                    .enumerate()
                    // Spread angles so consecutive points never coincide.
                    .map(|(i, &(jitter, r))| {
                        let a = std::f32::consts::TAU * (i as f32 + jitter * 0.9) / n;
                        Vec2::new(cx, cy) + Vec2::from_angle(a) * r
                    })
                    .collect(),
            );
            (polygon, Vec2::new(cx, cy))
        })
}

/// Arbitrary junk: any floats, including NaN, infinities and huge values.
fn junk_ring() -> impl Strategy<Value = Vec<Vec2>> {
    let coord = prop_oneof![
        8 => -10.0f32..10.0,
        1 => Just(f32::NAN),
        1 => Just(f32::INFINITY),
        1 => Just(f32::NEG_INFINITY),
        1 => Just(1e30f32),
        1 => Just(0.0f32),
    ];
    prop::collection::vec(
        (coord.clone(), coord).prop_map(|(x, y)| Vec2::new(x, y)),
        0..12,
    )
}

/// Distance from `p` to the segment `a`–`b`.
fn segment_distance(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(f32::MIN_POSITIVE)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

fn close(a: f32, b: f32, scale: f32) -> bool {
    (a - b).abs() <= 2e-3 * scale.max(1.0)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn boolean_area_identities(a in star(24), b in star(24)) {
        let (sa, sb) = (vec![a.clone()], vec![b.clone()]);
        let (aa, ab) = (a.area(), b.area());
        let u = poly::total_area(&poly::union(&sa, &sb));
        let i = poly::total_area(&poly::intersection(&sa, &sb));
        let d = poly::total_area(&poly::difference(&sa, &sb));
        let x = poly::total_area(&poly::xor(&sa, &sb));
        let scale = aa + ab;
        prop_assert!(close(u, aa + ab - i, scale), "union {u} vs {aa} + {ab} - {i}");
        prop_assert!(close(d, aa - i, scale), "difference {d} vs {aa} - {i}");
        prop_assert!(close(x, u - i, scale), "xor {x} vs {u} - {i}");
        prop_assert!(i <= aa.min(ab) + 2e-3 * scale);
        prop_assert!(u + 2e-3 * scale >= aa.max(ab));
    }

    #[test]
    fn triangulation_covers_area((outer, c) in star_with_centre(40), hole_scale in 0.05f32..0.9) {
        // A disc around the star's centre, smaller than the distance to the
        // nearest edge, is inside the polygon (a hole sized by the distance
        // to the vertices can cross an edge of a thin polygon, which makes
        // the input invalid).
        let ring = &outer.outer;
        let clearance = (0..ring.len())
            .map(|i| segment_distance(c, ring[i], ring[(i + 1) % ring.len()]))
            .fold(f32::MAX, f32::min);
        let hole: Vec<Vec2> = (0..8)
            .map(|i| c + Vec2::from_angle(std::f32::consts::TAU * i as f32 / 8.0) * clearance * hole_scale)
            .collect();
        let p = outer.with_hole(hole);
        let t = poly::triangulate(&p);
        prop_assert!(close(t.area(), p.area(), p.area()), "{} vs {}", t.area(), p.area());
        // Counter-clockwise triangles only.
        for tri in t.indices.as_chunks::<3>().0 {
            let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| t.points[i as usize]);
            prop_assert!((b - a).perp_dot(c - a) > 0.0);
        }
    }

    #[test]
    fn offsets_are_monotonic(a in star(24), d in 0.01f32..0.4) {
        let s = vec![a.clone()];
        let base = poly::total_area(&poly::make_valid(&s));
        for join in [Join::Miter { limit: 4.0 }, Join::Round { tolerance: 0.01 }, Join::Bevel] {
            let grow = poly::total_area(&poly::offset(&s, d, join));
            let grow2 = poly::total_area(&poly::offset(&s, d * 2.0, join));
            let shrink = poly::total_area(&poly::offset(&s, -d, join));
            prop_assert!(shrink <= base + 1e-3, "{join:?}: shrink {shrink} > {base}");
            prop_assert!(grow >= base - 1e-3, "{join:?}: grow {grow} < {base}");
            prop_assert!(grow2 >= grow - 1e-3, "{join:?}: grow2 {grow2} < {grow}");
        }
    }

    #[test]
    fn no_panics_on_junk(a in junk_ring(), b in junk_ring(), h in junk_ring(), d in prop_oneof![-2.0f32..2.0, Just(f32::NAN), Just(f32::INFINITY)]) {
        let pa = Polygon::new(a.clone()).with_hole(h.clone());
        let pb = Polygon::new(b.clone());
        let (sa, sb) = (vec![pa.clone()], vec![pb.clone()]);
        let results = [
            poly::union(&sa, &sb),
            poly::difference(&sa, &sb),
            poly::intersection(&sa, &sb),
            poly::xor(&sa, &sb),
            poly::make_valid(&sa),
            poly::offset(&sa, d, Join::Miter { limit: 2.0 }),
            poly::offset(&sa, d, Join::Round { tolerance: d }),
        ];
        for r in &results {
            let area = poly::total_area(r);
            prop_assert!(area.is_finite() && area >= 0.0, "{area}");
            for p in r {
                prop_assert!(p.outer.iter().all(|q| q.is_finite()));
            }
        }
        let t = poly::triangulate(&pa);
        prop_assert!(t.indices.iter().all(|&i| (i as usize) < t.points.len()));
        let _ = poly::simplify_ring(&a, 0.01);
        let _ = poly::chamfer_corners(&a, d);
        let _ = poly::round_corners(&a, d, 4);
        let _ = pa.contains(Vec2::ZERO);
        let _ = pa.area();
    }
}
