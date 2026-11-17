//! Property tests over random valid parameters: every generator output is
//! finite, valid and consistently wound, the builder never has to drop a
//! degenerate triangle (openings included), closed shells are closed
//! where claimed, and output is deterministic.
//!
//! `cargo test -p box_arch --test props`

mod common;

use box_arch::{
    Footprint, Opening, OpeningKind, OpeningTop, Platform, PlatformEdge, RidgeAxis, Roof, RoofEnd,
    RoofShape, Stair, StairStyle, Wall, WallEnd, WallPath, WallPoint,
};
use box_core::pmath;
use box_proc::mesh_hash;
use common::{boundary_edges, check_output, slots, wide_open_edges};
use glam::{Vec2, Vec3};
use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;

fn kind() -> impl Strategy<Value = OpeningKind> {
    prop_oneof![
        Just(OpeningKind::Window),
        Just(OpeningKind::Door),
        Just(OpeningKind::Passage)
    ]
}

fn top() -> impl Strategy<Value = OpeningTop> {
    prop_oneof![
        Just(OpeningTop::Flat),
        Just(OpeningTop::Round),
        Just(OpeningTop::Pointed)
    ]
}

fn opening() -> impl Strategy<Value = Opening> {
    (
        kind(),
        top(),
        0.0f32..20.0,
        0.3f32..2.0,
        0.5f32..3.0,
        0.0f32..2.0,
    )
        .prop_map(|(kind, top, offset, width, height, sill)| Opening {
            kind,
            top,
            offset,
            width,
            height,
            sill,
        })
}

/// Polylines of 2–5 points with segments of 1–6 m turning by any angle.
fn polyline() -> impl Strategy<Value = Vec<Vec2>> {
    prop::collection::vec((1.0f32..6.0, -3.0f32..3.0), 1..5).prop_map(|segs| {
        let mut p = vec![Vec2::ZERO];
        let mut heading = 0.0f32;
        for (len, turn) in segs {
            heading += turn;
            let last = *p.last().unwrap();
            p.push(last + Vec2::new(pmath::cos(heading), pmath::sin(heading)) * len);
        }
        p
    })
}

fn end() -> impl Strategy<Value = WallEnd> {
    prop_oneof![
        Just(WallEnd::Cap),
        Just(WallEnd::Open),
        Just(WallEnd::Abut),
        (0.0f32..1.0).prop_map(|distance| WallEnd::Extend { distance }),
        (0.0f32..std::f32::consts::TAU).prop_map(|a| WallEnd::Mitre {
            neighbour: Vec2::new(pmath::cos(a), pmath::sin(a))
        }),
        (-1.0f32..1.0, 0.0f32..std::f32::consts::PI, any::<bool>()).prop_map(|(d, a, cap)| {
            WallEnd::Butt {
                point: Vec2::new(d, d * 0.5),
                direction: Vec2::new(pmath::cos(a), pmath::sin(a)),
                cap,
                band_shift: 0.035,
                concave: [cap, !cap],
            }
        }),
    ]
}

fn wall() -> impl Strategy<Value = Wall> {
    (
        polyline(),
        prop::collection::vec(-0.6f32..0.6, 5),
        any::<bool>(),
        0.1f32..0.8,
        1.0f32..8.0,
        prop::collection::vec(opening(), 0..5),
        (end(), end()),
        (
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
            prop_oneof![Just(0.0f32), Just(1.0)],
        ),
    )
        .prop_map(
            |(
                points,
                bulges,
                closed,
                thickness,
                height,
                openings,
                (start, end),
                (plinth, cornice, coping, ao),
            )| {
                let closed = closed && points.len() >= 3;
                Wall {
                    path: WallPath::Points {
                        points: points
                            .iter()
                            .zip(bulges.iter().cycle())
                            .map(|(p, b)| WallPoint::new(p.x, p.y).with_bulge(*b))
                            .collect(),
                        closed,
                    },
                    thickness,
                    height,
                    start,
                    end,
                    openings,
                    plinth,
                    cornice,
                    coping,
                    ao,
                    ..Wall::default()
                }
            },
        )
}

/// The wall's shell (faces, reveals, top) must be closed except along its
/// foot when both ends are capped (or it is a loop).
///
/// `gap`: cracks narrower than this are tolerated. 0 for walls in general;
/// under sloped roofs occlusion slices can cross within a millimetre of a
/// roof kink and leave sub-millimetre T-junction slivers, so those walls
/// are checked for cracks wider than 2 mm.
fn assert_shell_closed_but_foot(w: &Wall, gap: f32) {
    let out = w.build(None);
    let m = check_output(&out);
    let closed = matches!(w.path, WallPath::Points { closed: true, .. });
    let capped = closed || (w.start == WallEnd::Cap && w.end == WallEnd::Cap);
    // A wall too thick for its corners says so; it is not claimed closed.
    let folded = out.warnings.iter().any(|w| w.contains("overlaps itself"));
    if !capped || folded {
        return;
    }
    let shell = slots(&m, &[0, 1, 2]);
    let open: Vec<_> = boundary_edges(&shell, 2e-5)
        .into_iter()
        .filter(|(a, b)| a.y.abs() > 1e-4 || b.y.abs() > 1e-4)
        .collect();
    let open = if gap > 0.0 {
        wide_open_edges(&open, gap)
    } else {
        open
    };
    assert!(
        open.is_empty(),
        "open edges above the foot: {:?}",
        &open[..open.len().min(8)]
    );
}

/// 300 cases; failures are saved next to this file in
/// `props.proptest-regressions` (commit new lines) and re-run first. The
/// default `SourceParallel` looks for a `lib.rs` above `tests/`, warns
/// that it found none and only then falls back to this file.
fn config() -> ProptestConfig {
    ProptestConfig {
        cases: 300,
        failure_persistence: Some(Box::new(FileFailurePersistence::WithSource(
            "proptest-regressions",
        ))),
        ..ProptestConfig::default()
    }
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn walls_are_valid_closed_and_deterministic(w in wall()) {
        assert_shell_closed_but_foot(&w, 0.0);
        let a = w.build(None);
        let b = w.build(None);
        prop_assert_eq!(a.warnings, b.warnings);
        prop_assert_eq!(
            a.mesh.as_ref().map(mesh_hash),
            b.mesh.as_ref().map(mesh_hash)
        );
    }

    #[test]
    fn straight_walls_with_openings_never_degenerate(
        len in 2.0f32..20.0,
        t in 0.1f32..0.8,
        h in 2.0f32..8.0,
        openings in prop::collection::vec(opening(), 0..8),
        ao in prop_oneof![Just(0.0f32), Just(1.0)],
    ) {
        let w = Wall {
            thickness: t,
            height: h,
            openings,
            ao,
            plinth: true,
            ..Wall::through(&[Vec2::ZERO, Vec2::new(len, 0.0)])
        };
        assert_shell_closed_but_foot(&w, 0.0);
    }

    #[test]
    fn walls_under_roofs_are_valid(
        sx in 2.0f32..10.0,
        sz in 2.0f32..10.0,
        pitch in 0.1f32..1.2,
        hip in any::<bool>(),
        openings in prop::collection::vec(opening(), 0..6),
    ) {
        let t = 0.3;
        let roof = Roof {
            shape: if hip {
                RoofShape::Hip { size: Vec2::new(sx, sz), ridge: RidgeAxis::Longer }
            } else {
                RoofShape::Gable { size: Vec2::new(sx, sz), ridge: RidgeAxis::Longer }
            },
            pitch,
            ..Roof::default()
        };
        let (hx, hz) = (sx * 0.5 - t * 0.5, sz * 0.5 - t * 0.5);
        let w = Wall {
            thickness: t,
            roof: Some(roof.underside().transformed(&glam::Affine3A::from_translation(Vec3::new(0.0, 3.0, 0.0)))),
            openings,
            plinth: true,
            cornice: true,
            ..Wall::closed_loop(&[
                Vec2::new(hx, hz),
                Vec2::new(hx, -hz),
                Vec2::new(-hx, -hz),
                Vec2::new(-hx, hz),
            ])
        };
        assert_shell_closed_but_foot(&w, 2e-3);
    }

    #[test]
    fn roofs_are_valid_and_their_slab_is_closed(
        sx in 0.5f32..12.0,
        sz in 0.5f32..12.0,
        r in 0.3f32..5.0,
        shape in 0usize..5,
        pitch in 0.05f32..1.4,
        overhang in 0.0f32..1.5,
        thickness in 0.02f32..0.6,
        ends in (any::<bool>(), any::<bool>(), any::<bool>()),
        boards in any::<bool>(),
        ridge in prop_oneof![Just(RidgeAxis::Longer), Just(RidgeAxis::X), Just(RidgeAxis::Z)],
    ) {
        let size = Vec2::new(sx, sz);
        let shape = match shape {
            0 => RoofShape::Gable { size, ridge },
            1 => RoofShape::Hip { size, ridge },
            2 => RoofShape::Shed { size, ridge, high_abuts: ends.2 },
            3 => RoofShape::Cone { radius: r, segments: 24 },
            _ => RoofShape::Dome { radius: r, rise: r * 0.8, segments: 24, rings: 8 },
        };
        let roof = Roof {
            shape,
            pitch,
            overhang,
            thickness,
            start: if ends.0 { RoofEnd::Abut } else { RoofEnd::Free },
            end: if ends.1 { RoofEnd::Abut } else { RoofEnd::Free },
            boards,
            ..Roof::default()
        };
        let out = roof.build();
        let m = check_output(&out);
        prop_assert!(
            out.warnings.iter().all(|w| w.contains("ridge axis ignored")),
            "{:?}",
            out.warnings
        );
        let slab = slots(&m, &[0, 1]);
        let open = boundary_edges(&slab, 2e-5);
        prop_assert!(open.is_empty(), "slab open edges: {:?}", &open[..open.len().min(6)]);
        prop_assert_eq!(mesh_hash(&m), mesh_hash(&roof.build().mesh.unwrap()));
    }

    #[test]
    fn platforms_are_valid(
        // Star-shaped outlines: points at increasing angles around the origin.
        pts in prop::collection::vec((0.3f32..5.0, 0.05f32..1.0), 3..9),
        thickness in 0.05f32..2.0,
        edge in 0usize..3,
        h in 0.05f32..1.0,
        wdt in 0.05f32..1.0,
    ) {
        let p = Platform {
            footprint: Footprint::Polygon {
                points: {
                    let total: f32 = pts.iter().map(|p| p.1).sum();
                    let mut a = 0.0f32;
                    pts.iter()
                        .map(|&(r, da)| {
                            a += da / total * std::f32::consts::TAU * 0.999;
                            Vec2::new(pmath::cos(a), pmath::sin(a)) * r
                        })
                        .collect()
                },
            },
            thickness,
            edge: match edge {
                0 => PlatformEdge::None,
                1 => PlatformEdge::Trim { height: h, proud: wdt * 0.2 },
                _ => PlatformEdge::Lip { height: h, width: wdt },
            },
            ..Platform::default()
        };
        // Any outline gives valid output (closedness is checked on
        // regular outlines below: near-collinear random points can leave
        // T-junctions in the triangulated caps).
        check_output(&p.build());
    }

    #[test]
    fn regular_platforms_are_closed(
        round in any::<bool>(),
        size in (0.3f32..8.0, 0.3f32..8.0),
        segments in 3u32..48,
        thickness in 0.05f32..2.0,
        edge in 0usize..2,
        h in 0.05f32..1.0,
        wdt in 0.05f32..0.5,
    ) {
        let p = Platform {
            footprint: if round {
                Footprint::Circle { radius: size.0, segments }
            } else {
                Footprint::Rectangle { size: Vec2::new(size.0, size.1) }
            },
            thickness,
            edge: if edge == 0 {
                PlatformEdge::None
            } else {
                PlatformEdge::Lip { height: h, width: wdt }
            },
            ..Platform::default()
        };
        let out = p.build();
        let m = check_output(&out);
        if out.warnings.is_empty() {
            let open = boundary_edges(&m, 2e-5);
            prop_assert!(open.is_empty(), "platform open edges: {:?}", &open[..open.len().min(6)]);
        }
    }

    #[test]
    fn stairs_are_valid_and_closed(
        dx in -6.0f32..6.0,
        dz in -6.0f32..6.0,
        rise in -4.0f32..4.0,
        width in 0.3f32..3.0,
        max_riser in 0.1f32..0.3,
        landing in 0.0f32..2.0,
        open in any::<bool>(),
    ) {
        let s = Stair {
            start: Vec3::ZERO,
            end: Vec3::new(dx, rise, dz),
            width,
            max_riser,
            landing,
            style: if open { StairStyle::Open } else { StairStyle::Solid },
            ..Stair::default()
        };
        let out = s.build();
        let m = check_output(&out);
        if m.triangle_count() > 0 {
            let open_edges = boundary_edges(&m, 2e-5);
            prop_assert!(open_edges.is_empty(), "stair open edges: {:?}", &open_edges[..open_edges.len().min(6)]);
        }
    }
}

/// Found by the property test: a closed arc-cornered loop left a 0.1 mm
/// open triangle in its face at y = 0.65.
#[test]
fn closed_arc_loop_has_no_sliver_hole_regression() {
    let w = Wall {
        path: WallPath::Points {
            points: vec![
                WallPoint::new(0.0, 0.0).with_bulge(0.116_539_8),
                WallPoint::new(4.825_836_7, 0.0).with_bulge(-0.141_337_25),
                WallPoint::new(1.527_641_8, -4.727_497),
            ],
            closed: true,
        },
        thickness: 0.592_061_34,
        height: 5.172_451_5,
        start: WallEnd::Cap,
        end: WallEnd::Cap,
        openings: vec![],
        plinth: false,
        cornice: false,
        coping: false,
        ao: 1.0,
        ..Wall::default()
    };
    assert_shell_closed_but_foot(&w, 0.0);
}

/// Found by the property test: an open stair with a single riser of
/// nearly `max_riser` reported open edges or degenerate output.
#[test]
fn stair_single_open_riser_regression() {
    let s = Stair {
        start: Vec3::ZERO,
        end: Vec3::new(0.0, 0.287_136_14, -2.551_266),
        width: 0.3,
        max_riser: 0.294_542_16,
        landing: 0.0,
        style: StairStyle::Open,
        ..Stair::default()
    };
    let out = s.build();
    let m = check_output(&out);
    if m.triangle_count() > 0 {
        let open_edges = boundary_edges(&m, 2e-5);
        assert!(
            open_edges.is_empty(),
            "stair open edges: {:?}",
            &open_edges[..open_edges.len().min(6)]
        );
    }
}
