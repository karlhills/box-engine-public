//! Golden output hashes of `WallGen` and `FenceGen` (the generator golden
//! harness, `box_proc::golden`): changing either generator's output
//! without bumping its `VERSION` fails here. Fixtures in
//! `tests/golden/generators/`; re-record with `BOX_UPDATE_GOLDEN=1` after
//! a version bump (and look at the captures of `tests/captures.rs`).

use std::path::PathBuf;

use box_engine::box_proc::golden::{GoldenCase, assert_golden};
use box_engine::prelude::*;
use walls::{FenceGen, WallEnd, WallGen};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/generators")
}

fn pts(p: &[[f32; 2]]) -> Vec<Vec3> {
    p.iter().map(|q| Vec3::new(q[0], 0.0, q[1])).collect()
}

#[test]
fn wall_gen_golden_hashes() {
    assert_golden::<WallGen>(
        dir(),
        &[
            GoldenCase::new("free", WallGen::default(), 0),
            GoldenCase::new(
                "mitre_end",
                WallGen {
                    points: pts(&[[0.0, 0.0], [4.0, 0.0]]),
                    end: WallEnd::Mitre {
                        neighbour_dir: Vec3::new(0.0, 0.0, -3.0),
                    },
                    ..WallGen::default()
                },
                0,
            ),
            GoldenCase::new(
                "butt_owner_and_cut",
                WallGen {
                    points: pts(&[[0.0, 0.0], [4.0, 0.0]]),
                    start: WallEnd::Butt {
                        point: Vec3::new(0.0, 0.0, 0.1),
                        normal: Vec3::Z,
                        cap: false,
                    },
                    end: WallEnd::Butt {
                        point: Vec3::new(4.1, 0.0, 0.0),
                        normal: Vec3::X,
                        cap: true,
                    },
                    ..WallGen::default()
                },
                0,
            ),
            GoldenCase::new(
                "corners_and_bevel_fill",
                WallGen {
                    points: pts(&[[0.0, 0.0], [0.0, -4.0], [4.0, -4.0], [4.0, -1.0]]),
                    height: 3.0,
                    thickness: 0.35,
                    start: WallEnd::Open,
                    end: WallEnd::Cap,
                    fills: pts(&[[4.0, -1.0], [4.175, -1.0], [4.1, -0.9]]),
                },
                0,
            ),
        ],
    );
}

#[test]
fn fence_gen_golden_hashes() {
    assert_golden::<FenceGen>(
        dir(),
        &[
            GoldenCase::new("default", FenceGen::default(), 0),
            GoldenCase::new(
                "corner_shared_post",
                FenceGen {
                    points: pts(&[[0.0, 0.0], [5.0, 0.0], [5.0, -3.0]]),
                    rail_count: 3,
                    start_post: false,
                    ..FenceGen::default()
                },
                0,
            ),
            GoldenCase::new(
                "clamped",
                FenceGen {
                    rail_count: 40,
                    post_spacing: 0.05,
                    ..FenceGen::default()
                },
                0,
            ),
        ],
    );
}
