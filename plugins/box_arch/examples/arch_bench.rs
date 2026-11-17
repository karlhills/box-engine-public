//! Generation timings of the box_arch generators at typical and large
//! parameters, and anchor resolution cost. Prints a Markdown table.
//!
//! `cargo run --release -p box_arch --example arch_bench`

use box_core::pmath;
use std::time::Instant;

use box_arch::{
    Anchor, AnchorSurface, ArchPlugin, Footprint, Opening, Platform, PlatformEdge, RidgeAxis, Roof,
    RoofShape, Stair, StairStyle, Wall, WallPath, WallPoint, resolve_anchors,
};
use box_engine::box_proc::generator::{GenContext, GenOutput, Generator};
use box_engine::prelude::*;

/// Median microseconds of `runs` generations, and the output's size.
fn time<G: Generator>(g: &G, runs: usize) -> (f64, usize, usize, Vec<String>) {
    let ctx = GenContext::new(0);
    let mut out: GenOutput = g.generate(&ctx);
    let mut samples: Vec<f64> = (0..runs)
        .map(|_| {
            let t = Instant::now();
            out = std::hint::black_box(g.generate(&ctx));
            t.elapsed().as_secs_f64() * 1e6
        })
        .collect();
    samples.sort_by(f64::total_cmp);
    let m = out.mesh.unwrap_or_default();
    (
        samples[samples.len() / 2],
        m.vertex_count(),
        m.triangle_count(),
        out.warnings,
    )
}

fn row<G: Generator>(name: &str, g: &G) {
    let (us, v, t, w) = time(g, 300);
    let note = if w.is_empty() {
        String::new()
    } else {
        format!(" ({} warning(s))", w.len())
    };
    println!("| {name} | {v} | {t} | {us:.1}{note} |");
}

fn house_loop(hx: f32, hz: f32) -> Vec<Vec2> {
    vec![
        Vec2::new(hx, hz),
        Vec2::new(hx, -hz),
        Vec2::new(-hx, -hz),
        Vec2::new(-hx, hz),
    ]
}

fn main() {
    println!("| generator | vertices | triangles | median µs |");
    println!("|---|---:|---:|---:|");
    row(
        "Wall: straight 6 m, door + window",
        &Wall {
            openings: vec![Opening::door(2.0), Opening::window(4.2)],
            ..Wall::through(&[Vec2::ZERO, Vec2::new(6.0, 0.0)])
        },
    );
    let house_openings = vec![
        Opening::window(1.5),
        Opening::window(4.5),
        Opening::arched_window(8.0),
        Opening::window(12.0),
        Opening::window(17.0),
        Opening::door(22.0),
        Opening::window(24.5),
        Opening::arched_window(26.5),
    ];
    row(
        "Wall: house loop 8 × 6 m, 8 openings, plinth, cornice",
        &Wall {
            openings: house_openings.clone(),
            plinth: true,
            cornice: true,
            ..Wall::closed_loop(&house_loop(4.0, 3.0))
        },
    );
    let hip = Roof {
        shape: RoofShape::Hip {
            size: Vec2::new(8.3, 6.3),
            ridge: RidgeAxis::Longer,
        },
        ..Roof::default()
    };
    row(
        "Wall: same loop under a hip roof (top follows the roof)",
        &Wall {
            openings: house_openings,
            plinth: true,
            cornice: true,
            roof: Some(
                hip.underside()
                    .transformed(&Affine3A::from_translation(Vec3::new(0.0, 3.0, 0.0))),
            ),
            ..Wall::closed_loop(&house_loop(4.0, 3.0))
        },
    );
    row(
        "Wall (large): 40 m S curve (arcs), 20 openings, plinth, cornice",
        &Wall {
            path: WallPath::Points {
                points: vec![
                    WallPoint::new(0.0, 0.0).with_bulge(0.4),
                    WallPoint::new(14.0, 0.0).with_bulge(-0.4),
                    WallPoint::new(28.0, 0.0),
                ],
                closed: false,
            },
            openings: (0..20)
                .map(|k| Opening::window(1.2 + k as f32 * 1.6))
                .collect(),
            plinth: true,
            cornice: true,
            ..Wall::default()
        },
    );
    row("Roof: gable 5 × 4 m", &Roof::default());
    row("Roof: hip 8.3 × 6.3 m", &hip);
    row(
        "Roof: cone r 1.35 m, 48 segments",
        &Roof {
            shape: RoofShape::Cone {
                radius: 1.35,
                segments: 48,
            },
            ..Roof::default()
        },
    );
    row(
        "Roof (large): dome r 6 m, 128 × 32",
        &Roof {
            shape: RoofShape::Dome {
                radius: 6.0,
                rise: 4.0,
                segments: 128,
                rings: 32,
            },
            ..Roof::default()
        },
    );
    row("Platform: 4 × 3 m", &Platform::default());
    row(
        "Platform: circle 64 segments with a lip",
        &Platform {
            footprint: Footprint::Circle {
                radius: 3.0,
                segments: 64,
            },
            edge: PlatformEdge::Lip {
                height: 0.55,
                width: 0.22,
            },
            ..Platform::default()
        },
    );
    row(
        "Platform (large): 200-point outline with a lip",
        &Platform {
            footprint: Footprint::Polygon {
                points: (0..200)
                    .map(|k| {
                        let a = k as f32 / 200.0 * std::f32::consts::TAU;
                        let r = 8.0 + pmath::sin(a * 5.0) * 1.5;
                        Vec2::new(pmath::cos(a), pmath::sin(a)) * r
                    })
                    .collect(),
            },
            edge: PlatformEdge::Lip {
                height: 0.55,
                width: 0.22,
            },
            ..Platform::default()
        },
    );
    row("Stair: 6 risers, solid", &Stair::default());
    row(
        "Stair (large): 30 risers, open, landing",
        &Stair {
            end: Vec3::new(0.0, 6.0, -10.0),
            landing: 1.2,
            style: StairStyle::Open,
            ..Stair::default()
        },
    );

    // Anchors: 1,000 decorations on 100 walls.
    let game = Game::new().with_plugin(ArchPlugin);
    let mut app = game.new_app();
    let mut walls = Vec::new();
    for i in 0..100 {
        let uid = Uid::new();
        let e = app
            .world
            .spawn_saved(
                uid,
                "wall",
                (
                    Transform::from_xyz((i % 10) as f32 * 8.0, 0.0, (i / 10) as f32 * 8.0),
                    Wall {
                        openings: vec![Opening::door(2.0)],
                        ..Wall::through(&[Vec2::ZERO, Vec2::new(6.0, 0.0)])
                    },
                ),
            )
            .unwrap();
        walls.push(e);
        for k in 0..10 {
            app.world.spawn((
                Transform::default(),
                Anchor::new(
                    uid,
                    AnchorSurface::WallTop,
                    Vec3::new(0.3 + k as f32 * 0.55, 0.0, 0.0),
                ),
            ));
        }
    }
    let t = Instant::now();
    let moved = resolve_anchors(&mut app.world);
    let first = t.elapsed().as_secs_f64() * 1e3;
    let mut idle = Vec::new();
    for _ in 0..100 {
        let t = Instant::now();
        std::hint::black_box(resolve_anchors(&mut app.world));
        idle.push(t.elapsed().as_secs_f64() * 1e3);
    }
    idle.sort_by(f64::total_cmp);
    app.world.component_mut::<Wall>(walls[0]).unwrap().height = 3.5;
    let t = Instant::now();
    let edit_moved = resolve_anchors(&mut app.world);
    let edit = t.elapsed().as_secs_f64() * 1e3;
    println!();
    println!(
        "Anchors, 1,000 on 100 walls: first resolve {first:.2} ms ({moved} moved), idle {:.3} ms median, one wall edited {edit:.3} ms ({edit_moved} moved)",
        idle[idle.len() / 2]
    );
}
