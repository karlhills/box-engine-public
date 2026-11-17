//! Skinned animation (M7): the procedural test character
//! (`examples/animation_assets/models/character.glb`, made by
//! `cargo run -p box_proc --example gen_character`) instanced several
//! times, each driven by an animation state machine whose `speed`
//! parameter and `jump` trigger follow an eight-second schedule: idle,
//! walk, run, jump, idle, with crossfades. Characters start at different
//! points of the schedule; same-pose instances of the mesh are one
//! instanced draw. Esc quits.
//!
//! `cargo run --example animation`
//! `cargo run --example animation -- --capture target/captures/animation.png`
//! `cargo run --example animation -- --start 1.2 --frames 24 --capture 'target/captures/animation/f_{frame}.png'`
//!
//! Options: `--count <n>` characters (default 7; laid out in a grid),
//! `--start <seconds>` where the schedule clock starts (default 0),
//! `--spread <seconds>` schedule offset between characters (default 1.1;
//! 0 puts every character in the same pose), `--hero` frames only the
//! front character, `--bind-pose` draws the meshes unskinned (joints still
//! animate) to measure what GPU skinning costs.
//!
//! Benchmark: `cargo run --release --example animation -- --count 500
//! --spread 0.37 --size 1920x1080 --headless --frames 300 --report <json>`.

mod common;

use box_engine::prelude::*;
use common::{TimeOfDay, set_time_of_day, spawn_sun_and_camera};

const MODEL: &str = "models/character.glb";
/// Schedule length in seconds.
const CYCLE: f32 = 8.0;

/// Drives one character's state machine from the schedule clock.
struct Schedule {
    offset: f32,
    jumped: bool,
}

/// The schedule clock's start (`--start`).
struct Clock {
    start: f32,
}

fn arg<T: std::str::FromStr>(name: &str) -> Option<T> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
}

fn clip(name: &str) -> AssetRef {
    AssetRef::path(&format!("{MODEL}#Animation/{name}"))
}

/// Idle, Walk, Run and a one-shot Jump, steered by `speed` (m/s) and the
/// `jump` trigger.
pub fn locomotion_graph() -> AnimationGraph {
    AnimationGraph {
        states: vec![
            AnimationGraphState::new("Idle", clip("Idle")),
            AnimationGraphState::new("Walk", clip("Walk")),
            AnimationGraphState::new("Run", clip("Run")),
            AnimationGraphState::new("Jump", clip("Jump")).once(),
        ],
        transitions: vec![
            AnimationTransition::new("", "Jump", 0.12).when("jump", ConditionTest::Triggered),
            AnimationTransition::new("Jump", "Idle", 0.3)
                .when("speed", ConditionTest::Less(0.5))
                .after(1.0),
            AnimationTransition::new("Jump", "Run", 0.3)
                .when("speed", ConditionTest::Greater(3.0))
                .after(1.0),
            AnimationTransition::new("Jump", "Walk", 0.3).after(1.0),
            AnimationTransition::new("Idle", "Walk", 0.3)
                .when("speed", ConditionTest::Greater(0.5)),
            AnimationTransition::new("Walk", "Idle", 0.3).when("speed", ConditionTest::Less(0.5)),
            AnimationTransition::new("Walk", "Run", 0.3).when("speed", ConditionTest::Greater(3.0)),
            AnimationTransition::new("Run", "Walk", 0.3).when("speed", ConditionTest::Less(3.0)),
        ],
        start: "Idle".into(),
    }
}

/// Speed (m/s) at schedule time `t`, and whether the jump fires then.
fn schedule(t: f32) -> (f32, bool) {
    let t = t.rem_euclid(CYCLE);
    let speed = match t {
        t if t < 1.5 => 0.0,
        t if t < 3.5 => 1.5,
        t if t < 5.5 => 4.5,
        _ => 0.0,
    };
    (speed, (5.5..6.0).contains(&t))
}

fn drive(world: &mut World) {
    let now = world.resource::<Time>().elapsed + world.resource::<Clock>().start;
    for (machine, s) in world.query_mut::<(&mut AnimationStateMachine, &mut Schedule)>() {
        let (speed, jump) = schedule(now + s.offset);
        machine.set_float("speed", speed);
        if jump && !s.jumped {
            machine.trigger("jump");
        }
        s.jumped = jump;
    }
}

fn main() -> anyhow::Result<()> {
    let count: usize = arg("--count").unwrap_or(7).max(1);
    let start: f32 = arg("--start").unwrap_or(0.0);
    let spread: f32 = arg("--spread").unwrap_or(1.1);
    let hero = std::env::args().any(|a| a == "--hero");
    let bind_pose = std::env::args().any(|a| a == "--bind-pose");
    let mut app = box_engine::default_app();
    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/animation_assets");
    app.world.insert_resource(box_engine::asset_server(assets)?);
    app.world.insert_resource(Clock { start });
    build_scene(&mut app.world, count, spread, hero);
    app.add_system(Stage::Update, drive);
    if bind_pose {
        // Without the binding the asset glue removes the runtime skin.
        app.add_system(Stage::Update, |world: &mut World| {
            for (e, _) in world.query::<(Entity, &SkinBinding)>().iter() {
                world.defer().remove::<SkinBinding>(e);
            }
        });
    }
    app.add_system(Stage::Update, |world: &mut World| {
        if world.resource::<Input>().key_pressed(KeyCode::Escape) {
            world.resource_mut::<AppExit>().0 = true;
        }
    });
    box_engine::run(app, RunOptions::new("box-engine animation"))
}

fn build_scene(world: &mut World, count: usize, spread: f32, hero: bool) {
    world.spawn((
        Transform::default(),
        Primitive::plane(Vec2::splat(60.0)),
        StandardMaterial::color(Color::hex(0x8a9a78)),
    ));
    // A grid, front row first, the hero in front of the middle.
    let columns = (count as f32).sqrt().ceil() as usize;
    for i in 0..count {
        let (row, col) = (i / columns, i % columns);
        let x = (col as f32 - (columns - 1) as f32 * 0.5) * 1.4;
        let z = row as f32 * 1.6;
        world.spawn((
            Name::new(format!("Character {i}")),
            Transform::from_xyz(x, 0.0, z)
                .with_rotation(Quat::from_rotation_y(0.35 * (col as f32 - 1.0))),
            PrefabInstance {
                scene: AssetRef::path(&format!("{MODEL}#Scene")),
            },
            AnimationStateMachine::new(locomotion_graph()),
            Schedule {
                offset: i as f32 * spread,
                jumped: false,
            },
        ));
    }
    let rows = count.div_ceil(columns);
    let (target, distance) = if hero {
        (Vec3::new(0.0, 0.9, 0.0), 4.0)
    } else {
        (
            Vec3::new(0.0, 0.8, (rows - 1) as f32 * 0.8),
            4.5 + rows as f32 * 1.6,
        )
    };
    spawn_sun_and_camera(
        world,
        OrbitCamera {
            target,
            distance,
            yaw: std::f32::consts::PI - 0.55,
            pitch: 0.28,
            ..Default::default()
        },
    );
    set_time_of_day(world, TimeOfDay::Day);
}
