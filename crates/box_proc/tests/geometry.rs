//! Golden geometry tests for the building blocks: fixed parameter sets with
//! exact counts and bounds, area and volume within tolerance, closed
//! manifolds where expected and outward normals. A change to any block's
//! output fails here; update the numbers only after looking at the capture
//! (`cargo run -p box_proc --example proc_gallery -- --capture ...`).

use box_core::mesh::MeshData;
use box_core::pmath;
use box_proc::extrude::{ExtrudeParams, extrude};
use box_proc::lathe::{LatheParams, lathe};
use box_proc::mesh_builder::{
    BuiltMesh, MeshBuilder, edge_report, inward_triangles, signed_volume, surface_area,
};
use box_proc::poly::{self, Polygon};
use box_proc::spline::{ControlPoint, Orientation, Path, Sampling, Spline, SplineMode};
use box_proc::sweep::{JointEnd, JointPlane, Profile, SweepParams, sweep};
use glam::{Affine3A, Vec2, Vec3};
use proptest::prelude::*;

/// Counts, bounds (to 1e-4) and closedness of a mesh.
#[derive(Debug, PartialEq)]
struct Summary {
    vertices: usize,
    triangles: usize,
    groups: Vec<(u32, u32)>,
    min: [i64; 3],
    max: [i64; 3],
    closed: bool,
}

fn q(v: Vec3) -> [i64; 3] {
    v.to_array().map(|x| (x * 1e4).round() as i64)
}

fn summary(m: &MeshData) -> Summary {
    m.validate().expect("valid mesh");
    let b = m.bounds();
    Summary {
        vertices: m.vertex_count(),
        triangles: m.triangle_count(),
        groups: m
            .groups
            .iter()
            .map(|g| (g.material_slot, g.index_count / 3))
            .collect(),
        min: q(b.min),
        max: q(b.max),
        closed: edge_report(m, 1e-5).is_closed_manifold(),
    }
}

fn check(b: &BuiltMesh, want: Summary, area: f32, volume: Option<f32>) {
    let m = &b.mesh;
    let s = summary(m);
    assert_eq!(s, want, "summary");
    assert_eq!(inward_triangles(m), 0, "inward triangles");
    let a = surface_area(m);
    assert!((a - area).abs() <= area * 1e-4 + 1e-4, "area {a} vs {area}");
    if let Some(v) = volume {
        let got = signed_volume(m);
        assert!(
            (got - v).abs() <= v.abs() * 1e-4 + 1e-4,
            "volume {got} vs {v}"
        );
    }
}

fn s(
    v: usize,
    t: usize,
    groups: &[(u32, u32)],
    min: [f32; 3],
    max: [f32; 3],
    closed: bool,
) -> Summary {
    Summary {
        vertices: v,
        triangles: t,
        groups: groups.to_vec(),
        min: q(Vec3::from(min)),
        max: q(Vec3::from(max)),
        closed,
    }
}

// ------------------------------------------------------------------ fixtures

fn extrude_with_hole() -> BuiltMesh {
    let outer = poly::round_corners(
        &Polygon::rect(Vec2::new(-1.5, -1.0), Vec2::new(1.5, 1.0)).outer,
        0.3,
        4,
    );
    let hole = Polygon::circle(Vec2::ZERO, 0.5, 24).outer;
    extrude(
        &Polygon::new(outer).with_hole(hole),
        &ExtrudeParams {
            depth: 1.5,
            ..Default::default()
        },
    )
}

fn extrude_chamfer() -> BuiltMesh {
    extrude(
        &Polygon::circle(Vec2::ZERO, 1.0, 6),
        &ExtrudeParams {
            depth: 0.8,
            bevel_width: 0.1,
            bevel_segments: 1,
            bevel_slot: 2,
            ..Default::default()
        },
    )
}

fn extrude_round_bevel() -> BuiltMesh {
    extrude(
        &Polygon::rect(Vec2::splat(-1.0), Vec2::splat(1.0)),
        &ExtrudeParams {
            depth: 1.0,
            bevel_width: 0.2,
            bevel_segments: 4,
            ..Default::default()
        },
    )
}

fn vase_profile() -> Vec<Vec2> {
    vec![
        Vec2::new(0.0, 0.0),
        Vec2::new(0.5, 0.0),
        Vec2::new(0.62, 0.35),
        Vec2::new(0.58, 0.7),
        Vec2::new(0.35, 1.05),
        Vec2::new(0.3, 1.3),
        Vec2::new(0.42, 1.5),
    ]
}

fn lathe_vase() -> BuiltMesh {
    lathe(&LatheParams {
        profile: Profile::new(vase_profile(), false),
        segments: 32,
        ..Default::default()
    })
}

fn lathe_partial() -> BuiltMesh {
    lathe(&LatheParams {
        profile: Profile::new(
            vec![
                Vec2::new(1.0, 0.0),
                Vec2::new(1.5, 0.0),
                Vec2::new(1.5, 2.0),
                Vec2::new(1.0, 2.0),
            ],
            true,
        ),
        segments: 24,
        start_angle: 0.0,
        sweep_angle: 270f32.to_radians(),
        caps: true,
        cap_slot: 1,
    })
}

fn lathe_dome() -> BuiltMesh {
    let mut pts = vec![Vec2::new(0.0, 0.0)];
    pts.extend((0..=8).map(|i| Vec2::from_angle(std::f32::consts::FRAC_PI_2 * i as f32 / 8.0)));
    lathe(&LatheParams {
        profile: Profile::new(pts, false),
        segments: 32,
        ..Default::default()
    })
}

fn tube_spline() -> Spline {
    Spline::from_positions(
        &[
            Vec3::new(-2.0, 0.5, 0.0),
            Vec3::new(0.0, 1.2, -1.5),
            Vec3::new(2.0, 0.5, 0.0),
            Vec3::new(0.0, 0.3, 1.5),
        ],
        SplineMode::CatmullRom,
        true,
    )
}

fn sweep_tube() -> BuiltMesh {
    let path = Path::from_spline(
        &tube_spline(),
        Sampling::Uniform { step: 0.25 },
        Orientation::RotationMinimizing(Vec3::Y),
    );
    sweep(
        &path,
        &SweepParams {
            profile: Profile::circle(0.15, 12),
            ..Default::default()
        },
    )
}

fn wall(points: &[Vec3], closed: bool, start: JointEnd, end: JointEnd) -> BuiltMesh {
    sweep(
        &Path::from_polyline(points, closed, Orientation::Up(Vec3::Y)),
        &SweepParams {
            profile: Profile::rect(0.3, 2.5).with_slots(vec![0, 0, 1, 0]),
            start,
            end,
            ..Default::default()
        },
    )
}

fn wall_loop() -> BuiltMesh {
    wall(
        &[
            Vec3::ZERO,
            Vec3::new(6.0, 0.0, 0.0),
            Vec3::new(6.0, 0.0, -4.0),
            Vec3::new(0.0, 0.0, -4.0),
        ],
        true,
        JointEnd::Cap,
        JointEnd::Cap,
    )
}

/// Two separate walls meeting at (4, 0, 0), mitred against each other.
fn wall_mitre_pair() -> (BuiltMesh, BuiltMesh) {
    // Slightly off-axis so nothing is exact by accident.
    let pa = [Vec3::new(0.1, 0.0, 0.3), Vec3::new(4.0, 0.0, 0.0)];
    let pb = [Vec3::new(4.0, 0.0, 0.0), Vec3::new(4.7, 0.0, -3.0)];
    // Raw neighbour segment vectors, as the neighbours' own paths compute them.
    let a = wall(
        &pa,
        false,
        JointEnd::Cap,
        JointEnd::Mitre {
            neighbour_dir: pb[1] - pb[0],
        },
    );
    let b = wall(
        &pb,
        false,
        JointEnd::Mitre {
            neighbour_dir: pa[1] - pa[0],
        },
        JointEnd::Cap,
    );
    (a, b)
}

/// The owner runs along x at z = -2 (faces at z = -2 ± 0.15); the
/// subordinate comes from +z and is cut on the owner's near face, uncapped.
fn wall_butt() -> (BuiltMesh, BuiltMesh) {
    let owner = wall(
        &[Vec3::new(0.0, 0.0, -2.0), Vec3::new(6.0, 0.0, -2.0)],
        false,
        JointEnd::Cap,
        JointEnd::Cap,
    );
    let face = JointPlane::through_line(Vec3::new(0.0, 0.0, -1.85), Vec3::X, Vec3::Y);
    let sub = wall(
        &[Vec3::new(3.0, 0.0, 2.0), Vec3::new(3.0, 0.0, -2.0)],
        false,
        JointEnd::Cap,
        JointEnd::Butt {
            plane: face,
            cap: false,
        },
    );
    (owner, sub)
}

fn merged(a: &MeshData, b: &MeshData) -> MeshData {
    let mut mb = MeshBuilder::new();
    mb.append(a, Affine3A::IDENTITY);
    mb.append(b, Affine3A::IDENTITY);
    mb.finish()
}

// --------------------------------------------------------------------- tests

#[test]
fn golden_extrude_with_hole() {
    let b = extrude_with_hole();
    assert!(b.warnings.is_empty(), "{:?}", b.warnings);
    // Rounded rectangle (4 corners × 5 points) smooth along the arcs, hole smooth.
    let area_2d = Polygon::new(poly::round_corners(
        &Polygon::rect(Vec2::new(-1.5, -1.0), Vec2::new(1.5, 1.0)).outer,
        0.3,
        4,
    ))
    .area()
        - Polygon::circle(Vec2::ZERO, 0.5, 24).area();
    let v = area_2d * 1.5;
    check(
        &b,
        s(
            180,
            176,
            &[(0, 88), (1, 88)],
            [-1.5, 0.0, -1.0],
            [1.5, 1.5, 1.0],
            true,
        ),
        29.186_396,
        Some(v),
    );
}

#[test]
fn golden_extrude_chamfer() {
    let b = extrude_chamfer();
    assert!(b.warnings.is_empty(), "{:?}", b.warnings);
    // ∫ area(inset) dh: a regular hexagon inset by d has area A - 6d + 6 tan(30°) d².
    let (a, w) = (1.5 * 3f32.sqrt(), 0.1f32);
    let bevel = a * w - 6.0 * w * w / 2.0 + 6.0 * pmath::tan(30f32.to_radians()) * w.powi(3) / 3.0;
    let v = a * 0.6 + 2.0 * bevel;
    check(
        &b,
        s(
            84,
            44,
            &[(0, 12), (1, 8), (2, 24)],
            [-1.0, 0.0, -0.866_025_4],
            [1.0, 0.8, 0.866_025_4],
            true,
        ),
        9.264_515,
        Some(v),
    );
}

#[test]
fn golden_extrude_round_bevel() {
    let b = extrude_round_bevel();
    assert!(b.warnings.is_empty(), "{:?}", b.warnings);
    check(
        &b,
        s(
            152,
            76,
            &[(0, 72), (1, 4)],
            [-1.0, 0.0, -1.0],
            [1.0, 1.0, 1.0],
            true,
        ),
        14.543_15,
        Some(3.856_910_5),
    );
}

#[test]
fn golden_lathe_vase() {
    let b = lathe_vase();
    assert!(b.warnings.is_empty(), "{:?}", b.warnings);
    check(
        &b,
        s(
            296,
            352,
            &[(0, 352)],
            [-0.62, 0.0, -0.62],
            [0.62, 1.5, 0.62],
            false,
        ),
        5.670_962,
        None,
    );
    // Open at the rim: exactly one boundary loop of 32 edges.
    assert_eq!(edge_report(&b.mesh, 1e-5).boundary, 32);
}

#[test]
fn golden_lathe_partial() {
    let b = lathe_partial();
    assert!(b.warnings.is_empty(), "{:?}", b.warnings);
    let n = 24.0f32;
    let step = 270f32.to_radians() / n;
    // Exact faceted volume: per segment, a wedge of the ring section.
    let v = n * 0.5 * pmath::sin(step) * (1.5f32.powi(2) - 1.0) * 2.0;
    check(
        &b,
        s(
            208,
            196,
            &[(0, 192), (1, 4)],
            [-1.5, 0.0, -1.5],
            [1.5, 2.0, 1.5],
            true,
        ),
        31.376_822,
        Some(v),
    );
}

#[test]
fn golden_lathe_dome() {
    let b = lathe_dome();
    assert!(b.warnings.is_empty(), "{:?}", b.warnings);
    check(
        &b,
        s(
            361,
            512,
            &[(0, 512)],
            [-1.0, 0.0, -1.0],
            [1.0, 1.0, 1.0],
            true,
        ),
        9.354_298,
        Some(2.060_970_8),
    );
}

#[test]
fn golden_sweep_tube() {
    let b = sweep_tube();
    assert!(b.warnings.is_empty(), "{:?}", b.warnings);
    let len = tube_spline().curve().length();
    // A closed tube: volume ≈ polygon area × centreline length.
    let poly_area = 6.0 * pmath::sin(std::f32::consts::TAU / 12.0) * 0.15 * 0.15;
    let v = signed_volume(&b.mesh);
    assert!(
        (v - poly_area * len).abs() < poly_area * len * 0.01,
        "{v} vs {}",
        poly_area * len
    );
    check(
        &b,
        s(
            572,
            1032,
            &[(0, 1032)],
            [-2.15, 0.1527, -1.6487],
            [2.1469, 1.3488, 1.6384],
            true,
        ),
        9.937_28,
        None,
    );
}

#[test]
fn golden_wall_loop() {
    let b = wall_loop();
    assert!(b.warnings.is_empty(), "{:?}", b.warnings);
    // Mitred loop, centred profile: volume = section area × centreline length.
    check(
        &b,
        s(
            64,
            32,
            &[(0, 24), (1, 8)],
            [-0.15, 0.0, -4.15],
            [6.15, 2.5, 0.15],
            true,
        ),
        112.0,
        Some(0.3 * 2.5 * 20.0),
    );
}

#[test]
fn mitre_pair_shares_the_joint_exactly() {
    let (a, b) = wall_mitre_pair();
    assert!(a.warnings.is_empty() && b.warnings.is_empty());
    // Each wall alone is open at the joint (4 boundary edges of the rect).
    assert_eq!(edge_report(&a.mesh, 1e-6).boundary, 4);
    // Together they are one closed solid.
    let m = merged(&a.mesh, &b.mesh);
    let r = edge_report(&m, 1e-6);
    assert!(r.is_closed_manifold(), "{r:?}");
    // The joint vertices are bit-identical: every vertex of A's open end is
    // a vertex of B.
    let joint: Vec<Vec3> = a
        .mesh
        .positions
        .iter()
        .copied()
        .filter(|p| p.x > 3.5)
        .collect();
    assert!(joint.len() >= 4);
    for p in &joint {
        assert!(b.mesh.positions.contains(p), "{p} not shared exactly");
    }
}

#[test]
fn butt_end_lies_on_the_owner_face() {
    let (owner, sub) = wall_butt();
    assert!(owner.warnings.is_empty() && sub.warnings.is_empty());
    assert!(edge_report(&owner.mesh, 1e-6).is_closed_manifold());
    assert_eq!(edge_report(&sub.mesh, 1e-6).boundary, 4);
    let near: Vec<&Vec3> = sub.mesh.positions.iter().filter(|p| p.z < 0.0).collect();
    assert!(!near.is_empty() && near.iter().all(|p| (p.z + 1.85).abs() < 1e-6));
    let bb = sub.mesh.bounds();
    assert!((bb.min.z + 1.85).abs() < 1e-6 && (bb.max.z - 2.0).abs() < 1e-6);
}

#[test]
fn sharp_corner_is_bevelled_and_closed() {
    // A 160° turn (20° interior angle) exceeds the default mitre limit of 4.
    let a = 160f32.to_radians();
    let b = wall(
        &[
            Vec3::ZERO,
            Vec3::new(4.0, 0.0, 0.0),
            Vec3::new(
                4.0 - 4.0 * pmath::cos(std::f32::consts::PI - a),
                0.0,
                -4.0 * pmath::sin(std::f32::consts::PI - a),
            ),
        ],
        false,
        JointEnd::Cap,
        JointEnd::Cap,
    );
    assert!(
        b.warnings.iter().any(|w| w.contains("bevelled")),
        "{:?}",
        b.warnings
    );
    assert!(edge_report(&b.mesh, 1e-5).is_closed_manifold());
    // The bevel stays near the corner: nothing reaches past x = 4.2.
    assert!(b.mesh.bounds().max.x < 4.2, "{}", b.mesh.bounds().max.x);
}

#[test]
fn extend_and_open_ends() {
    let b = wall(
        &[Vec3::ZERO, Vec3::new(4.0, 0.0, 0.0)],
        false,
        JointEnd::Extend {
            distance: 0.5,
            cap: true,
        },
        JointEnd::Open,
    );
    let bb = b.mesh.bounds();
    assert!((bb.min.x + 0.5).abs() < 1e-6 && (bb.max.x - 4.0).abs() < 1e-6);
    assert_eq!(edge_report(&b.mesh, 1e-6).boundary, 4);
}

#[test]
fn roll_turns_the_profile() {
    // A flat ribbon rolled 90° at both points stands on its edge.
    let pts = [
        ControlPoint::new(Vec3::ZERO).with_roll(std::f32::consts::FRAC_PI_2),
        ControlPoint::new(Vec3::new(0.0, 0.0, -3.0)).with_roll(std::f32::consts::FRAC_PI_2),
    ];
    let path = Path::from_control_points(&pts, false, Orientation::Up(Vec3::Y));
    let b = sweep(
        &path,
        &SweepParams {
            profile: Profile::new(vec![Vec2::new(-1.0, 0.0), Vec2::new(1.0, 0.0)], false),
            ..Default::default()
        },
    );
    let bb = b.mesh.bounds();
    assert!(
        bb.max.x.abs() < 1e-5 && (bb.max.y - 1.0).abs() < 1e-5 && (bb.min.y + 1.0).abs() < 1e-5,
        "{bb:?}"
    );
}

#[test]
fn scale_per_point_tapers() {
    let pts = [
        ControlPoint::new(Vec3::ZERO),
        ControlPoint::new(Vec3::new(0.0, 0.0, -3.0)).with_scale(Vec2::new(0.5, 2.0)),
    ];
    let path = Path::from_control_points(&pts, false, Orientation::Up(Vec3::Y));
    let b = sweep(
        &path,
        &SweepParams {
            profile: Profile::rect(1.0, 1.0),
            ..Default::default()
        },
    );
    let bb = b.mesh.bounds();
    assert!((bb.max.y - 2.0).abs() < 1e-5 && (bb.max.x - 0.5).abs() < 1e-5);
    assert!(edge_report(&b.mesh, 1e-6).is_closed_manifold());
}

#[test]
fn generation_is_deterministic() {
    assert_eq!(sweep_tube(), sweep_tube());
    assert_eq!(lathe_vase(), lathe_vase());
    assert_eq!(extrude_with_hole(), extrude_with_hole());
}

#[test]
fn uvs_are_arc_length_along_walls() {
    let b = wall(
        &[Vec3::ZERO, Vec3::new(4.0, 0.0, 0.0)],
        false,
        JointEnd::Open,
        JointEnd::Open,
    );
    let m = &b.mesh;
    for (p, uv) in m.positions.iter().zip(&m.uvs[0]) {
        assert!((uv.x - p.x).abs() < 1e-5, "U is arc length: {p} {uv}");
    }
}

fn any_f32() -> impl Strategy<Value = f32> {
    prop_oneof![8 => -5.0f32..5.0, 1 => Just(f32::NAN), 1 => Just(f32::INFINITY), 1 => Just(0.0f32)]
}

fn any_v2() -> impl Strategy<Value = Vec2> {
    (any_f32(), any_f32()).prop_map(|(x, y)| Vec2::new(x, y))
}

fn any_v3() -> impl Strategy<Value = Vec3> {
    (any_f32(), any_f32(), any_f32()).prop_map(|(x, y, z)| Vec3::new(x, y, z))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(150))]

    /// Random (often degenerate) inputs never panic and always give a valid mesh.
    #[test]
    fn blocks_never_panic(
        prof in prop::collection::vec(any_v2(), 0..8),
        path_pts in prop::collection::vec(any_v3(), 0..6),
        closed in any::<bool>(),
        k in any_f32(),
        mode in 0usize..3,
        end in 0usize..5,
    ) {
        let profile = Profile::new(prof.clone(), closed);
        let je = [
            JointEnd::Cap,
            JointEnd::Open,
            JointEnd::Extend { distance: k, cap: true },
            JointEnd::Mitre { neighbour_dir: Vec3::new(k, 0.0, 1.0) },
            JointEnd::Butt { plane: JointPlane::new(Vec3::splat(k), Vec3::new(1.0, k, 0.0)), cap: true },
        ][end];
        let modes = [SplineMode::CatmullRom, SplineMode::Bezier, SplineMode::Polyline];
        let spline = Spline::from_positions(&path_pts, modes[mode], closed);
        for o in [Orientation::Up(Vec3::Y), Orientation::RotationMinimizing(Vec3::new(k, 1.0, 0.0))] {
            let path = Path::from_spline(&spline, Sampling::Adaptive { tolerance: 0.05, max_step: 1.0 }, o);
            let b = sweep(&path, &SweepParams { profile: profile.clone(), start: je, end: je, miter_limit: k, cap_slot: 1, uv_scale: Vec2::splat(k) });
            prop_assert!(b.mesh.validate().is_ok());
            prop_assert!(b.mesh.positions.iter().all(|p| p.is_finite()));
        }
        let l = lathe(&LatheParams { profile: profile.clone(), segments: 12, start_angle: k, sweep_angle: k * 2.0, caps: true, cap_slot: 1 });
        prop_assert!(l.mesh.validate().is_ok());
        let e = extrude(&Polygon::new(prof), &ExtrudeParams { depth: k, bevel_width: k.abs() * 0.1, bevel_segments: 3, ..Default::default() });
        prop_assert!(e.mesh.validate().is_ok());
        prop_assert!(e.mesh.positions.iter().all(|p| p.is_finite()));
    }
}
