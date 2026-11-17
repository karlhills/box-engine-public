//! Reflection for glam 0.34 math types through `#[reflect_remote]`.
//!
//! bevy_reflect's own `glam` feature implements `Reflect` for glam 0.33 (its
//! dependency), which is a different crate from the glam 0.34 box_core uses, and
//! the orphan rule forbids implementing `Reflect` for `glam::Vec3` here. Remote
//! wrappers are the supported escape hatch: fields of these types must carry
//! `#[reflect(remote = Vec3Def)]`.
//!
//! The wrappers also carry hand-written serde impls so vectors are written as
//! `[x, y, z]` arrays (as in the scene format sketch) instead of `{x, y, z}` maps.

use bevy_reflect::std_traits::ReflectDefault;
use bevy_reflect::{ReflectDeserialize, ReflectSerialize, reflect_remote};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[reflect_remote(glam::Vec3)]
#[reflect(Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Vec3Def {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[reflect_remote(glam::Quat)]
#[reflect(Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuatDef {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Default for Vec3Def {
    fn default() -> Self {
        Self(glam::Vec3::ZERO)
    }
}
impl Default for QuatDef {
    fn default() -> Self {
        Self(glam::Quat::IDENTITY)
    }
}
impl Clone for Vec3Def {
    fn clone(&self) -> Self {
        *self
    }
}
impl Copy for Vec3Def {}
impl Clone for QuatDef {
    fn clone(&self) -> Self {
        *self
    }
}
impl Copy for QuatDef {}
impl PartialEq for Vec3Def {
    fn eq(&self, o: &Self) -> bool {
        self.0 == o.0
    }
}
impl PartialEq for QuatDef {
    fn eq(&self, o: &Self) -> bool {
        self.0 == o.0
    }
}

impl Serialize for Vec3Def {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.to_array().serialize(s)
    }
}
impl<'de> Deserialize<'de> for Vec3Def {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        <[f32; 3]>::deserialize(d).map(|a| Self(glam::Vec3::from_array(a)))
    }
}
impl Serialize for QuatDef {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.to_array().serialize(s)
    }
}
impl<'de> Deserialize<'de> for QuatDef {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        <[f32; 4]>::deserialize(d).map(|a| Self(glam::Quat::from_array(a)))
    }
}
