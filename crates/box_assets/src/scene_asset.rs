//! [`SceneAsset`]: a scene or prefab as an asset.

use box_core::scene::{SCENE_VERSION, SceneError};
use serde_json::Value as Json;

use crate::error::ImportError;

/// A scene in box_core's scene file form (`{"version", "resources"?,
/// "entities"}`), kept as the parsed JSON tree rather than a `World`:
/// component types may belong to a game whose registry the asset server does
/// not know, and a prefab is instantiated many times.
///
/// Produced by the `scene` importer (`.scene.json` files) and by the glTF
/// importer (`tower.glb#Scene`). To use it:
/// - a whole world: `box_core::scene::load_value(&asset.json, registry)`;
/// - an instance inside a world: clone [`entities`](Self::entities), give
///   them fresh uids with `box_core::scene::remap_uids`, then
///   `box_core::scene::instantiate(world, &records, placement)`.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneAsset {
    pub json: Json,
}

impl SceneAsset {
    /// A scene with no entities (the placeholder of a failed scene).
    pub fn empty() -> Self {
        Self {
            json: serde_json::json!({ "version": SCENE_VERSION, "entities": [] }),
        }
    }

    /// Checks the version and the shape (`entities` array, `resources`
    /// object when present) of a parsed scene file. An older format version
    /// is upgraded in memory ([`box_core::scene::upgrade_format`]); the
    /// component values are upgraded by their types' steps when the scene is
    /// loaded or instantiated with a registry.
    pub fn from_json(mut json: Json) -> Result<Self, ImportError> {
        let report = &mut box_core::migrate::TreeReport::default();
        box_core::scene::upgrade_format(&mut json, report).map_err(|e| match e {
            SceneError::NewerVersion { .. } => ImportError::Unsupported(e.to_string()),
            other => ImportError::Decode(other.to_string()),
        })?;
        if !json.get("entities").is_some_and(Json::is_array) {
            return Err(ImportError::Decode("missing \"entities\" array".into()));
        }
        if json
            .get("resources")
            .is_some_and(|r| !r.is_object() && !r.is_null())
        {
            return Err(ImportError::Decode(
                "\"resources\" must be an object".into(),
            ));
        }
        Ok(Self { json })
    }

    /// The entity records, parents before children.
    pub fn entities(&self) -> &[Json] {
        self.json
            .get("entities")
            .and_then(Json::as_array)
            .map_or(&[], Vec::as_slice)
    }
}
