//! Minimal native: a box-engine game built with only the `physics` and
//! `audio` engine modules (`box_engine = { default-features = false,
//! features = ["physics", "audio"] }`): no rigid bodies (`dynamics`, rapier)
//! and no Luau scripts (`script`, mlua).
//!
//! An orb circles a beacon pad; the pad's trigger collider lights the pad
//! and plays a chime while the orb passes through it.
//!
//! The start scene is built by [`main_scene`]; regenerate
//! `assets/scenes/main.scene.json` with
//! `cargo run -p minimal_native --example minimal_native_author` (a test fails when the
//! file is out of date).

use std::f32::consts::TAU;
use std::path::PathBuf;

use box_engine::box_core::scene;
use box_engine::prelude::*;
use box_engine::project::starter_environment;

/// Moves its entity on a horizontal circle around the world origin.
#[derive(Reflect, Clone)]
#[reflect(Component, Default, Clone)]
pub struct Orbit {
    /// Distance from the vertical axis through the origin.
    #[reflect(@Range { min: 0.0, max: 50.0 }, @Unit("m"))]
    pub radius: f32,
    /// Turns per second.
    #[reflect(@Range { min: -4.0, max: 4.0 }, @Unit("turn/s"))]
    pub speed: f32,
    /// Angle at time zero, measured from +X toward +Z.
    #[reflect(@Unit("rad"))]
    pub phase: f32,
}

impl Default for Orbit {
    fn default() -> Self {
        Self {
            radius: 2.0,
            speed: 0.25,
            phase: 0.0,
        }
    }
}

/// A trigger zone that glows while a collider is inside it and plays a
/// chime when one enters. Needs a trigger `Collider` and a
/// `StandardMaterial` on the same entity.
#[derive(Reflect, Clone)]
#[reflect(Component, Default, Clone)]
pub struct Beacon {
    /// Emitted light while something is inside.
    pub glow: Color,
    /// Played at the beacon when something enters (none: silent).
    pub chime: AssetRef,
}

impl Default for Beacon {
    fn default() -> Self {
        Self {
            glow: Color::linear(2.0, 1.2, 0.3),
            chime: AssetRef::NONE,
        }
    }
}

fn orbit(world: &mut World) {
    let t = world.resource::<Time>().elapsed;
    for (transform, orbit) in world.query_mut::<(&mut Transform, &Orbit)>() {
        let a = orbit.phase + orbit.speed * TAU * t;
        transform.translation.x = orbit.radius * a.cos();
        transform.translation.z = orbit.radius * a.sin();
    }
}

fn beacon(world: &mut World, event: &TriggerEvent) {
    let Some(beacon) = world
        .get::<Beacon>(event.trigger)
        .map(|b| Beacon::clone(&b))
    else {
        return;
    };
    let entered = event.kind == TriggerKind::Enter;
    if let Some(material) = world.component_mut::<StandardMaterial>(event.trigger) {
        material.emissive = if entered { beacon.glow } else { Color::BLACK };
    }
    if entered && beacon.chime.is_some() {
        let at = world
            .get::<GlobalTransform>(event.trigger)
            .map_or(Vec3::ZERO, |g| g.0.translation.into());
        world.send(PlaySound::clip(beacon.chime).at(at));
    }
    info!("beacon {}", if entered { "lit" } else { "dark" });
}

/// The game's types and systems.
pub struct MinimalGame;

impl Plugin for MinimalGame {
    fn register(&self, registry: &mut Registry) {
        registry.register_all::<(Orbit, Beacon)>();
    }

    fn build(&self, app: &mut App) {
        app.add_system(Stage::Update, orbit);
        app.on_event(Stage::Update, beacon);
    }
}

/// The game as the player and tests run it.
pub fn game() -> Game {
    Game::new().with_plugin(MinimalGame)
}

/// This sample's project folder.
pub fn project_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The start scene's file.
pub fn main_scene_path() -> PathBuf {
    project_dir().join("assets/scenes/main.scene.json")
}

/// The start scene, saved: ground, beacon pad, orbiting orb, sun and
/// camera, with fixed uids so the file is reproducible.
pub fn main_scene(game: &Game) -> Result<Vec<u8>, scene::SceneError> {
    let mut w = game.new_world();
    w.insert_resource(starter_environment());
    let uid = |n: u128| Uid::from_u128(0x6d696e69_0000_4000_8000_000000000000 | n);
    w.spawn_saved(
        uid(1),
        "Ground",
        (
            Transform::default(),
            Primitive::plane(Vec2::splat(40.0)).with_shadows(false),
            StandardMaterial::color(Color::hex(0x56704a)),
        ),
    )?;
    w.spawn_saved(
        uid(2),
        "Beacon",
        (
            Transform::from_xyz(2.5, 0.1, 0.0),
            Primitive::cylinder(0.7, 0.2),
            // Taller than the pad, so the orb passes through it.
            Collider::cylinder(0.7, 2.0).with_offset(Vec3::Y).trigger(),
            StandardMaterial::color(Color::hex(0x3a4a5c)),
            Beacon {
                chime: AssetRef::path("audio/chime.wav"),
                ..Beacon::default()
            },
        ),
    )?;
    w.spawn_saved(
        uid(3),
        "Orb",
        (
            Transform::from_xyz(-2.5, 0.6, 0.0),
            Primitive::sphere(0.35),
            Collider::from_primitive(),
            StandardMaterial::color(Color::hex(0xd8d0c0)),
            // Starts opposite the beacon and reaches it after one second.
            Orbit {
                radius: 2.5,
                speed: 0.5,
                phase: TAU / 2.0,
            },
        ),
    )?;
    w.spawn_saved(
        uid(4),
        "Sun",
        (
            Transform::default().looking_to(Vec3::new(0.35, -0.7, -0.6), Vec3::Y),
            DirectionalLight {
                shadow_distance: 30.0,
                ..Default::default()
            },
        ),
    )?;
    w.spawn_saved(
        uid(5),
        "Camera",
        (
            Transform::from_xyz(0.0, 4.5, 7.0).looking_at(Vec3::new(0.0, 0.3, 0.0), Vec3::Y),
            Camera::default(),
        ),
    )?;
    scene::save(&w)
}
