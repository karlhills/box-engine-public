//! Golden output hashes of the built-in generators (`Primitive`, `Lathe`,
//! `Extrude`, `TerrainChunk`). Changing one's output without bumping its `VERSION` fails
//! here; fixtures are in `tests/golden/`. Re-record a deliberate change
//! with `BOX_UPDATE_GOLDEN=1 cargo test -p box_proc --test builtin` after
//! bumping the version, and review `cargo test -p box_engine --test golden`.

use std::f32::consts::{FRAC_PI_2, PI};
use std::path::PathBuf;

use box_core::primitive::{Primitive, Shape};
use box_proc::golden::{GoldenCase, assert_golden};
use box_proc::{Extrude, Hole, Lathe, LatheShape, Outline};
use glam::{UVec2, Vec2, Vec3};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

#[test]
fn primitive_golden_hashes() {
    assert_golden::<Primitive>(
        dir(),
        &[
            GoldenCase::new("cube", Primitive::default(), 0),
            GoldenCase::new("box", Primitive::cuboid(Vec3::new(1.0, 2.0, 3.0)), 0),
            GoldenCase::new("sphere", Primitive::sphere(0.5), 0),
            GoldenCase::new("cylinder", Primitive::cylinder(0.5, 1.0), 0),
            GoldenCase::new("capsule", Primitive::capsule(0.4, 0.4), 0),
            GoldenCase::new(
                "plane",
                Primitive::new(Shape::Plane {
                    size: Vec2::new(4.0, 2.0),
                    cells: UVec2::new(4, 2),
                }),
                0,
            ),
            GoldenCase::new(
                "clamped",
                Primitive::new(Shape::Sphere {
                    radius: -1.0,
                    sectors: 0,
                    stacks: 1000,
                }),
                0,
            ),
        ],
    );
}

#[test]
fn lathe_golden_hashes() {
    assert_golden::<Lathe>(
        dir(),
        &[
            GoldenCase::new("vase", Lathe::default(), 0),
            GoldenCase::new(
                "column",
                Lathe {
                    shape: LatheShape::Column {
                        height: 2.0,
                        radius: 0.2,
                    },
                    ..Lathe::default()
                },
                0,
            ),
            GoldenCase::new(
                "dome",
                Lathe {
                    shape: LatheShape::Dome {
                        radius: 0.6,
                        rings: 12,
                    },
                    ..Lathe::default()
                },
                0,
            ),
            GoldenCase::new(
                "half_vase_capped",
                Lathe {
                    sweep_angle: PI * 1.25,
                    start_angle: -FRAC_PI_2,
                    ..Lathe::default()
                },
                0,
            ),
            GoldenCase::new(
                "faceted_column",
                Lathe {
                    shape: LatheShape::Column {
                        height: 3.0,
                        radius: 0.25,
                    },
                    segments: 6,
                    smooth_angle: 0.0,
                    ..Lathe::default()
                },
                0,
            ),
            GoldenCase::new(
                "custom_ring",
                Lathe {
                    shape: LatheShape::Custom {
                        profile: vec![
                            Vec2::new(0.3, 0.0),
                            Vec2::new(0.55, 0.0),
                            Vec2::new(0.55, 0.25),
                            Vec2::new(0.3, 0.25),
                        ],
                        closed: true,
                    },
                    segments: 48,
                    ..Lathe::default()
                },
                0,
            ),
            GoldenCase::new(
                "empty_custom",
                Lathe {
                    shape: LatheShape::Custom {
                        profile: Vec::new(),
                        closed: false,
                    },
                    ..Lathe::default()
                },
                0,
            ),
        ],
    );
}

#[test]
fn extrude_golden_hashes() {
    assert_golden::<Extrude>(
        dir(),
        &[
            GoldenCase::new("slab", Extrude::default(), 0),
            GoldenCase::new(
                "hole_and_bevel",
                Extrude {
                    outline: Outline::Rectangle {
                        size: Vec2::new(1.2, 0.8),
                        corner_radius: 0.1,
                    },
                    hole: Hole::Circle {
                        radius: 0.22,
                        segments: 32,
                        offset: Vec2::ZERO,
                    },
                    depth: 0.25,
                    bevel_width: 0.05,
                    bevel_segments: 4,
                    ..Extrude::default()
                },
                0,
            ),
            GoldenCase::new(
                "star_chamfer",
                Extrude {
                    outline: Outline::Star {
                        points: 5,
                        outer_radius: 0.6,
                        inner_radius: 0.27,
                    },
                    depth: 0.2,
                    bevel_width: 0.03,
                    bevel_segments: 1,
                    ..Extrude::default()
                },
                0,
            ),
            GoldenCase::new(
                "hexagon_frame",
                Extrude {
                    outline: Outline::Polygon {
                        sides: 6,
                        radius: 0.6,
                    },
                    hole: Hole::Inset { width: 0.15 },
                    depth: 0.35,
                    bevel_width: 0.02,
                    ..Extrude::default()
                },
                0,
            ),
            GoldenCase::new(
                "custom_outline_and_holes",
                Extrude {
                    outline: Outline::Custom {
                        points: vec![
                            Vec2::new(-1.0, -0.5),
                            Vec2::new(1.0, -0.5),
                            Vec2::new(1.0, 0.5),
                            Vec2::new(0.0, 0.9),
                            Vec2::new(-1.0, 0.5),
                        ],
                    },
                    hole: Hole::Custom {
                        rings: vec![
                            vec![
                                Vec2::new(-0.7, -0.2),
                                Vec2::new(-0.3, -0.2),
                                Vec2::new(-0.3, 0.2),
                                Vec2::new(-0.7, 0.2),
                            ],
                            vec![
                                Vec2::new(0.3, -0.2),
                                Vec2::new(0.7, -0.2),
                                Vec2::new(0.5, 0.2),
                            ],
                        ],
                    },
                    depth: 0.5,
                    bevel_width: 0.0,
                    ..Extrude::default()
                },
                0,
            ),
            GoldenCase::new(
                "hole_outside",
                Extrude {
                    hole: Hole::Circle {
                        radius: 0.1,
                        segments: 8,
                        offset: Vec2::new(5.0, 0.0),
                    },
                    ..Extrude::default()
                },
                0,
            ),
        ],
    );
}

#[test]
fn terrain_chunk_golden_hashes() {
    use box_proc::terrain::{TerrainChunk, TerrainStamp};
    use glam::Vec2;
    let stamps = vec![
        TerrainStamp::Noise {
            seed: 4,
            scale: 12.0,
            amplitude: 0.7,
            octaves: 3,
        },
        TerrainStamp::Hill {
            center: Vec2::new(10.0, 6.0),
            radius: 14.0,
            height: 4.0,
        },
        TerrainStamp::Ridge {
            points: vec![Vec2::new(-20.0, 20.0), Vec2::new(20.0, 28.0)],
            width: 12.0,
            height: 3.0,
        },
        TerrainStamp::Basin {
            center: Vec2::new(-8.0, -6.0),
            radius: 6.0,
            depth: 1.2,
        },
        TerrainStamp::Flatten {
            center: Vec2::new(0.0, 14.0),
            radius: 3.0,
            height: 1.5,
            bank: 3.0,
        },
        TerrainStamp::FlattenPath {
            points: vec![Vec3::new(-16.0, 0.2, -16.0), Vec3::new(16.0, 1.2, 16.0)],
            width: 2.5,
            bank: 1.5,
        },
    ];
    assert_golden::<TerrainChunk>(
        &dir(),
        &[
            GoldenCase::new(
                "every_stamp",
                TerrainChunk {
                    origin: Vec2::new(-16.0, -16.0),
                    stamps: stamps.clone(),
                    ..TerrainChunk::default()
                },
                0,
            ),
            GoldenCase::new(
                "corner_with_skirt_walkability",
                TerrainChunk {
                    origin: Vec2::new(-32.0, -32.0),
                    cells_x: 16,
                    cells_z: 24,
                    cell_size: 2.0,
                    stamps,
                    skirt: 2.0,
                    skirt_edges: 1 | 4,
                    show_walkability: true,
                    walk_slope: 20.0,
                    ..TerrainChunk::default()
                },
                0,
            ),
        ],
    );
}
