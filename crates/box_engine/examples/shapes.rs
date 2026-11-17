//! The built-in generators (`Primitive`, `Lathe`, `Extrude`) side by side,
//! all regenerated from their parameters by the generator runtime. Esc
//! quits.
//!
//! `cargo run --example shapes -- --capture target/captures/shapes.png`

mod common;

use box_engine::prelude::*;
use common::shapes::{build_shapes_scene, shapes_camera};
use common::{TimeOfDay, set_time_of_day, spawn_sun_and_camera};

struct Demo;

impl Plugin for Demo {
    fn build(&self, app: &mut App) {
        build_shapes_scene(&mut app.world);
        spawn_sun_and_camera(&mut app.world, shapes_camera());
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
    Game::new().with_plugin(Demo).run("box-engine shapes")
}
