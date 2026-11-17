//! Scatter with painted regions (M6): grass, bushes, rocks and flowers
//! placed over game-defined hills (`common/scatter.rs`). The meadow has a
//! slope limit, so the mesa's steep sides stay bare; rocks go anywhere; the
//! flowers grow only along a painted stroke. Instances are runtime
//! entities, regenerated per chunk; nothing but the `Scatter` parameters
//! would be saved. Esc quits.
//!
//! `cargo run --example scatter -- --capture target/captures/scatter.png`

mod common;

use box_engine::prelude::*;
use common::scatter::{ScatterDemoPlugin, build_scatter_scene, scatter_camera};
use common::{TimeOfDay, set_time_of_day, spawn_sun_and_camera};

struct Demo;

impl Plugin for Demo {
    fn build(&self, app: &mut App) {
        build_scatter_scene(&mut app.world);
        spawn_sun_and_camera(&mut app.world, scatter_camera());
        set_time_of_day(&mut app.world, TimeOfDay::Day);
        app.add_system(Stage::Update, quit);
    }
}

fn quit(world: &mut World) {
    if world.resource::<Input>().key_pressed(KeyCode::Escape) {
        world.resource_mut::<AppExit>().0 = true;
    }
}

fn main() -> anyhow::Result<()> {
    Game::new()
        .with_plugin(ScatterDemoPlugin)
        .with_plugin(Demo)
        .run("box-engine scatter")
}
