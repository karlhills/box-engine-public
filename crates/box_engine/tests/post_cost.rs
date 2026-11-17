//! RS3 effect costs: the showcase at 2560x1440 with ambient occlusion,
//! bloom and colour grading toggled. Configurations are interleaved frame by
//! frame so drift and other GPU work on the machine affect them alike; the
//! minimum and the 10th percentile over the frames are reported (other work
//! only ever adds time).
//!
//! `cargo test --release -p box_engine --test post_cost -- --ignored --nocapture`

#[path = "../examples/common/mod.rs"]
mod common;

use std::time::Instant;

use box_engine::prelude::*;
use common::*;

struct Config {
    name: &'static str,
    ao: bool,
    bloom: bool,
    grade: bool,
}

const CONFIGS: &[Config] = &[
    Config {
        name: "none",
        ao: false,
        bloom: false,
        grade: false,
    },
    Config {
        name: "ao",
        ao: true,
        bloom: false,
        grade: false,
    },
    Config {
        name: "bloom",
        ao: false,
        bloom: true,
        grade: false,
    },
    Config {
        name: "grade",
        ao: false,
        bloom: false,
        grade: true,
    },
    Config {
        name: "all",
        ao: true,
        bloom: true,
        grade: true,
    },
];

fn stats(v: &mut [f32]) -> (f32, f32) {
    v.sort_by(f32::total_cmp);
    (v[0], v[v.len() / 10])
}

#[test]
#[ignore = "measurement: prints effect costs at 2560x1440"]
fn post_effect_costs() {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        return;
    }
    let mut r = Renderer::new_headless(2560, 1440).expect("renderer");
    assert!(r.set_gpu_timing(true), "timestamp queries");
    let mut app = box_engine::default_app();
    build_showcase(&mut app.world);
    spawn_sun_and_camera(&mut app.world, showcase_camera());
    set_time_of_day(&mut app.world, TimeOfDay::Day);
    app.tick(1.0 / 60.0);
    let frames = 300;
    // Per configuration: wall time (render, which waits for the GPU), GPU
    // frame span, and pass intervals by label.
    let mut wall = vec![Vec::new(); CONFIGS.len()];
    let mut gpu = vec![Vec::new(); CONFIGS.len()];
    let mut passes: Vec<Vec<(&'static str, Vec<f32>)>> = vec![Vec::new(); CONFIGS.len()];
    for frame in 0..frames + 10 {
        for (i, c) in CONFIGS.iter().enumerate() {
            let env = app.world.resource_mut::<Environment>();
            env.ambient_occlusion.enabled = c.ao;
            env.bloom.enabled = c.bloom;
            env.color_grading = if c.grade {
                ColorGrading {
                    temperature: 0.06,
                    contrast: 1.12,
                    saturation: 0.96,
                    gain: Vec3::new(1.03, 1.0, 0.96),
                    ..Default::default()
                }
            } else {
                ColorGrading::default()
            };
            let t = Instant::now();
            r.render(&mut app.world).unwrap();
            let ms = t.elapsed().as_secs_f32() * 1000.0;
            if frame < 10 {
                continue;
            }
            wall[i].push(ms);
            gpu[i].push(r.stats().gpu_ms);
            let mut sums: Vec<(&'static str, f32)> = Vec::new();
            for &(label, ms) in &r.stats().gpu_passes {
                match sums.iter_mut().find(|s| s.0 == label) {
                    Some(s) => s.1 += ms,
                    None => sums.push((label, ms)),
                }
            }
            for (label, ms) in sums {
                match passes[i].iter_mut().find(|p| p.0 == label) {
                    Some(p) => p.1.push(ms),
                    None => passes[i].push((label, vec![ms])),
                }
            }
        }
    }
    println!("{} ({:?})", r.adapter_info().name, r.adapter_info().backend);
    for (i, c) in CONFIGS.iter().enumerate() {
        let (wmin, wp10) = stats(&mut wall[i]);
        let (gmin, gp10) = stats(&mut gpu[i]);
        let list: Vec<String> = passes[i]
            .iter_mut()
            .map(|(label, v)| {
                let (min, p10) = stats(v);
                format!("{label} {min:.3}/{p10:.3}")
            })
            .collect();
        println!(
            "{:<6} render (CPU + GPU wait) min {wmin:.3} p10 {wp10:.3} ms; GPU frame min {gmin:.3} p10 {gp10:.3} ms; pass intervals min/p10: {}",
            c.name,
            list.join(", ")
        );
    }
    assert_eq!(r.validation_errors(), 0);
}
