//! Components that resemble the real ones. Every one is plain data plus
//! `#[derive(Reflect)]`; nothing below this file has per-type code.

use bevy_reflect::Reflect;
use bevy_reflect::std_traits::ReflectDefault;
use glam::{Quat, Vec3};

use crate::math::{QuatDef, Vec3Def};
use crate::meta::{Range, Step, Unit};
use crate::registry::ReflectComponent;

/// Persistent identity. Serialized as the entity key, never as a component.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[reflect(Component, Default, Clone)]
#[type_path = "box_engine"]
pub struct Uid(pub uuid::Uuid);

impl Uid {
    pub fn from_u128(v: u128) -> Self {
        Self(uuid::Uuid::from_u128(v))
    }
}

/// Display name, shown in the hierarchy.
#[derive(Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default, Clone)]
#[type_path = "box_engine"]
pub struct Name(pub String);

/// Local transform. glam 0.34 types are reflected through remote wrappers because
/// bevy_reflect 0.20's `glam` feature is built against glam 0.33.
#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(Component, Default, Clone)]
#[type_path = "box_engine"]
pub struct Transform {
    /// Position relative to the parent, in metres.
    #[reflect(remote = Vec3Def, @Unit("m"))]
    pub translation: Vec3,
    /// Orientation relative to the parent.
    #[reflect(remote = QuatDef)]
    pub rotation: Quat,
    #[reflect(remote = Vec3Def, @Range { min: 0.001, max: 1000.0 })]
    pub scale: Vec3,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            translation: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        }
    }
}

/// An omnidirectional light.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone)]
#[type_path = "box_engine"]
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

/// A game-side component: nothing engine-specific about it.
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
