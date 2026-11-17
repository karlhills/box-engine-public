//! Game-like components and deterministic worlds shared by the integration tests.
#![allow(dead_code)]

use std::sync::Arc;

use box_core::glam::{Quat, Vec3};
use box_core::identity::Uid;
use box_core::reflect::{
    NotSaved, Range, Reflect, ReflectComponent, ReflectDefault, ReflectResource, Registry, Step,
    Unit,
};
use box_core::transform::Transform;
use box_core::world::World;

/// An omnidirectional light.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone)]
#[type_path = "game"]
pub struct PointLight {
    /// Linear RGB colour.
    pub color: [f32; 3],
    /// Luminous power.
    #[reflect(@Range { min: 0.0, max: 100_000.0 }, @Unit("lm"), @Step(10.0))]
    pub intensity: f32,
    /// Light falloff radius; beyond it the light contributes nothing.
    #[reflect(@Range { min: 0.0, max: 500.0 }, @Unit("m"))]
    pub radius: f32,
}

impl Default for PointLight {
    fn default() -> Self {
        Self {
            color: [1.0, 1.0, 1.0],
            intensity: 800.0,
            radius: 10.0,
        }
    }
}

#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone)]
#[type_path = "game"]
pub struct Health {
    pub current: f32,
    #[reflect(@Range { min: 1.0, max: 10_000.0 })]
    pub max: f32,
}

impl Default for Health {
    fn default() -> Self {
        Self {
            current: 100.0,
            max: 100.0,
        }
    }
}

#[derive(Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Default, Clone)]
#[type_path = "game"]
pub enum Behaviour {
    #[default]
    Idle,
    Patrol {
        speed: f32,
        waypoints: Vec<[f32; 3]>,
    },
    Follow(Option<Uid>),
}

/// Enum, Vec and Option fields.
#[derive(Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default, Clone)]
#[type_path = "game"]
pub struct Npc {
    pub behaviour: Behaviour,
    pub tags: Vec<String>,
    /// Cross-entity reference, stored by persistent id.
    pub target: Option<Uid>,
    pub loot_table: Option<String>,
}

/// Runtime-only state: registered but never saved.
#[derive(Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default, Clone, @NotSaved)]
#[type_path = "game"]
pub struct Cooldown(pub f32);

/// A scene resource, like the renderer's `Environment`.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Resource, Default, Clone)]
#[type_path = "game"]
pub struct Weather {
    /// Fog density.
    #[reflect(@Range { min: 0.0, max: 1.0 })]
    pub fog: f32,
    pub wind: Vec3,
}

impl Default for Weather {
    fn default() -> Self {
        Self {
            fog: 0.02,
            wind: Vec3::new(1.0, 0.0, 0.0),
        }
    }
}

pub fn registry() -> Arc<Registry> {
    let mut r = Registry::with_core_types();
    r.register::<PointLight>().unwrap();
    r.register::<Health>().unwrap();
    r.register::<Npc>().unwrap();
    r.register::<Cooldown>().unwrap();
    r.register_resource::<Weather>().unwrap();
    Arc::new(r)
}

pub fn uid(seed: u64, i: u128) -> Uid {
    Uid::from_u128(((seed as u128) << 64) | (i + 1))
}

/// Small deterministic PRNG (SplitMix64).
pub struct Rng(pub u64);

impl Rng {
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
    /// A float with a short decimal form, like values typed into an inspector.
    pub fn f(&mut self, lo: f32, hi: f32) -> f32 {
        let t = (self.next_u64() % 1001) as f32 / 1000.0;
        lo + (hi - lo) * t
    }
}

/// `n` saved entities with a random hierarchy (each parent is an earlier
/// entity or none) and a random mix of components, plus a `Weather` resource.
pub fn build_world(n: usize, seed: u64) -> World {
    let mut rng = Rng(seed);
    let mut world = World::with_registry(registry());
    world.insert_resource(Weather::default());
    let mut uids = Vec::with_capacity(n);
    for i in 0..n {
        let u = uid(seed, i as u128);
        let t = Transform {
            translation: Vec3::new(rng.f(-50.0, 50.0), rng.f(0.0, 10.0), rng.f(-50.0, 50.0)),
            rotation: Quat::from_rotation_y(rng.f(-3.0, 3.0)),
            scale: Vec3::splat(rng.f(0.5, 2.0)),
        };
        let e = world.spawn_saved(u, &format!("Entity {i}"), (t,)).unwrap();
        if rng.below(4) == 0 {
            let light = PointLight {
                color: [rng.f(0.0, 1.0), rng.f(0.0, 1.0), 1.0],
                intensity: rng.f(0.0, 5000.0),
                radius: 8.0,
            };
            world.insert_one(e, light).unwrap();
        }
        if rng.below(3) == 0 {
            let health = Health {
                current: rng.f(0.0, 100.0),
                max: 100.0,
            };
            world.insert_one(e, health).unwrap();
        }
        if rng.below(5) == 0 {
            let behaviour = match rng.below(3) {
                0 => Behaviour::Idle,
                1 => Behaviour::Patrol {
                    speed: rng.f(1.0, 5.0),
                    waypoints: vec![[0.0, 0.0, 0.0], [rng.f(0.0, 9.0), 0.0, 1.5]],
                },
                _ => Behaviour::Follow(uids.first().copied()),
            };
            let npc = Npc {
                behaviour,
                tags: vec!["enemy".into(), format!("wave{}", rng.below(5))],
                target: if i > 0 && rng.below(2) == 0 {
                    Some(uids[rng.below(i)])
                } else {
                    None
                },
                loot_table: (rng.below(2) == 0).then(|| "goblin".to_string()),
            };
            world.insert_one(e, npc).unwrap();
        }
        if rng.below(6) == 0 {
            world.insert_one(e, Cooldown(1.0)).unwrap();
        }
        if i > 0 && rng.below(3) != 0 {
            let parent = world.entity_by_uid(uids[rng.below(i)]).unwrap();
            world.set_parent(e, Some(parent)).unwrap();
        }
        uids.push(u);
    }
    world
}
