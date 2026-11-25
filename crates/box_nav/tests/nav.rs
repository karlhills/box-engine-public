//! box_nav on small fixtures: what is walkable (slopes, steps, headroom,
//! narrow gaps), paths around walls and across tile edges, partial paths,
//! line walkability, random points, and determinism.

use box_nav::{NavConfig, NavMesh, PathStatus, TileCoord, bake_tile};
use glam::{Vec2, Vec3};

/// The 12 triangles of an axis-aligned box.
fn cuboid(min: Vec3, max: Vec3) -> Vec<[Vec3; 3]> {
    let p = |x: bool, y: bool, z: bool| {
        Vec3::new(
            if x { max.x } else { min.x },
            if y { max.y } else { min.y },
            if z { max.z } else { min.z },
        )
    };
    let quads = [
        // top (+y), facing up
        [
            p(false, true, false),
            p(false, true, true),
            p(true, true, true),
            p(true, true, false),
        ],
        // bottom
        [
            p(false, false, false),
            p(true, false, false),
            p(true, false, true),
            p(false, false, true),
        ],
        // -x
        [
            p(false, false, false),
            p(false, false, true),
            p(false, true, true),
            p(false, true, false),
        ],
        // +x
        [
            p(true, false, false),
            p(true, true, false),
            p(true, true, true),
            p(true, false, true),
        ],
        // -z
        [
            p(false, false, false),
            p(false, true, false),
            p(true, true, false),
            p(true, false, false),
        ],
        // +z
        [
            p(false, false, true),
            p(true, false, true),
            p(true, true, true),
            p(false, true, true),
        ],
    ];
    quads
        .iter()
        .flat_map(|q| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]])
        .collect()
}

/// An upward-facing quad (a ramp) from `a0`–`a1` to `b1`–`b0`.
fn quad(a0: Vec3, a1: Vec3, b1: Vec3, b0: Vec3) -> Vec<[Vec3; 3]> {
    vec![[a0, b1, b0], [a0, a1, b1]]
}

fn floor(half: f32) -> Vec<[Vec3; 3]> {
    cuboid(Vec3::new(-half, -0.5, -half), Vec3::new(half, 0.0, half))
}

fn bake_all(tris: &[[Vec3; 3]], cfg: NavConfig) -> NavMesh {
    let lo = tris
        .iter()
        .flatten()
        .copied()
        .fold(Vec3::splat(f32::INFINITY), Vec3::min);
    let hi = tris
        .iter()
        .flatten()
        .copied()
        .fold(Vec3::splat(f32::NEG_INFINITY), Vec3::max);
    let mut mesh = NavMesh::new(cfg);
    for t in cfg.tiles_in(lo, hi) {
        if let Some(tile) = bake_tile(tris, t, &cfg).unwrap() {
            mesh.insert_tile(tile);
        }
    }
    mesh
}

fn cfg() -> NavConfig {
    NavConfig::default()
}

#[test]
fn flat_floor_straight_path() {
    let mesh = bake_all(&floor(10.0), cfg());
    assert!(mesh.poly_count() > 0);
    let path = mesh.find_path(Vec3::new(-5.0, 0.9, -5.0), Vec3::new(5.0, 0.9, 5.0));
    assert_eq!(path.status, PathStatus::Complete);
    assert_eq!(path.points.len(), 2, "{:?}", path.points);
    assert!(
        (path.length() - 200f32.sqrt()).abs() < 0.2,
        "{}",
        path.length()
    );
    for p in &path.points {
        assert!(p.y.abs() < 0.2, "on the floor: {p}");
    }
    // Erosion: nothing within the agent radius of the floor's edge.
    let edge = mesh.closest_point(Vec3::new(9.95, 0.0, 0.0)).unwrap();
    assert!(edge.x < 10.0 - 0.3, "{edge}");
}

#[test]
fn paths_go_through_the_gap_in_a_wall() {
    let mut tris = floor(10.0);
    // A wall across z = 0 with a 2 m gap at x = 0.
    tris.extend(cuboid(
        Vec3::new(-10.0, 0.0, -0.2),
        Vec3::new(-1.0, 3.0, 0.2),
    ));
    tris.extend(cuboid(Vec3::new(1.0, 0.0, -0.2), Vec3::new(10.0, 3.0, 0.2)));
    let mesh = bake_all(&tris, cfg());
    let a = Vec3::new(-5.0, 0.0, -5.0);
    let b = Vec3::new(-5.0, 0.0, 5.0);
    let path = mesh.find_path(a, b);
    assert_eq!(path.status, PathStatus::Complete);
    assert!(path.points.len() >= 3, "{:?}", path.points);
    // Crosses z = 0 through the gap.
    for w in path.points.windows(2) {
        if (w[0].z < 0.0) != (w[1].z < 0.0) {
            let t = -w[0].z / (w[1].z - w[0].z);
            let x = w[0].x + (w[1].x - w[0].x) * t;
            assert!(
                x.abs() < 1.0,
                "crosses the wall at x = {x}: {:?}",
                path.points
            );
        }
    }
    // Shortest through the gap: corners at the gap's near edge.
    let ideal = Vec2::new(-5.0, -5.0).distance(Vec2::new(-0.6, 0.0)) * 2.0;
    assert!(path.length() < ideal + 0.6, "{} vs {ideal}", path.length());
    assert!(!mesh.is_walkable_line(a, b));
    assert!(mesh.is_walkable_line(Vec3::new(0.0, 0.0, -5.0), Vec3::new(0.0, 0.0, 5.0)));
}

#[test]
fn a_closed_wall_gives_a_partial_path() {
    let mut tris = floor(10.0);
    tris.extend(cuboid(
        Vec3::new(-10.0, 0.0, -0.2),
        Vec3::new(10.0, 3.0, 0.2),
    ));
    let mesh = bake_all(&tris, cfg());
    let path = mesh.find_path(Vec3::new(-5.0, 0.0, -5.0), Vec3::new(-5.0, 0.0, 5.0));
    assert_eq!(path.status, PathStatus::Partial);
    let last = *path.points.last().unwrap();
    assert!(last.z < 0.0 && last.z > -1.5, "as close as it gets: {last}");
    // Off the mesh entirely.
    let none = mesh.find_path(Vec3::new(50.0, 0.0, 50.0), Vec3::ZERO);
    assert_eq!(none.status, PathStatus::NoStart);
}

/// A 3 m high platform reached by a ramp of `run` metres.
fn ramp_level(run: f32) -> Vec<[Vec3; 3]> {
    let mut tris = floor(12.0);
    tris.extend(cuboid(Vec3::new(4.0, 0.0, -3.0), Vec3::new(10.0, 3.0, 3.0)));
    tris.extend(quad(
        Vec3::new(4.0 - run, 0.0, -1.5),
        Vec3::new(4.0 - run, 0.0, 1.5),
        Vec3::new(4.0, 3.0, 1.5),
        Vec3::new(4.0, 3.0, -1.5),
    ));
    tris
}

#[test]
fn ramps_up_to_the_slope_limit() {
    let top = Vec3::new(7.0, 3.0, 0.0);
    // 30°: run = 3 / tan 30° ≈ 5.2 m.
    let mesh = bake_all(&ramp_level(5.2), cfg());
    let path = mesh.find_path(Vec3::new(-8.0, 0.0, 0.0), top);
    assert_eq!(path.status, PathStatus::Complete, "{:?}", path.points);
    assert!((path.points.last().unwrap().y - 3.0).abs() < 0.3);
    // 60°: run ≈ 1.7 m; too steep.
    let mesh = bake_all(&ramp_level(1.7), cfg());
    let path = mesh.find_path(Vec3::new(-8.0, 0.0, 0.0), top);
    assert_eq!(path.status, PathStatus::Partial);
}

/// Stairs of `rise` per step up to about 2 m, 0.5 m deep each.
fn stairs(rise: f32) -> Vec<[Vec3; 3]> {
    let mut tris = floor(12.0);
    let n = (2.0 / rise).round() as usize;
    for i in 0..n {
        let x0 = i as f32 * 0.5;
        tris.extend(cuboid(
            Vec3::new(x0, 0.0, -1.5),
            Vec3::new(x0 + 0.5, (i + 1) as f32 * rise, 1.5),
        ));
    }
    let x_end = n as f32 * 0.5;
    tris.extend(cuboid(
        Vec3::new(x_end, 0.0, -3.0),
        Vec3::new(x_end + 5.0, n as f32 * rise, 3.0),
    ));
    tris
}

#[test]
fn stairs_up_to_the_step_height() {
    let mesh = bake_all(&stairs(0.25), cfg());
    let path = mesh.find_path(Vec3::new(-6.0, 0.0, 0.0), Vec3::new(6.5, 2.0, 0.0));
    assert_eq!(path.status, PathStatus::Complete, "{:?}", path.points);
    let mesh = bake_all(&stairs(0.5), cfg());
    let path = mesh.find_path(Vec3::new(-6.0, 0.0, 0.0), Vec3::new(6.5, 2.0, 0.0));
    assert_eq!(path.status, PathStatus::Partial);
}

#[test]
fn low_ceilings_and_narrow_gaps_block() {
    // Two rooms joined by a 3 m long tunnel through a wall at z = 0.
    let tunnel = |height: f32, width: f32| {
        let mut tris = floor(10.0);
        let w = width * 0.5;
        tris.extend(cuboid(Vec3::new(-10.0, 0.0, -1.5), Vec3::new(-w, 4.0, 1.5)));
        tris.extend(cuboid(Vec3::new(w, 0.0, -1.5), Vec3::new(10.0, 4.0, 1.5)));
        tris.extend(cuboid(Vec3::new(-w, height, -1.5), Vec3::new(w, 4.0, 1.5)));
        bake_all(&tris, cfg())
    };
    let (a, b) = (Vec3::new(0.0, 0.0, -6.0), Vec3::new(0.0, 0.0, 6.0));
    assert!(
        tunnel(2.5, 2.0).find_path(a, b).is_complete(),
        "open tunnel"
    );
    assert_eq!(
        tunnel(1.2, 2.0).find_path(a, b).status,
        PathStatus::Partial,
        "low"
    );
    assert_eq!(
        tunnel(2.5, 0.6).find_path(a, b).status,
        PathStatus::Partial,
        "narrow"
    );
}

#[test]
fn paths_cross_tile_edges() {
    let tris = cuboid(Vec3::new(-40.0, -0.5, -6.0), Vec3::new(40.0, 0.0, 6.0));
    let mesh = bake_all(&tris, cfg());
    let tiles: Vec<TileCoord> = mesh.tiles().map(|t| t.coord).collect();
    assert!(tiles.len() >= 3, "{tiles:?}");
    let path = mesh.find_path(Vec3::new(-35.0, 0.0, 0.0), Vec3::new(35.0, 0.0, 0.0));
    assert_eq!(path.status, PathStatus::Complete, "{:?}", path.points);
    assert!((path.length() - 70.0).abs() < 0.5, "{}", path.length());
    // Removing the tiles across x = 0 cuts it.
    let mut cut = mesh.clone();
    let middle: Vec<TileCoord> = tiles.iter().copied().filter(|t| t.x == 0).collect();
    for t in &middle {
        cut.remove_tile(*t);
    }
    let path = cut.find_path(Vec3::new(-35.0, 0.0, 0.0), Vec3::new(35.0, 0.0, 0.0));
    assert_eq!(path.status, PathStatus::Partial);
    // Rebuilt tiles link again.
    for t in middle {
        let cfg = *cut.config();
        cut.insert_tile(bake_tile(&tris, t, &cfg).unwrap().unwrap());
    }
    assert!(
        cut.find_path(Vec3::new(-35.0, 0.0, 0.0), Vec3::new(35.0, 0.0, 0.0))
            .is_complete()
    );
}

#[test]
fn random_points_and_determinism() {
    let mut tris = floor(10.0);
    tris.extend(cuboid(
        Vec3::new(-10.0, 0.0, -0.2),
        Vec3::new(-1.0, 3.0, 0.2),
    ));
    let a = bake_all(&tris, cfg());
    let b = bake_all(&tris, cfg());
    assert_eq!(
        a.tiles().collect::<Vec<_>>(),
        b.tiles().collect::<Vec<_>>(),
        "same input, same mesh"
    );
    let (s, e) = (Vec3::new(-5.0, 0.0, -5.0), Vec3::new(-5.0, 0.0, 5.0));
    assert_eq!(a.find_path(s, e), b.find_path(s, e));
    for seed in 0..20 {
        let p = a
            .random_point_near(Vec3::new(3.0, 0.0, 3.0), 2.0, seed)
            .unwrap();
        assert!(
            Vec2::new(p.x - 3.0, p.z - 3.0).length() <= 2.0 + 1e-3,
            "{p}"
        );
        assert!(p.y.abs() < 0.2);
        assert_eq!(
            Some(p),
            a.random_point_near(Vec3::new(3.0, 0.0, 3.0), 2.0, seed)
        );
    }
}

#[test]
fn bad_settings_are_errors() {
    let bad = NavConfig {
        cell_size: 0.0,
        ..cfg()
    };
    let err = bake_tile(&floor(4.0), TileCoord::new(0, 0), &bad).unwrap_err();
    assert!(err.to_string().contains("cell_size"), "{err}");
    assert!(
        bake_tile(&[], TileCoord::new(0, 0), &cfg())
            .unwrap()
            .is_none()
    );
}
