//! Gameplay samples of the native-plugin guide: a player that walks with
//! the `move` action, pickups it collects, a score, an event and a
//! settings resource. The scene `assets/scenes/main.scene.json` uses them.

use std::f32::consts::FRAC_PI_2;

use box_engine::prelude::*;

/// The player: walks on the ground with the `move` action.
#[derive(Reflect, Clone)]
#[reflect(Component, Default, Clone)]
pub struct Mover {
    /// Walking speed.
    #[reflect(@Range { min: 0.0, max: 20.0 }, @Unit("m/s"))]
    pub speed: f32,
}

impl Default for Mover {
    fn default() -> Self {
        Self { speed: 5.0 }
    }
}

/// Something the player collects by walking into it.
#[derive(Reflect, Default, Clone)]
#[reflect(Component, Default, Clone)]
pub struct Pickup {
    /// Score for collecting it.
    #[reflect(@Range { min: 0.0, max: 100.0 }, @Step(1.0))]
    pub points: u32,
    /// How far it bobs up and down.
    #[reflect(@Range { min: 0.0, max: 1.0 }, @Unit("m"))]
    pub bob_height: f32,
    /// A note for level designers: saved, but not shown in the inspector.
    #[reflect(@Hidden)]
    pub note: String,
}

/// Tunables shared by every scene, in `assets/settings/Tuning.json`.
#[derive(Reflect, Clone)]
#[reflect(Resource, Default, Clone)]
pub struct Tuning {
    /// How close the player must come to collect a pickup.
    #[reflect(@Range { min: 0.1, max: 5.0 }, @Unit("m"))]
    pub pickup_radius: f32,
}

impl Default for Tuning {
    fn default() -> Self {
        Self { pickup_radius: 1.0 }
    }
}

/// The score: a plain runtime resource (not registered, so never saved),
/// which survives scene switches.
#[derive(Default)]
pub struct Score {
    pub points: u32,
    /// Levels cleared (every pickup collected).
    pub levels: u32,
}

/// Sent when a pickup is collected. Registered, so scripts can send it and
/// handle it by name too.
#[derive(Reflect, Default, Clone, Debug)]
#[reflect(Default, Clone)]
pub struct PickedUp {
    pub points: u32,
}

/// The gameplay plugin.
pub struct GameplayPlugin;

impl Plugin for GameplayPlugin {
    fn register(&self, registry: &mut Registry) {
        // Errors (a clashing name, a missing attribute) are logged by
        // `Game::with_plugin` with this plugin's name.
        registry.register_all::<(Mover, Pickup)>();
        let _ = registry.register_settings::<Tuning>();
        let _ = registry.register_event::<PickedUp>();
    }

    fn build(&self, app: &mut App) {
        app.world.resource_or_default::<Score>();
        // `assets/settings/input.actions.json` overrides these per action.
        app.world
            .resource_or_default::<ActionMap>()
            .merge_missing(&default_actions());
        app.add_event::<PickedUp>();
        app.system(Stage::FixedUpdate, move_player)
            .label("move_player");
        app.system(Stage::FixedUpdate, collect_pickups)
            .after("move_player");
        app.on_event(Stage::Update, add_score);
        app.on_add::<Mover>(|world, e| {
            // Moved in FixedUpdate: drawn between steps.
            let _ = world.insert_one(e, Interpolated::default());
        });
        app.on_add::<Pickup>(add_bob);
    }
}

/// The actions the game reads, unless the world's `ActionMap` (or the
/// project's action file) already defines them.
pub fn default_actions() -> ActionMap {
    ActionMap::new().axis2(
        "move",
        [
            Axis2Binding::wasd(),
            Axis2Binding::arrows(),
            Axis2Binding::Stick(GamepadStick::Left),
        ],
    )
}

/// Moves every player with the `move` action (+Y is forward, along -Z).
fn move_player(world: &mut World) {
    let dt = world.resource::<Time>().delta;
    let input = world.resource::<Actions>().axis2("move");
    let step = Vec3::new(input.x, 0.0, -input.y);
    for (transform, mover) in world.query_mut::<(&mut Transform, &Mover)>() {
        transform.translation += step * mover.speed * dt;
    }
}

/// Collects the pickups near a player: despawns each one and sends a
/// `PickedUp`. The despawn is deferred because the query is still iterating.
fn collect_pickups(world: &mut World) {
    let radius = world.resource::<Tuning>().pickup_radius;
    let players: Vec<Vec3> = world
        .query::<(&Transform, &Mover)>()
        .iter()
        .map(|(t, _)| t.translation)
        .collect();
    let mut collected = Vec::new();
    for (e, transform, pickup) in world.query::<(Entity, &Transform, &Pickup)>().iter() {
        if players
            .iter()
            .any(|p| p.distance(transform.translation) < radius)
        {
            world.defer().despawn(e);
            collected.push(PickedUp {
                points: pickup.points,
            });
        }
    }
    world
        .resource_mut::<Events<PickedUp>>()
        .send_batch(collected);
}

/// Adds a collected pickup's points to the score; when none are left,
/// counts the level and plays it again.
fn add_score(world: &mut World, event: &PickedUp) {
    let left = world.count::<&Pickup>();
    let (score, scenes) = world.resources_mut::<(Score, Scenes)>();
    score.points += event.points;
    info!("+{} points: {} ({left} left)", event.points, score.points);
    if left == 0 && !scenes.has_pending() {
        score.levels += 1;
        scenes.switch("main");
    }
}

/// A new pickup bobs up and down: a runtime `Tween`, never saved.
fn add_bob(world: &mut World, e: Entity) {
    let (Some(y), Some(height)) = (
        world.get::<Transform>(e).map(|t| t.translation.y),
        world.get::<Pickup>(e).map(|p| p.bob_height),
    ) else {
        return;
    };
    let bob = Tween::to("Transform.translation.y", y + height)
        .from(y - height)
        .over(FRAC_PI_2)
        .ease(Ease::SineInOut)
        .repeat(Repeat::PingPong);
    let _ = world.insert_one(e, bob);
}
