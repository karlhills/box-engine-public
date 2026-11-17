//! RS4 lights and particles.
//!
//! `cargo run --example interior` — a room lit by a shadowed spot light
//! and two point lights, with a window onto a bright exterior and an
//! `AmbientVolume` so no sky ambient leaks inside.
//!
//! `cargo run --example interior -- --capture target/captures/interior.png`
//! `cargo run --example interior -- --no-ambient-override --capture target/captures/interior_leak.png`
//! `cargo run --example interior -- --shadowed point --capture target/captures/interior_point.png`
//! `cargo run --example interior -- --scene particles --frames 90 --capture target/captures/particles.png`
//!
//! Options: `--scene interior|particles`, `--no-ambient-override` (the
//! room without its ambient volume), `--shadowed spot|point` (which light
//! casts the local shadow). Esc quits.

mod common;

use box_engine::prelude::*;
use common::interior::*;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .map(String::as_str)
    };
    let mut app = box_engine::default_app();
    match value("--scene").unwrap_or("interior") {
        "interior" => {
            let shadowed = match value("--shadowed").unwrap_or("spot") {
                "spot" => Shadowed::Spot,
                "point" => Shadowed::Point,
                other => anyhow::bail!("unknown --shadowed `{other}`; use spot or point"),
            };
            build_interior(
                &mut app.world,
                InteriorOptions {
                    ambient_override: !args.iter().any(|a| a == "--no-ambient-override"),
                    shadowed,
                },
            );
        }
        "particles" => {
            build_particles(&mut app.world);
            app.add_system(Stage::Update, projectile_system);
        }
        other => anyhow::bail!("unknown scene `{other}`; available: interior, particles"),
    }
    app.add_system(Stage::Update, |world: &mut World| {
        if world.resource::<Input>().key_pressed(KeyCode::Escape) {
            world.resource_mut::<AppExit>().0 = true;
        }
    });
    box_engine::run(app, RunOptions::new("box-engine interior"))
}
