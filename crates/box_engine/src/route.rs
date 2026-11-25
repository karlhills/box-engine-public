//! Route checks: walk a probe character through waypoints against a world's
//! colliders and report, per leg, whether it got there and how hard the way
//! was (the biggest step up, drops, the steepest slope, where it got stuck).
//!
//! The probe is a [`KinematicCharacter`] moved with the engine's
//! `move_and_slide`, so it climbs, slides and falls exactly as a game's
//! character of the same size does; it never touches the world (no entity
//! is spawned) and needs no game code, so a scene file can be checked
//! without playing it (`box walk`, the MCP `walk_route` tool).

use box_core::prelude::*;
use box_physics::{KinematicCharacter, Physics, QueryFilter};
use serde::{Deserialize, Serialize};

/// Fixed step of the walk, seconds.
const DT: f32 = 1.0 / 60.0;
/// How far above a waypoint the ground is looked for first (so a waypoint
/// under a roof or a tree crown finds the ground, not the roof), metres.
const PROBE_ABOVE: f32 = 2.0;
/// How far a waypoint may float above the ground, metres.
const PROBE_BELOW: f32 = 50.0;

/// What to walk and with which character.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteSpec {
    /// Points to walk to, in order, near the ground (within 2 m above to
    /// 50 m below it). Only x and z steer; the walk starts on the ground at
    /// the first point.
    pub waypoints: Vec<[f32; 3]>,
    /// Walking speed, m/s.
    #[serde(default = "default_speed")]
    pub speed: f32,
    /// How close (horizontally) counts as having reached a waypoint, metres.
    #[serde(default = "default_reach")]
    pub reach: f32,
    /// Capsule radius (default: the scene's first `KinematicCharacter`,
    /// else the component's default).
    pub radius: Option<f32>,
    /// Capsule height.
    pub height: Option<f32>,
    /// Highest step climbed without jumping.
    pub step_height: Option<f32>,
    /// Steepest walkable slope, degrees.
    pub max_slope: Option<f32>,
}

fn default_speed() -> f32 {
    4.0
}

fn default_reach() -> f32 {
    0.5
}

/// One leg: from a waypoint (or where the last leg ended) to the next.
#[derive(Clone, Debug, Serialize)]
pub struct LegReport {
    /// Index of the waypoint this leg walks to.
    pub to: usize,
    pub reached: bool,
    /// Seconds walked.
    pub time: f32,
    /// Where the probe stopped making progress, when it did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stuck_at: Option<[f32; 3]>,
    /// Highest rise of the ground under the probe between two steps
    /// (6–7 cm apart at 4 m/s), metres: a kerb or a stair shows its height,
    /// a slope only a few centimetres.
    pub max_step_up: f32,
    /// Times the probe dropped further than its step height.
    pub falls: u32,
    /// The largest such drop, metres.
    pub max_drop: f32,
    /// The steepest ground walked on (the surface under the probe's
    /// centre), degrees.
    pub steepest_slope: f32,
    /// Where the leg ended (the probe's feet).
    pub end: [f32; 3],
}

/// The whole walk.
#[derive(Clone, Debug, Serialize)]
pub struct RouteReport {
    /// Every leg reached its waypoint.
    pub reached_all: bool,
    /// Where the walk started (feet on the ground).
    pub start: [f32; 3],
    pub legs: Vec<LegReport>,
    /// The probe character used (radius, height, step height, max slope
    /// in degrees).
    pub character: [f32; 4],
}

/// The probe character: the world's first `KinematicCharacter` (the
/// player), overridden by `spec`, and that entity (excluded from the
/// probe's collisions).
fn probe(world: &World, spec: &RouteSpec) -> (KinematicCharacter, Option<Entity>) {
    let found = world
        .query::<(Entity, &KinematicCharacter)>()
        .iter()
        .next()
        .map(|(e, c)| (c.clone(), e));
    let (mut c, entity) = match found {
        Some((c, e)) => (c, Some(e)),
        None => (KinematicCharacter::default(), None),
    };
    if let Some(r) = spec.radius {
        c.radius = r;
    }
    if let Some(h) = spec.height {
        c.height = h;
    }
    if let Some(s) = spec.step_height {
        c.step_height = s;
    }
    if let Some(m) = spec.max_slope {
        c.max_slope = m.to_radians();
    }
    c.velocity = Vec3::ZERO;
    c.grounded = false;
    c.ground_entity = None;
    c.ground_pose = None;
    (c, entity)
}

/// The capsule centre standing on the ground at `point`: the first
/// surface below a point just above it, if there is one.
fn stand_on(physics: &Physics, c: &KinematicCharacter, point: Vec3) -> Option<Vec3> {
    let from = point + Vec3::Y * PROBE_ABOVE;
    let hit = physics.ray(from, Vec3::NEG_Y, PROBE_ABOVE + PROBE_BELOW)?;
    Some(hit.point + Vec3::Y * (c.half_height() + c.skin))
}

/// The height and normal of the surface under the probe's centre, within
/// its step height below its feet.
fn ground_below(
    physics: &Physics,
    c: &KinematicCharacter,
    me: Entity,
    centre: Vec3,
) -> Option<(f32, Vec3)> {
    let reach = c.half_height() + c.skin + c.step_height + 0.1;
    let hit = physics.raycast(
        &Ray::new(centre, Vec3::NEG_Y),
        reach,
        &QueryFilter::new().excluding(me),
    )?;
    Some((hit.point.y, hit.normal))
}

/// Walk `spec` through the colliders of `world` (its `Physics` resource,
/// as of the last `sync_physics`).
pub fn walk_route(world: &World, spec: &RouteSpec) -> Result<RouteReport, String> {
    if spec.waypoints.len() < 2 {
        return Err("a route needs at least two waypoints".into());
    }
    if !(spec.speed > 0.0 && spec.speed <= 50.0) {
        return Err(format!("speed {} is outside 0..50 m/s", spec.speed));
    }
    if spec.reach.is_nan() || spec.reach <= 0.0 {
        return Err("reach must be positive".into());
    }
    let physics = world
        .get_resource::<Physics>()
        .ok_or("the world has no physics (no colliders to walk on)")?;
    let (mut c, player) = probe(world, spec);
    let me = player.unwrap_or(Entity::DANGLING);
    let points: Vec<Vec3> = spec
        .waypoints
        .iter()
        .map(|p| Vec3::from_array(*p))
        .collect();
    let feet = |c: &KinematicCharacter, p: Vec3| p - Vec3::Y * (c.half_height() + c.skin);
    let mut pos = stand_on(physics, &c, points[0])
        .ok_or_else(|| {
            format!(
                "no collider below the first waypoint {:?}: give the ground a Collider \
                 (Primitive ground: {{\"shape\": \"Primitive\"}}; generated or imported ground: \"Mesh\")",
                spec.waypoints[0]
            )
        })?;
    // Settle onto the ground.
    for _ in 0..10 {
        c.velocity.x = 0.0;
        c.velocity.z = 0.0;
        pos = c.step(physics, me, pos, DT);
    }
    let start = feet(&c, pos).to_array();
    let lowest = points.iter().map(|p| p.y).fold(f32::INFINITY, f32::min) - 100.0;
    let mut legs = Vec::new();
    for (i, target) in points.iter().enumerate().skip(1) {
        let flat = |p: Vec3| Vec2::new(p.x, p.z);
        let distance = flat(*target).distance(flat(pos));
        let limit = distance / spec.speed * 3.0 + 3.0;
        let mut leg = LegReport {
            to: i,
            reached: false,
            time: 0.0,
            stuck_at: None,
            max_step_up: 0.0,
            falls: 0,
            max_drop: 0.0,
            steepest_slope: 0.0,
            end: [0.0; 3],
        };
        let mut best = distance;
        let mut since_progress = 0.0;
        // The ground height the probe last stood on before leaving it.
        let mut airborne_from: Option<f32> = None;
        // The ground under the probe's centre at the previous step.
        let mut last_ground = ground_below(physics, &c, me, pos).map(|g| g.0);
        let mut stood_on = last_ground;
        let mut moving = true;
        let mut settle = 0.0;
        loop {
            if moving {
                let to = flat(*target) - flat(pos);
                if to.length() <= spec.reach {
                    leg.reached = true;
                    moving = false;
                } else if leg.time >= limit {
                    break;
                }
            }
            if !moving {
                // Reached: stop and land, so a drop at the end counts.
                if c.grounded || settle > 3.0 {
                    break;
                }
                settle += DT;
            }
            let dir = if moving {
                (flat(*target) - flat(pos)).normalize_or_zero()
            } else {
                Vec2::ZERO
            };
            c.velocity.x = dir.x * spec.speed;
            c.velocity.z = dir.y * spec.speed;
            pos = c.step(physics, me, pos, DT);
            if moving {
                leg.time += DT;
            }
            if c.grounded {
                // Steps, drops and slopes from the ground under the centre
                // (a capsule rides over an edge in several frames, touching
                // it at a steep angle, and snaps part of the way down a
                // drop, so its own contacts mislead).
                if let Some((y, normal)) = ground_below(physics, &c, me, pos) {
                    if let Some(prev) = last_ground {
                        leg.max_step_up = leg.max_step_up.max(y - prev);
                    }
                    let slope = normal.y.clamp(-1.0, 1.0).acos().to_degrees();
                    leg.steepest_slope = leg.steepest_slope.max(slope);
                    if let Some(top) = airborne_from.take() {
                        let drop = top - y;
                        if drop > c.step_height {
                            leg.falls += 1;
                            leg.max_drop = leg.max_drop.max(drop);
                        }
                    }
                    last_ground = Some(y);
                    stood_on = Some(y);
                }
            } else {
                last_ground = None;
                if airborne_from.is_none() {
                    airborne_from = stood_on;
                }
            }
            if pos.y < lowest {
                leg.reached = false;
                leg.stuck_at = Some(feet(&c, pos).to_array());
                break;
            }
            if moving {
                let d = flat(*target).distance(flat(pos));
                if d < best - 0.05 {
                    best = d;
                    since_progress = 0.0;
                } else {
                    since_progress += DT;
                    if since_progress > 1.5 {
                        leg.stuck_at = Some(feet(&c, pos).to_array());
                        break;
                    }
                }
            }
        }
        if !leg.reached && leg.stuck_at.is_none() {
            leg.stuck_at = Some(feet(&c, pos).to_array());
        }
        leg.end = feet(&c, pos).to_array();
        let reached = leg.reached;
        legs.push(leg);
        if !reached {
            // Check the next legs on their own: continue from this
            // waypoint, on its ground.
            match stand_on(physics, &c, *target) {
                Some(p) => {
                    pos = p;
                    c.velocity = Vec3::ZERO;
                    c.grounded = false;
                    for _ in 0..10 {
                        pos = c.step(physics, me, pos, DT);
                    }
                }
                None => break,
            }
        }
    }
    let reached_all = legs.len() == points.len() - 1 && legs.iter().all(|l| l.reached);
    Ok(RouteReport {
        reached_all,
        start,
        legs,
        character: [c.radius, c.height, c.step_height, c.max_slope.to_degrees()],
    })
}
