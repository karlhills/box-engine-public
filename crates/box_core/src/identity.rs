//! Persistent identity: [`Uid`] and [`Name`].

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::reflect::{Hidden, Reflect, ReflectComponent, ReflectDefault};

/// Persistent entity identity, stable across save, load, Play and undo.
///
/// Required on entities that are saved in a scene or shown in the editor; the
/// [`World`](crate::world::World) keeps a `Uid -> Entity` index. Scene files
/// write it as the entity's `"uid"` key, never inside `"components"`.
#[derive(
    Reflect, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[reflect(Component, Clone, Debug, PartialEq, Hash, @Hidden)]
#[type_path = "box_engine"]
#[serde(transparent)]
pub struct Uid(pub uuid::Uuid);

// Not `Default`: a defaulted (nil) uid would collide with every other one.
#[allow(clippy::new_without_default)]
impl Uid {
    /// A new time-ordered (UUID v7) id.
    pub fn new() -> Self {
        Self(uuid::Uuid::now_v7())
    }

    /// A new random (UUID v4) id.
    pub fn new_random() -> Self {
        Self(uuid::Uuid::new_v4())
    }

    pub const fn from_u128(v: u128) -> Self {
        Self(uuid::Uuid::from_u128(v))
    }

    pub fn parse(s: &str) -> Option<Self> {
        uuid::Uuid::parse_str(s).ok().map(Self)
    }
}

impl fmt::Display for Uid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.hyphenated().fmt(f)
    }
}

impl FromStr for Uid {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        uuid::Uuid::parse_str(s).map(Self)
    }
}

/// Display name, shown in the hierarchy. Scene files write it as the entity's
/// `"name"` key. Renaming is a field edit of `Name`.
#[derive(Reflect, Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[reflect(Component, Default, Clone, Debug, PartialEq, Hash)]
#[type_path = "box_engine"]
pub struct Name(pub String);

impl Name {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for Name {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for Name {
    fn from(s: String) -> Self {
        Self(s)
    }
}
