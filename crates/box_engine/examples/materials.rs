//! RS2 material scenes: textures, alpha modes, double-sided faces, point
//! lights, multiple materials per mesh and a custom shader material.
//!
//! `cargo run --example materials -- --scene alpha_modes`
//! `cargo run --example materials -- --scene shader_material --capture target/captures/shader_material.png`
//!
//! Scenes: textured, alpha_modes, double_sided, point_lights_night,
//! point_lights_day, multi_material, shader_material. Esc quits.

mod common;

use box_engine::prelude::*;
use common::materials::*;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let scene = args
        .iter()
        .position(|a| a == "--scene")
        .and_then(|i| args.get(i + 1))
        .map_or("textured", String::as_str);
    let mut app = box_engine::default_app();
    if !build_material_scene(&mut app.world, scene) {
        anyhow::bail!(
            "unknown scene `{scene}`; available: {}",
            MATERIAL_SCENES.join(", ")
        );
    }
    app.add_system(Stage::Update, |world: &mut World| {
        if world.resource::<Input>().key_pressed(KeyCode::Escape) {
            world.resource_mut::<AppExit>().0 = true;
        }
    });
    box_engine::run(app, RunOptions::new("box-engine materials"))
}
