//! Error types: [`AssetError`] (asset server calls) and [`ImportError`]
//! (importers).

use std::path::PathBuf;

/// Why an asset server call failed. Import failures are not returned here:
/// they put the asset in [`LoadState::Failed`](crate::LoadState::Failed).
#[derive(Debug, thiserror::Error)]
pub enum AssetError {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The reference names no file in the assets folder (by id or path).
    #[error("asset not found: {0}")]
    NotFound(String),
    /// The asset's `.meta` sidecar exists but cannot be used. It is never
    /// overwritten; fix or delete it.
    #[error("{path}.meta: invalid .meta file: {message}")]
    BadMeta { path: String, message: String },
    /// No importer is registered for the file's extension.
    #[error("{0}: no importer is registered for this file type")]
    NoImporter(String),
    /// `load::<T>` for a type that was never registered with
    /// [`AssetServer::register_asset_type`](crate::AssetServer::register_asset_type).
    #[error("asset type {0} is not registered with the asset server")]
    UnregisteredType(&'static str),
    /// The asset is already loaded as a different type.
    #[error("{reference} is loaded as {loaded}, not {requested}")]
    TypeMismatch {
        reference: String,
        loaded: &'static str,
        requested: &'static str,
    },
    /// A path that is absolute, has a drive prefix or leaves the assets
    /// folder through `..`.
    #[error("`{0}` is not a relative path inside the assets folder")]
    OutsideAssets(String),
    #[error("{0} already exists")]
    AlreadyExists(String),
    #[error("file watcher: {0}")]
    Watch(String),
    /// The assets are served from a pak (an exported game): files cannot
    /// be written, created, renamed or watched.
    #[error("{0}: the assets are read-only (served from a pak)")]
    ReadOnly(String),
}

/// Why an import failed. The message is shown in the console and kept in
/// [`LoadState::Failed`](crate::LoadState::Failed).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ImportError {
    /// A file could not be read.
    #[error("{0}")]
    Io(String),
    /// The file is malformed (bad PNG, invalid glTF JSON, bad buffer).
    #[error("{0}")]
    Decode(String),
    /// The file uses a feature this engine does not support (Draco, meshopt,
    /// KTX2/BasisU, quantized attributes).
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// The `.meta` settings are invalid.
    #[error("bad import settings: {0}")]
    Settings(String),
}
