//! [`NavAgent`]: walks an entity's `KinematicCharacter` to a destination
//! along the navmesh (docs/NAVIGATION.md, N2; `navigation` feature).
//!
//! Each fixed step every agent with a destination steers toward the next
//! corner of its path (one vector operation) and is pushed apart from
//! agents closer than a metre. Paths are planned again only when the
//! destination moved more than [`REPLAN_DISTANCE`], the navmesh changed, or
//! the agent made no progress for [`STUCK_TIME`]; at most
//! [`REPLANS_PER_STEP`] agents replan in one step (the others keep their
//! old path a step longer), so many agents never plan in the same frame.

use box_core::reflect::{NotSaved, Range, Reflect, ReflectComponent, ReflectDefault, Step, Unit};
use box_core::time::Time;
use box_core::transform::Transform;
use box_core::world::{Entity, World};
use box_nav::PathStatus;
use box_physics::KinematicCharacter;
use glam::{Quat, Vec2, Vec3};

use crate::behaviours::view::{turn_towards, yaw_of};
use crate::nav_glue::Navigation;

/// A destination further than this from the planned one replans.
pub const REPLAN_DISTANCE: f32 = 1.0;
/// Seconds without progress before an agent replans.
pub const STUCK_TIME: f32 = 0.5;
/// Most agents planning a path in one fixed step.
pub const REPLANS_PER_STEP: usize = 8;
/// Agents closer than this push each other apart.
pub const SEPARATION_DISTANCE: f32 = 1.0;

/// Walks the entity's `KinematicCharacter` to `destination` along the
/// navmesh, around walls and up ramps and stairs, turning the entity to face
/// where it goes. Set `destination` from a script (`e:move_to(point)`), an
/// action (`SetField`) or another behaviour (a `Chaser`); clear it to stop.
/// A `KinematicCharacter` is added in Play when missing.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct NavAgent {
    /// Off: stands still.
    pub enabled: bool,
    /// Walking speed.
    #[reflect(@Range { min: 0.0, max: 50.0 }, @Unit("m/s"), @Step(0.1))]
    pub speed: f32,
    /// How fast it turns to face where it walks.
    #[reflect(@Range { min: 0.0, max: 50.0 }, @Unit("rad/s"), @Step(0.5))]
    pub turn_rate: f32,
    /// Closer than this to the destination counts as arrived.
    #[reflect(@Range { min: 0.05, max: 10.0 }, @Unit("m"), @Step(0.05))]
    pub arrive_distance: f32,
    /// Where to go (feet height is found on the navmesh). None: stand
    /// still. Not saved in scenes.
    #[reflect(@NotSaved)]
    pub destination: Option<Vec3>,
}

impl Default for NavAgent {
    fn default() -> Self {
        Self {
            enabled: true,
            speed: 3.5,
            turn_rate: 10.0,
            arrive_distance: 0.5,
            destination: None,
        }
    }
}

/// Where an agent stands with its destination.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NavStatus {
    /// No destination.
    #[default]
    Idle,
    /// The navmesh is not ready: walking straight at the destination.
    Baking,
    /// Walking a path.
    Moving,
    /// At the destination.
    Arrived,
    /// The destination cannot be reached: walking to (or standing at) the
    /// nearest reachable point.
    Unreachable,
    /// Not on the navmesh (in the air, off the edge): walking straight.
    OffMesh,
}

impl NavStatus {
    /// Lower-case name, as scripts see it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Baking => "baking",
            Self::Moving => "moving",
            Self::Arrived => "arrived",
            Self::Unreachable => "unreachable",
            Self::OffMesh => "off_mesh",
        }
    }
}

/// Runtime state of a [`NavAgent`] (never saved).
#[derive(Clone, Debug, Default)]
pub struct NavAgentState {
    pub status: NavStatus,
    /// The path's corners still ahead (the next first).
    pub corners: Vec<Vec3>,
    /// The destination the path was planned for.
    planned_for: Option<Vec3>,
    /// The navmesh revision it was planned on.
    revision: u64,
    /// Whether the last plan reached the destination.
    complete: bool,
    /// Time without getting closer to the next corner.
    stuck: f32,
    /// Distance to the next corner when last measured.
    best: f32,
    facing: Option<f32>,
}

fn feet(world: &World, e: Entity) -> Option<Vec3> {
    let t = world.get::<Transform>(e)?.translation;
    let k = world.get::<KinematicCharacter>(e)?;
    Some(t - Vec3::Y * (k.half_height() + k.skin))
}

/// Adds a `KinematicCharacter` when missing (Play only).
pub(crate) fn on_agent_added(world: &mut World, e: Entity) {
    if !world.has::<KinematicCharacter>(e) {
        let _ = world.insert_one(e, KinematicCharacter::default());
    }
    if !world.has::<box_physics::Collider>(e) {
        let k = world
            .get::<KinematicCharacter>(e)
            .map(|k| (*k).clone())
            .unwrap_or_default();
        let half = (k.height * 0.5 - k.radius).max(0.0);
        let _ = world.insert_one(e, box_physics::Collider::capsule(k.radius, half));
    }
}

/// The `nav_agents` system (FixedUpdate, before `move_characters`).
pub fn drive_nav_agents(world: &mut World) {
    let agents: Vec<(Entity, NavAgent)> = world
        .query::<(Entity, &NavAgent)>()
        .iter()
        .map(|(e, a)| (e, a.clone()))
        .collect();
    if agents.is_empty() {
        return;
    }
    let dt = world.get_resource::<Time>().map_or(1.0 / 60.0, |t| t.delta);
    let (revision, has_mesh, baking) = world
        .get_resource::<Navigation>()
        .map_or((0, false, true), |n| {
            (n.mesh().revision(), !n.mesh().is_empty(), n.is_baking())
        });
    // Positions, for separation.
    let positions: Vec<(Entity, Vec3)> = agents
        .iter()
        .filter_map(|(e, _)| Some((*e, feet(world, *e)?)))
        .collect();
    let mut replans = 0;
    for (e, agent) in &agents {
        let e = *e;
        // A spawner's hidden template does not walk.
        if crate::behaviours::is_spawn_template(world, e) {
            continue;
        }
        let Some(at) = positions.iter().find(|(p, _)| *p == e).map(|(_, p)| *p) else {
            continue;
        };
        let mut state = world
            .get::<NavAgentState>(e)
            .map(|s| (*s).clone())
            .unwrap_or_default();
        let Some(goal) = agent.destination.filter(|_| agent.enabled) else {
            state.status = NavStatus::Idle;
            state.corners.clear();
            state.planned_for = None;
            set_velocity(world, e, Vec3::ZERO);
            let _ = world.insert_one(e, state);
            continue;
        };
        let flat = |v: Vec3| Vec2::new(v.x, v.z);
        // Replan when needed and allowed this step.
        let stale = state
            .planned_for
            .is_none_or(|p| flat(p).distance(flat(goal)) > REPLAN_DISTANCE)
            || (has_mesh && state.revision != revision)
            || state.stuck > STUCK_TIME;
        if stale && has_mesh && replans < REPLANS_PER_STEP {
            replans += 1;
            if let Some(nav) = world.get_resource::<Navigation>() {
                let path = nav.mesh().find_path(at, goal);
                state.revision = revision;
                state.planned_for = Some(goal);
                state.stuck = 0.0;
                state.best = f32::INFINITY;
                match path.status {
                    PathStatus::NoStart => {
                        state.corners = vec![goal];
                        state.complete = true;
                        state.status = NavStatus::OffMesh;
                    }
                    s => {
                        state.corners = path.points.into_iter().skip(1).collect();
                        state.complete = s == PathStatus::Complete;
                        state.status = NavStatus::Moving;
                    }
                }
            }
        } else if !has_mesh {
            state.corners = vec![goal];
            state.planned_for = Some(goal);
            state.complete = true;
            state.status = if baking {
                NavStatus::Baking
            } else {
                NavStatus::OffMesh
            };
        }
        // Drop corners already reached.
        while state.corners.len() > 1 && flat(state.corners[0]).distance(flat(at)) < 0.25 {
            state.corners.remove(0);
            state.best = f32::INFINITY;
        }
        let last = state.corners.len() <= 1;
        let target = state.corners.first().copied().unwrap_or(goal);
        let to_target = flat(target) - flat(at);
        let end_distance = if last {
            to_target.length()
        } else {
            f32::INFINITY
        };
        let mut velocity = Vec3::ZERO;
        if last && end_distance <= agent.arrive_distance {
            state.status = if state.complete {
                NavStatus::Arrived
            } else {
                NavStatus::Unreachable
            };
            state.stuck = 0.0;
        } else if to_target.length() > 1e-4 {
            let d = to_target.normalize();
            // Slow down over the last metre.
            let speed = if last {
                agent.speed * end_distance.clamp(0.3, 1.0)
            } else {
                agent.speed
            };
            velocity = Vec3::new(d.x, 0.0, d.y) * speed;
            // Progress toward the corner.
            let dist = to_target.length();
            if dist < state.best - 0.02 {
                state.best = dist;
                state.stuck = 0.0;
            } else {
                state.stuck += dt;
            }
        }
        // Push apart from nearby agents.
        let mut push = Vec2::ZERO;
        for (other, p) in &positions {
            if *other == e {
                continue;
            }
            let away = flat(at) - flat(*p);
            let d = away.length();
            if d < SEPARATION_DISTANCE && d > 1e-4 && (at.y - p.y).abs() < 1.5 {
                push += away / d * (SEPARATION_DISTANCE - d);
            }
        }
        if velocity != Vec3::ZERO || push != Vec2::ZERO {
            velocity += Vec3::new(push.x, 0.0, push.y) * agent.speed;
            let flat_v = Vec2::new(velocity.x, velocity.z);
            if flat_v.length() > agent.speed && agent.speed > 0.0 {
                let v = flat_v.normalize() * agent.speed;
                velocity = Vec3::new(v.x, 0.0, v.y);
            }
        }
        set_velocity(world, e, velocity);
        // Face where it walks.
        if velocity.length_squared() > 0.01 {
            let from = state.facing.unwrap_or_else(|| {
                world
                    .get::<Transform>(e)
                    .map_or(0.0, |t| yaw_of(t.rotation * Vec3::NEG_Z))
            });
            let yaw = turn_towards(from, yaw_of(velocity), agent.turn_rate * dt);
            state.facing = Some(yaw);
            if let Some(t) = world.component_mut::<Transform>(e) {
                t.rotation = Quat::from_rotation_y(yaw);
            }
        }
        let _ = world.insert_one(e, state);
    }
}

fn set_velocity(world: &mut World, e: Entity, v: Vec3) {
    if let Some(k) = world.component_mut::<KinematicCharacter>(e) {
        k.velocity.x = v.x;
        k.velocity.z = v.z;
    }
}

impl NavAgentState {
    /// The destination the current path was planned for.
    pub fn planned_for(&self) -> Option<Vec3> {
        self.planned_for
    }
}

/// Whether the agent's walk to `goal` is over: it arrived there, or found
/// it cannot get closer. False while its status still describes an earlier
/// destination.
pub fn finished_walk_to(world: &World, e: Entity, goal: Vec3) -> bool {
    world.get::<NavAgentState>(e).is_some_and(|s| {
        matches!(s.status, NavStatus::Arrived | NavStatus::Unreachable)
            && s.planned_for
                .is_some_and(|p| Vec2::new(p.x - goal.x, p.z - goal.z).length() < REPLAN_DISTANCE)
    })
}

/// The agent's status.
pub fn nav_status(world: &World, e: Entity) -> NavStatus {
    world
        .get::<NavAgentState>(e)
        .map_or(NavStatus::Idle, |s| s.status)
}
