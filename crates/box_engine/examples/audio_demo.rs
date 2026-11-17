//! Audio demo: a positional wind loop circles the scene (the blue sphere),
//! and Space plays a chime at the next marker, alternating left and right
//! of the camera. Sounds are heard from the camera (orbit with the mouse).
//! P pauses and resumes time (and with it the audio), M mutes and unmutes
//! the master volume, Esc quits.
//!
//! The clips are generated (`cargo run -p box_audio --example gen_clips`)
//! and live in `examples/audio_assets/audio`.
//!
//! `cargo run --example audio_demo` (a window, with sound), or headless and
//! silent: `cargo run --example audio_demo -- --simulate --frames 90 --capture target/captures/audio_demo.png`.
//! `--simulate` presses Space every 30 frames.

mod common;

use box_engine::prelude::*;
use common::{TimeOfDay, set_time_of_day, spawn_sun_and_camera};

/// The sphere carrying the wind loop, circling at `radius`.
struct Orbiter {
    radius: f32,
    speed: f32,
}

/// The two places chimes play from; `next` alternates.
struct Markers {
    places: [Vec3; 2],
    next: usize,
}

/// A marker flash that shrinks away.
struct Flash {
    age: f32,
}

fn main() -> anyhow::Result<()> {
    let mut app = box_engine::default_app();
    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/audio_assets");
    app.world.insert_resource(box_engine::asset_server(assets)?);
    build_scene(&mut app.world);
    app.add_system(Stage::Update, keys);
    app.add_system(Stage::Update, orbit);
    app.add_system(Stage::Update, flashes);
    if std::env::args().any(|a| a == "--simulate") {
        app.add_system(Stage::Update, |world: &mut World| {
            if world.resource::<Time>().frame % 30 == 10 {
                chime(world);
            }
        });
    }
    box_engine::run(app, RunOptions::new("box-engine audio demo"))
}

fn build_scene(world: &mut World) {
    world.spawn((
        Transform::default(),
        Primitive::plane(Vec2::splat(40.0)),
        StandardMaterial::color(Color::hex(0x8fa677)),
    ));
    let places = [Vec3::new(-4.0, 0.5, 0.0), Vec3::new(4.0, 0.5, 0.0)];
    for p in places {
        world.spawn((
            Transform::from_translation(p),
            Primitive::cylinder(0.3, 1.0),
            StandardMaterial::color(Color::hex(0xb9b0a2)),
        ));
    }
    world.insert_resource(Markers { places, next: 0 });
    world.spawn((
        Transform::from_xyz(6.0, 1.0, 0.0),
        Primitive::sphere(0.4),
        StandardMaterial::color(Color::hex(0x4b8bf5)),
        AudioSource::ambience("audio/ambience.wav").spatial(1.0, 25.0),
        Orbiter {
            radius: 6.0,
            speed: 0.6,
        },
    ));
    spawn_sun_and_camera(
        world,
        OrbitCamera {
            target: Vec3::new(0.0, 0.5, 0.0),
            distance: 14.0,
            yaw: 0.0,
            pitch: 0.5,
            ..Default::default()
        },
    );
    set_time_of_day(world, TimeOfDay::Day);
}

/// Plays the chime at the next marker and flashes it.
fn chime(world: &mut World) {
    let markers = world.resource_mut::<Markers>();
    let at = markers.places[markers.next];
    markers.next = (markers.next + 1) % markers.places.len();
    world.send(
        PlaySound::new("audio/pickup.wav")
            .at(at)
            .distances(2.0, 30.0),
    );
    world.spawn((
        Transform::from_translation(at + Vec3::Y * 0.9),
        Primitive::cuboid(Vec3::splat(0.5)),
        StandardMaterial::color(Color::hex(0xf2c14e)),
        Flash { age: 0.0 },
    ));
    info!("chime at {at}");
}

fn keys(world: &mut World) {
    let input = world.resource::<Input>();
    let (space, pause, mute, quit) = (
        input.key_pressed(KeyCode::Space),
        input.key_pressed(KeyCode::P),
        input.key_pressed(KeyCode::M),
        input.key_pressed(KeyCode::Escape),
    );
    if space {
        chime(world);
    }
    if pause {
        let time = world.resource_mut::<Time>();
        time.paused = !time.paused;
    }
    if mute {
        let settings = world.resource_or_default::<AudioSettings>();
        settings.master = if settings.master > 0.0 { 0.0 } else { 1.0 };
    }
    if quit {
        world.resource_mut::<AppExit>().0 = true;
    }
}

fn orbit(world: &mut World) {
    let t = world.resource::<Time>().elapsed;
    for (transform, o) in world.query_mut::<(&mut Transform, &Orbiter)>() {
        let a = t * o.speed;
        transform.translation = Vec3::new(a.cos() * o.radius, 1.0, a.sin() * o.radius);
    }
}

fn flashes(world: &mut World) {
    let dt = world.resource::<Time>().delta;
    for (e, t, f) in world.query::<(Entity, &mut Transform, &mut Flash)>().iter() {
        f.age += dt;
        t.scale = Vec3::splat((1.0 - f.age / 0.4).max(0.0));
        if f.age >= 0.4 {
            world.defer().despawn(e);
        }
    }
}
