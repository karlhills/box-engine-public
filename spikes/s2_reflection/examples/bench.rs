//! Serialization timings. Run with `cargo run --release --example bench`.

use std::time::Instant;

use s2_reflection::{default_registry, fixtures::build_world, scene};

fn time<T>(label: &str, runs: u32, mut f: impl FnMut() -> T) -> T {
    let mut best = f64::MAX;
    let mut out = None;
    for _ in 0..runs {
        let t = Instant::now();
        let v = f();
        best = best.min(t.elapsed().as_secs_f64() * 1000.0);
        out = Some(v);
    }
    println!("{label:<40} {best:>8.2} ms (best of {runs})");
    out.unwrap()
}

fn main() {
    let r = default_registry();
    for n in [10_000usize, 1_000] {
        println!("--- {n} entities ---");
        let world = time("build world (direct inserts)", 3, || build_world(n, 1));
        let bytes = time("save (reflect -> pretty JSON)", 5, || {
            scene::save(&world, &r).unwrap()
        });
        println!("{:<40} {:>8.2} MB", "file size", bytes.len() as f64 / 1e6);
        let parsed: serde_json::Value = time("parse JSON to value tree", 5, || {
            serde_json::from_slice(&bytes).unwrap()
        });
        time("value tree -> world (reflect)", 5, || {
            scene::load_value(&parsed, &r).unwrap()
        });
        let loaded = time("load total (parse + build)", 5, || {
            scene::load(&bytes, &r).unwrap()
        });
        time("play clone (save + load)", 5, || {
            scene::clone_world(&world, &r).unwrap()
        });
        assert_eq!(scene::save(&loaded, &r).unwrap(), bytes);
    }
}
