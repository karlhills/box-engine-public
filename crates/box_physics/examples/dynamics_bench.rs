//! Benchmark (M7 dynamics): rigid-body step times.
//!
//! ```text
//! cargo run --release -p box_physics --features dynamics --example dynamics_bench -- \
//!     [--bodies 1000,5000] [--projectiles 200] [--frames 600]
//! ```
//!
//! For each body count: boxes, balls and capsules dropped in piles onto a
//! ground box, simulated for `--frames` fixed steps. Then `--projectiles`
//! small CCD spheres fired at 150 m/s at a ring of thin kinematic walls,
//! relaunched when they stop. Prints per-frame timings (ms) of the dynamics
//! step alone and of the whole tick (step, transform propagation, query
//! world sync), split into the first two seconds (everything moving) and
//! the rest.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use box_core::prelude::*;
use box_physics::*;

const DT: f32 = 1.0 / 60.0;

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn list(name: &str, default: &[usize]) -> Vec<usize> {
    arg(name).map_or_else(
        || default.to_vec(),
        |v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect(),
    )
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

fn stats(name: &str, samples: &[f64]) {
    let mut s = samples.to_vec();
    s.sort_by(f64::total_cmp);
    let n = s.len().max(1);
    let mean = s.iter().sum::<f64>() / n as f64;
    let pct = |p: f64| {
        s.get(((n as f64 - 1.0) * p).round() as usize)
            .copied()
            .unwrap_or(0.0)
    };
    println!(
        "  {name:<28} mean {mean:7.3}  p50 {:7.3}  p95 {:7.3}  max {:7.3}",
        pct(0.5),
        pct(0.95),
        pct(1.0)
    );
}

/// An app whose dynamics step is timed into the returned log.
fn timed_app() -> (App, Rc<RefCell<Vec<f64>>>) {
    let mut app = App::new();
    app.add_plugin(&PhysicsPlugin);
    let started = Rc::new(RefCell::new(Instant::now()));
    let log = Rc::new(RefCell::new(Vec::new()));
    let s = started.clone();
    app.system(Stage::FixedUpdate, move |_: &mut World| {
        *s.borrow_mut() = Instant::now();
    })
    .before("physics_step");
    let l = log.clone();
    app.system(Stage::FixedUpdate, move |_: &mut World| {
        l.borrow_mut()
            .push(started.borrow().elapsed().as_secs_f64() * 1e3);
    })
    .after("physics_step");
    (app, log)
}

fn run(app: &mut App, frames: usize, mut each: impl FnMut(&mut App)) -> Vec<f64> {
    let mut tick_ms = Vec::with_capacity(frames);
    for _ in 0..frames {
        each(app);
        let t = Instant::now();
        app.tick(DT);
        tick_ms.push(t.elapsed().as_secs_f64() * 1e3);
    }
    tick_ms
}

fn report(tick: &[f64], step: &[f64]) {
    let active = 120.min(tick.len());
    stats("step, first 2 s", &step[..active.min(step.len())]);
    stats("tick, first 2 s", &tick[..active]);
    if tick.len() > active {
        stats("step, after", &step[active.min(step.len())..]);
        stats("tick, after", &tick[active..]);
    }
}

fn bodies(count: usize, frames: usize) {
    let (mut app, step) = timed_app();
    let side = ((count as f32 / 6.0).sqrt().ceil() as usize).max(1);
    let extent = side as f32 * 1.6;
    app.world.spawn((
        Transform::from_xyz(0.0, -0.5, 0.0),
        Collider::cuboid(Vec3::new(extent + 10.0, 1.0, extent + 10.0)),
    ));
    let mut rng = Lcg(11);
    for i in 0..count {
        let pile = i / 6;
        let level = i % 6;
        let (gx, gz) = ((pile % side) as f32, (pile / side) as f32);
        let at = Vec3::new(
            (gx - side as f32 * 0.5) * 1.6 + (rng.next() - 0.5) * 0.3,
            0.5 + level as f32 * 1.1 + rng.next() * 0.2,
            (gz - side as f32 * 0.5) * 1.6 + (rng.next() - 0.5) * 0.3,
        );
        let collider = match i % 3 {
            0 => Collider::cuboid(Vec3::splat(0.8)),
            1 => Collider::sphere(0.45),
            _ => Collider::capsule(0.3, 0.25),
        };
        app.world.spawn((
            Transform::from_translation(at).with_rotation(Quat::from_rotation_y(rng.next() * 6.0)),
            collider,
            RigidBody::dynamic(1.0),
        ));
    }
    let tick = run(&mut app, frames, |_| {});
    let stats_ = app.world.resource::<Dynamics>().stats();
    println!(
        "{count} dynamic bodies in {} piles, {frames} frames: {} awake at the end",
        side * side,
        stats_.awake
    );
    report(&tick, &step.borrow());
}

/// Fired from the origin at the wall ring.
struct Projectile {
    dir: Vec3,
    launched: u64,
}

fn projectiles(count: usize, frames: usize) {
    let (mut app, step) = timed_app();
    const RADIUS: f32 = 20.0;
    const SPEED: f32 = 150.0;
    const SHOTS: u32 = 1 << 1;
    // A ring of thin kinematic walls (0.1 m), so only CCD stops the shots.
    for i in 0..24 {
        let a = i as f32 / 24.0 * std::f32::consts::TAU;
        let at = Vec3::new(a.cos() * RADIUS, 2.0, a.sin() * RADIUS);
        app.world.spawn((
            Transform::from_translation(at).with_rotation(Quat::from_rotation_y(-a)),
            Collider::cuboid(Vec3::new(0.1, 4.0, 5.4)),
            RigidBody::kinematic(),
        ));
    }
    let mut rng = Lcg(5);
    let mut shots = Vec::with_capacity(count);
    for i in 0..count {
        let a = rng.next() * std::f32::consts::TAU;
        let dir = Vec3::new(a.cos(), (rng.next() - 0.5) * 0.05, a.sin());
        let mut rb = RigidBody::dynamic(0.05).with_ccd();
        rb.gravity_scale = 0.0;
        // Shots are on layer 1 and ignore each other (they all start at
        // the origin).
        shots.push(app.world.spawn((
            Transform::from_xyz(0.0, 2.0, 0.0),
            Collider::sphere(0.05).with_layers(SHOTS, !SHOTS),
            rb,
            Velocity::linear(dir * SPEED),
            Projectile {
                dir,
                launched: i as u64 % 30,
            },
        )));
    }
    let mut escaped = 0usize;
    let mut launches = 0usize;
    let mut frame = 0u64;
    let tick = run(&mut app, frames, |app| {
        frame += 1;
        for &e in &shots {
            let p = app.world.get::<Transform>(e).unwrap().translation;
            if Vec2::new(p.x, p.z).length() > RADIUS + 1.0 {
                escaped += 1;
            }
            let (dir, launched) = {
                let s = app.world.get::<Projectile>(e).unwrap();
                (s.dir, s.launched)
            };
            // Relaunch half a second after firing (they stop at the wall).
            if frame >= launched + 30 {
                app.world.component_mut::<Transform>(e).unwrap().translation =
                    Vec3::new(0.0, 2.0, 0.0);
                *app.world.component_mut::<Velocity>(e).unwrap() = Velocity::linear(dir * SPEED);
                app.world.component_mut::<Projectile>(e).unwrap().launched = frame;
                launches += 1;
            }
        }
    });
    println!(
        "{count} CCD projectiles at {SPEED} m/s against 24 thin kinematic walls, {frames} frames: \
         {launches} relaunches, {escaped} samples outside the ring (tunnelled)"
    );
    report(&tick, &step.borrow());
}

fn main() {
    let frames = list("--frames", &[600])[0].max(1);
    for count in list("--bodies", &[1000, 5000]) {
        bodies(count, frames);
    }
    for count in list("--projectiles", &[200]) {
        projectiles(count, frames);
    }
}
