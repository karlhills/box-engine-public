//! A game-defined generator (`Steps`, in `common/steps.rs`): how a game
//! declares a procedural object type and registers it, then two flights
//! of steps regenerated from their parameters. Up/Down add or remove a
//! step on the wobbly flight (a parameter edit, picked up by the throttled
//! flush); Esc quits.
//!
//! `cargo run --example generator -- --capture target/captures/generator.png`

mod common;

use box_engine::prelude::*;
use common::steps::{Steps, StepsPlugin, build_steps_scene, steps_camera};
use common::{TimeOfDay, set_time_of_day, spawn_sun_and_camera};

struct Demo;

impl Plugin for Demo {
    fn build(&self, app: &mut App) {
        build_steps_scene(&mut app.world);
        spawn_sun_and_camera(&mut app.world, steps_camera());
        set_time_of_day(&mut app.world, TimeOfDay::Day);
        app.add_system(Stage::Update, keys);
    }
}

fn keys(world: &mut World) {
    let input = world.resource::<Input>().clone();
    if input.key_pressed(KeyCode::Escape) {
        world.resource_mut::<AppExit>().0 = true;
    }
    let delta =
        i32::from(input.key_pressed(KeyCode::Up)) - i32::from(input.key_pressed(KeyCode::Down));
    if delta != 0 {
        for (steps, _) in world.query::<(&mut Steps, &Procedural)>().iter() {
            steps.count = steps.count.saturating_add_signed(delta).clamp(1, 64);
        }
    }
}

fn main() -> anyhow::Result<()> {
    Game::new()
        .with_plugin(StepsPlugin)
        .with_plugin(Demo)
        .run("box-engine generator")
}
