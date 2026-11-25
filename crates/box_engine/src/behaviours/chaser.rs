//! [`Chaser`]: a simple enemy. It waits or patrols, chases the player along
//! the navmesh when it sees them, hurts them on contact, searches where it
//! last saw them, and goes home when it gives up (docs/BEHAVIOURS.md, B2b;
//! `navigation` feature).

use box_core::animation::AnimationStateMachine;
use box_core::math::Ray;
use box_core::reflect::{Range, Reflect, ReflectComponent, ReflectDefault, Step, Unit};
use box_core::time::Time;
use box_core::transform::Transform;
use box_core::visibility::is_hidden;
use box_core::world::{Entity, World};
use box_physics::{KinematicCharacter, Physics, QueryFilter};
use glam::{Vec2, Vec3};

use super::rules::{GamePhase, GameState};
use crate::interact::find_interactor;
use crate::nav_agent::{NavAgent, finished_walk_to};

/// A simple enemy: waits (or walks its `patrol` points) until it sees the
/// player within `sight`, then chases them along the navmesh at
/// `chase_speed`, around walls and up stairs. Touching the player costs
/// them `damage` (their `Health`, or a respawn without one) at most once
/// per `attack_cooldown`. When it loses sight of the player it searches
/// where it saw them last; beyond `give_up` it walks back home. Uses a
/// `NavAgent` and a `KinematicCharacter` (added in Play when missing).
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Chaser {
    /// Off: it stops where it is.
    pub enabled: bool,
    /// Speed while patrolling, searching and going home.
    #[reflect(@Range { min: 0.0, max: 50.0 }, @Unit("m/s"), @Step(0.1))]
    pub speed: f32,
    /// Speed while chasing.
    #[reflect(@Range { min: 0.0, max: 50.0 }, @Unit("m/s"), @Step(0.1))]
    pub chase_speed: f32,
    /// How far it sees the player.
    #[reflect(@Range { min: 0.0, max: 200.0 }, @Unit("m"), @Step(0.5))]
    pub sight: f32,
    /// It gives up chasing beyond this distance.
    #[reflect(@Range { min: 0.0, max: 500.0 }, @Unit("m"), @Step(0.5))]
    pub give_up: f32,
    /// It must see the player (no wall in between) to start chasing.
    pub line_of_sight: bool,
    /// Points it walks between while it has not seen the player, as
    /// offsets from where it starts. Empty: it waits at home.
    #[reflect(@Unit("m"), @Step(0.1))]
    pub patrol: Vec<Vec3>,
    /// Seconds it waits at each patrol point and where it searched.
    #[reflect(@Range { min: 0.0, max: 60.0 }, @Unit("s"), @Step(0.1))]
    pub wait: f32,
    /// Health the player loses on contact.
    #[reflect(@Range { min: 0.0, max: 100.0 }, @Step(1.0))]
    pub damage: f32,
    /// Seconds between two hits.
    #[reflect(@Range { min: 0.0, max: 60.0 }, @Unit("s"), @Step(0.1))]
    pub attack_cooldown: f32,
    /// How close counts as touching (between the two capsules' edges).
    #[reflect(@Range { min: 0.0, max: 5.0 }, @Unit("m"), @Step(0.05))]
    pub reach: f32,
}

impl Default for Chaser {
    fn default() -> Self {
        Self {
            enabled: true,
            speed: 2.0,
            chase_speed: 4.0,
            sight: 8.0,
            give_up: 14.0,
            line_of_sight: true,
            patrol: Vec::new(),
            wait: 1.0,
            damage: 1.0,
            attack_cooldown: 1.0,
            reach: 0.3,
        }
    }
}

/// What a [`Chaser`] is doing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChaserMode {
    /// At home or walking its patrol.
    #[default]
    Patrol,
    /// Following the player.
    Chase,
    /// Walking to where it last saw the player, then looking around.
    Search,
    /// Walking back home.
    Return,
}

/// Runtime state of a [`Chaser`] (never saved; a loaded game starts it at
/// home again).
#[derive(Clone, Debug, Default)]
pub struct ChaserState {
    pub mode: ChaserMode,
    /// Where it started.
    pub home: Vec3,
    pub last_seen: Option<Vec3>,
    patrol_index: usize,
    /// Seconds left to wait (patrol points, searching).
    waiting: f32,
    /// Game time of its last hit.
    last_hit: f64,
    /// Hits it landed, for tests and scripts.
    pub hits: u32,
}

/// Adds a `NavAgent` when missing (Play only).
pub(crate) fn on_chaser_added(world: &mut World, e: Entity) {
    if !world.has::<NavAgent>(e) {
        let _ = world.insert_one(e, NavAgent::default());
    }
}

fn sees(world: &World, me: Entity, from: Vec3, player: Entity, to: Vec3, max: f32) -> bool {
    // Before the first collider sync nothing blocks a ray yet: seeing
    // through every wall on the first frame would start a chase.
    let Some(physics) = world.get_resource::<Physics>().filter(|p| !p.is_empty()) else {
        return false;
    };
    let d = to - from;
    let len = d.length();
    if len < 1e-3 {
        return true;
    }
    if len > max {
        return false;
    }
    let ray = Ray::new(from, d / len);
    match physics.raycast(&ray, len, &QueryFilter::default().excluding(me)) {
        None => true,
        Some(hit) => hit.entity == player || hit.toi >= len - 0.3,
    }
}

/// The `chasers` system (FixedUpdate, before `nav_agents`).
pub fn drive_chasers(world: &mut World) {
    let chasers: Vec<(Entity, Chaser)> = world
        .query::<(Entity, &Chaser)>()
        .iter()
        .map(|(e, c)| (e, c.clone()))
        .collect();
    if chasers.is_empty() {
        return;
    }
    let playing = world
        .get_resource::<GameState>()
        .is_none_or(|s| s.phase == GamePhase::Playing);
    let (dt, now) = world
        .get_resource::<Time>()
        .map_or((1.0 / 60.0, 0.0), |t| (t.delta, t.elapsed_f64));
    let player = find_interactor(world);
    let player_at = player.and_then(|p| {
        let t = world.get::<Transform>(p)?.translation;
        Some((p, t))
    });
    let player_radius = player
        .and_then(|p| world.get::<KinematicCharacter>(p).map(|k| k.radius))
        .unwrap_or(0.4);
    for (e, c) in chasers {
        // A spawner's hidden template, or a hidden enemy, does nothing.
        if super::is_spawn_template(world, e) || is_hidden(world, e) {
            continue;
        }
        let Some(at) = world.get::<Transform>(e).map(|t| t.translation) else {
            continue;
        };
        let mut state = match world.get::<ChaserState>(e) {
            Some(s) => (*s).clone(),
            None => ChaserState {
                home: at,
                ..Default::default()
            },
        };
        let mut destination = None;
        let mut speed = c.speed;
        if c.enabled && playing {
            let radius = world.get::<KinematicCharacter>(e).map_or(0.4, |k| k.radius);
            let eye = Vec3::Y * 0.6;
            let flat = |v: Vec3| Vec2::new(v.x, v.z);
            let seen = player_at.filter(|(p, pt)| {
                let d = flat(*pt).distance(flat(at));
                let limit = if state.mode == ChaserMode::Chase {
                    c.give_up
                } else {
                    c.sight
                };
                d <= limit
                    && (!c.line_of_sight || sees(world, e, at + eye, *p, *pt + eye, limit + 1.0))
            });
            match seen {
                Some((p, pt)) => {
                    state.mode = ChaserMode::Chase;
                    state.last_seen = Some(pt);
                    destination = Some(pt);
                    speed = c.chase_speed;
                    // Contact: the capsules' edges within reach.
                    let gap = flat(pt).distance(flat(at)) - radius - player_radius;
                    if gap <= c.reach
                        && (pt.y - at.y).abs() < 1.5
                        && now - state.last_hit >= c.attack_cooldown as f64
                    {
                        state.last_hit = now;
                        state.hits += 1;
                        if world.has::<super::Health>(p) {
                            super::damage(world, p, c.damage, Some(e));
                        } else if c.damage > 0.0 {
                            super::respawn(world, p);
                        }
                    }
                }
                None => {
                    if state.mode == ChaserMode::Chase {
                        state.mode = ChaserMode::Search;
                        state.waiting = c.wait.max(0.5) * 2.0;
                    }
                }
            }
            // Done with the walk this mode asks for (not an earlier one).
            let done = |world: &World, goal: Option<Vec3>| {
                goal.is_some_and(|g| finished_walk_to(world, e, g))
            };
            match state.mode {
                ChaserMode::Chase => {}
                ChaserMode::Search => {
                    destination = state.last_seen;
                    if done(world, destination) {
                        state.waiting -= dt;
                        if state.waiting <= 0.0 {
                            state.mode = ChaserMode::Return;
                        }
                    }
                }
                ChaserMode::Return => {
                    destination = Some(state.home);
                    if done(world, destination) {
                        state.mode = ChaserMode::Patrol;
                        state.patrol_index = 0;
                        state.waiting = c.wait;
                    }
                }
                ChaserMode::Patrol => {
                    if c.patrol.is_empty() {
                        destination = None;
                    } else {
                        let i = state.patrol_index % c.patrol.len();
                        destination = Some(state.home + c.patrol[i]);
                        if done(world, destination) {
                            state.waiting -= dt;
                            if state.waiting <= 0.0 {
                                state.patrol_index = (i + 1) % c.patrol.len();
                                state.waiting = c.wait;
                            }
                        }
                    }
                }
            }
        }
        if let Some(a) = world.component_mut::<NavAgent>(e) {
            a.destination = destination;
            a.speed = speed;
        }
        let moving = world
            .get::<KinematicCharacter>(e)
            .map_or(0.0, |k| Vec2::new(k.velocity.x, k.velocity.z).length());
        feed_animation(world, e, moving, state.mode == ChaserMode::Chase);
        let _ = world.insert_one(e, state);
    }
}

/// Sets `speed` and `chasing` on the first state machine in the enemy (its
/// model), when it has those parameters.
fn feed_animation(world: &mut World, e: Entity, speed: f32, chasing: bool) {
    let mut stack = vec![e];
    while let Some(c) = stack.pop() {
        if let Some(m) = world.component_mut::<AnimationStateMachine>(c) {
            m.set_float("speed", speed);
            m.set_bool("chasing", chasing);
            return;
        }
        stack.extend(world.children(c));
    }
}
