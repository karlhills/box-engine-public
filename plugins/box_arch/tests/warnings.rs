//! Every validation path warns and degrades, never panics: opening
//! clearances (corner, overlap, top, outside, closed seam, arch too
//! short), sharp corners and ends, bad junction parameters, impossible
//! stairs, empty footprints and non-finite input.

mod common;

use box_arch::{
    Footprint, Opening, OpeningKind, OpeningTop, Platform, RidgeAxis, Roof, RoofShape, Stair, Wall,
    WallEnd, WallPath, WallPoint,
};
use box_proc::generator::GenOutput;
use common::check_output;
use glam::{Vec2, Vec3};

fn has(out: &GenOutput, needle: &str) -> bool {
    out.warnings.iter().any(|w| w.contains(needle))
}

fn straight(openings: Vec<Opening>) -> Wall {
    Wall {
        openings,
        ..Wall::through(&[Vec2::ZERO, Vec2::new(6.0, 0.0)])
    }
}

/// How many holes the faces have (each opening adds reveal geometry).
fn reveal_triangles(out: &GenOutput) -> u32 {
    out.mesh
        .as_ref()
        .map(|m| {
            m.groups
                .iter()
                .filter(|g| g.material_slot == box_arch::wall::slot::REVEAL)
                .map(|g| g.index_count / 3)
                .sum()
        })
        .unwrap_or(0)
}

#[test]
fn openings_too_close_overlapping_or_too_tall_are_dropped_with_a_warning() {
    let ok = straight(vec![Opening::window(3.0)]).build(None);
    assert!(ok.warnings.is_empty(), "{:?}", ok.warnings);
    let one = reveal_triangles(&ok);
    assert!(one > 0);

    // At a free end the clearance is 0.25 m from the end.
    let near_end = straight(vec![Opening::window(0.5)]).build(None);
    assert!(
        has(&near_end, "too close to the corner or end"),
        "{:?}",
        near_end.warnings
    );
    assert_eq!(reveal_triangles(&near_end), 0);

    // Near a hard corner the corner's reach is added.
    let corner = Wall {
        openings: vec![Opening::window(3.6)],
        ..Wall::through(&[Vec2::ZERO, Vec2::new(4.0, 0.0), Vec2::new(4.0, 4.0)])
    }
    .build(None);
    assert!(
        has(&corner, "too close to the corner"),
        "{:?}",
        corner.warnings
    );

    let overlap = straight(vec![Opening::window(2.5), Opening::window(3.2)]).build(None);
    assert!(
        has(&overlap, "overlaps another opening"),
        "{:?}",
        overlap.warnings
    );
    assert_eq!(reveal_triangles(&overlap), one, "the second one is dropped");

    // Above each other (storeys) is fine.
    let stacked = Wall {
        height: 6.0,
        ..straight(vec![
            Opening::window(3.0),
            Opening {
                sill: 3.5,
                ..Opening::window(3.0)
            },
        ])
    }
    .build(None);
    assert!(stacked.warnings.is_empty(), "{:?}", stacked.warnings);

    let tall = straight(vec![Opening {
        height: 2.0,
        ..Opening::window(3.0)
    }])
    .build(None);
    assert!(has(&tall, "too tall"), "{:?}", tall.warnings);

    let outside = straight(vec![Opening::window(9.0)]).build(None);
    assert!(has(&outside, "outside the wall"), "{:?}", outside.warnings);

    let bad = straight(vec![Opening {
        width: f32::NAN,
        ..Opening::window(3.0)
    }])
    .build(None);
    assert!(has(&bad, "dropped"), "{:?}", bad.warnings);
    check_output(&bad);
}

#[test]
fn an_arch_too_short_for_its_head_falls_back_to_flat() {
    let out = straight(vec![Opening {
        kind: OpeningKind::Window,
        top: OpeningTop::Pointed,
        width: 1.2,
        height: 0.8,
        ..Opening::window(3.0)
    }])
    .build(None);
    assert!(
        has(&out, "too short for its arched head"),
        "{:?}",
        out.warnings
    );
    check_output(&out);
}

#[test]
fn an_opening_across_the_seam_of_a_closed_wall_warns() {
    let w = Wall {
        openings: vec![Opening::window(0.1)],
        ..Wall::closed_loop(&[
            Vec2::new(2.0, 2.0),
            Vec2::new(2.0, -2.0),
            Vec2::new(-2.0, -2.0),
            Vec2::new(-2.0, 2.0),
        ])
    };
    let out = w.build(None);
    assert!(!out.warnings.is_empty());
    check_output(&out);
    // A round tower: a smooth seam, so the seam warning names it.
    let tower = Wall {
        path: WallPath::Points {
            points: vec![
                WallPoint::new(-1.5, 0.0).with_bulge(1.0),
                WallPoint::new(1.5, 0.0).with_bulge(1.0),
            ],
            closed: true,
        },
        openings: vec![Opening::window(0.0)],
        ..Wall::default()
    };
    let out = tower.build(None);
    assert!(has(&out, "start of a closed wall"), "{:?}", out.warnings);
}

#[test]
fn sharp_corners_split_and_bad_ends_fall_back_to_caps() {
    let acute = Wall::through(&[
        Vec2::new(0.0, -3.0),
        Vec2::new(0.0, 2.0),
        Vec2::new(1.0, -3.0),
    ])
    .build(None);
    assert!(
        has(&acute, "sharper than the mitre limit"),
        "{:?}",
        acute.warnings
    );
    check_output(&acute);

    let mitre = Wall {
        end: WallEnd::Mitre {
            neighbour: Vec2::new(-1.0, 0.05),
        },
        ..Wall::default()
    }
    .build(None);
    assert!(
        has(&mitre, "mitre sharper than the mitre limit"),
        "{:?}",
        mitre.warnings
    );
    check_output(&mitre);

    let parallel = Wall {
        end: WallEnd::Butt {
            point: Vec2::new(2.0, 0.0),
            direction: Vec2::X,
            cap: false,
            band_shift: 0.0,
            concave: [true, true],
        },
        ..Wall::default()
    }
    .build(None);
    assert!(has(&parallel, "within 15°"), "{:?}", parallel.warnings);

    let missed = Wall {
        start: WallEnd::Butt {
            point: Vec2::new(30.0, 0.0),
            direction: Vec2::Y,
            cap: true,
            band_shift: 0.0,
            concave: [false, false],
        },
        ..Wall::default()
    }
    .build(None);
    assert!(has(&missed, "does not cross"), "{:?}", missed.warnings);

    let extend = Wall {
        start: WallEnd::Extend {
            distance: f32::INFINITY,
        },
        ..Wall::default()
    }
    .build(None);
    assert!(has(&extend, "extend distance"), "{:?}", extend.warnings);
    check_output(&extend);
}

#[test]
fn degenerate_walls_warn_and_draw_nothing_or_fall_back() {
    let one = Wall::through(&[Vec2::ZERO]).build(None);
    assert!(
        one.mesh.is_none() && has(&one, "at least 2"),
        "{:?}",
        one.warnings
    );
    let same = Wall::through(&[Vec2::ZERO, Vec2::ZERO]).build(None);
    assert!(same.mesh.is_none());
    let nan =
        Wall::through(&[Vec2::NAN, Vec2::new(1.0, 0.0), Vec2::new(2.0, f32::NAN)]).build(None);
    assert!(nan.mesh.is_none());
    let thin = Wall {
        thickness: 0.0,
        height: f32::NAN,
        ..Wall::default()
    }
    .build(None);
    assert!(
        has(&thin, "thickness") && has(&thin, "height"),
        "{:?}",
        thin.warnings
    );
    check_output(&thin);
    // Too thick for a tight curve: said, not hidden.
    let curl = Wall {
        thickness: 2.0,
        path: WallPath::Points {
            points: vec![
                WallPoint::new(-0.5, 0.0).with_bulge(1.0),
                WallPoint::new(0.5, 0.0),
            ],
            closed: false,
        },
        ..Wall::default()
    }
    .build(None);
    assert!(has(&curl, "overlaps itself"), "{:?}", curl.warnings);
    let no_spline = Wall {
        path: WallPath::Spline {
            entity: None,
            tolerance: 0.01,
        },
        ..Wall::default()
    }
    .build(None);
    assert!(
        has(&no_spline, "spline is missing"),
        "{:?}",
        no_spline.warnings
    );
}

#[test]
fn roofs_clamp_and_warn() {
    let hip = Roof {
        shape: RoofShape::Hip {
            size: Vec2::new(2.0, 6.0),
            ridge: RidgeAxis::X,
        },
        ..Roof::default()
    }
    .build();
    assert!(has(&hip, "ridge axis ignored"), "{:?}", hip.warnings);
    check_output(&hip);
    let wild = Roof {
        pitch: f32::NAN,
        overhang: -3.0,
        thickness: 100.0,
        ..Roof::default()
    }
    .build();
    assert!(
        has(&wild, "pitch") && has(&wild, "overhang"),
        "{:?}",
        wild.warnings
    );
    check_output(&wild);
}

#[test]
fn stairs_say_when_the_slope_is_impossible() {
    let steep = Stair {
        end: Vec3::new(0.0, 3.0, -1.0),
        ..Stair::default()
    }
    .build();
    assert!(has(&steep, "too steep"), "{:?}", steep.warnings);
    check_output(&steep);
    let shallow = Stair {
        end: Vec3::new(0.0, 0.4, -6.0),
        ..Stair::default()
    }
    .build();
    assert!(has(&shallow, "very shallow"), "{:?}", shallow.warnings);
    let flat = Stair {
        end: Vec3::new(0.0, 0.0, -3.0),
        ..Stair::default()
    }
    .build();
    assert!(flat.mesh.is_none() && has(&flat, "nothing to climb"));
    let vertical = Stair {
        end: Vec3::new(0.0, 3.0, 0.0),
        ..Stair::default()
    }
    .build();
    assert!(vertical.mesh.is_none() && has(&vertical, "horizontal run"));
    let landing = Stair {
        landing: 10.0,
        ..Stair::default()
    }
    .build();
    assert!(has(&landing, "landing"), "{:?}", landing.warnings);
    let nan = Stair {
        end: Vec3::NAN,
        ..Stair::default()
    }
    .build();
    assert!(nan.mesh.is_none() && !nan.warnings.is_empty());
}

#[test]
fn empty_footprints_warn() {
    for fp in [
        Footprint::Polygon { points: vec![] },
        Footprint::Polygon {
            points: vec![Vec2::ZERO, Vec2::X, Vec2::new(2.0, 0.0)],
        },
        Footprint::Rectangle {
            size: Vec2::new(0.0, 3.0),
        },
        Footprint::Circle {
            radius: f32::NAN,
            segments: 12,
        },
    ] {
        let out = Platform {
            footprint: fp.clone(),
            ..Platform::default()
        }
        .build();
        assert!(out.mesh.is_none() && !out.warnings.is_empty(), "{fp:?}");
    }
    let bow = Platform {
        footprint: Footprint::Polygon {
            points: vec![
                Vec2::ZERO,
                Vec2::new(2.0, 2.0),
                Vec2::new(2.0, 0.0),
                Vec2::new(0.0, 2.0),
            ],
        },
        ..Platform::default()
    }
    .build();
    assert!(has(&bow, "keeping the largest"), "{:?}", bow.warnings);
    check_output(&bow);
}
