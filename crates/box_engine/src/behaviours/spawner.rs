//! [`Spawner`]: makes copies of a template entity at an interval, inside an
//! area, up to a limit. Copies are runtime-only (`Transient`: never saved,
//! removed when a save game loads); the template is hidden while playing
//! and left out of interactable groups.

use std::collections::HashMap;

use box_core::identity::Uid;
use box_core::reflect::{Range, Reflect, ReflectComponent, ReflectDefault, Step, Unit};
use box_core::scene::{instantiate, save_subtree};
use box_core::time::Time;
use box_core::transform::{Transform, compute_global};
use box_core::visibility::{Transient, Visible};
use box_core::world::{Entity, World};
use glam::Vec3;
use serde_json::Value as Json;

/// Makes copies of its template every `interval` seconds while fewer than
/// `max_alive` are around, at random spots within `radius` of itself. The
/// template is the entity in `template`, else the spawner's first child; it
/// is hidden while playing, so keep it where the spawner is. Copies are not
/// saved: a save game or Restart starts the spawner again.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Spawner {
    /// Off: no new copies (the ones already made stay).
    pub enabled: bool,
    /// The entity to copy. Empty: the spawner's first child.
    pub template: Option<Uid>,
    /// Time between two copies; the first comes after one interval.
    #[reflect(@Range { min: 0.05, max: 600.0 }, @Unit("s"), @Step(0.1))]
    pub interval: f32,
    /// At most this many copies at once.
    #[reflect(@Range { min: 1.0, max: 1000.0 }, @Step(1.0))]
    pub max_alive: u32,
    /// Stop after this many copies in all. 0: never stop.
    #[reflect(@Range { min: 0.0, max: 100000.0 }, @Step(1.0))]
    pub total: u32,
    /// Copies appear at random spots within this distance (on the ground
    /// plane around the spawner). 0: exactly at the template.
    #[reflect(@Range { min: 0.0, max: 500.0 }, @Unit("m"), @Step(0.1))]
    pub radius: f32,
    /// Each copy disappears after this long. 0: copies stay until
    /// something removes them (a `Despawn` action, `Health` at zero).
    #[reflect(@Range { min: 0.0, max: 3600.0 }, @Unit("s"), @Step(0.1))]
    pub lifetime: f32,
}

impl Default for Spawner {
    fn default() -> Self {
        Self {
            enabled: true,
            template: None,
            interval: 3.0,
            max_alive: 5,
            total: 0,
            radius: 0.0,
            lifetime: 0.0,
        }
    }
}

/// Runtime marker on a spawner's template (never saved).
#[derive(Clone, Copy, Debug)]
pub struct SpawnTemplate;

/// True when `e` or an ancestor is a spawner's template.
pub fn is_spawn_template(world: &World, e: Entity) -> bool {
    let mut cur = Some(e);
    for _ in 0..4096 {
        let Some(c) = cur else { return false };
        if world.has::<SpawnTemplate>(c) {
            return true;
        }
        cur = world.parent(c);
    }
    false
}

/// Runtime state of a [`Spawner`] (never saved).
#[derive(Clone, Debug, Default)]
pub struct SpawnerState {
    /// Seconds since the last copy.
    pub timer: f32,
    /// Copies made so far.
    pub spawned: u32,
    /// Live copies with the game time they expire at (`f64::INFINITY`
    /// without a lifetime).
    pub alive: Vec<(Entity, f64)>,
    /// The template's records, read once.
    records: Option<Vec<Json>>,
    template: Option<Entity>,
}

impl SpawnerState {
    /// Live copies.
    pub fn alive_count(&self) -> usize {
        self.alive.len()
    }
}

/// The `spawners` system (Update): ticks every enabled spawner, removes
/// expired copies and makes new ones. Per frame: one pass over the spawners;
/// a copy costs one instantiate of the template's records (read once).
pub fn run_spawners(world: &mut World) {
    let (dt, now) = world
        .get_resource::<Time>()
        .map_or((0.0, 0.0), |t| (t.delta, t.elapsed_f64));
    let spawners: Vec<(Entity, Spawner)> = world
        .query::<(Entity, &Spawner)>()
        .iter()
        .map(|(e, s)| (e, s.clone()))
        .collect();
    for (e, spawner) in spawners {
        let mut state = world
            .get::<SpawnerState>(e)
            .map(|s| (*s).clone())
            .unwrap_or_default();
        if state.template.is_none_or(|t| !world.contains(t)) {
            state.template = find_template(world, e, &spawner);
            state.records = None;
            if let Some(t) = state.template {
                hide_template(world, t);
            }
        }
        // Copies whose lifetime ran out go; copies removed elsewhere drop
        // out of the count.
        for (c, until) in &state.alive {
            if *until <= now && world.contains(*c) {
                world.despawn(*c);
            }
        }
        state.alive.retain(|(c, _)| world.contains(*c));
        let room = (state.alive.len() as u32) < spawner.max_alive
            && (spawner.total == 0 || state.spawned < spawner.total);
        if spawner.enabled && room {
            state.timer += dt;
            if state.timer >= spawner.interval {
                state.timer = 0.0;
                if let Some(copy) = spawn_copy(world, e, &spawner, &mut state) {
                    let until = if spawner.lifetime > 0.0 {
                        now + spawner.lifetime as f64
                    } else {
                        f64::INFINITY
                    };
                    state.alive.push((copy, until));
                    state.spawned += 1;
                }
            }
        } else if !room {
            state.timer = 0.0;
        }
        let _ = world.insert_one(e, state);
    }
}

fn find_template(world: &World, spawner: Entity, s: &Spawner) -> Option<Entity> {
    match s.template {
        Some(uid) => world.entity_by_uid(uid),
        None => world.children(spawner).into_iter().next(),
    }
}

fn hide_template(world: &mut World, t: Entity) {
    let _ = world.insert_one(t, SpawnTemplate);
    let _ = world.insert_one(t, Visible(false));
    #[cfg(feature = "physics")]
    let _ = world.insert_one(t, box_physics::Collidable(false));
}

/// A small deterministic hash (splitmix64), so playtests replay the same
/// spots.
fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

fn spawn_copy(
    world: &mut World,
    spawner: Entity,
    s: &Spawner,
    state: &mut SpawnerState,
) -> Option<Entity> {
    let template = state.template?;
    if state.records.is_none() {
        match save_subtree(world, template) {
            Ok(r) => state.records = Some(r),
            Err(err) => {
                tracing::warn!(target: "box_engine", "Spawner: cannot copy its template: {err}");
                return None;
            }
        }
    }
    let mut records = state.records.clone()?;
    // Fresh uids; the root becomes a scene root.
    let mut map: HashMap<String, String> = HashMap::new();
    for r in &records {
        if let Some(u) = r.get("uid").and_then(Json::as_str) {
            map.insert(u.to_string(), Uid::new().to_string());
        }
    }
    for (i, r) in records.iter_mut().enumerate() {
        let Some(obj) = r.as_object_mut() else {
            continue;
        };
        if let Some(u) = obj
            .get("uid")
            .and_then(Json::as_str)
            .and_then(|u| map.get(u))
        {
            obj.insert("uid".into(), Json::String(u.clone()));
        }
        if i == 0 {
            obj.insert("parent".into(), Json::Null);
        } else if let Some(p) = obj
            .get("parent")
            .and_then(Json::as_str)
            .and_then(|p| map.get(p))
        {
            obj.insert("parent".into(), Json::String(p.clone()));
        }
    }
    let spawned = match instantiate(world, &records, None) {
        Ok(s) => s,
        Err(err) => {
            tracing::warn!(target: "box_engine", "Spawner: cannot copy its template: {err}");
            return None;
        }
    };
    let root = *spawned.first()?;
    // Where the template is in the world, moved by a random offset.
    let global = compute_global(world, template);
    let (scale, rotation, translation) = global.to_scale_rotation_translation();
    let seed = world.get::<Uid>(spawner).map_or(0, |u| u.0.as_u64_pair().0)
        ^ (state.spawned as u64).wrapping_mul(0x2545_f491_4f6c_dd1d);
    let a = mix(seed);
    let b = mix(a);
    let angle = (a >> 11) as f32 / (1u64 << 53) as f32 * std::f32::consts::TAU;
    let dist = ((b >> 11) as f32 / (1u64 << 53) as f32).sqrt() * s.radius;
    let offset = Vec3::new(angle.cos() * dist, 0.0, angle.sin() * dist);
    let _ = world.insert_one(
        root,
        Transform {
            translation: translation + offset,
            rotation,
            scale,
        },
    );
    let _ = world.insert_one(root, Transient);
    let _ = world.insert_one(root, Visible(true));
    #[cfg(feature = "physics")]
    let _ = world.insert_one(root, box_physics::Collidable(true));
    Some(root)
}
