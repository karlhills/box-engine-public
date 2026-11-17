//! Deterministic test worlds.

use box_core::world::World;
use glam::{Quat, Vec3};

use crate::components::*;
use crate::ops;

/// Small deterministic PRNG (SplitMix64) so fixtures need no extra dependency.
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

/// `n` entities with a random hierarchy (each entity's parent is an earlier one
/// or none) and a random mix of components.
pub fn build_world(n: usize, seed: u64) -> World {
    let mut rng = Rng(seed);
    let mut world = World::new();
    let mut uids = Vec::with_capacity(n);
    for i in 0..n {
        let uid = Uid::from_u128(((seed as u128) << 64) | (i as u128 + 1));
        let e = ops::spawn_with_uid(&mut world, uid, Some(&format!("Entity {i}")));
        let t = Transform {
            translation: Vec3::new(rng.f(-50.0, 50.0), rng.f(0.0, 10.0), rng.f(-50.0, 50.0)),
            rotation: Quat::from_rotation_y(rng.f(-3.0, 3.0)),
            scale: Vec3::splat(rng.f(0.5, 2.0)),
        };
        let _ = world.insert_one(e, t);
        if rng.below(4) == 0 {
            let _ = world.insert_one(
                e,
                PointLight {
                    color: [rng.f(0.0, 1.0), rng.f(0.0, 1.0), 1.0],
                    intensity: rng.f(0.0, 5000.0),
                    radius: 8.0,
                },
            );
        }
        if rng.below(3) == 0 {
            let _ = world.insert_one(
                e,
                Health {
                    current: rng.f(0.0, 100.0),
                    max: 100.0,
                },
            );
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
            let _ = world.insert_one(
                e,
                Npc {
                    behaviour,
                    tags: vec!["enemy".into(), format!("wave{}", rng.below(5))],
                    target: if i > 0 && rng.below(2) == 0 {
                        Some(uids[rng.below(i)])
                    } else {
                        None
                    },
                    loot_table: (rng.below(2) == 0).then(|| "goblin".to_string()),
                },
            );
        }
        if i > 0 && rng.below(3) != 0 {
            let parent = ops::entity(&world, uids[rng.below(i)]).unwrap();
            world.set_parent(e, Some(parent));
        }
        uids.push(uid);
    }
    world
}
