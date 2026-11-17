//! The [`Importer`] trait and the [`ImportContext`] importers work with.

use std::any::{Any, TypeId};
use std::path::Path;
use std::sync::{Arc, RwLock};

use box_core::asset_ref::{AssetId, AssetRef};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value as Json;

use crate::error::ImportError;
use crate::index::{Index, normalize_rel};
use crate::meta::parse_settings;

/// Turns a source file into one or more assets. Implemented by the crate
/// that owns the asset type (box_assets for images, models, materials and
/// scenes; shaders, audio and scripts elsewhere) and registered with
/// [`AssetServer::register_importer`](crate::AssetServer::register_importer).
///
/// `import` runs on a worker thread. It must not panic on bad input (a
/// panic is caught and reported as a failed import, but is still a bug) and
/// must not touch the world: it only adds values to the context.
///
/// ```
/// use box_assets::{ImportContext, ImportError, Importer};
///
/// /// Plain text files as `String` assets.
/// struct TextImporter;
///
/// impl Importer for TextImporter {
///     fn name(&self) -> &str { "text" }
///     fn extensions(&self) -> &[&str] { &["txt"] }
///     fn import(&self, ctx: &mut ImportContext) -> Result<(), ImportError> {
///         let text = String::from_utf8(ctx.bytes().to_vec())
///             .map_err(|e| ImportError::Decode(e.to_string()))?;
///         ctx.add(None, text);
///         Ok(())
///     }
/// }
/// ```
pub trait Importer: Send + Sync + 'static {
    /// Stable name written to `.meta` files (`"texture"`, `"gltf"`).
    fn name(&self) -> &str;

    /// File name suffixes handled, lowercase, without the leading dot. A
    /// suffix may contain dots (`"material.json"`); the longest match wins.
    fn extensions(&self) -> &[&str];

    /// Settings written into new `.meta` files (a JSON object).
    fn default_settings(&self) -> Json {
        Json::Object(Default::default())
    }

    /// Reads [`ImportContext::bytes`] and adds the resulting assets.
    fn import(&self, ctx: &mut ImportContext) -> Result<(), ImportError>;
}

/// One value produced by an import.
pub(crate) struct ImportedAsset {
    pub label: Option<String>,
    pub type_id: TypeId,
    pub type_name: &'static str,
    pub value: Box<dyn Any + Send>,
}

/// What an importer sees: the source bytes and settings, a way to read
/// files it depends on, reference helpers, and the output list.
pub struct ImportContext<'a> {
    pub(crate) id: AssetId,
    pub(crate) path: &'a str,
    pub(crate) bytes: Vec<u8>,
    pub(crate) settings: Json,
    pub(crate) root: &'a Path,
    pub(crate) index: &'a RwLock<Index>,
    pub(crate) outputs: Vec<ImportedAsset>,
    pub(crate) dependencies: Vec<String>,
}

impl<'a> ImportContext<'a> {
    /// The source file's asset id.
    pub fn id(&self) -> AssetId {
        self.id
    }

    /// The source file's path relative to `assets/` (`models/tower.glb`).
    pub fn path(&self) -> &str {
        self.path
    }

    /// The source file's content.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The raw `settings` object from the `.meta` file.
    pub fn settings_json(&self) -> &Json {
        &self.settings
    }

    /// Typed settings with defaults for missing fields. Unknown fields are
    /// ignored with a warning; a wrongly typed field is an error.
    pub fn settings<S: DeserializeOwned + Serialize + Default>(&self) -> Result<S, ImportError> {
        let (s, unknown) = parse_settings::<S>(&self.settings).map_err(ImportError::Settings)?;
        for key in unknown {
            tracing::warn!("{}.meta: unknown setting `{key}` ignored", self.path);
        }
        Ok(s)
    }

    /// `relative` (as written inside the source file, relative to the
    /// source's folder) as a path relative to `assets/`.
    pub fn resolve_relative(&self, relative: &str) -> Result<String, ImportError> {
        let dir = self.path.rsplit_once('/').map_or("", |(d, _)| d);
        let joined = if dir.is_empty() {
            relative.to_string()
        } else {
            format!("{dir}/{relative}")
        };
        normalize_rel(&joined).ok_or_else(|| {
            ImportError::Unsupported(format!("`{relative}` points outside the assets folder"))
        })
    }

    /// Reads another file (path relative to `assets/`) and records it as a
    /// dependency: when it changes, this asset is re-imported.
    pub fn read_dependency(&mut self, assets_path: &str) -> Result<Vec<u8>, ImportError> {
        let path = normalize_rel(assets_path).ok_or_else(|| {
            ImportError::Unsupported(format!("`{assets_path}` is outside the assets folder"))
        })?;
        let bytes = crate::vfs::read(self.root.join(&path))
            .map_err(|e| ImportError::Io(format!("{path}: {e}")));
        if !self.dependencies.contains(&path) {
            self.dependencies.push(path);
        }
        bytes
    }

    /// A reference to another asset file (path relative to `assets/`), with
    /// its id when the file is indexed.
    pub fn asset_ref(&self, assets_path: &str) -> AssetRef {
        let id = self.index.read().ok().and_then(|i| i.id_of(assets_path));
        AssetRef {
            id,
            label: None,
            path: Some(assets_path.to_string()),
        }
    }

    /// True when `assets_path` is an indexed asset file.
    pub fn is_indexed(&self, assets_path: &str) -> bool {
        self.index
            .read()
            .is_ok_and(|i| i.id_of(assets_path).is_some())
    }

    /// A reference to one of this file's own sub-assets.
    pub fn sub_asset_ref(&self, label: &str) -> AssetRef {
        AssetRef::id(self.id).with_label(label).with_path(self.path)
    }

    /// Adds an imported value: the file's main asset when `label` is `None`,
    /// otherwise the sub-asset `label` (`Mesh/Roof`). Adding a label twice
    /// keeps the last value.
    pub fn add<T: Send + 'static>(&mut self, label: Option<String>, value: T) {
        self.outputs.retain(|o| o.label != label);
        self.outputs.push(ImportedAsset {
            label,
            type_id: TypeId::of::<T>(),
            type_name: std::any::type_name::<T>(),
            value: Box::new(value),
        });
    }
}

/// The registered importers.
#[derive(Clone, Default)]
pub(crate) struct ImporterSet {
    importers: Vec<Arc<dyn Importer>>,
}

impl ImporterSet {
    /// Adds an importer; a later importer replaces an earlier one with the
    /// same name.
    pub fn add(&mut self, importer: Arc<dyn Importer>) {
        self.importers.retain(|i| i.name() != importer.name());
        self.importers.push(importer);
    }

    /// The importer for a file name (longest matching suffix).
    pub fn for_path(&self, path: &str) -> Option<&Arc<dyn Importer>> {
        let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
        let mut best: Option<(&Arc<dyn Importer>, usize)> = None;
        for imp in &self.importers {
            for ext in imp.extensions() {
                let matches = name.len() > ext.len() + 1
                    && name.ends_with(ext)
                    && name.as_bytes()[name.len() - ext.len() - 1] == b'.';
                if matches && best.is_none_or(|(_, l)| ext.len() > l) {
                    best = Some((imp, ext.len()));
                }
            }
        }
        best.map(|(i, _)| i)
    }

    pub fn by_name(&self, name: &str) -> Option<&Arc<dyn Importer>> {
        self.importers.iter().find(|i| i.name() == name)
    }
}
