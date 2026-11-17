//! RS4 costs at 2560x1440: spot lights (1 to 256), the shadowed local light
//! (spot: one view, point: six), the ambient volume, and 10,000 particles
//! (CPU simulation, instance building + sort + upload, GPU draw). Each
//! configuration runs as a block of frames after a warm-up; minimum and
//! 10th percentile are reported (other work on the machine only adds time).
//!
//! `cargo test --release -p box_engine --test rs4_cost -- --ignored --nocapture`

#[path = "../examples/common/mod.rs"]
mod common;

use std::time::Instant;

use box_engine::prelude::*;
use common::interior::*;

const FRAMES: usize = 120;
const WARMUP: usize = 10;

fn stats(v: &mut [f32]) -> (f32, f32) {
    v.sort_by(f32::total_cmp);
    (v[0], v[v.len() / 10])
}

/// (sim min, sim p10, submit min, submit p10, GPU min, GPU p10) in ms.
fn measure(r: &mut Renderer, world: &mut World, simulate: bool) -> [f32; 6] {
    let (mut sim, mut submit, mut gpu) = (Vec::new(), Vec::new(), Vec::new());
    for frame in 0..FRAMES + WARMUP {
        let t = Instant::now();
        if simulate {
            box_engine::box_render::simulate_particles(world);
            world.apply_deferred();
        }
        let s = t.elapsed().as_secs_f32() * 1000.0;
        r.render(world).unwrap();
        if frame >= WARMUP {
            sim.push(s);
            submit.push(r.stats().cpu_submit_ms);
            gpu.push(r.stats().gpu_ms);
        }
    }
    let (a, b) = stats(&mut sim);
    let (c, d) = stats(&mut submit);
    let (e, f) = stats(&mut gpu);
    [a, b, c, d, e, f]
}

fn report(name: &str, base: Option<[f32; 6]>, m: [f32; 6]) {
    let delta = base.map_or(String::new(), |b| {
        format!(
            " (delta GPU min {:+.3}, CPU submit min {:+.3})",
            m[4] - b[4],
            m[2] - b[2]
        )
    });
    println!(
        "{name:<34} sim {:.3}/{:.3}  submit {:.3}/{:.3}  GPU {:.3}/{:.3} ms{delta}",
        m[0], m[1], m[2], m[3], m[4], m[5]
    );
}

fn interior(options: InteriorOptions) -> World {
    let mut app = box_engine::default_app();
    build_interior(&mut app.world, options);
    app.tick(1.0 / 60.0);
    app.world
}

fn remove_local_lights(world: &mut World) {
    let lights: Vec<Entity> = world
        .query::<Entity>()
        .iter()
        .filter(|&e| world.has::<SpotLight>(e) || world.has::<PointLight>(e))
        .collect();
    for e in lights {
        world.despawn(e);
    }
}

fn add_spots(world: &mut World, n: usize) {
    for i in 0..n {
        let (u, v) = ((i % 16) as f32 / 15.0, (i / 16) as f32 / 15.0);
        let at = Vec3::new(-3.6 + 7.2 * u, 2.8, -2.6 + 5.2 * v.min(1.0));
        world.spawn((
            Transform::from_translation(at).looking_at(at - Vec3::Y, Vec3::Z),
            GlobalTransform::default(),
            SpotLight {
                intensity: 4.0,
                range: 4.0,
                ..Default::default()
            },
        ));
    }
    box_engine::box_core::transform::propagate_transforms(world);
}

#[test]
#[ignore = "measurement: prints RS4 light and particle costs at 2560x1440"]
fn rs4_costs() {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        return;
    }
    let mut r = Renderer::new_headless(2560, 1440).expect("renderer");
    assert!(r.set_gpu_timing(true), "timestamp queries");
    println!("{} ({:?})", r.adapter_info().name, r.adapter_info().backend);

    // Lights in the interior.
    let mut world = interior(InteriorOptions::default());
    let full = measure(&mut r, &mut world, false);
    report("interior (shadowed spot, 2 points)", None, full);
    let mut bare = interior(InteriorOptions::default());
    remove_local_lights(&mut bare);
    let base = measure(&mut r, &mut bare, false);
    report("no local lights", None, base);
    for n in [1, 16, 64, 256] {
        let mut w = interior(InteriorOptions::default());
        remove_local_lights(&mut w);
        add_spots(&mut w, n);
        let m = measure(&mut r, &mut w, false);
        assert_eq!(r.stats().spot_lights as usize, n);
        report(&format!("{n} spot lights (unshadowed)"), Some(base), m);
    }
    for (name, shadowed) in [("spot", Shadowed::Spot), ("point", Shadowed::Point)] {
        for resolution in [1024, 2048] {
            let mut w = interior(InteriorOptions {
                shadowed,
                ..Default::default()
            });
            let mut unshadowed = interior(InteriorOptions {
                shadowed,
                ..Default::default()
            });
            for (l, s) in [(&mut w, true), (&mut unshadowed, false)] {
                for p in l.query::<&mut PointLight>().iter() {
                    p.shadows &= s;
                    p.shadow.resolution = resolution;
                }
                for p in l.query::<&mut SpotLight>().iter() {
                    p.shadows &= s;
                    p.shadow.resolution = resolution;
                }
            }
            let off = measure(&mut r, &mut unshadowed, false);
            let on = measure(&mut r, &mut w, false);
            assert!(r.stats().local_shadow_views > 0);
            report(
                &format!(
                    "shadowed {name} {resolution}² ({} views)",
                    r.stats().local_shadow_views
                ),
                Some(off),
                on,
            );
        }
    }
    let mut leak = interior(InteriorOptions {
        ambient_override: false,
        ..Default::default()
    });
    let no_volume = measure(&mut r, &mut leak, false);
    report("ambient volume (vs none)", Some(no_volume), full);

    // 10,000 particles in the particles scene's view.
    let scene = || {
        let mut app = box_engine::default_app();
        build_particles(&mut app.world);
        let emitters: Vec<Entity> = app
            .world
            .query::<(Entity, &ParticleEmitter)>()
            .iter()
            .map(|(e, _)| e)
            .collect();
        for e in emitters {
            app.world.despawn(e);
        }
        app.tick(1.0 / 60.0);
        app.world
    };
    let mut empty = scene();
    let none = measure(&mut r, &mut empty, true);
    report("particles: none", None, none);
    for (name, blend, lit, soft) in [
        (
            "10k alpha (sorted), lit, soft",
            ParticleBlend::Alpha,
            true,
            0.3,
        ),
        ("10k alpha (sorted)", ParticleBlend::Alpha, false, 0.0),
        ("10k additive", ParticleBlend::Additive, false, 0.0),
    ] {
        let mut w = scene();
        w.spawn((
            Transform::from_xyz(0.0, 1.0, 0.0),
            GlobalTransform::default(),
            ParticleEmitter {
                rate: 5000.0,
                lifetime: Vec2::splat(2.0),
                max_particles: 10_000,
                speed: Vec2::new(0.2, 1.0),
                cone_angle: std::f32::consts::PI,
                spawn_radius: 1.5,
                drag: 0.5,
                size: vec![CurveKey {
                    t: 0.0,
                    value: 0.12,
                }],
                blend,
                lit,
                soft_distance: soft,
                ..Default::default()
            },
        ));
        box_engine::box_core::transform::propagate_transforms(&mut w);
        // Fill to the cap first.
        for _ in 0..150 {
            w.resource_mut::<Time>().delta = 1.0 / 60.0;
            box_engine::box_render::simulate_particles(&mut w);
            w.apply_deferred();
        }
        w.resource_mut::<Time>().delta = 1.0 / 60.0;
        let m = measure(&mut r, &mut w, true);
        // Particles fully faded out at the end of their life are skipped.
        assert!(r.stats().particles > 9_800, "{}", r.stats().particles);
        report(&format!("particles: {name}"), Some(none), m);
    }
    assert_eq!(r.validation_errors(), 0);
}
