//! `.meta` sidecars and typed import settings.
//!
//! Every file under `assets/` that has an importer gets a `<file>.meta` next
//! to it, created the first time the asset server sees the file:
//!
//! ```json
//! {
//!   "version": 1,
//!   "id": "0193a4f2-7c1e-4b3a-9d2e-5f0a1c2b3d4e",
//!   "importer": "texture",
//!   "settings": { "color_space": "srgb", "mipmaps": true }
//! }
//! ```
//!
//! `settings` belongs to the importer named by `importer`. Missing settings
//! take their defaults (nothing is written back); unknown settings are
//! ignored with a warning. The asset server writes a `.meta` only when it
//! creates a missing one, when it gives a copied file a fresh id, when it
//! renames an asset, and in `AssetServer::set_settings` (the editor's import
//! settings).

use std::path::{Path, PathBuf};

use box_core::asset_ref::AssetId;
use box_core::image::ColorSpace;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

/// The `.meta` format version this engine writes and reads.
pub const META_VERSION: u32 = 1;
/// The `.meta` format's migration chain (step *i* upgrades `"version"`
/// *i + 1* to *i + 2*). Empty: the format has not changed yet.
pub fn format_chain() -> box_core::migrate::Chain {
    box_core::migrate::Chain::new(Vec::new())
}

/// Suffix appended to an asset's file name to get its sidecar.
pub const META_SUFFIX: &str = ".meta";

/// The content of a `.meta` sidecar.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Meta {
    pub version: u32,
    /// The asset's persistent id.
    pub id: AssetId,
    /// Name of the importer ([`Importer::name`](crate::Importer::name)).
    pub importer: String,
    /// Importer settings (a JSON object).
    #[serde(default = "empty_object")]
    pub settings: Json,
}

fn empty_object() -> Json {
    Json::Object(Default::default())
}

impl Meta {
    /// A new sidecar with a fresh id.
    pub fn new(importer: &str, settings: Json) -> Self {
        Self {
            version: META_VERSION,
            id: AssetId::new(),
            importer: importer.to_string(),
            settings,
        }
    }

    /// Parses and checks a sidecar.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let meta: Meta = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if meta.version > META_VERSION {
            return Err(format!(
                "format version {} is newer than this engine supports ({META_VERSION}); update box-engine",
                meta.version
            ));
        }
        if !meta.settings.is_object() {
            return Err("\"settings\" must be an object".into());
        }
        Ok(meta)
    }

    /// Pretty JSON with a trailing newline.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = serde_json::to_vec_pretty(self).unwrap_or_default();
        out.push(b'\n');
        out
    }
}

/// The sidecar path of `asset`.
pub fn meta_path(asset: &Path) -> PathBuf {
    let mut s = asset.as_os_str().to_owned();
    s.push(META_SUFFIX);
    PathBuf::from(s)
}

/// Reads `asset`'s sidecar. `Ok(None)` when there is none.
pub(crate) fn read_meta(asset: &Path) -> Result<Option<Meta>, String> {
    match crate::vfs::read(meta_path(asset)) {
        Ok(bytes) => Meta::parse(&bytes).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Writes `asset`'s sidecar through a hidden temporary file and a rename,
/// so readers (and the file watcher) never see a half-written file.
pub(crate) fn write_meta(asset: &Path, meta: &Meta) -> std::io::Result<()> {
    let path = meta_path(asset);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.tmp"));
    std::fs::write(&tmp, meta.to_bytes())?;
    std::fs::rename(&tmp, &path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Parses importer settings: missing fields take `S::default()`'s values.
/// Returns the settings and the names of unknown fields (ignored).
pub fn parse_settings<S>(settings: &Json) -> Result<(S, Vec<String>), String>
where
    S: DeserializeOwned + Serialize + Default,
{
    let defaults = serde_json::to_value(S::default()).map_err(|e| e.to_string())?;
    let (Json::Object(defaults), Json::Object(given)) = (defaults, settings) else {
        return Err("settings must be an object".into());
    };
    let unknown = given
        .keys()
        .filter(|k| !defaults.contains_key(*k))
        .cloned()
        .collect();
    let mut merged = defaults;
    for (k, v) in given {
        if merged.contains_key(k) {
            merged.insert(k.clone(), v.clone());
        }
    }
    let s = serde_json::from_value(Json::Object(merged)).map_err(|e| e.to_string())?;
    Ok((s, unknown))
}

/// Colour space setting of a texture (`"srgb"` or `"linear"`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorSpaceSetting {
    /// Colour data: base colour, emissive, UI.
    #[default]
    Srgb,
    /// Non-colour data: normal maps, ORM, masks.
    Linear,
}

impl From<ColorSpaceSetting> for ColorSpace {
    fn from(c: ColorSpaceSetting) -> Self {
        match c {
            ColorSpaceSetting::Srgb => ColorSpace::Srgb,
            ColorSpaceSetting::Linear => ColorSpace::Linear,
        }
    }
}

/// Settings of the `texture` importer (PNG, JPEG).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TextureSettings {
    /// How the pixel values are encoded. Default `srgb`; set `linear` for
    /// normal maps and ORM textures.
    pub color_space: ColorSpaceSetting,
    /// Generate the full mip chain at import (box filter, in linear light
    /// for sRGB). Default `true`.
    pub mipmaps: bool,
}

impl Default for TextureSettings {
    fn default() -> Self {
        Self {
            color_space: ColorSpaceSetting::Srgb,
            mipmaps: true,
        }
    }
}

/// Settings of the `gltf` importer (`.gltf`, `.glb`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModelSettings {
    /// Uniform scale applied to vertex positions and node translations
    /// (0.01 for files authored in centimetres). Must be finite and > 0.
    /// Default `1.0`.
    pub scale: f32,
    /// Generate MikkTSpace tangents for meshes that have normals and UVs but
    /// no tangents. Default `true`.
    pub generate_tangents: bool,
    /// Give every mesh node of the `Scene` sub-asset (skinned meshes and
    /// nodes named `*-nocol` excepted) a static triangle-mesh `Collider`
    /// (`shape: Mesh`), so an imported level collides as it is drawn.
    /// Default `false`.
    pub colliders: bool,
}

impl Default for ModelSettings {
    fn default() -> Self {
        Self {
            scale: 1.0,
            generate_tangents: true,
            colliders: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_fill_defaults_and_report_unknown_fields() {
        let (s, unknown) =
            parse_settings::<TextureSettings>(&serde_json::json!({"color_space": "linear"}))
                .unwrap();
        assert_eq!(s.color_space, ColorSpaceSetting::Linear);
        assert!(s.mipmaps);
        assert!(unknown.is_empty());
        let (s, unknown) =
            parse_settings::<ModelSettings>(&serde_json::json!({"compress": "auto"})).unwrap();
        assert_eq!(s, ModelSettings::default());
        assert_eq!(unknown, vec!["compress".to_string()]);
        assert!(parse_settings::<TextureSettings>(&serde_json::json!({"mipmaps": "yes"})).is_err());
        assert!(parse_settings::<TextureSettings>(&serde_json::json!([1])).is_err());
    }

    #[test]
    fn meta_round_trips_and_refuses_newer_versions() {
        let m = Meta::new(
            "texture",
            serde_json::to_value(TextureSettings::default()).unwrap(),
        );
        assert_eq!(Meta::parse(&m.to_bytes()).unwrap(), m);
        let newer = serde_json::json!({"version": 99, "id": m.id, "importer": "texture"});
        let err = Meta::parse(newer.to_string().as_bytes()).unwrap_err();
        assert!(err.contains("newer"), "{err}");
        assert!(Meta::parse(b"{").is_err());
        let extra = serde_json::json!({"version": 1, "id": m.id, "importer": "x", "foo": 1});
        assert!(Meta::parse(extra.to_string().as_bytes()).is_err());
    }
}
