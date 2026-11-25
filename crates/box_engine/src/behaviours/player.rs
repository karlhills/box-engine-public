//! [`PlayerController`]: moves a kinematic character from the `move`,
//! `jump` and `run` actions, relative to the camera, and turns and animates
//! its body.

use box_core::action::Actions;
use box_core::animation::AnimationStateMachine;
use box_core::identity::Uid;
use box_core::interpolate::Interpolated;
use box_core::reflect::{Range, Reflect, ReflectComponent, ReflectDefault, Step, Unit};
use box_core::time::Time;
use box_core::transform::{GlobalTransform, Transform};
use box_core::world::{Entity, World};
use box_physics::{Collider, KinematicCharacter};
use box_render::Camera;
use glam::{Quat, Vec2, Vec3};

use super::follow_camera::FollowCameraState;
use super::rules::{GamePhase, GameState, respawn};
use super::view::{turn_towards, wish_dir, yaw_of};

/// The player: moves its `KinematicCharacter` with the `move` action
/// (WASD, arrows, left stick) relative to the camera's view, jumps with
/// `jump` (Space, gamepad South), runs while `run` is held (Shift, gamepad
/// West), turns its body toward where it moves and feeds the body's
/// animation state machine (`speed`, `grounded`, `jump`). Below
/// `fall_limit` it respawns at the last checkpoint. It is the player for
/// interactables and the follow camera.
///
/// A `KinematicCharacter`, a capsule `Collider` and `Interpolated` are added
/// in Play when missing.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct PlayerController {
    /// Off: the player stands still (cut scenes, dialogue).
    pub enabled: bool,
    /// Walking speed at full input.
    #[reflect(@Range { min: 0.0, max: 50.0 }, @Unit("m/s"), @Step(0.1))]
    pub speed: f32,
    /// Speed while `run` is held.
    #[reflect(@Range { min: 0.0, max: 50.0 }, @Unit("m/s"), @Step(0.1))]
    pub run_speed: f32,
    /// The player can jump (height: the character's `jump_speed`).
    pub jump: bool,
    /// How fast the body turns toward where it moves.
    #[reflect(@Range { min: 0.0, max: 50.0 }, @Unit("rad/s"), @Step(0.5))]
    pub turn_rate: f32,
    /// Falling below this height respawns the player.
    #[reflect(@Range { min: -1000.0, max: 0.0 }, @Unit("m"), @Step(1.0))]
    pub fall_limit: f32,
    /// The camera movement is relative to. None: the active camera.
    pub camera: Option<Uid>,
    /// The model that turns and animates (a child, usually with an
    /// `AnimationStateMachine`). None: the first child with a state machine,
    /// else the player itself.
    pub body: Option<Uid>,
}

impl Default for PlayerController {
    fn default() -> Self {
        Self {
            enabled: true,
            speed: 6.0,
            run_speed: 10.0,
            jump: true,
            turn_rate: 12.0,
            fall_limit: -20.0,
            camera: None,
            body: None,
        }
    }
}

/// Runtime state of a player.
#[derive(Clone, Copy, Debug, Default)]
struct PlayerRuntime {
    facing: f32,
    jumped: bool,
}

/// Adds what a player needs to move when it is missing (Play only: the
/// edit world runs no hooks).
pub(crate) fn on_player_added(world: &mut World, e: Entity) {
    if !world.has::<KinematicCharacter>(e) {
        let _ = world.insert_one(e, KinematicCharacter::default());
    }
    if !world.has::<Collider>(e) {
        let k = world
            .get::<KinematicCharacter>(e)
            .map(|k| (*k).clone())
            .unwrap_or_default();
        let half = (k.height * 0.5 - k.radius).max(0.0);
        let _ = world.insert_one(e, Collider::capsule(k.radius, half));
    }
    if !world.has::<Interpolated>(e) {
        let _ = world.insert_one(e, Interpolated::default());
    }
    let facing = world
        .get::<Transform>(e)
        .map_or(0.0, |t| yaw_of(t.rotation * Vec3::NEG_Z));
    let _ = world.insert_one(
        e,
        PlayerRuntime {
            facing,
            jumped: false,
        },
    );
}

/// FixedUpdate, before `move_characters`: input to velocity, jumps, falls.
pub(crate) fn drive_players(world: &mut World) {
    // No player, no input read (games without one may not define `move`).
    if world.query::<&PlayerController>().iter().next().is_none() {
        return;
    }
    if super::ensure_game_actions(world) {
        return;
    }
    let playing = world
        .get_resource::<GameState>()
        .is_none_or(|s| s.phase == GamePhase::Playing);
    let (wish, run, jump) = world
        .get_resource::<Actions>()
        .map_or((Vec2::ZERO, false, false), |a| {
            (a.axis2("move"), a.held("run"), a.pressed("jump"))
        });
    let players: Vec<(Entity, PlayerController)> = world
        .query::<(Entity, &PlayerController)>()
        .iter()
        .map(|(e, p)| (e, p.clone()))
        .collect();
    let mut fallen = Vec::new();
    for (e, pc) in players {
        let yaw = view_yaw(world, pc.camera);
        let active = pc.enabled && playing;
        let dir = if active {
            wish_dir(wish, yaw)
        } else {
            Vec3::ZERO
        };
        let speed = if run { pc.run_speed } else { pc.speed };
        let mut jumped = false;
        if let Some(k) = world.component_mut::<KinematicCharacter>(e) {
            k.velocity.x = dir.x * speed;
            k.velocity.z = dir.z * speed;
            if active && pc.jump && jump {
                jumped = k.jump();
            }
        }
        if jumped && let Some(r) = world.component_mut::<PlayerRuntime>(e) {
            r.jumped = true;
        }
        if world
            .get::<Transform>(e)
            .is_some_and(|t| t.translation.y < pc.fall_limit)
        {
            fallen.push(e);
        }
    }
    for e in fallen {
        respawn(world, e);
        // Out of lives: the game is lost, but the player still must not fall
        // forever.
        if world
            .get::<Transform>(e)
            .zip(world.get::<PlayerController>(e))
            .is_some_and(|(t, pc)| t.translation.y < pc.fall_limit)
            && let Some(k) = world.component_mut::<KinematicCharacter>(e)
        {
            k.velocity = Vec3::ZERO;
            k.gravity = 0.0;
        }
    }
}

/// Update: turn the body toward the movement and feed its state machine.
pub(crate) fn animate_players(world: &mut World) {
    let dt = world.get_resource::<Time>().map_or(0.0, |t| t.delta);
    let players: Vec<(Entity, PlayerController)> = world
        .query::<(Entity, &PlayerController)>()
        .iter()
        .map(|(e, p)| (e, p.clone()))
        .collect();
    for (e, pc) in players {
        let Some((velocity, grounded)) = world
            .get::<KinematicCharacter>(e)
            .map(|k| (k.velocity, k.grounded))
        else {
            continue;
        };
        let body = body_of(world, e, pc.body);
        let flat = Vec3::new(velocity.x, 0.0, velocity.z);
        let speed = flat.length();
        let (facing, jumped) = {
            let Some(r) = world.component_mut::<PlayerRuntime>(e) else {
                continue;
            };
            if speed > 0.1 {
                r.facing = turn_towards(r.facing, yaw_of(flat), pc.turn_rate * dt);
            }
            let jumped = std::mem::take(&mut r.jumped);
            (r.facing, jumped)
        };
        if let Some(m) = world.component_mut::<AnimationStateMachine>(body) {
            m.set_float("speed", speed);
            m.set_bool("grounded", grounded);
            if jumped {
                m.trigger("jump");
            }
        }
        if speed > 0.1
            && let Some(t) = world.component_mut::<Transform>(body)
        {
            t.rotation = Quat::from_rotation_y(facing);
        }
    }
}

/// The entity that turns and animates for player `e`.
fn body_of(world: &World, e: Entity, body: Option<Uid>) -> Entity {
    if let Some(b) = body.and_then(|u| world.entity_by_uid(u)) {
        return b;
    }
    let mut stack = world.children(e);
    while let Some(c) = stack.pop() {
        if world.has::<AnimationStateMachine>(c) {
            return c;
        }
        stack.extend(world.children(c));
    }
    e
}

/// The yaw the camera looks along: a follow camera's orbit yaw, else the
/// camera's forward direction.
fn view_yaw(world: &World, camera: Option<Uid>) -> f32 {
    let cam = camera.and_then(|u| world.entity_by_uid(u)).or_else(|| {
        world
            .query::<(Entity, &Camera)>()
            .iter()
            .find(|(_, c)| c.active)
            .map(|(e, _)| e)
    });
    let Some(cam) = cam else {
        return 0.0;
    };
    if let Some(s) = world.get::<FollowCameraState>(cam) {
        return s.yaw;
    }
    world
        .get::<GlobalTransform>(cam)
        .map_or(0.0, |g| yaw_of(g.0.transform_vector3(Vec3::NEG_Z)))
}
