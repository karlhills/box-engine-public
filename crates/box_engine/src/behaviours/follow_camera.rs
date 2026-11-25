//! [`FollowCamera`]: a third-person camera that follows the player.

use box_core::action::Actions;
use box_core::identity::Uid;
use box_core::input::{CursorOptions, Input, KeyCode, MouseButton};
use box_core::interpolate::interpolated_transform;
use box_core::reflect::{Range, Reflect, ReflectComponent, ReflectDefault, Step, Unit};
use box_core::time::Time;
use box_core::transform::{GlobalTransform, Transform};
use box_core::world::{Entity, World};
use box_render::Camera;
use box_ui::Ui;
use glam::{Quat, Vec3};

use super::rules::{GamePhase, GameState};
use super::view::yaw_of;

/// How a [`FollowCamera`] looks at its target.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub enum FollowMode {
    /// Orbits the target with the `look` action (right stick) or the mouse
    /// (click the game to capture it, Escape to let go), and pulls in when
    /// a wall is in the way.
    #[default]
    Orbit,
    /// Keeps the angle it starts with (top-down and side-on games): the
    /// camera's saved rotation sets the direction, `pitch` the angle down.
    Fixed,
}

/// A camera that follows a target from behind and above. Put it on the
/// entity with the `Camera`. In Play it becomes the active camera (other
/// cameras are switched off), so the scene's starting camera need not be
/// removed.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct FollowCamera {
    /// Off: the camera stays where it is.
    pub enabled: bool,
    /// What to follow. None: the player (`PlayerController`).
    pub target: Option<Uid>,
    /// Orbit with the mouse and stick, or a fixed angle.
    pub mode: FollowMode,
    /// Distance from the point it looks at.
    #[reflect(@Range { min: 0.5, max: 200.0 }, @Unit("m"), @Step(0.1))]
    pub distance: f32,
    /// Height of that point above the target's origin.
    #[reflect(@Range { min: -10.0, max: 50.0 }, @Unit("m"), @Step(0.05))]
    pub height: f32,
    /// Angle down toward the target (the starting angle in `Orbit`).
    #[reflect(@Range { min: -0.5, max: 1.55 }, @Unit("rad"), @Step(0.01))]
    pub pitch: f32,
    /// How quickly it catches up (higher: stiffer).
    #[reflect(@Range { min: 0.5, max: 50.0 }, @Unit("1/s"), @Step(0.5))]
    pub smoothing: f32,
    /// Turn rate at full stick.
    #[reflect(@Range { min: 0.0, max: 10.0 }, @Unit("rad/s"), @Step(0.1))]
    pub look_speed: f32,
    /// Turn per unit of mouse movement while the mouse is captured.
    #[reflect(@Range { min: 0.0, max: 0.05 }, @Unit("rad"), @Step(0.0005))]
    pub mouse_sensitivity: f32,
    /// Clicking the game captures the mouse for looking (Escape releases
    /// it). Off: only the stick turns the camera.
    pub capture_mouse: bool,
}

impl Default for FollowCamera {
    fn default() -> Self {
        Self {
            enabled: true,
            target: None,
            mode: FollowMode::Orbit,
            distance: 7.0,
            height: 1.2,
            pitch: 0.35,
            smoothing: 10.0,
            look_speed: 2.5,
            mouse_sensitivity: 0.004,
            capture_mouse: true,
        }
    }
}

/// Runtime orbit of a follow camera. The player reads `yaw` to move
/// relative to the view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FollowCameraState {
    pub yaw: f32,
    pub pitch: f32,
    fresh: bool,
}

/// Update, after the players: orbit input, follow, wall pull-in.
pub(crate) fn follow_cameras(world: &mut World) {
    let cams: Vec<(Entity, FollowCamera)> = world
        .query::<(Entity, &FollowCamera)>()
        .iter()
        .filter(|(_, c)| c.enabled)
        .map(|(e, c)| (e, c.clone()))
        .collect();
    if cams.is_empty() {
        return;
    }
    let fresh_actions = super::ensure_game_actions(world);
    let dt = world.get_resource::<Time>().map_or(0.0, |t| t.delta);
    let playing = world
        .get_resource::<GameState>()
        .is_none_or(|s| s.phase == GamePhase::Playing);
    let ui_captures = world.get_resource::<Ui>().is_some_and(Ui::captures_input);
    for (e, cam) in cams {
        if !world.has::<FollowCameraState>(e) {
            let rot = world
                .get::<Transform>(e)
                .map_or(Quat::IDENTITY, |t| t.rotation);
            let _ = world.insert_one(
                e,
                FollowCameraState {
                    yaw: yaw_of(rot * Vec3::NEG_Z),
                    pitch: cam.pitch,
                    fresh: true,
                },
            );
            take_over(world, e);
        }
        let Some(target) = target_of(world, &cam) else {
            continue;
        };
        let orbit = cam.mode == FollowMode::Orbit;
        if orbit && cam.capture_mouse && playing && !ui_captures {
            let (click, escape) = world.get_resource::<Input>().map_or((false, false), |i| {
                (
                    i.button_pressed(MouseButton::Left),
                    i.key_pressed(KeyCode::Escape),
                )
            });
            if let Some(c) = world.get_resource_mut::<CursorOptions>() {
                if click && !c.is_locked() {
                    c.lock();
                } else if escape && c.is_locked() {
                    c.release();
                }
            }
        }
        let locked = world
            .get_resource::<CursorOptions>()
            .is_some_and(CursorOptions::is_locked);
        let stick = match world.get_resource::<Actions>() {
            Some(a) if !fresh_actions => a.axis2("look"),
            _ => glam::Vec2::ZERO,
        };
        let mouse = match world.get_resource::<Input>() {
            Some(i) if locked => i.mouse_delta,
            _ => glam::Vec2::ZERO,
        };
        let (yaw, pitch, fresh) = {
            let Some(s) = world.component_mut::<FollowCameraState>(e) else {
                continue;
            };
            if orbit && playing {
                s.yaw -= stick.x * cam.look_speed * dt + mouse.x * cam.mouse_sensitivity;
                s.pitch = (s.pitch - stick.y * cam.look_speed * dt
                    + mouse.y * cam.mouse_sensitivity)
                    .clamp(-0.3, 1.3);
            } else if !orbit {
                s.pitch = cam.pitch;
            }
            let fresh = std::mem::take(&mut s.fresh);
            (s.yaw, s.pitch, fresh)
        };
        // Follow where the target is drawn this frame, so the camera is as
        // smooth as it is.
        let at = interpolated_transform(world, target)
            .map(|t| t.translation)
            .or_else(|| {
                world
                    .get::<GlobalTransform>(target)
                    .map(|g| g.0.translation.into())
            })
            .unwrap_or(Vec3::ZERO);
        let focus = at + Vec3::Y * cam.height;
        let rotation = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(-pitch);
        let back = rotation * Vec3::Z;
        let distance = if orbit {
            pulled_in(world, focus, back, cam.distance, target)
        } else {
            cam.distance
        };
        let desired = focus + back * distance;
        let ease = if fresh {
            1.0
        } else {
            1.0 - (-cam.smoothing * dt).exp()
        };
        if let Some(t) = world.component_mut::<Transform>(e) {
            let pos = t.translation + (desired - t.translation) * ease;
            t.translation = pos;
            let look = focus - pos;
            if look.length_squared() > 1e-8 {
                t.rotation = look_rotation(look);
            }
        }
    }
}

/// Makes `e`'s `Camera` the active one: a follow camera is what the game
/// is seen through, even when the scene still has its starting camera
/// (placing the Player preset adds one beside it). Play world only.
fn take_over(world: &mut World, e: Entity) {
    if !world.has::<Camera>(e) {
        return;
    }
    let others: Vec<Entity> = world
        .query::<(Entity, &Camera)>()
        .iter()
        .filter(|(o, c)| *o != e && c.active)
        .map(|(o, _)| o)
        .collect();
    for o in others {
        if let Some(c) = world.component_mut::<Camera>(o) {
            c.active = false;
        }
    }
    if let Some(c) = world.component_mut::<Camera>(e)
        && !c.active
    {
        c.active = true;
    }
}

/// The distance the camera can be from `focus` along `back` before a wall.
#[cfg(feature = "physics")]
fn pulled_in(world: &World, focus: Vec3, back: Vec3, distance: f32, target: Entity) -> f32 {
    let Some(physics) = world.get_resource::<box_physics::Physics>() else {
        return distance;
    };
    let ray = box_core::math::Ray::new(focus, back);
    let filter = box_physics::QueryFilter::default().excluding(target);
    match physics.raycast(&ray, distance, &filter) {
        Some(hit) => (hit.toi - 0.3).max(0.5),
        None => distance,
    }
}

#[cfg(not(feature = "physics"))]
fn pulled_in(_: &World, _: Vec3, _: Vec3, distance: f32, _: Entity) -> f32 {
    distance
}

/// The rotation that looks along `dir` with +Y up (−Z forward).
fn look_rotation(dir: Vec3) -> Quat {
    let f = dir.normalize();
    let yaw = yaw_of(f);
    let pitch = f.y.clamp(-1.0, 1.0).asin();
    Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch)
}

fn target_of(world: &World, cam: &FollowCamera) -> Option<Entity> {
    if let Some(e) = cam.target.and_then(|u| world.entity_by_uid(u)) {
        return Some(e);
    }
    crate::interact::find_interactor(world)
}
