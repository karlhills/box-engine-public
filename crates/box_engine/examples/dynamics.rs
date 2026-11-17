//! Rigid-body dynamics (M7): a stack of boxes knocked over by a ball rolling
//! down a ramp, a hinged door swung open by a thrown ball, a swinging chain,
//! and a kinematic character walking up stairs and pushing a crate off the
//! landing, all on scripted input so every run is the same. Esc quits.
//!
//! `cargo run --example dynamics -- --frames 240 --capture target/captures/dynamics.png`
//!
//! The scene is in `common/dynamics.rs` (shared with the dynamics tests).

mod common;

use box_engine::prelude::*;
use common::dynamics::{DynamicsDemo, dynamics_camera};
use common::{TimeOfDay, set_time_of_day, spawn_sun_and_camera};

struct Look;

impl Plugin for Look {
    fn build(&self, app: &mut App) {
        spawn_sun_and_camera(&mut app.world, dynamics_camera());
        set_time_of_day(&mut app.world, TimeOfDay::Day);
        app.add_system(Stage::Update, quit_on_escape);
    }
}

fn quit_on_escape(world: &mut World) {
    if world.resource::<Input>().key_pressed(KeyCode::Escape) {
        world.resource_mut::<AppExit>().0 = true;
    }
}

fn main() -> anyhow::Result<()> {
    Game::new()
        .with_plugin(DynamicsDemo)
        .with_plugin(Look)
        .run("box-engine dynamics")
}
