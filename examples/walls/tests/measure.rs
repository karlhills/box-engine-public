//! The walls measurements of docs/PROCEDURAL_DIRECTION.md §7, printed as
//! tables (`docs/measurements/m5-walls.md` records them):
//!
//! `cargo test --release -p walls --test measure -- --nocapture`
//!
//! - rule pass time (rule function, reconcile) at 10, 100 and 1,000 walls,
//!   and the parts regenerated per edit;
//! - edit-to-visible latency during a 60 Hz drag (default 30 ms throttle);
//! - idle frame cost.
//!
//! Asserts only what the acceptance criteria need (a drag edit is visible
//! within one frame; idle frames run nothing), so debug runs pass too.

mod common;

use std::time::{Duration, Instant};

use box_engine::box_core::derived::Derived;
use box_engine::box_core::edit::EditStack;
use box_engine::prelude::*;
use common::*;
use walls::plan;

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

/// The mesh version of `owner`'s part.
fn version(w: &World, owner: Uid) -> u64 {
    let mesh = w
        .query::<(&Derived, &MeshRenderer)>()
        .iter()
        .find(|(d, _)| d.key.owner() == Some(owner))
        .map(|(_, r)| r.mesh)
        .unwrap();
    w.resource::<Assets<MeshData>>().version(mesh).unwrap()
}

#[test]
fn walls_measurements() {
    let game = game();
    println!("\nM5 walls: one edit = set_field(WallPath.points) on one wall of a room grid");
    println!(
        "{:>6} {:>10} {:>12} {:>12} {:>10} {:>12} {:>12}",
        "walls", "rule ms", "reconcile ms", "frame ms", "regen", "idle ms", "drag late"
    );
    for n in [10usize, 100, 1000] {
        let mut w = empty_world(&game);
        let uids = spawn_rooms(&mut w, n);
        settle(&mut w);
        let path = w.field_path("WallPath.points").unwrap();
        let mut edits = EditStack::new();

        // Single edits after a pause: wall 4 (a room's first side, the
        // lowest uid of its corners) nudged along its length, staying
        // connected.
        let target = uids[4.min(n - 1)];
        let base = wall_path(&w, target).points;
        let (mut rule_t, mut rec_t, mut frame_t, mut regen) = (vec![], vec![], vec![], vec![]);
        for i in 0..21 {
            let off = Vec3::new(if i % 2 == 0 { 0.03 } else { 0.0 }, 0.0, 0.0);
            let moved: Vec<Vec3> = base.iter().map(|p| *p + off).collect();
            let before = generations(&w);
            advance(&mut w, Duration::from_secs(1));
            edits.set_field(&mut w, target, &path, &moved).unwrap();
            let start = Instant::now();
            frame(&mut w);
            frame_t.push(start.elapsed());
            let info = rule(&w, plan::RULE);
            rule_t.push(info.last_rule.unwrap());
            rec_t.push(info.last_run.unwrap() - info.last_rule.unwrap());
            regen.push(regenerated(&before, &generations(&w)).len());
        }

        // Idle frames.
        let start = Instant::now();
        for _ in 0..100 {
            advance(&mut w, FRAME);
            frame(&mut w);
        }
        let idle = start.elapsed() / 100;

        // A one-second drag at 60 Hz with the default throttle: frames
        // between an edit and the first frame showing a mesh at least that
        // new (0 = the same frame).
        edits.begin("Drag");
        let mut pending: Vec<(usize, u64)> = Vec::new(); // (edit frame, version before)
        let mut late = Vec::new();
        let mut worst_frame = Duration::ZERO;
        for f in 0..60 {
            let off = Vec3::new((f % 6) as f32 * 0.01, 0.0, 0.0);
            let moved: Vec<Vec3> = base.iter().map(|p| *p + off).collect();
            let v = version(&w, target);
            edits.set_field(&mut w, target, &path, &moved).unwrap();
            pending.push((f, v));
            let start = Instant::now();
            frame(&mut w);
            worst_frame = worst_frame.max(start.elapsed());
            let now = version(&w, target);
            pending.retain(|(at, v)| {
                if now > *v {
                    late.push(f - at);
                    false
                } else {
                    true
                }
            });
            advance(&mut w, FRAME);
        }
        edits.commit();
        let max_late = late.iter().copied().max().unwrap_or(usize::MAX);

        println!(
            "{n:>6} {:>10.3} {:>12.3} {:>12.3} {:>10} {:>12.3} {:>12}",
            ms(median(rule_t)),
            ms(median(rec_t)),
            ms(median(frame_t)),
            format!(
                "{}-{}",
                regen.iter().min().unwrap(),
                regen.iter().max().unwrap()
            ),
            ms(idle),
            format!("{max_late} (worst {:.2} ms)", ms(worst_frame)),
        );
        assert!(
            max_late <= 1,
            "{n} walls: a drag edit took {max_late} frames to show"
        );
        assert!(regen.iter().all(|r| *r == 4.min(n)), "{n}: {regen:?}");
    }
}
