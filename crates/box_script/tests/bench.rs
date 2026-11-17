//! Script boundary benchmarks (docs/measurements/m7-script.md). Numbers
//! mean something in release:
//! `cargo test --release -p box_script --test bench -- --nocapture`.

mod common;

use std::time::{Duration, Instant};

use box_engine::box_script::{ScriptRuntime, script_update};
use box_engine::prelude::*;
use common::*;

/// Mean time of the scripts' Update stage over `frames` frames.
fn stage_time(app: &mut App, frames: u32) -> Duration {
    for _ in 0..3 {
        script_update(&mut app.world);
    }
    let start = Instant::now();
    for _ in 0..frames {
        script_update(&mut app.world);
    }
    start.elapsed() / frames
}

fn ms(d: Duration) -> String {
    format!("{:.3} ms", d.as_secs_f64() * 1000.0)
}

fn callbacks(body: &str, n: usize, watchdog: bool) -> (Duration, u64) {
    let src = format!(
        "local M = {{}}\nfunction M.on_start(self) self.t = 0 end\nfunction M.on_update(self, dt)\n{body}\nend\nreturn M\n"
    );
    let p = Project::new("bench_cb", &[("scripts/cb.luau", &src)]);
    let mut app = p.app();
    for i in 0..n {
        app.world.spawn((
            Uid::new(),
            Transform::from_xyz(i as f32, 0.0, 0.0),
            Script::new("scripts/cb.luau"),
        ));
    }
    assert!(run_until(&mut app, 30.0, |app| stats(app).instances == n));
    if !watchdog {
        app.world.resource::<ScriptRuntime>().set_watchdog(None);
    }
    let t = stage_time(&mut app, 20);
    let count = stats(&app).callbacks;
    assert_eq!(stats(&app).disabled, 0);
    (t, count)
}

fn manager(body: &str, n: usize) -> Duration {
    let src = format!("local M = {{}}\nfunction M.on_update(self, dt)\n{body}\nend\nreturn M\n");
    let p = Project::new("bench_batch", &[("scripts/m.luau", &src)]);
    let mut app = p.app();
    for i in 0..n {
        app.world.spawn((
            Name::new(format!("e{i}")),
            Transform::from_xyz(i as f32, 0.0, 0.0),
        ));
    }
    spawn_script(&mut app, "Manager", Script::new("scripts/m.luau"));
    assert!(run_until(&mut app, 30.0, |app| stats(app).instances == 1));
    let t = stage_time(&mut app, 20);
    assert_eq!(stats(&app).disabled, 0, "the batch script failed");
    t
}

fn per_op(body: &str) -> f64 {
    let src = format!(
        "local M = {{}}\nfunction M.on_start(self)\n    local e = self.entity\n    local v = vector.create(1, 2, 3)\n    local n = 100000\n    local t0 = os.clock()\n    for i = 1, n do\n        {body}\n    end\n    self.us = (os.clock() - t0) / n * 1e6\nend\nreturn M\n"
    );
    let p = Project::new("bench_op", &[("scripts/op.luau", &src)]);
    let mut app = p.app();
    let e = spawn_script(&mut app, "Op", Script::new("scripts/op.luau"));
    assert!(run_until(&mut app, 30.0, |app| stats(app).instances == 1));
    assert_eq!(stats(&app).disabled, 0, "{body}");
    field::<f64>(&app, e, "us").unwrap()
}

#[test]
fn script_boundary_costs() {
    println!(
        "\n== box_script benchmarks ({}) ==",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    let (t, n) = callbacks("    self.t += dt", 10_000, true);
    assert_eq!(n, 10_000);
    println!(
        "10,000 on_update callbacks (self.t += dt)          {:>12}",
        ms(t)
    );
    let (t2, _) = callbacks("    self.t += dt", 10_000, false);
    println!(
        "  ... without the watchdog interrupt                 {:>12}",
        ms(t2)
    );
    let (t3, _) = callbacks(
        "    self.entity:translate(vector.create(0, dt, 0))",
        10_000,
        true,
    );
    println!(
        "10,000 on_update callbacks, each e:translate(v)     {:>12}",
        ms(t3)
    );
    let t4 = manager(
        "    local up = vector.create(0, dt, 0)\n    for _, e in world.query(\"Transform\", \"Name\") do\n        e:translate(up)\n    end",
        5_000,
    );
    println!(
        "batch: world.query over 5,000 + e:translate         {:>12}",
        ms(t4)
    );
    let t5 = manager(
        "    local n = 0\n    world.each(\"Transform\", function(e, t) n += t.translation.x end)\n    self.n = n",
        5_000,
    );
    println!(
        "batch: world.each over 5,000 with Transform copies  {:>12}",
        ms(t5)
    );
    let t6 = manager(
        "    local n = #world.query(\"Transform\", \"Name\")\n    self.n = n",
        5_000,
    );
    println!(
        "batch: world.query(\"Transform\", \"Name\") alone      {:>12}",
        ms(t6)
    );
    for (label, body) in [
        ("e:position()", "local p = e:position()"),
        ("e:set_position(v)", "e:set_position(v)"),
        (
            "e:get(\"Transform\") (generic copy)",
            "local t = e:get(\"Transform\")",
        ),
        (
            "e:get(\"Transform.translation\")",
            "local p = e:get(\"Transform.translation\")",
        ),
        (
            "e:set(\"Transform\", { translation = v })",
            "e:set(\"Transform\", { translation = v })",
        ),
        ("e:get(\"Name\")", "local s = e:get(\"Name\")"),
        (
            "Quat.from_rotation_y(a) * v",
            "local r = Quat.from_rotation_y(0.5) * v",
        ),
        ("vector math (v + v) * 0.5", "v = (v + v) * 0.5"),
    ] {
        println!("{label:<44} {:>9.3} us/op", per_op(body));
    }
}
