//! RS3 on-demand look scenes: atmosphere over a layered sky, grading and
//! vignette, alpha-tested foliage shadows, wind sway, soft transparency,
//! lit windows at night, and a wide view of pillars in a cloud sea.
//!
//! `cargo run --example look -- --scene pillars`
//! `cargo run --example look -- --scene wind --frames 30 --capture target/captures/look/wind.png`
//!
//! Scenes: pillars, atmosphere, foliage, wind, soft_particles, night.
//! Flags: `--hour <13..21.5>` (time of day for pillars, atmosphere and
//! night), `--time-lapse <hours per second>` (animates it), `--no-haze`,
//! `--no-mist`, `--no-quieting`, `--simple-sky`, `--no-stars`, `--no-grade`,
//! `--no-vignette`, `--no-bloom`, `--calm` (no wind), `--no-depth-fade`.
//! Esc quits.

mod common;

use box_engine::prelude::*;
use common::look::*;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let value = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
    };
    let flag = |name: &str| args.iter().any(|a| a == name);
    let scene = value("--scene").map_or("pillars", String::as_str);
    let mut app = box_engine::default_app();
    if !build_look_scene(&mut app.world, scene) {
        anyhow::bail!(
            "unknown scene `{scene}`; available: {}",
            LOOK_SCENES.join(", ")
        );
    }
    let hour = value("--hour").and_then(|v| v.parse::<f32>().ok());
    if let Some(h) = hour {
        set_reference_time(&mut app.world, h);
    }
    if let Some(speed) = value("--time-lapse").and_then(|v| v.parse::<f32>().ok()) {
        let mut h = hour.unwrap_or(13.0);
        app.add_system(Stage::Update, move |w: &mut World| {
            h += speed * w.resource::<Time>().delta;
            if h > 21.5 {
                h = 13.0;
            }
            set_reference_time(w, h);
        });
    }
    let env = app.world.resource_mut::<Environment>();
    if flag("--no-haze") {
        env.aerial_haze.enabled = false;
    }
    if flag("--no-mist") {
        env.height_mist.enabled = false;
    }
    if flag("--no-quieting") {
        env.distance_quieting.enabled = false;
    }
    if flag("--simple-sky") {
        env.sky = Sky::default();
    }
    if flag("--no-stars") {
        env.sky.stars = 0.0;
    }
    if flag("--no-grade") {
        env.color_grading = ColorGrading::default();
    }
    if flag("--no-vignette") {
        env.vignette = Vignette::default();
    }
    if flag("--no-bloom") {
        env.bloom.enabled = false;
    }
    if flag("--calm") {
        env.wind.strength = 0.0;
    }
    if flag("--no-depth-fade") {
        let handles: Vec<Handle<StandardMaterial>> = app
            .world
            .query::<&MeshRenderer>()
            .iter()
            .flat_map(|r| r.materials.clone())
            .filter_map(|m| match m {
                MaterialHandle::Standard(h) => Some(h),
                MaterialHandle::Shader(_) => None,
            })
            .collect();
        let materials = app.world.resource_mut::<Assets<StandardMaterial>>();
        for h in handles {
            if let Some(m) = materials.get_mut(h) {
                m.depth_fade = 0.0;
            }
        }
    }
    app.add_system(Stage::Update, |world: &mut World| {
        if world.resource::<Input>().key_pressed(KeyCode::Escape) {
            world.resource_mut::<AppExit>().0 = true;
        }
    });
    box_engine::run(app, RunOptions::new("box-engine look"))
}
