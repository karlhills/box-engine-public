//! Persistent asset identity: [`AssetId`] and [`AssetRef`].
//!
//! An [`AssetId`] is the UUID stored in an asset's `.meta` sidecar; it never
//! changes when the file is renamed or moved. An [`AssetRef`] is what saved
//! data (scene components, material files) stores to point at an asset: the
//! id, an optional sub-asset label (`Mesh/Roof` inside `tower.glb`) and the
//! path as a hint for repair and for people reading the file.
//!
//! JSON forms of an `AssetRef` (all accepted when reading):
//! - `null`: no asset;
//! - `"textures/stone.png"` or `"models/tower.glb#Mesh/Roof"`: by path
//!   (relative to the project's `assets/`), resolved to an id when loaded;
//! - `{"id": "<uuid>", "label": "Mesh/Roof", "path": "models/tower.glb"}`:
//!   `label` and `path` optional. This is the form written back.
//!
//! Resolving references to loaded assets is the asset server's job
//! (`box_assets`); this module only defines the data.

use std::fmt;
use std::str::FromStr;

use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::reflect::{Reflect, ReflectDefault, ReflectDeserialize, ReflectSerialize};

/// The persistent id of an asset (a UUID kept in its `.meta` sidecar).
#[derive(
    Reflect, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[reflect(opaque)]
#[reflect(Clone, Debug, PartialEq, Hash, Serialize, Deserialize)]
#[type_path = "box_engine"]
#[serde(transparent)]
pub struct AssetId(pub uuid::Uuid);

// Not `Default`: a nil id would collide with every other defaulted one.
#[allow(clippy::new_without_default)]
impl AssetId {
    /// A new random (UUID v4) id.
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }

    pub const fn from_u128(v: u128) -> Self {
        Self(uuid::Uuid::from_u128(v))
    }

    pub fn parse(s: &str) -> Option<Self> {
        uuid::Uuid::parse_str(s).ok().map(Self)
    }
}

impl fmt::Display for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.hyphenated().fmt(f)
    }
}

impl FromStr for AssetId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        uuid::Uuid::parse_str(s).map(Self)
    }
}

/// A reference from saved data to an asset or sub-asset. The default value
/// references nothing ([`AssetRef::is_none`]).
#[derive(Reflect, Clone, Debug, Default, PartialEq, Eq, Hash)]
#[reflect(opaque)]
#[reflect(Default, Clone, Debug, PartialEq, Hash, Serialize, Deserialize)]
#[type_path = "box_engine"]
pub struct AssetRef {
    /// The asset's persistent id. `None` until a path-only reference has been
    /// resolved (hand-written or agent-written files).
    pub id: Option<AssetId>,
    /// Sub-asset label inside the source file, such as `Mesh/Roof`,
    /// `Material/Stone` or `Scene`. `None` for the file's main asset.
    pub label: Option<String>,
    /// Source path relative to the project's `assets/` folder, with `/`
    /// separators. A hint: the id wins when both are present.
    pub path: Option<String>,
}

impl AssetRef {
    /// References nothing.
    pub const NONE: Self = Self {
        id: None,
        label: None,
        path: None,
    };

    /// A reference by id.
    pub fn id(id: AssetId) -> Self {
        Self {
            id: Some(id),
            ..Self::NONE
        }
    }

    /// A reference by path, with an optional `#label` suffix
    /// (`models/tower.glb#Mesh/Roof`).
    pub fn path(path: &str) -> Self {
        let (path, label) = match path.split_once('#') {
            Some((p, l)) => (p, Some(l.to_string()).filter(|l| !l.is_empty())),
            None => (path, None),
        };
        Self {
            id: None,
            label,
            path: Some(path.replace('\\', "/")).filter(|p| !p.is_empty()),
        }
    }

    /// The same asset with a sub-asset label.
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// The same reference with a path hint.
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// True when it references nothing.
    pub fn is_none(&self) -> bool {
        self.id.is_none() && self.path.is_none()
    }

    pub fn is_some(&self) -> bool {
        !self.is_none()
    }
}

impl fmt::Display for AssetRef {
    /// `path#label`, or the id when there is no path, or `none`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.path, &self.id) {
            (Some(p), _) => f.write_str(p)?,
            (None, Some(id)) => write!(f, "{id}")?,
            (None, None) => return f.write_str("none"),
        }
        if let Some(l) = &self.label {
            write!(f, "#{l}")?;
        }
        Ok(())
    }
}

impl Serialize for AssetRef {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        if self.is_none() {
            return serializer.serialize_none();
        }
        let len = [self.id.is_some(), self.label.is_some(), self.path.is_some()]
            .iter()
            .filter(|b| **b)
            .count();
        let mut map = serializer.serialize_map(Some(len))?;
        if let Some(id) = &self.id {
            map.serialize_entry("id", id)?;
        }
        if let Some(label) = &self.label {
            map.serialize_entry("label", label)?;
        }
        if let Some(path) = &self.path {
            map.serialize_entry("path", path)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for AssetRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct RefVisitor;

        impl<'de> Visitor<'de> for RefVisitor {
            type Value = AssetRef;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("null, a path string, or { id?, label?, path? }")
            }

            fn visit_unit<E: de::Error>(self) -> Result<AssetRef, E> {
                Ok(AssetRef::NONE)
            }

            fn visit_none<E: de::Error>(self) -> Result<AssetRef, E> {
                Ok(AssetRef::NONE)
            }

            fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<AssetRef, D::Error> {
                d.deserialize_any(RefVisitor)
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<AssetRef, E> {
                Ok(AssetRef::path(v))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<AssetRef, A::Error> {
                let mut r = AssetRef::NONE;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "id" => {
                            let s = map.next_value::<String>()?;
                            r.id = Some(AssetId::parse(&s).ok_or_else(|| {
                                de::Error::invalid_value(de::Unexpected::Str(&s), &"a UUID")
                            })?);
                        }
                        "label" => r.label = map.next_value::<Option<String>>()?,
                        "path" => r.path = map.next_value::<Option<String>>()?,
                        other => {
                            return Err(de::Error::unknown_field(other, &["id", "label", "path"]));
                        }
                    }
                }
                Ok(r)
            }
        }

        deserializer.deserialize_any(RefVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_forms_round_trip() {
        let id = AssetId::from_u128(7);
        let full = AssetRef::id(id)
            .with_label("Mesh/Roof")
            .with_path("models/tower.glb");
        let json = serde_json::to_value(&full).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "id": "00000000-0000-0000-0000-000000000007",
                "label": "Mesh/Roof",
                "path": "models/tower.glb"
            })
        );
        assert_eq!(serde_json::from_value::<AssetRef>(json).unwrap(), full);

        let none: AssetRef = serde_json::from_str("null").unwrap();
        assert!(none.is_none());
        assert_eq!(serde_json::to_string(&none).unwrap(), "null");

        let by_path: AssetRef = serde_json::from_str(r#""models/tower.glb#Mesh/Roof""#).unwrap();
        assert_eq!(by_path.path.as_deref(), Some("models/tower.glb"));
        assert_eq!(by_path.label.as_deref(), Some("Mesh/Roof"));
        assert_eq!(by_path.id, None);
        assert_eq!(by_path.to_string(), "models/tower.glb#Mesh/Roof");

        assert!(serde_json::from_str::<AssetRef>(r#"{"id": "nope"}"#).is_err());
        assert!(serde_json::from_str::<AssetRef>(r#"{"file": "x"}"#).is_err());
    }

    #[test]
    fn reflected_values_use_the_compact_forms() {
        let registry = crate::reflect::Registry::with_core_types();
        let r = AssetRef::path("textures/stone.png");
        let json = registry.value_to_json(&r).unwrap();
        assert_eq!(json, serde_json::json!({"path": "textures/stone.png"}));
        let none = registry.value_to_json(&AssetRef::NONE).unwrap();
        assert_eq!(none, serde_json::Value::Null);
    }
}
