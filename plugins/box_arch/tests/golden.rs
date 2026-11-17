//! Golden output hashes of the box_arch generators. Changing a
//! generator's output without bumping its `VERSION` fails here; fixtures
//! are in `tests/golden/`. Re-record a deliberate change with
//! `BOX_UPDATE_GOLDEN=1 cargo test -p box_arch --test golden` after
//! bumping the version, and look at the gallery capture
//! (`cargo run -p box_arch --example arch_gallery -- --capture ...`).

use std::path::PathBuf;

use box_arch::junction::{self, Arm};
use box_arch::{
    Footprint, Opening, OpeningTop, Platform, PlatformEdge, RidgeAxis, Roof, RoofEnd, RoofShape,
    Stair, StairStyle, Wall, WallEnd, WallPath, WallPoint,
};
use box_core::identity::Uid;
use box_proc::golden::{GoldenCase, assert_golden};
use box_proc::spline::{Spline, SplineMode};
use glam::{Affine3A, Vec2, Vec3};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn v(p: &[(f32, f32)]) -> Vec<Vec2> {
    p.iter().map(|&(x, z)| Vec2::new(x, z)).collect()
}

#[test]
fn wall_golden_hashes() {
    let gable = Roof {
        shape: RoofShape::Gable {
            size: Vec2::new(6.0, 4.0),
            ridge: RidgeAxis::X,
        },
        ..Roof::default()
    };
    let loop_pts = v(&[(2.85, 1.85), (2.85, -1.85), (-2.85, -1.85), (-2.85, 1.85)]);
    let owner = Arm::end(Vec2::new(-4.0, 0.0), Vec2::ZERO, 0.4);
    let sub = Arm::start(Vec2::ZERO, Vec2::new(0.0, 3.0), 0.25);
    let (owner_end, sub_start) = junction::butt(&owner, &sub);
    let spline_uid = Uid::from_u128(0xa7c4_0000_0000_4000_8000_0000_0000_0001);
    let spline = Spline::from_positions(
        &[
            Vec3::new(-3.0, 0.0, 0.0),
            Vec3::new(-1.0, 0.0, -1.5),
            Vec3::new(1.0, 0.0, -1.5),
            Vec3::new(3.0, 0.0, 0.0),
        ],
        SplineMode::CatmullRom,
        false,
    );
    assert_golden::<Wall>(
        dir(),
        &[
            GoldenCase::new("default", Wall::default(), 0),
            GoldenCase::new(
                "l_mitred_openings",
                Wall {
                    openings: vec![
                        Opening::door(1.5),
                        Opening::window(3.2),
                        Opening::arched_window(5.5),
                    ],
                    plinth: true,
                    ..Wall::through(&v(&[(-4.0, 0.0), (0.0, 0.0), (0.0, 4.0)]))
                },
                0,
            ),
            GoldenCase::new(
                "loop_under_gable",
                Wall {
                    roof: Some(
                        gable
                            .underside()
                            .transformed(&Affine3A::from_translation(Vec3::new(0.0, 3.0, 0.0))),
                    ),
                    openings: vec![Opening::door(15.0), Opening::window(2.0)],
                    plinth: true,
                    cornice: true,
                    ..Wall::closed_loop(&loop_pts)
                },
                0,
            ),
            GoldenCase::new(
                "arc_pointed",
                Wall {
                    path: WallPath::Points {
                        points: vec![
                            WallPoint::new(-3.0, 0.0).with_bulge(0.4),
                            WallPoint::new(3.0, 0.0),
                        ],
                        closed: false,
                    },
                    openings: vec![
                        Opening::window(2.0).with_top(OpeningTop::Pointed),
                        Opening::archway(4.5),
                    ],
                    ..Wall::default()
                },
                0,
            ),
            GoldenCase::new(
                "butt_owner",
                Wall {
                    thickness: 0.4,
                    end: owner_end,
                    plinth: true,
                    ..Wall::through(&v(&[(-4.0, 0.0), (0.0, 0.0)]))
                },
                0,
            ),
            GoldenCase::new(
                "butt_subordinate_and_tee",
                Wall {
                    thickness: 0.25,
                    height: 2.0,
                    start: sub_start,
                    end: junction::tee(
                        &Arm::end(Vec2::ZERO, Vec2::new(0.0, 3.0), 0.25),
                        Vec2::X,
                        0.3,
                    ),
                    plinth: true,
                    coping: true,
                    ..Wall::through(&v(&[(0.0, 0.0), (0.0, 3.0)]))
                },
                0,
            ),
            GoldenCase::new(
                "extend_abut_ao_off",
                Wall {
                    start: WallEnd::Extend { distance: 0.5 },
                    end: WallEnd::Abut,
                    ao: 0.0,
                    ..Wall::default()
                },
                0,
            ),
            GoldenCase::new(
                "acute_split",
                Wall::through(&v(&[(0.0, -3.0), (0.0, 2.0), (1.6, -3.0)])),
                0,
            ),
            GoldenCase::new(
                "spline",
                Wall {
                    path: WallPath::Spline {
                        entity: None,
                        tolerance: 0.01,
                    },
                    openings: vec![Opening::window(3.0)],
                    ..Wall::default()
                },
                0,
            )
            .with_owner(spline_uid)
            .with_input(spline_uid, spline),
        ],
    );
}

#[test]
fn roof_golden_hashes() {
    let size = Vec2::new(5.0, 4.0);
    assert_golden::<Roof>(
        dir(),
        &[
            GoldenCase::new("gable", Roof::default(), 0),
            GoldenCase::new(
                "hip",
                Roof {
                    shape: RoofShape::Hip {
                        size,
                        ridge: RidgeAxis::Longer,
                    },
                    ..Roof::default()
                },
                0,
            ),
            GoldenCase::new(
                "hip_abut",
                Roof {
                    shape: RoofShape::Hip {
                        size,
                        ridge: RidgeAxis::Longer,
                    },
                    start: RoofEnd::Abut,
                    ..Roof::default()
                },
                0,
            ),
            GoldenCase::new(
                "gable_abut_bare",
                Roof {
                    end: RoofEnd::Abut,
                    boards: false,
                    ridge_rolls: false,
                    ..Roof::default()
                },
                0,
            ),
            GoldenCase::new(
                "shed",
                Roof {
                    shape: RoofShape::Shed {
                        size,
                        ridge: RidgeAxis::X,
                        high_abuts: false,
                    },
                    ..Roof::default()
                },
                0,
            ),
            GoldenCase::new(
                "lean_to",
                Roof {
                    shape: RoofShape::Shed {
                        size,
                        ridge: RidgeAxis::Z,
                        high_abuts: true,
                    },
                    pitch: 0.3,
                    ..Roof::default()
                },
                0,
            ),
            GoldenCase::new(
                "cone",
                Roof {
                    shape: RoofShape::Cone {
                        radius: 1.5,
                        segments: 24,
                    },
                    pitch: 0.9,
                    ..Roof::default()
                },
                0,
            ),
            GoldenCase::new(
                "dome",
                Roof {
                    shape: RoofShape::Dome {
                        radius: 2.0,
                        rise: 1.5,
                        segments: 24,
                        rings: 8,
                    },
                    ..Roof::default()
                },
                0,
            ),
        ],
    );
}

#[test]
fn platform_golden_hashes() {
    assert_golden::<Platform>(
        dir(),
        &[
            GoldenCase::new("default", Platform::default(), 0),
            GoldenCase::new(
                "round_lip",
                Platform {
                    footprint: Footprint::Circle {
                        radius: 1.2,
                        segments: 24,
                    },
                    edge: PlatformEdge::Lip {
                        height: 0.55,
                        width: 0.22,
                    },
                    ..Platform::default()
                },
                0,
            ),
            GoldenCase::new(
                "l_shape_trim",
                Platform {
                    footprint: Footprint::Polygon {
                        points: v(&[
                            (0.0, 0.0),
                            (4.0, 0.0),
                            (4.0, 2.0),
                            (2.0, 2.0),
                            (2.0, 4.0),
                            (0.0, 4.0),
                        ]),
                    },
                    edge: PlatformEdge::Trim {
                        height: 0.12,
                        proud: 0.04,
                    },
                    ..Platform::default()
                },
                0,
            ),
        ],
    );
}

#[test]
fn stair_golden_hashes() {
    assert_golden::<Stair>(
        dir(),
        &[
            GoldenCase::new("default", Stair::default(), 0),
            GoldenCase::new(
                "open_landing_descending",
                Stair {
                    start: Vec3::new(1.0, 2.0, -3.0),
                    end: Vec3::new(1.0, 0.0, 1.0),
                    landing: 1.0,
                    style: StairStyle::Open,
                    ..Stair::default()
                },
                0,
            ),
            GoldenCase::new(
                "too_steep",
                Stair {
                    end: Vec3::new(0.0, 2.0, -1.0),
                    ..Stair::default()
                },
                0,
            ),
        ],
    );
}
