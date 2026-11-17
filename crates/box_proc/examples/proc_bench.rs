//! Timings for the geometry building blocks at typical sizes (CPU only).
//! Recorded in docs/measurements/m5-geometry.md.
//!
//! `cargo run --release -p box_proc --example proc_bench`

use box_core::pmath;
use std::hint::black_box;
use std::time::Instant;

use box_proc::extrude::{ExtrudeParams, extrude};
use box_proc::lathe::{LatheParams, lathe};
use box_proc::poly::{self, Join, Polygon};
use box_proc::spline::{Orientation, Path, Sampling, Spline, SplineMode};
use box_proc::sweep::{Profile, SweepParams, sweep};
use glam::{Vec2, Vec3};

/// Median wall time of `runs` calls, in microseconds, and the last result's size.
fn time<T>(runs: usize, mut f: impl FnMut() -> T, size: impl Fn(&T) -> String) -> (f64, String) {
    let mut out = None;
    for _ in 0..3 {
        out = Some(black_box(f()));
    }
    let mut v: Vec<f64> = (0..runs)
        .map(|_| {
            let t = Instant::now();
            out = Some(black_box(f()));
            t.elapsed().as_secs_f64() * 1e6
        })
        .collect();
    v.sort_by(f64::total_cmp);
    (v[v.len() / 2], out.as_ref().map(size).unwrap_or_default())
}

/// A wavy star polygon: `n` vertices, simple.
fn star(n: usize, r: f32, phase: f32) -> Vec<Vec2> {
    (0..n)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / n as f32;
            let rr = r * (1.0 + 0.25 * pmath::sin(7.0 * a + phase));
            Vec2::from_angle(a) * rr
        })
        .collect()
}

fn main() {
    let mut rows: Vec<(&str, f64, String)> = Vec::new();
    let tris = |b: &box_proc::mesh_builder::BuiltMesh| {
        format!(
            "{} verts, {} tris",
            b.mesh.vertex_count(),
            b.mesh.triangle_count()
        )
    };

    // Lathe: 32-point smooth profile × 64 segments.
    let profile: Vec<Vec2> = (0..32)
        .map(|i| {
            let t = i as f32 / 31.0;
            Vec2::new(0.4 + 0.3 * pmath::sin(t * 5.0), t * 2.0)
        })
        .collect();
    let lp = LatheParams {
        profile: Profile::new(profile, false),
        segments: 64,
        ..Default::default()
    };
    let (us, n) = time(300, || lathe(&lp), tris);
    rows.push(("lathe, 32-point profile × 64 segments", us, n));

    // Sweep: 20-point Catmull-Rom spline, 16-point circle, adaptive sampling.
    let pts: Vec<Vec3> = (0..20)
        .map(|i| {
            let a = i as f32 * 0.5;
            Vec3::new(
                pmath::cos(a) * (4.0 + i as f32 * 0.2),
                pmath::sin(a * 0.7),
                -pmath::sin(a) * 4.0,
            )
        })
        .collect();
    let spline = Spline::from_positions(&pts, SplineMode::CatmullRom, false);
    let sp = SweepParams {
        profile: Profile::circle(0.2, 16),
        ..Default::default()
    };
    let (us, n) = time(
        300,
        || {
            let path = Path::from_spline(
                &spline,
                Sampling::Adaptive {
                    tolerance: 0.01,
                    max_step: 0.5,
                },
                Orientation::RotationMinimizing(Vec3::Y),
            );
            (path.stations.len(), sweep(&path, &sp))
        },
        |(s, b)| format!("{s} stations, {}", tris(b)),
    );
    rows.push((
        "sweep, 20-point spline × 16-point profile (path + mesh)",
        us,
        n,
    ));

    // Wall: 20-point polyline, rectangle profile, mitred corners.
    let wall_pts: Vec<Vec3> = (0..20)
        .map(|i| Vec3::new(i as f32 * 2.0, 0.0, if i % 2 == 0 { 0.0 } else { -1.5 }))
        .collect();
    let wp = SweepParams {
        profile: Profile::rect(0.3, 3.0),
        ..Default::default()
    };
    let (us, n) = time(
        1000,
        || {
            sweep(
                &Path::from_polyline(&wall_pts, false, Orientation::Up(Vec3::Y)),
                &wp,
            )
        },
        tris,
    );
    rows.push(("wall sweep, 20-point polyline × rectangle", us, n));

    // Booleans of two 200-vertex polygons.
    let a = vec![Polygon::new(star(200, 3.0, 0.0))];
    let b = vec![Polygon::new(star(200, 3.0, 1.3)).translated(Vec2::new(1.0, 0.5))];
    let count = |m: &poly::MultiPolygon| {
        format!(
            "{} polygons, {} verts",
            m.len(),
            m.iter().map(|p| p.vertex_count()).sum::<usize>()
        )
    };
    for (name, op) in [
        ("boolean union, 2 × 200 vertices", poly::BoolOp::Union),
        (
            "boolean difference, 2 × 200 vertices",
            poly::BoolOp::Difference,
        ),
        (
            "boolean intersection, 2 × 200 vertices",
            poly::BoolOp::Intersection,
        ),
    ] {
        let (us, n) = time(300, || poly::boolean(&a, &b, op), count);
        rows.push((name, us, n));
    }
    let (us, n) = time(
        300,
        || poly::offset(&a, 0.2, Join::Round { tolerance: 0.01 }),
        count,
    );
    rows.push(("offset +0.2 m round, 200 vertices", us, n));
    let (us, n) = time(
        300,
        || poly::offset(&a, -0.2, Join::Miter { limit: 4.0 }),
        count,
    );
    rows.push(("offset -0.2 m mitre, 200 vertices", us, n));

    // Triangulation: 1,000 vertices (800 outer + 4 holes × 50).
    let mut p = Polygon::new(star(800, 10.0, 0.0));
    for (x, y) in [(-3.0, -3.0), (3.0, -3.0), (3.0, 3.0), (-3.0, 3.0)] {
        p.holes
            .push(Polygon::circle(Vec2::new(x, y), 1.0, 50).outer);
    }
    let (us, n) = time(
        300,
        || poly::triangulate(&p),
        |t| format!("{} triangles", t.triangle_count()),
    );
    rows.push(("triangulate, 1,000 vertices with 4 holes", us, n));

    // Extrude a 200-vertex polygon with a hole and a rounded bevel.
    let ep = ExtrudeParams {
        depth: 1.0,
        bevel_width: 0.05,
        bevel_segments: 3,
        ..Default::default()
    };
    let holed =
        Polygon::new(star(200, 3.0, 0.0)).with_hole(Polygon::circle(Vec2::ZERO, 1.0, 48).outer);
    let (us, n) = time(300, || extrude(&holed, &ep), tris);
    rows.push(("extrude, 248 vertices, 3-step round bevel", us, n));

    println!("| Block | Median µs | Output |");
    println!("|---|---:|---|");
    for (name, us, n) in rows {
        println!("| {name} | {us:.1} | {n} |");
    }
}
