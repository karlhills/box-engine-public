use glam::{Quat, Vec3};
use serde::{Deserialize, Serialize};

#[derive(
    Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord,
)]
pub struct Uid(pub uuid::Uuid);

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Name(pub String);

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    /// Position relative to the parent, in metres.
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PointLight {
    pub color: [f32; 3],
    pub intensity: f32,
    pub radius: f32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Health {
    pub current: f32,
    pub max: f32,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub enum Behaviour {
    #[default]
    Idle,
    Patrol {
        speed: f32,
        waypoints: Vec<[f32; 3]>,
    },
    Follow(Option<Uid>),
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Npc {
    pub behaviour: Behaviour,
    pub tags: Vec<String>,
    pub target: Option<Uid>,
    pub loot_table: Option<String>,
}
