//! Collector: the M3 sample game. A capsule character, moved by the
//! engine's kinematic character controller, collects spinning gems in an
//! arena; when the last gem of an arena is taken the game switches to the
//! next arena (`arena` -> `arena2` -> `arena` ...), keeping the score.
//!
//! - Saved, inspector-visible components: [`PlayerController`],
//!   [`Collectible`], [`FollowCamera`] and [`ArenaExit`].
//! - Runtime-only state (never saved), added by `on_add` hooks: the
//!   player's `KinematicCharacter` and [`Respawn`] point, the camera's orbit
//!   and the gems' bobbing `Tween`. [`Score`] is a resource, so it survives
//!   scene switches.
//! - Sound: each arena has a wind loop (an `AudioSource` entity in the
//!   scene), and a gem plays its `pickup_sound` where it was taken (a
//!   positional `PlaySound`). Volumes are in `assets/settings/AudioSettings.json`.
//! - Project settings, shared by both arenas: [`CharacterSettings`] in
//!   `assets/settings/CharacterSettings.json` (Game ▸ Project Settings in the
//!   editor; hot-reloads, also during Play).
//! - Actions: `move` (WASD, arrows, d-pad, left stick), `jump` (Space,
//!   gamepad South) and `look` (right stick; the mouse while the cursor is
//!   locked: click to lock, Escape to release).
//!
//! The scenes are data in `assets/scenes`, generated from the JSON edit
//! commands in `authoring/` ([`authoring`]).

pub mod authoring;

use std::f32::consts::{FRAC_PI_2, TAU};
use std::path::PathBuf;

use box_engine::prelude::*;

/// Collision layer of the player (bit 1). Gems only report the player.
pub const PLAYER_LAYER: u32 = 1 << 1;
/// Collision layer of collectibles (bit 2).
pub const COLLECTIBLE_LAYER: u32 = 1 << 2;
/// Character tunables shared by every arena: a project settings resource
/// (`assets/settings/CharacterSettings.json`).
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Resource, Default, Clone)]
pub struct CharacterSettings {
    /// How far below its feet the character looks for ground each step.
    #[reflect(@Range { min: 0.0, max: 1.0 }, @Unit("m"))]
    pub ground_probe: f32,
    /// Fastest fall.
    #[reflect(@Range { min: 1.0, max: 200.0 }, @Unit("m/s"))]
    pub max_fall_speed: f32,
    /// Below this height the character is put back at its start.
    #[reflect(@Range { min: -1000.0, max: 0.0 }, @Unit("m"))]
    pub fall_limit: f32,
}

impl Default for CharacterSettings {
    fn default() -> Self {
        Self {
            ground_probe: 0.1,
            max_fall_speed: 40.0,
            fall_limit: -30.0,
        }
    }
}

/// A character moved by the `move` and `jump` actions. Its settings drive
/// the engine's `KinematicCharacter` (added at runtime). The entity's origin
/// is the centre of its capsule.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone)]
pub struct PlayerController {
    /// Walking speed.
    #[reflect(@Range { min: 0.0, max: 30.0 }, @Unit("m/s"))]
    pub speed: f32,
    /// Downward acceleration while airborne.
    #[reflect(@Range { min: 0.0, max: 100.0 }, @Unit("m/s²"))]
    pub gravity: f32,
    /// Upward speed at the start of a jump.
    #[reflect(@Range { min: 0.0, max: 30.0 }, @Unit("m/s"))]
    pub jump_speed: f32,
    /// Capsule radius used for movement queries.
    #[reflect(@Range { min: 0.05, max: 2.0 }, @Unit("m"))]
    pub radius: f32,
    /// Total capsule height used for movement queries.
    #[reflect(@Range { min: 0.1, max: 4.0 }, @Unit("m"))]
    pub height: f32,
}

impl Default for PlayerController {
    fn default() -> Self {
        Self {
            speed: 6.0,
            gravity: 20.0,
            jump_speed: 7.5,
            radius: 0.4,
            height: 1.8,
        }
    }
}

/// Something to pick up. Needs a trigger `Collider`; the player collects it
/// by touching it, which plays `pickup_sound` where it was.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone)]
pub struct Collectible {
    /// Score for collecting it.
    #[reflect(@Range { min: 0.0, max: 1000.0 })]
    pub points: u32,
    /// Spin around the vertical axis.
    #[reflect(@Range { min: -20.0, max: 20.0 }, @Unit("rad/s"))]
    pub spin_speed: f32,
    /// How far it bobs up and down.
    #[reflect(@Range { min: 0.0, max: 2.0 }, @Unit("m"))]
    pub bob_height: f32,
    /// Played where it is collected.
    pub pickup_sound: AssetRef,
}

/// The default [`Collectible::pickup_sound`].
pub const PICKUP_SOUND: &str = "audio/pickup.wav";

impl Default for Collectible {
    fn default() -> Self {
        Self {
            points: 10,
            spin_speed: 2.0,
            bob_height: 0.15,
            pickup_sound: AssetRef::path(PICKUP_SOUND),
        }
    }
}

/// A third-person camera that follows the player and orbits with `look`.
/// Its saved transform sets the starting view direction.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone)]
pub struct FollowCamera {
    /// Distance from the point it looks at.
    #[reflect(@Range { min: 1.0, max: 50.0 }, @Unit("m"))]
    pub distance: f32,
    /// Height of the point it looks at above the player's origin.
    #[reflect(@Range { min: -2.0, max: 10.0 }, @Unit("m"))]
    pub height: f32,
    /// Starting downward angle.
    #[reflect(@Range { min: -0.5, max: 1.4 }, @Unit("rad"))]
    pub pitch: f32,
    /// How quickly it catches up with the player (higher is snappier).
    #[reflect(@Range { min: 0.5, max: 50.0 }, @Unit("1/s"))]
    pub smoothing: f32,
    /// Turn rate at full right-stick deflection.
    #[reflect(@Range { min: 0.0, max: 10.0 }, @Unit("rad/s"))]
    pub look_speed: f32,
    /// Turn per unit of mouse motion while the cursor is locked.
    #[reflect(@Range { min: 0.0, max: 0.05 }, @Unit("rad"))]
    pub mouse_sensitivity: f32,
}

impl Default for FollowCamera {
    fn default() -> Self {
        Self {
            distance: 8.0,
            height: 1.0,
            pitch: 0.4,
            smoothing: 10.0,
            look_speed: 2.5,
            mouse_sensitivity: 0.004,
        }
    }
}

/// The scene to switch to when every collectible in this one is collected.
#[derive(Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default, Clone)]
pub struct ArenaExit {
    /// A scene name (`arena2`) or a path relative to the project.
    pub next_scene: String,
}

/// Where the player started; it returns here after falling out (runtime).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Respawn(pub Vec3);

/// Resource: the score. Runtime only, so it survives scene switches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Score {
    pub points: u32,
    pub collected: u32,
    pub arenas_completed: u32,
}

/// Runtime state of a [`FollowCamera`].
struct FollowState {
    yaw: f32,
    pitch: f32,
    /// Not yet placed: jump to the desired position instead of easing.
    fresh: bool,
}

/// The game plugin.
pub struct Collector;

impl Plugin for Collector {
    fn register(&self, registry: &mut Registry) {
        // Errors are logged by `Game::with_plugin`.
        registry.register_all::<(PlayerController, Collectible, FollowCamera, ArenaExit)>();
        let _ = registry.register_settings::<CharacterSettings>();
    }

    fn build(&self, app: &mut App) {
        // `assets/settings/input.actions.json` overrides these per action.
        app.world
            .resource_or_default::<ActionMap>()
            .merge_missing(&default_actions());
        app.world.resource_or_default::<Score>();
        // The player sets the project root; an editor Play session does not
        // (yet), so fall back to this sample's own folder for scene names.
        let scenes = app.world.resource_or_default::<Scenes>();
        if scenes.project_root().is_none() {
            scenes.set_project_root(Some(PathBuf::from(env!("CARGO_MANIFEST_DIR"))));
        }
        scenes.set_runtime_entities(RuntimeEntities::DespawnUnlessPersistent);
        app.add_plugin(&CharacterPlugin);
        app.on_add::<PlayerController>(add_character);
        app.on_add::<Collectible>(add_bob);
        // Load the pickup sound now, not on the first pickup.
        app.on_add::<Collectible>(|world, e| {
            let sound = world.get::<Collectible>(e).map(|c| c.pickup_sound.clone());
            if let (Some(sound), Some(audio)) = (sound, world.get_resource_mut::<Audio>()) {
                audio.preload(sound);
            }
        });
        app.on_add::<FollowCamera>(|world, e| {
            let yaw = world.get::<Transform>(e).map_or(0.0, |t| yaw_of(&t));
            let pitch = world.get::<FollowCamera>(e).map_or(0.4, |f| f.pitch);
            let _ = world.insert_one(
                e,
                FollowState {
                    yaw,
                    pitch,
                    fresh: true,
                },
            );
        });
        app.system(Stage::FixedUpdate, move_players)
            .before("move_characters");
        app.on_event(Stage::Update, collect);
        app.add_system(Stage::Update, spin_collectibles);
        app.add_system(Stage::Update, grab_cursor);
        app.add_system(Stage::Update, follow_camera);
    }
}

/// The actions the game reads, unless the world's `ActionMap` already
/// defines them.
fn default_actions() -> ActionMap {
    ActionMap::new()
        .axis2(
            "move",
            [
                Axis2Binding::wasd(),
                Axis2Binding::arrows(),
                Axis2Binding::dpad(),
                Axis2Binding::Stick(GamepadStick::Left),
            ],
        )
        .button(
            "jump",
            [
                Binding::Key(KeyCode::Space),
                Binding::Gamepad(GamepadButton::South),
            ],
        )
        .axis2("look", [Axis2Binding::Stick(GamepadStick::Right)])
}

// ---- character -----------------------------------------------------------

/// A new player gets the engine's character controller, interpolation
/// (it moves at 60 Hz; the display may run faster) and a respawn point.
fn add_character(world: &mut World, e: Entity) {
    let at = world
        .get::<Transform>(e)
        .map_or(Vec3::ZERO, |t| t.translation);
    let character = KinematicCharacter::default();
    let _ = world.insert(e, (character, Interpolated::default(), Respawn(at)));
}

/// Yaw of the view the player moves relative to: the follow camera's orbit
/// (not its smoothed, lagging transform), else the facing of a follow or
/// active camera, else 0 (looking down -Z).
fn view_yaw(world: &World) -> f32 {
    if let Some(s) = world.query::<&FollowState>().iter().next() {
        return s.yaw;
    }
    let active = || {
        let mut cameras = world.query::<(Entity, &Camera)>();
        cameras.iter().find(|(_, c)| c.active).map(|(e, _)| e)
    };
    let camera = world.first_with::<FollowCamera>().or_else(active);
    camera
        .and_then(|e| world.get::<Transform>(e))
        .map_or(0.0, |t| yaw_of(&t))
}

/// The heading of a transform's forward (-Z) direction; 0 looks down -Z.
fn yaw_of(t: &Transform) -> f32 {
    let f = t.forward();
    if f.x.abs() + f.z.abs() > 1e-4 {
        (-f.x).atan2(-f.z)
    } else {
        0.0
    }
}

/// The horizontal world direction for a `move` value under a view yaw.
pub fn wish_direction(input: Vec2, yaw: f32) -> Vec3 {
    let (sin, cos) = yaw.sin_cos();
    let forward = Vec3::new(-sin, 0.0, -cos);
    let right = Vec3::new(cos, 0.0, -sin);
    (right * input.x + forward * input.y).clamp_length_max(1.0)
}

/// Input to the character controller, each fixed step before it moves
/// (live edits of `PlayerController` apply at once); a player that fell
/// out of the arena goes back to its start.
fn move_players(world: &mut World) {
    let actions = world.resource::<Actions>();
    let (input, jump) = (actions.axis2("move"), actions.pressed("jump"));
    let wish = wish_direction(input, view_yaw(world));
    let settings = world
        .get_resource::<CharacterSettings>()
        .cloned()
        .unwrap_or_default();
    let mut fell = Vec::new();
    for (e, p, c, t, respawn) in world.query_mut::<(
        Entity,
        &PlayerController,
        &mut KinematicCharacter,
        &mut Transform,
        &Respawn,
    )>() {
        (c.radius, c.height, c.gravity, c.jump_speed) =
            (p.radius, p.height, p.gravity, p.jump_speed);
        (c.max_fall_speed, c.ground_snap) = (settings.max_fall_speed, settings.ground_probe);
        (c.velocity.x, c.velocity.z) = (wish.x * p.speed, wish.z * p.speed);
        if jump {
            c.jump();
        }
        if t.translation.y < settings.fall_limit || !t.translation.is_finite() {
            info!("fell out of the arena; back to the start");
            t.translation = respawn.0;
            c.velocity = Vec3::ZERO;
            fell.push(e);
        }
    }
    for e in fell {
        Interpolated::reset(world, e);
    }
}

// ---- collectibles ----------------------------------------------------------

/// Gems bob up and down with a sine-shaped tween, out of step with each
/// other.
fn add_bob(world: &mut World, e: Entity) {
    let (Some(p), Some(h)) = (
        world.get::<Transform>(e).map(|t| t.translation),
        world.get::<Collectible>(e).map(|c| c.bob_height),
    ) else {
        return;
    };
    let phase = (p.x * 0.7 + p.z * 1.3).rem_euclid(TAU) / 2.0;
    let bob = Tween::to("Transform.translation.y", p.y + h)
        .from(p.y - h)
        .over(FRAC_PI_2)
        .ease(Ease::SineInOut)
        .repeat(Repeat::PingPong)
        .offset(phase);
    let _ = world.insert_one(e, bob);
}

fn collect(world: &mut World, event: &TriggerEvent) {
    if event.kind != TriggerKind::Enter || !world.has::<PlayerController>(event.other) {
        return;
    }
    let Some((points, sound)) = world
        .get::<Collectible>(event.trigger)
        .map(|c| (c.points, c.pickup_sound.clone()))
    else {
        return; // already collected, or not a collectible
    };
    let at = world
        .get::<GlobalTransform>(event.trigger)
        .map_or(Vec3::ZERO, |g| g.0.translation.into());
    world.send(PlaySound::clip(sound).at(at).distances(2.0, 40.0));
    world.despawn(event.trigger);
    let left = world.count::<&Collectible>();
    let score = world.resource_mut::<Score>();
    score.points += points;
    score.collected += 1;
    info!(
        "collected +{points}: score {}, {left} left in this arena",
        score.points
    );
    if left == 0 {
        complete_arena(world);
    }
}

/// Every collectible is gone: count the arena and switch to its exit scene.
fn complete_arena(world: &mut World) {
    let mut exits = world.query::<&ArenaExit>();
    let next = exits
        .iter()
        .map(|a| a.next_scene.clone())
        .find(|s| !s.is_empty());
    drop(exits);
    let (score, scenes) = world.resources_mut::<(Score, Scenes)>();
    score.arenas_completed += 1;
    let done = format!(
        "arena complete ({} so far, score {})",
        score.arenas_completed, score.points
    );
    match next {
        Some(next) => {
            info!("{done}; switching to `{next}`");
            scenes.switch(next);
        }
        None => info!("{done}; no exit set"),
    }
}

fn spin_collectibles(world: &mut World) {
    let dt = world.resource::<Time>().delta;
    for (transform, c) in world.query_mut::<(&mut Transform, &Collectible)>() {
        transform.rotation = Quat::from_rotation_y(c.spin_speed * dt) * transform.rotation;
    }
}

// ---- camera ----------------------------------------------------------------

/// Click to lock the cursor for mouse look; Escape releases it.
fn grab_cursor(world: &mut World) {
    let input = world.resource::<Input>();
    let (click, escape) = (
        input.button_pressed(MouseButton::Left),
        input.key_pressed(KeyCode::Escape),
    );
    let Some(cursor) = world.get_resource_mut::<CursorOptions>() else {
        return;
    };
    if escape {
        cursor.release();
    } else if click {
        cursor.lock();
    }
}

fn follow_camera(world: &mut World) {
    let dt = world.resource::<Time>().delta;
    let stick = world.resource::<Actions>().axis2("look");
    let mouse = match world.get_resource::<CursorOptions>() {
        Some(c) if c.is_locked() => world.resource::<Input>().mouse_delta,
        _ => Vec2::ZERO,
    };
    let Some(player) = world.first_with::<PlayerController>() else {
        return;
    };
    // Where the player is drawn this frame, so the camera moves as smoothly.
    let target = interpolated_transform(world, player).map_or(Vec3::ZERO, |t| t.translation);
    let physics = world.get_resource::<Physics>();
    let mut cameras = world.query::<(&mut Transform, &FollowCamera, &mut FollowState)>();
    for (t, follow, s) in cameras.iter() {
        s.yaw -= stick.x * follow.look_speed * dt + mouse.x * follow.mouse_sensitivity;
        s.pitch = (s.pitch - stick.y * follow.look_speed * dt + mouse.y * follow.mouse_sensitivity)
            .clamp(-0.3, 1.3);
        let focus = target + Vec3::Y * follow.height;
        let rotation = Quat::from_rotation_y(s.yaw) * Quat::from_rotation_x(-s.pitch);
        let back = rotation * Vec3::Z;
        // Pull in when a wall is between the player and the camera.
        let filter = QueryFilter::new().excluding(player);
        let distance = physics
            .and_then(|p| p.raycast(&Ray::new(focus, back), follow.distance, &filter))
            .map_or(follow.distance, |hit| (hit.toi - 0.3).max(0.5));
        let desired = focus + back * distance;
        let ease = if std::mem::take(&mut s.fresh) {
            1.0
        } else {
            1.0 - (-follow.smoothing * dt).exp()
        };
        t.translation = t.translation.lerp(desired, ease);
        let look = focus - t.translation;
        if look.length_squared() > 1e-4 {
            t.look_to(look, Vec3::Y);
        } else {
            t.rotation = rotation;
        }
    }
}
