//! Benchmark (ROADMAP M3): N moving entities with colliders, each running an
//! overlap query every frame, plus a few triggers and ray casts.
//!
//! ```text
//! cargo run --release -p box_physics --example overlap_bench -- [--entities 5000] [--frames 600]
//! ```
//!
//! Prints per-frame timings (ms): the whole frame (move system, transform
//! propagation and physics sync), the physics sync alone, and the queries.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use box_core::prelude::*;
use box_physics::*;

struct Mover {
    center: Vec3,
    radius: f32,
    speed: f32,
    phase: f32,
}

fn arg(name: &str, default: usize) -> usize {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// A small deterministic generator, so runs are comparable.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
}

fn stats(name: &str, samples: &mut [f64]) {
    samples.sort_by(f64::total_cmp);
    let n = samples.len().max(1);
    let mean = samples.iter().sum::<f64>() / n as f64;
    let pct = |p: f64| samples[((n as f64 - 1.0) * p).round() as usize];
    println!(
        "{name:<22} mean {mean:7.3}  p50 {:7.3}  p95 {:7.3}  max {:7.3}",
        pct(0.5),
        pct(0.95),
        pct(1.0)
    );
}

fn main() {
    let entities = arg("--entities", 5000);
    let frames = arg("--frames", 600).max(1);
    let side = (entities as f32).sqrt() * 2.0; // ~4 m² per entity

    let mut app = App::new();
    app.world.insert_resource(Physics::new());
    app.add_event::<TriggerEvent>();
    let sync_ms = Rc::new(RefCell::new(Vec::with_capacity(frames)));
    let sync_log = sync_ms.clone();
    app.add_system(Stage::PostUpdate, move |w| {
        let t = Instant::now();
        sync_physics(w);
        sync_log.borrow_mut().push(t.elapsed().as_secs_f64() * 1e3);
    });
    app.add_system(Stage::Update, |w| {
        let t = w.resource::<Time>().elapsed;
        for (m, tr) in w.query_mut::<(&Mover, &mut Transform)>() {
            let a = m.phase + t * m.speed;
            tr.translation = m.center + Vec3::new(a.cos(), 0.0, a.sin()) * m.radius;
            tr.rotation = Quat::from_rotation_y(a);
        }
    });

    let mut rng = Lcg(7);
    let mut movers = Vec::with_capacity(entities);
    for i in 0..entities {
        let center = Vec3::new(
            (rng.next() - 0.5) * side,
            rng.next() * 2.0,
            (rng.next() - 0.5) * side,
        );
        let collider = match i % 4 {
            0 => Collider::cuboid(Vec3::splat(0.6)),
            1 => Collider::sphere(0.35),
            2 => Collider::capsule(0.25, 0.3),
            _ => Collider::cylinder(0.3, 0.6),
        };
        let e = app.world.spawn((
            Transform::from_translation(center),
            collider,
            Mover {
                center,
                radius: 0.5 + rng.next() * 2.0,
                speed: 0.5 + rng.next(),
                phase: rng.next() * std::f32::consts::TAU,
            },
        ));
        movers.push(e);
    }
    // Static triggers scattered over the area.
    for _ in 0..50 {
        let p = Vec3::new((rng.next() - 0.5) * side, 1.0, (rng.next() - 0.5) * side);
        app.world.spawn((
            Transform::from_translation(p),
            Collider::cuboid(Vec3::splat(3.0)).trigger(),
        ));
    }

    let mut frame_ms = Vec::with_capacity(frames);
    let mut query_ms = Vec::with_capacity(frames);
    let mut cursor = EventCursor::<TriggerEvent>::default();
    let (mut overlaps, mut ray_hits, mut trigger_events) = (0usize, 0usize, 0usize);
    let probe = QueryShape::sphere(0.75);
    for frame in 0..=frames {
        let t = Instant::now();
        app.tick(1.0 / 60.0);
        let frame_time = t.elapsed().as_secs_f64() * 1e3;
        trigger_events += cursor
            .read(app.world.resource::<Events<TriggerEvent>>())
            .count();

        let t = Instant::now();
        let physics = app.world.resource::<Physics>();
        for &e in &movers {
            let Some(p) = app.world.get::<GlobalTransform>(e).map(|g| g.translation()) else {
                continue;
            };
            physics.overlap_each(&probe, p, &QueryFilter::new().excluding(e), |_| {
                overlaps += 1;
                true
            });
        }
        for i in 0..100 {
            let a = i as f32 * 0.0628;
            let ray = Ray::new(Vec3::new(0.0, 1.0, 0.0), Vec3::new(a.cos(), 0.0, a.sin()));
            ray_hits += usize::from(physics.raycast(&ray, side, &QueryFilter::new()).is_some());
        }
        let query_time = t.elapsed().as_secs_f64() * 1e3;
        // Frame 0 inserts everything; measure steady state.
        if frame > 0 {
            frame_ms.push(frame_time);
            query_ms.push(query_time);
        }
    }
    let mut sync = sync_ms.borrow()[1..].to_vec();
    let last = app.world.resource::<Physics>().last_sync();
    println!(
        "{entities} moving colliders + 50 triggers, {frames} frames (box_physics overlap_bench)"
    );
    println!("timings in ms per frame:");
    stats("frame (tick)", &mut frame_ms);
    stats("  of which sync", &mut sync);
    stats(&format!("{entities} overlaps + 100 rays"), &mut query_ms);
    println!(
        "last sync: {} checked, {} updated, {} shapes built; {:.1} overlaps/query, {} ray hits, {} trigger events",
        last.checked,
        last.updated,
        last.shapes_built,
        overlaps as f64 / (entities * frames.max(1)) as f64,
        ray_hits,
        trigger_events
    );
}
