//! RS3 on-demand feature costs at 2560x1440: atmosphere, layered sky,
//! grading with shadows/midtones/highlights and vignette, soft
//! transparency (depth fade), alpha-tested foliage shadows, wind sway and
//! stars, each against the same scene without it; and a time-of-day sweep
//! that must not build pipelines. Each configuration runs as a block of
//! frames after a warm-up; the minimum and 10th percentile are reported
//! (other work on the machine only adds time).
//!
//! `cargo test --release -p box_engine --test look_cost -- --ignored --nocapture`

#[path = "../examples/common/mod.rs"]
mod common;

use std::time::Instant;

use box_engine::box_render::SelectionOutline;
use box_engine::prelude::*;
use common::look::*;

const FRAMES: usize = 150;
const WARMUP: usize = 10;

fn stats(v: &mut [f32]) -> (f32, f32) {
    v.sort_by(f32::total_cmp);
    (v[0], v[v.len() / 10])
}

/// Renders `FRAMES` frames after `update` and returns (wall min, wall p10,
/// GPU min, GPU p10) in milliseconds.
fn measure(
    r: &mut Renderer,
    world: &mut World,
    mut update: impl FnMut(&mut World, usize),
) -> [f32; 4] {
    let (mut wall, mut gpu) = (Vec::new(), Vec::new());
    for frame in 0..FRAMES + WARMUP {
        update(world, frame);
        let t = Instant::now();
        r.render(world).unwrap();
        let ms = t.elapsed().as_secs_f32() * 1000.0;
        if frame >= WARMUP {
            wall.push(ms);
            gpu.push(r.stats().gpu_ms);
        }
    }
    let (wmin, wp10) = stats(&mut wall);
    let (gmin, gp10) = stats(&mut gpu);
    [wmin, wp10, gmin, gp10]
}

fn report(scene: &str, name: &str, base: Option<[f32; 4]>, m: [f32; 4]) {
    let delta = base.map_or(String::new(), |b| {
        format!(
            " (delta GPU min {:+.3}, p10 {:+.3})",
            m[2] - b[2],
            m[3] - b[3]
        )
    });
    println!(
        "{scene:<10} {name:<22} render min {:.3} p10 {:.3} ms; GPU frame min {:.3} p10 {:.3} ms{delta}",
        m[0], m[1], m[2], m[3]
    );
}

fn scene(name: &str) -> World {
    let mut app = box_engine::default_app();
    assert!(build_look_scene(&mut app.world, name));
    app.tick(1.0 / 60.0);
    app.world
}

/// Every look feature off (the RS3 core look), AO off.
fn look_off(world: &mut World) {
    let env = world.resource_mut::<Environment>();
    env.aerial_haze.enabled = false;
    env.height_mist.enabled = false;
    env.distance_quieting.enabled = false;
    env.sky = Sky::default();
    env.color_grading = ColorGrading::default();
    env.vignette = Vignette::default();
    env.ambient_occlusion.enabled = false;
}

fn set_depth_fade(world: &mut World, fade: f32) {
    let handles: Vec<Handle<ShaderMaterial>> = world
        .query::<&MeshRenderer>()
        .iter()
        .flat_map(|r| r.materials.clone())
        .filter_map(|m| match m {
            MaterialHandle::Shader(h) => Some(h),
            MaterialHandle::Standard(_) => None,
        })
        .collect();
    let materials = world.resource_mut::<Assets<ShaderMaterial>>();
    for h in handles {
        if let Some(m) = materials.get_mut(h)
            && m.depth_fade != fade
        {
            m.depth_fade = fade;
        }
    }
}

fn standard_handles(world: &World) -> Vec<Handle<StandardMaterial>> {
    let mut v: Vec<Handle<StandardMaterial>> = world
        .query::<&MeshRenderer>()
        .iter()
        .flat_map(|r| r.materials.clone())
        .filter_map(|m| match m {
            MaterialHandle::Standard(h) => Some(h),
            MaterialHandle::Shader(_) => None,
        })
        .collect();
    v.sort_by_key(|h| h.id());
    v.dedup();
    v
}

#[test]
#[ignore = "measurement: prints look feature costs at 2560x1440"]
fn look_feature_costs() {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        return;
    }
    let mut r = Renderer::new_headless(2560, 1440).expect("renderer");
    assert!(r.set_gpu_timing(true), "timestamp queries");
    println!("{} ({:?})", r.adapter_info().name, r.adapter_info().backend);

    // Pillars: each feature against everything off.
    let mut world = scene("pillars");
    let full = *world.resource::<Environment>();
    look_off(&mut world);
    set_depth_fade(&mut world, 0.0);
    let off = *world.resource::<Environment>();
    let base = measure(&mut r, &mut world, |_, _| {});
    report("pillars", "all off", None, base);
    type Apply = fn(&mut Environment, &Environment);
    let configs: [(&str, Apply); 5] = [
        ("haze+mist+quieting", |e, f| {
            e.aerial_haze = f.aerial_haze;
            e.height_mist = f.height_mist;
            e.distance_quieting = f.distance_quieting;
        }),
        ("layered sky", |e, f| e.sky = f.sky),
        ("stars (sky)", |e, f| {
            e.sky = f.sky;
            e.sky.stars = 1.0;
        }),
        ("grade SMH+vignette", |e, f| {
            e.color_grading = f.color_grading;
            e.vignette = f.vignette;
        }),
        ("vignette", |e, f| e.vignette = f.vignette),
    ];
    for (name, apply) in configs {
        *world.resource_mut::<Environment>() = off;
        apply(world.resource_mut::<Environment>(), &full);
        let m = measure(&mut r, &mut world, |_, _| {});
        report("pillars", name, Some(base), m);
    }
    *world.resource_mut::<Environment>() = off;
    set_depth_fade(&mut world, 8.0);
    let m = measure(&mut r, &mut world, |_, _| {});
    report("pillars", "depth fade (prepass)", Some(base), m);
    *world.resource_mut::<Environment>() = full;
    world
        .resource_mut::<Environment>()
        .ambient_occlusion
        .enabled = false;
    let all_no_ao = measure(&mut r, &mut world, |_, _| {});
    report("pillars", "all on, AO off", Some(base), all_no_ao);
    world
        .resource_mut::<Environment>()
        .ambient_occlusion
        .enabled = true;
    let all = measure(&mut r, &mut world, |_, _| {});
    report("pillars", "all on + AO", Some(all_no_ao), all);

    // Time of day animated every frame: uniforms only.
    let pipelines = r.stats().material_pipelines;
    let m = measure(&mut r, &mut world, |w, frame| {
        set_reference_time(w, 13.0 + 8.5 * (frame % 60) as f32 / 60.0);
    });
    report("pillars", "time-of-day sweep", Some(all), m);
    assert_eq!(
        r.stats().material_pipelines,
        pipelines,
        "no pipeline built while the time of day animates"
    );
    assert_eq!(r.stats().buffer_reallocations, 0);

    // Foliage cards: alpha-tested (main, shadow and prepass discard) vs
    // the same cards opaque.
    let mut world = scene("foliage");
    let handles = standard_handles(&world);
    let masked = measure(&mut r, &mut world, |_, _| {});
    for h in &handles {
        let m = world
            .resource_mut::<Assets<StandardMaterial>>()
            .get_mut(*h)
            .unwrap();
        if matches!(m.alpha_mode, AlphaMode::Mask { .. }) {
            m.alpha_mode = AlphaMode::Opaque;
        }
    }
    let opaque = measure(&mut r, &mut world, |_, _| {});
    report("foliage", "cards opaque", None, opaque);
    report("foliage", "cards alpha-tested", Some(opaque), masked);

    // Wind: swaying materials vs the same materials without sway.
    let mut world = scene("wind");
    world.resource_mut::<SelectionOutline>().primary.clear();
    let windy = measure(&mut r, &mut world, |_, _| {});
    for h in standard_handles(&world) {
        let m = world
            .resource_mut::<Assets<StandardMaterial>>()
            .get_mut(h)
            .unwrap();
        m.wind.strength = 0.0;
    }
    let calm = measure(&mut r, &mut world, |_, _| {});
    report("wind", "no sway", None, calm);
    report("wind", "sway (all passes)", Some(calm), windy);

    // Night: stars and lit windows.
    let mut world = scene("night");
    let night = measure(&mut r, &mut world, |_, _| {});
    world.resource_mut::<Environment>().sky.stars = 0.0;
    let no_stars = measure(&mut r, &mut world, |_, _| {});
    report("night", "no stars", None, no_stars);
    report("night", "stars", Some(no_stars), night);

    assert_eq!(r.validation_errors(), 0);
}
